//! 用户配置持久化：%APPDATA%\BlenderUpdater\config.json

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// 版本探测缓存：按 blender.exe 的 (mtime, size) 判定是否需要重新探测
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct VersionCacheEntry {
    pub exe_mtime: i64,
    pub exe_size: u64,
    pub version: String,
    pub version_num: String,
    pub series: String,
    pub build_hash: String,
    pub branch: String,
    /// 构建日期时间（2026-11-10 01:33），旧缓存里没有这个字段
    #[serde(default)]
    pub build_date: String,
    #[serde(default)]
    pub build_type: String,
    #[serde(default)]
    pub cycle: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AppConfig {
    /// 用户手动登记 / 扫描到的安装目录
    #[serde(default)]
    pub installs: Vec<String>,
    /// 自动备份保留份数
    #[serde(default = "default_keep")]
    pub keep_count: u32,
    /// 自定义备份根目录；None 表示用默认位置
    #[serde(default)]
    pub snapshot_dir: Option<String>,
    /// 更新前自动备份配置
    #[serde(default = "default_true")]
    pub auto_backup: bool,
    /// 优先使用国内镜像下载
    #[serde(default = "default_true")]
    pub prefer_mirror: bool,
    /// 版本探测缓存
    #[serde(default)]
    pub version_cache: HashMap<String, VersionCacheEntry>,
}

fn default_keep() -> u32 {
    5
}

fn default_true() -> bool {
    true
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            installs: vec![],
            keep_count: default_keep(),
            snapshot_dir: None,
            auto_backup: true,
            prefer_mirror: true,
            version_cache: HashMap::new(),
        }
    }
}

pub fn data_root() -> PathBuf {
    if let Some(dir) = dirs::data_dir() {
        dir.join("BlenderUpdater")
    } else {
        PathBuf::from(".")
    }
}

fn config_path() -> PathBuf {
    data_root().join("config.json")
}

pub fn load() -> AppConfig {
    let p = config_path();
    std::fs::read_to_string(&p)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(cfg: &AppConfig) {
    let p = config_path();
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(s) = serde_json::to_string_pretty(cfg) {
        let _ = std::fs::write(&p, s);
    }
}

/// 默认的备份根目录
pub fn default_snapshot_root() -> PathBuf {
    data_root().join("snapshots")
}

/// 当前生效的备份根目录
pub fn snapshot_root() -> PathBuf {
    let cfg = load();
    cfg.snapshot_dir
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(default_snapshot_root)
}
