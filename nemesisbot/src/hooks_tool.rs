//! 会话钩子自管面（2026-11-16 HOOK 会话圈定 goal 批次三；2026-11-19 统一
//! 存储重构）：`hooks` 工具——单工具多 action（add/delete/list），让 agent
//! 在对话里给自己（当前会话）设置生命周期钩子。
//!
//! **只能给自己设置是结构保证**：schema 无 session 参数，sid 一律取
//! `context.session_key`（剥 [`WEB_SESSION_KEY_PREFIX`]）——模型想给别人设
//! 都没有入参通道。
//!
//! 存储与生效链路（统一存储）：写 `hooks.json` 的工具专属组（scope 圈定
//! 单会话；[`add_session_hook`] 等 RMW API 原子写，与 Dashboard hooks.set
//! 共享文件互斥）→ 桥（`CcHookBridge`）按内容比对热更在下次分发时捡到
//! （工具不直改桥内存，单一写入路径）。桥缺席（此前无任何钩子的冷启动）
//! = 已落盘但无热更载体，诚实提示「重启 Agent 后生效」。
//!
//! 注册面：仅主 gateway loop（`register_hooks_tool`）；集群走
//! `register_tools_and_mcp` 公共路径天然排除；run/acp headless 不装。
//!
//! 安全：add/delete 投影 file_write + `force_approval` 强审批卡（默认
//! 策略下 file_write 只审计放行，强审批把「钩子安装/移除必须过人」变
//! 真）；`list` 经 `operation_override` 投影 internal（只读不审批）。
//! tier Normal+。

use std::path::PathBuf;
use std::sync::Arc;

use nemesis_agent::cc_hooks::{
    HookEntryView, PROMPT_HOOK_EVENTS, SESSION_HOOK_EVENTS, WEB_SESSION_KEY_PREFIX,
    add_session_hook, delete_session_hook, list_session_hooks,
};
use nemesis_agent::context::RequestContext;

/// 工具注册名。
pub const TOOL_NAME: &str = "hooks";

/// add 缺省超时（秒）。
pub(crate) const DEFAULT_TIMEOUT_SECS: u64 = 60;
/// add 超时上限（秒）——超过钳到该值（响应里注明）。
pub(crate) const MAX_TIMEOUT_SECS: u64 = 600;

/// hooks 工具：给自己（当前会话）增删查生命周期钩子。
pub struct HooksTool {
    /// `<workspace>/config`（hooks.json 落位目录）。
    config_dir: PathBuf,
    /// 主 loop 的 cc 桥（None = 冷启动无钩子，无热更载体——响应诚实降级）。
    bridge: Option<Arc<nemesis_agent::cc_hooks::CcHookBridge>>,
}

impl HooksTool {
    pub fn new(
        config_dir: PathBuf,
        bridge: Option<Arc<nemesis_agent::cc_hooks::CcHookBridge>>,
    ) -> Self {
        Self { config_dir, bridge }
    }

    /// 当前会话的裸 sid（存储键；整键兜底——桥侧 lookup 同款双形态）。
    fn sid_of(context: &RequestContext) -> String {
        context
            .session_key
            .strip_prefix(WEB_SESSION_KEY_PREFIX)
            .unwrap_or(&context.session_key)
            .to_string()
    }
}

/// 解析后的子命令（execute 内部；独立纯函数便于单测）。
#[derive(Debug)]
pub(crate) enum HooksCommand {
    Add {
        event: String,
        command: String,
        /// 提示词型钩子（LLM 执行体；仅 Stop 事件合法）。command 与
        /// prompt 恰好一项非空（Exactly one，双非空混合条目 loud 拒绝）
        /// ——prompt 非空即 prompt 型条目。
        prompt: String,
        matcher: Option<String>,
        timeout: Option<u64>,
    },
    Delete {
        id: String,
    },
    List,
}

