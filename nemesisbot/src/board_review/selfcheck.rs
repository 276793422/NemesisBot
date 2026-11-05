// ---------------------------------------------------------------------------
// 机械拆分：本文件内容由 board_review.rs 原样搬入（纯代码搬家，行为零变更）。
// ---------------------------------------------------------------------------

use super::*;

/// 二段验收（fire-and-forget）：gateway 回调闭包在 [`SelfcheckRegistry`]
/// 命中后调用（issue_id 由闭包 `take` 出，避免双取竞态）。worker 回报失败
/// （status=error）→ 诚实转人工；正常回报 → 证据带入二段评审
/// （`allow_selfcheck=false`：取证只带一程，二段结论直接三态处置）。
///
/// `task_id` = 本轮二段对应的取证任务（执行档案直读路径无任务传 `None`）：
/// —在途标记防 sweep 恢复臂双跑；完成（含失败臂）后清
/// 标记 + 删 `selfcheck_pending` 持久行（行生命周期 = 二段完成；完成前
/// gateway 崩溃由 sweep 重放）。
pub(crate) fn spawn_selfcheck_second_stage(
    deps: BoardReviewDeps,
    issue_id: i64,
    status: String,
    response: String,
    task_id: Option<String>,
) {
    if let Some(tid) = task_id.as_deref() {
        deps.selfcheck.mark_review_started(tid);
    }
    let store = deps.store.clone();
    let cluster = deps.cluster.clone();
    spawn_watched(store, cluster, Some(issue_id), "二段验收", async move {
        run_second_stage_review(&deps, issue_id, status, response).await;
        // 行生命周期收口：二段结束（无论三态处置还是失败臂停车）即删持久
        // 行——后续恢复臂不再重放；行删除失败 warn（sweep 臂 A 会因在途
        // 标记已清而重放一次二段，review_still_relevant 守卫兜住幂等）。
        if let Some(tid) = task_id {
            deps.selfcheck.mark_review_finished(&tid);
            if let Some(sc_store) = deps.selfcheck.store.as_ref()
                && let Err(e) = sc_store.delete_selfcheck_pending(&tid)
            {
                warn!("[BoardReview] selfcheck 持久行清理失败（task {tid}）：{e}");
            }
        }
    });
}

/// 二段验收主体（[`spawn_selfcheck_second_stage`] 的 async 内容；独立成
/// fn 让收口清理不随主体早退丢失——原 error 臂 `return` 会跳过删行）。
async fn run_second_stage_review(
    deps: &BoardReviewDeps,
    issue_id: i64,
    status: String,
    response: String,
) {
    let store = deps.store.clone();
    let cluster = deps.cluster.clone();
    if status == "error" {
        warn!(
            "[BoardReview] issue {issue_id} 自检取证回报失败（status=error）→ 转人工（评审保持 in_review）"
        );
        // U-A：取证闭环无法完成也是停车路径（worker 侧出错——重发
        // 取证或放弃自检直评都需要明确入口）。
        let handoff_note = match store.get_issue(issue_id) {
            Ok(issue) => park_with_handoff(
                deps,
                &issue.number,
                issue_id,
                nemesis_board::handoff::HandoffSpec {
                    stop_kind: "retry_exhausted",
                    suggested_action: "retry",
                    hint: "自检取证回报失败（worker 侧出错）：核查 worker 状态后 review.retrigger 重新验收（评审会按需重新取证）".into(),
                    resume_entry: "review.retrigger（number=NB-x）".into(),
                    environment: last_dispatch_env(
                        &store.list_dispatches(issue_id).unwrap_or_default(),
                    ),
                    attempted: vec![serde_json::json!({
                        "action": "selfcheck_evidence",
                        "result": nemesis_utils::truncate(response.trim(), 300),
                    })],
                },
            ),
            Err(_) => String::new(),
        };
        let _ = post_review_comment(
            &store,
            issue_id,
            &cluster,
            &format!(
                "⚠ 自检取证回报失败（worker 侧出错），取证闭环无法完成，请人工裁决或重新派发。{handoff_note}"
            ),
        );
        return;
    }
    info!("[BoardReview] issue {issue_id} 取证回报到位 → 二段验收");
    let ctx = ReviewCtx {
        allow_selfcheck: false,
        evidence: Some(response.to_string()),
        retry: 0,
    };
    match review_issue(deps, issue_id, ctx).await {
        Ok(true) => {}
        Ok(false) => {} // 跳过类结果已在 review_issue 内记日志
        Err(e) => warn!("[BoardReview] issue {issue_id} 二段验收失败：{e}（人工兜底）"),
    }
}

