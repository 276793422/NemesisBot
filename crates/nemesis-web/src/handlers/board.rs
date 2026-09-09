//! Board handler — managed-agent 看板（W2 P1；P2 加角色门控）。
//!
//! 所有命令经 `AppState.board` 的 [`nemesis_board::BoardService`]（gateway 在
//! `board` feature 开启时注入；未注入时统一报 "board service not available"）。
//! 状态转移 / 指派走 store 的状态机接口（非法转移被拒），普通字段更新走 patch。
//! 操作者身份：dashboard 登录用户 → `Actor::admin(session_id)`。
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
use nemesis_board::assignment::{Actor, AssignmentType};
use nemesis_board::models::{
    CommentType, IssueFilter, IssuePatch, IssueStatus, NewComment, NewIssue, ProjectPatch,
};
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

/// Request author → Actor（dashboard 用户记为 admin）。
fn ctx_actor(ctx: &RequestContext) -> Actor {
    Actor::admin(&ctx.session_id)
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

fn issue_to_view(
    store: &BoardStore,
    issue: &nemesis_board::Issue,
) -> Result<serde_json::Value, String> {
    let comments = store.list_comments(issue.id)?;
    let activity = store.list_activity(issue.id)?;
    let subscribers = store.list_subscribers(issue.id)?;
    let mut v = serde_json::to_value(issue).map_err(|e| format!("serialize issue: {e}"))?;
    if let Some(obj) = v.as_object_mut() {
        obj.insert(
            "comments".to_string(),
            serde_json::to_value(comments).unwrap_or_default(),
        );
        obj.insert(
            "activity".to_string(),
            serde_json::to_value(activity).unwrap_or_default(),
        );
        obj.insert(
            "subscribers".to_string(),
            serde_json::to_value(subscribers).unwrap_or_default(),
        );
    }
    Ok(v)
}

fn build_filter(data: &serde_json::Value) -> Result<IssueFilter, String> {
    let mut filter = IssueFilter {
        query: get_opt_str(data, "query"),
        ..Default::default()
    };
    if let Some(s) = get_opt_str(data, "status") {
        filter.status = Some(IssueStatus::from_str(&s).ok_or_else(|| format!("未知 status: {s}"))?);
    }
    if let Some((at, aid)) = parse_assignee(data)? {
        filter.assignee = Some((at, aid));
    }
    if let Some(p) = data.get("project_id").and_then(|v| v.as_i64()) {
        filter.project_id = Some(p);
    }
    if let Some(p) = data.get("priority").and_then(|v| v.as_i64()) {
        filter.priority = Some(p as i32);
    }
    Ok(filter)
}

fn build_patch(data: &serde_json::Value) -> IssuePatch {
    IssuePatch {
        title: get_opt_str(data, "title"),
        description: get_opt_str(data, "description"),
        priority: data
            .get("priority")
            .and_then(|v| v.as_i64())
            .map(|p| p as i32),
        project_id: data.get("project_id").and_then(|v| v.as_i64()),
        due_date: data.get("due_date").and_then(|v| v.as_i64()),
        position: data.get("position").and_then(|v| v.as_i64()),
        acceptance_criteria: get_opt_str(data, "acceptance_criteria"),
        parent_issue_id: data.get("parent_issue_id").and_then(|v| v.as_i64()),
    }
}

fn build_new_issue(data: &serde_json::Value, actor: Actor) -> Result<NewIssue, String> {
    let mut ni = NewIssue {
        title: get_str(data, "title")?,
        description: get_opt_str(data, "description").unwrap_or_default(),
        priority: data
            .get("priority")
            .and_then(|v| v.as_i64())
            .map(|p| p as i32)
            .unwrap_or(nemesis_board::models::priority::MEDIUM),
        creator: actor,
        acceptance_criteria: get_opt_str(data, "acceptance_criteria"),
        due_date: data.get("due_date").and_then(|v| v.as_i64()),
        parent_issue_id: data.get("parent_issue_id").and_then(|v| v.as_i64()),
        project_id: data.get("project_id").and_then(|v| v.as_i64()),
        ..NewIssue::default()
    };
    if let Some((at, aid)) = parse_assignee(data)? {
        ni.assignee = Some(at);
        ni.assignee_id = Some(aid);
    }
    if let Some(o) = get_opt_str(data, "origin_type") {
        ni.origin = Some(nemesis_board::models::TaskOrigin {
            origin_type: o,
            origin_id: get_opt_str(data, "origin_id").unwrap_or_default(),
        });
    }
    Ok(ni)
}

/// issue → worker 任务提示词（`issue.dispatch` 的 peer_chat content）。
/// worker 端按普通任务 prompt 走自己的 agent（工具/安全层照常生效），
/// 结尾固定要求结果汇报——最终回复是唯一回传渠道（经 callback 写回看板）。
#[cfg(feature = "cluster")]
/// 汇报格式五段真相源在 nemesis-board::report（交付线程首评 / M4 验收
/// agent 同源解析；此处只引用）。
use nemesis_board::report::REPORT_FORMAT_SECTION;

/// 派发提示词（impl-plan §3.3 结构化 PRD 模板）：背景/验收/交付物要求/
/// 汇报格式五段。description 是自由文本（人写）或结构化分段（planner 写），
/// 原文放「背景」段不做二次加工。唯一调用方 dispatch_issue_core（cluster），
/// 同 cfg 门控（REPORT_FORMAT_SECTION 是 cluster-only const）。
#[cfg(feature = "cluster")]
/// `review_feedback`（Swarm M4）：FAIL 重派时验收 agent 的差距意见，渲染
/// 成「上轮验收意见」段——worker 必须知道为什么被打回，否则重派就是掷
/// 骰子（§6.1 检视 2）。首派传 `None`。
/// `experience_section`（Swarm M4.5）：团队过往经验注入段（注入端已渲染），
/// 无匹配传 `None`——不塞空段，保持提示词字节与无经验时一致。
fn build_dispatch_prompt(
    issue: &nemesis_board::Issue,
    assets_section: Option<&str>,
    review_feedback: Option<&str>,
    experience_section: Option<&str>,
) -> String {
    let mut p = format!("# 看板任务 {}\n\n## 标题\n{}\n", issue.number, issue.title);
    p.push_str(&format!(
        "\n## 背景\n{}\n",
        if issue.description.trim().is_empty() {
            "（未提供）"
        } else {
            issue.description.trim()
        }
    ));
    p.push_str(&format!(
        "\n## 验收标准\n{}\n",
        match issue.acceptance_criteria.as_deref().map(str::trim) {
            Some(ac) if !ac.is_empty() => ac,
            _ => "（未提供——完成判据以背景描述为准，无法客观判定时在自检结果中说明）",
        }
    ));
    // Swarm M4（§6.1 检视 2）：重派上下文延续——上轮验收差距原文随包走。
    if let Some(feedback) = review_feedback
        && !feedback.trim().is_empty()
    {
        p.push_str(&format!("\n## 上轮验收意见（本次重派原因，必须针对性整改）\n{}\n", feedback.trim()));
    }
    // Swarm M3（§5.4 层 1）：issue 名下资产随包走（引用+签名 token；
    // 内容对端按需 HTTP 拉）。
    if let Some(section) = assets_section {
        p.push_str(section);
    }
    // Swarm M4.5（§6.5.2）：scope 匹配的团队经验随包走（经验段是参考，
    // 不是验收标准——提示词里写明）。
    if let Some(section) = experience_section {
        p.push_str(section);
    }
    p.push_str(
        "\n## 交付物要求\n\
         - 列出所有改动/新建文件的完整路径\n\
         - 涉及代码产出时给出 branch 名与 commit 列表（如适用）\n",
    );
    p.push_str(&format!(
        "\n## 汇报格式（最终回复必须严格按以下五段组织，标题原样保留）\n{REPORT_FORMAT_SECTION}\n\n\
         最终回复会原样写回看板任务，是唯一回传渠道。\n"
    ));
    p
}

/// `issue.dispatch` 实现（cluster 编译时）：解析派发目标 →
/// [`dispatch_issue_core`]。先做纯本地校验（目标），状态/重复派发闸在
/// core 内、集群缺失最后报——错误信息更有指向性，且校验矩阵不依赖集群
/// 实例（可单测）。
#[cfg(feature = "cluster")]
async fn issue_dispatch(
    store: &Arc<BoardStore>,
    actor: Actor,
    ctx: &RequestContext,
    data: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    let data = data.ok_or("missing data")?;
    let id = data
        .get("id")
        .and_then(|v| v.as_i64())
        .ok_or("missing field: id")?;
    let issue = store.get_issue(id)?;

    // 目标解析：显式 target 优先，否则取 worker 指派；manager_self 由
    // coordinator 本机执行（P4 autopilot），不支持远端派发。
    let target = match get_opt_str(&data, "target") {
        Some(t) if !t.trim().is_empty() => t.to_string(),
        Some(_) => return Err("target 不能为空".to_string()),
        None => match (&issue.assignee, &issue.assignee_id) {
            (Some(AssignmentType::Worker), Some(wid)) => wid.clone(),
            (Some(AssignmentType::ManagerSelf), _) => {
                return Err("manager_self 指派由 coordinator 本机执行，不支持远端派发".to_string());
            }
            _ => return Err("缺少派发目标：提供 target 或先把 issue 指派给 worker".to_string()),
        },
    };

    let cluster = ctx.state.cluster.clone();
    let out = dispatch_issue_core(store, cluster.as_ref(), id, &target, &actor, None)?;
    Ok(Some(out))
}

/// dispatch prompt 的「## 任务资产」段渲染（Swarm M3 §5.4 层 1）。签发
/// 上下文挂 store（gateway 装配 set 一次）——未注入 / issue 无资产逐级
/// 诚实降级为 None。
#[cfg(feature = "cluster")]
fn render_dispatch_assets_section(
    store: &Arc<BoardStore>,
    issue_id: i64,
) -> Result<Option<String>, String> {
    let Some(signing) = store.asset_signing() else {
        return Ok(None);
    };
    let assets = store.assets_for_issue(issue_id)?;
    Ok(nemesis_board::render_assets_section(
        &signing,
        &assets,
        nemesis_board::DEFAULT_TOKEN_TTL_SECS,
    ))
}

/// 派发经验注入的检索（M4.5 §6.5.2）：查库（滤 deprecated）→ scope 对
/// 任务文本（标题+描述+验收标准）与 required_tags 匹配 → top-N。返回
/// （渲染好的注入段, 命中条目 id——发车成功后由调用方计 use_count）。
/// 查库失败诚实降级为无注入（经验是增值段，任何异常都不阻断派发主线）。
#[cfg(feature = "cluster")]
fn render_issue_experience_section(
    store: &Arc<BoardStore>,
    issue: &nemesis_board::Issue,
) -> (Option<String>, Vec<i64>) {
    let entries = match store.list_team_memory(None, false) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("[Board] team_memory 读取失败，派发跳过经验注入：{e}");
            return (None, Vec::new());
        }
    };
    let issue_text = format!(
        "{}\n{}\n{}",
        issue.title,
        issue.description,
        issue.acceptance_criteria.as_deref().unwrap_or("")
    );
    let hits = nemesis_board::match_experiences(
        &entries,
        &issue_text,
        &issue.required_tags,
        nemesis_board::MAX_MATCHED_EXPERIENCES,
    );
    let ids: Vec<i64> = hits.iter().map(|e| e.id).collect();
    (nemesis_board::render_dispatch_experience_section(&hits), ids)
}