/// args JSON → 结构化子命令。event 合法性 / command+prompt 二选一非空 /
/// timeout 数值型在这里守（args_validator 兜 schema 层，语义层自己守）。
pub(crate) fn parse_hooks_args(args: &str) -> Result<HooksCommand, String> {
    let v: serde_json::Value =
        serde_json::from_str(args).map_err(|e| format!("invalid JSON args: {e}"))?;
    let action = v
        .get("action")
        .and_then(|x| x.as_str())
        .ok_or("missing field: action (expected \"add\", \"delete\" or \"list\")")?;
    match action {
        "add" => {
            let event = v
                .get("event")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if event.is_empty() {
                return Err(
                    "add: missing field \"event\"（12 个合法事件名见 schema enum）".to_string(),
                );
            }
            if !SESSION_HOOK_EVENTS.contains(&event.as_str()) {
                return Err(format!(
                    "add: 未知 hook 事件名 {event:?}（合法：{}）",
                    SESSION_HOOK_EVENTS.join(", ")
                ));
            }
            let command = v
                .get("command")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let prompt = v
                .get("prompt")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            // 二选一非空：command 型（shell 执行体）或 prompt 型（LLM 执行体）。
            if command.is_empty() && prompt.is_empty() {
                return Err(
                    "add: \"command\" 与 \"prompt\" 至少一项非空（command=shell 执行体；\
                     prompt=提示词注入，turn 结束后作为新用户消息开新轮）"
                        .to_string(),
                );
            }
            // 恰好一项（schema 契约 Exactly one）：双非空的混合条目语义未定
            // （shell 执行路径不读 prompt → command/prompt 必有一侧被静默
            // 忽略）——loud 拒绝而非静默偏向。
            if !command.is_empty() && !prompt.is_empty() {
                return Err(
                    "add: \"command\" 与 \"prompt\" 只能二选一（command=shell 执行体；\
                     prompt=提示词注入）——两类执行体不可混合"
                        .to_string(),
                );
            }
            // prompt 型仅 PROMPT_HOOK_EVENTS 合法（单一真相源；今天只有 Stop
            // ——唯一有「turn 结束」语义的钩点）。
            if !prompt.is_empty() && !PROMPT_HOOK_EVENTS.contains(&event.as_str()) {
                return Err(format!(
                    "add: prompt 型钩子仅在 {} 事件合法（收到 {event:?}）——\
                     prompt 语义是「本轮结束后注入提示词开新轮」，只有 Stop 有这个时机",
                    PROMPT_HOOK_EVENTS.join("/")
                ));
            }
            let matcher = v
                .get("matcher")
                .and_then(|x| x.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            let timeout = match v.get("timeout") {
                None | Some(serde_json::Value::Null) => None,
                Some(x) => Some(x.as_u64().ok_or("add: \"timeout\" 必须为正整数秒")?),
            };
            Ok(HooksCommand::Add {
                event,
                command,
                prompt,
                matcher,
                timeout,
            })
        }
        "delete" => {
            let id = v
                .get("id")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if id.is_empty() {
                return Err(
                    "delete: missing field \"id\"（add/list 返回的钩子 id，如 \"h1\"）".to_string(),
                );
            }
            Ok(HooksCommand::Delete { id })
        }
        "list" => Ok(HooksCommand::List),
        other => Err(format!(
            "未知 action {other:?}（合法：add / delete / list）"
        )),
    }
}

/// 条目的单行渲染（add 响应与 list 共用）。prompt 型条目带
/// 「提示词」标注——command 与 prompt 是两种执行体，list 面必须可区分。
fn render_entry(view: &HookEntryView) -> String {
    let matcher = if view.matcher.is_empty() {
        "全部工具".to_string()
    } else {
        format!("matcher={}", view.matcher)
    };
    let timeout = view
        .timeout
        .map(|t| t.to_string())
        .unwrap_or_else(|| "默认".to_string());
    if !view.prompt.is_empty() {
        // 提示词型（LLM 执行体）：Stop 钩点注入提示词开新轮，无 shell。
        let preview: String = view.prompt.chars().take(60).collect();
        let ellipsis = if view.prompt.chars().count() > 60 {
            "…"
        } else {
            ""
        };
        return format!(
            "- [{}] {} · 提示词型 · {matcher} · 「{preview}{ellipsis}」",
            view.id, view.event
        );
    }
    format!(
        "- [{}] {} · {matcher} · 超时 {timeout}s · `{}`",
        view.id, view.event, view.command
    )
}

/// 桥在/不在的生效语义文案（None = 冷启动无热更载体，诚实降级）。
fn activation_note(bridge: Option<&Arc<nemesis_agent::cc_hooks::CcHookBridge>>) -> &'static str {
    if bridge.is_some() {
        "落盘即热更，下条消息起生效。"
    } else {
        "已落盘；Agent 启动时未加载钩子桥（冷启动），重启 Agent 后生效。"
    }
}

