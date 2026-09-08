//! Chat handler — 会话维护 WSAPI。
//!
//! E6（2026-09-05）：`chat.compact` / `chat.clear`；H1（2026-09-05）：
//! `chat.todo_get`；M5（2026-09-05）：`chat.context_status`（context 占用快照）；
//! F1（2026-09-05）：`chat.set_mode` / `chat.get_mode`（plan/build 双模式）；
//! L2（2026-09-06）：`chat.sync`（WS chat 帧断线补拉，per-session 环形缓冲）。
//!
//! 双入口对齐：与聊天侧 `/compact` `/clear` slash 命令走**同一 AgentLoop
//! 实现**（`compact_session` / `clear_session`，busy 闸 + 摘要持久化 +
//! chat_log 截断同源），不绕过 loop——对比 `sessions.clear` 的直改 store
//! 管理路径（那条路径不清 loop 侧 busy 语义，也不走摘要契约）。
//!
//! 注意：`chat` 作为 **request 型** module 与 message 型的 `chat` 模块
//! （chat.send / history_request，websocket_handler 分派）互不相干——
//! WsRouter 只接 `type=="request"`。

use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_agent::r#loop::AgentLoop;
use std::sync::Arc;

pub struct ChatHandler;

#[async_trait::async_trait]
impl ModuleHandler for ChatHandler {
    fn module_name(&self) -> &str {
        "chat"
    }

    fn commands(&self) -> &'static [&'static str] {
        &[
            "compact",
            "clear",
            "todo_get",
            "context_status",
            "set_mode",
            "get_mode",
            "sync",
        ]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        // L2（devtool-upgrade 阶段 6）：WS chat 帧断线补拉——useWebSocket
        // 重连点调用。只读 + 不依赖 AgentLoop（提前返回，不占 loop 句柄）。
        // session_id 缺省取本连接会话；after_seq 缺省 0（重放窗口内全量）。
        // 环形缓冲按 **agent 会话键** 记录（chat_id 是连接级 id，重连即变；
        // OutboundMeta.session_key 随行盖章），这里同构变换出键再查。
        if cmd == "sync" {
            let sync_session = data
                .as_ref()
                .and_then(|d| d.get("session_id"))
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or_else(|| ctx.session_id.clone());
            let session_key = format!(
                "agent:main:session:{}",
                nemesis_agent::session::SessionStore::sanitize_session_id(&sync_session)
            );
            let after_seq = data
                .as_ref()
                .and_then(|d| d.get("after_seq"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let (events, gap) = crate::chat_event_log::replay_after(&session_key, after_seq);
            return Ok(Some(serde_json::json!({
                "session_id": sync_session,
                "session_key": session_key,
                "after_seq": after_seq,
                "gap": gap,
                "events": events,
            })));
        }
        let session_id = data
            .as_ref()
            .and_then(|d| d.get("session_id"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing session_id".to_string())?
            .to_string();
        let session_key = format!(
            "agent:main:session:{}",
            nemesis_agent::session::SessionStore::sanitize_session_id(&session_id)
        );
        // H1（2026-09-05）：todo_get 是只读文件读取，不需要 AgentLoop——
        // 提前返回，不占用 loop 句柄。L6++ G4（2026-09-08）：项目会话的
        // todo 落在项目目录（TodoWriteTool 按 loop 的 workspace root 解析
        // sessions/），先判项目根再读；无归属回落主 workspace。
        if cmd == "todo_get" {
            let workspace = crate::handlers::projects::project_root_for_session(&session_key)
                .or_else(|| ctx.workspace.clone())
                .ok_or_else(|| "workspace not configured".to_string())?;
            let todos = read_session_todos(&workspace, &session_id);
            return Ok(Some(serde_json::json!({
                "session_id": session_id,
                "session_key": session_key,
                "todos": todos,
            })));
        }
        // L6++ G4（2026-09-08）：归属解析单一裁决点——项目会话拿项目 loop
        // （不可用诚实报错），其余拿主槽（不扩 AppState）。锁内 clone Arc，
        // 不持 guard 跨 await（compact 是分钟级 LLM 调用）。
        let agent_loop: Arc<AgentLoop> =
            crate::handlers::projects::resolve_session_loop(ctx, &session_key)?;
        // M5（2026-09-05）：会话级 context 占用快照——只读，同走 AgentLoop
        // 的测量口径（与压缩压力同公式，见 loop.rs session_context_status）。
        if cmd == "context_status" {
            let status = agent_loop.session_context_status(&session_key);
            return Ok(Some(serde_json::json!({
                "session_id": session_id,
                "session_key": session_key,
                "context": status,
            })));
        }
        // F1（devtool-upgrade 阶段 4）：plan/build 模式切换。与 /plan /build
        // slash 走同一 `set_mode_with_event`（翻转 + ModeChanged 事件发布，
        // 前端徽标实时刷新）；chat_id 用 `web:{session_id}`——与 agent 事件
        // pump 的路由前缀约定一致（server.rs pump_agent_events）。
        if cmd == "set_mode" {
            let mode_str = data
                .as_ref()
                .and_then(|d| d.get("mode"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| "missing mode (expected \"plan\" | \"build\")".to_string())?;
            let mode = nemesis_agent::types::AgentMode::parse(mode_str).ok_or_else(|| {
                format!("unknown mode: {mode_str:?} (expected \"plan\" | \"build\")")
            })?;
            agent_loop.set_mode_with_event(mode, &session_key, &format!("web:{session_id}"));
            return Ok(Some(serde_json::json!({
                "session_id": session_id,
                "session_key": session_key,
                "mode": mode.as_str(),
            })));
        }
        // F1：当前模式只读查询（前端打开会话时初始化徽标用）。
        if cmd == "get_mode" {
            let mode = agent_loop.mode();
            return Ok(Some(serde_json::json!({
                "session_id": session_id,
                "session_key": session_key,
                "mode": mode.as_str(),
            })));
        }
        let receipt = match cmd {
            "compact" => agent_loop.compact_session(&session_key).await?,
            "clear" => agent_loop.clear_session(&session_key).await?,
            _ => return Err(format!("unknown chat cmd: {cmd}")),
        };
        Ok(Some(serde_json::json!({
            "session_id": session_id,
            "session_key": session_key,
            "receipt": receipt,
        })))
    }
}

/// H1（2026-09-05）：读会话 todo 清单（`chat.todo_get` 支撑）。
///
/// 路径与 `TodoWriteTool` 的写路径严格同构：session_key 构造（E6 顶部）
/// → `':' → '_'` 净化 → `sessions/todo_{safe}.json`。文件缺失 = 空清单
/// （会话没有 todo 是常态，不是错误）；损坏 JSON = 空清单 + warn
/// （渲染型接口不把坏文件炸给前端，但日志留痕）。
pub(crate) fn read_session_todos(
    workspace: &str,
    session_id: &str,
) -> Vec<nemesis_types::agent::TodoItem> {
    let s = nemesis_agent::session::SessionStore::sanitize_session_id(session_id);
    let safe_key = format!("agent:main:session:{s}").replace(':', "_");
    let dir = nemesis_path::resolve_sessions_dir_in_workspace(std::path::Path::new(workspace));
    let path = dir.join(format!("todo_{safe_key}.json"));
    match std::fs::read_to_string(&path) {
        Ok(body) => match serde_json::from_str(&body) {
            Ok(todos) => todos,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "corrupt todo file; returning empty");
                Vec::new()
            }
        },
        Err(_) => Vec::new(),
    }
}
