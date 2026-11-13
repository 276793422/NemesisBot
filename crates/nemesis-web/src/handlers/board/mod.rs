//! Board handler — managed-agent 看板（加角色门控）。
//!
//! 所有命令经 `AppState.board` 的 [`nemesis_board::BoardService`]（gateway 在
//! `board` feature 开启时注入；未注入时统一报 "board service not available"）。
//! 状态转移 / 指派走 store 的状态机接口（非法转移被拒），普通字段更新走 patch。
//! 操作者身份：dashboard 登录用户 → `Actor::admin("dashboard")`（固定 id，见
//! `ctx_actor` 注释——session 每连必换，按 session 建身份会让通知按订阅者扇出重复）。
//!
//! **角色门控（已移除，2026-08-31，见 board/tests.rs 回归钉）**：board.db 是
//! 节点本地数据（无集群同步、CLI 同权直写），worker 对本机看板有完整写权
//! （完整 CRUD；回归钉 `test_worker_role_never_403` 防止 role 403 门控回归）。
//! 集群权威语义只体现在 `issue.dispatch`/`issue.cancel` 的派发链路上
//! （dispatch 把任务发到 coordinator 选定的 worker，回报写回发起方看板）。

use crate::handlers::{get_opt_str, get_str, require_workspace};
use crate::ws_router::{ModuleHandler, RequestContext};
use base64::Engine;
use nemesis_board::BoardStore;
// ⏸ 暂缓评论去重标记（闭环-13 从本文件上移）：停车判定 SQL
// 与 sweep 落评论共用统一定义，消灭跨 crate 字面量漂移面。
use nemesis_board::PARK_NOTICE_MARK;
use nemesis_board::assignment::{Actor, AssignmentType};
use nemesis_board::models::{
    CommentType, IssueFilter, IssuePatch, IssueStatus, NewComment, NewIssue, ProjectPatch,
};
// 只被 cluster 门控的 sync/cascade 系函数使用（batch-3 SAN-06/08）。
#[cfg(feature = "cluster")]
use nemesis_board::store::DescendantEdges;
use std::sync::Arc;

pub struct BoardHandler;

/// 附件大小上限（base64 解码后字节数；WS 帧默认 16MB，留余量）。
const MAX_ATTACHMENT_BYTES: usize = 8 * 1024 * 1024;

/// Get the board service handle from AppState, or error if not injected.
fn require_board(ctx: &RequestContext) -> Result<Arc<BoardStore>, String> {
    ctx.state
        .board
        .as_ref()
        .map(|svc| svc.store().clone())
        .ok_or_else(|| "board service not available".to_string())
}

/// Request author → Actor（dashboard 用户记为 admin，固定身份 "dashboard"）。
///
/// ⚠ 不用 WS `session_id`：session 每次页面加载/重连都换新（session.rs
/// `generate_session_id` 随机 16 hex），会以新身份落订阅表（`issue_subscriber`
/// 唯一键含 subscriber_id，去重失效）并永久累积——而通知按订阅者扇出，
/// 同一事件给 admin 落 N 份，收件箱同一条消息重复 N 次（
/// 一次验收失败收件箱 6 份，18 个历史 admin 身份）。board.db 是
/// 节点本地数据、单管理员语义（inbox 面向 admin 通配），固定 id 即可让
/// 订阅 ON CONFLICT 去重生效；鉴权在 WS token 层，与此 id 无关。
fn ctx_actor(_ctx: &RequestContext) -> Actor {
    Actor::admin("dashboard")
}

/// 解析可选指派对：`assignee_type`（"manager_self"/"worker"）+ `assignee_id`。
/// 二者必须同时出现或同时缺失。
fn parse_assignee(data: &serde_json::Value) -> Result<Option<(AssignmentType, String)>, String> {
    let at = get_opt_str(data, "assignee_type");
    let aid = get_opt_str(data, "assignee_id");
    match (at, aid) {
        (None, None) => Ok(None),
        (Some(t), Some(id)) => {
            let at = AssignmentType::from_str(&t)
                .ok_or_else(|| format!("未知 assignee_type: {t}（可选 manager_self/worker）"))?;
            Ok(Some((at, id)))
        }
        (Some(_), None) => Err("assignee_type 与 assignee_id 必须成对提供".to_string()),
        (None, Some(_)) => Err("assignee_type 与 assignee_id 必须成对提供".to_string()),
    }
}

/// 解析必填 status 字符串 → IssueStatus。
fn parse_status(data: &serde_json::Value) -> Result<IssueStatus, String> {
    let s = get_str(data, "status")?;
    IssueStatus::from_str(&s).ok_or_else(|| format!("未知 status: {s}"))
}

/// 附件文件名消毒：取路径最后一段（防目录穿越），拒空/`.`/`..`/控制字符/
/// 超长。只保留基本名，原始名存 attachment.filename 供展示。
fn sanitize_filename(name: &str) -> Result<String, String> {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("").trim();
    if base.is_empty() || base == "." || base == ".." {
        return Err("非法附件文件名".to_string());
    }
    if base.chars().any(char::is_control) {
        return Err("附件文件名含控制字符".to_string());
    }
    if base.len() > 200 {
        return Err("附件文件名过长（>200 字节）".to_string());
    }
    Ok(base.to_string())
}

#[async_trait::async_trait]
impl ModuleHandler for BoardHandler {
    fn module_name(&self) -> &str {
        "board"
    }

