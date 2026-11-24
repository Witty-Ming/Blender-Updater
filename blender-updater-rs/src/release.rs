//! 稳定版源：download.blender.org/release 的 autoindex + 国内镜像。
//!
//! 关键教训（来自 Python 版）：单个镜像可能同步不全（清华源曾缺失全部
//! windows-x64.zip），因此所有列表都必须取多镜像并集。
//!
//! 性能：三个镜像并发请求，并对目录列表做 5 分钟的进程内缓存，
//! 这样切换通道 / 安装时重复检查几乎是瞬时的。

use crate::model::Build;
use crate::scanner::version_key;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const MIRRORS: &[&str] = &[
    "https://mirrors.tuna.tsinghua.edu.cn/blender/release",
    "https://mirrors.aliyun.com/blender/release",
    "https://download.blender.org/release",
];

const CACHE_TTL: Duration = Duration::from_secs(300);

/// 单次目录抓取的总等待预算：慢镜像不再无限期拖住检查流程
const FETCH_BUDGET: Duration = Duration::from_millis(5000);

static DIR_CACHE: OnceLock<Mutex<HashMap<String, (Instant, Vec<String>)>>> = OnceLock::new();

fn dir_cache() -> &'static Mutex<HashMap<String, (Instant, Vec<String>)>> {
    DIR_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub struct ReleaseSource {
    agent: ureq::Agent,
}

