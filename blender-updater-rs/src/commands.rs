//! 前端命令入口：扫描、检查更新、执行更新、快照、设置。

use crate::buildinfo;
use crate::config::{self, AppConfig};
use crate::daily::DailySource;
use crate::downloader;
use crate::installer;
use crate::model::{CheckResult, DonePayload, InstallInfo, MigrateReport, ProgressPayload, Settings, SnapshotInfo};
use crate::release::ReleaseSource;
use crate::scanner;
use crate::snapshots;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

pub static CANCELLED: AtomicBool = AtomicBool::new(false);

const CHECK_TTL: Duration = Duration::from_secs(180);

static CHECK_CACHE: OnceLock<Mutex<HashMap<String, (Instant, CheckResult)>>> = OnceLock::new();

fn check_cache() -> &'static Mutex<HashMap<String, (Instant, CheckResult)>> {
    CHECK_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cache_key(path: &str, channel: &str) -> String {
    format!("{}|{}", path.to_lowercase(), channel)
}

/// 某个安装的所有缓存失效（更新完成后调用）
fn invalidate(path: &str) {
    let prefix = format!("{}|", path.to_lowercase());
    if let Ok(mut c) = check_cache().lock() {
        c.retain(|k, _| !k.starts_with(&prefix));
    }
}

pub fn emit_progress(app: &AppHandle, phase: &str, percent: f64, message: &str, detail: &str) {
    let _ = app.emit(
        "update-progress",
        ProgressPayload {
            phase: phase.to_string(),
            percent,
            message: message.to_string(),
            detail: detail.to_string(),
        },
    );
}

pub fn emit_done(app: &AppHandle, ok: bool, message: &str) {
    let _ = app.emit(
        "update-done",
        DonePayload {
            ok,
            message: message.to_string(),
        },
    );
}

fn series_of(version_num: &str) -> String {
    let mut it = version_num.split('.');
    let a = it.next().unwrap_or("0");
    let b = it.next().unwrap_or("0");
    format!("{}.{}", a, b)
}

fn vkey(v: &str) -> (u32, u32, u32) {
    scanner::version_key(v)
}

/// 周期标签（给界面看的中文）
fn cycle_label(cycle: &str) -> &'static str {
    match cycle.trim().to_lowercase().as_str() {
        "alpha" => "内测",
        "beta" => "测试",
        "candidate" => "候选",
        "release" | "stable" => "正式",
        _ => "",
    }
}

// ───────────────────────── 检查更新 ─────────────────────────

