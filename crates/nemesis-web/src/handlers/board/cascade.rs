// ---------------------------------------------------------------------------
// 2026-11-05 机械拆分：本文件内容由 board.rs 原样搬入（纯代码搬家，行为零变更）。
// ---------------------------------------------------------------------------

use super::*;

/// 父单状态联动：子树（Subtree 闭包，SAN-06/08）全部 done → 父单
/// in_review；子树任一 cancelled → 父单 in_review + 系统评论标注缺口（人工
/// 裁决重开或取消）。backlog/todo 先推 in_progress（状态机不允许跨列）。
/// **沿祖先链向上传播**（visited 防环）：终态父单不再早退阻断——状态机
/// 拒绝终态转移（不强行推列）但缺口评论照写、上层继续重估。旧实现只动
/// 直接父单且终态早退，中间层 done 之后其下取消叶子对上层永久冻结（收口
/// 汇报声称完成而子树实际存在取消缺口）。
#[cfg(feature = "cluster")]
pub fn sync_parent_status(
    store: &Arc<BoardStore>,
    parent_id: i64,
    actor: &Actor,
) -> Result<(), String> {
    let mut visited: std::collections::HashSet<i64> = std::collections::HashSet::new();
    let mut cur = Some(parent_id);
    while let Some(pid) = cur {
        if !visited.insert(pid) {
            break; // 数据异常 parent 自引用——防御性截断
        }
        cur = sync_one_parent(store, pid, actor)?;
    }
    Ok(())
}

