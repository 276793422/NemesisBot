//! Automations（B12，2026-10-18）：**工具事件触发 workflow** 的注册表 + 匹配泵。
//!
//! 注册表落 `<workspace>/config/automation_triggers.json`（数组，每条
//! `{id, pattern, workflow_id, cooldown_secs, enabled, created_at}`）；
//! `pattern` 是精确工具名（`exec`）或 `*` 结尾前缀通配（`mcp_*`）。
//!
//! 三个部件：
//! - **WSAPI**（`automations.list/set/remove`）：注册表 CRUD，写盘后刷新
//!   进程缓存（gateway 同进程热生效，无需重启）。
//! - **进程缓存**（`TRIGGER_CACHE`）：静态槽先例 =
//!   `plugins_wasm::set_tool_sync_hook`（AppState 字面量散布测试文件太广，
//!   新增字段是全库爆破——同款 OnceLock/RwLock 模块级快照裁决）。泵每次
//!   事件读缓存，零 IO（「匹配不上零开销」验收线）。
//! - **事件泵**（`spawn_event_pump`）：gateway 侧一次性任务，订阅 agent
//!   事件广播，`ToolStarted` 匹配 → 冷却闸 → fire-and-forget
//!   `engine.start_async` 起跑 workflow。
//!
//! 防自我循环（re-entrancy）：workflow 的 `agent` 节点把会话键命名空间在
//! `workflow:{agent_id}` 下（agent_build 装配注释），泵对 `workflow:` 前缀
//! 会话的工具事件直接跳过——否则「exec 触发的 workflow 又调 exec」会每轮
//! 自触发。冷却（默认 60s）是第二道闸：纯内存 `HashMap<trigger_id, Instant>`，
//! **重启即清**（防风暴是瞬时语义，不落盘；诚实边界）。
//!
//! feature 闸：随 `workflow` feature 进出（引擎不在则规则永不触发，诚实
//! 裁剪比留死配置好）。estop 不直接接泵——冻结面在 workflow 引擎自身的
//! agent 节点执行（安全 8 层同源），泵只做匹配与起跑。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::ws_router::{ModuleHandler, RequestContext};

/// 注册表文件名（`<workspace>/config/` 下，与 hooks.json 同目录约定）。
const REGISTRY_FILE: &str = "automation_triggers.json";
/// 缺省冷却秒数（同 pattern 高频工具调用的防风暴底线）。
const DEFAULT_COOLDOWN_SECS: u64 = 60;

/// 一条自动化触发规则。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AutomationTrigger {
    /// 稳定标识（upsert 键；缺省由服务端生成 `at-<hex8>`）。
    pub id: String,
    /// 精确工具名或 `*` 结尾前缀通配（`exec` / `mcp_*`）。
    pub pattern: String,
    /// 要起跑的 workflow 名（`engine.start_async` 按名查找）。
    pub workflow_id: String,
    /// 同规则两次触发之间的最小间隔（秒）。
    pub cooldown_secs: u64,
    /// 关闭的规则保留在注册表里（enabled=false 不触发不删除）。
    pub enabled: bool,
    /// 创建时间（毫秒时间戳；upsert 更新其他字段时保留原值）。
    pub created_at: u64,
}

// ---------------------------------------------------------------------------
// Registry file + process cache
// ---------------------------------------------------------------------------

/// 注册表路径（`<workspace>/config/automation_triggers.json`）。
pub(crate) fn registry_path(workspace: &Path) -> PathBuf {
    nemesis_path::workspace_config_dir(workspace).join(REGISTRY_FILE)
}

/// 读注册表文件。缺文件 = 空表（fresh 常态，不报错）；坏 JSON = 诚实报错
/// （静默当空表会让用户以为规则还在生效）。
pub(crate) fn load_triggers_from_file(path: &Path) -> Result<Vec<AutomationTrigger>, String> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("failed to read {}: {e}", path.display())),
    };
    serde_json::from_str(&content)
        .map_err(|e| format!("automation_triggers.json 解析失败（请修正后重试）: {e}"))
}

