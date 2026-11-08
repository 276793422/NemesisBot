// ---------------------------------------------------------------------------
// 触点体系 P3b：原型里程碑单的机器行为闸（playtest runner + 红队）。
//
// 语义评审（review LLM 三态）通过后、收货/待人工确认之前，里程碑单
// （issue.milestone 且 quality=experience）先过机器行为闸：master 侧主持人
// LLM 出题（3~5 个测机制面场景）→ 交付 worker 试玩+红队取证（marker 任务，
// 优先通道）→ 报告 JSON 全过 = PASS。PASS 在有人值守时挂原型触点卡等人工
// 方向校验（关门不锁门）；无人值守直接记档收货（现有 auto_accept 终局，
// 不挂卡）。FAIL = 证据回灌重派（计入既有预算/熔断体系），机器闸复验取代
// 人工复验（默认不再回人工）。基础设施失败（主持人缺席/出题解析失败/派发
// 失败/worker 回报失败）→ 挂卡注明「机器闸未执行」，人工是唯一闸（与模式
// 无关——关门不锁门，mode-independent）。
//
// 取证派发不写 issue_dispatch（同 selfcheck 缺陷谱系），路由凭
// [`PlaytestRegistry`]（内存 + `playtest_pending` 持久行双写）；回调经
// gateway 拦截臂进 [`spawn_prototype_verdict`] 定案。
// ---------------------------------------------------------------------------

use super::*;

/// 单个试玩场景（主持人出题产物）：测机制面的一条可执行验证路径。
#[derive(Clone, Debug)]
pub(crate) struct PlaytestScenario {
    pub id: String,
    pub name: String,
    pub steps: Vec<String>,
    pub expect: String,
}

/// 机器闸裁决（纯数据，`run_prototype_verdict` 消费）。
#[derive(Debug)]
pub(crate) enum PlaytestVerdict {
    /// 全部场景通过。
    Pass,
    /// 有场景未通过（排序去重后的失败场景 id 清单）。
    Fail(Vec<String>),
    /// 报告不可解析（基础设施失败面：报告缺 scenarios 数组/整体非 JSON）。
    InfraFail(String),
}

/// 里程碑单机器闸目标判定：`issue.milestone` 且质量档=experience。
/// quality 在建单时继承（`confirm_plan` 对非 experience 父单已把
/// milestone 归一 false），因此无需回查父单。
pub(crate) fn is_prototype_gate_target(issue: &nemesis_board::Issue) -> bool {
    issue.milestone && issue.quality == "experience"
}

/// 主持人出题 system prompt（检查员侧出题）：产出结构化试玩场景，
/// 只测机制面（可客观判定 pass/fail），不测审美/主观体验。
const QUESTS_SYSTEM_PROMPT: &str = r#"你是看板验收的试玩出题员。针对一个「原型里程碑」交付（首个可运行构建），你唯一职责是设计 3~5 个试玩场景，用于机器自动取证验证该构建的**核心机制面**是否可玩。

要求：
- 只出可客观判定通过/失败的机制面场景（点这个按钮→预期发生 X；输入 N→预期出现 Y），不出审美/主观体验类场景。
- 场景必须可由一个 agent 在本机实际执行（运行构建、模拟操作、观察输出），不要求真实人类手感。
- 覆盖验收标准里最核心的玩法闭环（至少一个场景端到端走通主循环）。

输出：仅一个 JSON 对象，无其它文字：
{"scenarios":[{"id":"S1","name":"场景名","steps":["步骤1","步骤2"],"expect":"预期可观察结果"}]}"#;

/// 主持人出题 user prompt：原型单的上下文材料。
fn build_quests_user_prompt(issue: &nemesis_board::Issue) -> String {
    format!(
        "原型里程碑单：{}（{}）\n\n## 交付说明\n{}\n\n## 验收标准（出题对照）\n{}\n\n请给出 3~5 个试玩场景（JSON）。",
        issue.number,
        issue.title,
        issue.description.trim(),
        issue
            .acceptance_criteria
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("（未提供）"),
    )
}

/// 解析主持人出题 JSON（纯函数）：scenarios 数组 1~8 条，id 非空去重。
fn parse_playtest_quests(raw: &str) -> Result<Vec<PlaytestScenario>, String> {
    let v: serde_json::Value =
        serde_json::from_str(raw.trim()).map_err(|e| format!("出题输出不是 JSON：{e}"))?;
    let arr = v
        .get("scenarios")
        .and_then(|s| s.as_array())
        .ok_or("出题输出缺 scenarios 数组")?;
    if arr.is_empty() {
        return Err("出题输出 scenarios 为空".into());
    }
    if arr.len() > 8 {
        return Err(format!("出题输出场景数超上限（{} > 8）", arr.len()));
    }
    let mut out = Vec::with_capacity(arr.len());
    for s in arr {
        let id = s
            .get("id")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or("场景缺 id")?
            .to_string();
        if out.iter().any(|p: &PlaytestScenario| p.id == id) {
            continue; // 重复 id 丢弃（幂等去重）
        }
        out.push(PlaytestScenario {
            id,
            name: s
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            steps: s
                .get("steps")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            expect: s
                .get("expect")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        });
    }
    if out.is_empty() {
        return Err("出题输出无有效场景（全部缺 id）".into());
    }
    Ok(out)
}

