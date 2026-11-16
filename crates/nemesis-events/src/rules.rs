//! 事件规则层：`<workspace>/config/events.json` 的 schema、加载与校验。
//!
//! 规则形态（项1 计划钦定）：
//! ```json
//! [{"id": "build-done", "watch": {"kind": "fs", "path": "C:/logs/build",
//!    "glob": "*.log", "debounce_ms": 500},
//!   "deliver": {"session_key": "agent:main:session:<uuid>", "prompt": "构建日志有更新：{paths}",
//!               "cooldown_secs": 60, "merge_window_secs": 5, "max_per_hour": 10,
//!               "enabled": true}}]
//! ```
//!
//! 校验纪律（加载期诚实剔除 + warning 收集，不让一条坏规则拖垮全部）：
//! - `id` 非空且唯一（重复保留首条）；
//! - `watch.kind` 仅支持 `"fs"`（其余 kind 是 P3 事件源，加载期剔除留痕）；
//! - `watch.path` / `deliver.session_key` / `deliver.prompt` 非空；
//! - `session_key` 必须是 preset 形态（`agent:` / `agents:` 前缀）——裸会话 id
//!   会被 admission 路由进默认主会话（静默错路由），加载期直接拒绝。

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// 单条事件唤醒规则（events.json 数组元素）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRule {
    pub id: String,
    pub watch: WatchSpec,
    pub deliver: DeliverSpec,
}

/// 监视源。P1 仅 `kind: "fs"`；`webhook` / `probe` 等是 P3 事件源，
/// 加载期诚实剔除并留 warning。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchSpec {
    pub kind: String,
    pub path: PathBuf,
    /// 文件名通配（`*` / `?`；对变更条目的 file_name 匹配）。缺省 = 全收。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glob: Option<String>,
    /// 同路径事件去重窗口（毫秒，缺省 500）——编辑器一次保存常发多个事件，
    /// 窗口内同路径重复折叠为 1 次（计数诚实，不虚报「共 N 个」）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debounce_ms: Option<u64>,
}

/// 投递目标与防风暴参数。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliverSpec {
    /// 目标会话（必须 preset 形态 `agent:main:session:<id>`）。
    pub session_key: String,
    /// 注入模板；占位符 `{count}` / `{path}` / `{paths}`。
    pub prompt: String,
    /// 冷却秒数（缺省 60；0=关）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cooldown_secs: Option<u64>,
    /// burst 合并静默窗（秒，缺省 5；0=立即投）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_window_secs: Option<u64>,
    /// per-rule 每小时投递上限（缺省 10；0=不限）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_per_hour: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

const DEFAULT_DEBOUNCE_MS: u64 = 500;
const DEFAULT_COOLDOWN_SECS: u64 = 60;
const DEFAULT_MERGE_WINDOW_SECS: u64 = 5;
const DEFAULT_MAX_PER_HOUR: u32 = 10;

impl WatchSpec {
    pub fn debounce_ms_or_default(&self) -> u64 {
        self.debounce_ms.unwrap_or(DEFAULT_DEBOUNCE_MS)
    }
}

impl DeliverSpec {
    pub fn cooldown_secs_or_default(&self) -> u64 {
        self.cooldown_secs.unwrap_or(DEFAULT_COOLDOWN_SECS)
    }
    pub fn merge_window_secs_or_default(&self) -> u64 {
        self.merge_window_secs.unwrap_or(DEFAULT_MERGE_WINDOW_SECS)
    }
    pub fn max_per_hour_or_default(&self) -> u32 {
        self.max_per_hour.unwrap_or(DEFAULT_MAX_PER_HOUR)
    }
    pub fn enabled_or_default(&self) -> bool {
        self.enabled.unwrap_or(true)
    }
}

/// 事件规则是否命中一条 fs 变更路径：监视根（component 级前缀）+ 文件名 glob。
///
/// glob 只对 file_name 匹配（`*` 不跨路径分隔符的语义由此天然成立）；
/// 无 glob = 全收。
pub fn rule_matches(rule: &EventRule, path: &Path) -> bool {
    if rule.watch.kind != "fs" {
        return false;
    }
    let root = Path::new(&rule.watch.path);
    if !path.starts_with(root) {
        return false;
    }
    match rule.watch.glob.as_deref() {
        None => true,
        Some(pattern) => path
            .file_name()
            .map(|n| super::glob::wildcard_match(pattern, &n.to_string_lossy()))
            .unwrap_or(false),
    }
}

/// 加载结果：有效规则 + 剔除/异常 warning 明细（调用方决定怎么留痕）。
#[derive(Debug, Default, Clone)]
pub struct LoadedRules {
    pub rules: Vec<EventRule>,
    pub warnings: Vec<String>,
}

/// 加载 + 校验。文件不存在 = 空规则集（首次启动正常态，非错误）；
/// 读取/解析失败 = Err（调用方实施「保留旧规则」的重载策略）。
pub fn load_rules(path: &Path) -> Result<LoadedRules, String> {
    if !path.exists() {
        return Ok(LoadedRules::default());
    }
    let raw = std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let parsed: Vec<EventRule> =
        serde_json::from_str(&raw).map_err(|e| format!("parse {}: {e}", path.display()))?;
    Ok(validate(parsed))
}

/// 校验（去重 id / kind 白名单 / 非空必填 / session_key preset 形态），
/// 坏规则剔除并收集 warning，好规则全部保留。
pub fn validate(rules: Vec<EventRule>) -> LoadedRules {
    let mut out = LoadedRules::default();
    let mut seen = HashSet::new();
    for (idx, r) in rules.into_iter().enumerate() {
        let tag = format!("rules[{idx}] (id={:?})", r.id);
        if r.id.trim().is_empty() {
            out.warnings.push(format!("{tag}: id 为空，剔除"));
            continue;
        }
        if !seen.insert(r.id.clone()) {
            out.warnings
                .push(format!("{tag}: id 重复，保留首条，本条剔除"));
            continue;
        }
        if r.watch.kind != "fs" {
            out.warnings.push(format!(
                "{tag}: watch.kind={:?} 暂不支持（P1 仅 fs；webhook/探针是 P3 事件源），剔除",
                r.watch.kind
            ));
            continue;
        }
        if r.watch.path.as_os_str().is_empty() {
            out.warnings.push(format!("{tag}: watch.path 为空，剔除"));
            continue;
        }
        if r.deliver.session_key.trim().is_empty() {
            out.warnings
                .push(format!("{tag}: deliver.session_key 为空，剔除"));
            continue;
        }
        if !r.deliver.session_key.starts_with("agent:")
            && !r.deliver.session_key.starts_with("agents:")
        {
            out.warnings.push(format!(
                "{tag}: session_key={:?} 非 preset 形态（须 agent:/agents: 前缀，裸 id 会错路由进默认会话），剔除",
                r.deliver.session_key
            ));
            continue;
        }
        if r.deliver.prompt.trim().is_empty() {
            out.warnings
                .push(format!("{tag}: deliver.prompt 为空，剔除"));
            continue;
        }
        out.rules.push(r);
    }
    out
}

/// 保存（美化 JSON）。测试与 P2 管理面共用。
pub fn save_rules(path: &Path, rules: &[EventRule]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(rules).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| format!("write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests;
