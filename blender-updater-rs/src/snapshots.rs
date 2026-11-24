//! 配置快照：只备份安装根下的 portable/config（用户偏好、启动文件、插件设置等）。
//!
//! 备份根可以通过设置修改（默认 %APPDATA%\BlenderUpdater\snapshots），
//! 修改时调用 migrate_to 把已有快照整体搬过去，保证位置唯一、不残留。

use crate::buildinfo;
use crate::config;
use crate::model::{MigrateReport, SnapshotInfo};
use sha1::{Digest, Sha1};
use std::fs;
use std::path::{Path, PathBuf};

pub const FILES_DIR: &str = "files";
pub const META_NAME: &str = "meta.json";

pub fn snapshot_root() -> PathBuf {
    config::snapshot_root()
}

/// 安装路径 -> 稳定的短目录名（小写、去尾分隔符后取 sha1 前 12 位）
pub fn install_key(install: &Path) -> String {
    let raw = install.to_string_lossy().to_lowercase();
    let trimmed = raw.trim_end_matches('\\');
    let mut h = Sha1::new();
    h.update(trimmed.as_bytes());
    let full = hex::encode(h.finalize());
    full[..12].to_string()
}

pub fn snap_dir(install: &Path) -> PathBuf {
    snapshot_root().join(install_key(install))
}

fn copy_dir(src: &Path, dst: &Path) -> Result<(u64, u64), String> {
    fs::create_dir_all(dst).map_err(|e| format!("创建目录失败：{}", e))?;
    let mut files = 0u64;
    let mut bytes = 0u64;
    let entries = fs::read_dir(src).map_err(|e| format!("读取目录失败：{}", e))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("读取目录项失败：{}", e))?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let meta = entry.metadata().map_err(|e| format!("读取元数据失败：{}", e))?;
        if meta.is_dir() {
            let (f, b) = copy_dir(&from, &to)?;
            files += f;
            bytes += b;
        } else {
            fs::copy(&from, &to).map_err(|e| format!("复制 {} 失败：{}", from.display(), e))?;
            files += 1;
            bytes += meta.len();
        }
    }
    Ok((files, bytes))
}

fn read_meta(dir: &Path) -> Option<SnapshotInfo> {
    let text = fs::read_to_string(dir.join(META_NAME)).ok()?;
    serde_json::from_str::<SnapshotInfo>(&text).ok()
}

/// 为某安装创建一份配置快照。没有 portable/config 时返回 Ok(None)。
pub fn create(install: &Path, reason: &str, note: &str) -> Result<Option<SnapshotInfo>, String> {
    let cfg_dir = install.join("portable").join("config");
    if !cfg_dir.is_dir() {
        return Ok(None);
    }
    let root = snap_dir(install);
    fs::create_dir_all(&root).map_err(|e| format!("创建快照目录失败：{}", e))?;

    let now = chrono::Local::now();
    let mut id = now.format("%Y%m%d-%H%M%S").to_string();
    let mut n = 2;
    while root.join(&id).exists() {
        id = format!("{}-{}", now.format("%Y%m%d-%H%M%S"), n);
        n += 1;
        if n > 99 {
            return Err("同一秒内创建的快照过多".into());
        }
    }

    let dest = root.join(&id);
    let (file_count, bytes) = copy_dir(&cfg_dir, &dest.join(FILES_DIR))?;

    let info = buildinfo::read_install(install).unwrap_or_default();
    let snap = SnapshotInfo {
        id: id.clone(),
        created_at: now.format("%Y-%m-%d %H:%M:%S").to_string(),
        reason: reason.to_string(),
        note: note.to_string(),
        blender_version: info.version.clone(),
        build_hash: info.build_hash.clone(),
        source_path: cfg_dir.display().to_string(),
        file_count,
        bytes,
        locked: reason == "manual",
    };

    let meta = serde_json::to_string_pretty(&snap).map_err(|e| format!("序列化失败：{}", e))?;
    fs::write(dest.join(META_NAME), meta).map_err(|e| format!("写入元数据失败：{}", e))?;
    Ok(Some(snap))
}

/// 全部快照（按创建时间倒序）
pub fn list(install: &Path) -> Vec<SnapshotInfo> {
    let root = snap_dir(install);
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(&root) {
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            if let Some(info) = read_meta(&entry.path()) {
                out.push(info);
            }
        }
    }
    out.sort_by(|a, b| b.created_at.cmp(&a.created_at).then_with(|| b.id.cmp(&a.id)));
    out
}

pub fn delete(install: &Path, id: &str) -> Result<(), String> {
    let dir = snap_dir(install).join(id);
    if !dir.is_dir() {
        return Err("该快照不存在".into());
    }
    fs::remove_dir_all(&dir).map_err(|e| format!("删除失败：{}", e))
}

