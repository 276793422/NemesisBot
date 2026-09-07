//! L7 ACP server（devtool-upgrade 阶段 7）—— Agent Client Protocol agent 侧。
//!
//! `nemesisbot acp` 以 stdio JSON-RPC（ndjson 帧，一行一帧）说 ACP v1，
//! 编辑器等 ACP 客户端直接接入。每个 ACP session = 一次完整 K1 式装配
//! （`build_security_plugin` + `SharedResources` + `build_agent_loop`，
//! 与 `commands/run.rs` 逐行同构）——安全 8 层 + guardian + tier 过滤 +
//! spill + turn_guard 与 gateway **同源生效**，编辑器不是安全旁路。
//! session cwd 即 workspace（编辑器打开的项目目录），多轮对话靠 session
//! 持久 `AgentInstance`（与 K1 临时 instance 的唯一实质分叉）。
//!
//! 事件映射（M1a broadcast + `run_with_trace` 返回 Vec，run.rs json 模式
//! 同款 select 模式）：
//! - `ToolStarted`（实时）→ `session/update` `tool_call`（status=in_progress）
//! - `ToolFinished`（实时）→ `tool_call_update`（completed/failed）
//! - `Done(final)` → `agent_message_chunk` + 响应 `stopReason=end_turn`
//! - `Error(e)` → 错误文本 chunk + `stopReason=refusal`
//! - cancel token 触发 → `stopReason=cancelled`
//!
//! permission 映射 M7：[`AcpApprovalManager`] 实现
//! `nemesis_security::auditor::ApprovalManager`，审批请求经
//! `session/request_permission` 发给编辑器用户阻塞等裁决（ACP 进程无
//! dashboard，编辑器即唯一审批面；v1 只供 allow_once/reject_once 两选项
//! ——无 F3 规则表背书，不假供 always 记忆）。
//!
//! 诚实边界（v1）：
//! - **文本无增量流式**：`AgentEvent::Message`（nemesis-agent）在 loop.rs
//!   从未构造，M1a broadcast 无文本事件——文本只在 Done 整段到达（K2
//!   NDJSON 同界）。工具流实时、文本收尾送达。
//! - `loadSession=false`：进程重启会话即失（instance 在内存）；
//!   session/resume/list/close/delete 不宣告，未知方法 → -32601。
//! - `image/audio/embeddedContext=false`：收到未宣告 block 即 -32602；
//!   resource_link（基线必选）降级为文本行拼入 prompt。
//! - session/new 的 `mcpServers` 忽略（warn）：per-session 动态 MCP 要动
//!   loop_tools 注册层，v1 只用 config.json 的 MCP。
//! - fs/read_text_file、fs/write_text_file、terminal/* 永不调用（agent 用
//!   自家工具）。
//! - 轮次预算耗尽 → end_turn（不伪造 max_tokens/max_turn_requests 保真度）。
//! - session/cancel 只作用于在飞 turn（排队中的下一轮不受影响）。
//!
//! 测试 seams：transport 泛化 AsyncRead/AsyncWrite（duplex 全流程单测）；
//! session 装配注入 [`SessionFactory`]；协议逻辑与 [`PromptDriver`] 解耦。

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, BufWriter};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// 我们支持的协议版本（ACP v1；版本只随破坏性变更递增）。
pub const PROTOCOL_VERSION: u16 = 1;

// ---------------------------------------------------------------------------
// 纯函数映射层（测试对象）
// ---------------------------------------------------------------------------

/// 一轮 prompt 的结局（内部语义），映射 ACP StopReason。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOutcome {
    /// 正常完成，携带最终回复文本。
    Completed(String),
    /// agent 报错（映射 refusal——turn 未正常完成，诚实呈现）。
    Failed(String),
    /// 客户端取消。
    Cancelled,
}

impl TurnOutcome {
    /// ACP StopReason 字符串。
    pub fn stop_reason(&self) -> &'static str {
        match self {
            TurnOutcome::Completed(_) => "end_turn",
            TurnOutcome::Failed(_) => "refusal",
            TurnOutcome::Cancelled => "cancelled",
        }
    }

    /// 作为最终 `agent_message_chunk` 送达的文本（编辑器看得到原因）。
    /// Cancelled 无文本（客户端有自己的取消 UI）。
    pub fn final_text(&self) -> Option<&str> {
        match self {
            TurnOutcome::Completed(m) | TurnOutcome::Failed(m) => Some(m),
            TurnOutcome::Cancelled => None,
        }
    }
}