/// 单层联动（[`sync_parent_status`] 内部）：评估 parent 的整棵子树并推
/// 进状态 → 返回再上一层父单 id（None = 已到顶）。
#[cfg(feature = "cluster")]
fn sync_one_parent(
    store: &Arc<BoardStore>,
    parent_id: i64,
    actor: &Actor,
) -> Result<Option<i64>, String> {
    let parent = store.get_issue(parent_id)?;
    let next = parent.parent_issue_id;
    let terminal = parent.status.is_terminal();
    // 子树闭包（SAN-08 Subtree）：只走父子边，穿过中间层看全部后代。
    let subtree = store.descendants(&[parent_id], DescendantEdges::Subtree);
    if subtree.is_empty() {
        return Ok(next);
    }
    let all_done = subtree.iter().all(|c| c.status == IssueStatus::Done);
    let any_cancelled = subtree.iter().any(|c| c.status == IssueStatus::Cancelled);
    if !all_done && !any_cancelled {
        return Ok(next);
    }

    // AOR 批二b（⑥ FINISH 谓词）：全 done 垫升前过收口谓词（只 gate
    // all_done 分支——any_cancelled 分支维持原样：缺口评论+进评审等人工，
    // 语义不变）。谓词在正常路径恒真；全 done 却不满足 = 活跃派发在途或
    // 未裁决 unknown（批一c ③ 落盘的 `unknown:` 清单）——阻塞垫升/收口 +
    // 缺口评论显式列出（C-F4 去重），防「评审还没裁决完单子就被钩子拉走」。
    if all_done && !terminal {
        let unsatisfied = nemesis_board::finish::evaluate_parent_finish(store, &subtree);
        if !unsatisfied.is_empty() {
            tracing::warn!(
                "[Board] 父单 {parent_id} FINISH 收口闸未满足（{} 条），垫升/收口阻塞",
                unsatisfied.len()
            );
            if let Err(e) =
                nemesis_board::finish::write_finish_gap_comment(store, parent_id, &unsatisfied)
            {
                tracing::warn!("[Board] 父单 {parent_id} FINISH 缺口评论写入失败：{e}");
            }
            return Ok(next);
        }
    }

    // backlog/todo 不能直接进 in_review，先垫 in_progress（blocked 同理：
    // blocked→in_progress 合法而 blocked→in_review 非法——B5 停车联动的
    // 受阻父单全子落定走这里必须先垫）。终态父单跳过一切推列。
    if !terminal
        && matches!(
            parent.status,
            IssueStatus::Backlog | IssueStatus::Todo | IssueStatus::Blocked
        )
    {
        store.transition_issue(parent_id, IssueStatus::InProgress, actor)?;
    }
    if all_done {
        if !terminal && parent.status != IssueStatus::InReview {
            store.transition_issue(parent_id, IssueStatus::InReview, actor)?;
        }
        // 全自动流转 P1（A3）：子单全部落定 → 触发父单收口验收钩子。
        // 旗标判定在 hook 内（未注册 / auto_close_parent=false = no-op，
        // 等价现行为）；cancelled 缺口闸也在 hook 侧。终态父单已收过口，
        // 不重复触发。
        if !terminal && let Some(hook) = PARENT_REVIEW_HOOK.get() {
            hook(parent_id);
        }
    } else if !terminal && parent.status != IssueStatus::InReview {
        store.transition_issue(parent_id, IssueStatus::InReview, actor)?;
    }
    if !all_done {
        // 缺口评论列全子树非 done 项（旧实现只列直接子单——孙层缺口不可
        // 见）。终态父单（无法推列）也写：SAN-06 冻结缺口就此显形而非
        // 永久沉默。非终态父单维持旧防重语义（已在 in_review 不重复写）。
        if terminal || parent.status != IssueStatus::InReview {
            let gap: Vec<String> = subtree
                .iter()
                .filter(|c| c.status != IssueStatus::Done)
                .map(|c| format!("{}（{}）", c.number, c.status))
                .collect();
            let content = format!(
                "⚠ 子任务存在未完成项，父任务收口进评审等待人工裁决：{}",
                gap.join("、")
            );
            // C-F4（复核 2026-09-16）：终态父单缺口评论去重——terminal 分支
            // 没有状态变化可依（旧防重语义「已在 in_review 不重复写」对终态
            // 单恒不命中），此后每次子单落定触发 sync 都会重写同款评论刷屏。
            // 与最近一条同款系统评论逐字比较，内容未变则跳过；缺口集合变化
            // 时仍会补写显形（SAN-06 语义不变）。
            let duplicated = store
                .list_comments(parent_id)
                .map(|cs| {
                    cs.iter()
                        .rev()
                        .find(|cm| {
                            cm.ctype == nemesis_board::CommentType::System
                                && cm.content.starts_with("⚠ 子任务存在未完成项")
                        })
                        .is_some_and(|cm| cm.content == content)
                })
                .unwrap_or(false);
            if !duplicated {
                store.add_comment(nemesis_board::NewComment {
                    issue_id: parent_id,
                    author: nemesis_board::Actor::system("board"),
                    content,
                    parent_id: None,
                    ctype: nemesis_board::CommentType::System,
                })?;
            }
        }
    }
    Ok(next)
}