/// 派发核心（W2 P4 从 `issue_dispatch` 提取，cluster 编译时）：状态/重复
/// 派发闸 → 登记 task + 派发绑定 → 推进 in_progress → fire-and-forget 发
/// peer_chat RPC。`issue.dispatch`（WSAPI）与 gateway 的 autopilot 定时
/// 触发共用（单一真相源）。`cluster` 传 `None` 时本地闸先行、集群缺失
/// 最后报（校验矩阵不依赖集群实例，可单测）。`review_feedback`（Swarm
/// M4）为 FAIL 重派专用——验收差距渲染进 dispatch prompt「上轮验收意见」
/// 段；常规派发传 `None`。
#[cfg(feature = "cluster")]
pub fn dispatch_issue_core(
    store: &Arc<BoardStore>,
    cluster: Option<&Arc<nemesis_cluster::cluster::Cluster>>,
    issue_id: i64,
    target: &str,
    actor: &Actor,
    review_feedback: Option<&str>,
) -> Result<serde_json::Value, String> {
    let issue = store.get_issue(issue_id)?;

    // 状态闸：blocked/终态不可派发；backlog/todo/in_review 派发即转
    // in_progress（都合法）。
    match issue.status {
        IssueStatus::Backlog
        | IssueStatus::Todo
        | IssueStatus::InProgress
        | IssueStatus::InReview => {}
        other => return Err(format!("issue 处于 {other} 状态，不可派发")),
    }

    // 已有未完结派发 → 拒绝重复派发。
    if store.has_active_dispatch(issue.id)? {
        return Err("该 issue 已有进行中的派发（等 worker 回报或超时后再试）".to_string());
    }

    // 派发 = 给远端 worker 发 peer_chat，集群必需。
    let cluster = cluster.ok_or("集群未运行，无法派发（issue.dispatch 需要集群）")?;

    // M4.5：经验注入段在 prompt 组装前检索（命中计数待发车成功后落）。
    let (experience_section, experience_ids) = render_issue_experience_section(store, &issue);

    let prompt = build_dispatch_prompt(
        &issue,
        render_dispatch_assets_section(store, issue.id)?.as_deref(),
        review_feedback,
        experience_section.as_deref(),
    );

    // 1. 登记本地 task（与 dashboard peer_chat 同契约），拿 task_id。
    //    worker 端会话键 = cluster_rpc:{source}/board:{number}：同 issue 多次
    //    派发收敛到同一 worker 会话（上下文延续）。
    let source_node_id = cluster.node_id().to_string();
    let chat_id = format!("board:{}", issue.number);
    let source_payload = serde_json::json!({
        "node_id": source_node_id,
        "channel": "board",
        "chat_id": chat_id,
    });
    let task_id = cluster.submit_peer_chat(
        target,
        "peer_chat",
        serde_json::json!({ "content": prompt, "_source": source_payload }),
        "board",
        &chat_id,
    )?;

    // M4.5：注入计数只认实际发车的派发（提交失败不计 use_count）。
    if !experience_ids.is_empty()
        && let Err(e) = store.mark_team_memory_used(&experience_ids)
    {
        tracing::warn!("[Board] team_memory use_count 更新失败（不影响派发）：{e}");
    }

    // 2. 登记派发绑定（peer_chat_callback 的写回路由键）+ 审计活动。
    store.insert_dispatch(&task_id, issue.id, target, actor)?;

    // 3. 状态推进：→ in_progress（状态机转移，写 status_change 审计）。
    let issue = if issue.status != IssueStatus::InProgress {
        store.transition_issue(issue.id, IssueStatus::InProgress, actor)?
    } else {
        issue
    };

    // 4. 发 RPC（fire-and-forget）：ACK 后 worker 异步处理，回报走
    //    peer_chat_callback。目标不可达时立刻终结派发 + 系统评论留痕
    //    （callback 不会再来，不留悬挂 dispatched 态）。
    let issue_id = issue.id;
    let rpc_client = cluster.rpc_client_arc().ok_or("RPC client not available")?;
    let request = nemesis_cluster::rpc_types::RPCRequest {
        id: task_id.clone(),
        action: nemesis_cluster::rpc_types::ActionType::Known(
            nemesis_cluster::rpc_types::KnownAction::PeerChat,
        ),
        payload: serde_json::json!({
            "content": prompt,
            "task_id": task_id,
            "_source": source_payload,
        }),
        source: source_node_id,
        target: Some(target.to_string()),
    };
    let store_for_rpc = store.clone();
    let task_id_for_rpc = task_id.clone();
    let target_for_rpc = target.to_string();
    tokio::spawn(async move {
        let timeout = std::time::Duration::from_secs(30);
        match rpc_client
            .call_with_timeout(&target_for_rpc, request, timeout)
            .await
        {
            Ok(_) => {
                tracing::info!("[Board] dispatch RPC ACK received (task_id={task_id_for_rpc})");
            }
            Err(e) => {
                tracing::warn!("[Board] dispatch RPC send failed (task_id={task_id_for_rpc}): {e}");
                let _ = store_for_rpc.finish_dispatch(
                    &task_id_for_rpc,
                    nemesis_board::models::dispatch_state::FAILED,
                );
                let _ = store_for_rpc.add_comment(nemesis_board::models::NewComment {
                    issue_id,
                    author: nemesis_board::Actor::system("board"),
                    content: format!("⛔ 派发失败：RPC 送达失败（{e}）"),
                    parent_id: None,
                    ctype: nemesis_board::CommentType::System,
                });
            }
        }
    });

    Ok(serde_json::json!({
        "dispatched": true,
        "task_id": task_id,
        "issue": issue_to_view(store, &issue)?,
    }))
}

