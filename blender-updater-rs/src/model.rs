//! 通用数据结构（与前端 JSON 契约一致）。

use serde::{Deserialize, Serialize};

/// 徽章：侧栏安装卡片右上角的状态标记
#[derive(Serialize, Clone, Default, Debug)]
pub struct StatusBadge {
    pub variant: String,
    pub text: String,
}

impl StatusBadge {
    pub fn none() -> Self {
        Self::default()
    }
    pub fn new(variant: &str, text: &str) -> Self {
        Self { variant: variant.into(), text: text.into() }
    }
}

/// 一个 Blender 安装
#[derive(Serialize, Clone, Debug)]
pub struct InstallInfo {
    pub path: String,
    pub name: String,
    /// 显示用版本，可能带后缀：5.2.2 LTS
    pub version: String,
    /// 比较用版本：5.2.2
    pub version_num: String,
    /// 版本线：5.2
    pub series: String,
    pub build_hash: String,
    pub branch: String,
    pub is_portable: bool,
    pub has_config: bool,
    pub status_badge: StatusBadge,
}

/// 检查更新的结果
#[derive(Serialize, Clone, Debug)]
pub struct CheckResult {
    /// "latest" | "update" | "error"
    pub status: String,
    /// "patch"（本系列小版本）| "rebuild"（同版本新构建）| "none" | "error"
    pub kind: String,
    pub channel: String,
    pub current_version: String,
    pub current_hash: String,
    pub series: String,
    pub latest_version: String,
    pub latest_hash: String,
    /// 最新构建的周期（alpha / beta / candidate / release）
    pub latest_cycle: String,
    /// 已安装版本的周期（来自 blender.exe --version 的 build type）
    pub current_cycle: String,
    /// 已安装版本的构建日期时间（build date + build time，本地时间）
    pub current_build_date: String,
    /// 来源说明，例如「官方稳定版 · 5.2 系列」
    pub source_note: String,
    /// 更高的稳定版本线（不自动更新，只提示）
    pub higher_version: String,
    pub higher_series: String,
    pub higher_url: String,
    pub higher_note: String,
    pub download_url: String,
    pub mirror_urls: Vec<String>,
    pub sha256: String,
    pub size_bytes: u64,
    pub file_name: String,
    pub date: String,
    pub error: String,
}

impl CheckResult {
    pub fn error(current: &str, channel: &str, msg: impl Into<String>) -> Self {
        Self {
            status: "error".into(),
            kind: "error".into(),
            channel: channel.into(),
            current_version: current.into(),
            current_hash: String::new(),
            series: String::new(),
            latest_version: String::new(),
            latest_hash: String::new(),
            latest_cycle: String::new(),
            current_cycle: String::new(),
            current_build_date: String::new(),
            source_note: String::new(),
            higher_version: String::new(),
            higher_series: String::new(),
            higher_url: String::new(),
            higher_note: String::new(),
            download_url: String::new(),
            mirror_urls: vec![],
            sha256: String::new(),
            size_bytes: 0,
            file_name: String::new(),
            date: String::new(),
            error: msg.into(),
        }
    }
}

/// 一个构建（稳定版或每日构建）
#[derive(Clone, Debug, Default)]
pub struct Build {
    pub version: String,
    pub file_name: String,
    /// 主下载地址
    pub url: String,
    /// 备用镜像（同一文件）
    pub mirrors: Vec<String>,
    pub sha256: String,
    pub size_bytes: u64,
    pub date: String,
    pub hash: String,
    pub branch: String,
    /// 构建周期：release / candidate / beta / alpha（稳定版固定 release）
    pub cycle: String,
}

/// 构建周期的先后：alpha < beta < candidate < release
/// 未知 / 空值按 release 处理（保守：宁可不提示更新，也不把人降到候选版）
pub fn cycle_rank(cycle: &str) -> i32 {
    match cycle.trim().to_lowercase().as_str() {
        "alpha" => 0,
        "beta" => 1,
        "candidate" => 2,
        _ => 3,
    }
}

/// 配置快照
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SnapshotInfo {
    pub id: String,
    pub created_at: String,
    pub reason: String,
    pub note: String,
    pub blender_version: String,
    pub build_hash: String,
    pub source_path: String,
    pub file_count: u64,
    pub bytes: u64,
    pub locked: bool,
}

/// 设置页需要的数据
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Settings {
    pub snapshot_dir: String,
    pub default_snapshot_dir: String,
    pub snapshot_dir_custom: bool,
    pub keep_count: u32,
    pub auto_backup: bool,
    pub prefer_mirror: bool,
    pub data_dir: String,
    pub app_version: String,
    pub author: String,
    pub homepage: String,
}

/// 迁移结果
#[derive(Serialize, Clone, Debug, Default)]
pub struct MigrateReport {
    pub moved: u32,
    pub failed: u32,
    pub from: String,
    pub to: String,
}

/// 进度事件负载
#[derive(Serialize, Clone, Debug)]
pub struct ProgressPayload {
    pub phase: String,
    pub percent: f64,
    pub message: String,
    pub detail: String,
}

/// 完成事件负载
#[derive(Serialize, Clone, Debug)]
pub struct DonePayload {
    pub ok: bool,
    pub message: String,
}