fn do_check(path: &Path, channel: &str) -> CheckResult {
    let Some(info) = buildinfo::read_install(path) else {
        return CheckResult::error("", channel, "无法读取该目录的 Blender 版本");
    };

    let series = if !info.series.is_empty() {
        info.series.clone()
    } else {
        series_of(&info.version_num)
    };
    let current = info.version.clone();
    let current_num = info.version_num.clone();
    let current_hash = info.build_hash.clone();

    let is_release = channel != "daily";

    // 稳定版通道：先把版本线列表预热进缓存，再并行查「当前系列最新 patch」
    // 与「更高系列提示」，避免两次串行的网络往返。
    let mut higher: Option<(String, u32)> = None;
    // 每日构建通道用：该系列在发布目录里的正式版（用于解释"为什么归档里只有候选版"）
    let mut release_hint = String::new();

    let build = if is_release {
        let src = ReleaseSource::new();
        let _ = src.list_series();
        let (latest_res, higher_res) = std::thread::scope(|s| {
            let t1 = s.spawn(|| src.latest_for_series(&series));
            let t2 = s.spawn(|| src.newest_series_above(&series));
            (
                t1.join().unwrap_or_else(|_| Err("查询版本线失败".to_string())),
                t2.join().unwrap_or_else(|_| Ok(None)),
            )
        });
        higher = higher_res.ok().flatten();
        match latest_res {
            Ok(Some(b)) => b,
            Ok(None) => {
                let mut r = CheckResult::error(&current, channel, "");
                r.status = "latest".into();
                r.kind = "none".into();
                r.series = series.clone();
                r.current_version = current.clone();
                r.source_note = format!(
                    "官方发布目录里还没有 {} 系列的正式版（每日构建可能有），暂时无法比较",
                    series
                );
                r.latest_version = current_num.clone();
                return r;
            }
            Err(e) => return CheckResult::error(&current, channel, format!("检查失败：{}", e)),
        }
    } else {
        match DailySource::new().for_series(&series) {
            Ok(Some(b)) => {
                // 归档里最后一条是候选 / 测试版，通常意味着该系列已经发布了正式版、
                // 每日构建随之停止。补一句说明，免得用户以为程序给出的版本和官网不一致。
                if crate::model::cycle_rank(&b.cycle) < 3 {
                    if let Ok(Some(rel)) = ReleaseSource::new().latest_for_series(&series) {
                        release_hint = format!("（正式版 {} 已发布）", rel.version);
                    }
                }
                b
            }
            Ok(None) => {
                let mut r = CheckResult::error(&current, channel, "");
                r.status = "latest".into();
                r.kind = "none".into();
                r.series = series.clone();
                r.current_version = current.clone();
                r.source_note = format!("官方每日构建里暂时没有 {} 系列的包", series);
                r.latest_version = current_num.clone();
                return r;
            }
            Err(e) => return CheckResult::error(&current, channel, format!("检查失败：{}", e)),
        }
    };

    let latest_num = build.version.trim().to_string();
    let latest_hash = build.hash.clone();

    let cmpu = vkey(&current_num);
    let cmpl = vkey(&latest_num);
    let same_hash = !current_hash.is_empty()
        && !latest_hash.is_empty()
        && current_hash.eq_ignore_ascii_case(&latest_hash);

    // 判断顺序：版本号 → 构建周期（alpha<beta<candidate<release）→ 构建日期。
    // 只看 hash 会把「装了正式版、归档里最后一条是发布前的候选版」误判成可更新，
    // 实际是把用户降级回测试版，所以必须先把周期排明白。
    let cur_rank = crate::model::cycle_rank(&info.cycle);
    let lat_rank = crate::model::cycle_rank(&build.cycle);

    let (status, kind) = if cmpl > cmpu {
        ("update", "patch")
    } else if cmpl < cmpu {
        ("latest", "none")
    } else if same_hash {
        ("latest", "none")
    } else if is_release {
        // 稳定版通道：同版本号换了构建，官方基本不会重新打包，保守不提示
        ("latest", "none")
    } else if lat_rank > cur_rank {
        ("update", "rebuild")
    } else if lat_rank < cur_rank {
        ("latest", "none")
    } else if !info.build_date.is_empty()
        && !build.date.is_empty()
        && build.date.as_str() > info.build_date.as_str()
    {
        ("update", "rebuild")
    } else {
        ("latest", "none")
    };

    let mut r = CheckResult {
        status: status.into(),
        kind: kind.into(),
        channel: channel.into(),
        current_version: current,
        current_hash,
        series: series.clone(),
        latest_version: latest_num,
        latest_hash,
        latest_cycle: if is_release {
            "release".to_string()
        } else {
            build.cycle.clone()
        },
        current_cycle: info.cycle.clone(),
        current_build_date: info.build_date.clone(),
        source_note: if is_release {
            format!("稳定版通道 · Blender{}", series)
        } else {
            let tag = cycle_label(&build.cycle);
            if tag.is_empty() {
                format!("每日构建 · {} 分支{}", build.branch, release_hint)
            } else {
                format!("每日构建 · {} 分支 · {}版{}", build.branch, tag, release_hint)
            }
        },
        higher_version: String::new(),
        higher_series: String::new(),
        higher_url: String::new(),
        higher_note: String::new(),
        download_url: build.url.clone(),
        mirror_urls: build.mirrors.clone(),
        sha256: build.sha256.clone(),
        size_bytes: build.size_bytes,
        file_name: build.file_name.clone(),
        date: build.date.clone(),
        error: String::new(),
    };

    // 大版本提示：只在稳定版通道、且更高系列确实有正式版时给出
    if let Some((hs, hp)) = higher {
        r.higher_version = format!("{}.{}", hs, hp);
        r.higher_series = hs.clone();
        r.higher_url = ReleaseSource::series_url(&hs);
        r.higher_note = format!(
            "{} 系列已经发布，但跨系列升级不会自动覆盖（配置与插件兼容性无法保证）。可自行到官方目录下载。",
            hs
        );
    }

    r
}