/// `issue.dispatch`（未编译 cluster）：派发即发集群 RPC，无从谈起。
#[cfg(not(feature = "cluster"))]
async fn issue_dispatch(
    _store: &Arc<BoardStore>,
    _actor: Actor,
    _ctx: &RequestContext,
    _data: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    Err("issue.dispatch 需要集群支持（cluster feature 未编译）".to_string())
}

// ---------------------------------------------------------------------------
// Swarm M1：issue.plan 两段式 AI 拆解 + 依赖补派（2026-09-09 swarm-goal §1）
//
// 两段式：`issue.plan {id}`（confirm 缺省/false）异步跑 planner（裸提示词
// detached LLM 调用，无人格/零工具），产出推送 `board.plan_ready` /
// `board.plan_failed`（SSE/WS push），返回 plan_id；`issue.plan {id,
// plan_id, confirm:true}` 取缓存落库（parent/required_*/origin=planner）
// + 批内依赖边（序号→真实 id 映射）+ 依赖闸派发波 + 父单联动。
// ---------------------------------------------------------------------------

/// plan 预览缓存条目（一段产出 → 二段确认的中间态）。
#[cfg(feature = "cluster")]
struct PlanPreview {
    issue_id: i64,
    subs: Vec<nemesis_board::PlannedSubIssue>,
    created_at: std::time::Instant,
}

/// plan 预览缓存（plan_id → 预览）。模块级 static 而非 AppState 字段：
/// AppState 在全库有大量字面构造测试点，加字段是断点级改动；缓存是本
/// handler 私有的进程内运行态，单例语义。插入时惰性清过期条目（无后台
/// 清扫任务）。
#[cfg(feature = "cluster")]
static PLAN_CACHE: std::sync::LazyLock<
    parking_lot::Mutex<std::collections::HashMap<String, PlanPreview>>,
> = std::sync::LazyLock::new(|| parking_lot::Mutex::new(std::collections::HashMap::new()));

/// 预览有效期：确认发车须在 10 分钟内，过期重拆（防陈旧 plan 覆盖用户
/// 在预览期间对父单的手工编辑）。
#[cfg(feature = "cluster")]
const PLAN_TTL: std::time::Duration = std::time::Duration::from_secs(600);

/// planner LLM 编排（一段）：裸提示词 detached 调用（max_turns=1 单轮出
/// JSON）→ `parse_plan` 失败带 [`nemesis_board::build_retry_prompt`] 回灌
/// 重试 ≤2 次（首跑 + 2 次自纠）。纯编排：提示词/解析/校验真相源在
/// nemesis-board::planner。`team_experience`（M4.5）是 meta 级经验注入
/// （调用方检索渲染后传入，空 = 不注入）。
#[cfg(feature = "cluster")]
async fn run_planner(
    agent_loop: &Arc<nemesis_agent::r#loop::AgentLoop>,
    parent: &nemesis_board::Issue,
    team_experience: Vec<String>,
) -> Result<Vec<nemesis_board::PlannedSubIssue>, String> {
    let mut prompt = nemesis_board::build_planner_user_prompt(
        &parent.title,
        &parent.description,
        parent.acceptance_criteria.as_deref(),
        &team_experience,
    );
    let mut last_err = String::new();
    for _ in 0..=2 {
        let raw = agent_loop
            .run_detached(
                &prompt,
                nemesis_agent::r#loop::DetachedOpts {
                    system_prompt: Some(nemesis_board::PLANNER_SYSTEM_PROMPT),
                    no_tools: true,
                    max_turns: 1,
                    label: Some("board-planner"),
                    ..Default::default()
                },
            )
            .await?;
        match nemesis_board::parse_plan(&raw) {
            Ok(subs) => return Ok(subs),
            Err(e) => {
                prompt = nemesis_board::build_retry_prompt(&raw, &e);
                last_err = e.message;
            }
        }
    }
    Err(format!("planner 连续 3 次输出无法解析：{last_err}"))
}