/// 主持人出题：detached 纯文本调用，解析失败回灌重试 ≤2 次（共 3 轮）。
/// 调用失败（网络/上游）首轮立即返回，不烧重试轮。
async fn run_playtest_quests(
    agent_loop: &std::sync::Arc<nemesis_agent::r#loop::AgentLoop>,
    issue: &nemesis_board::Issue,
) -> Result<Vec<PlaytestScenario>, String> {
    let mut prompt = build_quests_user_prompt(issue);
    let mut last_err = String::new();
    for _ in 0..=2 {
        let opts = nemesis_agent::r#loop::DetachedOpts {
            system_prompt: Some(QUESTS_SYSTEM_PROMPT),
            no_tools: true,
            max_turns: 1,
            label: Some("board-playtest-quests"),
            ..Default::default()
        };
        let raw = match agent_loop.run_detached(&prompt, opts).await {
            Ok(raw) => raw,
            Err(e) => return Err(format!("出题 LLM 调用失败：{e}")),
        };
        if is_system_terminal_text(&raw) {
            return Err(format!("出题输出为系统终态文本（不重试防污染）：{raw}"));
        }
        match parse_playtest_quests(&raw) {
            Ok(q) => return Ok(q),
            Err(e) => {
                last_err = e;
                // 回灌：带契约要求文本让模型一次性自纠。
                prompt = format!(
                    "{prompt}\n\n## 上一轮输出无法解析\n{last_err}\n请重新输出，严格只输出一个 JSON 对象（形如 {{\"scenarios\":[{{\"id\":\"S1\",\"name\":\"…\",\"steps\":[\"…\"],\"expect\":\"…\"}}]}}），无其它文字。"
                );
            }
        }
    }
    Err(format!("出题输出连续 3 轮无法解析：{last_err}"))
}

/// 试玩取证任务提示词：给交付 worker 的指令——本机实际运行构建、执行
/// 场景清单、红队扫一轮、回报结构化 JSON。marker 头（共享常量
/// [`nemesis_cluster::PLAYTEST_PROMPT_MARKER`]）同时是人读标记与 worker 侧
/// 队列优先通道的路由判据（原型评审在 master 挂起等待，插队理由与取证
/// 同款）。
fn build_playtest_task_prompt(issue: &nemesis_board::Issue, quests: &[PlaytestScenario]) -> String {
    let mut quest_lines = String::new();
    for q in quests {
        quest_lines.push_str(&format!(
            "- {}（{}）：步骤 {}\n  预期：{}\n",
            q.id,
            q.name,
            if q.steps.is_empty() {
                "（出题方未给步骤，自行探索该机制面）".to_string()
            } else {
                q.steps.join(" → ")
            },
            if q.expect.is_empty() {
                "（未给预期，如实描述观察结果）"
            } else {
                &q.expect
            },
        ));
    }
    format!(
        "{marker}{number}] 该单是原型里程碑，已通过语义评审，现需**机器行为闸**取证：请在本机实际运行交付的构建，逐场景试玩并如实回报结果，另做一轮红队探索。不要修改已交付内容、不要下最终验收结论，只收集证据：\n\n## 试玩场景（逐条执行）\n{quests}\n## 红队探索\n以新玩家身份自由操作 5~10 分钟，记录让你困惑、卡住或与预期不符的地方（每条注 severity: high/medium/low）。\n\n## 回报格式（严格 JSON，作为回复全文）\n{{\"scenarios\":[{{\"id\":\"S1\",\"pass\":true,\"evidence\":\"实际操作与观察到的输出原文\"}}],\"redteam\":[{{\"severity\":\"low\",\"where\":\"位置\",\"confusion\":\"困惑点\"}}],\"screenshots\":[\"截图文件路径\"],\"console_tail\":\"构建/运行控制台尾部关键输出\"}}\n\n- evidence 必须是实际观察（命令输出/页面状态原文），不得凭推测写 pass。\n- 截图存到工作区并回报绝对/相对路径（取不到截图就回报空数组，不阻塞）。\n- 构建失败/无法运行 = 场景全部 pass:false，evidence 写明失败原因。",
        marker = nemesis_cluster::PLAYTEST_PROMPT_MARKER,
        number = issue.number,
        quests = quest_lines,
    )
}

/// 试玩派发：向交付 worker 发 peer_chat 取证请求（与
/// `dispatch_selfcheck` 同形态——同步等 RPC 送达 30s，送不到就不挂起，
/// 诚实返回 Err 走基础设施失败臂；B 端拒单 ACK status=error 同样 Err）。
async fn dispatch_playtest(
    deps: &BoardReviewDeps,
    issue: &nemesis_board::Issue,
    worker: &str,
    prompt: &str,
) -> Result<String, String> {
    let cluster = &deps.cluster;
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
        .map_err(|e| format!("试玩 RPC 送达失败：{e}"))?;
    if nemesis_cluster::rpc::peer_chat_handler::peer_chat_ack_rejected(resp.result.as_ref()) {
        let reason = resp
            .result
            .as_ref()
            .and_then(|r| r.get("error"))
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_else(|| "B 端工作队列满，任务未被接受（ACK status=error）".into());
        return Err(format!("试玩派发被 worker 拒单（{reason}）"));
    }
    Ok(task_id)
}

