//! 覆盖式更新：解压到同盘 staging → 验证 → 原子改名交换。
//!
//! 目录路径保持不变（用户决策）。交换窗口内原安装不可用，通常 < 2 秒。

use crate::buildinfo;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use zip::ZipArchive;

pub struct InstallResult {
    pub install_path: String,
    pub old_version: String,
    pub new_version: String,
    pub elapsed: f64,
}

/// staging 根：与安装目录同盘，保证 rename 是元数据操作
pub fn staging_root(install: &Path) -> PathBuf {
    let parent = install.parent().unwrap_or(Path::new("."));
    parent.join(".blender-updater").join("staging")
}

fn old_path_of(install: &Path) -> PathBuf {
    PathBuf::from(format!("{}.old", install.display()))
}

fn rm_rf(p: &Path) {
    if p.is_dir() {
        let _ = std::fs::remove_dir_all(p);
    } else if p.is_file() {
        let _ = std::fs::remove_file(p);
    }
}

/// 检查 zip 是否只有单一顶层目录，返回该目录名
fn zip_top_prefix(zip_path: &Path) -> Result<String, String> {
    let file = File::open(zip_path).map_err(|e| format!("打开压缩包失败：{}", e))?;
    let archive = ZipArchive::new(file).map_err(|e| format!("读取压缩包失败：{}", e))?;
    let mut top: Option<String> = None;
    for i in 0..archive.len().min(2000) {
        let Some(name) = archive.name_for_index(i) else { continue };
        let first = name.split('/').next().unwrap_or("");
        if first.is_empty() || first == "." {
            continue;
        }
        match &top {
            None => top = Some(first.to_string()),
            Some(t) if t == first => {}
            Some(t) => {
                return Err(format!("压缩包结构异常：包含多个顶层目录（{} / {}）", t, first));
            }
        }
    }
    top.ok_or_else(|| "压缩包为空".to_string())
}

/// 解压（剥掉顶层目录），返回文件数
fn extract(zip_path: &Path, dest: &Path, cancel: &AtomicBool, mut tick: impl FnMut(usize, usize)) -> Result<usize, String> {
    let file = File::open(zip_path).map_err(|e| format!("打开压缩包失败：{}", e))?;
    let mut archive = ZipArchive::new(file).map_err(|e| format!("读取压缩包失败：{}", e))?;
    let total = archive.len();

    for i in 0..total {
        if cancel.load(Ordering::Relaxed) {
            return Err("已取消".into());
        }
        let mut entry = archive.by_index(i).map_err(|e| format!("读取第 {} 项失败：{}", i, e))?;
        let Some(rel) = entry.enclosed_name() else { continue };

        let comps: Vec<_> = rel.components().collect();
        if comps.len() < 2 {
            continue; // 顶层目录条目本身
        }
        let stripped: PathBuf = comps[1..].iter().collect();
        if stripped.as_os_str().is_empty() {
            continue;
        }
        let out = dest.join(&stripped);

        if entry.is_dir() {
            let _ = std::fs::create_dir_all(&out);
            continue;
        }
        if let Some(parent) = out.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut f = File::create(&out).map_err(|e| format!("写入 {} 失败：{}", out.display(), e))?;
        std::io::copy(&mut entry, &mut f).map_err(|e| format!("解压 {} 失败：{}", out.display(), e))?;
        let _ = f.flush();

        if i % 150 == 0 {
            tick(i, total);
        }
    }
    tick(total, total);
    Ok(total)
}

