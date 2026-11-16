//! 事件规则层：`<workspace>/config/events.json` 的 schema、加载与校验。
//!
//! 规则形态（项1 计划钦定 + 2026-11-16 事件源扩展）：
//! ```json
//! [{"id": "build-done", "watch": {"kind": "fs", "path": "C:/logs/build",
//!    "glob": "*.log", "content_pattern": "(?i)fatal|panic"},
//!   "deliver": {"session_key": "agent:main:session:<uuid>", "prompt": "构建日志有更新：{paths}",
//!               "cooldown_secs": 60, "merge_window_secs": 5, "max_per_hour": 10,
//!               "enabled": true}},
//!  {"id": "ci-hook", "watch": {"kind": "webhook", "token": "s3cret"},
//!   "deliver": {"session_key": "agent:main:session:<uuid>", "prompt": "外部推送：{payload}"}},
//!  {"id": "build-err", "watch": {"kind": "proc", "job_id": 7, "pattern": "(?i)error|FAILED",
//!    "notify_start": true},
//!   "deliver": {"session_key": "...", "prompt": "后台任务 #{job_id} 命中：{line}"}},
//!  {"id": "cpu-hot", "watch": {"kind": "sys", "metric": "cpu", "above": 90.0, "below": 80.0},
//!   "deliver": {"session_key": "...", "prompt": "{payload}"}},
//!  {"id": "api-down", "watch": {"kind": "probe", "probe_type": "http",
//!    "target": "https://api.example.com/health", "interval_secs": 60, "timeout_secs": 10},
//!   "deliver": {"session_key": "...", "prompt": "{payload}"}}]
//! ```
//!
//! 投递占位符按 kind 分派（deliver.render）：
//! - `fs`：`{count}` / `{path}` / `{paths}`；
//! - `proc`：`{count}` / `{line}`（本轮命中行合并）/ `{job_id}`；
//! - `webhook` / `sys` / `probe`：`{count}` / `{payload}`（外部原文 / 预渲染摘要 /
//!   翻转语句）。
//!
//! 校验纪律（加载期诚实剔除 + warning 收集，不让一条坏规则拖垮全部）：
//! - `id` 非空且唯一（重复保留首条）；
//! - `watch.kind` 放行面：fs/webhook/proc/sys/probe（cron 已砍单——nemesis-cron
//!   已覆盖到点唤醒，events.json 再做 = 同一件事换机制；未知 kind 剔除）；
//! - 各 kind 必填字段见 [`WatchSpec`] 注释；正则加载期编译校验；
//! - `deliver.session_key` 必须是 preset 形态（`agent:` / `agents:` 前缀）——裸会话 id
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