#[tauri::command]
pub async fn check_update(path: String, channel: String, force: Option<bool>) -> Result<CheckResult, String> {
    let force = force.unwrap_or(false);
    let key = cache_key(&path, &channel);

    if !force {
        if let Ok(c) = check_cache().lock() {
            if let Some((at, r)) = c.get(&key) {
                if at.elapsed() < CHECK_TTL {
                    return Ok(r.clone());
                }
            }
        }
    }

    let p = PathBuf::from(&path);
    let ch = channel.clone();
    let result = tauri::async_runtime::spawn_blocking(move || do_check(&p, &ch))
        .await
        .map_err(|e| format!("检查任务失败：{}", e))?;

    if result.status != "error" {
        if let Ok(mut c) = check_cache().lock() {
            c.insert(key, (Instant::now(), result.clone()));
        }
    }
    Ok(result)
}

// ───────────────────────── 扫描 / 安装列表 ─────────────────────────

#[tauri::command]
pub fn scan_installs() -> Vec<InstallInfo> {
    scanner::scan_all()
}

#[tauri::command]
pub fn add_install(path: String) -> Result<Vec<InstallInfo>, String> {
    scanner::add_install(&path)?;
    Ok(scanner::scan_all())
}

#[tauri::command]
pub fn remove_install(path: String) -> Vec<InstallInfo> {
    let mut cfg = config::load();
    let low = path.to_lowercase();
    cfg.installs.retain(|p| p.to_lowercase() != low);
    let _ = config::save(&cfg);
    invalidate(&path);
    scanner::scan_all()
}

// ───────────────────────── 执行更新 ─────────────────────────

fn fmt_duration(d: Duration) -> String {
    let s = d.as_secs_f64();
    if s >= 60.0 {
        format!("{:.1} 分钟", s / 60.0)
    } else {
        format!("{:.1} 秒", s)
    }
}

