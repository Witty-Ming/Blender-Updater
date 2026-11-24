//! 读取 Blender 安装的版本信息。
//!
//! 版本号必须来自 "blender.exe --version"：目录名常常和实际版本不一致
//! （例如目录 blender-5.2.0-windows-x64 里装的其实是 5.2.2 LTS）。
//! 探测一次约 0.3–1 秒，因此按 exe 的 (mtime, size) 做进程内 + 磁盘两级缓存。

use crate::config::{self, VersionCacheEntry};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Blender 版本信息
#[derive(Clone, Debug, Default)]
pub struct BuildInfo {
    /// 显示用：5.2.2 LTS
    pub version: String,
    /// 比较用：5.2.2
    pub version_num: String,
    /// 版本线：5.2
    pub series: String,
    pub build_hash: String,
    pub branch: String,
    /// 构建日期时间，本地口径：2026-11-10 01:33
    pub build_date: String,
    /// blender.exe --version 里的 build type 原文：Release / Release Candidate / Alpha / Beta
    pub build_type: String,
    /// 周期归一化：release / candidate / beta / alpha
    pub cycle: String,
}

/// 把 build type 原文归一化成周期
pub fn cycle_of_build_type(raw: &str) -> String {
    let t = raw.trim().to_lowercase();
    if t.contains("candidate") {
        "candidate".into()
    } else if t.contains("alpha") {
        "alpha".into()
    } else if t.contains("beta") {
        "beta".into()
    } else if !t.is_empty() {
        "release".into()
    } else {
        String::new()
    }
}

static CACHE: OnceLock<Mutex<HashMap<String, VersionCacheEntry>>> = OnceLock::new();
static INIT: OnceLock<()> = OnceLock::new();

fn cache() -> &'static Mutex<HashMap<String, VersionCacheEntry>> {
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 首次访问时把磁盘缓存载入内存
fn ensure_cache() {
    INIT.get_or_init(|| {
        let cfg = config::load();
        if let Ok(mut c) = cache().lock() {
            *c = cfg.version_cache;
        }
    });
}

/// 把内存缓存导出（供写回 config.json）
pub fn dump_cache() -> HashMap<String, VersionCacheEntry> {
    ensure_cache();
    cache().lock().map(|c| c.clone()).unwrap_or_default()
}

