//! 邮件基础设施 B4：`mail` agent 工具（单工具多 action，forge/board_issue
//! 同款哲学——压工具表上下文）。
//!
//! `action = send | list | read` 三动作：
//! - `send`：外发（to/subject/body）——走完整安全 8 层（network 分档）；
//! - `list`：列信（未读过滤/分页/按地址分库）；
//! - `read`：按邮件 id 读全文。
//!
//! 结构性边界（契约同文，`nemesis-plugin` mail.rs「能力矩阵」节）：
//! - **无 delete**——schema 层面不存在（非隐藏）。删信只经宿主治理面
//!   （WSAPI `mail.*` / CLI `mail`）。邮件库在 `<home>/mail/`（workspace
//!   围栏外），LLM 文件工具结构性够不着——防删信掩盖的纵深不止 schema。
//! - **地址管理不在 agent 面**——`mail_addr_*` 只开放宿主治理面；agent
//!   地址申请随 v2 规则管线（v2 红线）。
//!
//! 装载条件（`agent_factory::build_agent_loop`）：handle 缺席（总闸关 /
//! 名册关 / DLL 缺席 / 装载失败）= 工具不注册（board_issue store 缺席同款
//! 先例）。gateway 装载链先于 agent loop 构造（GatewayCtx 构造期 load，
//! post_agent 后段建 loop），注册时判定无时序缺口；run/acp headless 不
//! load 插件 = 自然不注册。
//!
//! **工具面热同步**（2026-11-20 第二批；wasm ToolSync hook 同款先例）：
//! loop 构造期 handle 缺席未注册 + 运行期热装载成功 = 经
//! [`set_tool_sync_hook`] 静态槽通知 post_agent 注入的闭包，把本工具补
//! 注册进当前 AgentLoop（[`sync_mail_tool_into_loop`] 幂等——已注册 =
//! 无操作）。停机窗口（current() None）= 通知丢弃诚实降级：agent 重启
//! 路径 agent_factory 注册段按装载态拉起。
//!
//! tier：`min_tier("normal")` 双闸（供给 + dispatch 秩比较，插件复查 #2
//! 同款机制）——防 mini 误用外传面。
//!
//! 安全分档：`operation_override` 按 action 投影（workflow 三合一先例）：
//! `send` → `network_request`（外发面，8 层全跑 + 网络工具族同档）；
//! `list`/`read` → `internal`（LOW 纯查询）。args 畸形 = None → 静态表
//! 兜底按 NetworkRequest（保守——send 是最危险动作，宁过勿漏）。
//!
//! 调用链：execute → `call_mail`（spawn_blocking 包装——契约「宿主调用侧
//! 必须 spawn_blocking，不卡 async executor 线程」）→ `MailPluginHandle`
//! fn 指针 → 插件内 block_on 自持 runtime。

use std::sync::Arc;

use nemesis_agent::context::RequestContext;

/// 工具注册名。
pub const TOOL_NAME: &str = "mail";

// ---------------------------------------------------------------------------
// 工具面热同步槽（2026-11-20 第二批；plugins_wasm `set_tool_sync_hook`
// 同款静态槽形态——OnceLock 首个胜出，gateway 生命周期内注入一次）
// ---------------------------------------------------------------------------

type ToolSyncFn = Arc<dyn Fn() + Send + Sync>;
static TOOL_SYNC: std::sync::OnceLock<ToolSyncFn> = std::sync::OnceLock::new();

/// 注入工具面热同步闭包（gateway post_agent 装配期；幂等失败 = 重复注入，
/// 首个胜出——闭包捕获 adapter Arc，`current()` 每次解析最新 loop，agent
/// 重启自动跟进）。
pub fn set_tool_sync_hook(f: ToolSyncFn) {
    let _ = TOOL_SYNC.set(f);
}

/// 触发工具面热同步（热装载闭包 Loaded 臂调用；hook 未注入 = 无 loop 可
/// 同步（CLI/测试形态），静默略过）。
pub fn notify_tool_sync() {
    if let Some(h) = TOOL_SYNC.get() {
        h();
    }
}