/// 报告裁决（纯函数）：不可解析 → InfraFail；全部场景 pass → Pass；
/// 任一场景 pass != true → Fail（排序去重失败 id）。
/// 红队发现（redteam）**从不**改变裁决——它们是触点卡的展示材料
/// （人工方向校验时看），不是机器闸的闸门。
fn evaluate_playtest_report(raw: &str) -> PlaytestVerdict {
    let v: serde_json::Value = match serde_json::from_str(raw.trim()) {
        Ok(v) => v,
        Err(e) => return PlaytestVerdict::InfraFail(format!("试玩报告不是 JSON：{e}")),
    };
    let Some(arr) = v.get("scenarios").and_then(|s| s.as_array()) else {
        return PlaytestVerdict::InfraFail("试玩报告缺 scenarios 数组".into());
    };
    if arr.is_empty() {
        return PlaytestVerdict::InfraFail("试玩报告 scenarios 为空".into());
    }
    let mut failing: Vec<String> = Vec::new();
    for s in arr {
        let pass = s.get("pass").and_then(|p| p.as_bool()).unwrap_or(false);
        if !pass {
            let id = s
                .get("id")
                .and_then(|i| i.as_str())
                .unwrap_or("unknown")
                .to_string();
            failing.push(id);
        }
    }
    if failing.is_empty() {
        return PlaytestVerdict::Pass;
    }
    failing.sort();
    failing.dedup();
    PlaytestVerdict::Fail(failing)
}

/// 原型机器行为闸主入口（review_issue 在 AutoAccept/SuggestManual 分流点
/// 调用）。返回 Ok(true) = 本轮评审周期已被闸消费（试玩派发挂起 / 卡已挂 /
/// 在途等待）；Ok(false) = 闸让位（在途触点卡待人工——常规收货臂继续，
/// settle 联动会收口卡片）。
pub(crate) async fn run_prototype_gate(
    deps: &BoardReviewDeps,
    issue_id: i64,
    round: u64,
) -> Result<bool, String> {
    let store = deps.store.clone();
    // estop 保险丝：挂起即冻结（同 AutoAccept 臂检查点形态）。
    if estop_fuse_engaged(deps, issue_id, ParkedKind::Issue) {
        return Ok(false);
    }
    let issue = store.get_issue(issue_id)?;
    // 已有待决原型触点卡（上一轮挂的）→ 闸让位：常规臂继续收货/待人工，
    // settle 联动会收口卡片；这里不再叠一层机器闸。
    if let Ok(Some(tp)) = store.get_latest_touchpoint_by_issue(issue_id)
        && tp.kind == nemesis_board::TouchpointKind::Prototype
        && tp.status == nemesis_board::TouchpointStatus::Pending
    {
        info!("[BoardReview] issue {issue_id} 原型触点卡待人工，机器闸让位（常规臂继续）");
        return Ok(false);
    }
    // 试玩在途（防重入）→ 消费本轮，等在途定案。
    if deps.playtest.has_inflight(issue_id) {
        info!("[BoardReview] issue {issue_id} 试玩取证在途，评审周期消费等待定案");
        return Ok(true);
    }
    // 主持人 LLM 缺席 = 基础设施失败面。
    let Some(moderator) = deps.moderator_loop.read().clone() else {
        return gate_infra_arm(deps, &issue, "主持人 LLM 未装配，机器闸未执行").await;
    };
    // 出题（解析重试 ×3）。
    let quests = match run_playtest_quests(&moderator, &issue).await {
        Ok(q) => q,
        Err(e) => return gate_infra_arm(deps, &issue, &format!("试玩出题失败：{e}")).await,
    };
    // 取证目标 = 最新派发 worker（里程碑单必然经派发到达评审；无派发史
    // = 装配残缺，诚实走基础设施失败臂）。
    let dispatches = store.list_dispatches(issue_id)?;
    let worker = dispatches
        .last()
        .map(|d| d.worker_id.clone())
        .filter(|w| !w.trim().is_empty())
        .ok_or_else(|| "无派发历史，试玩取证无目标".to_string())?;
    let prompt = build_playtest_task_prompt(&issue, &quests);
    let task_id = match dispatch_playtest(deps, &issue, &worker, &prompt).await {
        Ok(t) => t,
        Err(e) => {
            return gate_infra_arm(deps, &issue, &format!("试玩派发失败：{e}")).await;
        }
    };
    deps.playtest.register(task_id.clone(), issue_id, &worker);
    let reviewer = Actor::agent(deps.cluster.node_id());
    store.add_comment(NewComment {
        issue_id,
        author: reviewer.clone(),
        content: format!(
            "🎮 原型机器行为闸：试玩+红队取证已派发至 {worker}（{} 个场景），评审在此挂起等取证回报（任务 {task_id}）。",
            quests.len()
        ),
        parent_id: None,
        ctype: CommentType::System,
    })?;
    record_auto_decide(
        deps,
        issue_id,
        "prototype_gate",
        "dispatched",
        serde_json::json!({
            "round": round,
            "worker": worker,
            "task_id": task_id,
            "quests": quests.iter().map(|q| q.id.clone()).collect::<Vec<_>>(),
        }),
    );
    info!(
        "[BoardReview] issue {issue_id} 机器行为闸：试玩取证已派发（task {task_id} → {worker}），评审挂起"
    );
    Ok(true)
}

