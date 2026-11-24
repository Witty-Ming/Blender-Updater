//! 每日构建源：builder.blender.org 的「归档」JSON 接口。
//!
//! 关键教训：
//! `/download/daily/?format=json&v=2` 只列「当前在产」的少数构建，非 main 分支上
//! 放的其实是已发布的正式版（实测 v53 里是 5.3.0 stable 02f0899d78e3），
//! 而且 file_mtime 是 CDN 同步时间（每天变），和官网 archive 页的构建日期对不上。
//! 因此改用 `/download/daily/archive/?format=json&v=2`：约 764 KB / 1080 条，
//! 是真正的每日构建历史，file_mtime 固定在构建当天。
//!
//! 另一个规律：某个系列一旦发布正式版，该分支的每日构建就停止了
//! （实测 v52/v53 最后一条都是发布前的 candidate，日期 11-09；正式版 build date 是 11-10）。

use crate::model::Build;
use serde::Deserialize;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const ARCHIVE_API: &str = "https://builder.blender.org/download/daily/archive/?format=json&v=2";

/// 归档接口有 764 KB，切换通道 / 多个安装重复检查时没必要重拉
const CACHE_TTL: Duration = Duration::from_secs(180);

#[derive(Deserialize, Debug, Clone)]
pub struct DailyEntry {
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub branch: String,
    #[serde(default)]
    pub hash: String,
    #[serde(default)]
    pub platform: String,
    #[serde(default)]
    pub architecture: String,
    #[serde(default)]
    pub file_name: String,
    #[serde(default)]
    pub file_size: u64,
    #[serde(default)]
    pub file_mtime: i64,
    #[serde(default)]
    pub file_extension: String,
    #[serde(default)]
    pub checksum: String,
    /// 构建周期：stable / candidate / beta / alpha
    #[serde(default)]
    pub release_cycle: String,
    /// 与 release_cycle 同值，老接口只有这个字段
    #[serde(default)]
    pub risk_id: String,
}

type EntryCache = Mutex<Option<(Instant, Arc<Vec<DailyEntry>>)>>;

static ENTRIES: OnceLock<EntryCache> = OnceLock::new();

fn cache() -> &'static EntryCache {
    ENTRIES.get_or_init(|| Mutex::new(None))
}

pub struct DailySource {
    agent: ureq::Agent,
}

impl DailySource {
    pub fn new() -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(15))
            .timeout_read(Duration::from_secs(90))
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) BlenderUpdater/0.3")
            .build();
        Self { agent }
    }

    /// 拉取归档并筛出 Windows x64 zip，按时间从新到旧（带进程内缓存）
    fn entries(&self) -> Result<Arc<Vec<DailyEntry>>, String> {
        if let Ok(g) = cache().lock() {
            if let Some((at, v)) = g.as_ref() {
                if at.elapsed() < CACHE_TTL {
                    return Ok(v.clone());
                }
            }
        }

        let resp = self
            .agent
            .get(ARCHIVE_API)
            .call()
            .map_err(|e| format!("访问每日构建归档失败：{}", e))?;
        let text = resp
            .into_string()
            .map_err(|e| format!("读取响应失败：{}", e))?;
        let mut entries: Vec<DailyEntry> =
            serde_json::from_str(&text).map_err(|e| format!("解析 JSON 失败：{}", e))?;

        entries.retain(|e| {
            e.platform.eq_ignore_ascii_case("windows")
                && (e.architecture.eq_ignore_ascii_case("amd64")
                    || e.architecture.eq_ignore_ascii_case("x86_64"))
                && (e.file_extension.eq_ignore_ascii_case("zip") || e.file_name.ends_with(".zip"))
                && !e.url.is_empty()
        });
        entries.sort_by(|a, b| b.file_mtime.cmp(&a.file_mtime));

        let list = Arc::new(entries);
        if let Ok(mut g) = cache().lock() {
            *g = Some((Instant::now(), list.clone()));
        }
        Ok(list)
    }

    /// 全部 Windows x64 zip 条目，从新到旧
    pub fn list_windows(&self) -> Result<Vec<DailyEntry>, String> {
        Ok((*self.entries()?).clone())
    }

    /// 某个版本线（"5.3"）最新的每日构建。
    /// 优先取该版本线的发布分支（branch=v53）；还没出正式版的版本线
    /// （例如 main 上的 5.4.0-alpha）退回到 main 分支里同系列的构建。
    pub fn for_series(&self, series: &str) -> Result<Option<Build>, String> {
        let list = self.entries()?;
        let want = format!("v{}", series.replace('.', ""));

        if let Some(e) = list.iter().find(|e| e.branch.eq_ignore_ascii_case(&want)) {
            return Ok(Some(to_build(e)));
        }

        let prefix = format!("{}.", series);
        if let Some(e) = list.iter().find(|e| {
            e.branch.eq_ignore_ascii_case("main") && e.version.starts_with(&prefix)
        }) {
            return Ok(Some(to_build(e)));
        }

        Ok(None)
    }

    /// 该版本线是否有每日构建（不下载，仅判断）
    pub fn has_series(&self, series: &str) -> bool {
        self.for_series(series).ok().flatten().is_some()
    }
}

/// 条目的构建周期：release_cycle 与 risk_id 实测同值
fn cycle_of(e: &DailyEntry) -> String {
    let raw = if !e.release_cycle.trim().is_empty() {
        &e.release_cycle
    } else {
        &e.risk_id
    };
    raw.trim().to_lowercase()
}

pub fn to_build(e: &DailyEntry) -> Build {
    // file_mtime 是 UTC 时间戳，转成本地时区（与稳定版通道的日期口径一致）
    let date = chrono::DateTime::from_timestamp(e.file_mtime, 0)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_default();
    Build {
        version: e.version.clone(),
        file_name: e.file_name.clone(),
        url: e.url.clone(),
        mirrors: vec![],
        sha256: e.checksum.to_lowercase(),
        size_bytes: e.file_size,
        date,
        hash: e.hash.clone(),
        branch: e.branch.clone(),
        cycle: cycle_of(e),
    }
}