impl ReleaseSource {
    pub fn new() -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(6))
            .timeout_read(Duration::from_secs(12))
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) BlenderUpdater/0.3")
            .build();
        Self { agent }
    }

    /// 并发抓取所有镜像上同一路径的正文，返回成功的那些
    /// 并发抓取所有镜像上同一路径的正文，返回成功的那些。
    /// 用的是「总预算 + 先到先用」：最快的镜像 1~2 秒就回来，
    /// 不会再被某个慢镜像按 12 秒超时拖住整条检查链路。
    fn get_all(&self, path: &str) -> Vec<String> {
        let (tx, rx) = std::sync::mpsc::channel::<Option<String>>();
        for m in MIRRORS {
            let url = format!("{}{}", m, path);
            let agent = self.agent.clone();
            let tx = tx.clone();
            std::thread::spawn(move || {
                let text = agent
                    .get(&url)
                    .call()
                    .ok()
                    .and_then(|r| r.into_string().ok())
                    .filter(|t| !t.trim().is_empty());
                let _ = tx.send(text);
            });
        }
        drop(tx);

        let deadline = Instant::now() + FETCH_BUDGET;
        let mut out: Vec<String> = Vec::new();
        let mut done = 0usize;
        while done < MIRRORS.len() {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            match rx.recv_timeout(left) {
                Ok(Some(t)) => {
                    out.push(t);
                    done += 1;
                }
                Ok(None) => done += 1,
                Err(_) => break,
            }
        }
        out
    }

    fn hrefs_union(&self, path: &str) -> Result<Vec<String>, String> {
        // 命中缓存
        if let Ok(c) = dir_cache().lock() {
            if let Some((at, v)) = c.get(path) {
                if at.elapsed() < CACHE_TTL {
                    return Ok(v.clone());
                }
            }
        }

        let texts = self.get_all(path);
        if texts.is_empty() {
            return Err("所有镜像都无法访问，请检查网络连接".into());
        }
        let re = Regex::new(r#"href="([^"]+)""#).unwrap();
        let mut set = std::collections::BTreeSet::new();
        for t in texts {
            for c in re.captures_iter(&t) {
                set.insert(c[1].to_string());
            }
        }
        let out: Vec<String> = set.into_iter().collect();

        if let Ok(mut c) = dir_cache().lock() {
            c.insert(path.to_string(), (Instant::now(), out.clone()));
        }
        Ok(out)
    }

    /// 所有可用的版本线，从旧到新
    pub fn list_series(&self) -> Result<Vec<String>, String> {
        let hrefs = self.hrefs_union("/")?;
        let re = Regex::new(r"^Blender(\d+\.\d+)/$").unwrap();
        let mut set: std::collections::BTreeSet<(u32, u32, String)> = Default::default();
        for h in hrefs {
            if let Some(c) = re.captures(&h) {
                let v = c[1].to_string();
                let (a, b, _) = version_key(&format!("{}.0", v));
                set.insert((a, b, v));
            }
        }
        Ok(set.into_iter().map(|(_, _, v)| v).collect())
    }

    /// 某个版本线下所有 Windows x64 的 patch 号，从旧到新
    pub fn list_patches(&self, series: &str) -> Result<Vec<u32>, String> {
        let hrefs = self.hrefs_union(&format!("/Blender{}/", series))?;
        let pat = format!(r"^blender-{}\.(\d+)-windows-x64\.zip$", regex::escape(series));
        let re = Regex::new(&pat).unwrap();
        let mut set: std::collections::BTreeSet<u32> = Default::default();
        for h in hrefs {
            if let Some(c) = re.captures(&h) {
                if let Ok(n) = c[1].parse::<u32>() {
                    set.insert(n);
                }
            }
        }
        Ok(set.into_iter().collect())
    }

    /// 某个 patch 的 sha256 清单（文件名 -> 哈希），多镜像并集
    pub fn fetch_sha256(&self, series: &str, patch: u32) -> HashMap<String, String> {
        let path = format!("/Blender{}/blender-{}.{}.sha256", series, series, patch);
        let mut map = HashMap::new();
        for text in self.get_all(&path) {
            for line in text.lines() {
                let mut it = line.split_whitespace();
                let (Some(hash), Some(name)) = (it.next(), it.next()) else { continue };
                if hash.len() == 64 {
                    map.insert(name.trim_start_matches('*').to_string(), hash.to_lowercase());
                }
            }
        }
        map
    }

    /// 组装一个 Build（含大小与日期）
    pub fn find_build(&self, series: &str, patch: u32) -> Result<Build, String> {
        let file_name = format!("blender-{}.{}-windows-x64.zip", series, patch);
        let rel_path = format!("/Blender{}/{}", series, file_name);

        let url = format!("https://download.blender.org/release{}", rel_path);
        let mirrors = MIRRORS
            .iter()
            .filter(|m| !m.contains("download.blender.org"))
            .map(|m| format!("{}{}", m, rel_path))
            .collect::<Vec<_>>();

        // 校验和文件与 HEAD 请求互不依赖，并发发出
        let (sha_map, size, date) = std::thread::scope(|s| {
            let t_sha = s.spawn(|| self.fetch_sha256(series, patch));
            let head_url = url.clone();
            let head_agent = self.agent.clone();
            let t_head = s.spawn(move || -> (u64, String) {
                let mut size = 0u64;
                let mut date = String::new();
                if let Ok(resp) = head_agent.head(&head_url).call() {
                    if let Some(v) = resp.header("content-length") {
                        size = v.parse().unwrap_or(0);
                    }
                    if let Some(v) = resp.header("last-modified") {
                        date = fmt_http_date(v);
                    }
                }
                (size, date)
            });
            let map = t_sha.join().unwrap_or_default();
            let (size, date) = t_head.join().unwrap_or((0, String::new()));
            (map, size, date)
        });
        let sha256 = sha_map.get(&file_name).cloned().unwrap_or_default();

        Ok(Build {
            version: format!("{}.{}", series, patch),
            file_name,
            url,
            mirrors,
            sha256,
            size_bytes: size,
            date,
            hash: String::new(),
            branch: format!("v{}", series),
            cycle: "release".into(),
        })
    }

    /// 某个版本线（"5.2"）在 release 目录里的最新 patch。
    /// 该版本线不存在（例如用户装的是 5.4 的 alpha 而正式版还没发布）时返回 Ok(None)。
    pub fn latest_for_series(&self, series: &str) -> Result<Option<Build>, String> {
        let series_list = self.list_series()?;
        if !series_list.iter().any(|s| s == series) {
            return Ok(None);
        }
        let patches = self.list_patches(series)?;
        let Some(&p) = patches.last() else { return Ok(None) };
        Ok(Some(self.find_build(series, p)?))
    }

    /// 比 current（"5.2"）更高的最新稳定版线 -> ("5.3", 0)。
    /// 只查最高的那条线，避免对 25 个版本线逐个发请求。
    pub fn newest_series_above(&self, current: &str) -> Result<Option<(String, u32)>, String> {
        let list = self.list_series()?;
        let (cmaj, cmin, _) = version_key(&format!("{}.0", current));
        let mut best: Option<(u32, u32, String)> = None;
        for s in list {
            let (maj, min, _) = version_key(&format!("{}.0", s));
            if (maj, min) <= (cmaj, cmin) {
                continue;
            }
            let better = best.as_ref().map(|(a, b, _)| (maj, min) > (*a, *b)).unwrap_or(true);
            if better {
                best = Some((maj, min, s));
            }
        }
        let Some((_, _, s)) = best else { return Ok(None) };
        let patches = self.list_patches(&s)?;
        Ok(patches.last().map(|&p| (s.clone(), p)))
    }

    /// 官方下载目录页（给「更高版本」提示用）
    pub fn series_url(series: &str) -> String {
        format!("https://download.blender.org/release/Blender{}/", series)
    }
}

/// 保序去重
pub fn dedup_urls(list: Vec<String>) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    list.into_iter().filter(|u| seen.insert(u.clone())).collect()
}

/// HTTP 日期头（RFC 2822）转成本地时间，失败就原样返回
fn fmt_http_date(raw: &str) -> String {
    match chrono::DateTime::parse_from_rfc2822(raw) {
        Ok(dt) => dt
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
            .to_string(),
        Err(_) => raw.to_string(),
    }
}