/// 原子写注册表（统一走 `nemesis_utils::write_file_atomic`，同 hooks.json）。
pub(crate) fn save_triggers(path: &Path, triggers: &[AutomationTrigger]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("failed to create config dir: {e}"))?;
    }
    let content = serde_json::to_string_pretty(triggers)
        .map_err(|e| format!("failed to serialize triggers: {e}"))?;
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), content.as_bytes(), 0o600)
        .map_err(|e| format!("failed to write {REGISTRY_FILE}: {e}"))
}

/// 进程级触发器缓存（泵读、WSAPI 写）。静态槽裁决同 handlers/mod.rs
/// COMMANDS_REGISTRY：AppState 字面量散布全库测试文件，新增字段是爆破半径。
static TRIGGER_CACHE: std::sync::LazyLock<parking_lot::RwLock<Arc<Vec<AutomationTrigger>>>> =
    std::sync::LazyLock::new(|| parking_lot::RwLock::new(Arc::new(Vec::new())));

/// 泵用的当前触发器集（Arc 快照读，锁内仅 clone 指针）。
pub fn cached_triggers() -> Arc<Vec<AutomationTrigger>> {
    TRIGGER_CACHE.read().clone()
}

/// 从给定触发器集刷新缓存（WSAPI 写后 / gateway 启动加载共用）。
pub(crate) fn refresh_cache_from(triggers: Vec<AutomationTrigger>) {
    *TRIGGER_CACHE.write() = Arc::new(triggers);
}

/// 加载注册表并刷新进程缓存；返回加载条数。gateway 启动 / WSAPI set·remove
/// 写盘后共用（同进程热生效）。
pub fn load_and_cache(workspace: &Path) -> Result<usize, String> {
    let triggers = load_triggers_from_file(&registry_path(workspace))?;
    let n = triggers.len();
    refresh_cache_from(triggers);
    Ok(n)
}

// ---------------------------------------------------------------------------
// Matching core（纯函数，单测面）
// ---------------------------------------------------------------------------

/// pattern 匹配：`*` 结尾 = 前缀通配（`mcp_*` 命中 `mcp_fetch`、裸 `*` 命中
/// 一切）；否则精确相等。
pub(crate) fn pattern_matches(pattern: &str, tool: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => tool.starts_with(prefix),
        None => pattern == tool,
    }
}

/// pattern 合法性：非空；`*` 只允许结尾一个（中间星号/多个星号拒绝——
/// 匹配语义刻意收窄，不猜用户想表达什么）。
pub(crate) fn validate_pattern(pattern: &str) -> Result<(), String> {
    if pattern.trim().is_empty() {
        return Err("pattern 不能为空".to_string());
    }
    if pattern.contains('*') && !pattern.ends_with('*') {
        return Err("pattern 通配符只允许结尾 `*`（如 mcp_*）".to_string());
    }
    if pattern.chars().filter(|c| *c == '*').count() > 1 {
        return Err("pattern 只允许一个通配符 `*`".to_string());
    }
    Ok(())
}

/// 冷却闸：按 trigger_id 记录上次触发时刻，`cooldown_secs` 内的再次触发
/// 被抑制。纯内存（重启即清 = 防风暴瞬时语义，见模块头注）。
#[derive(Default)]
pub(crate) struct CooldownGate {
    last_fired: HashMap<String, Instant>,
}

impl CooldownGate {
    /// 可触发则记账并返回 true；冷却中返回 false。
    pub(crate) fn check_and_mark(
        &mut self,
        trigger_id: &str,
        cooldown_secs: u64,
        now: Instant,
    ) -> bool {
        if cooldown_secs > 0
            && let Some(&t) = self.last_fired.get(trigger_id)
            && now.duration_since(t) < Duration::from_secs(cooldown_secs)
        {
            return false;
        }
        self.last_fired.insert(trigger_id.to_string(), now);
        true
    }
}