/// `issue.plan` 实现（cluster 编译时）：一段异步拆解 / 二段确认落库+派发波。
#[cfg(feature = "cluster")]
async fn issue_plan(
    store: &Arc<BoardStore>,
    actor: Actor,
    ctx: &RequestContext,
    data: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    let data = data.ok_or("missing data")?;
    let id = data
        .get("id")
        .and_then(|v| v.as_i64())
        .ok_or("missing field: id")?;
    let issue = store.get_issue(id)?;

    if data.get("confirm").and_then(|v| v.as_bool()).unwrap_or(false) {
        // ---- 二段：确认发车（缓存一次性消费）----
        let plan_id = get_str(&data, "plan_id")?.to_string();
        let preview = {
            let mut cache = PLAN_CACHE.lock();
            match cache.remove(plan_id.as_str()) {
                Some(p) if p.issue_id == id && p.created_at.elapsed() <= PLAN_TTL => p,
                Some(p) => {
                    // 与该 issue 不匹配 → 放回（可能是用户在别的 issue
                    // 表单里贴错了 plan_id，不应误伤那个在途预览）。
                    cache.insert(plan_id.clone(), p);
                    return Err("plan 与该 issue 不匹配，请重新拆解".to_string());
                }
                None => {
                    return Err("plan_id 不存在或已被消费（每个 plan 只能确认一次）".to_string());
                }
            }
        };
        let cluster = ctx.state.cluster.clone();
        let out = confirm_plan(store, cluster.as_ref(), &issue, preview.subs, &actor)?;
        Ok(Some(out))
    } else {
        // ---- 一段：异步拆解（重复点按钮 = 重复 LLM 消耗，新 plan_id
        //      覆盖旧预览；结果幂等——confirm 以 plan_id 定位）----
        let agent_loop = ctx
            .state
            .agent_loop
            .read()
            .clone()
            .ok_or("agent 未运行，无法执行 AI 拆解")?;
        // M4.5：planner 的 meta 级经验注入——spawn 前同步检索渲染（发起
        // 点定格，避免长 LLM 调用期间的经验变更串进本次拆解）。meta 注入
        // 不计 use_count（计数只认面向 worker 的派发注入）。
        let team_experience: Vec<String> = {
            let entries = store.list_team_memory(None, false).unwrap_or_default();
            let issue_text = format!(
                "{}\n{}\n{}",
                issue.title,
                issue.description,
                issue.acceptance_criteria.as_deref().unwrap_or("")
            );
            let hits = nemesis_board::match_experiences(
                &entries,
                &issue_text,
                &issue.required_tags,
                nemesis_board::MAX_MATCHED_EXPERIENCES,
            );
            nemesis_board::render_planner_experience_strings(&hits)
        };
        let plan_id = format!("plan-{}", uuid::Uuid::new_v4());
        let hub = ctx.state.event_hub.clone();
        let plan_id_for_task = plan_id.clone();
        tokio::spawn(async move {
            let started = std::time::Instant::now();
            match run_planner(&agent_loop, &issue, team_experience).await {
                Ok(subs) => {
                    let count = subs.len();
                    {
                        let mut cache = PLAN_CACHE.lock();
                        cache.retain(|_, p| p.created_at.elapsed() <= PLAN_TTL);
                        cache.insert(
                            plan_id_for_task.clone(),
                            PlanPreview {
                                issue_id: issue.id,
                                subs: subs.clone(),
                                created_at: std::time::Instant::now(),
                            },
                        );
                    }
                    tracing::info!(
                        "[Board] planner done issue={} subs={count} ({}ms)",
                        issue.id,
                        started.elapsed().as_millis()
                    );
                    hub.publish(
                        "board.plan_ready",
                        serde_json::json!({
                            "plan_id": plan_id_for_task,
                            "issue_id": issue.id,
                            "subs": subs,
                        }),
                    );
                }
                Err(e) => {
                    tracing::warn!("[Board] planner failed issue={}: {e}", issue.id);
                    hub.publish(
                        "board.plan_failed",
                        serde_json::json!({ "issue_id": issue.id, "error": e }),
                    );
                }
            }
        });
        Ok(Some(
            serde_json::json!({ "status": "planning", "plan_id": plan_id }),
        ))
    }
}

/// `issue.plan`（未编译 cluster）：planner 产物的派发语义（required_* 匹配、
/// 依赖补派波）全依赖集群。
#[cfg(not(feature = "cluster"))]
async fn issue_plan(
    _store: &Arc<BoardStore>,
    _actor: Actor,
    _ctx: &RequestContext,
    _data: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    Err("issue.plan 需要集群支持（cluster feature 未编译）".to_string())
}

/// 二段确认：预览 → 落库（parent/required_*/origin=planner）→ 依赖边
/// （批内序号 → 真实 id）→ 依赖闸派发波（无依赖的立即派，其余留 backlog
/// 等补派触发器）→ 父单联动。单张派发失败不回滚整批（落库已成事实），
/// 系统评论留痕继续。
#[cfg(feature = "cluster")]
fn confirm_plan(
    store: &Arc<BoardStore>,
    cluster: Option<&Arc<nemesis_cluster::cluster::Cluster>>,
    parent: &nemesis_board::Issue,
    subs: Vec<nemesis_board::PlannedSubIssue>,
    actor: &Actor,
) -> Result<serde_json::Value, String> {
    // 1) 落库（保持批内顺序；id_by_idx 供依赖映射）。
    let mut id_by_idx: Vec<i64> = Vec::with_capacity(subs.len());
    for sub in &subs {
        let ac = sub.acceptance_criteria.trim();
        let role = sub.required_role.trim();
        let created = store.create_issue(nemesis_board::NewIssue {
            title: sub.title.clone(),
            description: sub.description.clone(),
            acceptance_criteria: (!ac.is_empty()).then(|| ac.to_string()),
            parent_issue_id: Some(parent.id),
            required_role: (!role.is_empty()).then(|| role.to_string()),
            required_tags: sub.required_tags.clone(),
            origin: Some(nemesis_board::TaskOrigin {
                origin_type: "planner".to_string(),
                origin_id: parent.number.clone(),
            }),
            creator: actor.clone(),
            ..nemesis_board::NewIssue::default()
        })?;
        id_by_idx.push(created.id);
    }

    // 2) 依赖边（批内序号 → 真实 id；越界 parse_plan 已挡，防御再钳一次）。
    for (idx, sub) in subs.iter().enumerate() {
        let deps: Vec<i64> = sub
            .depends_on
            .iter()
            .filter(|&&d| d < id_by_idx.len() && d != idx)
            .map(|&d| id_by_idx[d])
            .collect();
        store.set_issue_dependencies(id_by_idx[idx], &deps)?;
    }

    // 3) 派发波：依赖闸放行的立即派，其余留 backlog 等补派触发器。
    let mut dispatched = 0usize;
    let mut deferred: Vec<i64> = Vec::new();
    for &cid in &id_by_idx {
        match dispatch_subissue_auto(store, cluster, cid, actor) {
            Ok(Some(_)) => dispatched += 1,
            Ok(None) => deferred.push(cid),
            Err(e) => {
                tracing::warn!("[Board] 子单 {cid} 自动派发失败：{e}");
                let _ = store.add_comment(nemesis_board::NewComment {
                    issue_id: cid,
                    author: nemesis_board::Actor::system("board"),
                    content: format!("⛔ 自动派发失败：{e}"),
                    parent_id: None,
                    ctype: nemesis_board::CommentType::System,
                });
            }
        }
    }

    // 4) 父单联动（首派 → in_progress 已在 dispatch_subissue_auto 内处理）。
    let _ = sync_parent_status(store, parent.id, actor);

    Ok(serde_json::json!({
        "created": id_by_idx,
        "dispatched": dispatched,
        "deferred": deferred,
        "issue": issue_to_view(store, &store.get_issue(parent.id)?)?,
    }))
}