/// 工具名 → ACP ToolKind。保守映射：认得出的给语义 kind，认不出的一律
/// other（客户端按 other 渲染，不会错）。
pub fn map_tool_kind(tool: &str) -> &'static str {
    match tool {
        "exec" | "async_shell" | "shell" => "execute",
        "read_file" | "grep" | "ls" | "dir_list" | "lsp" => "read",
        "write_file" | "edit_file" | "edit" | "file_append" => "edit",
        "web_search" | "web_fetch" => "fetch",
        "todowrite" | "think" | "plan" => "think",
        _ => "other",
    }
}

/// session/prompt 的 content blocks → 任务文本。
///
/// 基线必选 text + resource_link；image/audio/resource 未宣告
/// （promptCapabilities 全 false），收到即 `Err`（客户端违约）。
/// resource_link 降级为文本行拼入（agent 用自家 fs 工具读得到同一文件）。
pub fn content_blocks_to_text(blocks: &[Value]) -> Result<String, String> {
    let mut parts: Vec<String> = Vec::new();
    for b in blocks {
        match b.get("type").and_then(|v| v.as_str()) {
            Some("text") => {
                let t = b.get("text").and_then(|v| v.as_str()).unwrap_or("");
                if !t.is_empty() {
                    parts.push(t.to_string());
                }
            }
            Some("resource_link") => {
                let uri = b.get("uri").and_then(|v| v.as_str()).unwrap_or("");
                let name = b
                    .get("title")
                    .or_else(|| b.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                parts.push(format!(
                    "[引用资源] {}{}",
                    uri,
                    if name.is_empty() {
                        String::new()
                    } else {
                        format!(" ({name})")
                    }
                ));
            }
            Some(other) => {
                return Err(format!(
                    "unsupported content block '{other}' (capability not advertised)"
                ));
            }
            None => return Err("content block missing 'type'".to_string()),
        }
    }
    if parts.is_empty() {
        return Err("empty prompt".to_string());
    }
    Ok(parts.join("\n"))
}

/// `ToolStarted` → `session/update` 载荷（tool_call，status=in_progress）。
pub fn tool_started_update(call_id: &str, tool: &str, args_preview: &str) -> Value {
    json!({
        "sessionUpdate": "tool_call",
        "toolCallId": call_id,
        "title": tool,
        "kind": map_tool_kind(tool),
        "status": "in_progress",
        "rawInput": args_preview,
    })
}

/// `ToolFinished` → `session/update` 载荷（tool_call_update）。
pub fn tool_finished_update(call_id: &str, ok: bool, result_preview: &str) -> Value {
    json!({
        "sessionUpdate": "tool_call_update",
        "toolCallId": call_id,
        "status": if ok { "completed" } else { "failed" },
        "rawOutput": result_preview,
    })
}

/// 客户端对 `session/request_permission` 的响应 result → 内部裁决。
/// `None` = 载荷不可解析（按拒绝处理是调用方语义——失败关闭）。
pub fn parse_permission_outcome(result: &Value) -> Option<PermissionOutcome> {
    let outcome = result.get("outcome")?;
    match outcome.get("outcome").and_then(|v| v.as_str()) {
        Some("cancelled") => Some(PermissionOutcome::Denied),
        Some("selected") => match outcome.get("optionId").and_then(|v| v.as_str()) {
            Some("allow_once") | Some("allow_always") => Some(PermissionOutcome::Allowed),
            Some("reject_once") | Some("reject_always") => Some(PermissionOutcome::Denied),
            _ => None,
        },
        _ => None,
    }
}

/// session/new 的 cwd 校验（纯函数）：绝对路径 + 存在且是目录。
pub fn validate_cwd(cwd: &str) -> Result<PathBuf, String> {
    let p = PathBuf::from(cwd);
    if !p.is_absolute() {
        return Err(format!("cwd must be an absolute path: {cwd}"));
    }
    if !p.is_dir() {
        return Err(format!("cwd does not exist or is not a directory: {cwd}"));
    }
    Ok(p)
}

// ---------------------------------------------------------------------------
// JSON-RPC 帧构造（纯函数）
// ---------------------------------------------------------------------------

pub fn make_result(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

pub fn make_error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

pub fn make_notification(method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "method": method, "params": params})
}