/// 热同步体（幂等）：注册表已有 mail 工具 = `false` 无操作（重触发 /
/// 装配期已注册）；未注册 = 注册 [`MailTool`] 并回 `true`。
///
/// 安全分档无需 `declare_tool_operation`——`mail` 在
/// `nemesis_security::tool_to_operation` 静态表（NetworkRequest 兜底）+
/// [`MailTool::operation_override`] 按 action 精确投影，dispatch 动态
/// 消费（tool_dispatch 嗅探臂）；tier 闸同样动态（build_tool_defs 供给
/// 过滤 + dispatch 秩比较，注册时机无关）。
pub(crate) fn sync_mail_tool_into_loop(loop_arc: &nemesis_agent::r#loop::AgentLoop) -> bool {
    if loop_arc.tool_names().iter().any(|n| n == TOOL_NAME) {
        return false;
    }
    let actual = loop_arc.register_plugin_tool(TOOL_NAME.to_string(), Arc::new(MailTool::new()));
    tracing::info!("[Mail] tool hot-registered: {actual}");
    true
}

/// 解析后的子命令（execute 内部；独立成纯函数便于单测——插件链路归 UAT U12）。
#[derive(Debug)]
pub(crate) enum MailCommand {
    Send {
        to: String,
        subject: String,
        body: String,
    },
    List {
        mailbox: String,
        unread_only: bool,
        limit: u32,
        offset: u32,
    },
    Read {
        id: String,
    },
}

/// args JSON → 结构化子命令（缺字段 / 空 to / 空 id 一律诚实报错——
/// args_validator 兜的是 schema 层，语义层这里自己守）。
pub(crate) fn parse_mail_args(args: &str) -> Result<MailCommand, String> {
    let v: serde_json::Value =
        serde_json::from_str(args).map_err(|e| format!("invalid JSON args: {e}"))?;
    let action = v
        .get("action")
        .and_then(|x| x.as_str())
        .ok_or("missing field: action (expected \"send\", \"list\" or \"read\")")?;
    match action {
        "send" => {
            let to = v
                .get("to")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if to.is_empty() {
                return Err("send requires a non-empty \"to\" recipient address".to_string());
            }
            let subject = v
                .get("subject")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            let body = v
                .get("body")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            Ok(MailCommand::Send { to, subject, body })
        }
        "list" => {
            let mailbox = v
                .get("mailbox")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let unread_only = v
                .get("unread_only")
                .and_then(|x| x.as_bool())
                .unwrap_or(false);
            let limit = match v.get("limit") {
                None | Some(serde_json::Value::Null) => 20,
                Some(n) => {
                    let n = n.as_u64().ok_or("limit must be a non-negative integer")?;
                    if n == 0 || n > 100 {
                        return Err("limit must be in 1..=100".to_string());
                    }
                    n as u32
                }
            };
            let offset = match v.get("offset") {
                None | Some(serde_json::Value::Null) => 0,
                Some(n) => {
                    let n = n.as_u64().ok_or("offset must be a non-negative integer")?;
                    n as u32
                }
            };
            Ok(MailCommand::List {
                mailbox,
                unread_only,
                limit,
                offset,
            })
        }
        "read" => {
            let id = v
                .get("id")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if id.is_empty() {
                return Err("read requires a non-empty \"id\" (mail id from list)".to_string());
            }
            Ok(MailCommand::Read { id })
        }
        other => Err(format!(
            "unknown action {other:?} (expected \"send\", \"list\" or \"read\")"
        )),
    }
}

/// agent 邮件工具：收发与读取（删信/地址注册不在面）。
///
/// 无状态——handle 经 loader 全局槽（[`crate::mail_plugin_loader`]
/// `call_mail`）按调用取，注册判定在 agent_factory。
pub struct MailTool;

