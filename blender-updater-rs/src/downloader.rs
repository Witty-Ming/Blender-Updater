//! 安装包下载：多镜像 + 校验 + 可取消。

use crate::model::Build;
use crate::release::dedup_urls;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub const CHUNK: usize = 128 * 1024;

pub struct DownloadProgress {
    pub percent: f64,
    pub done: u64,
    pub total: u64,
    pub bytes_per_sec: f64,
    pub url: String,
}

fn human(n: u64) -> String {
    let mb = n as f64 / 1024.0 / 1024.0;
    if mb >= 1024.0 {
        format!("{:.2} GB", mb / 1024.0)
    } else {
        format!("{:.1} MB", mb)
    }
}

impl DownloadProgress {
    pub fn detail(&self) -> String {
        let speed = if self.bytes_per_sec > 0.0 {
            format!(" · {}/s", human(self.bytes_per_sec as u64))
        } else {
            String::new()
        };
        if self.total > 0 {
            format!("{} / {}{}", human(self.done), human(self.total), speed)
        } else {
            format!("{}{}", human(self.done), speed)
        }
    }
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut f = File::open(path).map_err(|e| format!("打开文件失败：{}", e))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = f.read(&mut buf).map_err(|e| format!("读取文件失败：{}", e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// 从单个 url 抓一份到 part 文件，返回 (已下载字节, 总字节)。
/// 出错时不删除 part，交给调用方清理。
fn fetch_one<F>(
    agent: &ureq::Agent,
    url: &str,
    part: &Path,
    size_hint: u64,
    cancel: &AtomicBool,
    on: &mut F,
) -> Result<(u64, u64), String>
where
    F: FnMut(&DownloadProgress),
{
    let resp = agent
        .get(url)
        .call()
        .map_err(|e| format!("{} 无法访问：{}", url, e))?;

    let total = resp
        .header("content-length")
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(size_hint);

    let _ = fs::remove_file(part);
    let file = File::create(part).map_err(|e| format!("创建临时文件失败：{}", e))?;
    let mut writer = BufWriter::with_capacity(CHUNK * 4, file);
    let mut reader = resp.into_reader();
    let mut buf = vec![0u8; CHUNK];
    let mut done = 0u64;
    let mut last_emit = Instant::now();
    let mut window_start = Instant::now();
    let mut window_bytes = 0u64;
    let mut speed = 0.0f64;

    on(&DownloadProgress {
        percent: 0.0,
        done: 0,
        total,
        bytes_per_sec: 0.0,
        url: url.to_string(),
    });

    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = writer.flush();
            return Err("已取消".into());
        }
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => return Err(format!("{} 下载中断：{}", url, e)),
        };
        writer
            .write_all(&buf[..n])
            .map_err(|e| format!("写入磁盘失败：{}", e))?;
        done += n as u64;
        window_bytes += n as u64;

        let elapsed = window_start.elapsed();
        if elapsed >= Duration::from_millis(500) {
            speed = window_bytes as f64 / elapsed.as_secs_f64();
            window_start = Instant::now();
            window_bytes = 0;
        }
        if last_emit.elapsed() >= Duration::from_millis(150) {
            last_emit = Instant::now();
            let percent = if total > 0 { done as f64 / total as f64 } else { 0.0 };
            on(&DownloadProgress {
                percent: percent.min(1.0),
                done,
                total,
                bytes_per_sec: speed,
                url: url.to_string(),
            });
        }
    }

    writer.flush().map_err(|e| format!("刷新磁盘失败：{}", e))?;
    Ok((done, total))
}

/// 下载到 dest_dir/file_name，返回最终路径。
/// prefer_mirror 为 true 时先试国内镜像，失败再退到官方源。
pub fn download<F>(
    build: &Build,
    dest_dir: &Path,
    prefer_mirror: bool,
    cancel: &AtomicBool,
    mut on: F,
) -> Result<PathBuf, String>
where
    F: FnMut(&DownloadProgress),
{
    fs::create_dir_all(dest_dir).map_err(|e| format!("创建下载目录失败：{}", e))?;
    let final_path = dest_dir.join(&build.file_name);
    let part_path = dest_dir.join(format!("{}.part", build.file_name));

    let mut candidates: Vec<String> = if prefer_mirror {
        let mut v = build.mirrors.clone();
        v.push(build.url.clone());
        v
    } else {
        let mut v = vec![build.url.clone()];
        v.extend(build.mirrors.clone());
        v
    };
    candidates = dedup_urls(candidates);
    if candidates.is_empty() {
        return Err("没有可用的下载地址".into());
    }

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(30))
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) BlenderUpdater/0.3")
        .build();

    let mut last_err = String::from("下载失败");
    for url in candidates.iter() {
        if cancel.load(Ordering::Relaxed) {
            let _ = fs::remove_file(&part_path);
            return Err("已取消".into());
        }

        let (done, total) = match fetch_one(&agent, url, &part_path, build.size_bytes, cancel, &mut on) {
            Ok(v) => v,
            Err(e) => {
                let _ = fs::remove_file(&part_path);
                if e == "已取消" {
                    return Err(e);
                }
                last_err = e;
                continue;
            }
        };

        // 大小校验：镜像偶有截断
        if total > 0 && done != total {
            let _ = fs::remove_file(&part_path);
            last_err = format!("下载不完整（{} / {} 字节），换下一个源", done, total);
            continue;
        }

        // 哈希校验
        if !build.sha256.is_empty() {
            match sha256_file(&part_path) {
                Ok(got) if got.eq_ignore_ascii_case(&build.sha256) => {}
                Ok(got) => {
                    let _ = fs::remove_file(&part_path);
                    last_err = format!("sha256 校验失败（期望 {}，实际 {}）", build.sha256, got);
                    continue;
                }
                Err(e) => {
                    let _ = fs::remove_file(&part_path);
                    last_err = e;
                    continue;
                }
            }
        }

        let _ = fs::remove_file(&final_path);
        fs::rename(&part_path, &final_path).map_err(|e| format!("重命名失败：{}", e))?;
        on(&DownloadProgress {
            percent: 1.0,
            done,
            total: if total > 0 { total } else { done },
            bytes_per_sec: 0.0,
            url: url.clone(),
        });
        return Ok(final_path);
    }

    let _ = fs::remove_file(&part_path);
    Err(last_err)
}

/// 清理下载目录：只保留 keep 指定的那个安装包
pub fn cleanup(dir: &Path, keep: Option<&Path>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let p = entry.path();
        if !p.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".zip") && !name.ends_with(".part") {
            continue;
        }
        if let Some(k) = keep {
            if p == k {
                continue;
            }
        }
        let _ = fs::remove_file(&p);
    }
}