// security feature off 时无生产调用方（AcpApprovalManager 桥被裁）——
// 协议层机制随测试保留，--no-default-features 编译不告警。
#[allow(dead_code)]
pub fn make_request(id: &str, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

// ---------------------------------------------------------------------------
// permission 闸（M7 → ACP 桥）
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionOutcome {
    Allowed,
    Denied,
}

/// 在飞的 permission 请求登记表（server 持有；响应帧按 JSON-RPC id 回灌）。
#[derive(Default)]
pub struct PendingPermissions {
    inner: Mutex<HashMap<String, oneshot::Sender<PermissionOutcome>>>,
}

impl PendingPermissions {
    pub fn new() -> Self {
        Self::default()
    }

    // 同上：security off 时仅 PermissionGate::request 与测试使用。
    #[allow(dead_code)]
    fn register(&self, rpc_id: &str) -> oneshot::Receiver<PermissionOutcome> {
        let (tx, rx) = oneshot::channel();
        self.inner
            .lock()
            .expect("pending permissions lock poisoned")
            .insert(rpc_id.to_string(), tx);
        rx
    }

    /// server 收到响应帧时路由。`result=None`（error 响应/丢帧）按拒绝。
    /// 返回 false = 无人等待（迟到/未知 id，诚实丢弃）。
    pub fn resolve(&self, rpc_id: &str, result: Option<&Value>) -> bool {
        let outcome = result
            .and_then(parse_permission_outcome)
            .unwrap_or(PermissionOutcome::Denied);
        if let Some(tx) = self
            .inner
            .lock()
            .expect("pending permissions lock poisoned")
            .remove(rpc_id)
        {
            tx.send(outcome).is_ok()
        } else {
            false
        }
    }
}

/// 单个 ACP session 的 permission 出口：发 `session/request_permission`
/// 请求帧给客户端（编辑器），阻塞等裁决。
// security off 时 request 无生产调用方（同 make_request 注释）。
#[allow(dead_code)]
pub struct PermissionGate {
    session_id: String,
    next_id: AtomicU64,
    outbound: mpsc::Sender<Value>,
    pending: Arc<PendingPermissions>,
}

impl PermissionGate {
    pub fn new(
        session_id: String,
        outbound: mpsc::Sender<Value>,
        pending: Arc<PendingPermissions>,
    ) -> Self {
        Self {
            session_id,
            next_id: AtomicU64::new(0),
            outbound,
            pending,
        }
    }

    /// 发起 permission 请求并等裁决。超时 / 通道断开 / 载荷不可解析一律
    /// `Denied`（失败关闭——审批语境下永不放行兜底）。
    #[allow(dead_code)]
    pub async fn request(
        &self,
        tool: &str,
        target: &str,
        risk_level: &str,
        reason: &str,
        timeout: Duration,
    ) -> PermissionOutcome {
        let rpc_id = format!("perm-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let rx = self.pending.register(&rpc_id);
        let params = json!({
            "sessionId": self.session_id,
            "toolCall": {
                "toolCallId": rpc_id,
                "title": tool,
                "kind": map_tool_kind(tool),
                "rawInput": target,
            },
            "options": [
                {"optionId": "allow_once", "name": "Allow once", "kind": "allow_once"},
                {"optionId": "reject_once", "name": "Reject once", "kind": "reject_once"},
            ],
            "_meta": {
                "nemesisbot": {"risk_level": risk_level, "reason": reason},
            },
        });
        let frame = make_request(&rpc_id, "session/request_permission", params);
        if self.outbound.send(frame).await.is_err() {
            return PermissionOutcome::Denied;
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(outcome)) => outcome,
            // 超时 / oneshot 发送端已 drop（登记表被清）/ 接收失败 → 拒绝
            _ => PermissionOutcome::Denied,
        }
    }
}

/// M7 审批 → ACP permission 桥：实现
/// `nemesis_security::auditor::ApprovalManager`（sync 阻塞 trait）。
/// 装配进该 session 的 `approval_slot`——auditor 发起审批时经
/// [`PermissionGate`] 走 `session/request_permission`，编辑器即审批 UI。
/// 阻塞桥接用 block_in_place（WebQuestionBroker 同款；要求 multi-thread
/// runtime——`nemesisbot acp` 经 `#[tokio::main]` 默认 multi-thread）。
/// security feature off（IoT 裁剪构建）时整个桥不存在——协议层（gate/
/// permission 往返）不依赖它，照常工作。
#[cfg(feature = "security")]
pub struct AcpApprovalManager {
    gate: Arc<PermissionGate>,
}

#[cfg(feature = "security")]
impl nemesis_security::auditor::ApprovalManager for AcpApprovalManager {
    fn is_running(&self) -> bool {
        true
    }

    fn request_approval_sync(
        &self,
        _request_id: &str,
        operation: &str,
        target: &str,
        risk_level: &str,
        reason: &str,
        timeout_secs: u64,
    ) -> Result<nemesis_security::auditor::ApprovalVerdict, String> {
        let handle = tokio::runtime::Handle::current();
        let gate = Arc::clone(&self.gate);
        let outcome = tokio::task::block_in_place(|| {
            handle.block_on(async move {
                gate.request(
                    operation,
                    target,
                    risk_level,
                    reason,
                    Duration::from_secs(timeout_secs.max(1)),
                )
                .await
            })
        });
        Ok(match outcome {
            PermissionOutcome::Allowed => nemesis_security::auditor::ApprovalVerdict {
                approved: true,
                note: None,
            },
            PermissionOutcome::Denied => nemesis_security::auditor::ApprovalVerdict {
                approved: false,
                note: Some("user rejected via editor (ACP)".to_string()),
            },
        })
    }
}

// ---------------------------------------------------------------------------
// 驱动 seams（协议层 ⇄ agent 装配层解耦）
// ---------------------------------------------------------------------------

/// turn 期间 update 载荷的出口；server 包装成 `session/update` 帧
/// （sessionId 由 server 盖章，驱动不感知）。
pub type UpdateSink = mpsc::Sender<Value>;

/// 一轮 prompt 的执行方（生产 = LoopDriver，测试 = 假驱动）。
/// dyn 兼容：手写 boxed future（项目无 async_trait 依赖）。
pub trait PromptDriver: Send + Sync {
    fn prompt<'a>(
        &'a self,
        text: String,
        sink: UpdateSink,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = TurnOutcome> + Send + 'a>>;
}

/// session 装配方（生产 = RealSessionFactory 的 K1 同源装配；测试 = 假）。
pub trait SessionFactory: Send + Sync {
    fn create<'a>(
        &'a self,
        cwd: PathBuf,
        session_id: String,
        gate: Arc<PermissionGate>,
    ) -> Pin<Box<dyn Future<Output = Result<Box<dyn PromptDriver>, String>> + Send + 'a>>;
}

