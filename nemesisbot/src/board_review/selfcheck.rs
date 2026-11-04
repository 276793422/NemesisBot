// ---------------------------------------------------------------------------
// 2026-11-05 机械拆分：本文件内容由 board_review.rs 原样搬入（纯代码搬家，行为零变更）。
// ---------------------------------------------------------------------------

use super::*;

/// B2b 二段验收（fire-and-forget）：gateway 回调闭包在 [`SelfcheckRegistry`]
/// 命中后调用（issue_id 由闭包 `take` 出，避免双取竞态）。worker 回报失败
/// （status=error）→ 诚实转人工；正常回报 → 证据带入二段评审
/// （`allow_selfcheck=false`：取证只带一程，二段结论直接三态处置）。
pub(crate) fn spawn_selfcheck_second_stage(
    deps: BoardReviewDeps,
    issue_id: i64,
    status: String,
    response: String,
) {
    let store = deps.store.clone();
    let cluster = deps.cluster.clone();
    spawn_watched(store, cluster, Some(issue_id), "二段验收", async move {
        if status == "error" {
            warn!(
                "[BoardReview] issue {issue_id} 自检取证回报失败（status=error）→ 转人工（评审保持 in_review）"
            );
            // U-A：取证闭环无法完成也是停车路径（worker 侧出错——重发
            // 取证或放弃自检直评都需要明确入口）。
            let handoff_note = match deps.store.get_issue(issue_id) {
                Ok(issue) => park_with_handoff(
                    &deps,
                    &issue.number,
                    issue_id,
                    nemesis_board::handoff::HandoffSpec {
                        stop_kind: "retry_exhausted",
                        suggested_action: "retry",
                        hint: "自检取证回报失败（worker 侧出错）：核查 worker 状态后 review.retrigger 重新验收（评审会按需重新取证）".into(),
                        resume_entry: "review.retrigger（number=NB-x）".into(),
                        environment: last_dispatch_env(
                            &deps.store.list_dispatches(issue_id).unwrap_or_default(),
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
                &deps.store,
                issue_id,
                &deps.cluster,
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
        match review_issue(&deps, issue_id, ctx).await {
            Ok(true) => {}
            Ok(false) => {} // 跳过类结果已在 review_issue 内记日志
            Err(e) => warn!("[BoardReview] issue {issue_id} 二段验收失败：{e}（人工兜底）"),
        }
    });
}

/// B2b 自检取证提示词：给执行 worker 的指令——只收集证据如实回报，不下
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

/// B2b 自检派发：向执行 worker 发 peer_chat 取证请求。与正式派发
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
    // BUG-1（2026-10-17）：B 端拒单（工作队列满，ACK status=error）=
    // 任务从未开始，不能当派发成功——返回 Err 走调用方既有取证失败
    // 路径，评审链不被悬挂等待一个永不会来的回调。
    if nemesis_cluster::rpc::peer_chat_handler::peer_chat_ack_rejected(resp.result.as_ref()) {
        // L13-b：ACK error 字段（mandate 拒绝原因等）优先透传。
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

/// 取证软件化（2026-10-15）：执行档案直读取证。档案管线
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
/// 清单路径形态（NB-105 真机实证 2026-10-15）：评审员照清单用
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