#[async_trait::async_trait]
impl nemesis_agent::r#loop::Tool for HooksTool {
    fn description(&self) -> String {
        "Manage lifecycle hooks **for the current session only** (session-scoped hooks). \
         `add` registers either a shell command or a prompt hook: shell hooks run a command \
         on a lifecycle event (PreToolUse can block tools with exit 2, UserPromptSubmit can \
         block prompts, others observe); prompt hooks (Stop event only) re-inject a prompt \
         as a new user message after each turn ends — the loop repeats naturally with no \
         turn cap (stop by deleting the hook, estop, or asking the agent to hooks delete). \
         `delete` removes one by id; `list` shows this session's hooks. \
         There is no session parameter on purpose — hooks always attach to the session \
         you are running in. add/delete write config and require user approval (HIGH); \
         list is read-only. Note: a SessionStart hook added mid-session takes effect \
         from the NEXT session start."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        // 平铺 schema（board_issue 同款；压测 D 场景教训——oneOf 分支形态
        // 小模型构造不出）。**刻意无 session 参数**：只能给自己设的结构保证。
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["add", "delete", "list"],
                    "description": "\"add\" = register a hook for this session; \"delete\" = remove one by id; \"list\" = show this session's hooks."
                },
                "event": {
                    "type": "string",
                    "enum": SESSION_HOOK_EVENTS,
                    "description": "add: lifecycle event to attach to. PreToolUse/PostToolUse(+Failure) fire around tool calls (matcher applies); SessionStart/UserPromptSubmit/Stop fire on conversation lifecycle; SessionEnd/PreCompact/PostCompact observe; SubagentStart/SubagentStop fire on subagent spawn/finish; Notification fires on approval/question boundaries."
                },
                "command": {
                    "type": "string",
                    "description": "add: shell command to run. Receives the event payload as JSON on stdin. Exit 2 blocks (PreToolUse/UserPromptSubmit/SubagentStart); exit 0 passes. Exactly one of command/prompt must be non-empty."
                },
                "prompt": {
                    "type": "string",
                    "description": "add: prompt text for a prompt-type hook (Stop event ONLY). After each turn ends, this prompt is injected as a new user message starting a new round — the loop repeats naturally (persistent self-review). Stop it by deleting the hook or via estop. timeout is ignored for prompt hooks."
                },
                "matcher": {
                    "type": "string",
                    "description": "add: optional tool-name regex for tool events (PreToolUse/PostToolUse/PostToolUseFailure); empty = all tools."
                },
                "timeout": {
                    "type": "integer",
                    "description": "add: optional per-run timeout in seconds (default 60, capped at 600)."
                },
                "id": {
                    "type": "string",
                    "description": "delete: hook id as returned by add/list (e.g. \"h1\")."
                }
            },
            "required": ["action"]
        })
    }

    /// list 投影 internal（只读不审批）；add/delete 投影 file_write +
    /// **强审批旗标**（`force_approval`——默认安全策略下 file_write 是
    /// allow 只审计，钩子安装/移除 = 写入任意命令的生命周期配置，必须
    /// 过人：强审批把卡变真，且跳过「总是允许」规则表自动放行；会话级
    /// 全部放行仍按其既有裁决语义覆盖）。解析失败 → None（落静态声明
    /// 表 FileWrite，语义层报错）。
    fn operation_override(&self, args: &str) -> Option<nemesis_agent::r#loop::OperationOverride> {
        let action = serde_json::from_str::<serde_json::Value>(args)
            .ok()
            .and_then(|v| v.get("action").and_then(|a| a.as_str()).map(String::from));
        match action.as_deref() {
            Some("list") => Some(nemesis_agent::r#loop::OperationOverride {
                op_type: "internal".to_string(),
                force_approval: None,
                receipt_ctx: None,
            }),
            Some("add") | Some("delete") => Some(nemesis_agent::r#loop::OperationOverride {
                op_type: "file_write".to_string(),
                force_approval: Some(
                    "hooks 工具写入/移除会话钩子（生命周期任意命令配置）需用户确认".to_string(),
                ),
                receipt_ctx: None,
            }),
            _ => None,
        }
    }

    fn min_tier(&self) -> Option<&str> {
        Some("normal")
    }

    async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String> {
        let cmd = parse_hooks_args(args)?;
        if context.session_key.is_empty() {
            return Err(
                "无法确定当前会话（session_key 为空）——会话钩子必须挂在具体会话上".to_string(),
            );
        }
        let sid = Self::sid_of(context);
        match cmd {
            HooksCommand::Add {
                event,
                command,
                prompt,
                matcher,
                timeout,
            } => {
                // 超时缺省 + 上限钳制（0 拒绝；>600 钳到 600 并注明）。
                let timeout = match timeout {
                    None => Some(DEFAULT_TIMEOUT_SECS),
                    Some(0) => return Err("add: \"timeout\" 必须为正秒数（0 无意义）".to_string()),
                    Some(t) if t > MAX_TIMEOUT_SECS => Some(MAX_TIMEOUT_SECS),
                    Some(t) => Some(t),
                };
                let clamped_note = if timeout == Some(MAX_TIMEOUT_SECS) {
                    "（超时已钳到上限 600s）"
                } else {
                    ""
                };
                let matcher_str = matcher.unwrap_or_default();
                // 统一存储 RMW API：id 分配/工具专属组建组/原子写全部在
                // cc_hooks 侧（与 Dashboard hooks.set 同一文件互斥闸）。
                let id = add_session_hook(
                    &self.config_dir,
                    &sid,
                    &event,
                    &matcher_str,
                    &command,
                    &prompt,
                    timeout,
                )?;
                let view = HookEntryView {
                    event: SESSION_HOOK_EVENTS
                        .iter()
                        .find(|e| **e == event.as_str())
                        .copied()
                        .unwrap_or(""),
                    id: id.clone(),
                    matcher: matcher_str,
                    command: command.clone(),
                    prompt: prompt.clone(),
                    timeout,
                };
                let rendered = render_entry(&view);
                tracing::info!(
                    "[HooksTool] session hook added: sid={sid} id={id} event={event} kind={}",
                    if prompt.is_empty() {
                        "command"
                    } else {
                        "prompt"
                    }
                );
                let mut out = format!(
                    "已为当前会话添加钩子 {id}（{event}）{clamped_note}\n{rendered}\n{}",
                    activation_note(self.bridge.as_ref()),
                );
                if !prompt.is_empty() {
                    // 提示词型钩子的语义说明（永续循环 = 每轮结束 Stop 钩点
                    // 再触发；estop/删除钩子即可停止）。
                    out.push_str(
                        "\n说明：这是提示词型钩子（LLM 执行体）——每轮对话结束时，\
                         提示词会作为新的用户消息注入本会话开启新轮；如此往复自然循环，\
                         没有轮数上限。停止方式：删除本钩子、触发急停，或让 Agent \
                         用 hooks delete 自行移除。timeout 对本型无效（不跑 shell）。",
                    );
                }
                if event == "SessionStart" {
                    out.push_str(
                        "\n注意：SessionStart 在本会话已错过——该钩子从下个会话开始时触发。",
                    );
                }
                Ok(out)
            }
            HooksCommand::Delete { id } => {
                match delete_session_hook(&self.config_dir, &sid, &id) {
                    Ok(view) => {
                        tracing::info!("[HooksTool] session hook deleted: sid={sid} id={id}");
                        Ok(format!(
                            "已删除当前会话钩子 {id}（{}）\n{}\n{}",
                            view.event,
                            render_entry(&view),
                            activation_note(self.bridge.as_ref()),
                        ))
                    }
                    Err(e) => {
                        // 会话一个钩子都没有 → 友好空态文案；其余（id 不存在
                        // 等）原样透传（API 错误已带现有 id 清单）。
                        if list_session_hooks(&self.config_dir, &sid)
                            .map(|v| v.is_empty())
                            .unwrap_or(false)
                        {
                            return Err(format!("本会话（{sid}）当前没有已设置的钩子"));
                        }
                        Err(e)
                    }
                }
            }
            HooksCommand::List => {
                let views = list_session_hooks(&self.config_dir, &sid)?;
                if views.is_empty() {
                    return Ok(format!(
                        "本会话（{sid}）当前没有已设置的钩子。\n全局钩子（对全部会话生效）请在 Dashboard Hooks 页或 hooks.json 配置。"
                    ));
                }
                let lines: Vec<String> = views.iter().map(render_entry).collect();
                Ok(format!(
                    "本会话（{sid}）共 {} 个钩子（只对本会话生效）：\n{}",
                    lines.len(),
                    lines.join("\n")
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests;