// ---------------------------------------------------------------------------
// Server 核心
// ---------------------------------------------------------------------------

struct AcpSession {
    driver: Box<dyn PromptDriver>,
    /// 在飞 turn 的取消令牌（cancel 通知取走并触发；轮间清空）。
    cancel: Mutex<Option<CancellationToken>>,
    /// 同会话串行（spec：一轮未完不发下一轮）。
    turn_lock: tokio::sync::Mutex<()>,
}

struct AcpServerCore {
    sessions: Mutex<HashMap<String, Arc<AcpSession>>>,
    outbound: mpsc::Sender<Value>,
    pending: Arc<PendingPermissions>,
    factory: Arc<dyn SessionFactory>,
    version: String,
}

impl AcpServerCore {
    async fn send(&self, frame: Value) {
        let _ = self.outbound.send(frame).await;
    }

    fn initialize_result(&self, client_version: u16) -> Value {
        // 版本协商：client 版本 = 我们支持的 → 原样回；否则回我们最新（1），
        // 客户端不支持则自行断开（spec 语义）。
        let negotiated = if client_version == PROTOCOL_VERSION {
            client_version
        } else {
            PROTOCOL_VERSION
        };
        json!({
            "protocolVersion": negotiated,
            "agentInfo": {"name": "nemesisbot", "version": self.version},
            "agentCapabilities": {
                "loadSession": false,
                "promptCapabilities": {"image": false, "audio": false, "embeddedContext": false},
                "mcpCapabilities": {"http": false, "sse": false},
            },
            "authMethods": [],
        })
    }

