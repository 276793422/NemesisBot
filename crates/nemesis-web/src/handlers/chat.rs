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
            "set_effort",
            "get_effort",
            "sync",
            "queue_status",
            "spawn",
            "work_current",
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
        // session_id 必填；after_seq 缺省 0（重放窗口内全量）。
        // 环形缓冲按 **agent 会话键** 记录（chat_id 是连接级 id，重连即变；
        // OutboundMeta.session_key 随行盖章），这里同构变换出键再查。
        // 旧 None 回落 ctx.session_id（WS 连接 id，非聊天
        // 会话 id）——对不存在的键 replay 恒空且 gap:false，漏传 session_id
        // 的调用方会把「没补到任何事件」误当「已同步」，L2 断线窗口静默丢帧。
        // 必填诚实报错（首方前端 useWebSocket 两个调用点均显式传 session_id，
        // 零破坏）。
        if cmd == "sync" {
            let sync_session = data
                .as_ref()
                .and_then(|d| d.get("session_id"))
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or("missing session_id (chat.sync replays a specific session)")?
                .to_string();
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
            // L10-量测（改进项三期 2026-11-02）：tool defs 供给面字节随行
            // ——AgentLoop 单一真相源量测（openai 形态代理口径），与压缩
            // 压力快照同请求返回；L10-建设惰性 schema 的前后对比基线源。
            let tool_defs = agent_loop.tool_defs_metrics();
            return Ok(Some(serde_json::json!({
                "session_id": session_id,
                "session_key": session_key,
                "context": status,
                "tool_defs": tool_defs,
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
        // G3（2026-11-09）：会话级思考强度 override。data.effort =
        // "off"|"low"|"medium"|"high"（设置）| null / 缺省（清除回落
        // config 模型级值）。仅内存态（重启清空）；按会话分键——同 loop
        // 其他会话不受牵连。生效点 = 下一轮 LLM 请求组装（chat_opts）。
        if cmd == "set_effort" {
            const LEGAL: [&str; 4] = ["off", "low", "medium", "high"];
            let effort = data.as_ref().and_then(|d| d.get("effort"));
            match effort {
                None | Some(serde_json::Value::Null) => {
                    // 清除：回落 config 模型级解析。
                    agent_loop.set_session_effort(&session_key, None);
                }
                Some(serde_json::Value::String(s)) => {
                    let v = s.trim().to_ascii_lowercase();
                    if !LEGAL.contains(&v.as_str()) {
                        return Err(format!(
                            "unknown effort: {s:?} (expected \"off\" | \"low\" | \"medium\" | \"high\" | null)"
                        ));
                    }
                    agent_loop.set_session_effort(&session_key, Some(v));
                }
                Some(other) => {
                    return Err(format!("effort must be a string or null, got: {other}"));
                }
            }
            return Ok(Some(serde_json::json!({
                "session_id": session_id,
                "session_key": session_key,
                "effort": agent_loop.session_effort(&session_key),
            })));
        }
        // G3：会话强度 override 只读查询。effort=null = 未设置（生效值为
        // config 模型级解析，前端下拉显示「默认」）。
        if cmd == "get_effort" {
            return Ok(Some(serde_json::json!({
                "session_id": session_id,
                "session_key": session_key,
                "effort": agent_loop.session_effort(&session_key),
            })));
        }
        // Work 时间线活查询（2026-10-30 ChatPanel 长任务时间线改造）：进行中
        // turn 的「叙述+工具」交错时间线整体快照（直查 loop 内活累加器，无
        // 环 200 条滑窗限制）——切走再切回 / 刷新页面时经 detectPendingTurn
        // 拉取，全量恢复执行中任务的过程视图。无进行中 turn / 尚无中间轮 /
        // 会话不匹配 = work:null（前端回落环回放）。
        if cmd == "work_current" {
            let work = agent_loop.current_work_timeline(&session_key);
            return Ok(Some(serde_json::json!({
                "session_id": session_id,
                "session_key": session_key,
                "work": work,
            })));
        }
        // P4（能力扩展 WS8）：steer/followUp 队列可观测——`chat.queue_status`
        // 返回两条队列各几条（steer = next_step 插队，followUp = next_turn
        // 排队），前端徽标数据源。只读快照，与 `agent.inbox_status` 同源
        // 同口径（`InboxStatus` 单一真相源），字段名按 P19 契约语义命名；
        // loop 归属解析失败由 resolve 的 Err 传播（与 compact/clear 同姿态）。
        if cmd == "queue_status" {
            let s = agent_loop.inbox_status(&session_key);
            return Ok(Some(serde_json::json!({
                "available": true,
                "session_id": session_id,
                "session_key": session_key,
                "steer": s.next_step,
                "followUp": s.next_turn,
                "capacity": s.capacity,
                "busy": s.busy,
                "mode": s.mode,
            })));
        }
        // 客户端驱动委派（2026-09-28 角色目录与分档供给）：把任务派给指定
        // 角色的子代理，同步等最终回复。角色合法性走 AgentLoop 的
        // `check_client_role`（与 dispatch 闸同一 visible_roles 裁决）；
        // 派发走 `client_spawn`（与模型 spawn 同一 SpawnFn detached 通道，
        // 工具档位/角色模板/安全管线全同源）。
        if cmd == "spawn" {
            let d = data.as_ref().ok_or_else(|| "missing data".to_string())?;
            let task = d
                .get("task")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| "missing task".to_string())?;
            let role = d
                .get("role")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .unwrap_or("");
            let tools_profile = d
                .get("tools_profile")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .unwrap_or("");
            // 值域校验与 SpawnTool schema 同款；空 = readonly 缺省（BUG 修复
            // 2026-11-04：此前空串透传 client_spawn → detached_tools_for_profile
            // 直接 Err「Unknown tools profile ''」——注释声称缺省但没落地，
            // 现归一化后传 loop，响应回带归一化值）。
            let tools_profile = match tools_profile {
                "" => "readonly",
                p @ ("readonly" | "full") => p,
                other => {
                    return Err(format!(
                        "invalid tools_profile: {other:?} (expected \"readonly\" | \"full\")"
                    ));
                }
            };
            agent_loop.check_client_role(role)?;
            // 子代理可见性（BUG 修复 2026-11-04 + 路由修正同日）：父会话标识
            // 随行——chat_id 用发起连接级房间 id（`web:{连接id}`，即
            // SessionManager send_queue 的键；deliverables/tool_event 投递
            // 同款。会话级 `web:{sid}` 进不了房间表——pump 按 chat_id 剥前缀
            // 查 send_queues，会话 sid 查不到 = 帧静默丢弃）。会话键另传
            // `session_key`（注册表台账归属 + SubagentDrawer 过滤 + 前端
            // payload.session_id 注入）。委派请求/回复由 client_spawn 持久化
            // 进本会话（user+assistant 双行，会话首建发 SessionCreated）。
            let parent_chat_id = ctx.chat_id.clone();
            let result = agent_loop
                .client_spawn(task, role, tools_profile, &parent_chat_id, &session_key)
                .await?;
            return Ok(Some(serde_json::json!({
                "session_id": session_id,
                "session_key": session_key,
                "role": role,
                "tools_profile": tools_profile,
                "result": result,
            })));
        }
        // 会话闸（BUG-HUNT R6，2026-10-23）：compact/clear 都是有副作用的
        // 会话级操作——旧直调不持会话闸，在飞回合的行会在截断后回写（孤儿
        // 行/内容复活）。under_history_gate 真互斥：持有期间新消息排队/
        // 诚实拒绝，操作完成释放 + 排空。slash /compact /clear 路径经
        // Maintenance outcome 先 acquire 再进尾巴，语义本就同款——这里对齐
        // （文件头「busy 闸」注记由此真正对 WSAPI 入口成立）。
        let receipt = match cmd {
            "compact" => {
                agent_loop
                    .under_history_gate(&session_key, "压缩", || {
                        agent_loop.compact_session(&session_key)
                    })
                    .await?
            }
            "clear" => {
                // L07（2026-11-01）孤儿卡配对清理：会话清空 = 该会话的请求
                // 已终局——同 loop responder 槽的未决审批卡定向关闭（trait
                // 默认空实现 = 无审批面/非交互形态零开销；槽空静默跳过）。
                if let Some(responder) = agent_loop.approval_responder() {
                    let n = responder.resolve_orphans_for_session(&session_key, "session cleared");
                    if n > 0 {
                        tracing::info!(
                            "[Web/chat] session {session_key} cleared: closed {n} pending approval card(s)"
                        );
                    }
                }
                agent_loop
                    .under_history_gate(&session_key, "清空", || {
                        agent_loop.clear_session(&session_key)
                    })
                    .await?
            }
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
/// 路径走 nemesis-path 单一真相源 `session_todo_file_in_workspace`
/// （2026-11-05：与 TodoWriteTool 写路径、todo 收尾 hook、回收站寻址
/// 同源）。文件缺失 = 空清单（会话没有 todo 是常态，不是错误）；损坏
/// JSON = 空清单 + warn（渲染型接口不把坏文件炸给前端，但日志留痕）。
pub(crate) fn read_session_todos(
    workspace: &str,
    session_id: &str,
) -> Vec<nemesis_types::agent::TodoItem> {
    let s = nemesis_agent::session::SessionStore::sanitize_session_id(session_id);
    let path = nemesis_path::session_todo_file_in_workspace(
        std::path::Path::new(workspace),
        &format!("agent:main:session:{s}"),
    );
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

// AGT 覆盖率批次（2026-09-25）：todo_get 的两个确定性 bail 臂（缺
// session_id / workspace 未配置）。
#[cfg(test)]
mod agt_tests;

// P4（能力扩展 WS8）：chat.queue_status 契约测试（steer/followUp 计数面）。
#[cfg(test)]
mod queue_status_tests;