fn run_update(app: &AppHandle, path: &Path, channel: &str) -> Result<String, String> {
    if !buildinfo::is_install_root(path) {
        return Err("该目录下没有 blender.exe".into());
    }
    emit_progress(app, "check", 1.0, "查询最新版本", "");
    let result = do_check(path, channel);

    if result.status == "error" {
        return Err(result.error);
    }
    if result.status != "update" {
        return Ok(format!("已是最新版本（{}）", result.latest_version));
    }
    if result.kind == "rebuild" {
        // 同版本新构建：仅每日构建通道会出现
    }

    let build = crate::model::Build {
        version: result.latest_version.clone(),
        file_name: result.file_name.clone(),
        url: result.download_url.clone(),
        mirrors: result.mirror_urls.clone(),
        sha256: result.sha256.clone(),
        size_bytes: result.size_bytes,
        date: result.date.clone(),
        hash: result.latest_hash.clone(),
        branch: result.series.clone(),
        cycle: result.latest_cycle.clone(),
    };

    let cfg = config::load();
    let mut snap_note = String::new();
    if cfg.auto_backup {
        emit_progress(app, "snapshot", 3.0, "备份配置", "portable/config");
        match snapshots::create(path, "before_install", "") {
            Ok(Some(s)) => {
                snap_note = s.id.clone();
                snapshots::prune(path, cfg.keep_count);
                let _ = app.emit("snapshot-changed", ());
            }
            Ok(None) => snap_note = String::new(),
            Err(e) => {
                return Err(format!("备份配置失败，已中止更新：{}", e));
            }
        }
    }

    emit_progress(app, "download", 6.0, "开始下载", &result.file_name);
    let dl_dir = config::data_root().join("downloads");
    let started = Instant::now();
    let zip = downloader::download(&build, &dl_dir, cfg.prefer_mirror, &CANCELLED, |p| {
        let pct = 6.0 + p.percent * 72.0;
        let name = p.url.rsplit('/').next().unwrap_or("安装包");
        emit_progress(app, "download", pct, &format!("下载 {}", name), &p.detail());
    })?;

    emit_progress(app, "install", 80.0, "解压并替换", "");
    let info = installer::update_from_zip(&zip, path, &CANCELLED, |phase, pct, msg, detail| {
        let mapped = 80.0 + pct * 0.19;
        emit_progress(app, "install", mapped, &format!("{} {}", phase, msg), detail);
    })?;

    let _ = downloader::cleanup(&dl_dir, Some(&zip));
    invalidate(&path.to_string_lossy());

    let msg = format!(
        "已更新 {} → {}（耗时 {}）",
        info.old_version,
        info.new_version,
        fmt_duration(started.elapsed())
    );
    emit_progress(app, "done", 100.0, "更新完成", &msg);
    Ok(msg)
}

#[tauri::command]
pub async fn start_update(app: AppHandle, path: String, channel: String) -> Result<String, String> {
    CANCELLED.store(false, Ordering::Relaxed);
    let p = PathBuf::from(&path);
    let ch = channel.clone();
    let app2 = app.clone();

    let res = tauri::async_runtime::spawn_blocking(move || run_update(&app2, &p, &ch))
        .await
        .map_err(|e| format!("更新任务失败：{}", e))?;

    match &res {
        Ok(msg) => emit_done(&app, true, msg),
        Err(e) => emit_done(&app, false, e),
    }
    res
}

#[tauri::command]
pub fn cancel_update() -> bool {
    CANCELLED.store(true, Ordering::Relaxed);
    true
}

// ───────────────────────── 快照 ─────────────────────────

#[tauri::command]
pub fn list_snapshots(path: String) -> Vec<SnapshotInfo> {
    snapshots::list(&PathBuf::from(path))
}

#[tauri::command]
pub fn make_snapshot(path: String) -> Result<Vec<SnapshotInfo>, String> {
    let p = PathBuf::from(&path);
    snapshots::create(&p, "manual", "")?;
    Ok(snapshots::list(&p))
}

#[tauri::command]
pub fn restore_snapshot(path: String, id: String) -> Result<Vec<SnapshotInfo>, String> {
    let p = PathBuf::from(&path);
    snapshots::restore(&p, &id)?;
    Ok(snapshots::list(&p))
}

#[tauri::command]
pub fn delete_snapshot(path: String, id: String) -> Result<Vec<SnapshotInfo>, String> {
    let p = PathBuf::from(&path);
    snapshots::delete(&p, &id)?;
    Ok(snapshots::list(&p))
}

// ───────────────────────── 设置 ─────────────────────────

const AUTHOR: &str = "WittyMing";
const HOMEPAGE: &str = "https://space.bilibili.com/263206115";