    async fn handle_session_new(&self, id: Option<Value>, params: Value) {
        let Some(id) = id else { return };
        let cwd = params.get("cwd").and_then(|v| v.as_str()).unwrap_or("");
        let cwd = match validate_cwd(cwd) {
            Ok(p) => p,
            Err(e) => {
                self.send(make_error(id, -32602, &e)).await;
                return;
            }
        };
        let mcp_servers = params
            .get("mcpServers")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        if mcp_servers > 0 {
            // 诚实边界：per-session 动态 MCP v1 不支持（见模块头）。
            eprintln!(
                "acp: session mcpServers ({mcp_servers}) ignored in v1; configure MCP via config.json"
            );
        }
        let session_id = uuid::Uuid::new_v4().simple().to_string();
        let gate = Arc::new(PermissionGate::new(
            session_id.clone(),
            self.outbound.clone(),
            Arc::clone(&self.pending),
        ));
        match self.factory.create(cwd, session_id.clone(), gate).await {
            Ok(driver) => {
                self.sessions
                    .lock()
                    .expect("sessions lock poisoned")
                    .insert(
                        session_id.clone(),
                        Arc::new(AcpSession {
                            driver,
                            cancel: Mutex::new(None),
                            turn_lock: tokio::sync::Mutex::new(()),
                        }),
                    );
                self.send(make_result(id, json!({"sessionId": session_id})))
                    .await;
            }
            Err(e) => {
                self.send(make_error(
                    id,
                    -32603,
                    &format!("session create failed: {e}"),
                ))
                .await;
            }
        }
    }

