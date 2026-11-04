// ---------------------------------------------------------------------------
// 2026-11-05 机械拆分：本文件内容由 loop_tools.rs 原样搬入（纯代码搬家，行为零变更）。
// ---------------------------------------------------------------------------

use super::*;

// ===========================================================================
// Bootstrap completion tool
// ===========================================================================

/// A tool that completes the bootstrap process by deleting BOOTSTRAP.md.
///
/// Mirrors Go's `CompleteBootstrapTool`. Requires `confirmed: true` to proceed.
pub struct BootstrapTool {
    workspace: String,
}

impl BootstrapTool {
    /// Create a new bootstrap tool with the workspace path.
    pub fn new(workspace: &str) -> Self {
        Self {
            workspace: workspace.to_string(),
        }
    }
}

#[async_trait]
impl Tool for BootstrapTool {
    fn description(&self) -> String {
        "Complete the bootstrap initialization by deleting BOOTSTRAP.md. Must confirm all initialization steps are done first.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "confirmed": {
                    "type": "boolean",
                    "description": "Confirm that initialization is complete and ready to delete BOOTSTRAP.md"
                }
            },
            "required": ["confirmed"]
        })
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let val: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid arguments: {}", e))?;

        let confirmed = val
            .get("confirmed")
            .and_then(|v| v.as_bool())
            .ok_or("Missing or invalid 'confirmed' parameter (must be a boolean)")?;

        if !confirmed {
            return Err(
                "Must confirm initialization is complete before deleting bootstrap file."
                    .to_string(),
            );
        }

        let bootstrap_path = Path::new(&self.workspace).join("BOOTSTRAP.md");

        if !bootstrap_path.exists() {
            return Ok(
                "BOOTSTRAP.md has already been removed. Initialization is complete.".to_string(),
            );
        }

        match tokio::fs::remove_file(&bootstrap_path).await {
            Ok(()) => Ok("Bootstrap initialization complete! BOOTSTRAP.md has been deleted. The system will load configuration files on next startup.".to_string()),
            Err(e) => Err(format!("Failed to delete BOOTSTRAP.md: {}", e)),
        }
    }
}

// ===========================================================================
// Cron tool
// ===========================================================================

/// A tool that manages cron jobs for scheduling tasks.
///
/// Mirrors Go's `CronTool` which is registered as "cron" in the agent.
pub struct CronTool {
    service: Arc<std::sync::Mutex<nemesis_cron::service::CronService>>,
    channel: Arc<std::sync::Mutex<String>>,
    chat_id: Arc<std::sync::Mutex<String>>,
}

impl CronTool {
    /// Create a new cron tool with the given cron service.
    pub fn new(service: Arc<std::sync::Mutex<nemesis_cron::service::CronService>>) -> Self {
        Self {
            service,
            channel: Arc::new(std::sync::Mutex::new(String::new())),
            chat_id: Arc::new(std::sync::Mutex::new(String::new())),
        }
    }
}

/// Convert a whole-second duration from tool args to milliseconds
/// (overflow-honest; 换算单一真相源在 [`nemesis_cron::secs_to_ms_checked`]).
fn secs_arg_to_ms(secs: u64) -> Result<i64, String> {
    nemesis_cron::secs_to_ms_checked(secs)
}