/// 自检取证提示词：给执行 worker 的指令——只收集证据如实回报，不下
/// 验收结论、不改已交付内容。marker 头（共享常量
/// [`nemesis_cluster::SELFCHECK_PROMPT_MARKER`]）同时是人读标记与 worker 侧
/// 队列优先通道的路由判据（压测②：取证插队不被正式任务串行堵住）。
pub(super) fn build_selfcheck_prompt(issue: &nemesis_board::Issue, request: &str) -> String {
    format!(
        "{marker}{number}] 验收方对照验收标准评审你的交付后，需要以下证据才能定案。\
请在本机完成取证并如实回报证据内容（逐项说明，含关键输出原文），不要下验收结论、不要修改已交付内容：\n\n{request}\n\n## 验收标准（取证对照）\n{ac}\n",
        marker = nemesis_cluster::SELFCHECK_PROMPT_MARKER,
        number = issue.number,
        request = request,
        ac = issue
            .acceptance_criteria
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("（未提供）"),
    )
}

/// 自检派发：向执行 worker 发 peer_chat 取证请求。与正式派发
/// （fire-and-forget）不同，这里**同步等 RPC 送达**（30s）——送不到就不
/// 挂起评审（回调永远不会来，挂起即悬挂），诚实返回 Err 转人工兜底。
/// 会话键与正式派发一致（`board:{number}`）：取证请求落在 worker 的既有
/// 任务会话里，上下文天然延续。返回 task_id（调用方注册进
/// [`SelfcheckRegistry`]）。
pub(super) async fn dispatch_selfcheck(
    deps: &BoardReviewDeps,
    issue: &nemesis_board::Issue,
    worker: &str,
    request: &str,
) -> Result<String, String> {
    let cluster = &deps.cluster;
    let prompt = build_selfcheck_prompt(issue, request);
    let source_node_id = cluster.node_id().to_string();
    let chat_id = format!("board:{}", issue.number);
    let source_payload = serde_json::json!({
        "node_id": source_node_id,
        "channel": "board",
        "chat_id": chat_id,
    });
    let task_id = cluster.submit_peer_chat(
        worker,
        "peer_chat",
        serde_json::json!({ "content": prompt, "_source": source_payload }),
        "board",
        &chat_id,
    )?;
    let rpc_client = cluster.rpc_client_arc().ok_or("RPC client not available")?;
    let request_rpc = nemesis_cluster::rpc_types::RPCRequest {
        id: task_id.clone(),
        action: nemesis_cluster::rpc_types::ActionType::Known(
            nemesis_cluster::rpc_types::KnownAction::PeerChat,
        ),
        payload: serde_json::json!({
            "content": prompt,
            "task_id": task_id,
            "_source": source_payload,
            "_source_rpc_port": cluster.rpc_port(),
        }),
        source: source_node_id,
        target: Some(worker.to_string()),
    };
    let resp = rpc_client
        .call_with_timeout(worker, request_rpc, std::time::Duration::from_secs(30))
        .await
        .map_err(|e| format!("取证 RPC 送达失败：{e}"))?;
    // BUG-1：B 端拒单（工作队列满，ACK status=error）=
    // 任务从未开始，不能当派发成功——返回 Err 走调用方既有取证失败
    // 路径，评审链不被悬挂等待一个永不会来的回调。
    if nemesis_cluster::rpc::peer_chat_handler::peer_chat_ack_rejected(resp.result.as_ref()) {
        // b：ACK error 字段（mandate 拒绝原因等）优先透传。
        let reason = resp
            .result
            .as_ref()
            .and_then(|r| r.get("error"))
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_else(|| "B 端工作队列满，任务未被接受（ACK status=error）".into());
        return Err(format!("取证派发被 worker 拒单（{reason}）"));
    }
    Ok(task_id)
}

// ---------------------------------------------------------------------------
// 自检取证路由持久层兜底 sweep
// ---------------------------------------------------------------------------