/// 恢复：先把当前配置存成 before_restore 快照，再覆盖回去；失败则回滚。
pub fn restore(install: &Path, id: &str) -> Result<(), String> {
    let root = snap_dir(install);
    let src = root.join(id);
    if !src.is_dir() {
        return Err("该快照不存在".into());
    }
    let cfg_dir = install.join("portable").join("config");
    let parent = install.join("portable");
    fs::create_dir_all(&parent).map_err(|e| format!("创建 portable 失败：{}", e))?;

    if cfg_dir.exists() {
        create(install, "before_restore", id)?;
    }

    let tmp = parent.join("config.restoring");
    let _ = fs::remove_dir_all(&tmp);
    let old = parent.join("config.old");
    let _ = fs::remove_dir_all(&old);

    if cfg_dir.exists() {
        fs::rename(&cfg_dir, &old).map_err(|e| format!("暂存当前配置失败：{}", e))?;
    }
    match copy_dir(&src.join(FILES_DIR), &tmp) {
        Ok(_) => {
            fs::rename(&tmp, &cfg_dir).map_err(|e| format!("写入新配置失败：{}", e))?;
            let _ = fs::remove_dir_all(&old);
            Ok(())
        }
        Err(e) => {
            let _ = fs::remove_dir_all(&tmp);
            if old.exists() {
                let _ = fs::rename(&old, &cfg_dir);
            }
            Err(format!("恢复失败：{}", e))
        }
    }
}

/// 自动备份保留份数：只淘汰未锁定的（手动快照永久保留）
pub fn prune(install: &Path, keep: u32) {
    let keep = keep.max(1) as usize;
    let all = list(install);
    let mut auto: Vec<&SnapshotInfo> = all.iter().filter(|s| !s.locked).collect();
    if auto.len() <= keep {
        return;
    }
    auto.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    for s in auto.into_iter().skip(keep) {
        let _ = delete(install, &s.id);
    }
}

fn move_one(src: &Path, dst: &Path, rep: &mut MigrateReport) {
    if dst.exists() {
        rep.failed += 1;
        return;
    }
    if fs::rename(src, dst).is_ok() {
        rep.moved += 1;
        return;
    }
    // 跨盘 rename 失败时退化为复制 + 删除
    let copied: Result<(), String> = if src.is_dir() {
        copy_dir(src, dst).map(|_| ())
    } else {
        fs::copy(src, dst).map(|_| ()).map_err(|e| e.to_string())
    };
    match copied {
        Ok(_) => {
            let _ = if src.is_dir() {
                fs::remove_dir_all(src)
            } else {
                fs::remove_file(src)
            };
            rep.moved += 1;
        }
        Err(_) => rep.failed += 1,
    }
}

/// 把快照库整体搬到新位置，并写入配置。目标已存在时逐个合并，同名不覆盖。
pub fn migrate_to(new_root: &Path) -> Result<MigrateReport, String> {
    let old_root = snapshot_root();
    let mut rep = MigrateReport {
        moved: 0,
        failed: 0,
        from: old_root.display().to_string(),
        to: new_root.display().to_string(),
    };
    fs::create_dir_all(new_root).map_err(|e| format!("创建目标目录失败：{}", e))?;

    if old_root != new_root && old_root.is_dir() {
        if let Ok(entries) = fs::read_dir(&old_root) {
            for entry in entries.flatten() {
                let src_key = entry.path();
                let dst_key = new_root.join(entry.file_name());
                if !src_key.is_dir() {
                    let _ = fs::create_dir_all(&dst_key);
                    move_one(&src_key, &dst_key, &mut rep);
                    continue;
                }
                let _ = fs::create_dir_all(&dst_key);
                if let Ok(snaps) = fs::read_dir(&src_key) {
                    for s in snaps.flatten() {
                        let dst = dst_key.join(s.file_name());
                        move_one(&s.path(), &dst, &mut rep);
                    }
                }
                if fs::read_dir(&src_key).map(|mut i| i.next().is_none()).unwrap_or(false) {
                    let _ = fs::remove_dir(&src_key);
                }
            }
        }
        if fs::read_dir(&old_root).map(|mut i| i.next().is_none()).unwrap_or(false) {
            let _ = fs::remove_dir(&old_root);
        }
    }

    let mut cfg = config::load();
    cfg.snapshot_dir = Some(new_root.display().to_string());
    config::save(&cfg);
    Ok(rep)
}

/// 快照库总占用（字节），给设置界面显示
pub fn total_bytes() -> u64 {
    fn walk(p: &Path) -> u64 {
        let mut sum = 0;
        if let Ok(entries) = fs::read_dir(p) {
            for e in entries.flatten() {
                let meta = match e.metadata() {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                if meta.is_dir() {
                    sum += walk(&e.path());
                } else {
                    sum += meta.len();
                }
            }
        }
        sum
    }
    let root = snapshot_root();
    if root.is_dir() {
        walk(&root)
    } else {
        0
    }
}
