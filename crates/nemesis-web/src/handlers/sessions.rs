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

/// L22 sessions.import 单次行数上限（与 export 读侧 100_000 上限同量级
/// 取整——正常会话远达不到，挡的是误传巨型载荷/恶意输入）。
const IMPORT_MAX_ROWS: usize = 50_000;

#[async_trait::async_trait]
impl ModuleHandler for SessionsHandler {
    fn module_name(&self) -> &str {
        "sessions"
    }

    fn commands(&self) -> &'static [&'static str] {
        &[
            "list",
            "create",
            "set_binding",
            "remove_binding",
            "rename",
            "delete",
            "clear",
            "export",
            "import",
            "changed_files",
            "rewind_to_message",
            "redo",
            "file_diff",
            "ledger_verify",
            "share_create",
            "share_list",
            "share_revoke",
            "mark_delivered",
            "snooze",
            "unsnooze",
            "detach",
            "trash_list",
            "trash_restore",
            "trash_purge",
            "trash_purge_all",
            "stats",
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
                let workspace = require_workspace(ctx)?.to_string();
                // BUG-20261102-302：scan_session_logs 对目录内每个 jsonl 全量
                // 解析（read_jsonl_lines 逐行 JSON parse）——侧栏热路径的同步
                // 阻塞 IO 直跑 tokio worker 会占住 worker（read_chat_log_async
                // 同款纪律），spawn_blocking 移出。
                let all = {
                    let ws = workspace.clone();
                    tokio::task::spawn_blocking(move || scan_session_logs(&ws))
                        .await
                        .map_err(|e| format!("sessions.list task join failed: {e}"))?
                };
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
                // 会话级全部放行投影（2026-10-24 长任务无人值守计划）：列表
                // 一眼可见哪个会话在放行（读运行态内存——gateway 启动 hydrate
                // 先于 WS 服务可用，无「sidecar 真但内存假」窗口）。
                // security feature 关闭时状态模块不编译，投影恒 false（诚实
                // 缺省）——引用必须同闸，否则裁剪 feature 集编译破损。
                #[cfg(feature = "security")]
                {
                    let allow_keys = crate::handlers::security::session_allow_enabled_keys();
                    for row in web.iter_mut() {
                        let Some(sid) = row["id"].as_str() else {
                            continue;
                        };
                        let key = format!(
                            "agent:main:session:{}",
                            nemesis_agent::session::SessionStore::sanitize_session_id(sid)
                        );
                        row["allow_all"] = serde_json::Value::Bool(allow_keys.contains(&key));
                    }
                }
                #[cfg(not(feature = "security"))]
                {
                    for row in web.iter_mut() {
                        row["allow_all"] = serde_json::Value::Bool(false);
                    }
                }
                // L22（2026-11-03 snooze 排队）：暂停期排队条数投影（>0 才
                // 加字段，前端据此渲染 💤+n 徽标）。一次 stat + 小文件读，
                // 无 pending 的会话（常态）接近零开销。
                for row in web.iter_mut() {
                    let Some(sid) = row["id"].as_str() else {
                        continue;
                    };
                    let key = format!(
                        "agent:main:session:{}",
                        nemesis_agent::session::SessionStore::sanitize_session_id(sid)
                    );
                    let pending = nemesis_agent::chat_log::session_pending_count(&key);
                    if pending > 0 {
                        row["pendingCount"] = serde_json::Value::from(pending);
                    }
                }
                // B（2026-09-23 多会话并行清账）：回带绑定表（仅存活绑定，
                // key → 裸 sid）——前端删掉 localStorage 映射，绑定真相源
                // 上移服务端。
                let bindings = crate::session_bindings::live_bindings(&workspace);
                Ok(Some(
                    serde_json::json!({ "sessions": web, "bindings": bindings }),
                ))
            }
            "mark_delivered" => {
                // P2（2026-09-11 真机日志）：前端拉取会话历史后清零未送达
                // 标记（adapters 推送失败时打点）。零计数/缺席 sidecar 幂等。
                let session_id = data
                    .as_ref()
                    .and_then(|d| d.get("session_id"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "missing session_id".to_string())?;
                let session_key = format!(
                    "agent:main:session:{}",
                    nemesis_agent::session::SessionStore::sanitize_session_id(session_id)
                );
                let cleared = nemesis_agent::chat_log::clear_undelivered_replies(&session_key);
                Ok(Some(serde_json::json!({ "cleared": cleared })))
            }
            "snooze" => {
                // B11（2026-10-18 snooze）：暂停会话到指定时刻（相对秒或
                // RFC3339）。幂等 upsert（重复 snooze = 改期）；到期由
                // gateway snooze sweeper 摘标记 + 注入恢复 system 行。期间
                // 消息照常处理——snooze 只是 UI 标记 + 到期唤醒语义。
                let d = data.as_ref().ok_or_else(|| "missing data".to_string())?;
                let session_id = d
                    .get("session_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "missing session_id".to_string())?;
                let raw_until = d
                    .get("until")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "missing until".to_string())?;
                let note = d
                    .get("note")
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                let sid = nemesis_agent::session::SessionStore::sanitize_session_id(session_id);
                let session_key = format!("agent:main:session:{sid}");
                if !nemesis_agent::chat_log::chat_log_exists(&session_key) {
                    return Err(format!("会话不存在: {sid}"));
                }
                let until = parse_snooze_until(raw_until)?;
                nemesis_agent::chat_log::write_session_snooze(
                    &session_key,
                    &until,
                    note.as_deref(),
                );
                ctx.state.event_hub.publish(
                    "session.updated",
                    serde_json::json!({ "session_id": sid, "snoozed_until": until }),
                );
                Ok(Some(
                    serde_json::json!({ "session_id": sid, "snoozed_until": until }),
                ))
            }
            "detach" => {
                // 脱离父分支（2026-10-20）：清血缘 meta 四字段（parent /
                // forked_at_turn / fork_reason / branch_summary）——UI 谱系
                // 视图回平、Branch Context 停止注入。消息内容零改动（fork
                // 时整段 VERBATIM 拷贝，零共享状态）；子分支跟随独立（子树
                // 挂在本会话下，本会话成根后树自动重组）。不可逆；boundary
                // 审计行保留（jsonl 内部，UI 不可见）。无血缘 = 诚实拒绝。
                let session_id = data
                    .as_ref()
                    .and_then(|d| d.get("session_id"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "missing session_id".to_string())?;
                let sid = nemesis_agent::session::SessionStore::sanitize_session_id(session_id);
                let session_key = format!("agent:main:session:{sid}");
                if !nemesis_agent::chat_log::chat_log_exists(&session_key) {
                    return Err(format!("会话不存在: {sid}"));
                }
                if !nemesis_agent::chat_log::clear_session_lineage(&session_key) {
                    return Err(format!("会话 {sid} 没有父分支血缘（非分支会话，无需脱离）"));
                }
                ctx.state.event_hub.publish(
                    "session.updated",
                    serde_json::json!({ "session_id": sid, "detached": true }),
                );
                Ok(Some(
                    serde_json::json!({ "session_id": sid, "detached": true }),
                ))
            }
            "unsnooze" => {
                // B11：立即解除暂停。无标记 = 幂等成功（unsnoozed=false 仍回
                // 200，前端刷新列表即可）。L22：解除后立即重放暂停期排队的
                // 消息（同 wake 重放语义；幂等臂无标记也照调——幂等调用的
                // 前提通常是前一次唤醒失败滞留，重放是无害的单次消费）。
                let session_id = data
                    .as_ref()
                    .and_then(|d| d.get("session_id"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "missing session_id".to_string())?;
                let sid = nemesis_agent::session::SessionStore::sanitize_session_id(session_id);
                let session_key = format!("agent:main:session:{sid}");
                let cleared = nemesis_agent::chat_log::clear_session_snooze(&session_key).is_some();
                let delivered = replay_pending_for(&session_key);
                ctx.state.event_hub.publish(
                    "session.updated",
                    serde_json::json!({
                        "session_id": sid,
                        "snoozed_until": serde_json::Value::Null,
                    }),
                );
                Ok(Some(
                    serde_json::json!({ "session_id": sid, "unsnoozed": cleared, "delivered": delivered }),
                ))
            }
            "create" => {
                let workspace = require_workspace(ctx)?;
                // B（2026-09-23 多会话并行清账）：可选 binding_key —— 原子
                // get-or-create。key 已绑且会话存活 → 返回既有会话（零新
                // 建，前端 localStorage 误判重建的复制机器在服务端终结）；
                // 未绑/陈旧 → 新建并写绑定。与 project_id 互斥设计：绑定
                // 场景是自由会话（agentGen），带 project_id 的调用方不传
                // binding_key。
                if let Some(binding_key) = data
                    .as_ref()
                    .and_then(|d| d.get("binding_key"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                {
                    // E7 同款 title 解析（显式命名 = manual）。
                    let explicit_title = data
                        .as_ref()
                        .and_then(|d| d.get("title"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.trim().to_string())
                        .filter(|s| {
                            !s.is_empty() && *s != nemesis_agent::chat_log::DEFAULT_SESSION_TITLE
                        });
                    let title = explicit_title.clone().unwrap_or_else(|| {
                        nemesis_agent::chat_log::DEFAULT_SESSION_TITLE.to_string()
                    });
                    // BUG-20261102-302：get_or_create_session 在 BINDING_LOCK
                    // 持锁段内做全目录扫描（session_title→scan_session_logs）
                    // ——持锁 + 阻塞 IO 双重占位，spawn_blocking 整体移出
                    // （锁在阻塞线程内自旋，不占 tokio worker）。
                    let has_explicit = explicit_title.is_some();
                    let outcome = {
                        let ws = workspace.to_string();
                        tokio::task::spawn_blocking(move || {
                            crate::session_bindings::get_or_create_session(
                                &ws,
                                &binding_key,
                                &title,
                                has_explicit,
                            )
                        })
                        .await
                        .map_err(|e| format!("sessions.create task join failed: {e}"))??
                    };
                    return Ok(Some(serde_json::json!({
                        "session_id": outcome.session_id,
                        "title": outcome.title,
                        "reused": !outcome.created,
                    })));
                }
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
                    let bridge = crate::handlers::projects::projects_bridge().ok_or_else(|| {
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
            // B（2026-09-23 多会话并行清账）：重绑——draft_apply 成功后把
            // 当前会话绑到新目标（键 upsert，旧持有者让出；目标不存在诚实
            // 拒绝，孤儿绑定不可能产生）。
            "set_binding" => {
                let workspace = require_workspace(ctx)?;
                let binding_key = data
                    .as_ref()
                    .and_then(|d| d.get("binding_key"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "missing binding_key".to_string())?;
                let session_id = data
                    .as_ref()
                    .and_then(|d| d.get("session_id"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "missing session_id".to_string())?;
                crate::session_bindings::set_binding(workspace, binding_key, session_id)?;
                Ok(Some(serde_json::json!({
                    "ok": true,
                    "binding_key": binding_key.trim(),
                    "session_id": session_id,
                })))
            }
            // B（2026-09-23 多会话并行清账）：摘键——draft_apply 后释放
            // 「__new__」引导键（会话已重绑到正式工作流名）。键不存在幂等。
            "remove_binding" => {
                let workspace = require_workspace(ctx)?;
                let binding_key = data
                    .as_ref()
                    .and_then(|d| d.get("binding_key"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "missing binding_key".to_string())?;
                let removed = crate::session_bindings::remove_binding(workspace, binding_key)?;
                Ok(Some(serde_json::json!({
                    "ok": true,
                    "binding_key": binding_key.trim(),
                    "removed": removed,
                })))
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
                // 回收站（2026-10-30 拍板③）：默认移入回收站（可恢复，90d
                // TTL 清扫）；`hard: true` = 旧硬删除语义原样保留。
                let hard = data
                    .as_ref()
                    .and_then(|d| d.get("hard"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                // 回收站条目来源（2026-10-30 浏览器 E2E T4 根修）：单删
                // 缺席/未知值落 "single"；侧栏批量循环显式传 "batch"。
                let source = match data
                    .as_ref()
                    .and_then(|d| d.get("source"))
                    .and_then(|v| v.as_str())
                {
                    Some("batch") => "batch",
                    _ => "single",
                };
                let session_key = format!(
                    "agent:main:session:{}",
                    nemesis_agent::session::SessionStore::sanitize_session_id(&session_id)
                );
                // 会话闸（BUG-HUNT R6，同 rewind 家族——admission
                // under_history_gate）：turn 在飞时的 delete 与在飞回合竞态，
                // jsonl 删掉后回合尾巴把内存会话回写，被删会话以孤儿行复活。
                // 经 resolve_session_loop 拿归属 loop（busy 旗标 per-loop，
                // 项目会话必须闸在项目 loop 上）；持有期间新消息排队/诚实
                // 拒绝，操作完成（含错误路径）释放 + 排空。参数解析在闸外
                // （零副作用）。无 loop（退化 ctx）诚实降级为不闸——没有
                // 回合引擎就没有在飞回合，竞态窗口不存在；S10b 记录的跳臂
                // 行为（store 臂跳过、jsonl 幸存）保持原样。
                match crate::handlers::projects::resolve_session_loop(ctx, &session_key) {
                    Ok(al) => {
                        al.under_history_gate(&session_key, "删除", || {
                            delete_session_body(ctx, &session_id, &session_key, hard, source)
                        })
                        .await
                    }
                    Err(_) => {
                        delete_session_body(ctx, &session_id, &session_key, hard, source).await
                    }
                }
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
                // 会话闸（BUG-HUNT R6，同 delete 臂）：clear 的 jsonl 截断
                // 与在飞回合竞态——截空后回合尾巴回写孤儿行。归属 loop 真互斥；
                // 无 loop 诚实降级（理由同 delete 臂）。
                match crate::handlers::projects::resolve_session_loop(ctx, &session_key) {
                    Ok(al) => {
                        al.under_history_gate(&session_key, "清空", || {
                            clear_session_body(ctx, &session_key)
                        })
                        .await?
                    }
                    Err(_) => clear_session_body(ctx, &session_key).await?,
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
                // BUG-20261102-302：export 全量读会话日志，同步阻塞 IO 直跑
                // tokio worker，spawn_blocking 移出（read_chat_log_async 同款
                // 纪律）。BUG-20261106-002：limit 用全量读者契约 `usize::MAX`
                // ——此前传 100_000 触发 whole_log 旗标但滑窗仍在 >limit 时
                // 从头驱逐，超 10 万行会话的最旧历史被静默截掉（count 照报
                // 真实总数，迁移副本永久缺前缀且无任何告警）。
                let (messages, total, _, _) = {
                    let key = session_key;
                    tokio::task::spawn_blocking(move || {
                        nemesis_agent::chat_log::read_chat_log(&key, usize::MAX, None)
                    })
                    .await
                    .map_err(|e| format!("sessions.export task join failed: {e}"))?
                };
                Ok(Some(serde_json::json!({
                    "session_id": session_id,
                    "messages": messages,
                    "count": total,
                })))
            }
            "import" => {
                // L22（2026-11-03 跨设备移动）：sessions.export 的配对面——
                // 导入导出消息行重建会话（搬运最小形态）。messages 行保持
                // 导出形态（jsonl Value 数组，逐行 VERBATIM 落盘——时间戳/
                // 模型徽标等全保真）；session_id 可选（缺省生成 uuid）；
                // 目标已存在 = 诚实拒绝（导入不覆盖既有会话）。落盘后零
                // store 触碰——SessionStore 首次 get_or_create 时按 jsonl
                // 自愈水合（fork 写入同机制），不双写两处真相源。
                let d = data.as_ref().ok_or_else(|| "missing data".to_string())?;
                let messages = d
                    .get("messages")
                    .and_then(|v| v.as_array())
                    .ok_or_else(|| "missing messages".to_string())?;
                if messages.is_empty() {
                    return Err("messages 为空".to_string());
                }
                if messages.len() > IMPORT_MAX_ROWS {
                    return Err(format!(
                        "messages 超上限（{IMPORT_MAX_ROWS} 行，收到 {} 行）",
                        messages.len()
                    ));
                }
                // 行校验：必须是 object 且带 role + content（导出契约的最小
                // 形状——其余字段不白名单不剥离，VERBATIM 保真）。
                let mut rows = Vec::with_capacity(messages.len());
                for (i, v) in messages.iter().enumerate() {
                    let obj = v
                        .as_object()
                        .ok_or_else(|| format!("messages[{i}] 不是对象"))?;
                    if !obj.contains_key("role") || !obj.contains_key("content") {
                        return Err(format!("messages[{i}] 缺 role/content 字段"));
                    }
                    rows.push(v.clone());
                }
                let sid = match data
                    .as_ref()
                    .and_then(|d| d.get("session_id"))
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    Some(raw) => {
                        let sid = nemesis_agent::session::SessionStore::sanitize_session_id(raw);
                        let key = format!("agent:main:session:{sid}");
                        // session_occupied（非裸 jsonl）：meta-only 残留键同样
                        // 拒绝——meta 写入是 upsert，导入复用会继承陈旧
                        // allow_all/branch_summary/title（BUG-20261106-038）。
                        // store 面（内存 + 盘）同查：store json 是
                        // get_or_create 磁盘回落的优先源（live store file
                        // always wins），store-only 残留键（detach 的 store
                        // json 删除为单发 remove_file 无重试）被导入会复活被
                        // 删会话的全部 LLM 上下文——横向排查同根同修。
                        let store_taken = {
                            let guard = ctx.state.agent_loop.read();
                            guard
                                .as_ref()
                                .and_then(|al| al.session_store())
                                .map(|s| s.contains(&key) || s.file_exists(&key))
                                .unwrap_or(false)
                        };
                        if store_taken || nemesis_agent::chat_log::session_occupied(&key) {
                            return Err(format!("会话已存在: {sid}（导入不覆盖既有会话）"));
                        }
                        sid
                    }
                    None => uuid::Uuid::new_v4().to_string(),
                };
                let session_key = format!("agent:main:session:{sid}");
                // BUG-20261106-003：落盘走排他创建（create_new）——上面的
                // exists 预检只是友好快速路径，真实「不覆盖」契约由文件系统
                // 原子性背书（check-then-create 竞态窗口不再能覆盖既有会话）。
                let written = {
                    let key = session_key.clone();
                    tokio::task::spawn_blocking(move || {
                        nemesis_agent::chat_log::create_chat_log_exclusive(&key, &rows)
                    })
                    .await
                    .map_err(|e| format!("sessions.import task join failed: {e}"))?
                    .map_err(|e| match e {
                        nemesis_agent::chat_log::ChatLogCreateError::AlreadyExists => {
                            format!("会话已存在: {sid}（导入不覆盖既有会话）")
                        }
                        nemesis_agent::chat_log::ChatLogCreateError::Io(m) => m,
                    })?
                };
                if written != messages.len() {
                    return Err(format!(
                        "落盘不完整: {written}/{} 行（会话 {sid} 已部分写入，请删除后重试）",
                        messages.len()
                    ));
                }
                // title（可选，manual 标记——自动标题永不覆盖，同 rename 语义）。
                if let Some(t) = d
                    .get("title")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    nemesis_agent::chat_log::write_session_meta_manual(&session_key, t);
                }
                ctx.state.event_hub.publish(
                    "session.updated",
                    serde_json::json!({ "session_id": sid, "imported": written }),
                );
                Ok(Some(
                    serde_json::json!({ "session_id": sid, "imported": written }),
                ))
            }
            "changed_files" => {
                // L22 session PR（最小形态）：分支会话的变更文件清单——
                // jsonl assistant 行 `file_changes` 聚合去重（按首次出现序）。
                // 前端拿清单逐文件调 `sessions.file_diff` 展示 unified diff。
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
                let files = {
                    let key = session_key;
                    tokio::task::spawn_blocking(move || {
                        nemesis_agent::chat_log::session_file_changes(&key)
                    })
                    .await
                    .map_err(|e| format!("sessions.changed_files task join failed: {e}"))?
                };
                Ok(Some(
                    serde_json::json!({ "session_id": session_id, "files": files }),
                ))
            }
            // E3（devtool-upgrade 阶段 5）：消息级回退——会话截断 + 文件恢复
            // 到锚 turn 开始时的状态 + 压 undo 栈。message_index 契约：本模块
            // `export`（= logs.session_detail 的行序，同一 jsonl）里
            // messages 数组下标。编排在 AgentLoop（checkpoint/undo 栈都住
            // 那边），这里只做参数解析与 agent_loop 装配检查。
            // P20（2026-09-25 能力扩展）：可选 `force`（bool，默认 false）——
            // 冲突预检检出「文件在 checkpoint 后被外部修改」时默认零副作用
            // 拒绝（blocked:true + 结构化冲突清单），force=true 强过（回执
            // 与日志留痕）。
            // mode（2026-10-20 重新生成修复）：缺省/"turn" = turn 对齐
            // （index 所在整轮保留，其后截掉）；"before" = 按轮首回退（保留
            // index 之前，该行及其后移除——前端「重新生成」：目标=提问行，
            // 文件滚回提问前，支持 cut=0 首轮）。未知值 loud 拒绝。
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
                let force = data
                    .as_ref()
                    .and_then(|d| d.get("force"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let mode = data
                    .as_ref()
                    .and_then(|d| d.get("mode"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("turn");
                let session_key = format!(
                    "agent:main:session:{}",
                    nemesis_agent::session::SessionStore::sanitize_session_id(&session_id)
                );
                // L6++ G6（2026-09-08）：undo 栈与 checkpoint store 都住 loop
                // 实例——项目会话的回退必须由项目 loop 执行（此前硬编码主
                // 槽，项目会话回退拿到主 loop 的空 checkpoint 索引，文件恢
                // 复静默 no-op 只截对话）。无归属/bridge 未装配回主槽不变。
                let al = crate::handlers::projects::resolve_session_loop(ctx, &session_key)?;
                let mut out = match mode {
                    "turn" => {
                        al.rewind_to_message(&session_key, message_index, force)
                            .await?
                    }
                    "before" => {
                        al.rewind_before_message(&session_key, message_index, force)
                            .await?
                    }
                    other => {
                        return Err(format!("未知 mode: {other}（支持 turn | before）"));
                    }
                };
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
                // M4（2026-10-27 整改）：可选 force——冲突预检（回退窗口内
                // 外部漂移）拒绝时前端确认后带 force 重试。
                let force = data
                    .as_ref()
                    .and_then(|d| d.get("force"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let al = crate::handlers::projects::resolve_session_loop(ctx, &session_key)?;
                let mut out = al.redo_rewind_with_force(&session_key, force).await?;
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
            // T6（扩展计划 D5）：会话事件账本全链重算验证。链断裂/篡改 =
            // Err（消息即失败原因），成功返回统计。
            "ledger_verify" => {
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
                let path = nemesis_agent::event_ledger::ledger_path(&session_key);
                if !path.exists() {
                    return Err(format!(
                        "会话 {session_id} 无事件账本（会话从未写入或账本文件缺失）"
                    ));
                }
                let stats = nemesis_agent::event_ledger::ledger_verify(&path)?;
                Ok(Some(serde_json::json!({
                    "session_id": session_id,
                    "events": stats.events,
                    "appends": stats.appends,
                    "truncates": stats.truncates,
                    "forks": stats.forks,
                    "rows_total": stats.rows_total,
                })))
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
                let workspace = require_workspace(ctx)?.to_string();
                // 标题实时解析（会话改名/删除跟随 live 语义）。
                // BUG-20261102-302：list_shares + 每条 session_title（每次 =
                // scan_session_logs 全目录全量解析）——N 条分享 = N 次全目录
                // 扫描，同步阻塞 IO 直跑 tokio worker，spawn_blocking 整体移出。
                let shares: Vec<serde_json::Value> = tokio::task::spawn_blocking(move || {
                    crate::share::list_shares(&workspace)
                        .into_iter()
                        .map(|s| {
                            let title = crate::share::session_title(&workspace, &s.session_id);
                            serde_json::json!({
                                "token": s.token,
                                "session_id": s.session_id,
                                "created_at": s.created_at,
                                "revoked": s.revoked,
                                "title": title,
                            })
                        })
                        .collect::<Vec<_>>()
                })
                .await
                .map_err(|e| format!("sessions.share_list task join failed: {e}"))?;
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
            // 会话回收站（2026-10-30）：sessions.delete 默认移入
            // `logs/trash/sessions/`，这里四个读侧命令管理它。条目寻址只见
            // 目录名（core 侧严格校验，防注入）；无在飞竞态（对象不是活会话），
            // 不进 history gate。
            "trash_list" => {
                let entries = nemesis_agent::chat_trash::trash_list();
                Ok(Some(serde_json::json!({ "entries": entries })))
            }
            "trash_restore" => {
                let entry = data
                    .as_ref()
                    .and_then(|d| d.get("entry"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "missing entry".to_string())?
                    .to_string();
                let new_sid = data
                    .as_ref()
                    .and_then(|d| d.get("new_sid"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                // BUG-20261031-248：trash_restore 内部 move_file_retry 带
                // 100ms×5 退避重试，spawn_blocking 包裹防阻塞 tokio worker
                // （LOW-5 同款纪律，与 nemesis-cluster 内 4 处先例对齐）。
                let out = {
                    let entry = entry.clone();
                    tokio::task::spawn_blocking(move || {
                        nemesis_agent::chat_trash::trash_restore(&entry, new_sid.as_deref())
                    })
                    .await
                    .map_err(|e| format!("trash_restore task join failed: {e}"))??
                };
                // 恢复成功（非 conflict）→ 项目归属重挂：meta 快照里的
                // project_id 仍注册则 bind 回去；已移除/未注册 = 摘除陈旧
                // sidecar 字段（会话落未分组，不进孤儿灰组）。
                let mut out = out;
                if out.get("conflict") != Some(&serde_json::Value::Bool(true))
                    && let Some(pid) = out
                        .get("project_id")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                {
                    let restored = out["restored"].as_str().unwrap_or_default().to_string();
                    let rebound = match crate::handlers::projects::projects_bridge() {
                        Some(bridge) => match bridge.bind_session(&restored, &pid) {
                            Ok(()) => Some(pid),
                            Err(_) => {
                                bridge.forget_session(&restored);
                                None
                            }
                        },
                        None => None,
                    };
                    out["project_rebound"] = match rebound {
                        Some(p) => serde_json::Value::String(p),
                        None => serde_json::Value::Null,
                    };
                }
                Ok(Some(out))
            }
            "trash_purge" => {
                let entry = data
                    .as_ref()
                    .and_then(|d| d.get("entry"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "missing entry".to_string())?
                    .to_string();
                nemesis_agent::chat_trash::trash_purge(&entry)?;
                Ok(Some(serde_json::json!({ "purged": entry })))
            }
            "trash_purge_all" => {
                let n = nemesis_agent::chat_trash::trash_purge_all()?;
                Ok(Some(serde_json::json!({ "purged": n })))
            }
            // L14 会话执行统计：8 字段只读视图（工具调用/失败/重试/校验失败/
            // 压缩/审批三面）。读盘不读内存——先经聚合器 flush pending 增量
            // 落盘再读 sidecar meta，进程重启后统计不归零。聚合器未装配
            // （headless 形态或老 gateway）= 诚实报错。
            "stats" => {
                let session_id = data
                    .as_ref()
                    .and_then(|d| d.get("session_id"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "missing session_id".to_string())?;
                // 双形态兼容：LogsView 会话浏览器传文件 stem
                // （`agent_main_session_*`，logs.session_list 口径），侧栏传
                // 裸 sid——统一剥前缀后走 sanitize。
                let bare = session_id
                    .strip_prefix("agent_main_session_")
                    .unwrap_or(session_id);
                let sid = nemesis_agent::session::SessionStore::sanitize_session_id(bare);
                let session_key = format!("agent:main:session:{sid}");
                if !nemesis_agent::chat_log::chat_log_exists(&session_key) {
                    return Err(format!("会话不存在: {sid}"));
                }
                let reader = ctx
                    .state
                    .session_stats
                    .as_ref()
                    .ok_or_else(|| "会话执行统计未装配（聚合器未运行）".to_string())?;
                let s = reader.flush_and_read(&session_key);
                Ok(Some(serde_json::json!({
                    "session_id": sid,
                    "stats": s,
                })))
            }
            _ => Err(format!("unknown sessions cmd: {}", cmd)),
        }
    }
}

// ---------------------------------------------------------------------------
// BUG-HUNT R6（2026-10-23）：delete/clear 操作体抽出——原 inline 臂原样搬移，
// 供 handle_cmd 在 `under_history_gate` 持闸下调用（归属 loop 在场时）；
// 无 loop（退化 ctx）直调不闸。本节函数自身不闸（参数解析同样留在闸外，
// 保证 busy 拒绝路径零副作用）。
// ---------------------------------------------------------------------------

/// BUG①（2026-11-05）：删除/清空链路的 live todo 清扫（回收站四件套之外
/// 的直删路径：hard 删除、clear 清空）。文件缺席 = no-op；路径走
/// nemesis-path 单一真相源（与 TodoWriteTool 写路径、回收站寻址同源）。
/// 删除失败 = Err 上浮（与 chat_log 同严格级——删不干净报出来，不静默
/// 留孤儿）；remove_file_retry 带 Windows 瞬时句柄重试，spawn_blocking
/// 包裹防阻塞 tokio worker（trash_chat_log 同款纪律）。
async fn remove_live_todo(workspace: &str, session_key: &str) -> Result<(), String> {
    let todo =
        nemesis_path::session_todo_file_in_workspace(std::path::Path::new(workspace), session_key);
    if !todo.exists() {
        return Ok(());
    }
    tokio::task::spawn_blocking(move || nemesis_utils::fsutil::remove_file_retry(&todo))
        .await
        .map_err(|e| format!("todo remove task join failed: {e}"))?
        .map_err(|e| format!("删除 todo 文件失败: {e}"))
}

/// sessions.delete 操作体：store 删除（内存 + sessions/*.json + jsonl）→
/// 方言 SessionEnd → cron 级联停用 → 项目归属/绑定摘除。
///
/// `hard=false`（默认，2026-10-30 回收站）：store 只 detach 内存/json
/// 上下文（不删 chat_log 三件套），三件套 rename 进
/// `logs/trash/sessions/{ms}_{safe}/`（可恢复，90d TTL 清扫）。顺序刻意
/// 先 detach 后移文件：两步间崩溃 = 会话从 store 消失但 jsonl 还在
/// sessions.list —— 表现为「没删掉」，重试即成；反序则 self-heal 会把
/// 已进回收站的会话复活成空 jsonl（删除丢失，方向错误）。
/// `hard=true` = 旧语义原样（store 删除含 jsonl + meta，不可恢复）。
async fn delete_session_body(
    ctx: &RequestContext,
    session_id: &str,
    session_key: &str,
    hard: bool,
    source: &str,
) -> Result<Option<serde_json::Value>, String> {
    // Clear SessionStore (in-memory + sessions/*.json) +
    // session_logs/*.jsonl. Best-effort; absence is not an error.
    let mut trash_entry: Option<String> = None;
    {
        let guard = ctx.state.agent_loop.read();
        if let Some(al) = guard.as_ref()
            && let Some(store) = al.session_store()
        {
            if hard {
                store.delete_session(session_key);
            } else {
                store.detach_session(session_key);
            }
        }
    }
    // BUG①（2026-11-05）：hard=true 不走回收站（旧语义不可恢复），todo
    // 文件也要一并删——四件套接管只覆盖软删链路。软删（!hard）的 todo 由
    // trash_chat_log 四件套化 rename 进条目，无需此处动作。
    if hard && let Some(ws) = ctx.workspace.as_deref() {
        remove_live_todo(ws, session_key).await?;
    }
    if !hard {
        // 移入回收站（guard 作用域外——trash 不持 store 锁）。三件套全
        // 缺席（从未物化）= Ok(None)，删除即完成（trashed:false）。
        // source 由调用方声明（单删 single / 批量循环 batch——回收站列表
        // 来源徽标的依据；展示性字段，未知值由 dispatch 层落 single）。
        // 移入回收站（guard 作用域外——trash 不持 store 锁）。三件套全
        // 缺席（从未物化）= Ok(None)，删除即完成（trashed:false）。
        // source 由调用方声明（单删 single / 批量循环 batch——回收站列表
        // 来源徽标的依据；展示性字段，未知值由 dispatch 层落 single）。
        // BUG-20261031-248：trash_chat_log 内部 move_file_retry 带
        // 100ms×5 退避重试，spawn_blocking 包裹防阻塞 tokio worker。
        trash_entry = {
            let key = session_key.to_string();
            let src = source.to_string();
            tokio::task::spawn_blocking(move || {
                nemesis_agent::chat_trash::trash_chat_log(&key, &src)
            })
            .await
            .map_err(|e| format!("trash_chat_log task join failed: {e}"))?
        }?;
    }
    // 方言 SessionEnd（观察型，2026-08-29 T3）：显式删除也触发
    // （桥经 AgentLoop 的 cc_hooks_bridge 访问；未装配 = 跳过）。
    // 先把桥 Arc 克隆出 guard 作用域，再 await（guard 不跨 await）。
    let session_end_bridge = {
        let guard = ctx.state.agent_loop.read();
        guard.as_ref().and_then(|al| al.cc_hooks_bridge())
    };
    if let Some(bridge) = session_end_bridge {
        bridge.on_session_end(session_key, "deleted").await;
    }
    // Cron cascade (2026-08-25): a scheduled job pinned to this
    // session_key would otherwise FIRE on a deleted conversation
    // and resurrect it — an empty jsonl re-created + cron rows
    // appended, a session the user explicitly deleted coming
    // back as a zombie. Disable (NOT remove: the job definition
    // stays on the Tasks page for re-pointing/re-enabling) every
    // ENABLED job whose payload targets this session, and report
    // what was paused. Disabled jobs are left untouched.
    let mut paused: Vec<serde_json::Value> = Vec::new();
    if let Some(svc) = ctx.state.cron.as_ref()
        && let Ok(svc) = svc.lock()
    {
        for job in svc.list_jobs(true) {
            if !job.enabled {
                continue;
            }
            if job.payload.session_key.as_deref() != Some(session_key) {
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
    // L23（三期 2026-11-03）：看板 autopilot 父会话绑定级联——绑定本会话
    // 的启用规则自动暂停 + 评论留痕（父会话删除后规则孤儿运行 = 绑定语义
    // 破约；回收站软删同样移走 chat_log → 一致裁决）。board 未装配 = 无
    // 绑定语义，跳过。
    #[cfg(feature = "board")]
    let paused_autopilots: Vec<i64> = match ctx.state.board.as_ref() {
        Some(svc) => crate::handlers::board::pause_autopilots_bound_to(
            svc.store(),
            session_key,
            "会话被删除",
        ),
        None => Vec::new(),
    };
    // L6++ G4（2026-09-08）：项目归属联动摘除（内存索引 + sidecar
    // project 字段——meta 已随 delete_chat_log 删除时为 no-op）。
    // bridge 未装配 = 无项目分组语义，跳过。
    if let Some(bridge) = crate::handlers::projects::projects_bridge() {
        bridge.forget_session(session_key);
    }
    // B（2026-09-23 多会话并行清账）：绑定注册表级联摘除（指向
    // 该会话的键全清，不留悬空绑定）。best-effort：workspace 缺
    // 席（退化 ctx）= 跳过；删除会话后绑定残留会让 get-or-create
    // 误判「已绑」，产生「保存了却找不到会话」的错觉。
    if let Some(ws) = ctx.workspace.as_deref() {
        let removed = crate::session_bindings::remove_session(ws, session_id);
        if removed > 0 {
            tracing::info!(
                "[sessions] delete cascade: removed {} binding(s) for {}",
                removed,
                session_id
            );
        }
    }
    let out = serde_json::json!({
        "deleted": session_id,
        "trashed": trash_entry.is_some(),
        "trash_entry": trash_entry,
        "paused_cron_jobs": paused,
    });
    // board 缺席时 out 不可变（影子投影——无 feature 不产 unused_mut）。
    #[cfg(feature = "board")]
    let out = {
        let mut out = out;
        out["paused_autopilots"] = serde_json::json!(paused_autopilots);
        out
    };
    Ok(Some(out))
}

/// sessions.clear 操作体：jsonl 先截 + store 清（self-heal 顺序）+
/// 项目归属摘除。clear 不做 cron 级联（会话还在，job 跟着空会话合法）。
async fn clear_session_body(ctx: &RequestContext, session_key: &str) -> Result<(), String> {
    // Clear SessionStore messages + session_logs jsonl (keep meta/key).
    // Order matters (2026-08-25 self-heal note): truncate the
    // chat_log FIRST, then drop the store json. If we crashed
    // between the two steps the other way round, the surviving
    // jsonl would let `rebuild_from_chat_log` resurrect the
    // content the user just asked to clear. jsonl-first keeps the
    // rebuild path unable to revive cleared data.
    nemesis_agent::chat_log::clear_chat_log(session_key)
        .map_err(|e| format!("清空会话失败: {e}"))?;
    // BUG①（2026-11-05）：清空 = 全新会话语义，todo 清单一并清（不参与
    // self-heal 重建，位置在 clear_chat_log 后保持「文件先动」惯例）。
    if let Some(ws) = ctx.workspace.as_deref() {
        remove_live_todo(ws, session_key).await?;
    }
    let guard = ctx.state.agent_loop.read();
    if let Some(al) = guard.as_ref()
        && let Some(store) = al.session_store()
    {
        store.clear_session(session_key);
    }
    drop(guard);
    // L6++ G4（2026-09-08）：clear 重置会话 → 项目归属一并解除
    // （摘索引 + sidecar project 字段，title 保留——防 owner_of
    // 的 sidecar 兜底把绑定「复活」）。bridge 未装配 = 跳过。
    if let Some(bridge) = crate::handlers::projects::projects_bridge() {
        bridge.forget_session(session_key);
    }
    // L23（三期 2026-11-03）：看板 autopilot 父会话绑定级联——clear 后
    // 父会话账本空（chat_log 截空 = L23 存活判据不成立），绑定规则自动
    // 暂停（规格明确 clear 也在裁决范围：空会话继承零上下文 = 绑定无意义，
    // 与 cron job 的「clear 不级联」刻意不同——那边会话骨架还在）。board
    // 未装配 = 无绑定语义，跳过。
    #[cfg(feature = "board")]
    if let Some(svc) = ctx.state.board.as_ref() {
        let paused = crate::handlers::board::pause_autopilots_bound_to(
            svc.store(),
            session_key,
            "会话被清空",
        );
        if !paused.is_empty() {
            tracing::info!(
                "[sessions] clear cascade: paused {} board autopilot(s) bound to {}",
                paused.len(),
                session_key
            );
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// B11（2026-10-18 snooze）：暂停时刻解析 + 到期唤醒 sweeper
// ---------------------------------------------------------------------------

/// 解析暂停时刻：i64 相对秒（自现在起）或 RFC3339 绝对时刻，统一归一为
/// RFC3339（本地时区）。过去时刻/非正秒数 = 诚实报错——「立即恢复」是
/// unsnooze 的语义，不静默吞掉（否则用户输错单位会把 8h 暂停变成 8s）。
pub(crate) fn parse_snooze_until(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    if let Ok(secs) = raw.parse::<i64>() {
        if secs <= 0 {
            return Err(format!("相对秒数必须为正整数: {raw}"));
        }
        // 用户可控秒数双重坏带：panicking 构造带（>~9.2e15s chrono 内部
        // 毫秒 i64 溢出）+ 加法溢出带（>~7.5e12s 溢出 NaDateTime 年域，
        // `+` 对 None expect panic）——try_seconds + checked_add_signed
        // 双闸，越界诚实报错不炸 handler。
        let delta = chrono::TimeDelta::try_seconds(secs)
            .ok_or_else(|| format!("秒数超出可表示范围: {raw}"))?;
        let due = chrono::Local::now()
            .checked_add_signed(delta)
            .ok_or_else(|| format!("暂停时刻超出可表示范围: {raw}"))?;
        return Ok(due.to_rfc3339());
    }
    let dt = chrono::DateTime::parse_from_rfc3339(raw).map_err(|_| {
        format!("until 需为相对秒数（如 3600）或 RFC3339（如 2026-10-19T09:00:00+08:00）: {raw}")
    })?;
    if dt.with_timezone(&chrono::Local) <= chrono::Local::now() {
        return Err(format!("暂停时刻已过去: {raw}"));
    }
    Ok(dt.with_timezone(&chrono::Local).to_rfc3339())
}

/// 到期唤醒 sweeper（gateway 常驻；upload sweeper 同款装配形态）。每 10s
/// 扫一遍 meta sidecar——量级 = 会话数（每个一个小 JSON 读），可忽略。
pub fn spawn_snooze_sweeper() {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
        interval.tick().await; // 首 tick 立即返回（启动期无需抢跑，10s 内兜住）
        loop {
            interval.tick().await;
            let woken = wake_due_sessions();
            if woken > 0 {
                tracing::info!("[sessions] snooze sweeper 唤醒 {woken} 个会话");
            }
            drain_orphan_pending();
        }
    });
}

// —— L22（2026-11-03 snooze 排队）：暂停期消息重放 hook ——

/// 重放 hook 签名：按会话键取走全部 pending（[`nemesis_agent::chat_log::
/// take_session_pending`]）并经该会话归属 loop 的 reinject 通道逐条重放，
/// 返回成功注入条数。loop 缺席（停机窗口/未装配）= 0——pending 落盘保
/// 留，sweeper 下轮重试（自愈不丢失）。
pub type SnoozeReplayHook = std::sync::Arc<dyn Fn(&str) -> usize + Send + Sync>;

/// 进程级 hook 槽。**设计偏差说明**（vs OnceLock，PROJECTS_BRIDGE 同款）：
/// OnceLock 只能 set 一次，crate 测试需要「无 hook」与「假 hook」形态并
/// 存（并行测试共享槽）——`RwLock<Option<_>>` 生产语义相同（gateway 装配
/// 期安装一次、sweeper/unsnooze 只读）且测试可置换。
static SNOOZE_REPLAY_HOOK: std::sync::RwLock<Option<SnoozeReplayHook>> =
    std::sync::RwLock::new(None);

/// gateway 装配期安装重放 hook（runtime.rs：三臂 loop 解析 + inject_inbound）。
pub fn set_snooze_replay_hook(hook: Option<SnoozeReplayHook>) {
    *SNOOZE_REPLAY_HOOK.write().expect("snooze replay hook lock") = hook;
}

/// 调 hook 重放某会话的 pending；无 hook（单测/未装配）= 0（pending 保
/// 留在盘上，不丢）。pub(crate) 供测试直调（wake_due_sessions 同先例）。
pub(crate) fn replay_pending_for(session_key: &str) -> usize {
    let hook = SNOOZE_REPLAY_HOOK
        .read()
        .expect("snooze replay hook lock")
        .clone();
    match hook {
        Some(h) => h(session_key),
        None => 0,
    }
}

/// L22 滞留兜底（sweeper 每轮）：摘标记后 loop 短暂不可用的停机窗口会把
/// pending 滞留在盘上（标记已摘，正常唤醒不再覆盖）——每轮扫 pending
/// 残留，对「非暂停中」的会话补投。会话已删除（jsonl 没了）= 重放无主，
/// 就地清 pending（防复活已删会话 + 防无限重试）。hook 返 0（loop 仍缺
/// 席）= 文件保留下轮重试。量级：pending 文件只在暂停窗口/滞留场景存在，
/// 常态零文件零开销。pub(crate) 供测试直调。
pub(crate) fn drain_orphan_pending() {
    for key in nemesis_agent::chat_log::pending_sessions() {
        if nemesis_agent::chat_log::session_snoozed_until(&key).is_some() {
            continue; // 仍在暂停期——正常唤醒流程负责
        }
        if !nemesis_agent::chat_log::chat_log_exists(&key) {
            // 会话已删除：pending 随之作废（delete_chat_log 级联遗漏的
            // 竞态残留在此收敛）。直接 take 后丢弃——**不能**走 hook：
            // loop 解析对已删会话仍命中主槽，注入会让 process_admitted
            // 早落盘复活已删会话。
            let discarded = nemesis_agent::chat_log::take_session_pending(&key);
            tracing::info!(
                "[sessions] 会话已删除，作废滞留 pending {} 条: {key}",
                discarded.len()
            );
            continue;
        }
        let delivered = replay_pending_for(&key);
        if delivered > 0 {
            tracing::info!("[sessions] 滞留 pending 补投 {delivered} 条: {key}");
        }
    }
}

/// 扫描到期暂停项并执行唤醒，返回唤醒数（pub(crate) 供测试直调）。
///
/// 唤醒编排五件套：摘标记（chat_log 真相源，竞速安全——并发 sweeper/
/// unsnooze 只有一方拿到 Some）→ 注入恢复 system 行（jsonl 真相源）→
/// `chat_event_log::record` 入环 + chat.activity 广播（sweep 不经 web 的
/// record 点，已开 ChatPanel 窗口的活同步必须在这里手动补）→
/// `publish_global_event("session.updated")`（侧栏 force 刷新信号）→
/// **重放暂停期排队的消息**（L22：经 replay hook 逐条 inject_inbound；
/// 标记已摘所以重放消息过 gate 不再被排队——顺序不可颠倒，先重放会被
/// 本闸再度排队成环）。
pub(crate) fn wake_due_sessions() -> usize {
    let now = chrono::Local::now();
    let mut woken = 0;
    for (stem, until, _) in nemesis_agent::chat_log::snoozed_sessions() {
        // 只认 web 命名空间（snooze 写入路径只有 sessions handler，只收
        // web sid）；其他 stem 防御性跳过，不做有损还原。
        let Some(sid) = stem.strip_prefix("agent_main_session_") else {
            continue;
        };
        let Ok(due) = chrono::DateTime::parse_from_rfc3339(&until) else {
            // 坏标记不阻塞其他项。写入路径只产 RFC3339，坏标记只会来自手
            // 工改盘——静默跳过（sweeper 10s 一轮，WARN 会周期刷屏）。
            continue;
        };
        if due.with_timezone(&chrono::Local) > now {
            continue;
        }
        let session_key = format!("agent:main:session:{sid}");
        // BUG-20261031-231：会话 jsonl 重写窗口（rewind/redo）进行中 →
        // 本轮整条目跳过（标记保留，10s 后下轮重试——窗口通常亚秒级）。
        // 必须先于摘标记：旧序「摘标记 → 落唤醒行」在窗口内行被截断吞掉
        // = 标记已摘、留痕永久丢失。
        if nemesis_agent::chat_log::history_rewrite_active(&session_key) {
            continue;
        }
        let Some((_, note)) = nemesis_agent::chat_log::clear_session_snooze(&session_key) else {
            continue; // 已被并发方摘掉
        };
        // 会话可能在暂停期间被删除——不复活（no jsonl = scan 不可见）。
        if !nemesis_agent::chat_log::chat_log_exists(&session_key) {
            continue;
        }
        let content = match note {
            Some(n) => format!("⏰ 会话暂停已到期，自动恢复（{n}）"),
            None => "⏰ 会话暂停已到期，自动恢复".to_string(),
        };
        nemesis_agent::chat_log::append_chat_log(&session_key, "system", &content);
        // BUG-HUNT R9：record 键空间契约是完整会话键（send_to_session
        // 「记录键优先会话键（跨连接稳定）」、record_tool「与 send_to_session
        // 同域」），chat.sync 按完整键重建桶查。此前传裸 sid——恢复行落进
        // 无人查询的孤儿桶，断线窗口内唤醒的会话重连补拉看不到此行
        // （jsonl 有、补拉无）。publish_activity 内部剥前缀还原裸 sid，
        // chat.activity 信号形态不变。
        crate::chat_event_log::record(&session_key, "system", &content, None, None, None);
        crate::chat_event_log::publish_global_event(
            "session.updated",
            serde_json::json!({
                "session_id": sid,
                "snoozed_until": serde_json::Value::Null,
            }),
        );
        // L22：重放暂停期排队的消息（标记已摘——重放消息过 gate 不再被
        // 排队）。loop 缺席（停机窗口）= 0，pending 落盘保留，sweeper 下
        // 轮的 orphan 兜底臂补投（自愈不丢失）。
        let delivered = replay_pending_for(&session_key);
        if delivered > 0 {
            tracing::info!("[sessions] snooze 唤醒重放 {delivered} 条暂停期消息: {session_key}");
        }
        woken += 1;
    }
    woken
}