/// 臂 B not_found 宽限（秒）：worker 工作队列正常秒级消费，ACK 后超宽限
/// 仍 not_found = 任务在 worker 侧丢失（队列丢弃 / worker 重装丢盘）。
const SELFCHECK_NOT_FOUND_GRACE_SECS: i64 = 30 * 60;

/// 行绝对超龄（秒；worker 持续离线 / 轮询永不成功的兜底上限）。与集群
/// stale 安全网 24h 下限对齐——超过它 TaskManager 侧也判死，两侧一致收口。
const SELFCHECK_PENDING_MAX_AGE_SECS: i64 = 24 * 60 * 60;

/// 自检取证路由持久层兜底 sweep（gateway 派发 sweep 同 tick 调用）。
/// 内存注册表是进程态——gateway 重启后 `selfcheck_pending` 表里的在途行
/// 分两臂处理：
/// - **臂 A**（已路由 status 非空、不在本进程在途集合）：二段验收进行中
///   进程崩溃 → 重放二段（证据已烧入行内，零 worker 依赖）。
/// - **臂 B**（未路由 status NULL）：取证回调丢失（gateway 停机窗口送达
///   失败时 worker `send_callback_or_persist` 落盘保留副本）→ 式
///   `query_task_result` 轮询捞回：done → 恢复路由进二段；running →
///   继续；not_found 超宽限 / 行绝对超龄 → 诚实停车转人工。
///
/// 保险丝：estop 挂起时评审冻结（不跑 LLM、不转人工收口）——两臂同
/// 冻结，行保留等 release 后处理。
pub(crate) async fn sweep_selfcheck_pending(deps: &BoardReviewDeps) {
    sweep_selfcheck_pending_at(deps, chrono::Utc::now().timestamp()).await;
}

/// [`sweep_selfcheck_pending`] 的时钟注入形态（测试伪造 `now` 用——
/// 超龄/宽限闸按行龄判定，真实时钟测不到）。
pub(crate) async fn sweep_selfcheck_pending_at(deps: &BoardReviewDeps, now: i64) {
    if deps.estop.is_engaged() {
        return;
    }
    let rows = match deps.store.list_selfcheck_pending() {
        Ok(r) => r,
        Err(e) => {
            warn!("[BoardReview] selfcheck 持久行枚举失败：{e}");
            return;
        }
    };
    for row in rows {
        match row.status.as_deref() {
            None => selfcheck_sweep_unrouted(deps, &row, now).await,
            Some(status) => {
                if deps.selfcheck.has_inflight_review(&row.task_id) {
                    continue; // 本进程评审在途（正常流），不重放
                }
                info!(
                    "[BoardReview] issue {} 自检二段残留行重放（task {}，进程重启恢复）",
                    row.issue_id, row.task_id
                );
                spawn_selfcheck_second_stage(
                    deps.clone(),
                    row.issue_id,
                    status.to_string(),
                    row.response.clone().unwrap_or_default(),
                    Some(row.task_id.clone()),
                );
            }
        }
    }
}