/// 基础设施失败臂（mode-independent）：挂原型触点卡注明「机器闸未执行」，
/// 人工是唯一闸。有人值守：卡等人工处理；无人值守：超时 sweep 走
/// timeout_default 默认动作。评审保持 in_review。
async fn gate_infra_arm(
    deps: &BoardReviewDeps,
    issue: &nemesis_board::Issue,
    note: &str,
) -> Result<bool, String> {
    let issue_id = issue.id;
    warn!("[BoardReview] issue {issue_id} 机器行为闸基础设施失败 → 挂卡转人工（{note}）");
    let payload = serde_json::json!({
        "version": 1,
        "machine_gate": "infra_fail",
        "note": note,
    });
    let now_ms = chrono::Utc::now().timestamp_millis();
    let cfg = nemesis_web::handlers::board::touchpoints::live_attendance_config();
    let deadline = nemesis_web::handlers::board::touchpoints::deadline_from_cfg(
        now_ms,
        &cfg,
        nemesis_board::TouchpointKind::Prototype,
    );
    let reviewer = Actor::agent(deps.cluster.node_id());
    nemesis_web::handlers::board::touchpoints::create_touchpoint_card(
        &deps.store,
        &reviewer,
        issue_id,
        issue.project_id,
        nemesis_board::TouchpointKind::Prototype,
        &payload.to_string(),
        deadline,
    )?;
    deps.store.add_comment(NewComment {
        issue_id,
        author: Actor::system("board-review"),
        content: format!("⚠ 机器闸未执行（{note}）：人工是唯一闸，请到工作台处理原型触点卡。"),
        parent_id: None,
        ctype: CommentType::System,
    })?;
    record_auto_decide(
        deps,
        issue_id,
        "prototype_gate",
        "infra_fail",
        serde_json::json!({ "note": nemesis_utils::string_utils::truncate(note, 300) }),
    );
    Ok(true)
}

// ---------------------------------------------------------------------------
// 定案臂（worker 回调 → gateway 拦截臂 / sweep 恢复臂进入）
// ---------------------------------------------------------------------------

/// 试玩定案（fire-and-forget）：gateway 回调闭包在 [`PlaytestRegistry`]
/// 命中后调用（issue_id 由闭包 `take` 出，避免双取竞态）。形态与
/// `spawn_selfcheck_second_stage` 同源：在途标记防 sweep 双跑；完成（含
/// 失败臂）后清标记 + 删 `playtest_pending` 持久行。
pub(crate) fn spawn_prototype_verdict(
    deps: BoardReviewDeps,
    issue_id: i64,
    status: String,
    response: String,
    task_id: Option<String>,
) {
    if let Some(tid) = task_id.as_deref() {
        deps.playtest.mark_verdict_started(tid);
    }
    let store = deps.store.clone();
    let cluster = deps.cluster.clone();
    spawn_watched(store, cluster, Some(issue_id), "试玩定案", async move {
        run_prototype_verdict(&deps, issue_id, status, response).await;
        if let Some(tid) = task_id {
            deps.playtest.mark_verdict_finished(&tid);
            if let Some(pt_store) = deps.playtest.store.as_ref()
                && let Err(e) = pt_store.delete_playtest_pending(&tid)
            {
                warn!("[BoardReview] playtest 持久行清理失败（task {tid}）：{e}");
            }
        }
    });
}

/// 定案主体（[`spawn_prototype_verdict`] 的 async 内容；独立成 fn 让收口
/// 清理不随主体早退丢失）。
async fn run_prototype_verdict(
    deps: &BoardReviewDeps,
    issue_id: i64,
    status: String,
    response: String,
) {
    let store = deps.store.clone();
    let Ok(issue) = store.get_issue(issue_id) else {
        warn!("[BoardReview] issue {issue_id} 试玩定案：单据读取失败，放弃定案（卡片/人工兜底）");
        return;
    };
    // worker 侧回报失败 = 基础设施失败面（不是原型失败，不重派 worker）。
    if status == "error" {
        let note = format!(
            "试玩取证回报失败（worker 侧出错）：{}",
            nemesis_utils::string_utils::truncate(response.trim(), 300)
        );
        let _ = gate_infra_arm(deps, &issue, &note).await;
        return;
    }
    match evaluate_playtest_report(&response) {
        PlaytestVerdict::InfraFail(reason) => {
            let _ = gate_infra_arm(deps, &issue, &reason).await;
        }
        PlaytestVerdict::Pass => gate_pass_arm(deps, &issue, &response).await,
        PlaytestVerdict::Fail(failing) => gate_fail_arm(deps, &issue, &failing, &response).await,
    }
}