fn cache_key(dir: &Path) -> String {
    let raw = dir.to_string_lossy().to_lowercase();
    // canonicalize 会给盘符路径加上 \\?\ 前缀，去掉后同一目录只占一条缓存
    let s = raw.strip_prefix(r"\\?\").unwrap_or(&raw);
    s.trim_end_matches('\\').to_string()
}

/// exe 的 (mtime 秒, 字节数)
fn exe_stamp(exe: &Path) -> (i64, u64) {
    let Ok(md) = std::fs::metadata(exe) else { return (0, 0) };
    let mtime = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    (mtime, md.len())
}

/// "5.2.2 LTS" -> ("5.2.2 LTS", "5.2.2")
fn split_version(raw: &str) -> (String, String) {
    let raw = raw.trim();
    let num = raw.split_whitespace().next().unwrap_or("").to_string();
    (raw.to_string(), num)
}

/// "5.2.2" -> "5.2"
pub fn series_of(version_num: &str) -> String {
    version_num.split('.').take(2).collect::<Vec<_>>().join(".")
}

/// 判断一个目录是不是 Blender 安装根目录
pub fn is_install_root(dir: &Path) -> bool {
    dir.join("blender.exe").is_file()
}

/// 从目录名解析版本，作为探测失败时的兜底：blender-5.3.0-windows-x64 -> 5.3.0
fn version_from_dirname(dir: &Path) -> Option<String> {
    let name = dir.file_name()?.to_string_lossy().to_lowercase();
    let idx = name.find("blender")?;
    let rest = &name[idx + 7..];
    let rest = rest.strip_prefix('-').unwrap_or(rest);
    let mut it = rest.split(|c: char| !c.is_ascii_digit() && c != '.');
    let head = it.next().unwrap_or("");
    let parts: Vec<&str> = head.split('.').filter(|s| !s.is_empty()).collect();
    if parts.len() >= 2 {
        Some(parts[..parts.len().min(3)].join("."))
    } else {
        None
    }
}

/// 运行 blender.exe --version 拿完整信息（慢，仅缓存未命中时用）
pub fn probe_exe(exe: &Path) -> BuildInfo {
    let mut cmd = Command::new(exe);
    cmd.arg("--version");
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    let mut info = BuildInfo::default();
    let Ok(out) = cmd.output() else { return info };

    let mut text = String::from_utf8_lossy(&out.stdout).to_string();
    if text.trim().is_empty() {
        text = String::from_utf8_lossy(&out.stderr).to_string();
    }

    let mut build_day = String::new();
    let mut build_time = String::new();

    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Blender ") {
            info.version = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("build hash:") {
            info.build_hash = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("build branch:") {
            info.branch = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("build date:") {
            build_day = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("build time:") {
            build_time = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("build type:") {
            info.build_type = rest.trim().to_string();
        }
    }

    // 构建日期时间：2026-11-10 + 01:33:43 -> "2026-11-10 01:33"
    if !build_day.is_empty() {
        let hhmm: String = build_time.chars().take(5).collect();
        info.build_date = if hhmm.len() == 5 {
            format!("{} {}", build_day, hhmm)
        } else {
            build_day
        };
    }
    info.cycle = cycle_of_build_type(&info.build_type);

    if !info.version.is_empty() {
        let (v, num) = split_version(&info.version);
        info.series = series_of(&num);
        info.version = v;
        info.version_num = num;
    }
    info
}

/// 获取安装信息：探测 exe（带缓存），失败时退回目录名
pub fn read_install(dir: &Path) -> Option<BuildInfo> {
    if !is_install_root(dir) {
        return None;
    }
    ensure_cache();

    let exe = dir.join("blender.exe");
    let key = cache_key(dir);
    let (mtime, size) = exe_stamp(&exe);

    // 缓存命中
    if mtime != 0 {
        if let Ok(c) = cache().lock() {
            if let Some(e) = c.get(&key) {
                // build_date 是后加的字段：旧缓存里没有，命中它会丢掉构建日期，
                // 这种情况当作失效，重新探测一次（每个安装只会多跑一次）
                if e.exe_mtime == mtime
                    && e.exe_size == size
                    && !e.version.is_empty()
                    && !e.build_date.is_empty()
                {
                    return Some(BuildInfo {
                        version: e.version.clone(),
                        version_num: e.version_num.clone(),
                        series: e.series.clone(),
                        build_hash: e.build_hash.clone(),
                        branch: e.branch.clone(),
                        build_date: e.build_date.clone(),
                        build_type: e.build_type.clone(),
                        cycle: e.cycle.clone(),
                    });
                }
            }
        }
    }

    let probed = probe_exe(&exe);
    if !probed.version.is_empty() {
        if mtime != 0 {
            if let Ok(mut c) = cache().lock() {
                c.insert(
                    key,
                    VersionCacheEntry {
                        exe_mtime: mtime,
                        exe_size: size,
                        version: probed.version.clone(),
                        version_num: probed.version_num.clone(),
                        series: probed.series.clone(),
                        build_hash: probed.build_hash.clone(),
                        branch: probed.branch.clone(),
                        build_date: probed.build_date.clone(),
                        build_type: probed.build_type.clone(),
                        cycle: probed.cycle.clone(),
                    },
                );
            }
        }
        return Some(probed);
    }

    // 兜底：探测不出来时用目录名，至少不要让这个安装消失
    let fallback = version_from_dirname(dir)?;
    let series = series_of(&fallback);
    Some(BuildInfo {
        version: fallback.clone(),
        version_num: fallback,
        series,
        build_hash: String::new(),
        branch: String::new(),
        build_date: String::new(),
        build_type: String::new(),
        cycle: String::new(),
    })
}

/// 安装目录的 portable 配置目录
pub fn config_dir(dir: &Path) -> PathBuf {
    dir.join("portable").join("config")
}

pub fn has_portable_config(dir: &Path) -> bool {
    config_dir(dir).is_dir()
}