/// 臂 B：未路由行（回调未到）——恢复轮询 / 超龄收口。
async fn selfcheck_sweep_unrouted(
    deps: &BoardReviewDeps,
    row: &nemesis_board::SelfcheckPendingRow,
    now: i64,
) {
    let age = now - row.created_at;
    if age > SELFCHECK_PENDING_MAX_AGE_SECS {
        selfcheck_sweep_give_up(deps, row, "自检取证超龄未回收").await;
        return;
    }
    let lost = age > SELFCHECK_NOT_FOUND_GRACE_SECS;
    if row.worker_id.is_empty() {
        // 异常行（无轮询目标）：按宽限闸收口。
        if lost {
            selfcheck_sweep_give_up(deps, row, "取证行缺 worker 身份").await;
        }
        return;
    }
    let Some(rpc_client) = deps.cluster.rpc_client_arc() else {
        if lost {
            selfcheck_sweep_give_up(deps, row, "RPC 客户端不可用").await;
        }
        return;
    };
    let request = nemesis_cluster::rpc_types::RPCRequest {
        id: row.task_id.clone(),
        action: nemesis_cluster::rpc_types::ActionType::Custom("query_task_result".to_string()),
        payload: serde_json::json!({ "task_id": row.task_id }),
        source: deps.cluster.node_id().to_string(),
        target: Some(row.worker_id.clone()),
    };
    let resp = match rpc_client
        .call_with_timeout(&row.worker_id, request, std::time::Duration::from_secs(30))
        .await
    {
        Ok(r) => r,
        Err(_) => return, // worker 暂时离线：下个 tick 再试（绝对超龄兜底）
    };
    let Some(result) = resp.result else {
        return;
    };
    match result.get("status").and_then(|v| v.as_str()).unwrap_or("") {
        "done" => {
            let result_status = result
                .get("result_status")
                .and_then(|v| v.as_str())
                .unwrap_or("error");
            let mapped = if result_status == "success" {
                "ok"
            } else {
                "error"
            };
            // ok 臂取取证正文；error 臂 error 字段优先（与回调路径
            // fail_text 合并语义一致）。
            let response_field = result
                .get("response")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let error_field = result.get("error").and_then(|v| v.as_str()).unwrap_or("");
            let text = if mapped == "ok" || error_field.is_empty() {
                response_field.to_string()
            } else {
                error_field.to_string()
            };
            info!(
                "[BoardReview] issue {} 取证回调丢失，恢复轮询捞回（task {}，{mapped}）→ 二段验收",
                row.issue_id, row.task_id
            );
            // 与回调路径同语义三连：路由落地 + TaskManager 收口 + 二段。
            if let Err(e) = deps
                .store
                .mark_selfcheck_routed(&row.task_id, mapped, &text)
            {
                warn!(
                    "[BoardReview] selfcheck 恢复路由落地失败（task {}）：{e}",
                    row.task_id
                );
            }
            let result_value = serde_json::json!({
                "status": mapped,
                "response": text,
                "source_node": row.worker_id,
            });
            if mapped == "error" {
                deps.cluster.fail_task(&row.task_id, &text);
            } else {
                deps.cluster.complete_task(&row.task_id, result_value);
            }
            spawn_selfcheck_second_stage(
                deps.clone(),
                row.issue_id,
                mapped.to_string(),
                text,
                Some(row.task_id.clone()),
            );
        }
        "running" => {} // worker 还在跑，继续等下个 tick
        _ => {
            // not_found / 未知：宽限内再等，超宽限 = worker 侧任务丢失。
            if lost {
                selfcheck_sweep_give_up(deps, row, "worker 侧无该任务结果（not_found）").await;
            }
        }
    }
}

/// 臂 B 收口：取证无法回收 → 停车登记 + 评论转人工（与回调 error 臂同
/// 形态，入口 review.retrigger）+ TaskManager 收口 + 删行。
async fn selfcheck_sweep_give_up(
    deps: &BoardReviewDeps,
    row: &nemesis_board::SelfcheckPendingRow,
    reason: &str,
) {
    warn!(
        "[BoardReview] issue {} 自检取证无法回收（task {}，{reason}）→ 转人工",
        row.issue_id, row.task_id
    );
    deps.cluster.fail_task(
        &row.task_id,
        &format!("selfcheck recovery give up: {reason}"),
    );
    let handoff_note = match deps.store.get_issue(row.issue_id) {
        Ok(issue) => park_with_handoff(
            deps,
            &issue.number,
            row.issue_id,
            nemesis_board::handoff::HandoffSpec {
                stop_kind: "retry_exhausted",
                suggested_action: "retry",
                hint: "自检取证回调丢失且无法恢复：核查 worker 状态后 review.retrigger 重新验收（评审会按需重新取证）".into(),
                resume_entry: "review.retrigger（number=NB-x）".into(),
                environment: last_dispatch_env(
                    &deps.store.list_dispatches(row.issue_id).unwrap_or_default(),
                ),
                attempted: vec![serde_json::json!({
                    "action": "selfcheck_evidence_recovery",
                    "result": reason,
                })],
            },
        ),
        Err(_) => String::new(),
    };
    let _ = post_review_comment(
        &deps.store,
        row.issue_id,
        &deps.cluster,
        &format!(
            "⚠ 自检取证回调丢失且无法恢复（{reason}），取证闭环无法完成，请人工裁决或重新验收。{handoff_note}"
        ),
    );
    let _ = deps.store.delete_selfcheck_pending(&row.task_id);
}

