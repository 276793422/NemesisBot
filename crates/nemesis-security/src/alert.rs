//! #53 安全事件流（W3 批一，2026-11-20）：统一发射漏斗 + 规则路由 +
//! 双出口（SSE 由 gateway 注册 / webhook 内建直发）+ 防风暴三闸 + jsonl
//! 落盘。上游真相源：`docs/PLAN/2026-11-20_安全图谱W3运行时运营批计划.md`
//! §二；上游评估：`docs/REPORT/2026-11-19_智能体安全能力图谱对照评估.md` #53。
//!
//! ## 架构（为什么是静态注册表）
//!
//! 产生点横跨四个 crate（security 管线内部 / agent estop+turn_guard /
//! bot 审批+记忆闸+MCP / web MCP handler），逐点注入 Arc 会把装配面撕开
//! 一道口子。这里沿用 nemesis-utils `degraded.rs` 的**进程级静态注册表**
//! 纪律：纯内存、parking_lot（无中毒语义——emit 在安全拦截路径上被调用，
//! 锁绝不能把拦截路径变成 panic 源）、fire-and-forget（告警链路故障不得
//! 反噬主管线）。gateway 启动时 [`configure`] + [`register_outlet`]（SSE）；
//! 各产生点 [`emit`]；gateway 降级泵旁挂 [`take_pending_digests`] 水泵。
//!
//! ## 数据流
//!
//! ```text
//! 产生点 emit(kind, level, session, tool, summary)
//!   → 规则路由（eval_rules 求值原语；rules 空 = 内置 critical 全量）
//!   → cooldown 闸（同 (rule, session) 缺省 60s）
//!   → jsonl 落盘（<dir>/<YYYY-MM-DD>.jsonl，按日轮转；写失败走
//!     nemesis-utils degraded 登记 → /ready + system-alert SSE）
//!   → 各出口 budget 闸（每出口每分钟缺省 20，超出累计 digest）
//!   → 派发：SSE（gateway 注册闭包）+ webhook（内建直发，5s 超时
//!     失败即弃不重试）
//! ```
//!
//! ## 已知边界（诚实声明）
//!
//! - webhook 在无 tokio runtime 的调用线程上静默跳过（`Handle::try_current`
//!   失败即弃——生产 gateway 恒有 runtime；同步上下文（如托盘线程）只有
//!   jsonl + tracing 面）。
//! - 事件历史面 = jsonl 文件本身（EventHub 内存环形重启即丢是既有 L2
//!   边界）；#60 态势页历史卡直接读 jsonl。
//! - 规则动作词表批一只有 `"alert"`；未知动作装配期 WARN 剥除，批二 #56
//!   处置动作在此接力。

use nemesis_config::eval_rules::{Condition, get_field, match_value};
use nemesis_config::{AlertRuleConfig, SecurityAlertsConfig};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// 事件类型与级别
// ---------------------------------------------------------------------------

/// 告警级别（与安全四档风险级别同词表；`PartialOrd` 按声明序 = 严重度序）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AlertLevel {
    Low,
    Medium,
    High,
    Critical,
}

impl AlertLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            AlertLevel::Low => "low",
            AlertLevel::Medium => "medium",
            AlertLevel::High => "high",
            AlertLevel::Critical => "critical",
        }
    }

    /// 解析（规则 JSON 匹配用小写；安全管线 danger 字段用大写——两者都收）。
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "low" => Some(AlertLevel::Low),
            "medium" | "warn" => Some(AlertLevel::Medium),
            "high" => Some(AlertLevel::High),
            "critical" => Some(AlertLevel::Critical),
            _ => None,
        }
    }
}

/// 事件类型枚举（批一定稿；批二 #12/#13 的两个变体为**预留注册位**——
/// 枚举先行只扩不改，路由器零变更，见计划 §九.6）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertKind {
    /// 安全管线拦截（①–⑦层 deny 漏斗 = `log_audit_event`）。
    PipelineIntercept,
    /// 出站复扫/聊天面脱敏命中（审计链台账 `append_direct_audit_event`）。
    OutboundRescanHit,
    /// 审批拒绝（Web/Channel ApprovalManager）。
    ApprovalDenied,
    /// 审批超时自动拒绝。
    ApprovalTimeout,
    /// 循环治理熔断/硬停（turn_guard 终级停车）。
    LoopGovernance,
    /// 急停触发。
    EstopTriggered,
    /// 急停释放。
    EstopReleased,
    /// 记忆投毒隔离入区（W2 #47）。
    MemoryQuarantine,
    /// MCP 装前扫描风险（W2 #14，非 Allow 裁决）。
    McpPreinstallRisk,
    /// 审计链验证失败（篡改信号，CRITICAL）。
    ChainVerifyFailed,
    /// 批二 #12 预留：环境基线偏离。
    BaselineDrift,
    /// 批二 #13 预留：模型端点退化。
    ModelEndpointDegradation,
}

