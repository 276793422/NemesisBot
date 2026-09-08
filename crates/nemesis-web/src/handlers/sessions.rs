//! Sessions handler — Dashboard multi-session management (list/create/delete).
//!
//! Each conversation is identified by a client-chosen `session_id`; the
//! backend turns it into session_key `agent:main:session:{sid}`
//! (see `server.rs` process_messages + `loop.rs` handle_history_request).
//! List source = `session_logs/*.jsonl` (reuses `logs::scan_session_logs`),
//! filtered to web conversations. Delete clears SessionStore + session_logs.

use crate::handlers::logs::scan_session_logs;
use crate::handlers::require_workspace;
use crate::ws_router::{ModuleHandler, RequestContext};

pub struct SessionsHandler;

#[async_trait::async_trait]
impl ModuleHandler for SessionsHandler {
    fn module_name(&self) -> &str {
        "sessions"
    }

    fn commands(&self) -> &'static [&'static str] {
        &[
            "list",
            "create",
            "rename",
            "delete",
            "clear",
            "export",
            "rewind_to_message",
            "redo",
            "file_diff",
            "share_create",
            "share_list",
            "share_revoke",
        ]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        match cmd {
            "list" => {
                let workspace = require_workspace(ctx)?;
                let all = scan_session_logs(workspace);
                // Web multi-session conversations only: files are named
                // `agent_main_session_{sid}.jsonl` (session_key
                // `agent:main:session:{sid}` with `:`→`_`). Strip the prefix
                // so the client gets the bare `sid` — exactly what it sends
                // back as moduleData.session_id. (Legacy `agent_main_main`
                // migration is Phase 2.)
                let mut web: Vec<_> = all
                    .into_iter()
                    .filter_map(|mut s| {
                        let id = s["id"].as_str()?.to_string();
                        let sid = id.strip_prefix("agent_main_session_")?.to_string();
                        s["id"] = serde_json::Value::String(sid);
                        Some(s)
                    })
                    .collect();
                // M5（2026-09-05）：侧栏条目回填 tokens/cost（session_key
                // 聚合；无记录时缺省，前端不展示）。
                crate::handlers::logs::backfill_session_usage(ctx, &mut web);
                Ok(Some(serde_json::json!({ "sessions": web })))
            }
            "create" => {
                // Backend generates the id; the conversation lazily
                // materializes in session_logs on the first message. Title is
                // written to a sidecar meta file immediately.
                let session_id = uuid::Uuid::new_v4().to_string();
                let session_key = format!(
                    "agent:main:session:{}",
                    nemesis_agent::session::SessionStore::sanitize_session_id(&session_id)
                );
                // L6++ G4（2026-09-08）：可选 project_id——先校验+烧归属
                // （bridge bind_session：项目存在 + 目录在 → sidecar project
                // 字段 + 内存索引），失败则整个 create 失败、不落任何 meta
                // （放在 title 写入之前：归属失败不产生半途副作用）。
                if let Some(project_id) = data
                    .as_ref()
                    .and_then(|d| d.get("project_id"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                {
                    let bridge = crate::handlers::projects::projects_bridge()
                        .ok_or_else(|| {
                            "项目管理未装配（projects bridge 未接线，无法创建项目会话）".to_string()
                        })?;
                    bridge.bind_session(&session_key, &project_id)?;
                }
                // E7：显式命名 = 用户意志（manual，自动标题永不覆盖）；
                // 未带 title = 默认占位符（自动标题可覆盖）。
                let explicit_title = data
                    .as_ref()
                    .and_then(|d| d.get("title"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| {
                        !s.is_empty() && *s != nemesis_agent::chat_log::DEFAULT_SESSION_TITLE
                    });
                let title = explicit_title
                    .clone()
                    .unwrap_or_else(|| nemesis_agent::chat_log::DEFAULT_SESSION_TITLE.to_string());
                if explicit_title.is_some() {
                    nemesis_agent::chat_log::write_session_meta_manual(&session_key, &title);
                } else {
                    nemesis_agent::chat_log::write_session_meta(&session_key, &title);
                }
                Ok(Some(
                    serde_json::json!({ "session_id": session_id, "title": title }),
                ))
            }
            "rename" => {
                let session_id = data
                    .as_ref()
                    .and_then(|d| d.get("session_id"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "missing session_id".to_string())?
                    .to_string();
                let title = data
                    .as_ref()
                    .and_then(|d| d.get("title"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "missing title".to_string())?
                    .to_string();
                let session_key = format!(
                    "agent:main:session:{}",
                    nemesis_agent::session::SessionStore::sanitize_session_id(&session_id)
                );
                // E7：rename 是用户意志——置 manual 标记，自动标题永不覆盖。
                nemesis_agent::chat_log::write_session_meta_manual(&session_key, &title);
                Ok(Some(
                    serde_json::json!({ "session_id": session_id, "title": title }),
                ))
            }
            "delete" => {
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
                // Clear SessionStore (in-memory + sessions/*.json) +
                // session_logs/*.jsonl. Best-effort; absence is not an error.
                {
                    let guard = ctx.state.agent_loop.read();
                    if let Some(al) = guard.as_ref()
                        && let Some(store) = al.session_store()
                    {
                        store.delete_session(&session_key);
                    }
                }
                // 方言 SessionEnd（观察型，2026-08-29 T3）：显式删除也触发
                // （桥经 AgentLoop 的 cc_hooks_bridge 访问；未装配 = 跳过）。
                // 先把桥 Arc 克隆出 guard 作用域，再 await（guard 不跨 await）。
                let session_end_bridge = {
                    let guard = ctx.state.agent_loop.read();
                    guard.as_ref().and_then(|al| al.cc_hooks_bridge())
                };
                if let Some(bridge) = session_end_bridge {
                    bridge.on_session_end(&session_key, "deleted").await;
                }
                // Cron cascade (2026-08-25): a scheduled job pinned to this
                // session_key would otherwise FIRE on a deleted conversation
                // and resurrect it — an empty jsonl re-created + cron rows
                // appended, a session the user explicitly deleted coming
                // back as a zombie. Disable (NOT remove: the job definition
                // stays on the Tasks page for re-pointing/re-enabling) every
                // ENABLED job whose payload targets this session, and report
                // what was paused. Disabled jobs are left untouched.
                // Guard released above so we never hold agent_loop + cron
                // mutexes together.
                let mut paused: Vec<serde_json::Value> = Vec::new();
                if let Some(svc) = ctx.state.cron.as_ref()
                    && let Ok(svc) = svc.lock()
                {
                    for job in svc.list_jobs(true) {
                        if !job.enabled {
                            continue;
                        }
                        if job.payload.session_key.as_deref() != Some(session_key.as_str()) {
                            continue;
                        }
                        match svc.enable_job(&job.id, false) {
                            Ok(j) => paused.push(serde_json::json!({
                                "id": j.id,
                                "name": j.name,
                            })),
                            Err(e) => tracing::warn!(
                                "[sessions] delete cascade: failed to pause cron job {} ({}): {}",
                                job.id,
                                job.name,
                                e
                            ),
                        }
                    }
                }
                // L6++ G4（2026-09-08）：项目归属联动摘除（内存索引 + sidecar
                // project 字段——meta 已随 delete_chat_log 删除时为 no-op）。
                // bridge 未装配 = 无项目分组语义，跳过。
                if let Some(bridge) = crate::handlers::projects::projects_bridge() {
                    bridge.forget_session(&session_key);
                }
                Ok(Some(serde_json::json!({
                    "deleted": session_id,
                    "paused_cron_jobs": paused,
                })))
            }
            "clear" => {
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
                // Clear SessionStore messages + session_logs jsonl (keep meta/key).
                // Order matters (2026-08-25 self-heal note): truncate the
                // chat_log FIRST, then drop the store json. If we crashed
                // between the two steps the other way round, the surviving
                // jsonl would let `rebuild_from_chat_log` resurrect the
                // content the user just asked to clear. jsonl-first keeps the
                // rebuild path unable to revive cleared data.
                nemesis_agent::chat_log::clear_chat_log(&session_key);
                let guard = ctx.state.agent_loop.read();
                if let Some(al) = guard.as_ref()
                    && let Some(store) = al.session_store()
                {
                    store.clear_session(&session_key);
                }
                drop(guard);
                // L6++ G4（2026-09-08）：clear 重置会话 → 项目归属一并解除
                // （摘索引 + sidecar project 字段，title 保留——防 owner_of
                // 的 sidecar 兜底把绑定「复活」）。bridge 未装配 = 跳过。
                if let Some(bridge) = crate::handlers::projects::projects_bridge() {
                    bridge.forget_session(&session_key);
                }
                Ok(Some(serde_json::json!({ "cleared": session_id })))
            }
            "export" => {
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
                let (messages, total, _, _) =
                    nemesis_agent::chat_log::read_chat_log(&session_key, 100_000, None);
                Ok(Some(serde_json::json!({
                    "session_id": session_id,
                    "messages": messages,
                    "count": total,
                })))
            }
            // E3（devtool-upgrade 阶段 5）：消息级回退——会话截断到
            // message_index 所在 turn 结束 + 文件恢复到其后第一个 checkpoint
            // turn 开始时的状态 + 压 undo 栈。message_index 契约：本模块
            // `export`（= logs.session_detail 的行序，同一 jsonl）里
            // messages 数组下标。编排在 AgentLoop（checkpoint/undo 栈都住
            // 那边），这里只做参数解析与 agent_loop 装配检查。
            "rewind_to_message" => {
                let session_id = data
                    .as_ref()
                    .and_then(|d| d.get("session_id"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "missing session_id".to_string())?
                    .to_string();
                let message_index = data
                    .as_ref()
                    .and_then(|d| d.get("message_index"))
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| "missing message_index".to_string())?
                    as usize;
                let session_key = format!(
                    "agent:main:session:{}",
                    nemesis_agent::session::SessionStore::sanitize_session_id(&session_id)
                );
                // L6++ G6（2026-09-08）：undo 栈与 checkpoint store 都住 loop
                // 实例——项目会话的回退必须由项目 loop 执行（此前硬编码主
                // 槽，项目会话回退拿到主 loop 的空 checkpoint 索引，文件恢
                // 复静默 no-op 只截对话）。无归属/bridge 未装配回主槽不变。
                let al = crate::handlers::projects::resolve_session_loop(ctx, &session_key)?;
                let mut out = al.rewind_to_message(&session_key, message_index).await?;
                // 回执带上调用方的裸 session_id（前端用它回显）。
                out["session_id"] = serde_json::Value::String(session_id);
                Ok(Some(out))
            }
            // E3 redo：弹本会话 undo 栈顶反向恢复（行回填 + 文件前向恢复；
            // 陈旧性守卫在 AgentLoop 侧）。undo 栈是内存态，网关重启即失。
            "redo" => {
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
                // L6++ G6：redo 的 undo 栈住 loop 实例，同 rewind 路由项目会话。
                let al = crate::handlers::projects::resolve_session_loop(ctx, &session_key)?;
                let mut out = al.redo_rewind(&session_key).await?;
                out["session_id"] = serde_json::Value::String(session_id);
                Ok(Some(out))
            }
            // M3（devtool-upgrade 阶段 5）：会话级文件 diff——聚合侧在
            // 前端（session_detail 行的 file_changes），这里按 (session_id,
            // path) 取「最早 checkpoint 基线 vs 现盘」unified diff。编排
            // 在 AgentLoop 侧（checkpoint store 住那边）。
            "file_diff" => {
                let session_id = data
                    .as_ref()
                    .and_then(|d| d.get("session_id"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "missing session_id".to_string())?
                    .to_string();
                let path = data
                    .as_ref()
                    .and_then(|d| d.get("path"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "missing path".to_string())?
                    .to_string();
                let session_key = format!(
                    "agent:main:session:{}",
                    nemesis_agent::session::SessionStore::sanitize_session_id(&session_id)
                );
                // L6++ G6：checkpoint 索引住 loop 实例，同 rewind 路由项目会话。
                let al = crate::handlers::projects::resolve_session_loop(ctx, &session_key)?;
                let mut out = al.session_file_diff(&session_key, &path).await?;
                out["session_id"] = serde_json::Value::String(session_id);
                Ok(Some(out))
            }
            // L4（devtool-upgrade 阶段 7）：会话分享——创建/列出/撤销只读
            // 分享 token（存储 + 白名单投影 + 公开 GET 端点都在 crate::share）。
            "share_create" => {
                let workspace = require_workspace(ctx)?;
                let session_id = data
                    .as_ref()
                    .and_then(|d| d.get("session_id"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "missing session_id".to_string())?
                    .to_string();
                let entry = crate::share::create_share(workspace, &session_id)?;
                Ok(Some(serde_json::json!({
                    "token": entry.token,
                    "path": format!("/share/?t={}", entry.token),
                    "created_at": entry.created_at,
                })))
            }
            "share_list" => {
                let workspace = require_workspace(ctx)?;
                // 标题实时解析（会话改名/删除跟随 live 语义）。
                let shares: Vec<serde_json::Value> = crate::share::list_shares(workspace)
                    .into_iter()
                    .map(|s| {
                        let title = crate::share::session_title(workspace, &s.session_id);
                        serde_json::json!({
                            "token": s.token,
                            "session_id": s.session_id,
                            "created_at": s.created_at,
                            "revoked": s.revoked,
                            "title": title,
                        })
                    })
                    .collect();
                Ok(Some(serde_json::json!({ "shares": shares })))
            }
            "share_revoke" => {
                let workspace = require_workspace(ctx)?;
                let token = data
                    .as_ref()
                    .and_then(|d| d.get("token"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "missing token".to_string())?;
                let found = crate::share::revoke_share(workspace, token)?;
                if !found {
                    return Err(format!("分享不存在: {token}"));
                }
                Ok(Some(serde_json::json!({ "ok": true })))
            }
            _ => Err(format!("unknown sessions cmd: {}", cmd)),
        }
    }
}