/// 监视源。kind 分派（2026-11-16 事件源扩展计划）：
/// - `"fs"`：文件系统变化（notify 推送）——`path` / `glob` / `debounce_ms` /
///   `content_pattern`（变更文件新增行正则，命中才触发；缺省 = 路径变化即触发）；
/// - `"webhook"`：外部 POST 推送——`token`（规则私有凭据）；
/// - `"proc"`：后台任务输出行匹配——`job_id` + `pattern`（正则）+ `notify_start`；
/// - `"sys"`：系统指标阈值——`metric`（cpu|mem|disk）+ `above` + `below`
///   （滞回回落阈值，缺省 above-10；disk 语义 = 使用率%，可选 `path` 定位挂载点）；
/// - `"probe"`：外部端点存活翻转——`probe_type`（http|tcp|command）+ `target` +
///   `interval_secs`（缺省 60，最小 5）+ `timeout_secs`（缺省 10）；状态翻转触发，
///   首探只定基态。管理员配置面（command 型=本机执行命令，非模型发起）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WatchSpec {
    pub kind: String,
    /// fs：监视根；sys+disk：定位挂载点的任意路径（最长前缀匹配挂载点；
    /// 缺省 = 任一盘过阈即算，取使用率最高盘）；其余 kind 可空。
    #[serde(default)]
    pub path: PathBuf,
    /// 文件名通配（`*` / `?`；对变更条目的 file_name 匹配）。缺省 = 全收。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glob: Option<String>,
    /// 同路径事件去重窗口（毫秒，缺省 500）——编辑器一次保存常发多个事件，
    /// 窗口内同路径重复折叠为 1 次（计数诚实，不虚报「共 N 个」）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debounce_ms: Option<u64>,
    /// fs 可选：变更文件**新增行**正则——新增段命中才触发，不命中静默丢弃
    /// （外部程序写的日志文件过滤；proc 源够不着外部写文件，此字段是唯一机制）。
    /// 首见文件 = 读尾部 64KB 基线窗匹配；此后增量（行边界对齐，残段留给
    /// 下次）；文件截断（len < 已消费 offset）= 重新基线；含 NUL 字节 = 按
    /// 二进制跳过。加载期编译校验。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_pattern: Option<String>,
    /// webhook：规则私有 token（HTTP 调用方凭据；明文存 config，与
    /// credentials.yaml 同级信任，文档声明）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// proc：后台任务 id（background_registry 坐标；spawn 结果里拿）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<u64>,
    /// proc：输出行匹配正则（加载期编译校验）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    /// proc：首次观测到任务运行中时补一条启动通知（缺省 false）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify_start: Option<bool>,
    /// sys：指标名（cpu / mem / disk，使用率口径统一——disk = 已用% 非剩余%）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metric: Option<String>,
    /// sys：触发阈值（使用率 ≥ above 触发一次）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub above: Option<f64>,
    /// sys：滞回回落阈值（< below 才回 normal 可再触发；缺省 above-10）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub below: Option<f64>,
    /// probe：探测形态（http | tcp | command）。http up = 2xx；tcp up = 连接
    /// 成功；command up = 退出码 0。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe_type: Option<String>,
    /// probe：目标（http=完整 URL；tcp=host:port；command=命令行，按空白切
    /// program+args，不经过 shell）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// probe：探测周期秒（缺省 60，最小 5——防把外部端点打成压力测试）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_secs: Option<u64>,
    /// probe：单次探测超时秒（缺省 10）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

/// probe 探测周期下限（秒）——低于此值的 interval 加载期剔除。
pub const PROBE_MIN_INTERVAL_SECS: u64 = 5;