fn build_settings(cfg: &AppConfig) -> Settings {
    let default_root = config::default_snapshot_root();
    Settings {
        snapshot_dir: config::snapshot_root().display().to_string(),
        default_snapshot_dir: default_root.display().to_string(),
        snapshot_dir_custom: cfg.snapshot_dir.is_some(),
        keep_count: cfg.keep_count,
        auto_backup: cfg.auto_backup,
        prefer_mirror: cfg.prefer_mirror,
        data_dir: config::data_root().display().to_string(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        author: AUTHOR.to_string(),
        homepage: HOMEPAGE.to_string(),
    }
}

#[tauri::command]
pub fn get_settings() -> Settings {
    build_settings(&config::load())
}

#[tauri::command]
pub fn save_settings(next: Settings) -> Result<MigrateReport, String> {
    let mut cfg = config::load();
    let mut report = MigrateReport {
        moved: 0,
        failed: 0,
        from: String::new(),
        to: String::new(),
    };

    let want = next.snapshot_dir.trim().to_string();
    let default_root = config::default_snapshot_root();
    let current = config::snapshot_root();
    let want_path = PathBuf::from(&want);

    if !want.is_empty() && want_path != current {
        report = snapshots::migrate_to(&want_path)?;
        let mut cfg2 = config::load();
        cfg2.snapshot_dir = if want_path == default_root {
            None
        } else {
            Some(want.clone())
        };
        config::save(&cfg2);
        cfg = config::load();
    } else if want_path == default_root && cfg.snapshot_dir.is_some() {
        cfg.snapshot_dir = None;
    }

    cfg.keep_count = next.keep_count.clamp(1, 50);
    cfg.auto_backup = next.auto_backup;
    cfg.prefer_mirror = next.prefer_mirror;
    config::save(&cfg);

    let keep = cfg.keep_count;
    for inst in &cfg.installs {
        snapshots::prune(Path::new(inst), keep);
    }

    Ok(report)
}

#[tauri::command]
pub fn set_keep_count(count: u32) -> u32 {
    let mut cfg = config::load();
    cfg.keep_count = count.clamp(1, 50);
    let _ = config::save(&cfg);
    let keep = cfg.keep_count;
    for inst in &cfg.installs {
        snapshots::prune(Path::new(inst), keep);
    }
    keep
}

#[tauri::command]
pub fn open_path(path: String) -> Result<(), String> {
    if path.starts_with("http://") || path.starts_with("https://") {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", &path])
            .spawn()
            .map_err(|e| format!("无法打开链接：{}", e))?;
        return Ok(());
    }
    if !Path::new(&path).exists() {
        return Err(format!("路径不存在：{}", path));
    }
    std::process::Command::new("explorer")
        .arg(&path)
        .spawn()
        .map_err(|e| format!("无法打开：{}", e))?;
    Ok(())
}

#[tauri::command]
pub fn open_snapshot_dir(path: String) -> Result<(), String> {
    let p = PathBuf::from(path);
    let dir = snapshots::snap_dir(&p);
    let target = if dir.is_dir() {
        dir
    } else {
        snapshots::snapshot_root()
    };
    let _ = std::fs::create_dir_all(&target);
    std::process::Command::new("explorer")
        .arg(target.display().to_string())
        .spawn()
        .map_err(|e| format!("无法打开：{}", e))?;
    Ok(())
}

#[tauri::command]
pub fn snapshot_usage() -> u64 {
    snapshots::total_bytes()
}

// ───────────────────────── 无边框窗口控制 ─────────────────────────

#[tauri::command]
pub fn win_minimize(app: AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.minimize();
    }
}

#[tauri::command]
pub fn win_toggle_maximize(app: AppHandle) -> bool {
    if let Some(w) = app.get_webview_window("main") {
        let max = w.is_maximized().unwrap_or(false);
        if max {
            let _ = w.unmaximize();
        } else {
            let _ = w.maximize();
        }
        return !max;
    }
    false
}

#[tauri::command]
pub fn win_close(app: AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.close();
    }
}

#[tauri::command]
pub fn win_is_maximized(app: AppHandle) -> bool {
    app.get_webview_window("main")
        .and_then(|w| w.is_maximized().ok())
        .unwrap_or(false)
}
