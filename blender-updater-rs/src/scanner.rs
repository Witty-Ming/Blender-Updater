//! 发现本机的 Blender 安装。

use crate::buildinfo;
use crate::config;
use crate::model::{InstallInfo, StatusBadge};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// 所有存在的盘符根
fn drive_roots() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for c in b'C'..=b'Z' {
        let p = PathBuf::from(format!("{}:\\", c as char));
        if p.is_dir() {
            out.push(p);
        }
    }
    out
}

/// 在某个根目录下扫描一层，找 Blender 安装
fn scan_root(root: &Path, found: &mut BTreeSet<String>) {
    let Ok(rd) = std::fs::read_dir(root) else { return };
    for e in rd.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        if !ft.is_dir() {
            continue;
        }
        let name = e.file_name().to_string_lossy().to_lowercase();
        // 只关心名字里带 blender 的目录，避免全盘遍历
        if !name.contains("blender") {
            continue;
        }
        // 更新过程中产生的 xxx.old 是待删除的旧目录，不算安装
        if name.ends_with(".old") {
            continue;
        }
        let p = e.path();
        if buildinfo::is_install_root(&p) {
            found.insert(p.to_string_lossy().to_string());
        }
    }
}

/// 额外检查标准安装位置（Program Files 里的 Blender Foundation）
fn scan_standard_locations(found: &mut BTreeSet<String>) {
    let roots = [
        PathBuf::from("C:\\Program Files\\Blender Foundation"),
        PathBuf::from("C:\\Program Files (x86)\\Blender Foundation"),
    ];
    for r in roots {
        let Ok(rd) = std::fs::read_dir(&r) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let sub = p.join("Blender");
            for cand in [&p, &sub] {
                if buildinfo::is_install_root(cand) {
                    found.insert(cand.to_string_lossy().to_string());
                }
            }
        }
    }
    if let Some(local) = dirs::data_local_dir() {
        let programs = local.join("Programs");
        let Ok(rd) = std::fs::read_dir(&programs) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if buildinfo::is_install_root(&p) {
                found.insert(p.to_string_lossy().to_string());
            }
        }
    }
}

fn to_install_info(path: &str) -> Option<InstallInfo> {
    let dir = PathBuf::from(path);
    let info = buildinfo::read_install(&dir)?;
    let name = dir
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string());
    Some(InstallInfo {
        path: path.to_string(),
        name,
        version: info.version,
        version_num: info.version_num,
        series: info.series,
        build_hash: info.build_hash,
        branch: info.branch,
        is_portable: dir.join("portable").is_dir(),
        has_config: buildinfo::has_portable_config(&dir),
        status_badge: StatusBadge::none(),
    })
}

/// 扫描全部安装：配置里记录的 + 盘符根目录一层 + 标准位置
pub fn scan_all() -> Vec<InstallInfo> {
    let cfg = config::load();
    let mut found: BTreeSet<String> = BTreeSet::new();

    for p in &cfg.installs {
        if p.to_lowercase().ends_with(".old") {
            continue;
        }
        if buildinfo::is_install_root(Path::new(p)) {
            found.insert(p.clone());
        }
    }

    // 盘符根目录一层扫描（只认名字含 blender 的目录）
    for root in drive_roots() {
        scan_root(&root, &mut found);
    }
    scan_standard_locations(&mut found);

    let mut list: Vec<InstallInfo> = found.iter().filter_map(|p| to_install_info(p)).collect();

    // 注册到配置（顺便清掉更新残留的 .old 条目）
    let mut cfg = config::load();
    let before = cfg.installs.len();
    cfg.installs.retain(|p| !p.to_lowercase().ends_with(".old"));
    let mut changed = cfg.installs.len() != before;

    let known: BTreeSet<String> = cfg.installs.iter().cloned().collect();
    let discovered: BTreeSet<String> = list.iter().map(|i| i.path.clone()).collect();
    if known != discovered {
        cfg.installs = discovered.iter().cloned().collect();
        changed = true;
    }
    if changed {
        config::save(&cfg);
    }

    // 版本从新到旧
    list.sort_by(|a, b| version_key(&b.version).cmp(&version_key(&a.version)));
    list
}

pub fn version_key(v: &str) -> (u32, u32, u32) {
    let mut it = v.split('.').map(|s| s.trim().parse::<u32>().unwrap_or(0));
    (it.next().unwrap_or(0), it.next().unwrap_or(0), it.next().unwrap_or(0))
}

/// 手动添加一个目录
pub fn add_install(path: &str) -> Result<(), String> {
    let dir = PathBuf::from(path);
    if !buildinfo::is_install_root(&dir) {
        return Err("该目录下没有找到 blender.exe".into());
    }
    let mut cfg = config::load();
    if !cfg.installs.iter().any(|p| p == path) {
        cfg.installs.push(path.to_string());
        config::save(&cfg);
    }
    Ok(())
}