#[async_trait]
impl Tool for CronTool {
    fn description(&self) -> String {
        "Schedule reminders and recurring tasks. For create, provide exactly ONE of at_seconds (one-time, fires N seconds from now), every_seconds (recurring interval), or cron_expr (5-field cron expression), plus a message to deliver when the job fires. Use list to get job ids; delete with an id to remove.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"action":{"type":"string","enum":["create","delete","list"],"description":"create = schedule a job; delete = remove a job by id; list = show all jobs with their ids"},"name":{"type":"string","description":"create: optional job name (default: unnamed)"},"at_seconds":{"type":"integer","description":"create (one-time): fire N seconds from now"},"every_seconds":{"type":"integer","description":"create (recurring): fire every N seconds"},"cron_expr":{"type":"string","description":"create (recurring): 5-field cron expression, e.g. \"0 9 * * *\""},"message":{"type":"string","description":"create: reminder message delivered to the conversation when the job fires"},"continue_session":{"type":"boolean","description":"true = when the job fires, continue THIS conversation's session (context/persona preserved, reply lands in this chat history). false (default) = fresh session at fire time."},"max_rounds":{"type":"integer","enum":[5,10,20],"default":10,"description":"Per-fire tool-round budget when continue_session=true: the fired turn stops gracefully after this many tool rounds (work is saved; the job is NOT deleted — next fire re-budgets). Fixed tiers only. Ignored when continue_session=false."},"id":{"type":"string","description":"delete: job id to remove (from list)"}}})
    }

    async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String> {
        let val: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid arguments: {}", e))?;

        let action = val.get("action").and_then(|v| v.as_str()).unwrap_or("");

        let svc = self
            .service
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;

        match action {
            "list" => {
                let jobs = svc.list_jobs(true);
                let result: Vec<serde_json::Value> = jobs
                    .iter()
                    .map(|j| {
                        serde_json::json!({
                            "id": j.id,
                            "name": j.name,
                            "schedule": j.schedule,
                            "enabled": j.enabled,
                        })
                    })
                    .collect();
                Ok(serde_json::to_string_pretty(&result).unwrap_or_else(|_| "[]".to_string()))
            }
            "create" => {
                let name = val
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unnamed");
                // Schedule resolution: structured fields (the declared schema
                // surface) first, legacy `schedule` string ("every:Ns" /
                // "at:RFC3339" / raw cron expr) second for backward
                // compatibility. Priority: at_seconds > every_seconds >
                // cron_expr > schedule.
                let schedule = if let Some(secs) = val.get("at_seconds").and_then(|v| v.as_u64()) {
                    // BUG-20261101-285：受检组合加法——secs_arg_to_ms 只受检
                    // 换算（ms ≤ i64::MAX），与 now 的最终合成此前是裸 add：
                    // at_seconds≈9.2233e15（schema 无上限；LLM 幻觉大整数是
                    // 常态）时换算过检、合成溢出——debug panic 打穿 gateway，
                    // release 回绕负值 at_ms 被 compute_next_run 的 ms > now
                    // 过滤 = 静默僵尸任务（BUG-20261024-11「绝不回绕」禁的
                    // 失败形态，换算层修了组合层漏网）。溢出 = 诚实 Err 回灌。
                    let at_ms = secs_arg_to_ms(secs)?
                        .checked_add(chrono::Local::now().timestamp_millis())
                        .ok_or_else(|| {
                            "at_seconds too large: scheduled time overflows".to_string()
                        })?;
                    nemesis_cron::service::CronSchedule {
                        kind: "at".to_string(),
                        at_ms: Some(at_ms),
                        every_ms: None,
                        expr: None,
                        tz: None,
                    }
                } else if let Some(secs) = val.get("every_seconds").and_then(|v| v.as_u64()) {
                    nemesis_cron::service::CronSchedule {
                        kind: "every".to_string(),
                        at_ms: None,
                        every_ms: Some(secs_arg_to_ms(secs)?),
                        expr: None,
                        tz: None,
                    }
                } else if let Some(expr) = val.get("cron_expr").and_then(|v| v.as_str()) {
                    nemesis_cron::service::CronSchedule {
                        kind: "cron".to_string(),
                        at_ms: None,
                        every_ms: None,
                        expr: Some(expr.to_string()),
                        tz: None,
                    }
                } else {
                    let schedule_str = val.get("schedule").and_then(|v| v.as_str()).unwrap_or("");
                    if schedule_str.is_empty() {
                        return Err("Missing schedule: provide one of at_seconds, every_seconds, or cron_expr".to_string());
                    }
                    if schedule_str.starts_with("every:") {
                        let secs_str = schedule_str
                            .trim_start_matches("every:")
                            .trim_end_matches('s');
                        let secs: i64 = secs_str.parse().map_err(|_| "Invalid interval")?;
                        // 负数/溢出与新结构化路径同源拒绝（u64 转换挡负值，
                        // checked mul 挡 i64::MAX*1000 溢出 panic/回绕）。
                        let secs = u64::try_from(secs).map_err(|_| "Invalid interval")?;
                        nemesis_cron::service::CronSchedule {
                            kind: "every".to_string(),
                            at_ms: None,
                            every_ms: Some(secs_arg_to_ms(secs)?),
                            expr: None,
                            tz: None,
                        }
                    } else if schedule_str.starts_with("at:") {
                        let ts_str = schedule_str.trim_start_matches("at:");
                        let ts = chrono::DateTime::parse_from_rfc3339(ts_str)
                            .map_err(|e| format!("Invalid timestamp: {}", e))?;
                        nemesis_cron::service::CronSchedule {
                            kind: "at".to_string(),
                            at_ms: Some(ts.timestamp_millis()),
                            every_ms: None,
                            expr: None,
                            tz: None,
                        }
                    } else {
                        nemesis_cron::service::CronSchedule {
                            kind: "cron".to_string(),
                            at_ms: None,
                            every_ms: None,
                            expr: Some(schedule_str.to_string()),
                            tz: None,
                        }
                    }
                };
                // 同源校验，与其余全部创建入口对齐（CLI cron.rs / web
                // tasks.rs / autopilot / dreaming / board autopilot 都前置
                // validate_schedule + 0 值守卫）。缺这层时坏表达式 / 0 间隔 /
                // 过去时刻的任务会以 enabled + next_run=None 的静默僵尸入库：
                // compute_next_run 对这类 schedule 恒返 None，任务永不触
                // 发、无任何报错、跨重启持久，模型与用户都以为排上了。
                match schedule.kind.as_str() {
                    "cron" => {
                        let expr = schedule.expr.as_deref().unwrap_or("");
                        nemesis_cron::service::CronService::validate_schedule(expr)
                            .map_err(|e| format!("Invalid cron expression '{expr}': {e}"))?;
                    }
                    // 0 间隔：compute_next_run 的 ms>0 过滤恒失 → 静默僵尸。
                    "every" if schedule.every_ms.unwrap_or(0) <= 0 => {
                        return Err(
                            "every_seconds must be > 0 (a zero interval never fires)".to_string()
                        );
                    }
                    // 时刻必须严格在未来：compute_next_run 对 at_ms <= now
                    // 恒返 None（at_seconds=0 即此形态）。
                    "at" if schedule
                        .at_ms
                        .is_none_or(|t| t <= chrono::Local::now().timestamp_millis()) =>
                    {
                        return Err(
                            "at time must be in the future (at_seconds must be >= 1)".to_string()
                        );
                    }
                    _ => {}
                }
                // message is the declared field; content is the legacy alias
                // kept readable for backward compatibility.
                let content = val
                    .get("message")
                    .and_then(|v| v.as_str())
                    .or_else(|| val.get("content").and_then(|v| v.as_str()))
                    .unwrap_or("");
                // Envelope field kept for store round-trip compat; not part of
                // the declared schema (the fire path delivers unconditionally).
                let deliver = val.get("deliver").and_then(|v| v.as_bool()).unwrap_or(true);

                // Use context first, fallback to stored values (mirrors MessageTool pattern).
                let channel = if context.channel.is_empty() {
                    self.channel
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .clone()
                } else {
                    context.channel.clone()
                };
                let chat_id = if context.chat_id.is_empty() {
                    self.chat_id
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .clone()
                } else {
                    context.chat_id.clone()
                };

                // H1 (U12): continue_session=true routes the fired job back
                // into THIS conversation's session (context/persona kept,
                // reply persisted to this chat's history) via add_job_ext's
                // session_key — the gateway's on_job handler consumes it.
                // The session key comes from the CALLING context (the
                // conversation creating the job), never from model input.
                let continue_session = val
                    .get("continue_session")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                // T3 (U12): per-fire tool-round budget, only meaningful with
                // continue_session=true. Fixed enum tiers (5/10/20) — the
                // args_validator rejects off-enum values before dispatch, so a
                // model cannot free-form this into a 1000-round budget. Default
                // 10 per the goal's tier table. None when not continuing (the
                // fired turn then runs under the global max_turns).
                let max_rounds: Option<u32> = if continue_session {
                    Some(val.get("max_rounds").and_then(|v| v.as_u64()).unwrap_or(10) as u32)
                } else {
                    None
                };

                let job = if continue_session {
                    svc.add_job_ext(
                        name,
                        schedule,
                        content,
                        deliver,
                        if channel.is_empty() {
                            None
                        } else {
                            Some(&channel)
                        },
                        if chat_id.is_empty() {
                            None
                        } else {
                            Some(&chat_id)
                        },
                        Some(&context.session_key),
                        max_rounds,
                        true,
                    )
                    .map_err(|e| e.to_string())?
                } else {
                    svc.add_job(
                        name,
                        schedule,
                        content,
                        deliver,
                        if channel.is_empty() {
                            None
                        } else {
                            Some(&channel)
                        },
                        if chat_id.is_empty() {
                            None
                        } else {
                            Some(&chat_id)
                        },
                    )
                    .map_err(|e| e.to_string())?
                };
                let mode_note = if continue_session {
                    format!(
                        " [continues this session; per-fire budget {} rounds]",
                        max_rounds.unwrap_or(10)
                    )
                } else {
                    String::new()
                };
                Ok(format!(
                    "Created cron job: {} (ID: {}){}",
                    job.name, job.id, mode_note
                ))
            }
            "delete" => {
                let id = val.get("id").and_then(|v| v.as_str()).unwrap_or("");
                if id.is_empty() {
                    return Err("Missing 'id' argument".to_string());
                }
                // Reserved-namespace guard (single truth source
                // `reserved_prefix_hit`): board-ap:/memory-dreaming: jobs are
                // owned by the board autopilot rules / memory.dreaming config
                // respectively. Deleting one from this tool silently breaks
                // the mount until the next gateway startup sync (the rule's
                // "mounted" status keeps lying in the meantime) — management
                // belongs to the board/memory surfaces, not the agent tool.
                let reserved = svc
                    .get_job(id)
                    .map(|j| nemesis_cron::reserved_prefix_hit(&j.name).is_some())
                    .unwrap_or(false);
                if reserved {
                    return Err(format!(
                        "job {id} is a system-internal job (reserved namespace); \
                         board autopilot rules are managed via board autopilot commands \
                         and memory dreaming via memory config — it cannot be deleted here"
                    ));
                }
                match svc.remove_job(id) {
                    Ok(true) => Ok(format!("Deleted cron job: {}", id)),
                    Ok(false) => Err(format!("Job not found: {}", id)),
                    Err(e) => Err(format!("Failed to delete cron job {}: {}", id, e)),
                }
            }
            _ => Err(format!(
                "Unknown cron action: '{}'. Use: list, create, delete",
                action
            )),
        }
    }

    fn set_context(&self, channel: &str, chat_id: &str) {
        if let Ok(mut ch) = self.channel.lock() {
            *ch = channel.to_string();
        }
        if let Ok(mut ci) = self.chat_id.lock() {
            *ci = chat_id.to_string();
        }
    }
}

