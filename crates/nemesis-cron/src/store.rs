//! Cron 存储文件 IO —— CLI（`nemesisbot cron`）与 gateway（`CronService`）
//! 共用的统一定义。
//!
//! 背景（cron-store-format-split-cli-vs-gateway-fix）：
//! CLI 曾自行拼「裸数组」形态写 `jobs.json`，gateway 的 `CronStoreData`
//! envelope（`{"version":1,"jobs":[...]}`）永远解析不了它——CLI 写盘后
//! gateway 启动即 WARN + 空 store，gateway 首次保存又把 CLI 任务整体顶掉。
//! 修复纪律：两边都只走本模块读写，任何一方都不得再自行拼 JSON。
//!
//! 旧 CLI 裸数组形态在这里做一次性迁移（条目字段映射成完整 `CronJob`），
//! 首次保存即落为 canonical envelope——存量文件自动愈合。

use crate::service::{CronJob, CronJobState, CronPayload, CronSchedule};
use chrono::Local;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Canonical store envelope.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CronStoreData {
    pub version: i32,
    pub jobs: Vec<CronJob>,
}

impl CronStoreData {
    /// Fresh empty store at the current schema version.
    pub fn new_v1() -> Self {
        Self {
            version: 1,
            jobs: vec![],
        }
    }
}

/// Load the store file. Missing file = fresh empty store.
pub fn load(path: &Path) -> Result<CronStoreData, String> {
    if !path.exists() {
        return Ok(CronStoreData::new_v1());
    }
    // 走 read_config_string 剥前导 UTF-8 BOM（BUG-20261120-025）：jobs.json
    // 是用户可手工编辑的持久化面（legacy 迁移机制的存在即证明手编 store 在
    // 野外存在），记事本「带 BOM 的 UTF-8」保存形态下裸 read_to_string 的
    // 前导 \u{FEFF} 会让 parse 的 `starts_with('[')` 判定走偏 + serde_json
    // 报 expected value at line 1 → load 失败置毒 → start()/save 全拒 =
    // 全部定时任务停摆，用户照报错删文件即丢任务清单（BUG-20261115-036
    // discipline 臂同构失败链）。read_config_string 是剥 BOM 单一真相源
    // （nemesis-utils fsutil 纪律锚：所有用户可编辑 JSON/TOML 配置读取点
    // 一律走它）；BOM 前缀的 legacy 裸数组迁移同样被掐死，同函数同愈。
    let data =
        nemesis_utils::fsutil::read_config_string(path).map_err(|e| format!("read: {}", e))?;
    parse(&data)
}

/// Parse store text: canonical envelope first; legacy CLI bare-array entries
/// are migrated (one-time) so pre-split files load instead of failing.
pub fn parse(text: &str) -> Result<CronStoreData, String> {
    if text.trim_start().starts_with('[') {
        let legacy: Vec<LegacyJob> =
            serde_json::from_str(text).map_err(|e| format!("parse legacy array: {}", e))?;
        migrate_legacy(legacy)
    } else {
        serde_json::from_str::<CronStoreData>(text).map_err(|e| format!("parse: {}", e))
    }
}

/// Save the store atomically（半截文件 = 任务清单损坏）。
/// 权限 0o600：任务清单可能带用户话术，不比 config 敏感面宽。
pub fn save(path: &Path, data: &CronStoreData) -> Result<(), String> {
    let json = serde_json::to_string_pretty(data).map_err(|e| format!("ser: {}", e))?;
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), json.as_bytes(), 0o600)
}

// ---------------------------------------------------------------------------
// Legacy CLI bare-array migration
// ---------------------------------------------------------------------------

/// 旧 CLI（commands/cron.rs 手拼 JSON 时代）的条目形态。
#[derive(Debug, Deserialize)]
struct LegacyJob {
    id: String,
    name: String,
    message: String,
    schedule: LegacySchedule,
    #[serde(default)]
    deliver: bool,
    #[serde(default)]
    to: Option<String>,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default = "default_true")]
    enabled: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
struct LegacySchedule {
    kind: String,
    every_ms: Option<i64>,
    expr: Option<String>,
}

/// 旧形态 → 完整 `CronJob`。任何一条不可映射即整体报错（带条目定位）：
/// 宁可拒绝也不静默造 job 或丢条目。
fn migrate_legacy(jobs: Vec<LegacyJob>) -> Result<CronStoreData, String> {
    let now_ms = Local::now().timestamp_millis();
    let mut out = Vec::with_capacity(jobs.len());
    for (i, j) in jobs.into_iter().enumerate() {
        let schedule = match j.schedule.kind.as_str() {
            "interval" => {
                let every_ms = j.schedule.every_ms.filter(|&ms| ms > 0).ok_or_else(|| {
                    format!("legacy job #{} ({}): interval 缺 every_ms", i, j.name)
                })?;
                CronSchedule {
                    kind: "every".to_string(),
                    at_ms: None,
                    every_ms: Some(every_ms),
                    expr: None,
                    tz: None,
                }
            }
            "cron" => {
                let expr = j
                    .schedule
                    .expr
                    .clone()
                    .ok_or_else(|| format!("legacy job #{} ({}): cron 缺 expr", i, j.name))?;
                CronSchedule {
                    kind: "cron".to_string(),
                    at_ms: None,
                    every_ms: None,
                    expr: Some(expr),
                    tz: None,
                }
            }
            other => {
                return Err(format!(
                    "legacy job #{} ({}): 不支持的 schedule.kind={}",
                    i, j.name, other
                ));
            }
        };
        out.push(CronJob {
            id: j.id,
            name: j.name,
            enabled: j.enabled,
            schedule,
            payload: CronPayload {
                kind: "agent_turn".to_string(),
                message: j.message,
                command: None,
                deliver: j.deliver,
                channel: j.channel,
                to: j.to,
                session_key: None,
                max_rounds: None,
            },
            state: CronJobState {
                next_run_at_ms: None,
                last_run_at_ms: None,
                last_status: None,
                last_error: None,
                history: Vec::new(),
            },
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
            delete_after_run: false,
        });
    }
    Ok(CronStoreData {
        version: 1,
        jobs: out,
    })
}

#[cfg(test)]
mod tests;