    async fn handle_prompt(&self, id: Option<Value>, params: Value) {
        let Some(id) = id else { return };
        let Some(sid) = params
            .get("sessionId")
            .and_then(|v| v.as_str())
            .map(str::to_string)
        else {
            self.send(make_error(id, -32602, "missing sessionId")).await;
            return;
        };
        let Some(session) = self
            .sessions
            .lock()
            .expect("sessions lock poisoned")
            .get(&sid)
            .cloned()
        else {
            self.send(make_error(id, -32602, &format!("unknown sessionId: {sid}")))
                .await;
            return;
        };
        let blocks = params
            .get("prompt")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let text = match content_blocks_to_text(&blocks) {
            Ok(t) => t,
            Err(e) => {
                self.send(make_error(id, -32602, &e)).await;
                return;
            }
        };

        // 同会话串行；锁住后才登记本轮 cancel token（cancel 只打在飞轮）。
        let _guard = session.turn_lock.lock().await;
        let token = CancellationToken::new();
        *session.cancel.lock().expect("cancel lock poisoned") = Some(token.clone());

        // update 泵：驱动 → session/update 帧（sessionId 盖章），FIFO 保证
        // 全部 update 先于 prompt 响应出站（spec：updates before response）。
        let (utx, mut urx) = mpsc::channel::<Value>(64);
        let pump_sid = sid.clone();
        let pump_out = self.outbound.clone();
        let pump = tokio::spawn(async move {
            while let Some(u) = urx.recv().await {
                let frame = make_notification(
                    "session/update",
                    json!({"sessionId": pump_sid, "update": u}),
                );
                if pump_out.send(frame).await.is_err() {
                    break;
                }
            }
        });

        let outcome = session.driver.prompt(text, utx, token).await;
        // prompt 返回时 utx 已 drop；泵排干缓冲后自行退出——await 它即
        // 保证「update 全部出站后才发响应」。
        let _ = pump.await;
        *session.cancel.lock().expect("cancel lock poisoned") = None;

        // 最终文本（Done/Error）作为收尾 chunk 诚实送达。
        if let Some(text) = outcome.final_text() {
            self.send(make_notification(
                "session/update",
                json!({"sessionId": sid, "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "content": {"type": "text", "text": text},
                }}),
            ))
            .await;
        }
        self.send(make_result(
            id,
            json!({"stopReason": outcome.stop_reason()}),
        ))
        .await;
    }

    fn handle_cancel(&self, params: &Value) {
        let Some(sid) = params.get("sessionId").and_then(|v| v.as_str()) else {
            return;
        };
        if let Some(session) = self
            .sessions
            .lock()
            .expect("sessions lock poisoned")
            .get(sid)
            .cloned()
            && let Some(token) = session.cancel.lock().expect("cancel lock poisoned").take()
        {
            token.cancel();
        }
    }

    async fn dispatch(self: &Arc<Self>, msg: Value) {
        let Some(method) = msg
            .get("method")
            .and_then(|v| v.as_str())
            .map(str::to_string)
        else {
            // 无 method = 对端响应帧：permission 裁决回灌按 JSON-RPC id 路由。
            if let Some(id) = msg.get("id").and_then(|v| v.as_str()) {
                let result = msg.get("result");
                self.pending.resolve(id, result);
            }
            return;
        };
        let id = msg.get("id").cloned().filter(|v| !v.is_null());
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        match method.as_str() {
            "initialize" => {
                let client_version = params
                    .get("protocolVersion")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(PROTOCOL_VERSION as u64) as u16;
                if let Some(id) = id {
                    self.send(make_result(id, self.initialize_result(client_version)))
                        .await;
                }
            }
            "session/new" => {
                let this = Arc::clone(self);
                tokio::spawn(async move {
                    this.handle_session_new(id, params).await;
                });
            }
            "session/prompt" => {
                let this = Arc::clone(self);
                tokio::spawn(async move {
                    this.handle_prompt(id, params).await;
                });
            }
            "session/cancel" => self.handle_cancel(&params),
            _ => {
                // 未知/未宣告方法：请求回 -32601（session/load 等不宣告的
                // 方法同此——诚实 method not found）；通知无 id，静默忽略。
                if let Some(id) = id {
                    self.send(make_error(
                        id,
                        -32601,
                        &format!("Method not found: {method}"),
                    ))
                    .await;
                }
            }
        }
    }
}

/// 读一行帧 → dispatch，直到 EOF。行解析失败回 -32700（id 未知 → null）。
async fn read_dispatch_loop(
    reader: impl AsyncRead + Unpin,
    core: Arc<AcpServerCore>,
) -> Result<(), String> {
    let mut lines = BufReader::new(reader).lines();
    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|e| format!("stdin read failed: {e}"))?
    {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(&line) {
            Ok(msg) => core.dispatch(msg).await,
            Err(_) => {
                core.send(make_error(Value::Null, -32700, "Parse error"))
                    .await
            }
        }
    }
    Ok(())
}

/// 跑 server 直到入站 EOF。transport 泛化（生产 stdio，测试 duplex）。
pub async fn serve(
    reader: impl AsyncRead + Unpin,
    writer: impl AsyncWrite + Unpin + Send + 'static,
    factory: Arc<dyn SessionFactory>,
    version: String,
) -> Result<(), String> {
    let (out_tx, mut out_rx) = mpsc::channel::<Value>(256);
    // 出站写者：一行一帧、逐行 flush。core drop（EOF 且在飞 turn 收尾）后
    // 所有 sender 归零 → 通道关闭 → 排干缓冲、flush、退出。
    let writer_task = tokio::spawn(async move {
        let mut w = BufWriter::new(writer);
        while let Some(frame) = out_rx.recv().await {
            let Ok(mut line) = serde_json::to_string(&frame) else {
                continue;
            };
            line.push('\n');
            if w.write_all(line.as_bytes()).await.is_err() {
                break;
            }
            if w.flush().await.is_err() {
                break;
            }
        }
    });

    let core = Arc::new(AcpServerCore {
        sessions: Mutex::new(HashMap::new()),
        outbound: out_tx,
        pending: Arc::new(PendingPermissions::new()),
        factory,
        version,
    });
    let result = read_dispatch_loop(reader, Arc::clone(&core)).await;
    drop(core); // sessions/gates/在飞任务持有的 Arc 归零后通道关闭
    let _ = writer_task.await;
    result
}