/// Maximum sleep duration: 1 hour (matches Go implementation).
const MAX_SLEEP_SECONDS: u64 = 3600;

/// A tool that sleeps for a specified duration in seconds.
///
/// This is a utility tool for testing delays and timeouts.
/// Maximum duration is 60 seconds.
pub struct SleepTool;

#[async_trait]
impl Tool for SleepTool {
    fn description(&self) -> String {
        "Suspend execution for a specified duration in seconds".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"seconds":{"type":"number","description":"Duration in seconds"}},"required":["seconds"]})
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let seconds = if let Ok(val) = serde_json::from_str::<serde_json::Value>(args) {
            val.get("seconds")
                .or_else(|| val.get("duration"))
                .and_then(|v| v.as_u64())
                .ok_or("Missing or invalid 'seconds' field (must be a positive integer)")?
        } else {
            args.trim()
                .parse::<u64>()
                .map_err(|_| "Invalid duration: must be a positive integer".to_string())?
        };

        if seconds < 1 {
            return Err("Duration must be at least 1 second".to_string());
        }
        if seconds > MAX_SLEEP_SECONDS {
            return Err(format!(
                "Duration cannot exceed {} seconds",
                MAX_SLEEP_SECONDS
            ));
        }

        sleep(Duration::from_secs(seconds)).await;
        Ok(format!("Slept for {} seconds", seconds))
    }
}