/// 子单落定（done/cancelled）后的联动 = M1 补派触发器 + 父单收口：扫
/// dependents 中依赖已满足的待派子单补派（`dispatch_subissue_auto` 自带
/// 状态/在途/依赖闸），随后父单状态联动。错误只 warn 不上抛（联动是
/// 附加动作，不阻断落定本身）。接线点：WSAPI `issue.status` / `issue.move`
/// 落到 done/cancelled 时（CLI 直写 store 不经此路径——M1 已知边界）。
/// 返回级联取消的编号列表（C3：`issue.cancel` 响应体披露）。
#[cfg(feature = "cluster")]
pub fn on_issue_settled(
    store: &Arc<BoardStore>,
    cluster: Option<&Arc<nemesis_cluster::cluster::Cluster>>,
    issue_id: i64,
    actor: &Actor,
) -> Vec<String> {
    let cancelled = store
        .get_issue(issue_id)
        .map(|i| i.status == IssueStatus::Cancelled)
        .unwrap_or(false);
    if let Ok(issue) = store.get_issue(issue_id)
        && let Some(pid) = issue.parent_issue_id
        && let Err(e) = sync_parent_status(store, pid, actor)
    {
        tracing::warn!("[Board] 父单 {pid} 联动失败：{e}");
    }
    // F3：顶层父单落定是项目收口验收的触发面（前置聚合检查在 notify 内，
    // 不过 = no-op）。
    notify_project_review_on_parent_done(store, issue_id);
    if cancelled {
        // B1：依赖闸只认 done 且状态机无 reopen——依赖取消后 dependents
        // 永远无法派发（死链）。级联取消让死链显形：各自留痕 + 父单经
        // sync_parent_status 进 in_review + 缺口评论。（reopen 已补上
        // cancelled→backlog 出口：级联取消的单可用 issue.reopen 复活。）
        // F-U4-3：级联边 = 依赖边 ∪ 父子边（见 cascade_cancel_dependents）。
        cascade_cancel_dependents(store, cluster, issue_id, actor)
    } else {
        match store.dependents_of(issue_id) {
            Ok(deps) => {
                for dep_id in deps {
                    if let Err(e) = dispatch_subissue_auto(store, cluster, dep_id, actor, true) {
                        tracing::warn!("[Board] 补派子单 {dep_id} 失败：{e}");
                    }
                }
            }
            Err(e) => tracing::warn!("[Board] dependents_of({issue_id}) 查询失败：{e}"),
        }
        Vec::new()
    }
}

/// 下行 task_cancel（fire-and-forget，30s 超时）：`issue.cancel` 在途取消
/// 与级联取消（[`cascade_cancel_dependents`]）共用的单一出口。送达失败不
/// 影响 A 侧终态（worker 回报被写回幂等早退兜住），评论留痕；RPC client
/// 不可用时 warn 放弃（best-effort，不阻断取消本体）。
#[cfg(feature = "cluster")]
/// pub：replan 执行臂（nemesisbot `board_replan.rs`）跨 crate 共用（同
/// `dispatch_issue_core`/`confirm_plan`/`cascade_cancel_dependents` 先例）
/// ——取消建议动作命中在途派发时必须走同一下行取消出口，不得另搓第二套
/// （AOR 批三 5.3）。
pub fn spawn_task_cancel(
    store: &Arc<BoardStore>,
    cluster: &Arc<nemesis_cluster::cluster::Cluster>,
    issue_id: i64,
    worker_id: &str,
    task_id: &str,
) {
    // 本地派发（target="self"）没有远端任务可取消：RPC 对 "self" 只会
    // peer unresolvable + 误导性失败评论。本地执行体不受 cancel 打断
    //（跑完即止），写回终态幂等守卫保证派发行一致性——诚实跳过。
    if worker_id == "self" {
        tracing::info!("[Board] task_cancel 跳过（本地派发无远端任务）：task_id={task_id}");
        return;
    }
    let Some(rpc_client) = cluster.rpc_client_arc() else {
        tracing::warn!("[Board] task_cancel 未下发（RPC client 不可用）：task_id={task_id}");
        return;
    };
    let request = nemesis_cluster::rpc_types::RPCRequest {
        id: format!("cancel-{task_id}"),
        action: nemesis_cluster::rpc_types::ActionType::Custom("task_cancel".to_string()),
        payload: serde_json::json!({ "task_id": task_id }),
        source: cluster.node_id().to_string(),
        target: Some(worker_id.to_string()),
    };
    let store = store.clone();
    let worker_id = worker_id.to_string();
    let task_id = task_id.to_string();
    tokio::spawn(async move {
        let timeout = std::time::Duration::from_secs(30);
        match rpc_client
            .call_with_timeout(&worker_id, request, timeout)
            .await
        {
            Ok(_) => {
                tracing::info!("[Board] task_cancel delivered (task_id={task_id})");
            }
            Err(e) => {
                tracing::warn!("[Board] task_cancel send failed (task_id={task_id}): {e}");
                let _ = store.add_comment(nemesis_board::models::NewComment {
                    issue_id,
                    author: nemesis_board::Actor::system("board"),
                    content: format!("⛔ 取消指令送达失败（{e}），worker 端可能仍在执行"),
                    parent_id: None,
                    ctype: nemesis_board::CommentType::System,
                });
            }
        }
    });
}