// ---------------------------------------------------------------------------
// 生产装配：RealSessionFactory + LoopDriver（K1 同源）
// ---------------------------------------------------------------------------

/// 生产 session 装配：以 cwd 为 workspace 走完整 K1 式装配
/// （run.rs 逐行同构），审批槽填 [`AcpApprovalManager`]（M7 → ACP 桥）。
pub struct RealSessionFactory {
    pub home: PathBuf,
}

impl SessionFactory for RealSessionFactory {
    fn create<'a>(
        &'a self,
        cwd: PathBuf,
        session_id: String,
        gate: Arc<PermissionGate>,
    ) -> Pin<Box<dyn Future<Output = Result<Box<dyn PromptDriver>, String>> + Send + 'a>> {
        Box::pin(async move {
            // 配置必须存在（headless 不隐式 onboard，与 run.rs 同纪律）。
            let config_path = crate::common::config_path(&self.home);
            if !config_path.exists() {
                return Err(format!(
                    "Configuration not found: {}. Run 'nemesisbot onboard default' first.",
                    config_path.display()
                ));
            }
            let cfg = nemesis_config::load_config(&config_path)
                .map_err(|e| format!("failed to load config: {e}"))?;
            // U15：模型 API key 走 credentials.yaml（与 gateway 同一解析路径）。
            nemesis_config::credentials::set_global_credentials_path(
                nemesis_config::credentials::credentials_path_for_home(&self.home),
            );
            let security_enabled = cfg.security.as_ref().map(|s| s.enabled).unwrap_or(true);
            let security_plugin =
                crate::security_setup::build_security_plugin(&self.home, security_enabled).await;

            let config_store = Arc::new(nemesis_config::ConfigStore::from_config(
                cfg.clone(),
                config_path,
            ));
            let (event_tx, event_rx) = tokio::sync::broadcast::channel(256);
            let shared = Arc::new(crate::agent_factory::SharedResources {
                home: self.home.clone(),
                workspace: cwd,
                config_store,
                security_plugin,
                mcp_enabled: cfg.mcp.as_ref().map(|m| m.enabled).unwrap_or(false),
                mcp_config_path: crate::common::mcp_config_path(&self.home),
                agent_event_tx: Some(event_tx),
                ..Default::default()
            });
            let agent_loop = crate::agent_factory::build_agent_loop(&shared)
                .map_err(|e| format!("failed to build agent loop: {e}"))?;
            // M7 审批桥：ACP 进程无 dashboard，编辑器即唯一审批面。
            // security feature 关闭时槽是 () 占位，审批整层不存在。
            #[cfg(feature = "security")]
            {
                *shared.approval_slot.write() = Some(Arc::new(AcpApprovalManager {
                    gate: Arc::clone(&gate),
                }));
            }
            let _ = &gate; // no-security 编译下 gate 仍被 session 持有语义引用
            let instance = nemesis_agent::instance::AgentInstance::new(agent_loop.config().clone());
            let session_key = format!("acp:{session_id}");
            Ok(Box::new(LoopDriver {
                agent_loop,
                instance: Arc::new(instance),
                event_rx,
                session_key,
            }) as Box<dyn PromptDriver>)
        })
    }
}

/// 生产驱动：持久 `AgentInstance` 跑 `run_with_trace`（多轮上下文延续），
/// M1a broadcast 实时消费工具事件（run.rs json 模式同款 select 模式——
/// JoinHandle 完成态在 select 分支内接住，完成后再 poll 即 panic）。
struct LoopDriver {
    agent_loop: Arc<nemesis_agent::r#loop::AgentLoop>,
    instance: Arc<nemesis_agent::instance::AgentInstance>,
    event_rx: tokio::sync::broadcast::Receiver<nemesis_types::agent::AgentEvent>,
    session_key: String,
}