// ===========================================================================
// Todo list tool (H1, devtool-upgrade 阶段 2)
// ===========================================================================

/// `todowrite`：全量提交式 todo 清单工具。
///
/// 语义：每次调用**替换整个清单**（模型无需 diff 思维），落盘
/// `{workspace}/sessions/todo_{safe_session_key}.json`（原子写：tmp + rename），
/// 成功后广播 [`nemesis_types::agent::AgentEvent::TodoUpdated`]（web pump 转
/// WS push + SSE，前端实时渲染）。**不回灌 prompt**——靠 H3 的 system
/// prompt few-shot 示范驱动模型维持清单。
pub struct TodoWriteTool {
    /// 存储根 workspace（`sessions/` 目录挂其下）。
    workspace: std::path::PathBuf,
    /// TodoUpdated 广播通道（None = 只落盘，不广播）。
    event_tx: Option<tokio::sync::broadcast::Sender<nemesis_types::agent::AgentEvent>>,
}

impl TodoWriteTool {
    pub fn new(
        workspace: std::path::PathBuf,
        event_tx: Option<tokio::sync::broadcast::Sender<nemesis_types::agent::AgentEvent>>,
    ) -> Self {
        Self {
            workspace,
            event_tx,
        }
    }
}

#[async_trait]
impl Tool for TodoWriteTool {
    fn description(&self) -> String {
        "Write the todo list for the current task. FULL REPLACEMENT: each call replaces the entire list. Use for multi-step tasks: create the list up front, keep exactly one item in_progress while working, mark items completed as you go. Before delivering your final answer, reconcile the list: mark finished items completed, set items you are not doing this turn back to pending — never end a turn with an item still in_progress."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "todos": {
                    "type": "array",
                    "description": "The complete todo list (replaces the previous list entirely)",
                    "items": {
                        "type": "object",
                        "properties": {
                            "content": {"type": "string", "description": "The task description"},
                            "status": {"type": "string", "enum": ["pending", "in_progress", "completed"], "description": "Item status"}
                        },
                        "required": ["content", "status"]
                    }
                }
            },
            "required": ["todos"]
        })
    }

    async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String> {
        let parsed: TodosArgs =
            serde_json::from_str(args).map_err(|e| format!("invalid todowrite args: {e}"))?;
        let todos = parsed.todos;

        // 存储路径：sessions/todo_{safe_session_key}.json。安全化走仓内
        // 单一真相源白名单消毒（SAN-05；F-U4-4：B 端 peer_chat 复合键
        // `cluster_rpc:{node}/{chat}` 含 '/'（peer_chat_handler 设计：chat_id
        // 可能含 ':'，故用 '/' 分隔），只替换 ':' 会把文件名拆出中间目录 →
        // write os error 3，2026-09-15 真机实证；当时本地实现
        // `replace([':', '/', '\\'], "_")`，现升级为
        // nemesis_utils::sanitize::sanitize_path_segment，对运行时键族映射
        // 一致，另兜住 `..` 与超长输入）。
        let safe_key = nemesis_utils::sanitize::sanitize_path_segment(&context.session_key);
        let dir = nemesis_path::resolve_sessions_dir_in_workspace(&self.workspace);
        let path = dir.join(format!("todo_{safe_key}.json"));

        // 原子写：BUG-20261025-51 起委托唯一临时名权威 helper（旧固定
        // `todo_{key}.json.tmp` 同键并行工具调用互踩可产损坏 todo 文件）。
        let body = serde_json::to_string_pretty(&todos)
            .map_err(|e| format!("serialize todos failed: {e}"))?;
        std::fs::create_dir_all(&dir).map_err(|e| format!("create sessions dir failed: {e}"))?;
        nemesis_utils::file::write_file_atomic(&path.to_string_lossy(), body.as_bytes(), 0o644)
            .map_err(|e| format!("write todo file failed: {e}"))?;

        // 回执统计（事件广播前先取 counts，todos 随后 move 进事件）。
        let total = todos.len();
        let done = todos
            .iter()
            .filter(|t| t.status == nemesis_types::agent::TodoStatus::Completed)
            .count();
        let in_progress = todos
            .iter()
            .filter(|t| t.status == nemesis_types::agent::TodoStatus::InProgress)
            .count();

        // 广播 TodoUpdated（无订阅者 = Err，静默——观察者通道空转是常态）。
        if let Some(tx) = &self.event_tx {
            let _ = tx.send(nemesis_types::agent::AgentEvent::TodoUpdated {
                session_key: context.session_key.clone(),
                chat_id: context.chat_id.clone(),
                todos,
            });
        }

        Ok(format!(
            "Todo list updated: {total} items ({done} completed, {in_progress} in_progress)"
        ))
    }
}