/// 泵匹配核：给定事件工具名，从缓存选出本次应触发的规则（enabled 过滤 →
/// pattern 匹配 → 冷却闸记账）。抽出纯函数是为了单测不碰 broadcast/engine。
pub(crate) fn select_firing(
    triggers: &[AutomationTrigger],
    tool: &str,
    gate: &mut CooldownGate,
    now: Instant,
) -> Vec<AutomationTrigger> {
    triggers
        .iter()
        .filter(|t| t.enabled && pattern_matches(&t.pattern, tool))
        .filter(|t| gate.check_and_mark(&t.id, t.cooldown_secs, now))
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------
// Event pump（gateway 侧一次性任务）
// ---------------------------------------------------------------------------

/// 订阅 agent 事件广播的工具事件泵。manager 级一次性任务（不随 agent 重启
/// 泄）；广播 sender 全部 drop（gateway 收尾）→ `recv()` Closed → 任务退出。
/// 返回 JoinHandle 供测试等待（gateway 忽略返回值）。
pub fn spawn_event_pump(
    mut rx: tokio::sync::broadcast::Receiver<nemesis_types::agent::AgentEvent>,
    engine: Arc<nemesis_workflow::engine::WorkflowEngine>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut gate = CooldownGate::default();
        loop {
            let ev = match rx.recv().await {
                Ok(ev) => ev,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("[Automations] event pump lagged, {n} event(s) dropped");
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            };
            let (session_key, call_id, tool) = match &ev {
                nemesis_types::agent::AgentEvent::ToolStarted {
                    session_key,
                    call_id,
                    tool,
                    ..
                } => (session_key.clone(), call_id.clone(), tool.clone()),
                _ => continue,
            };
            // 防自我循环：workflow agent 节点会话（workflow:*）触发的工具事件
            // 不回触发（否则 exec 触发的 workflow 再调 exec = 每轮自触发）。
            if session_key.starts_with("workflow:") {
                continue;
            }
            let triggers = cached_triggers();
            if triggers.is_empty() {
                continue;
            }
            for t in select_firing(&triggers, &tool, &mut gate, Instant::now()) {
                let input: HashMap<String, serde_json::Value> = HashMap::from([
                    ("trigger_id".to_string(), serde_json::json!(t.id)),
                    ("tool".to_string(), serde_json::json!(tool)),
                    ("session_key".to_string(), serde_json::json!(session_key)),
                    ("call_id".to_string(), serde_json::json!(call_id)),
                ]);
                let snapshot = serde_json::json!({
                    "tool": tool,
                    "session_key": session_key,
                    "call_id": call_id,
                });
                let eng = engine.clone();
                let trigger_id = t.id.clone();
                let workflow_id = t.workflow_id.clone();
                tracing::info!(
                    "[Automations] trigger '{}' (tool={tool}) -> workflow '{workflow_id}'",
                    trigger_id
                );
                tokio::spawn(async move {
                    match eng
                        .start_async(
                            &workflow_id,
                            input,
                            Some(nemesis_workflow::types::TriggerSource::Event {
                                event_type: format!("automation.{trigger_id}"),
                                data: snapshot,
                            }),
                        )
                        .await
                    {
                        Ok(execution_id) => tracing::info!(
                            "[Automations] workflow '{workflow_id}' started (execution {execution_id})"
                        ),
                        Err(e) => tracing::warn!(
                            "[Automations] workflow '{workflow_id}' start failed: {e}"
                        ),
                    }
                });
            }
        }
    })
}

// ---------------------------------------------------------------------------
// WSAPI handler
// ---------------------------------------------------------------------------

/// `automations.*` WSAPI（module: "automations"）。
///
/// - `list`：读盘返回全量（真相源是文件，缓存只服务泵）。
/// - `set`：upsert（`id` 缺省生成）；`pattern`/`workflow_id` 非空校验 +
///   pattern 语法校验 + workflow 存在性校验（engine 在场才校验，headless
///   形态诚实跳过）；写盘后刷新进程缓存。
/// - `remove`：按 id 删除，不存在 = 诚实报错。
pub struct AutomationsHandler;