/// 机器闸报告摘要（评审链评论用）：逐场景 pass + 红队条数 + 控制台尾部。
fn render_gate_report_brief(raw: &str) -> String {
    let mut out = String::new();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw.trim()) {
        if let Some(arr) = v.get("scenarios").and_then(|s| s.as_array()) {
            for s in arr {
                let id = s.get("id").and_then(|i| i.as_str()).unwrap_or("?");
                let pass = s.get("pass").and_then(|p| p.as_bool()).unwrap_or(false);
                let evidence = s
                    .get("evidence")
                    .and_then(|e| e.as_str())
                    .unwrap_or("")
                    .trim();
                out.push_str(&format!(
                    "- {} {}：{}\n",
                    id,
                    if pass { "✅" } else { "❌" },
                    nemesis_utils::string_utils::truncate(evidence, 200)
                ));
            }
        }
        if let Some(rt) = v.get("redteam").and_then(|r| r.as_array()) {
            out.push_str(&format!("\n红队发现 {} 条（详见触点卡）。\n", rt.len()));
        }
        if let Some(ct) = v.get("console_tail").and_then(|c| c.as_str())
            && !ct.trim().is_empty()
        {
            out.push_str(&format!(
                "\n控制台尾部：{}\n",
                nemesis_utils::string_utils::truncate(ct.trim(), 400)
            ));
        }
    } else {
        out.push_str(&nemesis_utils::string_utils::truncate(raw.trim(), 800));
    }
    out
}

/// PASS 臂：有人值守 → 挂原型触点卡（machine_gate:"pass"，载荷带场景
/// 清单/试玩报告/红队发现/截图/控制台尾部）保持 in_review 等人工方向校验；
/// 无人值守 → 记档后直接收货（现有 auto_accept 终局 + settle 联动，不挂卡，
/// 计划 §unattended）；无人值守 + 最近否决带「改完再看」复验诉求 → 例外
/// 不直接收货，重挂带 recheck_review 标记的卡让人工追加一次复验。
async fn gate_pass_arm(deps: &BoardReviewDeps, issue: &nemesis_board::Issue, raw: &str) {
    let issue_id = issue.id;
    let store = deps.store.clone();
    let reviewer = Actor::agent(deps.cluster.node_id());
    let brief = render_gate_report_brief(raw);
    // 机器闸通过 = 此前语义评审失败签名全部过时（issue 级清账）。
    let mut fail_streaks = nemesis_board::fail_streak::FailStreakLedger::load(&deps.workspace);
    fail_streaks.clear(issue_id);
    let unattended = nemesis_web::handlers::board::touchpoints::resolve_unattended(
        &store,
        issue,
        &nemesis_web::handlers::board::touchpoints::live_attendance_config(),
    )
    .unwrap_or(false);
    // 复验诉求（latest-veto-wins）：最近一次原型局部否决带 recheck（「改
    // 完再看」）→ 无人值守也不直接收货，重挂带 recheck_review 标记的卡
    // 让人工追加一次复验；之后一次不带 recheck 的否决自然关闭该诉求。
    let recheck = unattended
        && nemesis_web::handlers::board::touchpoints::latest_rework_recheck(&store, issue_id);
    let mut payload = serde_json::json!({
        "version": 1,
        "machine_gate": "pass",
        "report": raw,
        "summary": brief,
    });
    if recheck {
        // 复验标记（前端徽标 + 评审链溯源：这张卡是「改完再看」的追加
        // 人工复验，不是首轮方向校验）。
        payload["recheck_review"] = serde_json::json!(true);
    }
    if unattended && !recheck {
        // 触点照常记档并按默认收口（无人值守是默认态，全程留痕——汇报
        // 包的行为闸/迷路章节读它）。失败只 warn：记档是可观测面，收货
        // 不被它绑架。
        match nemesis_web::handlers::board::touchpoints::create_and_default_touchpoint(
            &store,
            &reviewer,
            issue_id,
            issue.project_id,
            nemesis_board::TouchpointKind::Prototype,
            &payload.to_string(),
        ) {
            Ok(tp) => info!(
                "[BoardReview] issue {issue_id} 机器闸 PASS（无人值守）触点 #{} 已按默认收口记档",
                tp.id
            ),
            Err(e) => warn!("[BoardReview] issue {issue_id} 无人值守触点记档失败：{e}（照常收货）"),
        }
        let _ = store.add_comment(NewComment {
            issue_id,
            author: reviewer.clone(),
            content: format!("🤖 原型机器行为闸通过（无人值守模式：不挂卡，直接收货）\n\n{brief}"),
            parent_id: None,
            ctype: CommentType::System,
        });
        let reasons = vec!["原型机器行为闸：全部试玩场景通过（无人值守自动收货）".to_string()];
        match auto_accept_and_settle(deps, issue_id, &reasons) {
            Ok(()) => {
                record_auto_decide(
                    deps,
                    issue_id,
                    "prototype_gate",
                    "pass_settled",
                    serde_json::json!({ "mode": "unattended" }),
                );
                info!("[BoardReview] issue {issue_id} 机器闸 PASS（无人值守）→ 直接收货");
            }
            Err(e) => warn!("[BoardReview] issue {issue_id} 机器闸 PASS 收货失败：{e}（人工兜底）"),
        }
        return;
    }
    let now_ms = chrono::Utc::now().timestamp_millis();
    let cfg = nemesis_web::handlers::board::touchpoints::live_attendance_config();
    let deadline = nemesis_web::handlers::board::touchpoints::deadline_from_cfg(
        now_ms,
        &cfg,
        nemesis_board::TouchpointKind::Prototype,
    );
    let hang = nemesis_web::handlers::board::touchpoints::create_touchpoint_card(
        &store,
        &reviewer,
        issue_id,
        issue.project_id,
        nemesis_board::TouchpointKind::Prototype,
        &payload.to_string(),
        deadline,
    );
    match hang {
        Ok(tp) => {
            if recheck {
                let _ = store.add_comment(NewComment {
                    issue_id,
                    author: reviewer.clone(),
                    content: format!(
                        "🔁 原型机器行为闸复验通过（人工此前要求「改完再看」）：重挂触点卡 #{} 追加一次人工复验\n\n{brief}",
                        tp.id
                    ),
                    parent_id: None,
                    ctype: CommentType::System,
                });
            }
            let (decision, mode) = if recheck {
                ("pass_card_recheck", "unattended_recheck")
            } else {
                ("pass_card", "attended")
            };
            record_auto_decide(
                deps,
                issue_id,
                "prototype_gate",
                decision,
                serde_json::json!({ "touchpoint_id": tp.id, "mode": mode }),
            );
            if recheck {
                info!(
                    "[BoardReview] issue {issue_id} 机器闸 PASS（无人值守·复验）→ 重挂复验卡 #{} 等人工复验",
                    tp.id
                );
            } else {
                info!(
                    "[BoardReview] issue {issue_id} 机器闸 PASS（有人值守）→ 挂原型触点卡 #{} 等人工方向校验",
                    tp.id
                );
            }
        }
        Err(e) => {
            warn!(
                "[BoardReview] issue {issue_id} 原型触点卡挂卡失败：{e} → 评论降级（人工看板评论裁决）"
            );
            let _ = store.add_comment(NewComment {
                issue_id,
                author: reviewer.clone(),
                content: format!(
                    "🎯 原型机器行为闸通过，请人工确认方向（触点卡挂卡失败，本评论即卡）：\n\n{brief}"
                ),
                parent_id: None,
                ctype: CommentType::System,
            });
        }
    }
}