/// todowrite 的 args 形态（serde 解析 + enum 校验一步完成：status 非法值
/// 直接 Err，由 args_validator 回灌层决定重试预算）。
#[derive(serde::Deserialize)]
struct TodosArgs {
    todos: Vec<nemesis_types::agent::TodoItem>,
}

// ===========================================================================
// Web search tools
// ===========================================================================
// ===========================================================================
// Hardware tools (I2C / SPI) — 薄适配器，委托 nemesis-tools 真实实现
// ===========================================================================

// nemesis-tools 侧 Tool trait 以 `_` 导入（仅方法解析用，不占名字——
// 与本文件的 loop 侧 `Tool` 不冲突，两个 trait 的实现类型不相交）。
use nemesis_tools::registry::Tool as _;

/// I2C bus tool - interacts with I2C devices (Linux only).
///
/// BUG-13（2026-10-14）根修：此前是回显桩（假成功、无真实 I/O——模型以为
/// 写入成功而硬件什么都没发生）。真实实现（ioctl 扫描/读写、参数校验、
/// P8 地址策略、非 Linux 平台诚实报错）全在
/// [`nemesis_tools::hardware::I2CTool`]，本层只做 `&str`→`Value` 解析与
/// `ToolResult` 折叠（is_error→Err(for_llm)）；description/parameters
/// 直接透传真实实现（单一真相源，随之获得 confirm/bus 等完整 schema
/// 字段——一次性 prompt 文案漂移，接受）。P8 地址策略仍从注册点
/// `with_policy` 注入（config `tools.hardware` 段），校验在真实实现内
/// 执行，与本路径同源。
pub struct I2CTool {
    inner: nemesis_tools::hardware::I2CTool,
}