/// 子单自动派发（依赖闸 + 匹配器选节点；confirm 派发波与补派触发器共用
/// ——单一真相源）。返回：`Ok(Some(out))` 已派出；`Ok(None)` 暂缓（非待派
/// 状态 / 已有在途派发 / 依赖未满足 / 无匹配节点——后者落系统评论留痕）；
/// `Err` 硬错误（集群缺失、派发核心失败）。
#[cfg(feature = "cluster")]
pub fn dispatch_subissue_auto(
    store: &Arc<BoardStore>,
    cluster: Option<&Arc<nemesis_cluster::cluster::Cluster>>,
    issue_id: i64,
    actor: &Actor,
) -> Result<Option<serde_json::Value>, String> {
    let issue = store.get_issue(issue_id)?;

    // 状态闸：只自动派待派态（planner 产物落 backlog；用户预排的 todo
    // 也算）。在途/已收口/阻塞的单不动。
    if !matches!(issue.status, IssueStatus::Backlog | IssueStatus::Todo) {
        return Ok(None);
    }
    // 已有在途派发（人工先派过）→ 不重复。
    if store.has_active_dispatch(issue.id)? {
        return Ok(None);
    }
    // 依赖闸：全部依赖 done 才派；否则留待补派触发器。
    for dep_id in store.dependencies_of(issue.id)? {
        if store.get_issue(dep_id)?.status != IssueStatus::Done {
            return Ok(None);
        }
    }

    // 目标解析：显式 worker 指派 > 匹配器。
    let target = match (&issue.assignee, &issue.assignee_id) {
        (Some(AssignmentType::Worker), Some(wid)) => wid.clone(),
        _ => {
            let cluster = cluster.ok_or("集群未运行，无法自动派发")?;
            match pick_target_by_matcher(store, cluster, &issue) {
                Some(t) => t,
                None => {
                    // 诚实留痕：不悄悄留 backlog。
                    store.add_comment(nemesis_board::NewComment {
                        issue_id: issue.id,
                        author: nemesis_board::Actor::system("board"),
                        content: "⏸ 自动派发暂缓：当前在线节点无可匹配（角色/标签）节点；节点上线或要求调整后随依赖补派自动重试".to_string(),
                        parent_id: None,
                        ctype: nemesis_board::CommentType::System,
                    })?;
                    return Ok(None);
                }
            }
        }
    };

    let out = dispatch_issue_core(store, cluster, issue.id, &target, actor, None)?;

    // 父单联动：首张子单派出 → 父单 backlog/todo → in_progress。
    if let Some(pid) = issue.parent_issue_id
        && let Ok(parent) = store.get_issue(pid)
        && matches!(parent.status, IssueStatus::Backlog | IssueStatus::Todo)
    {
        store.transition_issue(pid, IssueStatus::InProgress, actor)?;
    }
    Ok(Some(out))
}

/// 匹配器选节点：在线节点（不含本机）→ PeerCandidate 适配 →
/// [`nemesis_board::rank_peers`] 取榜首。M1 诚实降级：per-peer tags 未进
/// 节点注册表（announce 带了但 handle_discovered_node 未存），category 是
/// 当前唯一的节点粒度标签，适配为 `tags := [category]`；节点角色词表只有
/// worker/coordinator，planner 给的其他角色词（如 "qa"）转标签语义与
/// category 匹配。M3 统一信封扩节点画像后此处换真数据，matcher 纯函数不动。
#[cfg(feature = "cluster")]
fn pick_target_by_matcher(
    store: &BoardStore,
    cluster: &nemesis_cluster::cluster::Cluster,
    issue: &nemesis_board::Issue,
) -> Option<String> {
    let peers = cluster.get_online_peers_excluding_self();
    if peers.is_empty() {
        return None;
    }
    let candidates: Vec<nemesis_board::PeerCandidate> = peers
        .iter()
        .map(|p| nemesis_board::PeerCandidate {
            id: p.base.id.clone(),
            name: p.base.name.clone(),
            role: match p.base.role {
                nemesis_types::cluster::NodeRole::Coordinator => "coordinator",
                nemesis_types::cluster::NodeRole::Worker => "worker",
            }
            .to_string(),
            tags: {
                let c = p.base.category.trim();
                if c.is_empty() {
                    Vec::new()
                } else {
                    vec![c.to_string()]
                }
            },
            capabilities: p.capabilities.clone(),
        })
        .collect();

    // required_role 归一：角色词表外的词转标签语义（进 required_tags）。
    let (required_role, required_tags) = match issue.required_role.as_deref().map(str::trim) {
        Some(r) if !r.is_empty() && r != "worker" && r != "coordinator" => {
            let mut tags = issue.required_tags.clone();
            tags.push(r.to_string());
            (None, tags)
        }
        Some(r) if !r.is_empty() => (Some(r), issue.required_tags.clone()),
        _ => (None, issue.required_tags.clone()),
    };

    let load = store.count_active_dispatch_by_worker().unwrap_or_default();
    let input = nemesis_board::MatchInput {
        required_role,
        required_tags: &required_tags,
        description: &issue.description,
    };
    nemesis_board::rank_peers(&input, &candidates, &load)
        .into_iter()
        .next()
        .map(|(id, _)| id)
}

/// 父单状态联动：子单全部 done → 父单 in_review；任一子单 cancelled →
/// 父单 in_review + 系统评论标注缺口（人工裁决重开或取消）。backlog/todo
/// 先推 in_progress（状态机不允许跨列）。终态父单与无子单的 issue 不动。
#[cfg(feature = "cluster")]
pub fn sync_parent_status(
    store: &Arc<BoardStore>,
    parent_id: i64,
    actor: &Actor,
) -> Result<(), String> {
    let parent = store.get_issue(parent_id)?;
    if parent.status.is_terminal() {
        return Ok(());
    }
    let children = store.list_children(parent_id)?;
    if children.is_empty() {
        return Ok(());
    }
    let all_done = children.iter().all(|c| c.status == IssueStatus::Done);
    let any_cancelled = children.iter().any(|c| c.status == IssueStatus::Cancelled);
    if !all_done && !any_cancelled {
        return Ok(());
    }

    // backlog/todo 不能直接进 in_review，先垫 in_progress。
    if matches!(parent.status, IssueStatus::Backlog | IssueStatus::Todo) {
        store.transition_issue(parent_id, IssueStatus::InProgress, actor)?;
    }
    if all_done {
        if parent.status != IssueStatus::InReview {
            store.transition_issue(parent_id, IssueStatus::InReview, actor)?;
        }
    } else if parent.status != IssueStatus::InReview {
        store.transition_issue(parent_id, IssueStatus::InReview, actor)?;
        let gap: Vec<String> = children
            .iter()
            .filter(|c| c.status != IssueStatus::Done)
            .map(|c| format!("{}（{}）", c.number, c.status))
            .collect();
        store.add_comment(nemesis_board::NewComment {
            issue_id: parent_id,
            author: nemesis_board::Actor::system("board"),
            content: format!(
                "⚠ 子任务存在未完成项，父任务收口进评审等待人工裁决：{}",
                gap.join("、")
            ),
            parent_id: None,
            ctype: nemesis_board::CommentType::System,
        })?;
    }
    Ok(())
}

/// 子单落定（done/cancelled）后的联动 = M1 补派触发器 + 父单收口：扫
/// dependents 中依赖已满足的待派子单补派（`dispatch_subissue_auto` 自带
/// 状态/在途/依赖闸），随后父单状态联动。错误只 warn 不上抛（联动是
/// 附加动作，不阻断落定本身）。接线点：WSAPI `issue.status` / `issue.move`
/// 落到 done/cancelled 时（CLI 直写 store 不经此路径——M1 已知边界）。
#[cfg(feature = "cluster")]
pub fn on_issue_settled(
    store: &Arc<BoardStore>,
    cluster: Option<&Arc<nemesis_cluster::cluster::Cluster>>,
    issue_id: i64,
    actor: &Actor,
) {
    if let Ok(issue) = store.get_issue(issue_id)
        && let Some(pid) = issue.parent_issue_id
        && let Err(e) = sync_parent_status(store, pid, actor)
    {
        tracing::warn!("[Board] 父单 {pid} 联动失败：{e}");
    }
    match store.dependents_of(issue_id) {
        Ok(deps) => {
            for dep_id in deps {
                if let Err(e) = dispatch_subissue_auto(store, cluster, dep_id, actor) {
                    tracing::warn!("[Board] 补派子单 {dep_id} 失败：{e}");
                }
            }
        }
        Err(e) => tracing::warn!("[Board] dependents_of({issue_id}) 查询失败：{e}"),
    }
}