/// FAIL 臂：报告证据落评论 → 同因熔断账本（签名=失败场景 id 集合）→
/// 熔断停车转人工；未熔断走重派（estop 检查点 / 预算保险丝 / 换节点 /
/// 并发让位 / 发车分臂与既有 FAIL 重派同源），机器闸复验取代人工复验。
async fn gate_fail_arm(
    deps: &BoardReviewDeps,
    issue: &nemesis_board::Issue,
    failing: &[String],
    raw: &str,
) {
    let issue_id = issue.id;
    let store = deps.store.clone();
    let reviewer = Actor::agent(deps.cluster.node_id());
    let human_mention = if issue.creator.kind == "admin" {
        format!("@{} ", issue.creator.id)
    } else {
        String::new()
    };
    let brief = render_gate_report_brief(raw);
    let gap = format!(
        "机器行为闸未通过（失败场景：{}）\n\n{brief}",
        failing.join(", ")
    );
    let _ = store.add_comment(NewComment {
        issue_id,
        author: reviewer.clone(),
        content: format!("🎮❌ 原型机器行为闸未通过\n\n{gap}"),
        parent_id: None,
        ctype: CommentType::Comment,
    });
    record_auto_decide(
        deps,
        issue_id,
        "prototype_gate",
        "fail",
        serde_json::json!({ "failing": failing }),
    );
    // 同因熔断（签名 = 排序失败场景集合；同一批场景反复挂 → 停车转人工，
    // 无机械进展的重派授权到此为止）。
    let signature = format!("prototype_gate:{}", failing.join(","));
    let mut fail_streaks = nemesis_board::fail_streak::FailStreakLedger::load(&deps.workspace);
    if let nemesis_board::fail_streak::StreakOutcome::Tripped(n) =
        fail_streaks.record_and_check(issue_id, &signature)
    {
        fail_streaks.clear(issue_id);
        let dispatches = store.list_dispatches(issue_id).unwrap_or_default();
        let handoff_note = park_with_handoff(
            deps,
            &issue.number,
            issue_id,
            nemesis_board::handoff::HandoffSpec {
                stop_kind: "prototype_gate_streak",
                suggested_action: "manual_check",
                hint: format!(
                    "机器行为闸同一失败签名连续 {n} 轮（失败场景：{}）：试玩复验无收敛，人工上手试玩定位后复评或手动重派",
                    failing.join(", ")
                ),
                resume_entry: "review.retrigger（number=NB-x，人工处理后复评）".into(),
                environment: last_dispatch_env(&dispatches),
                attempted: vec![serde_json::json!({
                    "action": "prototype_gate_redispatch",
                    "result": format!("同因熔断：{signature} 连续 {n} 轮"),
                })],
            },
        );
        let comment = format!(
            "{}⛔ 原型机器行为闸同因熔断：同一批失败场景连续 {n} 轮未通过，已停止重派，请人工上手试玩后裁决。\n{handoff_note}",
            human_mention
        );
        let _ = store.add_comment(NewComment {
            issue_id,
            author: reviewer.clone(),
            content: comment,
            parent_id: None,
            ctype: CommentType::Comment,
        });
        record_auto_decide(
            deps,
            issue_id,
            "escalate_human",
            "prototype_gate_fail_streak",
            serde_json::json!({ "signature": signature, "streak": n }),
        );
        warn!("[BoardReview] issue {issue_id} 机器闸同因熔断（{signature} ×{n}）→ 转人工");
        return;
    }
    // estop 检查点：发车前最后一闸。
    if estop_fuse_engaged(deps, issue_id, ParkedKind::Issue) {
        return;
    }
    // 预算保险丝（重派前）：与既有 FAIL 重派同源。
    let cfg = match load_board_flags(&deps.home) {
        Ok(c) => c,
        Err(e) => {
            warn!("[BoardReview] issue {issue_id} 机器闸重派读配置失败：{e} → 挂卡转人工");
            let _ = gate_infra_arm(deps, issue, &format!("机器闸重派前置读配置失败：{e}")).await;
            return;
        }
    };
    let unlimited = cfg.unlimited_mode;
    if let Some(breach) = budget_breach(deps, issue, &cfg) {
        if unlimited {
            warn!(
                "[BoardReview] issue {issue_id} 机器闸重派预算超限（unlimited_mode 仅告警继续）：{breach}"
            );
        } else {
            let _ = store.add_comment(NewComment {
                issue_id,
                author: reviewer.clone(),
                content: format!(
                    "{}🛑 机器闸重派预算超限，停止自动重派，请人工裁决。\n\n超限项：{breach}",
                    human_mention
                ),
                parent_id: None,
                ctype: CommentType::Comment,
            });
            record_auto_decide(
                deps,
                issue_id,
                "escalate_human",
                "prototype_gate_budget",
                serde_json::json!({ "breach": breach }),
            );
            return;
        }
    }
    let dispatches = store.list_dispatches(issue_id).unwrap_or_default();
    let target = match pick_redispatch_target(deps, issue, &dispatches) {
        Ok(c) => c.into_target(),
        Err(e) => {
            let msg = format!("⛔ 机器闸未通过，确定重派目标失败：{e}（请人工派发或处置）");
            let _ = store.add_comment(NewComment {
                issue_id,
                author: Actor::system("board-review"),
                content: msg,
                parent_id: None,
                ctype: CommentType::System,
            });
            record_auto_decide(
                deps,
                issue_id,
                "escalate_human",
                "prototype_gate_target_pick_failed",
                serde_json::json!({ "error": e }),
            );
            return;
        }
    };
    if store.has_active_dispatch(issue_id).unwrap_or(false) {
        info!("[BoardReview] issue {issue_id} 机器闸 FAIL 复核期间已有在途派发接管，重派让位");
        return;
    }
    let dispatch_result = if target.eq_ignore_ascii_case("self") {
        nemesis_web::handlers::board::dispatch_issue_local(&store, issue_id, &reviewer, Some(&gap))
    } else {
        nemesis_web::handlers::board::dispatch_issue_core(
            &store,
            Some(&deps.cluster),
            issue_id,
            &target,
            &reviewer,
            Some(&gap),
        )
    };
    match dispatch_result {
        Ok(_) => {
            info!(
                "[BoardReview] issue {issue_id} 机器闸 FAIL → 证据回灌重派 → {target}（机器闸复验）"
            );
        }
        Err(e) => {
            // 发车失败三分诊（与既有 FAIL 重派 Err 臂同纪律）。
            if deps.estop.is_engaged() {
                warn!("[BoardReview] issue {issue_id} 机器闸重派失败（estop 挂起）：{e}");
                return;
            }
            if e.contains("已有在途派发") || e.contains("并发") {
                info!("[BoardReview] issue {issue_id} 机器闸重派并发让位：{e}");
                return;
            }
            warn!("[BoardReview] issue {issue_id} 机器闸重派失败：{e} → 挂卡转人工");
            let _ = gate_infra_arm(
                deps,
                issue,
                &format!(
                    "机器闸重派发车失败：{}",
                    nemesis_utils::string_utils::truncate(&e, 300)
                ),
            )
            .await;
        }
    }
}