impl I2CTool {
    /// 内置默认策略（可自由寻址空间 0x08-0x77）。
    pub fn new() -> Self {
        Self {
            inner: nemesis_tools::hardware::I2CTool::new(),
        }
    }

    /// 注入 config 侧策略（`tools.hardware` 段）。
    pub fn with_policy(policy: nemesis_tools::hardware::HardwarePolicy) -> Self {
        Self {
            inner: nemesis_tools::hardware::I2CTool::with_policy(policy),
        }
    }
}

impl Default for I2CTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for I2CTool {
    fn description(&self) -> String {
        self.inner.description().to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        self.inner.parameters()
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let val: serde_json::Value =
            serde_json::from_str(args).map_err(|_| "Invalid JSON arguments".to_string())?;
        let result = self.inner.execute(&val).await;
        if result.is_error {
            Err(result.for_llm)
        } else {
            Ok(result.for_llm)
        }
    }
}

/// SPI bus tool - interacts with SPI devices (Linux only).
///
/// 薄适配器同 [`I2CTool`]：委托 [`nemesis_tools::hardware::SPITool`] 真实
/// 实现（ZST，按调用临时构造；保持单元结构体形态，兼容既有
/// `let tool = SPITool;` 构造点）。
pub struct SPITool;

#[async_trait]
impl Tool for SPITool {
    fn description(&self) -> String {
        nemesis_tools::hardware::SPITool.description().to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        nemesis_tools::hardware::SPITool.parameters()
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let val: serde_json::Value =
            serde_json::from_str(args).map_err(|_| "Invalid JSON arguments".to_string())?;
        let result = nemesis_tools::hardware::SPITool.execute(&val).await;
        if result.is_error {
            Err(result.for_llm)
        } else {
            Ok(result.for_llm)
        }
    }
}

// ===========================================================================
// Tool registration
// ===========================================================================
/// Bridge tool that wraps a ForgeToolExecutor tool call into the agent's Tool trait.
/// Each instance wraps a single forge tool name (e.g. "forge_reflect").
#[cfg(feature = "forge")]
pub(super) struct ForgeBridgeTool {
    name: String,
    description: String,
    parameters: serde_json::Value,
    executor: Arc<nemesis_forge::forge_tools::ForgeToolExecutor>,
}

#[cfg(feature = "forge")]
impl ForgeBridgeTool {
    pub(super) fn new(
        name: String,
        description: String,
        parameters: serde_json::Value,
        executor: Arc<nemesis_forge::forge_tools::ForgeToolExecutor>,
    ) -> Self {
        Self {
            name,
            description,
            parameters,
            executor,
        }
    }
}

#[cfg(feature = "forge")]
#[async_trait]
impl Tool for ForgeBridgeTool {
    fn description(&self) -> String {
        self.description.clone()
    }

    fn parameters(&self) -> serde_json::Value {
        self.parameters.clone()
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let args_value =
            serde_json::from_str::<serde_json::Value>(args).unwrap_or(serde_json::Value::Null);
        let result = self.executor.execute(&self.name, &args_value).await;
        if result.success {
            Ok(result.content)
        } else {
            Err(result.content)
        }
    }
}

// ===========================================================================
// MCP Discovery Tools
// ===========================================================================
/// H1（2026-09-05）：`todowrite` 工具的接线配置。
#[derive(Clone, Debug)]
pub struct TodoToolConfig {
    /// 存储根（`{workspace}/sessions/todo_{safe_session_key}.json`）。
    pub workspace: std::path::PathBuf,
    /// TodoUpdated 广播通道（与 M1a ToolEventHook 同一 sender；None =
    /// 只落盘不广播——CLI / exec_worker 形态）。
    pub event_tx: Option<tokio::sync::broadcast::Sender<nemesis_types::agent::AgentEvent>>,
}