    fn commands(&self) -> &'static [&'static str] {
        &[
            "issue.list",
            "issue.get",
            "issue.decision",
            "issue.timeline",
            "issue.create",
            "issue.update",
            "issue.assign",
            "issue.status",
            "issue.move",
            "issue.dispatch",
            "issue.cancel",
            "issue.cancel_dispatch",
            "issue.reopen",
            "issue.plan",
            "issue.draft_ac",
            "board.resume",
            "plan.list",
            "plan.get",
            "issue.bulk_archive",
            "board.needs_human.list",
            "autopilot.list",
            "autopilot.create",
            "autopilot.update",
            "autopilot.remove",
            "autopilot.run",
            "autopilot.runs",
            "comment.add",
            "comment.list",
            "activity.list",
            "subscriber.add",
            "subscriber.remove",
            "subscriber.list",
            "project.list",
            "project.progress",
            "project.resume",
            "project.create",
            "project.update",
            "project.open_dir",
            "attachment.add",
            "attachment.get",
            "inbox.list",
            "inbox.mark_read",
            "attachment.list",
            "channel.list",
            "channel.messages",
            "channel.post",
            "stats",
            "audit.list",
            "audit.rollback",
            "audit.retry_merge",
            "review.retrigger",
            "unavailable.status",
            "unavailable.release",
            "config.get",
            "config.set",
            "dossier.export",
            "delivery.publish",
            "touchpoint.list",
        ]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        // 单节点模式下 board 命令不依赖 workspace（store 由 gateway 按已解析
        // home 打开），但保留 workspace 校验与其他 handler 一致。
        require_workspace(ctx)?;
        let store = require_board(ctx)?;
        let actor = ctx_actor(ctx);
        // P2：触点恢复钩子装配（install-once；首个 board 命令即装，之后
        // 零开销 OnceLock 早退）——确认/超时默认后 spec→规划链续跑的桥。
        #[cfg(feature = "cluster")]
        touchpoints::ensure_touchpoint_resume_hook_installed(ctx.state.clone());
        match cmd {
            // issue ---
            "issue.list" => {
                let data = data.ok_or("missing data")?;
                let issues = store.list_issues(&build_filter(&data)?)?;
                let total = issues.len();
                // goal /：逐单附环节（issue_stage 统一定义）——前端
                // 子单行环节徽标数据源；加法变更，旧前端兼容。
                // 拆解运行期投影（cluster 编译时）：命中 RUNNING_PLANS
                // 的单附 `planning: {elapsed_secs, attempt, phase}`（纯展示叠
                // 加，不改 issue 状态机——backlog 语义不变）。phase=
                // planning（planner LLM 运行中）/ dispatching（auto_confirm
                // 发车派发中——子单已可见，前端徽标应显示「发车中」）。
                #[cfg(feature = "cluster")]
                let planning_now: std::collections::HashMap<
                    i64,
                    (u64, u32, &'static str),
                > = {
                    let now = std::time::Instant::now();
                    RUNNING_PLANS
                        .lock()
                        .values()
                        .map(|r| {
                            (
                                r.issue_id,
                                (
                                    now.duration_since(r.started_at).as_secs(),
                                    r.attempt,
                                    r.phase,
                                ),
                            )
                        })
                        .collect()
                };
                let mut rows = Vec::with_capacity(issues.len());
                // 依赖徽标批量投影：边集 + 轻投影各一次查询，
                // 不逐单 dependencies_of（N+1；规模不设限——全表读千行毫秒级）。
                // briefs 是全表（不带 filter），上游被 exclude 过滤掉也照常投影。
                let briefs = store.issue_briefs()?;
                let mut deps_by_issue: std::collections::HashMap<i64, Vec<i64>> =
                    std::collections::HashMap::new();
                for (issue_id, dep_id) in store.all_dependency_edges()? {
                    deps_by_issue.entry(issue_id).or_default().push(dep_id);
                }
                // 人工触点徽标批量投影（P1）：活卡一次查询，per-issue 只做
                // map 命中（同依赖徽标模式，不 N+1）。同一单多个活卡取最早
                // deadline 的（list 按 deadline 升序 + 首见覆盖）——「评审中」
                // 列置顶排序 + 触点卡渲染的数据源。
                let mut pending_tp_by_issue: std::collections::HashMap<
                    i64,
                    nemesis_board::models::Touchpoint,
                > = std::collections::HashMap::new();
                for tp in store.list_pending_touchpoints()? {
                    pending_tp_by_issue.entry(tp.issue_id).or_insert(tp);
                }
                // 决策可见性投影：停车档案一次性扫 + PLAN_CACHE
                // 一次快照，per-issue 只做 map 命中与一条 last_system_comment
                // 查询（仅 in_progress 单，indexed）。
                let handoffs = scan_live_handoffs();
                #[cfg(feature = "cluster")]
                let plan_ready = plan_ready_by_issue();
                for issue in &issues {
                    let mut v =
                        serde_json::to_value(issue).map_err(|e| format!("serialize issue: {e}"))?;
                    if let Some(obj) = v.as_object_mut() {
                        let depends_on: Vec<serde_json::Value> = deps_by_issue
                            .get(&issue.id)
                            .map(|deps| {
                                deps.iter()
                                    .map(|dep_id| {
                                        let (number, status) = briefs
                                            .get(dep_id)
                                            .cloned()
                                            .unwrap_or(("?".to_string(), "unknown".to_string()));
                                        serde_json::json!({
                                            "id": dep_id,
                                            "number": number,
                                            "status": status,
                                        })
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        obj.insert("depends_on".to_string(), serde_json::json!(depends_on));
                        obj.insert(
                            "stage".to_string(),
                            serde_json::json!(issue_stage(&store, issue)?),
                        );
                        // 强信号：停车等裁决（✋ 待你决策徽标数据源）。
                        if let Some(nu) = needs_user_projection(&handoffs, issue) {
                            obj.insert("needs_user".to_string(), nu);
                        }
                        // 人工触点（P1）：活卡轻投影（badge/置顶/倒计时数据源；
                        // 全量触点走 touchpoint.list / workbench.get）。None
                        // 不插键（无卡单前端零感）。
                        if let Some(tp) = pending_tp_by_issue.get(&issue.id) {
                            obj.insert(
                                "touchpoint".to_string(),
                                serde_json::json!({
                                    "id": tp.id,
                                    "kind": tp.kind.as_str(),
                                    "deadline_ms": tp.deadline_ms,
                                    "veto_rounds": tp.veto_rounds,
                                }),
                            );
                        }
                        // 中信号：拆解完成等确认发车（📋 计划待确认徽标数据源）。
                        #[cfg(feature = "cluster")]
                        if let Some((plan_id, subs_count, age)) = plan_ready.get(&issue.id) {
                            obj.insert(
                                "plan_ready".to_string(),
                                serde_json::json!({
                                    "plan_id": plan_id,
                                    "subs_count": subs_count,
                                    "created_age_secs": age,
                                }),
                            );
                        }
                        // 弱信号：执行档案停滞（⚠ 停滞徽标数据源；只提示不给按钮）。
                        if issue_needs_attention(&store, issue) {
                            obj.insert("attention".to_string(), serde_json::json!(true));
                        }
                        #[cfg(feature = "cluster")]
                        if let Some((elapsed, attempt, phase)) = planning_now.get(&issue.id) {
                            obj.insert(
                                "planning".to_string(),
                                serde_json::json!({
                                    "elapsed_secs": elapsed,
                                    "attempt": attempt,
                                    "phase": phase,
                                }),
                            );
                        }
                    }
                    rows.push(v);
                }
                Ok(Some(serde_json::json!({ "issues": rows, "total": total })))
            }
            "issue.get" => {
                let data = data.ok_or("missing data")?;
                let issue = if let Ok(id) = data.get("id").and_then(|v| v.as_i64()).ok_or(()) {
                    store.get_issue(id)?
                } else {
                    store.get_issue_by_number(&get_str(&data, "number")?)?
                };
                Ok(Some(
                    serde_json::json!({ "issue": issue_to_view(&store, &issue)? }),
                ))
            }
            // 决策面板数据源（决策可见性-15）：`issue.decision
            // {number|id}` → needs_user / plan_ready / attention 投影 +
            // 停车档案全量（verdict_chain / environment / attempted /
            // resume_entry，详情页「更多上下文」折叠）+ 在线节点清单（重派
            // 按钮下拉）。与 issue.list 同构字段；档案全量只在此按需给
            //（列表面保持轻）。
            "issue.decision" => {
                let data = data.ok_or("missing data")?;
                let issue = if let Ok(id) = data.get("id").and_then(|v| v.as_i64()).ok_or(()) {
                    store.get_issue(id)?
                } else {
                    store.get_issue_by_number(&get_str(&data, "number")?)?
                };
                let handoffs = scan_live_handoffs();
                let needs_user = needs_user_projection(&handoffs, &issue);
                // 档案全量（仅 needs_user 成立时随发——状态闸外的档案是漏
                // 归档残留，不给面板喂过期现场）。
                let handoff_full = if needs_user.is_some() {
                    handoffs.get(&issue.number).cloned()
                } else {
                    None
                };
                #[cfg(feature = "cluster")]
                let plan_ready = plan_ready_by_issue().remove(&issue.id).map(
                    |(plan_id, subs_count, created_age_secs)| {
                        serde_json::json!({
                            "plan_id": plan_id,
                            "subs_count": subs_count,
                            "created_age_secs": created_age_secs,
                        })
                    },
                );
                #[cfg(not(feature = "cluster"))]
                let plan_ready: Option<serde_json::Value> = None;
                let attention = issue_needs_attention(&store, &issue);
                // 在线节点清单（重派下拉数据源）：非本节点、在线者。非
                // cluster 编译恒空（派发本就无从谈起）。
                #[allow(unused_mut)]
                let mut online_nodes: Vec<serde_json::Value> = Vec::new();
                #[cfg(feature = "cluster")]
                if let Some(c) = ctx.state.cluster.as_ref() {
                    let local = c.node_id();
                    online_nodes.extend(
                        c.list_nodes()
                            .iter()
                            .filter(|n| n.is_online() && n.base.id != local)
                            .map(|n| {
                                serde_json::json!({
                                    "name": n.base.name,
                                    "role": n.base.role,
                                    "professions": n.professions,
                                    "tier": n.tier,
                                })
                            }),
                    );
                }
                Ok(Some(serde_json::json!({
                    "issue": issue.number,
                    "issue_id": issue.id,
                    "status": issue.status.as_str(),
                    "needs_user": needs_user,
                    "plan_ready": plan_ready,
                    "attention": attention,
                    "handoff": handoff_full,
                    "online_nodes": online_nodes,
                })))
            }
            // U-C：单子全链路时间线（可追溯查询面——终结
            // 「人查 4 个库」）。聚合入口与 dossier timeline.md 同源
            // （nemesis-board::timeline）；集群侧事件（任务结果 / 续行快照
            // 在场）在此注入。按 (ts, source_rank) 升序。
            "issue.timeline" => {
                let data = data.ok_or("missing data")?;
                let issue = if let Ok(id) = data.get("id").and_then(|v| v.as_i64()).ok_or(()) {
                    store.get_issue(id)?
                } else {
                    store.get_issue_by_number(&get_str(&data, "number")?)?
                };
                #[allow(unused_mut)] // 非 cluster 编译：extra 恒空仍照常传聚合入口
                let mut extra: Vec<nemesis_board::timeline::TimelineEvent> = Vec::new();
                #[cfg(feature = "cluster")]
                if let Some(cluster) = ctx.state.cluster.as_ref() {
                    for d in store.list_dispatches(issue.id)? {
                        // 已终结任务：结果存储在案（success/failure + stored_at）。
                        if let Some(r) = cluster.result_store().get(&d.task_id) {
                            extra.push(nemesis_board::timeline::TimelineEvent {
                                ts: nemesis_board::timeline::parse_ts_to_epoch(&r.stored_at),
                                source: "cluster_task",
                                kind: if r.success { "success" } else { "failure" }.into(),
                                actor: d.worker_id.clone(),
                                summary: format!(
                                    "任务结果（{}）：{}",
                                    r.action,
                                    if r.success { "成功" } else { "失败" }
                                ),
                                detail: r.result.as_str().map(|s| s.to_string()).or_else(|| {
                                    r.result
                                        .get("error")
                                        .and_then(|e| e.as_str())
                                        .map(String::from)
                                }),
                            });
                        } else if cluster
                            .continuation_store()
                            .exists_on_disk(&d.task_id)
                            .await
                        {
                            // 在途：续行快照在场（任务未终结）。盘上判定
                            // （BUG-20261113-165）——快照由 agent 侧 store
                            // 落盘，cluster 侧内存 contains 恒空（分支永假）。
                            extra.push(nemesis_board::timeline::TimelineEvent {
                                ts: d.dispatched_at,
                                source: "continuation",
                                kind: "pending".into(),
                                actor: d.worker_id.clone(),
                                summary: format!("任务 {} 在途（续行快照在场）", d.task_id),
                                detail: Some(d.task_id),
                            });
                        }
                    }
                }
                let events =
                    nemesis_board::timeline::collect_issue_timeline(&store, issue.id, extra);
                Ok(Some(serde_json::json!({
                    "issue": issue.number,
                    "issue_id": issue.id,
                    "events": events,
                })))
            }
            "issue.create" => {
                let data = data.ok_or("missing data")?;
                let issue = store.create_issue(build_new_issue(&data, actor.clone())?)?;
                let mut out =
                    serde_json::json!({ "created": true, "issue": issue_to_view(&store, &issue)? });
                // 件①：建单警示——验收标准含 `[CHECK] re:` 锚点时提醒
                // 锚点下放纪律（叶子单直接执行不受影响；本单日后被 AI 拆解
                // 成父单则父层无交付文本，锚点必须由子单继承才可核验）。
                if nemesis_board::has_content_regex_anchor(
                    issue.acceptance_criteria.as_deref().unwrap_or(""),
                ) {
                    out["warning"] = serde_json::json!(
                        "验收标准含 [CHECK] re: 锚点：本单若后续被 AI 拆解为父单，父单层没有交付文本可核验该锚点（验收必败）——拆解时请确保至少一个子单继承该锚点。直接执行（不拆解）的单不受影响。"
                    );
                }
                // .5 接线补全（用户反馈）：列表页创建
                // 弹窗把指派目标内联在 create 请求里，此前自动派发钩子只接
                // 在 issue.assign 命令上——board.auto_dispatch=true 且创建
                // 即指派 worker 时同样发车，否则「创建+选目标」永远静默
                // 躺 backlog。失败不回滚创建（与 assign 路径同语义：⛔ 评论
                // 留痕在钩子内）。派发推进了状态 → 响应重取反映 in_progress。
                #[cfg(feature = "cluster")]
                let dispatched =
                    auto_dispatch_after_assign(&store, ctx.state.cluster.as_ref(), &issue, &actor);
                #[cfg(not(feature = "cluster"))]
                let dispatched = auto_dispatch_after_assign(&store, &issue, &actor);
                if dispatched {
                    let issue = store.get_issue(issue.id)?;
                    out["issue"] = issue_to_view(&store, &issue)?;
                }
                Ok(Some(out))
            }
            "issue.update" => {
                let data = data.ok_or("missing data")?;
                reject_nested_patch(&data)?;
                let id = data
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: id")?;
                let patch = build_patch(&data);
                let issue = store.update_issue(id, &patch, &actor)?;
                Ok(Some(
                    serde_json::json!({ "updated": true, "issue": issue_to_view(&store, &issue)? }),
                ))
            }
            "issue.assign" => {
                let data = data.ok_or("missing data")?;
                let id = data
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: id")?;
                let (assignee, assignee_id) = match parse_assignee(&data)? {
                    Some((at, aid)) => (Some(at), Some(aid)),
                    None => (None, None),
                };
                let issue = store.assign_issue(id, assignee, assignee_id, &actor)?;
                // 自动派发接口（.5 预留，默认关；board.auto_dispatch=true
                // 且指派给 worker 时触发）。触发后重取 issue，响应反映派发
                // 推进的 in_progress 态。
                #[cfg(feature = "cluster")]
                let dispatched =
                    auto_dispatch_after_assign(&store, ctx.state.cluster.as_ref(), &issue, &actor);
                #[cfg(not(feature = "cluster"))]
                let dispatched = auto_dispatch_after_assign(&store, &issue, &actor);
                let issue = if dispatched {
                    store.get_issue(id)?
                } else {
                    issue
                };
                Ok(Some(
                    serde_json::json!({ "assigned": true, "issue": issue_to_view(&store, &issue)? }),
                ))
            }
            "issue.status" => {
                let data = data.ok_or("missing data")?;
                let id = data
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: id")?;
                let to = parse_status(&data)?;
                let issue = store.transition_issue(id, to, &actor)?;
                // 已裁决（通过/取消）= 恢复动作之一：归档停车档案（清「在
                // 等人」徽标；best-effort，下同）。
                if matches!(to, IssueStatus::Done | IssueStatus::Cancelled) {
                    archive_handoff_best_effort(&issue.number);
                }
                // 补派触发器 + 父单收口（子单落定后联动；错误只 warn）。
                #[cfg(feature = "cluster")]
                if matches!(to, IssueStatus::Done | IssueStatus::Cancelled) {
                    on_issue_settled(&store, ctx.state.cluster.as_ref(), id, &actor);
                }
                Ok(Some(
                    serde_json::json!({ "changed": true, "issue": issue_to_view(&store, &issue)? }),
                ))
            }
            // 看板拖拽：状态转移 + 列内排序一个原子操作（同列重排
            // 只改 position；跨列走状态机）。
            "issue.move" => {
                let data = data.ok_or("missing data")?;
                let id = data
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: id")?;
                let to = parse_status(&data)?;
                let position = data
                    .get("position")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: position")?;
                let issue = store.move_issue(id, to, position, &actor)?;
                // 拖到终态同 issue.status（已裁决 → 归档停车档案）。
                if matches!(to, IssueStatus::Done | IssueStatus::Cancelled) {
                    archive_handoff_best_effort(&issue.number);
                }
                // 补派触发器 + 父单收口（跨列拖到 done/cancelled 同样落定）。
                #[cfg(feature = "cluster")]
                if matches!(to, IssueStatus::Done | IssueStatus::Cancelled) {
                    on_issue_settled(&store, ctx.state.cluster.as_ref(), id, &actor);
                }
                Ok(Some(
                    serde_json::json!({ "moved": true, "issue": issue_to_view(&store, &issue)? }),
                ))
            }
            // 派发到远端 worker：issue → peer_chat，task_id ↔ issue
            // 绑定入 issue_dispatch 表；worker 回报经 peer_chat_callback 由
            // gateway 写回看板（成功 → 结果评论 + in_review，失败 → 失败评论）。
            "issue.dispatch" => issue_dispatch(&store, actor, ctx, data).await,
            // 取消进行中的派发：A 侧派发/issue 双终态 + 下行
            // task_cancel 让 worker abort。赢竞态才动账（worker 恰好回报则
            // 拒绝取消，issue 保持写回的状态）。响应带 cascade_cancelled
            //（被级联取消的子单编号清单）。
            "issue.cancel" => issue_cancel(&store, actor, ctx, data).await,
            // 只撤派发、单据存活：与 issue.cancel（连单终杀）互补——
            // 派错节点换人重派 / worker 假死不等超时 sweep 的轻量出口。
            // issue 状态不动（可直接重派），写回幂等早退兜住迟到回报。
            "issue.cancel_dispatch" => issue_cancel_dispatch(&store, actor, ctx, data).await,
            // reopen（发现④）：cancelled → backlog 唯一终态
            // 出口，自动追加审计评论 + 解除 hidden；与 CLI `issue reopen`
            // 共用 store.reopen_issue 单一后端。
            "issue.reopen" => {
                let data = data.ok_or("missing data")?;
                let id = data
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: id")?;
                let issue = store.reopen_issue(id, &actor)?;
                // 重开 = 用户接手（恢复动作之一）：归档停车档案。
                archive_handoff_best_effort(&issue.number);
                Ok(Some(
                    serde_json::json!({ "reopened": true, "issue": issue_to_view(&store, &issue)? }),
                ))
            }
            // AI 拆解（Swarm）：confirm 缺省/false = 异步跑 planner（返回
            // plan_id，结果经 board.plan_ready/failed push）；confirm:true =
            // 按 plan_id 落库 + 依赖闸派发波 + 父单联动。
            "issue.plan" => issue_plan(&store, actor, ctx, data).await,
            // 验收标准 AI 起草（AC 缺失根修的人工面）：草稿只回前端
            // 预填，不落库——用户编辑后走既有 issue.update 保存（AI 只起草，
            // 落库决定权在用户）。父单（汇总子单材料）与子单/独立单通用。
            "issue.draft_ac" => {
                let data = data.ok_or("missing data")?;
                // 前置闸先行：agent 未运行时无论单据存在与否都报同一语义
                //（装配性错误优先于数据错误，报错不误导）。
                let agent_loop = ctx
                    .state
                    .agent_loop
                    .read()
                    .clone()
                    .ok_or("agent 未运行，无法执行 AI 起草")?;
                let issue = if let Ok(number) = get_str(&data, "number") {
                    store.get_issue_by_number(&number)?
                } else if let Some(id) = data.get("id").and_then(|v| v.as_i64()) {
                    store.get_issue(id)?
                } else {
                    return Err("issue.draft_ac 需要 number（NB-N）或 id 之一".into());
                };
                let prompt = build_issue_ac_draft_prompt(&store, &issue)?;
                let draft = run_ac_draft(&agent_loop, &prompt, None).await?;
                tracing::info!(
                    "[Board] issue.draft_ac issue={} by {}/{}（{} 字草稿回前端预填）",
                    issue.number,
                    actor.kind,
                    actor.id,
                    draft.chars().count()
                );
                Ok(Some(
                    serde_json::json!({ "issue": issue.number, "draft": draft }),
                ))
            }
            // 停车续做正门（主会话污染根修）：自动化经此读档续做，不再借道
            // 用户聊天会话。受理即回，run_detached 独立会话后台执行（带全量
            // 工具面），结论异步落回本单系统评论。人的聊天路径（「继续 NB-x」）
            // 保留——那是人对自己会话的选择；自动化从此走这扇门。
            "board.resume" => board_resume(store, ctx, data.as_ref()).await,
            // 观测面：plan 预览查询（list 全量存活按 issue_id 可选过滤 /
            // get 单查；TTL 惰性过滤与插入点同判据）。
            "plan.list" => plan_list(&store, data),
            "plan.get" => plan_get(data),
            // 取消单清理：批量给已取消单打 hidden 标记（永久收起，
            // 列表面无放行口）。只认 status=cancelled——混入非取消单整体拒绝
            //（不是跳过，语义诚实）；前端二次确认后调用；逐单写
            // issue_bulk_archive 审计。
            "issue.bulk_archive" => {
                let data = data.ok_or("missing data")?;
                let ids: Vec<i64> =
                    serde_json::from_value(data.get("ids").cloned().ok_or("missing field: ids")?)
                        .map_err(|e| format!("ids 应为数字数组: {e}"))?;
                if ids.is_empty() {
                    return Err("ids 不能为空".to_string());
                }
                let issues = store.bulk_archive_cancelled(&ids, &actor)?;
                Ok(Some(serde_json::json!({
                    "archived": issues.len(),
                    "issues": issues,
                })))
            }
            // B35「需要人的事」聚合面（注意力经济：四组一次返回——
            // escalated 待裁决 / fused 冻结 / plan_failed 拆解失败 /
            // shadow 影子待 promote；避免前端四处信号连发拉全量列表）。
            "board.needs_human.list" => {
                let handoffs = scan_live_handoffs();
                Ok(Some(aggregate_needs_human(&store, &handoffs)?))
            }
            // autopilot（定时派活）---
            "autopilot.list" => {
                let autopilots = store.list_autopilots()?;
                Ok(Some(serde_json::json!({ "autopilots": autopilots })))
            }
            "autopilot.create" => {
                let data = data.ok_or("missing data")?;
                let cron_expr = get_str(&data, "cron")?;
                let enabled = data
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                // 语法 + 语义双闸：解析通过但永不匹配的
                // 表达式在写 store 前拒绝，不留「已创建」的静默僵尸规则。
                validate_autopilot_schedule(&cron_expr, enabled)?;
                // B34（D3 拍板）：新建规则默认影子模式（首轮触发只建单
                // 留痕不派发）；显式传 mode:"live" 才直上全真，非法值
                // loud 拒绝（校验单点在 models::validate_autopilot_mode，
                // 与 CLI/store 同源）。
                let mode = match data.get("mode").and_then(|v| v.as_str()) {
                    None | Some("") => nemesis_board::models::AUTOPILOT_MODE_SHADOW.to_string(),
                    Some(m) => nemesis_board::models::validate_autopilot_mode(m)?,
                };
                let ap = store.create_autopilot(&nemesis_board::NewAutopilot {
                    name: get_str(&data, "name")?.to_string(),
                    title: get_str(&data, "title")?.to_string(),
                    cron: cron_expr.to_string(),
                    description: get_opt_str(&data, "description").unwrap_or_default(),
                    priority: data
                        .get("priority")
                        .and_then(|v| v.as_i64())
                        .map(|p| p as i32)
                        .unwrap_or(nemesis_board::models::priority::MEDIUM),
                    project_id: data.get("project_id").and_then(|v| v.as_i64()),
                    target: get_opt_str(&data, "target").unwrap_or_default(),
                    enabled,
                    // 全自动流转：建单后自动 planner 拆解（默认 false，
                    // 存量前端不传时行为不变）。
                    auto_plan: data
                        .get("auto_plan")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false),
                    // F-：验收标准（触发建单透传；空 = 不填）。
                    acceptance_criteria: get_opt_str(&data, "acceptance_criteria")
                        .filter(|s| !s.trim().is_empty()),
                    // 父会话绑定（空/空白 = 不绑定）。
                    session_key: get_opt_str(&data, "session_key")
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty()),
                    // B34 触发模式（D3：缺省影子）。
                    mode,
                })?;
                // cron 已注入 → 即时登记并回填 cron_job_id；未注入（单测/
                // 极简构建）→ 启动同步兜底。
                arm_autopilot_job(ctx, &store, &ap)?;
                let ap = store.get_autopilot(ap.id)?;
                Ok(Some(
                    serde_json::json!({ "created": true, "autopilot": ap }),
                ))
            }
            "autopilot.update" => {
                let data = data.ok_or("missing data")?;
                reject_nested_patch(&data)?;
                let id = data
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: id")?;
                // 语法 + 语义双闸：按「合并后的有效
                // cron + 有效 enabled」在写 store 前校验——改 cron 成死
                // 表达式、或把存量死规则 enable，都在此拒绝且 store 不动。
                let old = store.get_autopilot(id)?;
                let eff_cron = get_opt_str(&data, "cron").unwrap_or_else(|| old.cron.clone());
                let eff_enabled = data
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(old.enabled);
                validate_autopilot_schedule(&eff_cron, eff_enabled)?;
                let ap = store.update_autopilot(
                    id,
                    &nemesis_board::AutopilotPatch {
                        name: get_opt_str(&data, "name"),
                        cron: get_opt_str(&data, "cron"),
                        title: get_opt_str(&data, "title"),
                        description: get_opt_str(&data, "description"),
                        priority: data
                            .get("priority")
                            .and_then(|v| v.as_i64())
                            .map(|p| p as i32),
                        project_id: data.get("project_id").and_then(|v| v.as_i64()),
                        target: get_opt_str(&data, "target"),
                        enabled: data.get("enabled").and_then(|v| v.as_bool()),
                        auto_plan: data.get("auto_plan").and_then(|v| v.as_bool()),
                        // F-：验收标准（Some("") = 清空；缺省 = 不改）。
                        acceptance_criteria: get_opt_str(&data, "acceptance_criteria"),
                        // 父会话绑定（Some("") = 解除绑定；缺省 = 不改）。
                        session_key: get_opt_str(&data, "session_key")
                            .map(|s| s.trim().to_string()),
                        // B34：mode 切换（影子⇄全真双向；缺省 = 不改；
                        // 非法值 loud 拒绝）。
                        mode: data
                            .get("mode")
                            .and_then(|v| v.as_str())
                            .map(nemesis_board::models::validate_autopilot_mode)
                            .transpose()?,
                    },
                )?;
                arm_autopilot_job(ctx, &store, &ap)?;
                let ap = store.get_autopilot(ap.id)?;
                Ok(Some(
                    serde_json::json!({ "updated": true, "autopilot": ap }),
                ))
            }
            "autopilot.remove" => {
                let data = data.ok_or("missing data")?;
                let id = data
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: id")?;
                let ap = store.get_autopilot(id)?;
                disarm_autopilot_job(ctx, &ap);
                let removed = store.remove_autopilot(id)?;
                Ok(Some(serde_json::json!({ "removed": removed, "id": id })))
            }
            // 手动触发一次（到点自动触发走 gateway on_job → 同一
            // fire_autopilot 核心）。
            "autopilot.run" => {
                // F-：急停中拒绝手动触发（target 非空的规则会发车）。
                refuse_dispatch_when_estopped(ctx)?;
                let data = data.ok_or("missing data")?;
                let id = data
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: id")?;
                let ap = store.get_autopilot(id)?;
                let out = {
                    #[cfg(feature = "cluster")]
                    {
                        // WSAPI 有现成 moderator/事件上下文——直接共享
                        // AppState 的动态解析槽（槽空 = agent 未运行，
                        // auto_plan 走诚实降级；agent 重启后自动跟随，
                        // BUG-20261108-012：此前本地 OnceLock 快照一次，
                        // 重启后打死旧 loop）。
                        let auto_plan_ctx = AutoPlanContext {
                            moderator_slot: ctx.state.agent_loop.clone(),
                            home: ctx
                                .home
                                .clone()
                                .map(std::path::PathBuf::from)
                                .unwrap_or_default(),
                            hub: Some(ctx.state.event_hub.clone()),
                            cluster: ctx.state.cluster.clone(),
                        };
                        fire_autopilot(
                            &store,
                            ctx.state.cluster.as_ref(),
                            &ap,
                            &actor,
                            Some(&auto_plan_ctx),
                        )?
                    }
                    #[cfg(not(feature = "cluster"))]
                    {
                        fire_autopilot(&store, &ap, &actor)?
                    }
                };
                Ok(Some(out))
            }
            // run 历史：origin=autopilot 的 issue 列表（最新在前）。
            "autopilot.runs" => {
                let data = data.ok_or("missing data")?;
                let id = data
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: id")?;
                let limit = data
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(20)
                    .clamp(1, 100) as usize;
                let issues = store.list_issues_by_origin("autopilot", &id.to_string(), limit)?;
                Ok(Some(serde_json::json!({ "issues": issues })))
            }
            // comment / activity ---
            "comment.add" => {
                let data = data.ok_or("missing data")?;
                let issue_id = data
                    .get("issue_id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: issue_id")?;
                let content = get_str(&data, "content")?;
                let comment = store.add_comment(NewComment {
                    issue_id,
                    author: actor,
                    content: content.clone(),
                    parent_id: data.get("parent_id").and_then(|v| v.as_i64()),
                    ctype: CommentType::Comment,
                })?;
                // （终判 👍/👎，计划 §8）：人类评论过终判闸——命中则按
                // Done 子树完成派发入 worker 画像 + 系统回执。失败只
                // warn：反馈是增值动作，不阻塞评论主链路。
                if let Err(e) = nemesis_board::process_final_verdict_comment(
                    store.as_ref(),
                    issue_id,
                    comment.id,
                    &comment.author.kind,
                    &content,
                ) {
                    tracing::warn!("[Board] 终判处理失败（issue {issue_id}）：{e}");
                }
                // （集群无人值守硬化）：审批回执回源——
                // `/approve|/deny <短号>` 评论转发远程审批（短号在注册表才
                // 动作；失败留痕为系统评论）。异步旁路，不阻塞本响应。
                #[cfg(feature = "cluster")]
                if crate::remote_approval::looks_like_approval_reply(&content)
                    && let Some(cluster) = ctx.state.cluster.as_ref()
                {
                    let store = store.clone();
                    let cluster = cluster.clone();
                    tokio::spawn(async move {
                        crate::remote_approval::try_remote_approval_reply(
                            &store, &cluster, issue_id, &content,
                        )
                        .await;
                    });
                }
                Ok(Some(
                    serde_json::json!({ "added": true, "comment": comment }),
                ))
            }
            "comment.list" => {
                let data = data.ok_or("missing data")?;
                let issue_id = data
                    .get("issue_id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: issue_id")?;
                let comments = store.list_comments(issue_id)?;
                Ok(Some(serde_json::json!({ "comments": comments })))
            }
            "activity.list" => {
                let data = data.ok_or("missing data")?;
                let issue_id = data
                    .get("issue_id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: issue_id")?;
                let activity = store.list_activity(issue_id)?;
                Ok(Some(serde_json::json!({ "activity": activity })))
            }
            // subscriber ---
            "subscriber.add" => {
                let data = data.ok_or("missing data")?;
                let issue_id = data
                    .get("issue_id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: issue_id")?;
                store.subscribe(issue_id, &actor, "manual")?;
                Ok(Some(
                    serde_json::json!({ "subscribed": true, "issue_id": issue_id }),
                ))
            }
            "subscriber.remove" => {
                let data = data.ok_or("missing data")?;
                let issue_id = data
                    .get("issue_id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: issue_id")?;
                store.unsubscribe(issue_id, &actor)?;
                Ok(Some(
                    serde_json::json!({ "unsubscribed": true, "issue_id": issue_id }),
                ))
            }
            "subscriber.list" => {
                let data = data.ok_or("missing data")?;
                let issue_id = data
                    .get("issue_id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: issue_id")?;
                let subscribers = store.list_subscribers(issue_id)?;
                Ok(Some(serde_json::json!({ "subscribers": subscribers })))
            }
            // project / attachment / stats ---
            "project.list" => {
                let projects = store.list_projects()?;
                Ok(Some(serde_json::json!({ "projects": projects })))
            }
            // 项目进度聚合（goal /+）：带 project_id = 单项目（含逐单
            // 环节数据）；不带 = 全项目摘要（前端列表页一次拉取）。
            "project.progress" => {
                let project_id = data
                    .as_ref()
                    .and_then(|d| d.get("project_id"))
                    .and_then(|v| v.as_i64());
                match project_id {
                    Some(pid) => Ok(Some(project_progress(&store, pid)?)),
                    None => Ok(Some(project_progress_all(&store)?)),
                }
            }
            // D（goal）项目级恢复发车：入口项目上一键重试全部可派未派子单
            // （派发顺序由 AI/依赖闸自行决定）。estop 中拒绝（护栏）。
            "project.resume" => {
                let data = data.ok_or("missing data")?;
                let project_id = data
                    .get("project_id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: project_id")?;
                let dry_run = data
                    .get("dry_run")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                // 护栏三不变：estop 急停中拒绝恢复发车。
                if ctx
                    .state
                    .estop
                    .as_ref()
                    .map(|e| e.is_engaged())
                    .unwrap_or(false)
                {
                    return Err("⛔ 急停（E-STOP）生效中：恢复发车被拒绝（先释放急停）".to_string());
                }
                // as_ref 借用而非 clone：feature 关时字段退化为 Option<>（Copy），
                // `.clone()` 会撞 clippy clone_on_copy（裁剪构建 clippy -p 现实告警）；
                // 两个 cfg 臂的 project_resume 形参（&Arc<Cluster> / &）都恰好收借用。
                let cluster = ctx
                    .state
                    .cluster
                    .as_ref()
                    .ok_or("集群未运行，无法恢复派发")?;
                let board_cfg = live_board_config();
                let out = project_resume(
                    &store,
                    cluster,
                    board_cfg.as_ref(),
                    project_id,
                    dry_run,
                    &actor,
                )
                .await?;
                Ok(Some(out))
            }
            "project.create" => {
                let data = data.ok_or("missing data")?;
                // 全自动流转 /：可选验收标准 + 自动启动。保守默认
                // （auto_start=false）——存量前端建项目行为不变。
                let acceptance_criteria = get_opt_str(&data, "acceptance_criteria");
                let auto_start = data
                    .get("auto_start")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                // 看板项目档案 goal /-：可选目录入参（None = 自动分配
                // `<workspace>/board-projects/<安全化名>/`）。解析/安全化/
                // 重叠拒绝/四件套脚手架都在 `archive::resolve_project_directory`
                // + `ensure_scaffold`（绑定不可变，此后无修改入口）。
                let workspace = require_workspace(ctx)?;
                let name = get_str(&data, "name")?;
                // 同名预检（用户实测）：项目名有 UNIQUE 约束，
                // 重名此前透传裸 `UNIQUE constraint failed: project.name`
                // 用户看不懂也无处查。中文提示 + 现状指引；UNIQUE 约束
                // 仍在兜底并发窗口。
                if store.list_projects()?.iter().any(|p| p.name == name) {
                    return Err(format!(
                        "已存在同名项目「{name}」：项目名需唯一。请在项目列表先处理旧项目（归档或改名）再重试，或换一个项目名。"
                    ));
                }
                let requested_dir = get_opt_str(&data, "directory");
                // 人工触点体系：可选无人值守开关（None=未表态，继承
                // 全局 attendance.mode；Some(bool)=显式覆盖）。缺键即
                // None——存量前端建项目行为不变。
                let unattended = data.get("unattended").and_then(|v| v.as_bool());
                let existing_dirs: Vec<std::path::PathBuf> = store
                    .list_projects()?
                    .into_iter()
                    .filter_map(|p| p.directory.map(std::path::PathBuf::from))
                    .collect();
                let dir_path = nemesis_board::resolve_project_directory(
                    requested_dir.as_deref(),
                    std::path::Path::new(workspace),
                    &name,
                    &existing_dirs,
                )?;
                nemesis_board::ensure_scaffold(
                    &dir_path,
                    // project_id 尚未生成，manifest 在创建后由
                    // sync_project_manifest 回填真实 id。
                    0, &name, "active",
                )?;
                let project = store.create_project(
                    &name,
                    &get_opt_str(&data, "description").unwrap_or_default(),
                    None,
                    &get_opt_str(&data, "icon").unwrap_or_default(),
                    acceptance_criteria.as_deref().unwrap_or(""),
                    Some(&dir_path.to_string_lossy()),
                    unattended,
                )?;
                // manifest 回填真实 project_id（脚手架先建，id 后生成）。
                nemesis_board::sync_project_manifest(&store, project.id);
                let mut out = serde_json::json!({
                    "created": true,
                    "project": project,
                    "directory": dir_path.to_string_lossy(),
                });
                // 件①：建项目警示——项目层永远没有交付文本（收口评审
                // 的锚点核验对象是汇总摘要），AC 写 `[CHECK] re:` 锚点在
                // 项目层不可自动核验；应下放到具体任务单。
                if nemesis_board::has_content_regex_anchor(
                    acceptance_criteria.as_deref().unwrap_or(""),
                ) {
                    out["warning"] = serde_json::json!(
                        "项目验收标准含 [CHECK] re: 锚点：项目层没有交付文本可核验（项目收口时该锚点必然失败）——请把锚点要求写到具体任务单的验收标准里，而不是项目上。"
                    );
                }
                if auto_start {
                    // F-：自动开工链终点是拆解发车（plan 链 波），
                    // 急停中拒绝整段。项目照建已是事实——响应体照常返回，
                    // auto_start 字段注明拒绝（与非 cluster 降级路径同形），
                    // 不用 `?` 把已建项目信息吞成裸 Err。
                    if let Err(e) = refuse_dispatch_when_estopped(ctx) {
                        out["auto_start"] = serde_json::json!({ "error": e });
                        return Ok(Some(out));
                    }
                    // 自动开工链依赖集群派发（plan 链→节点执行），cluster
                    // feature 编译期裁掉时诚实降级：项目照建（已是事实），
                    // 注明未拆解，不报错不回滚——与 agent 未运行时的降级
                    // 语义一致。
                    #[cfg(feature = "cluster")]
                    {
                        let issue = spawn_project_auto_start(
                            &store,
                            ctx,
                            &actor,
                            &project.name,
                            &get_opt_str(&data, "description").unwrap_or_default(),
                            acceptance_criteria,
                            project.id,
                        )?;
                        out["auto_start"] = serde_json::json!({
                            "issue_id": issue.id,
                            "issue_number": issue.number,
                        });
                    }
                    #[cfg(not(feature = "cluster"))]
                    {
                        let _ = acceptance_criteria; // no-cluster 路径不消费
                        out["auto_start"] = serde_json::json!({
                            "skipped": "cluster 编译期裁剪未启用，无法自动拆解派发；项目已创建，可手动拆解或重新启用 cluster"
                        });
                    }
                }
                Ok(Some(out))
            }
            // 项目字段级更新：归档走 status="archived"（软删除）。
            "project.update" => {
                let data = data.ok_or("missing data")?;
                reject_nested_patch(&data)?;
                let id = data
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: id")?;
                let patch = ProjectPatch {
                    name: get_opt_str(&data, "name"),
                    description: get_opt_str(&data, "description"),
                    status: get_opt_str(&data, "status"),
                    icon: get_opt_str(&data, "icon"),
                    acceptance_criteria: get_opt_str(&data, "acceptance_criteria"),
                };
                let project = store.update_project(id, &patch)?;
                // C 里程碑 5（看板项目档案 goal）：状态/名称投影回
                // project.json（失败静默——投影可重建，不阻塞管理操作）。
                nemesis_board::archive_writer::sync_project_manifest(&store, id);
                // （看板项目档案）：人工收口 → completed 落定即触发
                // AI 收口总结（异步；hook 未注册/生成失败均不阻塞管理操作）。
                if patch.status.as_deref() == Some("completed")
                    && project.status == "completed"
                    && let Some(hook) = PROJECT_SUMMARY_HOOK.get()
                {
                    hook(id);
                }
                Ok(Some(
                    serde_json::json!({ "updated": true, "project": project }),
                ))
            }
            // 看板项目档案：打开档案目录。只放行 board 项目注册表
            // （board.db Project.directory——绑定不可变、无修改入口）已知路径，
            // 前端不可传任意路径；系统文件管理器打开（复用 projects 模块的
            // open_in_file_manager：CREATE_NO_WINDOW spawn 不等待）。
            "project.open_dir" => {
                let data = data.ok_or("missing data")?;
                let project_id = data
                    .get("project_id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: project_id")?;
                let project = store.get_project(project_id)?;
                let dir = project.directory.as_deref().ok_or("项目未绑定档案目录")?;
                let path = std::path::PathBuf::from(dir);
                if !path.is_dir() {
                    return Err(format!("档案目录不存在: {}", path.display()));
                }
                crate::handlers::projects::open_in_file_manager(&path)?;
                Ok(Some(
                    serde_json::json!({ "opened": true, "directory": dir }),
                ))
            }
            // 附件上传：base64 内容 → workspace/board/files/ 存文件
            // + 元数据入表。storage_path 记 workspace 相对路径（可移植）。
            "attachment.add" => {
                let data = data.ok_or("missing data")?;
                let issue_id = data
                    .get("issue_id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: issue_id")?;
                let filename = sanitize_filename(&get_str(&data, "filename")?)?;
                let content_b64 = get_str(&data, "content")?;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(content_b64.trim())
                    .map_err(|e| format!("content 不是合法 base64: {e}"))?;
                if bytes.len() > MAX_ATTACHMENT_BYTES {
                    return Err(format!(
                        "附件过大（{} 字节，上限 {} 字节）",
                        bytes.len(),
                        MAX_ATTACHMENT_BYTES
                    ));
                }
                // 先校验 issue 存在（比 FK 报错信息更友好），再落文件。
                store.get_issue(issue_id)?;
                let workspace = require_workspace(ctx)?;
                let files_dir = std::path::Path::new(workspace)
                    .join("board")
                    .join("files")
                    .join(format!("issue_{issue_id}"));
                std::fs::create_dir_all(&files_dir)
                    .map_err(|e| format!("创建附件目录失败: {e}"))?;
                // 毫秒时间戳前缀防同名覆盖。
                let stored_name = format!("{}_{}", chrono::Utc::now().timestamp_millis(), filename);
                std::fs::write(files_dir.join(&stored_name), &bytes)
                    .map_err(|e| format!("写入附件文件失败: {e}"))?;
                let rel_path = format!("board/files/issue_{issue_id}/{stored_name}");
                let attachment =
                    store.add_attachment(issue_id, &filename, &rel_path, bytes.len() as i64)?;
                Ok(Some(
                    serde_json::json!({ "added": true, "attachment": attachment }),
                ))
            }
            // 附件下载：读文件回 base64（MVP 经 WS 传小文件；大文件
            // 走 HTTP 静态路由留 评估）。
            "attachment.get" => {
                let data = data.ok_or("missing data")?;
                let id = data
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: id")?;
                let attachment = store.get_attachment(id)?;
                let workspace = require_workspace(ctx)?;
                let bytes =
                    std::fs::read(std::path::Path::new(workspace).join(&attachment.storage_path))
                        .map_err(|e| format!("读取附件文件失败: {e}"))?;
                let content = base64::engine::general_purpose::STANDARD.encode(&bytes);
                Ok(Some(
                    serde_json::json!({ "attachment": attachment, "content": content }),
                ))
            }
            // 收件箱：站内通知列表（store 事件钩子产生；经通道的
            // 站外投递留）。MVP 单管理员语义：admin 通知全员可见。
            "inbox.list" => {
                let data = data.unwrap_or(serde_json::Value::Null);
                let unread_only = data
                    .get("unread_only")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let limit = data
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(100)
                    .clamp(1, 500) as usize;
                let notifications = store.list_notifications("admin", None, unread_only, limit)?;
                let unread = store.unread_notification_count("admin", None)?;
                Ok(Some(
                    serde_json::json!({ "notifications": notifications, "unread": unread }),
                ))
            }
            "inbox.mark_read" => {
                let data = data.ok_or("missing data")?;
                let marked: i64 = if data.get("all").and_then(|v| v.as_bool()).unwrap_or(false) {
                    store.mark_all_notifications_read("admin", None)? as i64
                } else {
                    let id = data
                        .get("id")
                        .and_then(|v| v.as_i64())
                        .ok_or("missing field: id（或传 all=true 全部已读）")?;
                    if store.mark_notification_read(id)? {
                        1
                    } else {
                        0
                    }
                };
                let unread = store.unread_notification_count("admin", None)?;
                Ok(Some(
                    serde_json::json!({ "marked": marked, "unread": unread }),
                ))
            }
            "attachment.list" => {
                let data = data.ok_or("missing data")?;
                let issue_id = data
                    .get("issue_id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: issue_id")?;
                let attachments = store.list_attachments(issue_id)?;
                Ok(Some(serde_json::json!({ "attachments": attachments })))
            }
            // channel（Swarm 批次 E：讨论频道）---
            "channel.list" => {
                let channels = store.list_channels()?;
                Ok(Some(serde_json::json!({ "channels": channels })))
            }
            "channel.messages" => {
                let data = data.ok_or("missing data")?;
                let channel_id = data
                    .get("channel_id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: channel_id")?;
                let after_id = data.get("after_id").and_then(|v| v.as_i64()).unwrap_or(0);
                let limit = data
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(200)
                    .clamp(1, 500) as i64;
                let messages = store.list_channel_messages(channel_id, after_id, limit)?;
                Ok(Some(serde_json::json!({ "messages": messages })))
            }
            "channel.post" => {
                let data = data.ok_or("missing data")?;
                // 发言走 master 讨论管线（幂等/额度三闸/裁决投递与 worker
                // 上行同源）；桥未装配 = 讨论总线不可用，诚实拒绝。
                let ingress = ctx
                    .state
                    .board
                    .as_ref()
                    .and_then(|svc| svc.discussion())
                    .ok_or("讨论总线未装配（需要 board+cluster feature 且 gateway 运行中）")?;
                let channel_id = data
                    .get("channel_id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: channel_id")?;
                let content = get_str(&data, "content")?;
                if content.trim().is_empty() {
                    return Err("content must not be empty".to_string());
                }
                let reply_to = data.get("reply_to").and_then(|v| v.as_i64());
                let kind_tag = get_opt_str(&data, "kind_tag").unwrap_or_else(|| "text".to_string());
                let client_msg_id = format!("dash-{}", uuid::Uuid::new_v4());
                let posted = ingress.post(
                    &actor,
                    nemesis_board::models::thread_kind::CHANNEL,
                    channel_id,
                    &client_msg_id,
                    &content,
                    reply_to,
                    &kind_tag,
                )?;
                Ok(Some(serde_json::json!({ "posted": posted })))
            }
            "stats" => {
                let counts = store.count_by_status()?;
                let map: serde_json::Map<String, serde_json::Value> = counts
                    .into_iter()
                    .map(|(st, n)| (st.as_str().to_string(), serde_json::json!(n)))
                    .collect();
                Ok(Some(serde_json::json!({ "by_status": map })))
            }
            // audit（全自动流转 /：决策流视图）---
            "audit.list" => {
                let data = data.unwrap_or(serde_json::json!({}));
                let limit = data
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(50)
                    .clamp(1, 500) as u32;
                // F-：offset 翻页——刷屏类记录挤占 limit 窗口时仍可翻到最新。
                let offset = data.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let action = data
                    .get("action")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.trim().is_empty());
                let rows = store.list_recent_activity_paged(limit, offset, action)?;
                Ok(Some(serde_json::json!({ "decisions": rows })))
            }
            "audit.rollback" => {
                let data = data.ok_or("missing data")?;
                let activity_id = data
                    .get("activity_id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: activity_id")?;
                let issue = store.rollback_decision(activity_id)?;
                tracing::info!(
                    "[Board] audit.rollback activity={activity_id} issue={} → in_review",
                    issue.number
                );
                Ok(Some(
                    serde_json::json!({ "rolled_back": true, "issue": issue }),
                ))
            }
            // S-：合并停车人工重试（按 issue 扫档案树未合并 placement 重走合并）。
            "audit.retry_merge" => {
                let data = data.ok_or("missing data")?;
                let id = data
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: id")?;
                let issue = store.get_issue(id)?;
                let hook = RETRY_MERGE_HOOK
                    .get()
                    .ok_or("合并重试钩子未安装（gateway 未装配档案合并依赖）")?;
                let result = hook(&issue)?;
                tracing::info!(
                    "[Board] audit.retry_merge issue={} → {}",
                    issue.number,
                    result
                );
                Ok(Some(result))
            }
            // （机制加固）：评审重触发——卡 in_review 的单子
            // 人工一键重入验收评审。number（NB-N）优先、id 兜底；非
            // in_review 诚实拒绝（评审链内有竞态守卫，这里是入口闸）。
            "review.retrigger" => {
                let data = data.ok_or("missing data")?;
                // 项目收口复评臂（project_escalate_human
                // 死路根修）：`{"project_id": N}` 直接重触项目收口评审。
                // 前置闸与评审侧同源：项目存在、未终结、全顶层父单 done。
                if let Some(project_id) = data.get("project_id").and_then(|v| v.as_i64()) {
                    let project = store.get_project(project_id)?;
                    if matches!(project.status.as_str(), "completed" | "archived") {
                        return Err(format!(
                            "项目 {} 已是 {}，无需复评",
                            project.name, project.status
                        ));
                    }
                    let tops: Vec<nemesis_board::Issue> = store
                        .list_issues(&nemesis_board::models::IssueFilter {
                            project_id: Some(project_id),
                            ..Default::default()
                        })?
                        .into_iter()
                        .filter(|i| i.parent_issue_id.is_none())
                        .collect();
                    if tops.is_empty() {
                        return Err(format!("项目 {} 无顶层父单，无法复评", project.name));
                    }
                    if let Some(p) = tops.iter().find(|p| p.status != IssueStatus::Done) {
                        return Err(format!(
                            "项目 {} 存在非 done 顶层父单（{}={}），先收口父单再复评项目",
                            project.name,
                            p.number,
                            p.status.as_str()
                        ));
                    }
                    let hook = PROJECT_REVIEW_RETRIGGER_HOOK.get().ok_or(
                        "项目收口复评钩子未安装（gateway 未装配 board 评审依赖或非 master 节点）",
                    )?;
                    tracing::info!(
                        "[Board] review.retrigger project={} by {}/{}",
                        project.name,
                        actor.kind,
                        actor.id
                    );
                    // 留痕：项目无 comment 载体（惯例），系统注记落全部
                    // 顶层父单；消费掉的停车档案照常归档。
                    for p in &tops {
                        let _ = store.add_comment(NewComment {
                            issue_id: p.id,
                            author: actor.clone(),
                            content: "🔁 人工触发项目收口复评（review.retrigger project_id）"
                                .to_string(),
                            parent_id: None,
                            ctype: CommentType::System,
                        });
                        archive_handoff_best_effort(&p.number);
                    }
                    hook(project_id);
                    return Ok(Some(serde_json::json!({
                        "retriggered": true,
                        "project_id": project_id,
                        "project": project.name,
                    })));
                }
                let issue = if let Ok(number) = get_str(&data, "number") {
                    store.get_issue_by_number(&number)?
                } else if let Some(id) = data.get("id").and_then(|v| v.as_i64()) {
                    store.get_issue(id)?
                } else {
                    return Err("review.retrigger 需要 number（NB-N）或 id 之一".into());
                };
                if issue.status != IssueStatus::InReview {
                    return Err(format!(
                        "issue {} 当前状态为 {}，仅 in_review 单子可重触发评审",
                        issue.number,
                        issue.status.as_str()
                    ));
                }
                // 打回重开（用户裁决）：最近 auto_decide =
                // parent_escalate_human 的单，retrigger = 人工打回升级——
                // 返工轮数预算重置（board_meta 纪元键），自动验收重新开始。
                // 必须先于 hook 落定：评审侧防抖守卫读活动流（admin 评论
                // 晚于升级才放行）+ try_parent_remediation 读纪元键。
                let escalated = matches!(
                    store.latest_auto_decide(issue.id)?,
                    Some((_, ref d)) if d == "parent_escalate_human"
                );
                if escalated {
                    let epoch = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs() as i64;
                    store.set_meta(
                        &format!("remediation_reset:{}", issue.id),
                        &epoch.to_string(),
                    )?;
                }
                let hook = REVIEW_RETRIGGER_HOOK.get().ok_or(
                    "评审重触发钩子未安装（gateway 未装配 board 评审依赖或非 master 节点）",
                )?;
                // 留痕：人工触发记录进单据（决策流/审计可）——必须在
                // hook 发车之前落库（评审侧循环升级防抖守卫靠「升级后有
                // admin 活动」放行人工复评）。
                tracing::info!(
                    "[Board] review.retrigger issue={} by {}/{} escalated_reopen={escalated}",
                    issue.number,
                    actor.kind,
                    actor.id
                );
                let note = if escalated {
                    "🔁 人工打回升级停车（review.retrigger）——自动验收重新开始，返工预算已重置"
                        .to_string()
                } else {
                    "🔁 人工触发重新验收评审（review.retrigger）".to_string()
                };
                let _ = store.add_comment(NewComment {
                    issue_id: issue.id,
                    author: actor,
                    content: note,
                    parent_id: None,
                    ctype: CommentType::System,
                });
                hook(issue.id);
                // 复评启动 = 「等裁决」现场已被消费：归档停车档案。
                archive_handoff_best_effort(&issue.number);
                Ok(Some(
                    serde_json::json!({ "retriggered": true, "issue": issue.number, "escalated_reopen": escalated }),
                ))
            }
            // U-B（无人值守）：端点不可用封存观测 + 手动放行。
            // status = 状态机快照 + 待续做中断档案清单（不 load 内容）。
            "unavailable.status" => {
                let mut snap = super::board_unavailable::snapshot_json();
                if let Some(ws) = handoff_workspace_root().map(|p| p.as_path()) {
                    let pending: Vec<String> =
                        super::board_unavailable::read_pending_interruptions(ws)
                            .iter()
                            .filter_map(|d| {
                                d.get("issue")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string())
                            })
                            .collect();
                    snap["pending_interruptions"] = serde_json::json!(pending);
                }
                Ok(Some(snap))
            }
            "unavailable.release" => {
                let was = super::board_unavailable::manual_release();
                tracing::info!(
                    "[Board] board.unavailable.release by {}/{} (was_frozen={was})",
                    actor.kind,
                    actor.id
                );
                Ok(Some(
                    serde_json::json!({ "released": true, "was_frozen": was }),
                ))
            }
            // config（全自动流转 /：配置 TAB 读写 board 段旗标）---
            "config.get" => {
                let home = ctx.home.as_deref().ok_or("home 未解析")?;
                let cfg =
                    nemesis_config::load_config(&std::path::Path::new(home).join("config.json"))
                        .map_err(|e| format!("config.json 读取失败：{e}"))?;
                let board = cfg.board.unwrap_or_default();
                serde_json::to_value(&board)
                    .map(Some)
                    .map_err(|e| format!("board 段序列化失败：{e}"))
            }
            "config.set" => {
                let data = data.ok_or("missing data")?;
                let key = get_str(&data, "key")?;
                let value = data.get("value").ok_or("missing field: value")?;
                board_config_set(ctx, &key, value)
            }
            // 任务档案导出（goal：统一整合任务全部记录）---
            "dossier.export" => {
                let data = data.ok_or("missing data")?;
                let store = require_board(ctx)?;
                let workspace = require_workspace(ctx)?;
                let out_root = std::path::Path::new(workspace)
                    .join("logs")
                    .join("dossiers");
                // 用量闭包：DataStore 在（gateway 常态）→ per-task 聚合；
                // 不在 → 全部 None（usage.csv 缺行诚实，README 总量按有账计）。
                let ds = ctx.state.data_store.clone();
                let usage_of = |task_id: &str| {
                    let ds = ds.as_ref()?;
                    ds.aggregate_session_usage_by_task(task_id).ok().map(|agg| {
                        nemesis_board::dossier::DossierUsage {
                            input_tokens: agg.input_tokens,
                            output_tokens: agg.output_tokens,
                        }
                    })
                };
                let outcome = if let Ok(number) = get_str(&data, "number") {
                    nemesis_board::dossier::export_issue_tree(
                        &store,
                        &number,
                        std::path::Path::new(workspace),
                        &out_root,
                        &usage_of,
                    )?
                } else if let Some(pid) = data.get("project_id").and_then(|v| v.as_i64()) {
                    nemesis_board::dossier::export_project(
                        &store,
                        pid,
                        std::path::Path::new(workspace),
                        &out_root,
                        &usage_of,
                    )?
                } else {
                    return Err("dossier.export 需要 number（NB-N）或 project_id 之一".into());
                };
                Ok(Some(serde_json::json!({
                    "root": outcome.root.display().to_string(),
                    "issues": outcome.issue_numbers,
                    "notes": outcome.notes,
                })))
            }
            // 外循环（交付发布）---
            "delivery.publish" => {
                let data = data.ok_or("missing data")?;
                let bridge = crate::handlers::deliveries::delivery_publish_bridge().ok_or(
                    "发布桥未装配（delivery publish bridge 未接线——headless/测试形态不可发布）",
                )?;
                let outcome = bridge.publish(&data).await?;
                Ok(Some(outcome))
            }
            // 人工触点（P1）：看板侧列表（触点卡徽标/过滤片数据源；
            // 页面动作走 workbench.get/respond——列表归看板，页面归工作台）。
            "touchpoint.list" => {
                let data = data.unwrap_or(serde_json::json!({}));
                Ok(Some(touchpoints::list_touchpoints(&store, &data)?))
            }
            _ => Err(format!("unknown command: board.{}", cmd)),
        }
    }
}