/// 取证软件化：执行档案直读取证。档案管线
/// （[`crate::board_archive_ingest`]）把 worker 的执行记录（`00.request.md`
/// / `NN.AI.Response.md` 等）随交付回灌安置在
/// `<项目目录>/records/<编号>/execution/<ts>/`——评审取证优先从这份本地
/// 档案直读（零 token、零 RPC、零 worker 依赖；上游 LLM 链路故障时评审
/// 取证不被卡死），档案缺失/为空才回落向 worker 发 LLM 自检。
///
/// 纯函数（只读不写）：最新一轮 ts 目录优先（重派后旧轮次不代表当前
/// 交付）；`.raw.json` 线材件跳过（体积大且人不可读），取任务请求头 +
/// 最新一轮 AI 回复头作为证据主体；总输出有界。
///
/// 清单路径形态（真机实证）：评审员照清单用
/// `board_evidence_read` 逐个读取，path 以项目档案根为基准解析
/// （[`crate::board_evidence_tool`]）——清单必须列档案根相对路径
/// （`records/<编号>/execution/<ts>/<name>`）；裸文件名会让评审员的全部
/// 读取落空（「档案文件不存在或不可达」×N）→ UNSURE×3 熔断转人工。
pub(super) fn collect_execution_evidence(
    root: &std::path::Path,
    issue_number: &str,
) -> Option<String> {
    let exec_dir = root.join("records").join(issue_number).join("execution");
    // 目录名 = 安置时间戳（字典序=时间序，同 retry_merge_for_issue 口径）。
    let mut rounds: Vec<std::path::PathBuf> = std::fs::read_dir(&exec_dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    rounds.sort();
    let dir = rounds.pop()?;

    let mut files: Vec<(String, u64)> = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let size = e.metadata().ok()?.len();
            Some((e.file_name().to_string_lossy().into_owned(), size))
        })
        .collect();
    files.sort();
    if files.is_empty() {
        return None;
    }
    let read_head = |name: &str, cap: usize| -> Option<String> {
        let raw = std::fs::read_to_string(dir.join(name)).ok()?;
        let text = raw.trim();
        // 空文件与盒内数据丢失标记都不算证据。
        (!text.is_empty() && text != "_NEMESIS_UNREADABLE_")
            .then(|| nemesis_utils::truncate(text, cap))
    };
    let request_head = read_head("00.request.md", 1200);
    // 最新一轮 AI 回复 = 序号最大的 `NN.AI.Response.md`（raw.json 不匹配后缀）。
    let response_head = files
        .iter()
        .filter_map(|(name, _)| {
            name.strip_suffix(".AI.Response.md")
                .and_then(|idx| idx.parse::<usize>().ok())
        })
        .max()
        .and_then(|idx| read_head(&format!("{idx:02}.AI.Response.md"), 4000));
    if request_head.is_none() && response_head.is_none() {
        return None;
    }
    let ts = dir.file_name()?.to_string_lossy().into_owned();
    let prefix = format!("records/{issue_number}/execution/{ts}/");
    let inventory = files
        .iter()
        .map(|(name, size)| format!("- {prefix}{name}（{size} B）"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut out = format!(
        "执行档案（最新一轮 {ts}，共 {} 个文件；清单为档案内相对路径，可用 board_evidence_read 原样读取）：\n{inventory}",
        files.len()
    );
    if let Some(r) = request_head {
        out.push_str(&format!("\n\n## 任务请求（00.request.md 头部）\n{r}"));
    }
    if let Some(r) = response_head {
        out.push_str(&format!("\n\n## worker 最终回复（AI.Response 头部）\n{r}"));
    }
    Some(nemesis_utils::truncate(&out, 7600))
}

/// 从 deps 解析单据的项目档案根并直读取证：单据未绑项目 / 项目无档案
/// 目录 / 目录被手删 → None（诚实回落 LLM 自检，旧行为不变）。
pub(super) fn gather_execution_evidence(
    deps: &BoardReviewDeps,
    issue: &nemesis_board::Issue,
) -> Option<String> {
    let project = deps.store.get_project(issue.project_id?).ok()?;
    let root = project_archive_root(project.directory.as_deref()).ok()?;
    collect_execution_evidence(&root, &issue.number)
}
