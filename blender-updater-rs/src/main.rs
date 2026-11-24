// 发布版不弹控制台窗口
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod buildinfo;
mod commands;
mod config;
mod daily;
mod downloader;
mod installer;
mod model;
mod release;
mod scanner;
mod snapshots;

use std::sync::atomic::Ordering;

/// 退出前把版本探测缓存写回 config.json，下次启动免于再跑 blender.exe --version
fn persist_cache() {
    let dump = buildinfo::dump_cache();
    if dump.is_empty() {
        return;
    }
    let mut cfg = config::load();
    cfg.version_cache = dump;
    let _ = config::save(&cfg);
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            commands::scan_installs,
            commands::add_install,
            commands::remove_install,
            commands::check_update,
            commands::start_update,
            commands::cancel_update,
            commands::list_snapshots,
            commands::make_snapshot,
            commands::restore_snapshot,
            commands::delete_snapshot,
            commands::get_settings,
            commands::save_settings,
            commands::set_keep_count,
            commands::open_path,
            commands::open_snapshot_dir,
            commands::snapshot_usage,
            commands::win_minimize,
            commands::win_toggle_maximize,
            commands::win_close,
            commands::win_is_maximized,
        ])
        .setup(|_app| {
            // 后台清理上次中断留下的 xxx.old 与 staging（可能很大，别阻塞启动）
            std::thread::spawn(|| {
                let _ = installer::cleanup_leftovers();
            });
            Ok(())
        })
        .on_window_event(|_window, event| {
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                commands::CANCELLED.store(false, Ordering::Relaxed);
                persist_cache();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