// ---------------------------------------------------------------------------
// playtest_pending 兜底 sweep（gateway 派发 sweep 同 tick 调用；与
// sweep_selfcheck_pending 同构两臂）
// ---------------------------------------------------------------------------

/// 臂 B not_found 宽限（秒）：worker 工作队列正常秒级消费，ACK 后超宽限
/// 仍 not_found = 任务在 worker 侧丢失。
const PLAYTEST_NOT_FOUND_GRACE_SECS: i64 = 30 * 60;

/// 行绝对超龄（秒；worker 持续离线 / 轮询永不成功的兜底上限）。
const PLAYTEST_PENDING_MAX_AGE_SECS: i64 = 24 * 60 * 60;

/// 试玩取证路由持久层兜底 sweep（gateway 派发 sweep 同 tick 调用）。
/// - **臂 A**（已路由、不在本进程定案在途集合）：定案进行中进程崩溃 →
///   重放定案（报告已烧入行内，零 worker 依赖）。
/// - **臂 B**（未路由）：试玩回调丢失 → `query_task_result` 轮询捞回：
///   done → 恢复路由进定案；running → 继续；not_found 超宽限 / 行绝对
///   超龄 → 诚实收口（基础设施失败臂挂卡，人工兜底）。
///
/// 保险丝：estop 挂起时冻结，行保留等 release 后处理。
pub(crate) async fn sweep_playtest_pending(deps: &BoardReviewDeps) {
    sweep_playtest_pending_at(deps, chrono::Utc::now().timestamp()).await;
}