/// M1a hook 事件 → `session/update` 载荷。TodoUpdated/ModeChanged/审批/
/// 提问事件 v1 不透（审批走 request_permission 交互；TodoUpdated 无 ACP
/// 对应 update 形态，诚实跳过）。
fn map_hook_event(ev: &nemesis_types::agent::AgentEvent) -> Option<Value> {
    match ev {
        nemesis_types::agent::AgentEvent::ToolStarted {
            call_id,
            tool,
            args_preview,
            ..
        } => Some(tool_started_update(call_id, tool, args_preview)),
        nemesis_types::agent::AgentEvent::ToolFinished {
            call_id,
            ok,
            result_preview,
            ..
        } => Some(tool_finished_update(call_id, *ok, result_preview)),
        _ => None,
    }
}

impl PromptDriver for LoopDriver {
    fn prompt<'a>(
        &'a self,
        text: String,
        sink: UpdateSink,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = TurnOutcome> + Send + 'a>> {
        Box::pin(async move {
            let context = nemesis_agent::context::RequestContext::new(
                "acp",
                &self.session_key,
                "acp",
                &self.session_key,
            );
            let trace_id = format!("acp-{}", uuid::Uuid::new_v4().simple());
            let loop_ = Arc::clone(&self.agent_loop);
            let inst = Arc::clone(&self.instance);
            let cancel_for_run = cancel.clone();
            let mut handle = tokio::spawn(async move {
                loop_
                    .run_with_trace(
                        &inst,
                        &text,
                        &context,
                        &trace_id,
                        false,
                        &cancel_for_run,
                        None,
                        &[],
                    )
                    .await
            });
            let mut rx = self.event_rx.resubscribe();
            let events = loop {
                tokio::select! {
                    biased;
                    res = &mut handle => break res.unwrap_or_default(),
                    ev = rx.recv() => match ev {
                        Ok(ev) => {
                            // 出站通道关闭（客户端断开）：继续跑完，结局
                            // 无人接收而已（`let _ =` 显式吞错）。
                            if let Some(u) = map_hook_event(&ev) {
                                let _ = sink.send(u).await;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            eprintln!("acp: {n} live events dropped (consumer too slow)");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            // sender 全部消失（session 被拆）：等 run 收尾。
                        }
                    },
                }
            };
            // 收尾排干残余工具事件（完成先行发生于 try_recv——最后的
            // tool_end 不丢，编辑器侧不留悬空 in_progress 工具卡；排干后
            // future 结束、sink drop、泵排尽退出，顺序即「update 先于响应」）。
            while let Ok(ev) = rx.try_recv() {
                if let Some(u) = map_hook_event(&ev) {
                    let _ = sink.send(u).await;
                }
            }

            // fold（run_detached 同序：Done 优先；取消优先于一切——token 已
            // 触发时结局必为 cancelled，符合 spec「即使底层异常也回 cancelled」）。
            if cancel.is_cancelled() {
                return TurnOutcome::Cancelled;
            }
            let mut done: Option<String> = None;
            let mut error: Option<String> = None;
            for e in events {
                match e {
                    nemesis_agent::types::AgentEvent::Done(m) => done = Some(m),
                    nemesis_agent::types::AgentEvent::Error(e) => error = Some(e),
                    _ => {}
                }
            }
            match (done, error) {
                (Some(m), _) => TurnOutcome::Completed(m),
                (None, Some(e)) => TurnOutcome::Failed(e),
                (None, None) => TurnOutcome::Failed("agent produced no output".to_string()),
            }
        })
    }
}

// ---------------------------------------------------------------------------
// 入口
// ---------------------------------------------------------------------------

/// `nemesisbot acp`：stdio 上跑 ACP server 直到 EOF。日志全走 stderr
/// （main 侧 `ensure_default_logger` 为 stderr 形态；stdout 是协议帧专用
/// 通道，任何打印都会污染协议流）。
pub async fn run_server(home: PathBuf) -> Result<(), String> {
    let factory = Arc::new(RealSessionFactory { home });
    let version = crate::common::format_version();
    serve(tokio::io::stdin(), tokio::io::stdout(), factory, version).await
}

#[cfg(test)]
mod tests;