impl AlertKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            AlertKind::PipelineIntercept => "pipeline_intercept",
            AlertKind::OutboundRescanHit => "outbound_rescan_hit",
            AlertKind::ApprovalDenied => "approval_denied",
            AlertKind::ApprovalTimeout => "approval_timeout",
            AlertKind::LoopGovernance => "loop_governance",
            AlertKind::EstopTriggered => "estop_triggered",
            AlertKind::EstopReleased => "estop_released",
            AlertKind::MemoryQuarantine => "memory_quarantine",
            AlertKind::McpPreinstallRisk => "mcp_preinstall_risk",
            AlertKind::ChainVerifyFailed => "chain_verify_failed",
            AlertKind::BaselineDrift => "baseline_drift",
            AlertKind::ModelEndpointDegradation => "model_endpoint_degradation",
        }
    }
}

/// 一条安全告警事件（规则匹配记录 + jsonl 行 + 出口 envelope 三面同形）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SecurityAlertEvent {
    pub kind: String,
    pub level: String,
    /// epoch 毫秒。
    pub ts_ms: u64,
    /// 会话键（管线拦截等无会话语境的事件为空串）。
    pub session_key: String,
    /// 工具名/操作面（无则空串）。
    pub tool: String,
    pub summary: String,
}

impl SecurityAlertEvent {
    fn now(
        kind: AlertKind,
        level: AlertLevel,
        session_key: &str,
        tool: &str,
        summary: String,
    ) -> Self {
        Self {
            kind: kind.as_str().to_string(),
            level: level.as_str().to_string(),
            ts_ms: now_ms(),
            session_key: session_key.to_string(),
            tool: tool.to_string(),
            summary,
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Hub 状态（进程级静态注册表，degraded.rs 同纪律）
// ---------------------------------------------------------------------------

const DEFAULT_COOLDOWN: Duration = Duration::from_secs(60);
const DEFAULT_BUDGET_PER_MIN: u32 = 20;
/// 内置规则（rules 空 / 未配置时启用）：critical 级全量告警。
const BUILTIN_RULE_ID: &str = "__builtin_critical__";
/// webhook 出口的注册名（budget 计数键）。
const WEBHOOK_OUTLET: &str = "webhook";

struct CompiledRule {
    id: String,
    conditions: Vec<Condition>,
}

struct BudgetWindow {
    window_start: Instant,
    sent: u32,
    suppressed: u32,
    last: Option<SecurityAlertEvent>,
}

struct HubState {
    enabled: bool,
    cooldown: Duration,
    budget_per_min: u32,
    rules: Vec<CompiledRule>,
    /// 出口闭包（gateway 注册 SSE；webhook 由 configure 内建注册）。
    outlets: Vec<(String, Arc<dyn Fn(&SecurityAlertEvent) + Send + Sync>)>,
    /// cooldown 键：(rule_id, session_key) → 上次发射时刻。
    cooldowns: HashMap<(String, String), Instant>,
    /// 每出口预算窗口。
    budgets: HashMap<String, BudgetWindow>,
    /// 窗口滚动时物化的 digest，等水泵取走。
    pending_digests: Vec<AlertDigest>,
    jsonl_dir: Option<PathBuf>,
    webhook_urls: Arc<Vec<String>>,
    webhook_client: Option<reqwest::Client>,
}

impl HubState {
    fn new() -> Self {
        Self {
            enabled: true,
            cooldown: DEFAULT_COOLDOWN,
            budget_per_min: DEFAULT_BUDGET_PER_MIN,
            rules: Vec::new(),
            outlets: Vec::new(),
            cooldowns: HashMap::new(),
            budgets: HashMap::new(),
            pending_digests: Vec::new(),
            jsonl_dir: None,
            webhook_urls: Arc::new(Vec::new()),
            webhook_client: None,
        }
    }
}

static HUB: LazyLock<Mutex<HubState>> = LazyLock::new(|| Mutex::new(HubState::new()));

/// 预算窗口滚动时物化的一条聚合摘要（水泵取走后推 SSE）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct AlertDigest {
    pub outlet: String,
    pub suppressed: u32,
    pub last_kind: String,
    pub last_summary: String,
}

// ---------------------------------------------------------------------------
// 配置与出口注册（gateway 启动时一次性）
// ---------------------------------------------------------------------------

/// 装配告警 hub。重复调用 = 重新装配（热生效语义：下次 emit 用新配置）。
///
/// `jsonl_dir` = `<workspace>/logs/security_alerts`（目录按需创建）。
pub fn configure(cfg: &SecurityAlertsConfig, jsonl_dir: PathBuf) {
    let mut st = HUB.lock();
    st.enabled = cfg.enabled;
    st.cooldown = if cfg.cooldown_secs == 0 {
        DEFAULT_COOLDOWN
    } else {
        Duration::from_secs(cfg.cooldown_secs)
    };
    st.budget_per_min = if cfg.budget_per_min == 0 {
        DEFAULT_BUDGET_PER_MIN
    } else {
        cfg.budget_per_min
    };
    st.cooldowns.clear();
    st.budgets.clear();
    st.pending_digests.clear();
    st.jsonl_dir = Some(jsonl_dir);

    // 规则编译：op 词表外 / 正则编不过 / id 空的规则 WARN 丢弃（不静默
    // 失效——坏规则要么响要么说，不许装死）；动作词表外剥除。
    st.rules = cfg
        .rules
        .iter()
        .filter_map(compile_rule)
        .collect::<Vec<_>>();

    // webhook 出口内建注册（URL 配置层信任，与 model api_base 同语义）。
    st.webhook_urls = Arc::new(cfg.webhooks.clone());
    let has_webhook = !cfg.webhooks.is_empty();
    st.webhook_client = if has_webhook {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .ok()
    } else {
        None
    };
    st.outlets.retain(|(name, _)| name != WEBHOOK_OUTLET);
    if has_webhook {
        let urls = st.webhook_urls.clone();
        let client = st.webhook_client.clone();
        let handler: Arc<dyn Fn(&SecurityAlertEvent) + Send + Sync> =
            Arc::new(move |event: &SecurityAlertEvent| {
                dispatch_webhook(&client, &urls, event);
            });
        st.outlets.push((WEBHOOK_OUTLET.to_string(), handler));
    }
}

/// 编译单条规则；非法规则返回 None（调用点 WARN 丢弃）。
fn compile_rule(r: &AlertRuleConfig) -> Option<CompiledRule> {
    if r.id.trim().is_empty() {
        tracing::warn!("[security-alert] rule with empty id dropped");
        return None;
    }
    if r.when.is_empty() {
        tracing::warn!(
            "[security-alert] rule '{}' has no conditions, dropped",
            r.id
        );
        return None;
    }
    for c in &r.when {
        match c.op.as_str() {
            "equals" | "contains" | "regex" | "exists" | "gt" => {}
            other => {
                tracing::warn!(
                    "[security-alert] rule '{}' condition op '{}' unknown, rule dropped",
                    r.id,
                    other
                );
                return None;
            }
        }
        if c.op == "regex"
            && let Some(pat) = c.value.as_str()
            && regex::Regex::new(pat).is_err()
        {
            tracing::warn!(
                "[security-alert] rule '{}' regex '{}' does not compile, rule dropped",
                r.id,
                pat
            );
            return None;
        }
    }
    for a in &r.actions {
        if a != "alert" {
            // 批二 #56 处置动作在此接力；批一先剥除并留痕。
            tracing::warn!(
                "[security-alert] rule '{}' action '{}' not supported yet, stripped",
                r.id,
                a
            );
        }
    }
    Some(CompiledRule {
        id: r.id.clone(),
        conditions: r.when.clone(),
    })
}

/// 注册具名出口（gateway 注册 SSE 闭包用）。同名覆盖。
pub fn register_outlet(name: &str, handler: Arc<dyn Fn(&SecurityAlertEvent) + Send + Sync>) {
    let mut st = HUB.lock();
    if let Some(slot) = st.outlets.iter_mut().find(|(n, _)| n == name) {
        slot.1 = handler;
    } else {
        st.outlets.push((name.to_string(), handler));
    }
}

// ---------------------------------------------------------------------------
// 发射
// ---------------------------------------------------------------------------

/// 便捷发射入口（产生点用）。fire-and-forget，永不 panic 永不阻塞语义。
pub fn emit(
    kind: AlertKind,
    level: AlertLevel,
    session_key: &str,
    tool: &str,
    summary: impl Into<String>,
) {
    emit_event(SecurityAlertEvent::now(
        kind,
        level,
        session_key,
        tool,
        summary.into(),
    ));
}

/// 完整事件发射（jsonl 回放/测试用）。
pub fn emit_event(event: SecurityAlertEvent) {
    let plan = {
        let mut st = HUB.lock();
        if !st.enabled {
            return;
        }
        // 规则路由（AND 条件组；rules 空 = 内置 critical 全量）。
        let Some(rule_id) = first_matching_rule(&st, &event) else {
            return;
        };
        // cooldown 闸（同 (rule, session)）。
        let key = (rule_id.clone(), event.session_key.clone());
        let due = match st.cooldowns.get(&key) {
            Some(t) => t.elapsed() >= st.cooldown,
            None => true,
        };
        if !due {
            return;
        }
        st.cooldowns.insert(key, Instant::now());
        // 各出口 budget 闸 + 派发计划（锁内只做簿记，锁外派发；出口列表
        // 先快照——budget 簿记需要 &mut，不能在遍历借用里调）。
        let outlet_snapshot: Vec<(String, Arc<dyn Fn(&SecurityAlertEvent) + Send + Sync>)> = st
            .outlets
            .iter()
            .map(|(n, h)| (n.clone(), h.clone()))
            .collect();
        let mut dispatches: Vec<Arc<dyn Fn(&SecurityAlertEvent) + Send + Sync>> = Vec::new();
        for (name, handler) in outlet_snapshot {
            if budget_allows(&mut st, &name, &event) {
                dispatches.push(handler);
            }
        }
        let jsonl_dir = st.jsonl_dir.clone();
        (event, dispatches, jsonl_dir)
    };
    let (event, dispatches, jsonl_dir) = plan;

    // 落盘（先于出口——本地证据面不依赖出口健康）。
    if let Some(dir) = jsonl_dir {
        append_jsonl(&dir, &event);
    }
    tracing::warn!(
        kind = %event.kind,
        level = %event.level,
        session = %event.session_key,
        tool = %event.tool,
        "[security-alert] {}",
        event.summary
    );
    for handler in dispatches {
        handler(&event);
    }
}

/// 规则路由：返回首个命中规则的 id（rules 空 = 内置 critical 全量规则）。
fn first_matching_rule(st: &HubState, event: &SecurityAlertEvent) -> Option<String> {
    if st.rules.is_empty() {
        // 内置规则：level=critical。
        return (event.level == "critical").then(|| BUILTIN_RULE_ID.to_string());
    }
    let record = serde_json::to_value(event).ok()?;
    for rule in &st.rules {
        if rule
            .conditions
            .iter()
            .all(|c| match get_field(&record, &c.field) {
                Some(v) => match_value(c, v),
                // 字段缺失 = 条件不成立（含 exists——无值即无存在）。
                None => false,
            })
        {
            return Some(rule.id.clone());
        }
    }
    None
}

/// 预算闸：窗口滚动时物化 digest（拆「快照→落 digest」两段避免 entry
/// 借用与 pending_digests 冲突）；未超限 = true 并计数。
fn budget_allows(st: &mut HubState, outlet: &str, event: &SecurityAlertEvent) -> bool {
    let now = Instant::now();
    let budget = st.budget_per_min;
    let snapshot = {
        let w = st
            .budgets
            .entry(outlet.to_string())
            .or_insert_with(|| BudgetWindow {
                window_start: now,
                sent: 0,
                suppressed: 0,
                last: None,
            });
        let expired = now.duration_since(w.window_start) >= Duration::from_secs(60);
        // 先快照旧窗口值（BUG-20261121-072：digest 聚合的是翻滚前被压掉的
        // 计数——reset 放在快照前会把 suppressed_before 恒清成 0，emit 驱动
        // 的翻滚永不落 digest），后翻滚。
        let snap = (expired, w.suppressed, w.last.clone());
        if expired {
            w.window_start = now;
            w.sent = 0;
            w.suppressed = 0;
            w.last = None;
        }
        snap
    };
    let (expired, suppressed_before, last_before) = snapshot;
    if expired && suppressed_before > 0 {
        st.pending_digests.push(AlertDigest {
            outlet: outlet.to_string(),
            suppressed: suppressed_before,
            last_kind: last_before
                .as_ref()
                .map(|e| e.kind.clone())
                .unwrap_or_default(),
            last_summary: last_before
                .as_ref()
                .map(|e| e.summary.clone())
                .unwrap_or_default(),
        });
    }
    let w = st.budgets.get_mut(outlet).expect("entry inserted above");
    if w.sent < budget {
        w.sent += 1;
        true
    } else {
        w.suppressed += 1;
        w.last = Some(event.clone());
        false
    }
}

/// 水泵取走物化好的 digest（gateway 降级泵旁挂消费 → SSE）。
/// 取走前先惰性滚动已到期的窗口，让停发后的超限计数也能落 digest。
pub fn take_pending_digests() -> Vec<AlertDigest> {
    let mut st = HUB.lock();
    let now = Instant::now();
    let outlets: Vec<String> = st.budgets.keys().cloned().collect();
    for outlet in outlets {
        let snapshot = st.budgets.get(&outlet).and_then(|w| {
            if now.duration_since(w.window_start) >= Duration::from_secs(60) {
                Some((w.suppressed, w.last.clone()))
            } else {
                None
            }
        });
        if let Some((suppressed, last)) = snapshot {
            if suppressed > 0 {
                st.pending_digests.push(AlertDigest {
                    outlet: outlet.clone(),
                    suppressed,
                    last_kind: last.as_ref().map(|e| e.kind.clone()).unwrap_or_default(),
                    last_summary: last.as_ref().map(|e| e.summary.clone()).unwrap_or_default(),
                });
            }
            if let Some(w) = st.budgets.get_mut(&outlet) {
                w.window_start = now;
                w.sent = 0;
                w.suppressed = 0;
                w.last = None;
            }
        }
    }
    std::mem::take(&mut st.pending_digests)
}

// ---------------------------------------------------------------------------
// 出口：webhook 直发（内建）
// ---------------------------------------------------------------------------

/// webhook 直发：逐 URL POST JSON envelope，5s 超时失败即弃不重试
/// （best-effort——告警链路故障不得反噬主管线）。无 tokio runtime 的
/// 调用线程静默跳过（见模块文档已知边界）。
fn dispatch_webhook(client: &Option<reqwest::Client>, urls: &[String], event: &SecurityAlertEvent) {
    let Some(client) = client else { return };
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let payload = serde_json::to_value(event).unwrap_or(serde_json::Value::Null);
    let client = client.clone();
    let urls: Vec<String> = urls.to_vec();
    handle.spawn(async move {
        for url in &urls {
            let result = client.post(url).json(&payload).send().await;
            if let Err(e) = result {
                // 不重试不升级——单条 WARN 留痕即可（风暴面由 cooldown 闸压）。
                tracing::debug!("[security-alert] webhook {} delivery failed: {}", url, e);
            }
        }
    });
}

// ---------------------------------------------------------------------------
// 出口：jsonl 落盘（按日轮转）
// ---------------------------------------------------------------------------

fn append_jsonl(dir: &PathBuf, event: &SecurityAlertEvent) {
    let date = chrono::Utc::now().format("%Y-%m-%d");
    let path = dir.join(format!("{date}.jsonl"));
    // 目录按需创建（模块文档承诺的形态；首次发射时 logs/security_alerts
    // 尚不存在。失败不提前返回——open 的错误照常走 degraded 报告）。
    let _ = std::fs::create_dir_all(dir);
    let result = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| {
            let mut line = serde_json::to_string(event).unwrap_or_default();
            line.push('\n');
            std::io::Write::write_all(&mut f, line.as_bytes())
        });
    if let Err(e) = result {
        // 写失败不 panic 不升级：走 nemesis-utils degraded 登记表
        // （/ready + system-alert SSE 可见）；登记表自身去抖不会刷爆。
        nemesis_utils::degraded::report(
            "security_alerts_jsonl",
            format!("{}: {}", path.display(), e),
        );
    }
}

// ---------------------------------------------------------------------------
// 测试辅助
// ---------------------------------------------------------------------------

/// 清空 hub 状态（仅测试用）。生产代码禁止调用。
pub fn reset_for_tests() {
    let mut st = HUB.lock();
    *st = HubState::new();
}

/// 当前规则数与出口名（测试断言面）。
pub fn status_for_tests() -> (usize, Vec<String>) {
    let st = HUB.lock();
    (
        st.rules.len(),
        st.outlets.iter().map(|(n, _)| n.clone()).collect(),
    )
}

#[cfg(test)]
mod tests;