/// 全自动流转：board 配置段白名单写入。typed 改字段 → 重序列化
/// round-trip 验证（与 `config.set_field` 同契约：字段没活下来就 loud 拒绝，
/// 不谎报 updated）→ live/盘双路保存。白名单外键拒绝（配置页不作为任意
/// 配置写入口）。保存后热生效：消费点（load_board_flags/plan 自动发车/
/// QuotaLedger 探针/sweep）均每次现读。
fn board_config_set(
    ctx: &RequestContext,
    key: &str,
    value: &serde_json::Value,
) -> Result<Option<serde_json::Value>, String> {
    let home = ctx.home.as_deref().ok_or("home 未解析")?;
    // 锁在 update_config 内——typed 改字段 → round-trip 验证 → 落盘全程
    // 串行（live 缓存优先视图由共享入口承担， 闭包事务收口）。
    super::config::update_config(home, |config| {
        let board = config.board.get_or_insert_with(Default::default);

        fn need_bool(v: &serde_json::Value) -> Result<bool, String> {
            v.as_bool().ok_or_else(|| "需要布尔值".to_string())
        }

        match key {
            "auto_review" => board.auto_review = need_bool(value)?,
            "auto_accept" => board.auto_accept = need_bool(value)?,
            "auto_close_parent" => board.auto_close_parent = need_bool(value)?,
            "unlimited_mode" => {
                let v = need_bool(value)?;
                // 矛盾态硬拒（关闭方向）：autonomy=autonomous
                // 前置要求 unlimited_mode=true——先切回 assisted 才允许关。
                if !v && board.autonomy == nemesis_config::BoardAutonomy::Autonomous {
                    return Err(
                        "unlimited_mode=false 与 autonomy=autonomous 矛盾（autonomous 前置要求 unlimited_mode=true）：先把 autonomy 切回 assisted，再关 unlimited_mode".to_string(),
                    );
                }
                board.unlimited_mode = v;
            }
            // 自治模式总开关（"assisted"|"autonomous"；默认
            // assisted=现状零回归）。矛盾态硬拒（开启方向）：autonomous
            // 前置要求 unlimited_mode=true——先开 unlimited 再切 autonomous
            // （加载期矛盾态走诚实降级，见 nemesis-config
            // post_process_for_compatibility；写入口径从严硬拒）。
            "autonomy" => {
                let next: nemesis_config::BoardAutonomy = serde_json::from_value(value.clone())
                    .map_err(|e| format!("autonomy 需要 \"assisted\"|\"autonomous\": {e}"))?;
                if next == nemesis_config::BoardAutonomy::Autonomous && !board.unlimited_mode {
                    return Err(
                        "autonomy=autonomous 前置要求 unlimited_mode=true（防「监察环自动重派开着、评审 FAIL 却转人工」的矛盾态）：先开 unlimited_mode，或保持 assisted"
                            .to_string(),
                    );
                }
                board.autonomy = next;
            }
            // / 冲突漏斗全局开关（false=human 档：冻结转人工）。
            "conflict_auto_resolve" => board.conflict_auto_resolve = need_bool(value)?,
            // 重派决策量化（灰度默认关；true = 换节点重派叠加
            // worker × 任务类型成功率指纹三档权重）。
            "fingerprint_weighting" => board.fingerprint_weighting = need_bool(value)?,
            // 三档化：off/role/full（默认 role=丢职能保角色）；
            // 旧布尔 false/true 兼容接收（off/full），写盘统一为字符串档位。
            "dispatch_fallback" => {
                board.dispatch_fallback = serde_json::from_value(value.clone()).map_err(|e| {
                    format!("dispatch_fallback 需要 \"off\"|\"role\"|\"full\"（或旧布尔）: {e}")
                })?;
            }
            // （goal）派发准入：单 worker 在途派发上限（非负整数；0=不限）。
            "worker_max_inflight" => {
                board.worker_max_inflight = value
                    .as_i64()
                    .filter(|v| *v >= 0)
                    .ok_or("worker_max_inflight 需要非负整数")?;
            }
            // （B36）职能工具供给裁剪总开关（D5 拍板做成开关，默认
            // false=零行为变化；true = B 端按 CATALOG tool_trim 声明裁面，
            // 下轮任务装配 fresh-read 生效）。
            "worker.tool_trim.enabled" => {
                board.worker.tool_trim.enabled = need_bool(value)?;
            }
            "dispatch_fallback_target" => {
                board.dispatch_fallback_target = if value.is_null() {
                    None
                } else {
                    Some(
                        value
                            .as_str()
                            .ok_or("dispatch_fallback_target 需要字符串或 null")?
                            .trim()
                            .to_string(),
                    )
                };
            }
            "review.selfcheck" => board.review.selfcheck = need_bool(value)?,
            // 交付证据契约总闸：开启后所有单子验收
            // PASS 前置校验交付记录在场，缺失 = FAIL「缺交付证据」。
            "review.require_delivery_evidence" => {
                board.review.require_delivery_evidence = need_bool(value)?;
            }
            // 必检计划闸（SKIP≠PASS）：开启后任务文本反引号
            // 命令 = 必检计划，评审收口对账 delivery.checks——计划内没跑或
            // 失败 = PASS 降级 FAIL。
            "review.require_check_plan" => {
                board.review.require_check_plan = need_bool(value)?;
            }
            // exec 锚点开关 + 超时（默认关=诚实 skipped 注记，
            // 不进结果组；开=锚点命令在项目根执行，exit 0=过）。
            "review.exec_anchors_enabled" => board.review.exec_anchors_enabled = need_bool(value)?,
            "review.exec_anchor_timeout_secs" => {
                board.review.exec_anchor_timeout_secs = value
                    .as_u64()
                    .ok_or("review.exec_anchor_timeout_secs 需要非负整数")?;
            }
            "review.auto_close_project" => board.review.auto_close_project = need_bool(value)?,
            // 必需检查契约（项 B「必需的必须跑」）：whole-map
            // 键——value = 整个 {匹配键: [必需锚点 id 列表]} 对象（匹配键可含
            // 点，per-rule 点后缀键不可行；UI 每次 load→edit→save 全 map）。
            // 空数组规则拒绝（空规则无意义，直接摘除该键）；空 id 拒绝（不做
            // 静默纠偏——round-trip 值比对按原值核，静默归一会假红）。空对象
            // = 契约关闭；空串键 = 全局兜底规则（合法，保留）。
            "review.required_checks" => {
                let rules: std::collections::HashMap<String, Vec<String>> =
                    serde_json::from_value(value.clone()).map_err(|e| {
                        format!("review.required_checks 需要 {{匹配键: [锚点 id]}} 对象: {e}")
                    })?;
                for (k, ids) in &rules {
                    let key_label = if k.is_empty() {
                        "（全局兜底）"
                    } else {
                        k.as_str()
                    };
                    if ids.is_empty() {
                        return Err(format!(
                            "review.required_checks 规则 {key_label} 为空数组：空规则请直接摘除该键"
                        ));
                    }
                    if ids.iter().any(|s| s.trim().is_empty()) {
                        return Err(format!(
                            "review.required_checks 规则 {key_label} 含空锚点 id：请移除空项或填写完整 id"
                        ));
                    }
                }
                board.review.required_checks = rules;
            }
            "plan.auto_confirm" => board.plan.auto_confirm = need_bool(value)?,
            // 任务档案：项目收口自动导出（默认开）。
            "dossier.auto_on_complete" => board.dossier.auto_on_complete = need_bool(value)?,
            "plan.model" => {
                board.plan.model = if value.is_null() {
                    None
                } else {
                    Some(
                        value
                            .as_str()
                            .ok_or("plan.model 需要字符串或 null")?
                            .to_string(),
                    )
                };
            }
            "max_redispatch" => {
                board.max_redispatch =
                    u32::try_from(value.as_u64().ok_or("max_redispatch 需要非负整数")?)
                        .map_err(|_| "max_redispatch 超出 u32 范围")?;
            }
            "review.max_turns" => {
                board.review.max_turns =
                    u32::try_from(value.as_u64().ok_or("review.max_turns 需要非负整数")?)
                        .map_err(|_| "review.max_turns 超出 u32 范围")?;
            }
            // 连续 UNSURE 停车阈值（0=不限；用户：设置页缺旋钮）。
            "review.max_unsure_streak" => {
                board.review.max_unsure_streak = u32::try_from(
                    value
                        .as_u64()
                        .ok_or("review.max_unsure_streak 需要非负整数")?,
                )
                .map_err(|_| "review.max_unsure_streak 超出 u32 范围")?;
            }
            "review.checkers" => {
                board.review.checkers =
                    u32::try_from(value.as_u64().ok_or("review.checkers 需要非负整数")?)
                        .map_err(|_| "review.checkers 超出 u32 范围")?;
                // 消费侧（run_review_panel）同样收敛，这里前置拒绝给出明确文案。
                if board.review.checkers < 1 || board.review.checkers > 5 {
                    return Err("review.checkers 需要在 1..=5 范围内".to_string());
                }
            }
            "budget.max_subissues_per_parent" => {
                board.budget.max_subissues_per_parent = u32::try_from(
                    value
                        .as_u64()
                        .ok_or("budget.max_subissues_per_parent 需要非负整数")?,
                )
                .map_err(|_| "超出 u32 范围")?;
            }
            "budget.max_total_redispatch" => {
                board.budget.max_total_redispatch = u32::try_from(
                    value
                        .as_u64()
                        .ok_or("budget.max_total_redispatch 需要非负整数")?,
                )
                .map_err(|_| "超出 u32 范围")?;
            }
            // 送达失败自动重试熔断上限（0=关=无限重试；
            // unlimited_mode 视为 0；按本单 ⛔ 标记评论终身累计计数）。
            "budget.max_send_retries" => {
                board.budget.max_send_retries = u32::try_from(
                    value
                        .as_u64()
                        .ok_or("budget.max_send_retries 需要非负整数")?,
                )
                .map_err(|_| "超出 u32 范围")?;
            }
            "budget.wall_clock_budget_secs" => {
                board.budget.wall_clock_budget_secs = value
                    .as_u64()
                    .ok_or("budget.wall_clock_budget_secs 需要非负整数")?;
            }
            "budget.max_tokens_per_parent" => {
                board.budget.max_tokens_per_parent = value
                    .as_u64()
                    .ok_or("budget.max_tokens_per_parent 需要非负整数")?;
            }
            "dispatch_timeout_secs" => {
                board.dispatch_timeout_secs =
                    value.as_u64().ok_or("dispatch_timeout_secs 需要非负整数")?;
            }
            "discussion.retention_days" => {
                board.discussion.retention_days =
                    value.as_i64().ok_or("discussion.retention_days 需要整数")?;
            }
            "discussion.max_agent_turns_per_thread" => {
                board.discussion.max_agent_turns_per_thread = u32::try_from(
                    value
                        .as_u64()
                        .ok_or("discussion.max_agent_turns_per_thread 需要非负整数")?,
                )
                .map_err(|_| "超出 u32 范围")?;
            }
            "discussion.hourly_budget_per_node" => {
                board.discussion.hourly_budget_per_node = u32::try_from(
                    value
                        .as_u64()
                        .ok_or("discussion.hourly_budget_per_node 需要非负整数")?,
                )
                .map_err(|_| "超出 u32 范围")?;
            }
            "discussion.rate_limit_per_min" => {
                board.discussion.rate_limit_per_min = u32::try_from(
                    value
                        .as_u64()
                        .ok_or("discussion.rate_limit_per_min 需要非负整数")?,
                )
                .map_err(|_| "超出 u32 范围")?;
            }
            "discussion.review_wait_secs" => {
                board.discussion.review_wait_secs = u32::try_from(
                    value
                        .as_u64()
                        .ok_or("discussion.review_wait_secs 需要非负整数")?,
                )
                .map_err(|_| "超出 u32 范围")?;
            }
            // （职能→评审预设查表）：review.professions.<slug>.<field>
            // （slug 大小写归一后写入 map，与查表侧 to_ascii_lowercase 对齐；
            // slug 语法走 nemesis-prompts 单一校验源）。逐维度 Option 覆盖：
            // 只写显式提供的维度，其余沿用内置/全局。
            k if let Some(rest) = k.strip_prefix("review.professions.") => {
                let (slug, field) = rest.rsplit_once('.').ok_or_else(|| {
                    format!("职能预设键形态为 review.professions.<slug>.<field>（field: max_turns/checkers/selfcheck），收到：{k}")
                })?;
                let slug = slug.trim().to_ascii_lowercase();
                if let Err(e) = nemesis_prompts::professions::meta::validate_slug(&slug) {
                    return Err(format!("职能 slug 非法：{e}"));
                }
                let preset = board.review.professions.entry(slug.clone()).or_default();
                match field {
                    "max_turns" => {
                        // null = 清除该维度覆盖（全空条目顺手删除，防 map 积灰）。
                        if value.is_null() {
                            preset.max_turns = None;
                            drop_if_empty(&mut board.review.professions, &slug);
                        } else {
                            preset.max_turns =
                                Some(
                                    u32::try_from(value.as_u64().ok_or(
                                        "review.professions.<slug>.max_turns 需要非负整数",
                                    )?)
                                    .map_err(|_| "超出 u32 范围")?,
                                );
                        }
                    }
                    "checkers" => {
                        if value.is_null() {
                            preset.checkers = None;
                            drop_if_empty(&mut board.review.professions, &slug);
                        } else {
                            let v = u32::try_from(
                                value
                                    .as_u64()
                                    .ok_or("review.professions.<slug>.checkers 需要非负整数")?,
                            )
                            .map_err(|_| "超出 u32 范围")?;
                            // 消费侧（run_review_panel）同样收敛，这里前置拒绝给出明确文案。
                            if !(1..=5).contains(&v) {
                                return Err(
                                    "review.professions.<slug>.checkers 需要在 1..=5 范围内"
                                        .to_string(),
                                );
                            }
                            preset.checkers = Some(v);
                        }
                    }
                    "selfcheck" => {
                        if value.is_null() {
                            preset.selfcheck = None;
                            drop_if_empty(&mut board.review.professions, &slug);
                        } else {
                            preset.selfcheck = Some(need_bool(value)?);
                        }
                    }
                    other => {
                        return Err(format!(
                            "未知职能预设维度：{other}（允许：max_turns / checkers / selfcheck；null=清除维度）"
                        ));
                    }
                }
            }
            _ => {
                return Err(format!(
                    "未知或不允许的 board 配置键：{key}（允许：auto_review / auto_accept / \
                 auto_close_parent / unlimited_mode / autonomy / conflict_auto_resolve / dispatch_fallback / \
                 dispatch_fallback_target / fingerprint_weighting / \
                 max_redispatch / dispatch_timeout_secs / worker_max_inflight / \
                 worker.tool_trim.enabled / \
                 plan.auto_confirm / plan.model / review.max_turns / \
                 review.max_unsure_streak / review.selfcheck / \
                 review.require_delivery_evidence / \
                 review.require_check_plan / \
                 review.exec_anchors_enabled / review.exec_anchor_timeout_secs / \
                 review.auto_close_project / review.checkers / review.required_checks / \
                 review.professions.<slug>.* / \
                 budget.max_subissues_per_parent / \
                 budget.max_total_redispatch / budget.max_send_retries / budget.wall_clock_budget_secs / \
                 budget.max_tokens_per_parent / discussion.*）"
                ));
            }
        }

        // round-trip 验证：typed 重序列化后字段必须仍在且值一致（防 typed
        // 结构与键清单漂移时谎报 updated——config.set_field 契约同款）。
        let reserialized =
            serde_json::to_value(&*config).map_err(|e| format!("config 重序列化失败：{e}"))?;
        let Some(board_json) = reserialized.get("board") else {
            return Err("board 段 round-trip 丢失（typed 结构漂移）".to_string());
        };
        let leaf = key.rsplit('.').next().unwrap_or(key);
        // review.professions.<slug>.<field> 四段键 → 嵌套四跳定位
        // （slug 与写入侧同样归一小写后再核）。
        let persisted = if let Some(rest) = key.strip_prefix("review.professions.") {
            let mut seg = rest.rsplit('.');
            let field = seg.next();
            let slug = seg.next().map(|s| s.trim().to_ascii_lowercase());
            match (slug, field) {
                (Some(s), Some(f)) => board_json
                    .get("review")
                    .and_then(|r| r.get("professions"))
                    .and_then(|p| p.get(&s))
                    .and_then(|e| e.get(f)),
                _ => None,
            }
        } else if let Some(field) = key.strip_prefix("worker.tool_trim.") {
            // B36 三段键：board.worker.tool_trim.<field> 三跳定位
            // （通用两跳臂只认 parent.leaf 形态，够不到这层）。
            board_json
                .get("worker")
                .and_then(|w| w.get("tool_trim"))
                .and_then(|t| t.get(field))
        } else {
            match leaf {
                "plan" | "discussion" | "backup" | "review" | "budget" => board_json.get(leaf),
                _ => {
                    // plan.x / discussion.x 嵌套键到子对象里核。
                    if let Some((parent, _)) = key.split_once('.') {
                        board_json.get(parent).and_then(|p| p.get(leaf))
                    } else {
                        board_json.get(leaf)
                    }
                }
            }
        };
        match persisted {
            // null 清除语义：条目/维度已不在 = 清除成功（幂等）。
            None if value.is_null() => {}
            None => return Err(format!("配置键 {key} round-trip 丢失（typed 结构漂移）")),
            // 值不一致且布尔/数值维度也不等价（排除 serde 归一化等价形态）才报错。
            Some(actual)
                if actual != value
                    && actual.as_bool() != value.as_bool()
                    && actual.as_u64() != value.as_u64() =>
            {
                return Err(format!(
                    "配置键 {key} round-trip 不一致（值被拒绝或归一化）"
                ));
            }
            _ => {}
        }

        // rebase（clobber 根修）：update_config 的进程级写锁只串行
        // 进程内写者，跨进程写者（CLI model add、手工编辑）不在其列——落盘
        // 前以磁盘最新内容为底、只保留本闭包改过的 board 段；磁盘读失败 =
        // Err 不落盘（fail-closed，绝不拿陈旧快照重建磁盘）。
        let path = super::config::config_path(home);
        *config = rebase_board_onto_disk(config.clone(), &path)?;

        Ok(())
    })?;
    tracing::info!("[Board] board.config.set key={key}");
    Ok(Some(serde_json::json!({ "updated": true, "key": key })))
}