/// `issue.cancel` 实现（cluster 编译时，W2 P4 per-task cancel）：终结进行中
/// 的派发并下行取消。顺序即安全序：先 `cancel_dispatch` 竞态守卫（只认
/// dispatched 态，Some=赢），赢才转 issue → cancelled（状态机写
/// status_change 评论 + 活动 + 通知），最后 fire-and-forget 发 task_cancel
/// RPC（worker abort；迟到回调/回报被 B 端取消守卫与 A 端写回幂等早退兜住）。
#[cfg(feature = "cluster")]
async fn issue_cancel(
    store: &Arc<BoardStore>,
    actor: Actor,
    ctx: &RequestContext,
    data: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    let data = data.ok_or("missing data")?;
    let id = data
        .get("id")
        .and_then(|v| v.as_i64())
        .ok_or("missing field: id")?;
    let issue = store.get_issue(id)?;

    // 取消的前提：有进行中的派发。
    let dispatch = store
        .get_active_dispatch(id)?
        .ok_or_else(|| "该 issue 没有进行中的派发，无需取消".to_string())?;

    // 取消 = 下行 task_cancel 通知 worker abort，集群必需。
    let cluster = ctx
        .state
        .cluster
        .clone()
        .ok_or("集群未运行，无法取消（issue.cancel 需要集群）")?;

    // 竞态守卫：只认 dispatched 态；输掉竞态（worker 恰好回报/超时 sweep
    // 先到）→ 不动 issue，报错让前端刷新。
    let task_id = dispatch.task_id.clone();
    let worker_id = dispatch.worker_id.clone();
    if store.cancel_dispatch(&task_id, &actor)?.is_none() {
        return Err("派发已终结（worker 回报或超时），取消未生效".to_string());
    }

    // 状态机：→ cancelled（终态）。
    let issue = if issue.status != IssueStatus::Cancelled {
        store.transition_issue(id, IssueStatus::Cancelled, &actor)?
    } else {
        issue
    };

    // 下行取消（fire-and-forget）：B 端 gateway 收 task_cancel → abort 任务。
    // 送达失败不影响 A 侧终态（worker 回报被写回幂等早退兜住），评论留痕。
    let rpc_client = cluster.rpc_client_arc().ok_or("RPC client not available")?;
    let request = nemesis_cluster::rpc_types::RPCRequest {
        id: format!("cancel-{task_id}"),
        action: nemesis_cluster::rpc_types::ActionType::Custom("task_cancel".to_string()),
        payload: serde_json::json!({ "task_id": task_id }),
        source: cluster.node_id().to_string(),
        target: Some(worker_id.clone()),
    };
    let store_for_rpc = store.clone();
    let task_id_for_rpc = task_id.clone();
    tokio::spawn(async move {
        let timeout = std::time::Duration::from_secs(30);
        match rpc_client
            .call_with_timeout(&worker_id, request, timeout)
            .await
        {
            Ok(_) => {
                tracing::info!("[Board] task_cancel delivered (task_id={task_id_for_rpc})");
            }
            Err(e) => {
                tracing::warn!("[Board] task_cancel send failed (task_id={task_id_for_rpc}): {e}");
                let _ = store_for_rpc.add_comment(nemesis_board::models::NewComment {
                    issue_id: id,
                    author: nemesis_board::Actor::system("board"),
                    content: format!("⛔ 取消指令送达失败（{e}），worker 端可能仍在执行"),
                    parent_id: None,
                    ctype: nemesis_board::CommentType::System,
                });
            }
        }
    });

    Ok(Some(serde_json::json!({
        "cancelled": true,
        "task_id": task_id,
        "issue": issue_to_view(store, &issue)?,
    })))
}

/// `issue.cancel`（未编译 cluster）：取消要下行 task_cancel，无从谈起。
#[cfg(not(feature = "cluster"))]
async fn issue_cancel(
    _store: &Arc<BoardStore>,
    _actor: Actor,
    _ctx: &RequestContext,
    _data: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    Err("issue.cancel 需要集群支持（cluster feature 未编译）".to_string())
}

/// 自动派发判定（纯函数，可单测）：config 开关开 **且** 指派对象是 worker。
/// 开关 = `config.json` 的 `board.auto_dispatch`（W2.5 接口预留，用户拍板
/// 2026-08-31：现阶段不做自动派发，默认 false；置 true 即激活）。
fn should_auto_dispatch(
    board_cfg: Option<&nemesis_config::BoardFlagConfig>,
    assignee: Option<AssignmentType>,
) -> bool {
    board_cfg.map(|b| b.auto_dispatch).unwrap_or(false) && assignee == Some(AssignmentType::Worker)
}

/// 读 live config 的 board 段（全局 ConfigStore 单例；测试/CLI 无单例 →
/// None → 判定恒 false，与「默认关」语义一致）。
fn live_board_config() -> Option<nemesis_config::BoardFlagConfig> {
    nemesis_config::load_live().and_then(|c| c.board)
}

/// 指派成功后的自动派发（`issue.assign` 末尾调用；W2.5 接口预留）。走
/// [`dispatch_issue_core`] 单一派发入口。失败**不回滚指派**：warn + ⛔ 系统
/// 评论留痕（与派发 RPC 送达失败同语义）。返回是否实际派发（调用方据此
/// 重取 issue 保证响应反映派发后的 in_progress 态）。
#[cfg(feature = "cluster")]
fn auto_dispatch_after_assign(
    store: &Arc<BoardStore>,
    cluster: Option<&Arc<nemesis_cluster::cluster::Cluster>>,
    issue: &nemesis_board::Issue,
    actor: &Actor,
) -> bool {
    auto_dispatch_with_config(live_board_config().as_ref(), store, cluster, issue, actor)
}

/// [`auto_dispatch_after_assign`] 的可测内核：config 显式入参（测试不碰
/// 进程级全局单例），判定 + 派发 + 失败留痕全部在此。
#[cfg(feature = "cluster")]
fn auto_dispatch_with_config(
    board_cfg: Option<&nemesis_config::BoardFlagConfig>,
    store: &Arc<BoardStore>,
    cluster: Option<&Arc<nemesis_cluster::cluster::Cluster>>,
    issue: &nemesis_board::Issue,
    actor: &Actor,
) -> bool {
    if !should_auto_dispatch(board_cfg, issue.assignee) {
        return false;
    }
    let Some(target) = issue.assignee_id.clone() else {
        return false;
    };
    match dispatch_issue_core(store, cluster, issue.id, &target, actor, None) {
        Ok(_) => {
            tracing::info!(
                "[Board] auto-dispatch: issue #{} → {target}（board.auto_dispatch）",
                issue.number
            );
            true
        }
        Err(e) => {
            tracing::warn!(
                "[Board] auto-dispatch failed for issue #{}: {e}",
                issue.number
            );
            let _ = store.add_comment(nemesis_board::models::NewComment {
                issue_id: issue.id,
                author: nemesis_board::Actor::system("board"),
                content: format!("⛔ 自动派发失败：{e}"),
                parent_id: None,
                ctype: nemesis_board::CommentType::System,
            });
            false
        }
    }
}

/// 非 cluster 编译：派发无从谈起，接口恒零操作。仍跑同一判定 gate——
/// 开关被误开时 warn 提示（配置生效但能力缺失，行为可观测），并保持
/// helper 在两种 feature 配置下都被使用（零 dead_code）。
#[cfg(not(feature = "cluster"))]
fn auto_dispatch_after_assign(
    _store: &Arc<BoardStore>,
    issue: &nemesis_board::Issue,
    _actor: &Actor,
) -> bool {
    if should_auto_dispatch(live_board_config().as_ref(), issue.assignee) {
        tracing::warn!(
            "[Board] board.auto_dispatch=true 但 cluster feature 未编译，无法自动派发（issue #{})",
            issue.number
        );
    }
    false
}