#[async_trait::async_trait]
impl ModuleHandler for AutomationsHandler {
    fn module_name(&self) -> &str {
        "automations"
    }

    fn commands(&self) -> &'static [&'static str] {
        &["list", "set", "remove"]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let workspace = crate::handlers::require_workspace(ctx)?;
        match cmd {
            "list" => self.list(workspace),
            "set" => {
                let data = data.ok_or("missing data")?;
                self.set(workspace, &data, ctx)
            }
            "remove" => {
                let data = data.ok_or("missing data")?;
                self.remove(workspace, &data)
            }
            _ => Err(format!("unknown command: automations.{cmd}")),
        }
    }
}

impl AutomationsHandler {
    pub(crate) fn list(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let triggers = load_triggers_from_file(&registry_path(Path::new(workspace)))?;
        let count = triggers.len();
        Ok(Some(serde_json::json!({
            "triggers": triggers,
            "count": count,
        })))
    }

    pub(crate) fn set(
        &self,
        workspace: &str,
        data: &serde_json::Value,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let pattern = crate::handlers::get_str(data, "pattern")?;
        validate_pattern(&pattern)?;
        let workflow_id = crate::handlers::get_str(data, "workflow_id")?;
        if workflow_id.trim().is_empty() {
            return Err("workflow_id 不能为空".to_string());
        }
        // workflow 存在性校验（engine 在场才校验——headless/测试形态无引擎
        // 时诚实跳过，起跑时 start_async 还会再报一次 workflow_not_found）。
        if let Some(engine) = ctx.state.workflow_engine.as_ref()
            && engine.get_workflow(&workflow_id).is_none()
        {
            return Err(format!("workflow 不存在: {workflow_id}"));
        }
        let cooldown_secs = crate::handlers::get_opt_u64_loud(data, "cooldown_secs")?
            .unwrap_or(DEFAULT_COOLDOWN_SECS);
        let enabled = crate::handlers::get_opt_bool_loud(data, "enabled")?.unwrap_or(true);
        let id = match crate::handlers::get_opt_str(data, "id") {
            Some(id) if !id.trim().is_empty() => id,
            _ => generate_trigger_id(),
        };

        let path = registry_path(Path::new(workspace));
        let mut triggers = load_triggers_from_file(&path)?;
        match triggers.iter_mut().find(|t| t.id == id) {
            Some(t) => {
                // upsert：created_at 保留原值（身份字段不因编辑漂移）。
                t.pattern = pattern;
                t.workflow_id = workflow_id;
                t.cooldown_secs = cooldown_secs;
                t.enabled = enabled;
            }
            None => triggers.push(AutomationTrigger {
                id: id.clone(),
                pattern,
                workflow_id,
                cooldown_secs,
                enabled,
                created_at: now_millis(),
            }),
        }
        save_triggers(&path, &triggers)?;
        let count = triggers.len();
        refresh_cache_from(triggers);
        Ok(Some(serde_json::json!({
            "id": id,
            "saved": true,
            "count": count,
        })))
    }

    pub(crate) fn remove(
        &self,
        workspace: &str,
        data: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
        let id = crate::handlers::get_str(data, "id")?;
        let path = registry_path(Path::new(workspace));
        let mut triggers = load_triggers_from_file(&path)?;
        let before = triggers.len();
        triggers.retain(|t| t.id != id);
        if triggers.len() == before {
            return Err(format!("automation 触发器不存在: {id}"));
        }
        save_triggers(&path, &triggers)?;
        let count = triggers.len();
        refresh_cache_from(triggers);
        Ok(Some(serde_json::json!({
            "removed": id,
            "count": count,
        })))
    }
}

/// `at-<hex8>` 短 id（uuid v4 simple 前 8 位；碰撞面 = 注册表人工规模，
/// 够用；真撞了 upsert 键重名会静默覆盖既有条目——概率上可忽略）。
fn generate_trigger_id() -> String {
    format!("at-{}", &uuid::Uuid::new_v4().simple().to_string()[..8])
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests;