/// 级联取消（B1 + F-U4-3 + SAN-08）：root 落 cancelled 后，两条「挂在
/// 我身上就会陪葬」的边全部级联——**依赖边**（`dependents_of`：依赖我的
/// 单，依赖闸只认 done，死链显形）+ **父子边**（`list_children`：planner
/// 拆解的依赖边是兄弟链、从不指向父单，只走依赖边则 cancel 父单恒零级联，
/// 子单沦为僵尸 backlog 且补派触发器还会继续派它们空烧 token）。遍历统一
/// 走 [`nemesis_board::store::BoardStore::descendants`]（依赖∪父子闭包，
/// visited 防环）——**穿过终态单继续下探**：旧手搓 BFS 在终态单上直接
/// 跳过且不入队，done/cancelled 中间节点之下的死链从此不可见（依赖链
/// 穿不过已完成的环节）。终态过滤在动作侧：只有非终态节点被取消。
/// 在途派发连带取消（竞态守卫赢才动派发行 + 集群在则下行 task_cancel）。
/// 每单系统评论留痕（状态机自带 status_change 评论 + 活动 + 通知）；各
/// 父单经 [`sync_parent_status`] any_cancelled 分支进 in_review + 缺口
/// 评论。传导断路：某节点取消失败（竞态输给 worker 完成）则其下游不再
/// 连带取消（与旧「失败不入队」语义等价）。返回被连带取消的编号列表
/// （C3：cancel 响应体披露，不再无声）。
///
/// pub：replan 执行臂（nemesisbot `board_replan.rs`）跨 crate 共用（同
/// `dispatch_issue_core`/`confirm_plan` 先例）——取消建议动作必须走同一
/// 级联取消机制（传导断路/在途派发取消/父单同步/留痕全对齐），不得另搓
/// 第二套（AOR 批三 5.3）。
#[cfg(feature = "cluster")]
pub fn cascade_cancel_dependents(
    store: &Arc<BoardStore>,
    cluster: Option<&Arc<nemesis_cluster::cluster::Cluster>>,
    root_id: i64,
    actor: &Actor,
) -> Vec<String> {
    let mut cascaded: Vec<String> = Vec::new();
    let Ok(root_issue) = store.get_issue(root_id) else {
        return cascaded;
    };
    let mut stopped: std::collections::HashSet<i64> = std::collections::HashSet::new();
    for dep in store.descendants(&[root_id], DescendantEdges::CascadeUnion) {
        if dep.status.is_terminal() {
            continue;
        }
        // 传导断路：任一直接上游（父单/依赖）取消失败 → 本单不再连带。
        let mut upstream_blocked = dep.parent_issue_id.is_some_and(|p| stopped.contains(&p));
        if !upstream_blocked {
            match store.dependencies_of(dep.id) {
                Ok(ups) => upstream_blocked |= ups.iter().any(|u| stopped.contains(u)),
                Err(e) => {
                    tracing::warn!("[Board] 级联取消 dependencies_of({}) 查询失败：{e}", dep.id)
                }
            }
        }
        if upstream_blocked {
            continue;
        }
        let _ = store.add_comment(nemesis_board::NewComment {
            issue_id: dep.id,
            author: nemesis_board::Actor::system("board"),
            content: format!(
                "⛔ {} 已取消，本单级联取消（父单或依赖取消即死链；可 issue.reopen 复活）",
                root_issue.number
            ),
            parent_id: None,
            ctype: nemesis_board::CommentType::System,
        });
        // 在途派发连带取消：竞态守卫（赢 = 终结派发行）→ 集群在则下行
        // task_cancel（worker 不再空烧）。输给 worker 回报/超时 sweep →
        // 不动派发行（随后的 transition 状态机会诚实拒绝）。
        if let Ok(Some(d)) = store.get_active_dispatch(dep.id) {
            match store.cancel_dispatch(&d.task_id, actor) {
                Ok(Some(_)) => {
                    if let Some(c) = cluster {
                        spawn_task_cancel(store, c, dep.id, &d.worker_id, &d.task_id);
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!("[Board] 级联取消 {} 在途派发失败：{e}", dep.number)
                }
            }
        }
        if let Err(e) = store.transition_issue(dep.id, IssueStatus::Cancelled, actor) {
            tracing::warn!("[Board] 级联取消 {} 失败：{e}", dep.number);
            stopped.insert(dep.id);
            continue;
        }
        cascaded.push(dep.number.clone());
        if let Some(pid) = dep.parent_issue_id
            && let Err(e) = sync_parent_status(store, pid, actor)
        {
            tracing::warn!("[Board] 级联取消后父单 {pid} 联动失败：{e}");
        }
    }
    cascaded
}

/// `issue.cancel` 实现（cluster 编译时，W2 P4 per-task cancel + B2/B3 修复）：
/// 有在途派发 → 竞态守卫（只认 dispatched 态，Some=赢）→ 终态 → 下行
/// task_cancel；无在途派发（停车场单/依赖闸未放行单）→ 直接终态，集群
/// 非必需。两条路径落定后统一走 [`on_issue_settled`] 联动（父单收口 +
/// dependents 级联取消）——取消不再是孤岛操作。
#[cfg(feature = "cluster")]
pub(super) async fn issue_cancel(
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
    let dispatch = store.get_active_dispatch(id)?;

    // 有在途派发：集群必需 + 竞态守卫（只认 dispatched/running 态——D0b
    // running 同样在途、可取消，worker 侧 token cancel 打断执行中任务；
    // 输掉竞态——worker 恰好回报/超时 sweep 先到——不动 issue，报错让
    // 前端刷新）。
    let mut task_id = String::new();
    if let Some(dispatch) = dispatch {
        let cluster = ctx
            .state
            .cluster
            .clone()
            .ok_or("集群未运行，无法取消（issue.cancel 需要集群）")?;
        let worker_id = dispatch.worker_id.clone();
        let tid = dispatch.task_id.clone();
        if store.cancel_dispatch(&tid, &actor)?.is_none() {
            return Err("派发已终结（worker 回报或超时），取消未生效".to_string());
        }
        task_id = tid.clone();

        // 下行取消（fire-and-forget）：B 端 gateway 收 task_cancel → abort
        // 任务。送达失败不影响 A 侧终态（worker 回报被写回幂等早退兜住），
        // 评论留痕。（与级联取消共用 [`spawn_task_cancel`] 单一出口。）
        spawn_task_cancel(store, &cluster, id, &worker_id, &tid);
    }

    // 状态机：→ cancelled（终态）。无在途派发时直接转（B3：停车场单/
    // 依赖闸未放行单不需要集群参与）。
    let issue = if issue.status != IssueStatus::Cancelled {
        store.transition_issue(id, IssueStatus::Cancelled, &actor)?
    } else {
        issue
    };

    // B2：落定联动（父单收口 + dependents 级联取消）——错误只 warn，
    // 不回滚取消。C3：级联清单随响应披露（被连带取消的子单编号），不再
    // 无声——用户据此知道哪些单被连带、可用 issue.reopen 拉回。
    let cascade_cancelled = on_issue_settled(store, ctx.state.cluster.as_ref(), id, &actor);

    // 取消 = 已裁决（恢复动作之一）：归档停车档案。
    archive_handoff_best_effort(&issue.number);
    Ok(Some(serde_json::json!({
        "cancelled": true,
        "task_id": (!task_id.is_empty()).then_some(task_id),
        "cascade_cancelled": cascade_cancelled,
        "issue": issue_to_view(store, &issue)?,
    })))
}

/// `issue.cancel`（未编译 cluster）：取消要下行 task_cancel，无从谈起。
#[cfg(not(feature = "cluster"))]
pub(super) async fn issue_cancel(
    _store: &Arc<BoardStore>,
    _actor: Actor,
    _ctx: &RequestContext,
    _data: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    Err("issue.cancel 需要集群支持（cluster feature 未编译）".to_string())
}

/// `issue.cancel_dispatch` 实现（B14，2026-10-18 B 级清零）：**只撤派发、
/// 单据存活**——与 `issue.cancel`（连单终杀）互补的轻量出口。适用场景：
/// 派错节点想换人重派、worker 假死不等超时 sweep。语义：
/// - A 侧撤派发行（→ cancelled 终态）+ 下行 task_cancel（fire-and-forget，
///   与 issue.cancel 共用 [`spawn_task_cancel`] 单一出口）；
/// - 竞态守卫同构：赢竞态（cancel_dispatch Ok(Some)）才动账；输（worker
///   恰好回报/超时 sweep 先到）诚实报错——issue 保持写回的状态；
/// - **issue 状态不动**：留原地（in_progress 可直接重派——
///   dispatch_issue_core 状态闸放行 InProgress；转 blocked 反而挡住重派，
///   需两步操作）；
/// - 写回幂等早退兜住迟到回报（write_back_board_dispatch 对 cancelled
///   派发行 already_terminal 早退）——撤后 worker 迟到回报不会假 done；
/// - 系统评论留痕（撤回不改状态，评论是重派线索的唯一载体；评论失败
///   不回滚撤回本体，warn 留痕）。
#[cfg(feature = "cluster")]
pub(super) async fn issue_cancel_dispatch(
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
    let dispatch = store
        .get_active_dispatch(id)?
        .ok_or_else(|| "该 issue 无在途派发（未派发或已终结）".to_string())?;

    // 竞态守卫：只认 dispatched/running 态；输（worker 恰好回报/超时 sweep
    // 先到）= 派发已终结，诚实报错让前端刷新（对齐 issue.cancel）。
    if store.cancel_dispatch(&dispatch.task_id, &actor)?.is_none() {
        return Err("派发已终结（worker 回报或超时），撤回未生效".to_string());
    }

    // 下行取消（fire-and-forget）：集群缺席（理论不可达——派发本身需要
    // 集群）退化为仅本地撤回 + warn，不炸撤回本体。
    if let Some(cluster) = ctx.state.cluster.as_ref() {
        spawn_task_cancel(store, cluster, id, &dispatch.worker_id, &dispatch.task_id);
    } else {
        tracing::warn!(
            "[Board] cancel_dispatch: 集群未装配，task_cancel 未下行（task_id={})",
            dispatch.task_id
        );
    }

    // 留痕（best-effort）：撤回不改 issue 状态，评论是重派线索的唯一载体。
    if let Err(e) = store.add_comment(nemesis_board::NewComment {
        issue_id: id,
        author: nemesis_board::Actor::system("board"),
        content: format!(
            "⏹ 派发已撤回（worker={}，task_id={}）：单据保留，可直接重新派发。",
            dispatch.worker_id, dispatch.task_id
        ),
        parent_id: None,
        ctype: nemesis_board::CommentType::System,
    }) {
        tracing::warn!("[Board] cancel_dispatch 留痕评论失败（issue={id}）：{e}");
    }

    Ok(Some(serde_json::json!({
        "cancelled_dispatch": true,
        "task_id": dispatch.task_id,
        "issue": issue_to_view(store, &issue)?,
    })))
}

/// `issue.cancel_dispatch`（未编译 cluster）：与 issue.cancel 同构——无
/// 集群编译形态下不存在在途派发（派发本身需要集群）。
#[cfg(not(feature = "cluster"))]
pub(super) async fn issue_cancel_dispatch(
    _store: &Arc<BoardStore>,
    _actor: Actor,
    _ctx: &RequestContext,
    _data: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    Err("issue.cancel_dispatch 需要集群支持（cluster feature 未编译）".to_string())
}