/// 职能预设条目三维度全空时顺手删除（null 清除维度的收尾；防
/// `professions` map 积灰全空条目）。
fn drop_if_empty(
    map: &mut std::collections::HashMap<String, nemesis_config::BoardProfessionReviewPreset>,
    slug: &str,
) {
    if map.get(slug).is_some_and(|p| *p == Default::default()) {
        map.remove(slug);
    }
}

/// rebase（clobber 根修）：以磁盘最新 config.json 为底、只保留
/// `mutated` 的 board 段，返回合并结果。磁盘读失败 = Err（调用方不落盘，
/// fail-closed——绝不拿陈旧快照重建磁盘）。
fn rebase_board_onto_disk(
    mutated: nemesis_config::Config,
    path: &std::path::Path,
) -> Result<nemesis_config::Config, String> {
    let mut disk_fresh = nemesis_config::load_config(path)
        .map_err(|e| format!("config.json 重读失败（rebase 放弃，磁盘未写）：{e}"))?;
    disk_fresh.board = mutated.board;
    Ok(disk_fresh)
}

#[cfg(test)]
mod tests;

// （三期）：父会话绑定专项测试（home 单例隔离 + fire 裁决 +
// 级联暂停 + WSAPI 绑定面 roundtrip）。
#[cfg(test)]
mod l23_tests;

// 人工触点漏斗测试（P1：respond 五动作 / sweep 三拍 / 取数面 /
// workbench 路由壳 / 恢复钩子）。
#[cfg(test)]
mod touchpoint_tests;

// 机械拆分：按域分文件。纯代码搬家，行为零变更；
// 各子模块项经 pub use 保持原 handlers::board:路径不变。
mod cascade;
mod dispatch;
mod plan;
mod views;

// 触点漏斗单独 pub：nemesisbot 侧（P2 resume hook 装配 + gateway sweep
// ticker）与 workbench handler 直达此模块（不复经 board::* 再导出——
// 漏斗面是跨 crate 契约，不是 board handler 的别名面）。
pub mod touchpoints;

pub use cascade::*;
pub use dispatch::*;
pub use plan::*;
pub use views::*;