/// [`sweep_playtest_pending`] 的时钟注入形态（测试伪造 `now` 用）。
pub(crate) async fn sweep_playtest_pending_at(deps: &BoardReviewDeps, now: i64) {
    if deps.estop.is_engaged() {
        return;
    }
    let rows = match deps.store.list_playtest_pending() {
        Ok(r) => r,
        Err(e) => {
            warn!("[BoardReview] playtest 持久行枚举失败：{e}");
            return;
        }
    };
    for row in rows {
        match row.status.as_deref() {
            None => playtest_sweep_unrouted(deps, &row, now).await,
            Some(status) => {
                if deps.playtest.has_inflight_verdict(&row.task_id) {
                    continue; // 本进程定案在途（正常流），不重放
                }
                info!(
                    "[BoardReview] issue {} 试玩定案残留行重放（task {}，进程重启恢复）",
                    row.issue_id, row.task_id
                );
                spawn_prototype_verdict(
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
async fn playtest_sweep_unrouted(
    deps: &BoardReviewDeps,
    row: &nemesis_board::PlaytestPendingRow,
    now: i64,
) {
    let age = now - row.created_at;
    if age > PLAYTEST_PENDING_MAX_AGE_SECS {
        playtest_sweep_give_up(deps, row, "试玩取证超龄未回收").await;
        return;
    }
    let lost = age > PLAYTEST_NOT_FOUND_GRACE_SECS;
    if row.worker_id.is_empty() {
        if lost {
            playtest_sweep_give_up(deps, row, "试玩行缺 worker 身份").await;
        }
        return;
    }
    let Some(rpc_client) = deps.cluster.rpc_client_arc() else {
        if lost {
            playtest_sweep_give_up(deps, row, "RPC 客户端不可用").await;
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
                "[BoardReview] issue {} 试玩回调丢失，恢复轮询捞回（task {}，{mapped}）→ 定案臂",
                row.issue_id, row.task_id
            );
            if let Err(e) = deps.store.mark_playtest_routed(&row.task_id, mapped, &text) {
                warn!(
                    "[BoardReview] playtest 恢复路由落地失败（task {}）：{e}",
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
            spawn_prototype_verdict(
                deps.clone(),
                row.issue_id,
                mapped.to_string(),
                text,
                Some(row.task_id.clone()),
            );
        }
        "running" => {} // worker 还在跑，继续等下个 tick
        _ => {
            if lost {
                playtest_sweep_give_up(deps, row, "worker 侧无该任务结果（not_found）").await;
            }
        }
    }
}

/// 臂 B 收口：试玩无法回收 → 基础设施失败臂挂卡（机器闸未执行，人工兜底）
/// + TaskManager 收口 + 删行。
async fn playtest_sweep_give_up(
    deps: &BoardReviewDeps,
    row: &nemesis_board::PlaytestPendingRow,
    reason: &str,
) {
    warn!(
        "[BoardReview] issue {} 试玩取证无法回收（task {}，{reason}）→ 挂卡转人工",
        row.issue_id, row.task_id
    );
    deps.cluster.fail_task(
        &row.task_id,
        &format!("playtest recovery give up: {reason}"),
    );
    if let Ok(issue) = deps.store.get_issue(row.issue_id) {
        let _ = gate_infra_arm(
            deps,
            &issue,
            &format!("试玩取证回调丢失且无法恢复（{reason}）"),
        )
        .await;
    }
    let _ = deps.store.delete_playtest_pending(&row.task_id);
}

// ---------------------------------------------------------------------------
// 单测（纯函数面：出题解析 / 任务提示词 / 报告裁决）
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "prototype_gate/tests.rs"]
mod tests;