const DEFAULT_PROBE_INTERVAL_SECS: u64 = 60;
const DEFAULT_PROBE_TIMEOUT_SECS: u64 = 10;

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

    /// sys 滞回回落阈值（缺省 above-10，钳 0）。
    pub fn sys_below_or_default(&self) -> Option<f64> {
        match self.below {
            Some(b) => Some(b),
            None => self.above.map(|a| (a - 10.0).max(0.0)),
        }
    }

    /// probe 探测周期（缺省 60s；加载期拦 < [`PROBE_MIN_INTERVAL_SECS`]）。
    pub fn probe_interval_or_default(&self) -> u64 {
        self.interval_secs.unwrap_or(DEFAULT_PROBE_INTERVAL_SECS)
    }

    /// probe 单次超时（缺省 10s）。
    pub fn probe_timeout_or_default(&self) -> u64 {
        self.timeout_secs.unwrap_or(DEFAULT_PROBE_TIMEOUT_SECS)
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
        // kind 分派校验（2026-11-16 事件源扩展：fs/webhook/proc/sys/probe 放行；
        // cron 已砍单——nemesis-cron 已覆盖到点唤醒，不重复做通道；其余未知）。
        let supported = match r.watch.kind.as_str() {
            "fs" => {
                if r.watch.path.as_os_str().is_empty() {
                    out.warnings.push(format!("{tag}: watch.path 为空，剔除"));
                    continue;
                }
                match r.watch.content_pattern.as_deref().map(regex::Regex::new) {
                    Some(Err(e)) => {
                        out.warnings.push(format!(
                            "{tag}: watch.content_pattern 非法正则（{e}），剔除"
                        ));
                        continue;
                    }
                    _ => true,
                }
            }
            "webhook" => {
                if r.watch
                    .token
                    .as_deref()
                    .map(str::trim)
                    .unwrap_or("")
                    .is_empty()
                {
                    out.warnings.push(format!(
                        "{tag}: webhook 规则须 watch.token（调用方凭据），剔除"
                    ));
                    continue;
                }
                true
            }
            "proc" => {
                if r.watch.job_id.is_none() {
                    out.warnings.push(format!(
                        "{tag}: proc 规则须 watch.job_id（后台任务 id），剔除"
                    ));
                    continue;
                }
                match r.watch.pattern.as_deref().map(regex::Regex::new) {
                    None => {
                        out.warnings.push(format!(
                            "{tag}: proc 规则须 watch.pattern（输出行正则），剔除"
                        ));
                        continue;
                    }
                    Some(Err(e)) => {
                        out.warnings
                            .push(format!("{tag}: watch.pattern 非法正则（{e}），剔除"));
                        continue;
                    }
                    Some(Ok(_)) => true,
                }
            }
            "sys" => {
                let metric_ok = matches!(
                    r.watch.metric.as_deref(),
                    Some("cpu") | Some("mem") | Some("disk")
                );
                let above_ok = r.watch.above.is_some_and(|v| (0.0..=100.0).contains(&v));
                if !metric_ok || !above_ok {
                    out.warnings.push(format!(
                        "{tag}: sys 规则须 watch.metric（cpu|mem|disk）+ watch.above（0-100 使用率%），剔除"
                    ));
                    continue;
                }
                if let (Some(above), Some(below)) = (r.watch.above, r.watch.below)
                    && below >= above
                {
                    out.warnings.push(format!(
                        "{tag}: watch.below（{below}）须低于 watch.above（{above}）否则滞回永不回 normal，剔除"
                    ));
                    continue;
                }
                true
            }
            "probe" => {
                let type_ok = matches!(
                    r.watch.probe_type.as_deref(),
                    Some("http") | Some("tcp") | Some("command")
                );
                let target = r.watch.target.as_deref().map(str::trim).unwrap_or("");
                let target_ok = match r.watch.probe_type.as_deref() {
                    Some("http") => target.starts_with("http://") || target.starts_with("https://"),
                    Some("tcp") => target.rsplit_once(':').is_some_and(|(host, port)| {
                        !host.is_empty() && port.parse::<u16>().map(|p| p > 0).unwrap_or(false)
                    }),
                    Some("command") => !target.is_empty(),
                    _ => false,
                };
                let interval = r.watch.interval_secs.unwrap_or(60);
                let interval_ok = interval >= PROBE_MIN_INTERVAL_SECS;
                if !type_ok || !target_ok || !interval_ok {
                    out.warnings.push(format!(
                        "{tag}: probe 规则须 watch.probe_type（http|tcp|command）+ 合法 watch.target（http=URL/tcp=host:port/command=命令行）+ watch.interval_secs ≥ {PROBE_MIN_INTERVAL_SECS}（缺省 60），剔除"
                    ));
                    continue;
                }
                if r.watch.timeout_secs.is_some_and(|t| t == 0) {
                    out.warnings
                        .push(format!("{tag}: watch.timeout_secs=0（永不超时）非法，剔除"));
                    continue;
                }
                true
            }
            "cron" => {
                out.warnings.push(format!(
                    "{tag}: watch.kind=cron 已砍单（nemesis-cron 已覆盖到点唤醒，不重复做通道），剔除"
                ));
                continue;
            }
            other => {
                out.warnings.push(format!(
                    "{tag}: watch.kind={other:?} 未知（fs/webhook/proc/sys 之外），剔除"
                ));
                continue;
            }
        };
        let _ = supported;
        // content_pattern 是 fs 专属过滤字段——挂在别的 kind 上是死字段，
        // 不静默吞（语义混淆），loud 剔除。
        if r.watch.content_pattern.is_some() && r.watch.kind != "fs" {
            out.warnings.push(format!(
                "{tag}: watch.content_pattern 是 fs 专属字段（kind={}），剔除",
                r.watch.kind
            ));
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