/// 完整更新流程
pub fn update_from_zip(
    zip_path: &Path,
    install: &Path,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(&str, f64, &str, &str),
) -> Result<InstallResult, String> {
    let started = Instant::now();
    let install = install
        .canonicalize()
        .map_err(|e| format!("安装目录不可访问：{}", e))?;

    // ── 1. 预检 ──
    on_progress("precheck", 1.0, "准备更新", "检查安装目录…");
    if !buildinfo::is_install_root(&install) {
        return Err("目标目录不是 Blender 安装（找不到 blender.exe）".into());
    }
    let old_info = buildinfo::read_install(&install).unwrap_or_default();
    let old_version = old_info.version.clone();

    let top = zip_top_prefix(zip_path)?;
    on_progress("precheck", 4.0, "准备更新", &format!("压缩包内容：{}", top));

    // ── 2. 解压到同盘 staging ──
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let staging_base = staging_root(&install).join(&stamp);
    let unpacked = staging_base.join("unpacked");
    let _ = std::fs::create_dir_all(&unpacked);

    on_progress("extract", 6.0, "正在解压", "这需要一点时间…");
    let files = extract(zip_path, &unpacked, cancel, |done, total| {
        let pct = 6.0 + (done as f64 / total.max(1) as f64) * 78.0;
        on_progress("extract", pct, "正在解压", &format!("{}/{} 项", done, total));
    })?;
    on_progress("extract", 84.0, "解压完成", &format!("共 {} 项", files));

    // ── 3. 验证新版本 ──
    on_progress("verify", 86.0, "校验新版本", "读取版本信息…");
    let new_info = buildinfo::read_install(&unpacked).ok_or_else(|| "解压结果里找不到 blender.exe".to_string())?;
    if new_info.version.is_empty() {
        return Err("无法识别新版本号".into());
    }
    on_progress("verify", 89.0, "校验新版本", &format!("{} → {}", old_version, new_info.version));

    // ── 4. 把 portable 搬进新目录，再做原子交换 ──
    on_progress("swap", 91.0, "正在切换目录", "迁移用户配置…");
    let portable = install.join("portable");
    let mut moved_portable = false;
    if portable.is_dir() {
        let dst = unpacked.join("portable");
        if dst.exists() {
            rm_rf(&dst);
        }
        std::fs::rename(&portable, &dst).map_err(|e| format!("迁移 portable 失败：{}", e))?;
        moved_portable = true;
    }

    let old_bak = old_path_of(&install);
    if old_bak.exists() {
        rm_rf(&old_bak);
    }

    on_progress("swap", 93.0, "正在切换目录", "原目录改名…");
    if let Err(e) = std::fs::rename(&install, &old_bak) {
        // 回滚 portable
        if moved_portable {
            let _ = std::fs::rename(&unpacked.join("portable"), &portable);
        }
        return Err(format!("无法重命名原目录（Blender 是否正在运行？）：{}", e));
    }

    on_progress("swap", 95.0, "正在切换目录", "新目录就位…");
    if let Err(e) = std::fs::rename(&unpacked, &install) {
        let _ = std::fs::rename(&old_bak, &install);
        return Err(format!("切换失败，已回滚：{}", e));
    }

    // ── 5. 复查 ──
    let check = buildinfo::read_install(&install);
    if check.is_none() {
        // 严重异常：回滚
        let _ = std::fs::rename(&install, &unpacked);
        let _ = std::fs::rename(&old_bak, &install);
        return Err("新目录校验失败，已回滚到原版本".into());
    }

    // ── 6. 旧目录挪进隐藏的 staging，再后台删除 ──
    // 同盘 rename 是元数据操作，瞬间完成：安装目录旁边不会长期留 xxx.old
    on_progress("cleanup", 98.0, "清理旧文件", "后台删除旧版本…");
    let trash = staging_base.join("old");
    let relocated = std::fs::rename(&old_bak, &trash).is_ok();
    let trash_for_thread = if relocated { trash.clone() } else { old_bak.clone() };
    let sb = staging_base.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(600));
        rm_rf(&trash_for_thread);
        std::thread::sleep(std::time::Duration::from_millis(300));
        rm_rf(&sb);
    });

    on_progress("done", 100.0, "更新完成", &format!("已更新到 {}", new_info.version));

    Ok(InstallResult {
        install_path: install.to_string_lossy().to_string(),
        old_version,
        new_version: new_info.version,
        elapsed: started.elapsed().as_secs_f64(),
    })
}

/// 启动时清理上次残留的旧目录（xxx.old）和 staging 残留。
/// 返回清理掉的旧目录数量。
pub fn cleanup_leftovers() -> usize {
    let cfg = crate::config::load();
    let mut removed = 0usize;
    for p in &cfg.installs {
        let install = Path::new(p);
        // 安装目录旁边可能残留 xxx.old（更新过程中断）
        let bak = old_path_of(install);
        if bak.exists() {
            rm_rf(&bak);
            removed += 1;
        }
        // staging 下按时间戳分的子目录（含挪进去的 old）一并清掉
        let st = staging_root(install);
        if let Ok(rd) = std::fs::read_dir(&st) {
            for e in rd.flatten() {
                rm_rf(&e.path());
            }
        }
    }

    // 盘符根目录下孤立的 .blender-updater 也清掉
    for c in b'C'..=b'Z' {
        let root = PathBuf::from(format!("{}:\\", c as char));
        if !root.is_dir() {
            continue;
        }
        let stray = root.join(".blender-updater");
        if stray.is_dir() {
            rm_rf(&stray);
        }
    }
    removed
}