// ---------------------------------------------------------------------------
// autopilot（W2 P4 定时派活）：模板建单 + 可选派发 + cron 触发簿记
// ---------------------------------------------------------------------------

/// autopilot 模板 → NewIssue：标题 `{date}` 占位符替换为本地日期
/// （YYYY-MM-DD）；origin 记 autopilot（run 历史 =
/// `list_issues_by_origin("autopilot", id)`）；target 非空时预指派 worker。
fn autopilot_new_issue(ap: &nemesis_board::Autopilot, actor: &Actor) -> NewIssue {
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let mut ni = NewIssue {
        title: ap.title.replace("{date}", &date),
        description: ap.description.clone(),
        priority: ap.priority,
        project_id: ap.project_id,
        creator: actor.clone(),
        origin: Some(nemesis_board::models::TaskOrigin {
            origin_type: "autopilot".to_string(),
            origin_id: ap.id.to_string(),
        }),
        ..NewIssue::default()
    };
    let target = ap.target.trim();
    if !target.is_empty() {
        ni.assignee = Some(AssignmentType::Worker);
        ni.assignee_id = Some(target.to_string());
    }
    ni
}

/// autopilot 触发核心（W2 P4；WSAPI `autopilot.run` 与 gateway 的 cron
/// on_job 共用，单一真相源）：按模板建 issue → target 非空时派发 →
/// 记 last_run_at。`cluster` 传 `None` 且 target 非空 → 建单前拒绝
/// （不留半成品）。
#[cfg(feature = "cluster")]
pub fn fire_autopilot(
    store: &Arc<BoardStore>,
    cluster: Option<&Arc<nemesis_cluster::cluster::Cluster>>,
    ap: &nemesis_board::Autopilot,
    actor: &Actor,
) -> Result<serde_json::Value, String> {
    let target = ap.target.trim().to_string();
    if !target.is_empty() && cluster.is_none() {
        return Err(format!(
            "autopilot「{}」配置了派发目标 {target}，但集群未运行，无法派发",
            ap.name
        ));
    }
    let issue = store.create_issue(autopilot_new_issue(ap, actor))?;
    let dispatch = if target.is_empty() {
        None
    } else {
        Some(
            dispatch_issue_core(store, cluster, issue.id, &target, actor, None)
                .map_err(|e| format!("issue #{} 已创建但派发失败: {e}", issue.number))?,
        )
    };
    store.mark_autopilot_run(ap.id)?;
    Ok(serde_json::json!({
        "ran": true,
        "issue_id": issue.id,
        "issue_number": issue.number,
        "dispatch": dispatch,
    }))
}

/// 非 cluster 编译：autopilot 只支持 target 为空的周期建单。
#[cfg(not(feature = "cluster"))]
pub fn fire_autopilot(
    store: &Arc<BoardStore>,
    ap: &nemesis_board::Autopilot,
    actor: &Actor,
) -> Result<serde_json::Value, String> {
    if !ap.target.trim().is_empty() {
        return Err(format!(
            "autopilot「{}」配置了派发目标，但 cluster feature 未编译，无法派发",
            ap.name
        ));
    }
    let issue = store.create_issue(autopilot_new_issue(ap, actor))?;
    store.mark_autopilot_run(ap.id)?;
    Ok(serde_json::json!({
        "ran": true,
        "issue_id": issue.id,
        "issue_number": issue.number,
        "dispatch": Option::<serde_json::Value>::None,
    }))
}

/// autopilot 的 cron job 即时登记/跟随更新（W2 P4）。cron 服务未注入
/// （单测/极简构建）时跳过——gateway 启动同步会以 store 为真相源补注册。
/// 已登记过（cron_job_id 命中且 job 还在）→ patch 跟随 store；否则新登记
/// 并回存 job id（`board-ap:{autopilot_id}` 名字约定，启动同步据此清孤儿）。
fn arm_autopilot_job(
    ctx: &RequestContext,
    store: &Arc<BoardStore>,
    ap: &nemesis_board::Autopilot,
) -> Result<(), String> {
    let Some(cron) = ctx.state.cron.as_ref() else {
        return Ok(());
    };
    arm_autopilot_job_with(cron, store, ap)
}

/// arm 的核心（不依赖 RequestContext）：gateway 启动同步
/// （`sync_autopilot_jobs`）与 WSAPI handler 共用这一份构造逻辑——
/// 单一真相源，别在 gateway 里复制 CronSchedule/patch 语义。
fn arm_autopilot_job_with(
    cron: &Arc<std::sync::Mutex<nemesis_cron::service::CronService>>,
    store: &Arc<BoardStore>,
    ap: &nemesis_board::Autopilot,
) -> Result<(), String> {
    let schedule = nemesis_cron::CronSchedule {
        kind: "cron".to_string(),
        at_ms: None,
        every_ms: None,
        expr: Some(ap.cron.clone()),
        tz: None,
    };
    let svc = cron
        .lock()
        .map_err(|_| "cron service lock poisoned".to_string())?;
    if let Some(job_id) = ap.cron_job_id.as_deref()
        && svc.get_job(job_id).is_some()
    {
        svc.patch_job(
            job_id,
            &nemesis_cron::CronJobPatch {
                schedule: Some(schedule),
                enabled: Some(ap.enabled),
                ..Default::default()
            },
        )?;
        return Ok(());
    }
    // add_job_ext 返回随机 id（不支持指定 id），注册后回存映射。
    let job = svc.add_job_ext(
        &format!("board-ap:{}", ap.id),
        schedule,
        "",
        false,
        None,
        None,
        None,
        None,
        ap.enabled,
    )?;
    store.set_autopilot_cron_job(ap.id, Some(&job.id))
}

/// autopilot 删除时顺手摘掉 cron job（best-effort：失败只记日志，不阻断
/// 删除——启动同步的孤儿清理兜底）。
fn disarm_autopilot_job(ctx: &RequestContext, ap: &nemesis_board::Autopilot) {
    if let (Some(cron), Some(job_id)) = (ctx.state.cron.as_ref(), ap.cron_job_id.as_deref()) {
        match cron.lock() {
            Ok(svc) => {
                if !svc.remove_job(job_id) {
                    tracing::warn!("[Board] autopilot cron job not found: {job_id}");
                }
            }
            Err(_) => tracing::warn!(
                "[Board] cron service lock poisoned; autopilot job {job_id} left behind"
            ),
        }
    }
}