impl MailTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for MailTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl nemesis_agent::r#loop::Tool for MailTool {
    fn description(&self) -> String {
        "Send email, list the inbox, or read a message via the local mail \
         infrastructure. Use `send` when the user asks to email someone \
         (\"给 zoo@example.com 发封邮件…\"); use `list` to browse messages \
         (optionally filtered to a specific registered address mailbox or \
         unread-only, with pagination); use `read` with an id from `list` \
         to fetch the full message. There is NO delete action — message \
         deletion and address registration are host-administration \
         operations only (Dashboard/CLI), not available to the agent."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        // 拍平平铺 schema（board_issue 同款纪律：oneOf 分支形态小模型
        // 构造失败实录）。顶层声明全部字段，required 只留 action，分支
        // 专属 required（send.to / read.id）由 parse_mail_args 语义守兜底。
        // **无 delete**——schema 层面不存在（能力矩阵「不可删」红线）。
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["send", "list", "read"],
                    "description": "\"send\" = send an email; \"list\" = list inbox messages; \"read\" = read a full message by id."
                },
                "to": { "type": "string", "description": "send: recipient address. Required for send." },
                "subject": { "type": "string", "description": "send: subject line (may be empty)." },
                "body": { "type": "string", "description": "send: message body (may be empty)." },
                "mailbox": { "type": "string", "description": "list: registered address mailbox to read from; omit for the default mailbox." },
                "unread_only": { "type": "boolean", "description": "list: only show unread messages (default false)." },
                "limit": { "type": "integer", "description": "list: page size 1-100 (default 20)." },
                "offset": { "type": "integer", "description": "list: pagination offset (default 0)." },
                "id": { "type": "string", "description": "read: mail id from a previous list call. Required for read." }
            },
            "required": ["action"]
        })
    }

    /// tier 双闸（供给 + dispatch 秩比较）：mini 档不可见不可调，normal/big
    /// 可用（计划 B4「默认 normal+，防 mini 误用外传面」）。
    fn min_tier(&self) -> Option<&str> {
        Some("normal")
    }

    /// 按 action 投影精确分档（workflow 三合一先例）：send = 外发网络面，
    /// list/read = LOW 纯查询。args 畸形 = None → 静态表兜底按保守档。
    fn operation_override(&self, args: &str) -> Option<nemesis_agent::r#loop::OperationOverride> {
        let action = serde_json::from_str::<serde_json::Value>(args)
            .ok()
            .and_then(|v| v.get("action").and_then(|a| a.as_str()).map(String::from));
        match action.as_deref() {
            Some("send") => Some(nemesis_agent::r#loop::OperationOverride {
                op_type: "network_request".to_string(),
                force_approval: None,
                receipt_ctx: None,
            }),
            Some("list") | Some("read") => Some(nemesis_agent::r#loop::OperationOverride {
                op_type: "internal".to_string(),
                force_approval: None,
                receipt_ctx: None,
            }),
            _ => None,
        }
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let cmd = parse_mail_args(args)?;
        match cmd {
            MailCommand::Send { to, subject, body } => {
                let to_out = to.clone(); // 闭包 'static 拿走所有权，回执文案用克隆
                let out = crate::mail_plugin_loader::call_mail("mail.send", move |h| {
                    h.send(&to, &subject, &body)
                })
                .await?;
                let message_id = out.get("message_id").and_then(|m| m.as_str()).unwrap_or("");
                if message_id.is_empty() {
                    Ok(format!("邮件已发送到 {to_out}。"))
                } else {
                    Ok(format!(
                        "邮件已发送到 {to_out}（message_id: {message_id}）。"
                    ))
                }
            }
            MailCommand::List {
                mailbox,
                unread_only,
                limit,
                offset,
            } => {
                // 契约：list query JSON 四键齐，缺省补默认（mailbox 空 = 默认主库）。
                let query = serde_json::json!({
                    "mailbox": mailbox,
                    "unread_only": unread_only,
                    "limit": limit,
                    "offset": offset,
                });
                let out = crate::mail_plugin_loader::call_mail("mail.list", move |h| {
                    h.list(&query.to_string())
                })
                .await?;
                Ok(serde_json::to_string_pretty(&out).unwrap_or_else(|_| out.to_string()))
            }
            MailCommand::Read { id } => {
                let out =
                    crate::mail_plugin_loader::call_mail("mail.read", move |h| h.read(&id)).await?;
                Ok(serde_json::to_string_pretty(&out).unwrap_or_else(|_| out.to_string()))
            }
        }
    }
}

#[cfg(test)]
mod tests;
