//! 会话钩子自管面（2026-11-16 HOOK 会话圈定 goal 批次三）：`hooks` 工具——
//! 单工具多 action（add/delete/list），让 agent 在对话里给自己（当前会话）
//! 设置生命周期钩子。
//!
//! **只能给自己设置是结构保证**：schema 无 session 参数，sid 一律取
//! `context.session_key`（剥 [`WEB_SESSION_KEY_PREFIX`]）——模型想给别人设
//! 都没有入参通道。
//!
//! 存储与生效链路：写 `hooks.session.json`（[`write_session_hook_store`]
//! 原子写）→ 桥（`CcHookBridge`）按内容比对热更在下次分发时捡到（工具不
//! 直改桥内存，单一写入路径）。桥缺席（双文件双缺的冷启动）= 已落盘但无
//! 热更载体，诚实提示「重启 Agent 后生效」。
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
    SESSION_HOOK_EVENTS, SessionHookEntry, SessionHookStore, WEB_SESSION_KEY_PREFIX,
    read_session_hook_store, write_session_hook_store,
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
    /// `<workspace>/config`（hooks.session.json 落位目录）。
    config_dir: PathBuf,
    /// 主 loop 的 cc 桥（None = 冷启动双缺，无热更载体——响应诚实降级）。
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
        matcher: Option<String>,
        timeout: Option<u64>,
    },
    Delete {
        id: String,
    },
    List,
}

/// args JSON → 结构化子命令。event 合法性 / command 非空 / timeout 数值型
/// 在这里守（args_validator 兜 schema 层，语义层自己守）。
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
            if command.is_empty() {
                return Err("add: missing field \"command\"（要执行的 shell 命令）".to_string());
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

/// 本会话已有条目里的最大编号（`h<N>` 形态；无 → 0）。
fn max_hook_index(per_event: &std::collections::BTreeMap<String, Vec<SessionHookEntry>>) -> usize {
    let mut max = 0;
    for entries in per_event.values() {
        for e in entries {
            if let Some(n) = e.id.strip_prefix('h').and_then(|n| n.parse::<usize>().ok()) {
                max = max.max(n);
            }
        }
    }
    max
}

/// 本会话条目的单行渲染（add 响应与 list 共用）。
fn render_entry(event: &str, e: &SessionHookEntry) -> String {
    let matcher = if e.matcher.is_empty() {
        "全部工具".to_string()
    } else {
        format!("matcher={}", e.matcher)
    };
    let timeout = e
        .timeout
        .map(|t| t.to_string())
        .unwrap_or_else(|| "默认".to_string());
    format!(
        "- [{}] {} · {matcher} · 超时 {timeout}s · `{}`",
        e.id, event, e.command
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
         `add` registers a shell command that the gateway runs on a lifecycle event \
         (PreToolUse can block tools with exit 2, UserPromptSubmit can block prompts, \
         others observe); `delete` removes one by id; `list` shows this session's hooks. \
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
                    "description": "add: shell command to run. Receives the event payload as JSON on stdin. Exit 2 blocks (PreToolUse/UserPromptSubmit/SubagentStart); exit 0 passes."
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
                let mut store = read_session_hook_store(&self.config_dir);
                let per_event = store.hooks.entry(sid.clone()).or_default();
                let next_index = max_hook_index(per_event) + 1;
                let id = format!("h{next_index}");
                let entry = SessionHookEntry {
                    id: id.clone(),
                    matcher: matcher.unwrap_or_default(),
                    command: command.clone(),
                    timeout,
                };
                let rendered = render_entry(&event, &entry);
                per_event.entry(event.clone()).or_default().push(entry);
                write_session_hook_store(&self.config_dir, &store)?;
                tracing::info!("[HooksTool] session hook added: sid={sid} id={id} event={event}");
                let mut out = format!(
                    "已为当前会话添加钩子 {id}（{event}）{clamped_note}\n{rendered}\n{}",
                    activation_note(self.bridge.as_ref()),
                );
                if event == "SessionStart" {
                    out.push_str(
                        "\n注意：SessionStart 在本会话已错过——该钩子从下个会话开始时触发。",
                    );
                }
                Ok(out)
            }
            HooksCommand::Delete { id } => {
                let mut store = read_session_hook_store(&self.config_dir);
                let Some(per_event) = store.hooks.get_mut(&sid) else {
                    return Err(format!("本会话（{sid}）当前没有已设置的钩子"));
                };
                let mut removed: Option<(String, SessionHookEntry)> = None;
                for (event, entries) in per_event.iter_mut() {
                    if let Some(pos) = entries.iter().position(|e| e.id == id) {
                        let e = entries.remove(pos);
                        let event = event.clone();
                        removed = Some((event, e));
                        break;
                    }
                }
                let Some((event, e)) = removed else {
                    let ids: Vec<String> = per_event
                        .values()
                        .flat_map(|v| v.iter().map(|e| e.id.clone()))
                        .collect();
                    return Err(if ids.is_empty() {
                        format!("本会话（{sid}）当前没有已设置的钩子")
                    } else {
                        format!("未找到钩子 {id:?}——本会话现有 id：{}", ids.join(", "))
                    });
                };
                // 全空则整键摘除（存储不积攒空壳；is_empty 语义干净）。
                if per_event.values().all(Vec::is_empty) {
                    store.hooks.remove(&sid);
                }
                write_session_hook_store(&self.config_dir, &store)?;
                tracing::info!("[HooksTool] session hook deleted: sid={sid} id={id}");
                Ok(format!(
                    "已删除当前会话钩子 {id}（{event}）\n{}\n{}",
                    render_entry(&event, &e),
                    activation_note(self.bridge.as_ref()),
                ))
            }
            HooksCommand::List => {
                let store: SessionHookStore = read_session_hook_store(&self.config_dir);
                let Some(per_event) = store.hooks.get(&sid) else {
                    return Ok(format!(
                        "本会话（{sid}）当前没有已设置的钩子。\n全局钩子（对全部会话生效）请在 Dashboard Hooks 页或 hooks.json 配置。"
                    ));
                };
                let mut lines = Vec::new();
                for (event, entries) in per_event {
                    for e in entries {
                        lines.push(render_entry(event, e));
                    }
                }
                if lines.is_empty() {
                    return Ok(format!(
                        "本会话（{sid}）当前没有已设置的钩子。\n全局钩子（对全部会话生效）请在 Dashboard Hooks 页或 hooks.json 配置。"
                    ));
                }
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