/// W2 P4: autopilot cron job 启动同步（gateway 在 cron.start 前调用）。
/// store 是唯一真相源，把 cron 服务对齐到 store：
///   1. 删孤儿：cron 里 `board-ap:*` 命名空间的 job 在 store 侧已无对应
///      登记（规则被删/改时 backfill 失败留下的幽灵 job）→ 移除，防到点
///      触发；只动 board-ap: 前缀，不碰用户自己的 cron job。
///   2. 补登记：规则缺 job（上次登记时 cron 未注入、进程崩溃丢内存态）→
///      走 arm 重新登记并回存 job id。
///   3. 跟随：已登记的 job schedule/enabled 跟随 store（与 arm 同一逻辑，
///      幂等）。
///      返回重新登记的规则数（gateway 记日志用）。
pub fn sync_autopilot_jobs(
    cron: &Arc<std::sync::Mutex<nemesis_cron::service::CronService>>,
    store: &Arc<BoardStore>,
) -> Result<usize, String> {
    let aps = store.list_autopilots()?;
    let valid_job_ids: std::collections::HashSet<&str> = aps
        .iter()
        .filter_map(|a| a.cron_job_id.as_deref())
        .collect();
    // 1) 孤儿清理。锁只在扫描段持有，remove 后即释放，不与 2)3) 嵌套。
    {
        let svc = cron
            .lock()
            .map_err(|_| "cron service lock poisoned".to_string())?;
        for job in svc.list_jobs(true) {
            if job.name.starts_with("board-ap:") && !valid_job_ids.contains(job.id.as_str()) {
                tracing::info!(
                    "[Board] autopilot sync: removing orphan cron job {} ({})",
                    job.id,
                    job.name
                );
                svc.remove_job(&job.id);
            }
        }
    }
    // 2)+3) 逐条 arm（内部自判 patch 跟随 or 新登记；均幂等）。
    let mut rearmed = 0;
    for ap in &aps {
        let registered = match ap.cron_job_id.as_deref() {
            Some(jid) => {
                let svc = cron
                    .lock()
                    .map_err(|_| "cron service lock poisoned".to_string())?;
                svc.get_job(jid).is_some()
            }
            None => false,
        };
        arm_autopilot_job_with(cron, store, ap)?;
        if !registered {
            rearmed += 1;
        }
    }
    Ok(rearmed)
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
            "issue.create",
            "issue.update",
            "issue.assign",
            "issue.status",
            "issue.move",
            "issue.dispatch",
            "issue.cancel",
            "issue.plan",
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
            "project.create",
            "project.update",
            "attachment.add",
            "attachment.get",
            "inbox.list",
            "inbox.mark_read",
            "attachment.list",
            "channel.list",
            "channel.messages",
            "channel.post",
            "stats",
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
        match cmd {
            // --- issue ---
            "issue.list" => {
                let data = data.ok_or("missing data")?;
                let issues = store.list_issues(&build_filter(&data)?)?;
                let total = issues.len();
                Ok(Some(
                    serde_json::json!({ "issues": issues, "total": total }),
                ))
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
            "issue.create" => {
                let data = data.ok_or("missing data")?;
                let issue = store.create_issue(build_new_issue(&data, actor)?)?;
                Ok(Some(
                    serde_json::json!({ "created": true, "issue": issue_to_view(&store, &issue)? }),
                ))
            }
            "issue.update" => {
                let data = data.ok_or("missing data")?;
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
                // 自动派发接口（W2.5 预留，默认关；board.auto_dispatch=true
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
                // M1 补派触发器 + 父单收口（子单落定后联动；错误只 warn）。
                #[cfg(feature = "cluster")]
                if matches!(to, IssueStatus::Done | IssueStatus::Cancelled) {
                    on_issue_settled(&store, ctx.state.cluster.as_ref(), id, &actor);
                }
                Ok(Some(
                    serde_json::json!({ "changed": true, "issue": issue_to_view(&store, &issue)? }),
                ))
            }
            // 看板拖拽（W2 P3）：状态转移 + 列内排序一个原子操作（同列重排
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
                // M1 补派触发器 + 父单收口（跨列拖到 done/cancelled 同样落定）。
                #[cfg(feature = "cluster")]
                if matches!(to, IssueStatus::Done | IssueStatus::Cancelled) {
                    on_issue_settled(&store, ctx.state.cluster.as_ref(), id, &actor);
                }
                Ok(Some(
                    serde_json::json!({ "moved": true, "issue": issue_to_view(&store, &issue)? }),
                ))
            }
            // 派发到远端 worker（W2 P2）：issue → peer_chat，task_id ↔ issue
            // 绑定入 issue_dispatch 表；worker 回报经 peer_chat_callback 由
            // gateway 写回看板（成功 → 结果评论 + in_review，失败 → 失败评论）。
            "issue.dispatch" => issue_dispatch(&store, actor, ctx, data).await,
            // 取消进行中的派发（W2 P4）：A 侧派发/issue 双终态 + 下行
            // task_cancel 让 worker abort。赢竞态才动账（worker 恰好回报则
            // 拒绝取消，issue 保持写回的状态）。
            "issue.cancel" => issue_cancel(&store, actor, ctx, data).await,
            // AI 拆解（Swarm M1）：confirm 缺省/false = 异步跑 planner（返回
            // plan_id，结果经 board.plan_ready/failed push）；confirm:true =
            // 按 plan_id 落库 + 依赖闸派发波 + 父单联动。
            "issue.plan" => issue_plan(&store, actor, ctx, data).await,
            // --- autopilot（W2 P4 定时派活）---
            "autopilot.list" => {
                let autopilots = store.list_autopilots()?;
                Ok(Some(serde_json::json!({ "autopilots": autopilots })))
            }
            "autopilot.create" => {
                let data = data.ok_or("missing data")?;
                let cron_expr = get_str(&data, "cron")?;
                nemesis_cron::CronService::validate_schedule(&cron_expr)?;
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
                    enabled: data
                        .get("enabled")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(true),
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
                let id = data
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: id")?;
                if let Some(c) = get_opt_str(&data, "cron") {
                    nemesis_cron::CronService::validate_schedule(&c)?;
                }
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
                let data = data.ok_or("missing data")?;
                let id = data
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: id")?;
                let ap = store.get_autopilot(id)?;
                let out = {
                    #[cfg(feature = "cluster")]
                    {
                        fire_autopilot(&store, ctx.state.cluster.as_ref(), &ap, &actor)?
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
            // --- comment / activity ---
            "comment.add" => {
                let data = data.ok_or("missing data")?;
                let comment = store.add_comment(NewComment {
                    issue_id: data
                        .get("issue_id")
                        .and_then(|v| v.as_i64())
                        .ok_or("missing field: issue_id")?,
                    author: actor,
                    content: get_str(&data, "content")?,
                    parent_id: data.get("parent_id").and_then(|v| v.as_i64()),
                    ctype: CommentType::Comment,
                })?;
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
            // --- subscriber ---
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
            // --- project / attachment / stats ---
            "project.list" => {
                let projects = store.list_projects()?;
                Ok(Some(serde_json::json!({ "projects": projects })))
            }
            "project.create" => {
                let data = data.ok_or("missing data")?;
                let project = store.create_project(
                    &get_str(&data, "name")?,
                    &get_opt_str(&data, "description").unwrap_or_default(),
                    None,
                    &get_opt_str(&data, "icon").unwrap_or_default(),
                )?;
                Ok(Some(
                    serde_json::json!({ "created": true, "project": project }),
                ))
            }
            // 项目字段级更新（W2 P3）：归档走 status="archived"（软删除）。
            "project.update" => {
                let data = data.ok_or("missing data")?;
                let id = data
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: id")?;
                let patch = ProjectPatch {
                    name: get_opt_str(&data, "name"),
                    description: get_opt_str(&data, "description"),
                    status: get_opt_str(&data, "status"),
                    icon: get_opt_str(&data, "icon"),
                };
                let project = store.update_project(id, &patch)?;
                Ok(Some(
                    serde_json::json!({ "updated": true, "project": project }),
                ))
            }
            // 附件上传（W2 P3）：base64 内容 → workspace/board/files/ 存文件
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
            // 附件下载（W2 P3）：读文件回 base64（MVP 经 WS 传小文件；大文件
            // 走 HTTP 静态路由留 P4 评估）。
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
            // 收件箱（W2 P3）：站内通知列表（store 事件钩子产生；经通道的
            // 站外投递留 P4）。MVP 单管理员语义：admin 通知全员可见。
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
            // --- channel（Swarm M3 批次 E：讨论频道）---
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
                let after_id = data
                    .get("after_id")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
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
            _ => Err(format!("unknown command: board.{}", cmd)),
        }
    }
}

#[cfg(test)]
mod tests;
