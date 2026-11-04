// ---------------------------------------------------------------------------
// 2026-11-05 机械拆分：本文件内容由 board_review.rs 原样搬入（纯代码搬家，行为零变更）。
// ---------------------------------------------------------------------------

use super::*;

/// 父单收口汇总验收：输入=父单目标/验收标准 + 全部子单状态与最新交付摘要
/// （P1 阶段 LLM 汇总单判；P2 锚点检查器建成后自动升级双检）。三态：
/// PASS → done（auto_close_parent 本身即收货授权，评论注明）；FAIL/UNSURE
/// → 先试自动返工承接（差距/证据缺口可派发时，见 try_parent_remediation）
/// ——承接被拒（预算用满/配置关/双空无差距）才转人工保持 in_review。
/// 循环升级防抖：升级停车后人工未表态期间钩子复评一律跳过（出口 =
/// review.retrigger 打回重开）。返回 `Ok(false)` = 未评审即跳过。
pub(super) async fn review_parent_issue(
    deps: &BoardReviewDeps,
    parent_id: i64,
) -> Result<bool, String> {
    // estop 保险丝检查点 ①：父单收口同样是自动 agent 活动，挂起即冻结。
    if estop_fuse_engaged(deps, parent_id, ParkedKind::Parent) {
        return Ok(false);
    }
    let cfg = load_board_flags(&deps.home)?;
    // 双保险（hook 已读一次）：配置读失败/关闭一律 fail-closed 回人工。
    if !cfg.auto_close_parent || !cfg.auto_review {
        return Ok(false);
    }

    let store = &deps.store;
    let parent = store.get_issue(parent_id)?;
    // 只有 in_review 才收口评审：与人工操作竞态时让位人工，不覆盖。
    if parent.status != IssueStatus::InReview {
        return Ok(false);
    }
    // 循环升级防抖（2026-10-16 用户裁决：除崩溃性问题外不让用户反复验收）：
    // 最近一条 auto_decide = parent_escalate_human 且其后无任何 admin 活动
    // = 已停车等人工、人工尚未表态——子单噪音（评论/善后事件）触发的钩子
    // 复评一律跳过。此前每次钩子触发都重跑收口评审再升级一次（NB-101 实证
    // 20:45/20:50/22:01 三连升级），就是用户看到的「循环出错、循环人工」。
    // 出口 = 人工 review.retrigger（打回：先落 admin 评论再发车，此处放行
    // + 返工预算重置，自动验收重新开始）或人工直接处置状态。
    if let Some((esc_id, decision)) = store.latest_auto_decide(parent_id)?
        && decision == "parent_escalate_human"
        && !store.has_admin_activity_after(parent_id, esc_id)?
    {
        info!(
            "[BoardReview] 父单 {parent_id} 已停车等人工裁决（升级在案、人工未表态），跳过钩子复评防循环升级"
        );
        return Ok(false);
    }
    // B1（职能→评审预设查表）：收口评审参数按父单职能解析（父单通常无
    // 职能 = 全局值，行为不变）。
    let eff_review = cfg
        .review
        .resolve_for_profession(parent.required_profession.as_deref());
    // U-B（§9.2）：LLM 端点不可用封存闸——父单收口挂起（pending=review，
    // resume 走 spawn_board_review → review_issue 分流回本函数）。
    if nemesis_web::handlers::board_unavailable::frozen_now() {
        nemesis_web::handlers::board_unavailable::write_interruption(
            &deps.workspace,
            store,
            &parent,
            "review",
            "父单收口评审挂起：LLM 端点不可用封存中，恢复后探针自动续评".to_string(),
            serde_json::json!({}),
        );
        warn!("[BoardReview] 父单 {parent_id} 收口挂起：LLM 端点不可用封存中（恢复后自动续评）");
        return Ok(false);
    }
    // AOR 批二b（⑥ FINISH 谓词·入口面）：环境干净三条件（③ 活跃派发 /
    // ④ 未决 unknown / ⑤ cancelled 缺口闸——原 cancelled 检查并入谓词，语义
    // 不变）。①②（终态闭包/fail_class）刻意不在入口：in-flight 子单在场时
    // 缺陷闸（M8 停车泊车）/ 父单分流 / FAIL 返工承接链必须照常跑，谓词在
    // 入口挡 ①② 会漏停车。完整五条件在 Pass 臂落 done 前终检（收口面）。
    // 不满足 → 缺口评论显式列出（C-F4 去重）+ 不自动收口（转人工）。
    let subtree = store.descendants(&[parent_id], nemesis_board::store::DescendantEdges::Subtree);
    let unsatisfied = nemesis_board::finish::evaluate_parent_entry_finish(store, &subtree);
    if !unsatisfied.is_empty() {
        info!(
            "[BoardReview] 父单 {parent_id} FINISH 入口闸未满足（{} 条），不进入自动收口（转人工）",
            unsatisfied.len()
        );
        if let Err(e) =
            nemesis_board::finish::write_finish_gap_comment(store, parent_id, &unsatisfied)
        {
            warn!("[BoardReview] 父单 {parent_id} FINISH 缺口评论写入失败：{e}");
        }
        return Ok(false);
    }
    let children = store.list_children(parent_id)?;

    // ---- M8 缺陷闸门：子树在账缺陷非空 → 确定性转人工（不进语义评审）。
    // 扫整棵子树（SAN-08 同款闭包——done 子单之下的孙单声明也不能漏）。
    let defects = scan_unclosed_defects(store, &subtree)?;
    if !defects.is_empty() {
        warn!(
            "[BoardReview] 父单 {parent_id} 子树存在 {} 条未闭环缺陷声明，自动收口转人工",
            defects.len()
        );
        let mention = if parent.creator.kind == "admin" {
            format!("@{} ", parent.creator.id)
        } else {
            String::new()
        };
        // U-A：缺陷闸门停车档案（修复闭环后人工收口；worker 侧修好即
        // [FIXED] 闭环，无需重派）。
        let handoff_note = park_with_handoff(
            deps,
            &parent.number,
            parent_id,
            nemesis_board::handoff::HandoffSpec {
                stop_kind: "defect_gate",
                suggested_action: "manual_check",
                hint: format!(
                    "子树存在 {} 条未闭环 P0/P1 缺陷声明：worker 修复闭环（[FIXED]）后人工收口父单",
                    defects.len()
                ),
                resume_entry: "人工收口（子单 [FIXED] 闭环后父单设 done）".into(),
                environment: serde_json::json!({ "unclosed_defects": defects.len() }),
                attempted: vec![],
            },
        );
        let msg = format!(
            "{mention}🚨 [缺陷闸门] 子任务交付中存在 {n} 条未闭环缺陷声明（交付/评论**行首** `[P0]`/`[P1]` 声明、`[FIXED]` 闭环），父单自动收口转人工，修复闭环后请人工收口：\n\n{body}{handoff_note}",
            n = defects.len(),
            body = render_defect_gate_comment(&defects),
        );
        store.add_comment(NewComment {
            issue_id: parent_id,
            author: Actor::system("board-review"),
            content: msg,
            parent_id: None,
            ctype: CommentType::System,
        })?;
        record_auto_decide(
            deps,
            parent_id,
            "parent_defect_gate",
            "escalate",
            serde_json::json!({ "unclosed_defects": defects.len() }),
        );
        return Ok(true);
    }

    // ---- 组装汇总输入：每子单状态 + 最新交付摘要（截断防 prompt 爆炸）----
    // 段序（2026-10-17 NB-101 项目收口三轮误升级根修同款，横向扫同类）：
    // 实物证据段在最前、子单交付摘要（worker 自述）垫后——worker_report
    // 有字节内联上限，超限截断从尾部吃起：截断时先牺牲转述保实物。
    let mut summary = if let Some(pid) = parent.project_id
        && let Ok(project) = store.get_project(pid)
        && let Some(dir) = project.directory.as_deref()
    {
        render_project_artifacts_evidence(std::path::Path::new(dir))
    } else {
        String::new()
    };
    summary.push_str(&format!(
        "## 子任务完成情况汇总（共 {} 个子任务）\n",
        children.len()
    ));
    for c in &children {
        let latest_delivery = store
            .list_comments(c.id)?
            .iter()
            .rev()
            .find(|cm| cm.ctype == CommentType::Delivery)
            .map(|cm| nemesis_utils::truncate(cm.content.trim(), 1500))
            .unwrap_or_else(|| "（无结构化交付汇报）".to_string());
        summary.push_str(&format!(
            "### 子任务 {}「{}」状态={} 最新交付摘要：\n{}\n\n",
            c.number, c.title, c.status, latest_delivery
        ));
    }

    // （实物证据段已前置到 summary 开头——2026-10-17 段序根修，见上方说明；
    // 此处不再重复注入。）

    // 2026-10-17 NB-101 三轮误升级根修（父单同款）：收口 worker_report 携带
    // 实物证据段，默认 64KB 内联上限会把节选段截掉——用与项目收口一致的
    // 大预算（证据段自带 60K 字符节选总预算，此处只是不再二次截伤）。
    let mut prompt = nemesis_board::build_review_user_prompt_with_limits(
        &parent.number,
        &parent.title,
        &parent.description,
        parent.acceptance_criteria.as_deref(),
        &summary,
        // 父单线程评论以 status_change/system 机械记录为主（子单结论已在
        // 汇总段），不带入。
        &[],
        CLOSURE_WORKER_REPORT_MAX_BYTES,
        // 父单收口不走跨轮缺口清账（子单评审的清单语义不适用于父单汇总
        // 面；返工承接有自己的轮数预算）。
        &[],
    );

    // ---- 客观锚点双检（P2/B1，父单同款）：先便宜后昂贵 ----
    // 父单验收输入 = 子单交付汇总段；锚点全过 → 摘要拼进 prompt 仍审语义；
    // 有 fail → 合成 FAIL 短路（跳过 LLM），流经下方 verdict 分臂（父单
    // 不派发、无重派承接单位——转人工语义与 LLM FAIL 一致）。不安全锚点
    // 行同款解析期拒绝 + 系统告警评论（T2-3）。
    let (anchors, _semantic_fallback, rejected_anchors) =
        nemesis_board::parse_anchors(parent.acceptance_criteria.as_deref().unwrap_or(""));
    if !rejected_anchors.is_empty() {
        let mut warn_body =
            String::from("⚠ 验收标准含不安全/非法锚点行，已忽略（按普通语义标准评审）：\n");
        for r in &rejected_anchors {
            warn_body.push_str(&format!("- `{}`：{}\n", r.raw, r.reason));
        }
        if let Err(e) = store.add_comment(nemesis_board::NewComment {
            issue_id: parent_id,
            author: nemesis_board::Actor::system("board"),
            content: warn_body,
            parent_id: None,
            ctype: CommentType::System,
        }) {
            warn!("[BoardReview] 父单 {parent_id} 锚点告警评论落库失败：{e}");
        }
    }
    // 双根核验（子单同款，NB-22 锚点全败根修）：子单产物经档案管线合并在
    // 项目目录，父单锚点同样要能核到它们。
    let parent_fallback = parent
        .project_id
        .and_then(|pid| store.get_project(pid).ok())
        .and_then(|p| project_archive_root(p.directory.as_deref()).ok());
    // M2 三态分流（父单同款）：根全不可达 = 基础设施故障，转人工不产出
    // 锚点 FAIL（父单无重派承接单位，FAIL 合成只会污染子单结论叙事）。
    if let Err(root_err) = nemesis_board::validate_anchor_roots(
        &anchors,
        resolve_anchor_root(deps),
        parent_fallback.as_deref(),
    ) {
        warn!(
            "[BoardReview] 父单 {parent_id} 锚点根不可达（基础设施故障）→ 转人工，不产出锚点 FAIL：{root_err}"
        );
        let mention = if parent.creator.kind == "admin" {
            format!("@{} ", parent.creator.id)
        } else {
            String::new()
        };
        let parent_dispatches = store.list_dispatches(parent_id).unwrap_or_default();
        let handoff_note = park_with_handoff(
            deps,
            &parent.number,
            parent_id,
            nemesis_board::handoff::HandoffSpec {
                stop_kind: "infra",
                suggested_action: "retry",
                hint: "父单收口锚点主根与项目目录均不可达（基础设施故障，非交付质量问题）：核查工作区/项目档案目录是否被移动或删除，修好后复评".into(),
                resume_entry: "review.retrigger（number=NB-x，环境修复后复评）".into(),
                environment: last_dispatch_env(&parent_dispatches),
                attempted: vec![serde_json::json!({
                    "action": "validate_anchor_roots",
                    "result": root_err.to_string(),
                })],
            },
        );
        let msg = format!(
            "{mention}🧭 父单收口的客观锚点检查无法执行：锚点主根与项目目录均不可达（基础设施故障，非交付质量问题），请人工核查工作区环境后裁决。\n\n{root_err}{handoff_note}"
        );
        store.add_comment(nemesis_board::NewComment {
            issue_id: parent_id,
            author: nemesis_board::Actor::system("board-review"),
            content: msg,
            parent_id: None,
            ctype: CommentType::System,
        })?;
        // 决策审计（2026-10-17 补齐，子单同款）：转人工终局决策落记录——
        // 重放守卫据此判已定案，不跨重启反复重评审。
        record_auto_decide(
            deps,
            parent_id,
            "escalate_human",
            "indeterminate",
            serde_json::json!({ "reason": "anchor_root_missing", "detail": root_err }),
        );
        return Ok(true);
    }
    // M7 exec 策略闸（父单同款：默认关闭 = 跳过注记）。
    let exec_policy = if cfg.review.exec_anchors_enabled {
        nemesis_board::ExecAnchorPolicy::Enabled {
            timeout_secs: cfg.review.exec_anchor_timeout_secs,
        }
    } else {
        nemesis_board::ExecAnchorPolicy::Disabled
    };
    let (anchor_results, anchor_skipped) = run_anchors_dual_ext_blocking(
        &anchors,
        resolve_anchor_root(deps),
        parent_fallback.as_deref(),
        &summary,
        &exec_policy,
    )
    .await;
    if !anchor_skipped.is_empty() {
        prompt.push_str(&format!(
            "\n## 客观锚点检查（部分未执行）\n以下 exec: 锚点本轮未执行（开关关闭或环境依赖缺失，见各条注记），其验收项**未经系统核验**，请按语义项人工评判，不得视为已通过：\n{}",
            anchor_skipped
                .iter()
                .map(|s| format!("- {s}"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    // 锚点全过 → PASS 摘要评论（父单同款，确定性审计痕迹）。
    if !anchor_results.is_empty() && nemesis_board::all_passed(&anchor_results) {
        let pass_body = format!(
            "✅ 客观锚点检查通过（{} 条）：\n{}",
            anchor_results.len(),
            nemesis_board::render_anchor_summary(&anchor_results)
        );
        if let Err(e) = store.add_comment(nemesis_board::NewComment {
            issue_id: parent_id,
            author: nemesis_board::Actor::system("board"),
            content: pass_body,
            parent_id: None,
            ctype: CommentType::System,
        }) {
            warn!("[BoardReview] 父单 {parent_id} 锚点 PASS 评论落库失败：{e}");
        }
    }
    let output = if !anchor_results.is_empty() && !nemesis_board::all_passed(&anchor_results) {
        warn!("[BoardReview] 父单 {parent_id} 客观锚点检查失败 → 短路 FAIL（跳过 LLM 语义评审）");
        nemesis_board::ReviewOutput {
            verdict: nemesis_board::ReviewVerdict::Fail,
            reasons: anchor_results
                .iter()
                .filter(|r| !r.passed)
                .map(|r| format!("锚点失败: {} — {}", r.anchor.raw, r.detail))
                .collect(),
            // AOR 批一c §③：失败锚点自动成 FAIL 条目（同子单短路路径）。
            criteria: anchor_results
                .iter()
                .filter(|r| !r.passed)
                .map(|r| nemesis_board::CriterionVerdict {
                    criterion: format!("[anchor] {}", r.anchor.raw),
                    verdict: nemesis_board::ReviewVerdict::Fail,
                    evidence: r.detail.clone(),
                })
                .collect(),
            gap: format!(
                "客观锚点检查失败（确定性核验，未进入 AI 语义评审）：\n{}",
                nemesis_board::render_anchor_failures(&anchor_results)
            ),
            experience: None,
            need_evidence: None,
            evidence_request: None,
            unknowns: Vec::new(),
            gap_closure: Vec::new(),
        }
    } else {
        if !anchor_results.is_empty() {
            prompt.push_str(&format!(
                "\n## 客观锚点检查（已通过）\n以下客观验收锚点已由系统核验通过，无需重复核验，请专注评审其余语义项：\n{}",
                nemesis_board::render_anchor_summary(&anchor_results)
            ));
        }
        // 色审议需要评审 agent 在场；锚点短路 FAIL 是确定性判定，loop
        // 未就绪也成立（明细评论照常落库，人工兜底可见）。
        let Some(agent_loop) = deps.moderator_loop.get() else {
            warn!("[BoardReview] 父单 {parent_id} 主 agent 未就绪，跳过收口验收（人工兜底）");
            return Ok(false);
        };
        // PR4：父单收口预算块（恒纯文本单轮——见下方工具模式注释）。
        prompt.push_str(&render_review_budget_block(ReviewToolMode::NoTools));
        // 父单收口恒纯文本（B2a 执行型验收只适用子单——父单工件分散在
        // 多个子单的 worker 会话，master 就地读文件无从对应）。
        // B3/C2：父单职能分镜（父单通常无职能 = None）+ 预取禁令节（镜 1
        // 多检查员时消费）。
        let review_lens = nemesis_prompts::board::review_lens_for_profession(
            parent.required_profession.as_deref().unwrap_or(""),
        );
        let lens_reference = if review_lens.is_some() || eff_review.checkers > 1 {
            prefetch_ban_section(resolve_anchor_root(deps), parent_fallback.as_deref())
        } else {
            None
        };
        match run_review_panel(
            agent_loop,
            &prompt,
            ReviewToolMode::NoTools,
            eff_review.checkers,
            nemesis_prompts::board::parse_review_tier(&cfg.review.tier),
            review_lens,
            lens_reference.as_deref(),
            // 父单收口不走跨轮缺口清账（见 prompt 构建处注释）。
            &[],
        )
        .await
        {
            Ok(ok) => {
                // U-B：评审成功 = 端点活证据（入口闸保证封存期到不了这里）。
                nemesis_web::handlers::board_unavailable::record_llm_success(&cfg.unavailable);
                ok
            }
            Err(last_err) => {
                // U-B：评审失败喂封存判定；恰好触发封存 = 在途工作逐一写
                // 中断档案（本父单 + 在途拆解链）。
                let tripped = nemesis_web::handlers::board_unavailable::record_llm_failure(
                    &cfg.unavailable,
                    &last_err,
                );
                if tripped {
                    nemesis_web::handlers::board_unavailable::write_trip_interruptions(
                        Some(deps.workspace.as_path()),
                        store,
                        Some((
                            &parent,
                            "review",
                            "父单收口评审进行中 LLM 端点不可用封存：恢复后探针自动续评".to_string(),
                            serde_json::json!({}),
                        )),
                        &last_err,
                    );
                }
                // U-A：评审自身失败也是停车路径（父单无 N2 重评链——文本
                // 单轮），落停车档案 + 评论注记，resume=review.retrigger。
                let handoff_note = park_with_handoff(
                    deps,
                    &parent.number,
                    parent_id,
                    nemesis_board::handoff::HandoffSpec {
                        stop_kind: "retry_exhausted",
                        suggested_action: "retry",
                        hint: "父单收口验收评审自身失败（非交付质量问题）：核查模型端点/评审配置后复评".into(),
                        resume_entry: "review.retrigger（number=NB-x，环境修复后复评）".into(),
                        environment: serde_json::json!({ "review_tier": cfg.review.tier }),
                        attempted: vec![serde_json::json!({
                            "action": "review_panel",
                            "result": last_err.to_string(),
                        })],
                    },
                );
                let comment = format!(
                    "🤷 父单收口验收无法完成（验收评审自身失败），请人工裁决。\n\n原因：{last_err}{handoff_note}"
                );
                post_review_comment(store, parent_id, &deps.cluster, &comment)?;
                return Ok(true);
            }
        }
    };

    store_experience(
        store,
        &parent.number,
        deps.cluster.node_id(),
        &output.experience,
    );

    let reviewer = Actor::agent(deps.cluster.node_id());
    // S-O2 迟到评审守卫（同子单验收）：收口评审期间父单被人工改状态则
    // 结论整体让位（PASS 直推 done 是 backlog→done 合法边的同型竞态面）。
    if !review_still_relevant(deps, parent_id, output.verdict.as_str(), "父单收口") {
        return Ok(false);
    }
    match output.verdict {
        nemesis_board::ReviewVerdict::Pass => {
            // EST-03（2026-09-16 横扫加固；2026-11-05 R528 补父单收口 PASS
            // 臂）：收口评审 LLM 是长跑——发起时 estop 未生效不代表结束时
            // 仍未生效。落 done + on_issue_settled 级联（祖先链 sync/依赖
            // 补派/项目收口触发）前补检查点：挂起即停车入队（release 复评
            // 恢复），不越过急停继续自动收口。FAIL/UNSURE 臂同窗已由
            // try_parent_remediation 的 estop 窗口闸 + 转人工停车覆盖。
            if estop_fuse_engaged(deps, parent_id, ParkedKind::Parent) {
                return Ok(false);
            }
            // AOR 批二b（⑥ FINISH 谓词·收口面终检）：评审耗时数分钟，子树
            // 状态可能变化——落盘前以新鲜子树重验完整五条件，宁阻塞不盲收
            // 口。阻塞 → 缺口评论（C-F4 去重）+ 不落 PASS 评论不记
            // parent_auto_close；缺口消除后既有 settle 钩子自动复评。
            let subtree_now =
                store.descendants(&[parent_id], nemesis_board::store::DescendantEdges::Subtree);
            let unsatisfied = nemesis_board::finish::evaluate_parent_finish(store, &subtree_now);
            if !unsatisfied.is_empty() {
                info!(
                    "[BoardReview] 父单 {parent_id} FINISH 收口闸未满足（{} 条），评审通过但不落 done（缺口消除后自动复评）",
                    unsatisfied.len()
                );
                if let Err(e) =
                    nemesis_board::finish::write_finish_gap_comment(store, parent_id, &unsatisfied)
                {
                    warn!("[BoardReview] 父单 {parent_id} FINISH 缺口评论写入失败：{e}");
                }
                return Ok(false);
            }
            store.add_comment(NewComment {
                issue_id: parent_id,
                author: reviewer.clone(),
                content: format!(
                    "✅ 父单收口验收通过（board.auto_close_parent 已开启，自动收口）\n\n{}",
                    render_reasons(&output.reasons)
                ),
                parent_id: None,
                ctype: CommentType::Comment,
            })?;
            record_auto_decide(
                deps,
                parent_id,
                "parent_auto_close",
                output.verdict.as_str(),
                serde_json::json!({ "reasons": render_reasons(&output.reasons) }),
            );
            store.transition_issue(parent_id, IssueStatus::Done, &reviewer)?;
            info!("[BoardReview] 父单 {parent_id} 收口验收 PASS → done（auto_close_parent）");
            // C-F5（复核 2026-09-16）：父单落 done 改走与子单落定同源的
            // on_issue_settled 单一出口。此前只显式调项目收口触发
            // （notify_project_review_on_parent_done，on_issue_settled 内部
            // 同点调用，行为等价不重复），漏掉两条联动：多级看板沿祖先链
            // 向上 sync_parent_status（父单的父单收口感知）+ dependents
            // 依赖补派。收敛安全：收口 hook 是异步 spawn 无同步递归；
            // sync 带 visited 防环；终态单 terminal 早退不重复触发收口。
            nemesis_web::handlers::board::on_issue_settled(
                store,
                Some(&deps.cluster),
                parent_id,
                &reviewer,
            );
        }
        verdict @ (nemesis_board::ReviewVerdict::Fail | nemesis_board::ReviewVerdict::Unsure) => {
            let human_mention = if parent.creator.kind == "admin" {
                format!("@{} ", parent.creator.id)
            } else {
                String::new()
            };
            // 父单 FAIL 自动返工承接（2026-10-16）：差距具体（gap 非空）时
            // 先试自动承接——建「返工：」子单 + 子单自动派发单一入口发车，
            // 返工单落定后 sync 链经 PARENT_REVIEW_HOOK 自动复评本单（父单
            // 保持 in_review，不再无差别弹回人工）。UNSURE（证据不足）带
            // 可行动证据缺口（gap 或 reasons 非空）同样承接——补证据 = 补
            // 差距（2026-10-16 用户裁决：除崩溃性问题外不让用户反复验收）；
            // 双空（真无事可做）才落下方转人工路径。承接被拒/失败 → 落下方
            // 既有转人工路径（原因注记进评论）。
            let remediable = match verdict {
                nemesis_board::ReviewVerdict::Fail => {
                    // AOR 批一c §③：可返工性按渲染后差距判（条目清单在场即承接）。
                    !nemesis_board::render_gap_with_criteria(&output).is_empty()
                }
                nemesis_board::ReviewVerdict::Unsure => {
                    !output.gap.trim().is_empty() || !output.reasons.is_empty()
                }
                _ => false,
            };
            let mut remediation_declined: Option<String> = None;
            if remediable {
                match try_parent_remediation(deps, &cfg, &parent, &output) {
                    Ok(ParentRemediation::Taken(t)) => {
                        let dispatch_word = if t.dispatched {
                            "已派发"
                        } else {
                            "暂未派出（当前无可匹配节点，停车场 sweep 会在节点可用时自动接力）"
                        };
                        let lead = if verdict == nemesis_board::ReviewVerdict::Fail {
                            "父单收口验收未过（fail），差距具体可返工"
                        } else {
                            "父单收口验收证据不足（unsure），证据缺口可自动补齐"
                        };
                        let mut comment = format!(
                            "🔁 {lead}——已自动创建返工子单 {num}（第 {round}/{max} 轮，{dispatch_word}）承接；返工单落定后系统自动复评本单，无需人工裁决。\n\n差距：\n{gap}\n",
                            num = t.number,
                            round = t.round,
                            max = t.max,
                            gap = nemesis_board::render_gap_with_criteria(&output),
                        );
                        append_reasons_unless_gapped(&mut comment, &output);
                        store.add_comment(NewComment {
                            issue_id: parent_id,
                            author: reviewer.clone(),
                            content: comment,
                            parent_id: None,
                            ctype: CommentType::Comment,
                        })?;
                        record_auto_decide(
                            deps,
                            parent_id,
                            "parent_auto_remediate",
                            output.verdict.as_str(),
                            serde_json::json!({
                                "round": t.round,
                                "max": t.max,
                                "remediation_issue": t.number,
                                "remediation_id": t.issue_id,
                                "dispatched": t.dispatched,
                                "reasons": render_reasons(&output.reasons),
                            }),
                        );
                        info!(
                            "[BoardReview] 父单 {parent_id} 收口 FAIL → 自动返工承接 {}（第 {}/{} 轮，dispatched={}）",
                            t.number, t.round, t.max, t.dispatched
                        );
                        return Ok(true);
                    }
                    Ok(ParentRemediation::Declined(note)) => remediation_declined = Some(note),
                    Err(e) => {
                        warn!("[BoardReview] 父单 {parent_id} 自动返工承接失败：{e}（转人工兜底）");
                        remediation_declined = Some(format!("自动返工承接执行失败：{e}"));
                    }
                }
            }
            // U-A：父单无重派承接单位，FAIL/UNSURE = 收口停车（父单结论
            // 需要人核查子单后定夺）。FAIL 且承接被拒（配置关/预算用满/
            // 派发失败）时评论注记拒绝原因。
            let handoff_note = park_with_handoff(
                deps,
                &parent.number,
                parent_id,
                nemesis_board::handoff::HandoffSpec {
                    stop_kind: "parent_unresolved",
                    suggested_action: "manual_check",
                    hint: format!(
                        "父单收口验收 {}：核查各子单结论后人工定 done 或重开子单（父单不派发、无重派承接单位）",
                        verdict.as_str()
                    ),
                    resume_entry: "review.retrigger（number=NB-x，子单处理完后复评）".into(),
                    environment: serde_json::json!({
                        "verdict": verdict.as_str(),
                        "gap": if verdict == nemesis_board::ReviewVerdict::Fail { output.gap.trim().to_string() } else { String::new() },
                    }),
                    attempted: vec![],
                },
            );
            let mut comment = format!(
                "{human_mention}🤷 父单收口验收未定案（{ver}），请人工裁决。\n",
                ver = verdict.as_str()
            );
            if verdict == nemesis_board::ReviewVerdict::Fail {
                // AOR 批一c §③：人读评论用渲染后条目清单（旧格式回落 gap 原文）。
                comment.push_str(&format!(
                    "\n差距：\n{}\n",
                    nemesis_board::render_gap_with_criteria(&output)
                ));
            }
            append_reasons_unless_gapped(&mut comment, &output);
            // 说清楚（2026-10-16 用户裁决）：升级必须带自动返工全程账目
            //（试了几轮、哪些返工单）——不是一句「未定案」就让用户盲裁。
            // 前缀两种形态都认（存量半角单，同轮数清点宽容逻辑）。
            let rem_children: Vec<&nemesis_board::Issue> = children
                .iter()
                .filter(|c| c.title.starts_with("返工：") || c.title.starts_with("返工: "))
                .collect();
            if !rem_children.is_empty() {
                let nums: Vec<&str> = rem_children.iter().map(|c| c.number.as_str()).collect();
                comment.push_str(&format!(
                    "\n已自动返工 {} 轮（预算 {max}）：{}\n",
                    rem_children.len(),
                    nums.join("、"),
                    max = cfg.review.parent_remediation_max,
                ));
            }
            if let Some(note) = &remediation_declined {
                comment.push_str(&format!(
                    "\n（{note}——请检查各子单结论后人工定 done 或重开子单）"
                ));
            } else {
                comment.push_str(
                    "\n（父单不派发、无重派承接单位——请检查各子单结论后人工定 done 或重开子单）",
                );
            }
            comment.push_str(
                "\n（打回重开：决策面板「复评」/ review.retrigger —— 打回后自动验收重新开始，返工轮数预算重置）",
            );
            comment.push_str(&handoff_note);
            store.add_comment(NewComment {
                issue_id: parent_id,
                author: reviewer.clone(),
                content: comment,
                parent_id: None,
                ctype: CommentType::Comment,
            })?;
            record_auto_decide(
                deps,
                parent_id,
                "parent_escalate_human",
                verdict.as_str(),
                serde_json::json!({ "reasons": render_reasons(&output.reasons) }),
            );
            info!(
                "[BoardReview] 父单 {parent_id} 收口验收 verdict={} → 转人工（保持 in_review）",
                verdict.as_str()
            );
            // AOR 批三 5.3 触发点②：父单收口评审 FAIL 转人工 → 重规划起草
            //（UNSURE 是证据缺口不是计划缺口，不触发；返工承接已覆盖 FAIL
            // 可返工形态，本钩子只承接承接失败/被拒/预算用尽的残留流）。
            if verdict == nemesis_board::ReviewVerdict::Fail {
                crate::board_replan::spawn_replan_draft(
                    deps.clone(),
                    parent_id,
                    "parent_review_fail",
                );
            }
        }
    }
    Ok(true)
}

/// F3 项目收口汇总验收（fire-and-forget）：gateway 注册到
/// `nemesis_web::handlers::board::set_project_review_hook`；触发前置
/// （全部顶层父单 done + 项目非空 + 状态 active/in_progress）在 web 侧
/// notify 做过，此处 estop/配置双保险 + 验收执行 + 三态处置。
pub(crate) fn spawn_project_review(deps: BoardReviewDeps, project_id: i64) {
    let store = deps.store.clone();
    let cluster = deps.cluster.clone();
    // 项目级任务没有单一 issue 挂评论 → issue_id=None（panic 只记日志，
    // Err 路径的缺口评论/人工兜底链路不变）。
    spawn_watched(store, cluster, None, "项目收口评审", async move {
        match review_project_completion(&deps, project_id).await {
            Ok(true) => {}
            Ok(false) => {} // 跳过类结果已在 review_project_completion 内记日志
            Err(e) => warn!("[BoardReview] 项目 {project_id} 收口验收失败：{e}（人工兜底）"),
        }
    });
}

/// F3 项目收口汇总验收。返回 `Ok(false)` = 未评审即跳过（配置关/项目已
/// 收口/竞态防御复核不过/estop 冻结），`Ok(true)` = 评审闭环完成。
pub(super) async fn review_project_completion(
    deps: &BoardReviewDeps,
    project_id: i64,
) -> Result<bool, String> {
    // estop 保险丝检查点 ①：项目收口同样是自动 agent 活动。
    if estop_fuse_engaged(deps, project_id, ParkedKind::Project) {
        return Ok(false);
    }
    let cfg = load_board_flags(&deps.home)?;
    // 双保险（hook 已读一次）：auto_review 总闸 + review.auto_close_project。
    if !cfg.auto_review || !cfg.review.auto_close_project {
        return Ok(false);
    }

    let store = &deps.store;
    let project = store.get_project(project_id)?;
    if matches!(project.status.as_str(), "completed" | "archived") {
        debug!(
            "[BoardReview] 项目 {project_id} 已是 {}，跳过收口验收",
            project.status
        );
        return Ok(false);
    }

    // U-B 入口闸：LLM 端点封存期不进语义评审（项目档案单独落——项目无
    // comment 载体，`project-<id>.json` 是其唯一现场），恢复后探针续评。
    if nemesis_web::handlers::board_unavailable::frozen_now() {
        nemesis_web::handlers::board_unavailable::write_interruption_doc(
            &deps.workspace,
            &format!("project-{}", project.id),
            project.id,
            "project",
            format!(
                "项目「{}」收口验收挂起：LLM 端点不可用封存中，恢复后探针自动续评",
                project.name
            ),
            serde_json::json!({
                "project_id": project.id,
                "project_name": project.name,
            }),
        );
        warn!(
            "[BoardReview] 项目 {project_id} 收口验收挂起：LLM 端点不可用封存中（中断档案已落盘，恢复后自动续评）"
        );
        return Ok(false);
    }

    // 触发前置防御性复核（触发到评审排队之间人工可能又动了单子）。
    let parents: Vec<nemesis_board::Issue> = store
        .list_issues(&nemesis_board::models::IssueFilter {
            project_id: Some(project_id),
            ..Default::default()
        })?
        .into_iter()
        .filter(|i| i.parent_issue_id.is_none())
        .collect();
    if parents.is_empty() {
        // M5-4（2026-10-10）：触发到复核之间项目被清空是异常态——静默跳过
        // 会让「收口验收没跑」无人可见，升 WARN 带原因。
        warn!("[BoardReview] 项目 {project_id} 无顶层父单（触发后被清空/竞态），跳过收口验收");
        return Ok(false);
    }
    if parents.iter().any(|p| p.status != IssueStatus::Done) {
        warn!(
            "[BoardReview] 项目 {project_id} 存在非 done 顶层父单（竞态），跳过收口验收（等待父单终态后由收口链重触发）"
        );
        return Ok(false);
    }
    // 范围缺口边界（与父单收口同款语义 + SAN-08 子树闭包）：任何子孙
    // cancelled（含 done 父单之下的隐藏缺口）→ 范围缺口要人裁决，项目级
    // 验收补不了。
    for p in &parents {
        let subtree = store.descendants(&[p.id], nemesis_board::store::DescendantEdges::Subtree);
        if subtree.iter().any(|c| c.status == IssueStatus::Cancelled) {
            // M5-4：范围缺口是「该项目交付不全」的强信号——升 WARN 保证
            // 无人值守模式下可观测（转人工意图不变）。
            warn!(
                "[BoardReview] 项目 {project_id} 父单 {}（{}）子树存在 cancelled 单，不自动收口（范围缺口转人工）",
                p.number, p.title
            );
            return Ok(false);
        }
    }

    // ---- M8 缺陷闸门（项目级）：全部顶层父单子树聚合扫描，在账缺陷非空
    // → 确定性转人工不进语义评审。缺口评论落全部顶层父单（项目无 comment
    // 载体，F3/M2 根不可达臂同款惯例）。
    let mut project_defects: Vec<UnclosedDefect> = Vec::new();
    for p in &parents {
        let sub = store.descendants(&[p.id], nemesis_board::store::DescendantEdges::Subtree);
        project_defects.extend(scan_unclosed_defects(store, &sub)?);
    }
    if !project_defects.is_empty() {
        warn!(
            "[BoardReview] 项目 {project_id} 存在 {} 条未闭环缺陷声明，自动收口转人工",
            project_defects.len()
        );
        let reviewer = Actor::agent(deps.cluster.node_id());
        let body = format!(
            "🚨 [缺陷闸门] 项目「{name}」交付中存在 {n} 条未闭环缺陷声明（交付/评论**行首** `[P0]`/`[P1]` 声明、`[FIXED]` 闭环），项目自动收口转人工，修复闭环后请人工收口：\n\n{list}",
            name = project.name,
            n = project_defects.len(),
            list = render_defect_gate_comment(&project_defects),
        );
        for p in &parents {
            let human_mention = if p.creator.kind == "admin" {
                format!("@{} ", p.creator.id)
            } else {
                String::new()
            };
            // U-A：缺陷闸门停车档案随父单落盘（评论载体 = resume 载体）。
            let handoff_note = park_with_handoff(
                deps,
                &p.number,
                p.id,
                nemesis_board::handoff::HandoffSpec {
                    stop_kind: "defect_gate",
                    suggested_action: "manual_check",
                    hint: format!(
                        "项目「{}」存在 {} 条未闭环 P0/P1 缺陷声明：worker 修复闭环（[FIXED]）后人工收口项目",
                        project.name,
                        project_defects.len()
                    ),
                    resume_entry: "人工收口（缺陷闭环后项目设 completed）".into(),
                    environment: serde_json::json!({
                        "project_id": project.id,
                        "unclosed_defects": project_defects.len(),
                    }),
                    attempted: vec![],
                },
            );
            store.add_comment(NewComment {
                issue_id: p.id,
                author: reviewer.clone(),
                content: format!("{human_mention}{body}{handoff_note}"),
                parent_id: None,
                ctype: CommentType::Comment,
            })?;
            record_auto_decide(
                deps,
                p.id,
                "project_defect_gate",
                "escalate",
                serde_json::json!({
                    "project_id": project_id,
                    "unclosed_defects": project_defects.len()
                }),
            );
        }
        return Ok(true);
    }

    // ---- 组装汇总输入：每顶层父单段 = 自身最新交付（若有）+ 子单交付
    // 摘要（截断防 prompt 爆炸）。2026-09-12 根修：旧实现只取父单**自身**
    // 最新 Delivery——实际数据流中 worker 交付评论落在叶子子单上（对比
    // review_parent_issue：它遍历的就是子单），父单自身常无 Delivery，
    // 聚合到空集 → 项目级锚点对「（无结构化交付汇报）」实核必然全 FAIL
    // （双端真机 S2 实证：三条 re: 锚点全败，子单真实交付从未进入实核
    // 文本）。聚合深度对齐 review_parent_issue（单层子单——planner 拆解
    // 纪律即单层），不引入新的嵌套深度语义。
    //
    // 段序（2026-10-17 NB-101 三轮误升级根修的一部分）：**实物证据段在最
    // 前**、子单交付摘要（worker 自述）垫后——worker_report 有字节内联上
    // 限，超限截断从尾部吃起：实物证据（清单+最终报告节选）是不可再生的
    // 评审输入，交付摘要是已被独立验收过的转述（每张子单都有自己的
    // auto_decide），截断时先牺牲转述保实物。
    // ---- F-U6-2（2026-09-15 U6 真机）：项目目录实物证据段（review_parent_issue
    // 的 F-U3-6 同款——横向扫同类补齐）。项目收口验收标准与父单同形（「项目
    // 目录含 xxx」），但此前评审输入只有各子单 Delivery 声明摘要（worker 自述，
    // 且 exec 工作副本路径 Linux/Windows 混杂）——U6 实证：父单收口评审凭
    // 实物证据四条标准全过 PASS，项目收口评审却因「无项目目录实物直接取证 /
    // 子单仅有自评」判 UNSURE 转人工，两道评审同一标准结论矛盾。注入真实
    // 清单 + 内容节选后与父单收口同源对照（无绑定目录的存量项目诚实跳过）。
    // 无绑定目录时从空串起头（下层 push 汇总标题，行为同旧）。
    let mut summary = if let Some(dir) = project.directory.as_deref() {
        render_project_artifacts_evidence(std::path::Path::new(dir))
    } else {
        String::new()
    };
    summary.push_str(&format!(
        "## 顶层任务完成情况汇总（共 {} 个）\n",
        parents.len()
    ));
    for p in &parents {
        let children = store.list_children(p.id)?;
        summary.push_str(&format!(
            "### 任务 {}「{}」状态={}（子单 {} 个）\n",
            p.number,
            p.title,
            p.status,
            children.len(),
        ));
        if let Some(cm) = store
            .list_comments(p.id)?
            .iter()
            .rev()
            .find(|cm| cm.ctype == CommentType::Delivery)
        {
            summary.push_str(&format!(
                "父单最新交付摘要：\n{}\n\n",
                nemesis_utils::truncate(cm.content.trim(), 1500)
            ));
        }
        if children.is_empty() {
            summary.push_str("（无子单）\n\n");
        }
        for c in &children {
            let latest_delivery = store
                .list_comments(c.id)?
                .iter()
                .rev()
                .find(|cm| cm.ctype == CommentType::Delivery)
                .map(|cm| nemesis_utils::truncate(cm.content.trim(), 1500))
                .unwrap_or_else(|| "（无结构化交付汇报）".to_string());
            // F-U6-2：附子单独立验收结论（最新 auto_decide）——此前评审员
            // 只看到「状态=done」会误读为「worker 自评」，U6 实证 UNSURE
            // 理由即含「子单仅有自评 done、无独立验收结论」（事实错误：
            // 每张子单都有 agent 评审 auto_accept 决策入账）。只带决策词
            // 不带理由全文（防 prompt 爆炸）。
            let verdict_note = store
                .list_activity(c.id)?
                .iter()
                .rev()
                .find(|a| a.action == "auto_decide")
                .and_then(|a| a.details.as_deref())
                .and_then(|d| serde_json::from_str::<serde_json::Value>(d).ok())
                .and_then(|v| {
                    v.get("decision")
                        .and_then(|d| d.as_str())
                        .map(|s| s.to_string())
                })
                .map(|d| format!("（子单评审结论：{d}）"))
                .unwrap_or_default();
            summary.push_str(&format!(
                "- 子任务 {}「{}」状态={}{} 最新交付摘要：\n{}\n\n",
                c.number, c.title, c.status, verdict_note, latest_delivery
            ));
        }
    }

    // （实物证据段已前置到 summary 开头——见上方段序说明；此处不再重复
    // 注入。旧位置在交付摘要之后，64KB 内联截断会把节选段整体切掉，
    // 实录：2026-10-17 三轮项目评审 prompt 全部停在文件清单中段。）

    // 验收标准聚合（F3 项目收口判定依据）：项目级 AC 在前（v10 列——此前
    // 只聚合父单 AC，project.create 存的项目级标准被静默丢弃），各顶层父单
    // 非空 AC 依序拼接；锚点行照常参与客观双检。
    let joined_ac = join_project_review_ac(Some(&project.acceptance_criteria), &parents);

    // 内联上限：见模块级 CLOSURE_WORKER_REPORT_MAX_BYTES（2026-10-17 根修）。
    let mut prompt = nemesis_board::build_review_user_prompt_with_limits(
        &format!("PROJECT-{}", project.id),
        &project.name,
        &project.description,
        Some(&joined_ac),
        &summary,
        &[],
        CLOSURE_WORKER_REPORT_MAX_BYTES,
        // 项目收口不走跨轮缺口清账（同父单口径）。
        &[],
    );

    // ---- 客观锚点双检（父单/子单同款）：workspace 根，先便宜后昂贵 ----
    let (anchors, _semantic_fallback, rejected_anchors) = nemesis_board::parse_anchors(&joined_ac);
    if !rejected_anchors.is_empty() {
        warn!(
            "[BoardReview] 项目 {project_id} 验收标准含 {} 条不安全锚点行（已忽略，按普通语义标准评审）",
            rejected_anchors.len()
        );
    }
    // 双根核验（子单/父单同款，NB-22 锚点全败根修）：项目收口锚点核的也是
    // 全项目产物（子单变更集都在项目目录）。
    let project_fallback = project_archive_root(project.directory.as_deref()).ok();
    // M2 三态分流（项目收口同款）：根全不可达 = 基础设施故障，转人工——
    // 缺口评论落全部顶层父单（项目无 comment 载体，F3 惯例同款）。
    if let Err(root_err) = nemesis_board::validate_anchor_roots(
        &anchors,
        resolve_anchor_root(deps),
        project_fallback.as_deref(),
    ) {
        warn!(
            "[BoardReview] 项目 {project_id} 锚点根不可达（基础设施故障）→ 转人工，不产出锚点 FAIL：{root_err}"
        );
        let reviewer = Actor::agent(deps.cluster.node_id());
        let body = format!(
            "🏛 项目「{name}」收口的客观锚点检查无法执行：锚点主根与项目目录均不可达（基础设施故障，非交付质量问题），请人工核查工作区环境后裁决。\n\n{root_err}",
            name = project.name,
        );
        for p in parents {
            let human_mention = if p.creator.kind == "admin" {
                format!("@{} ", p.creator.id)
            } else {
                String::new()
            };
            // U-A：每个顶层父单各自落停车档案（父单是续做载体，项目无
            // comment 面——resume 入口按父单走）。
            let handoff_note = park_with_handoff(
                deps,
                &p.number,
                p.id,
                nemesis_board::handoff::HandoffSpec {
                    stop_kind: "infra",
                    suggested_action: "retry",
                    hint: format!(
                        "项目「{}」收口锚点主根与项目目录均不可达（基础设施故障）：核查工作区/项目档案目录后复评",
                        project.name
                    ),
                    resume_entry: "review.retrigger（number=NB-x，环境修复后复评）".into(),
                    environment: serde_json::json!({ "project_id": project.id, "project_name": project.name }),
                    attempted: vec![serde_json::json!({
                        "action": "validate_anchor_roots",
                        "result": root_err.to_string(),
                    })],
                },
            );
            store.add_comment(NewComment {
                issue_id: p.id,
                author: reviewer.clone(),
                content: format!("{human_mention}{body}{handoff_note}"),
                parent_id: None,
                ctype: CommentType::Comment,
            })?;
            // 决策审计（2026-10-17 补齐，项目收口 LLM 臂同款）：转人工终局
            // 决策按父单落记录（project_review_already_concluded 据此判项目
            // 评审已定案，不跨重启反复重评审）。
            record_auto_decide(
                deps,
                p.id,
                "project_escalate_human",
                "indeterminate",
                serde_json::json!({
                    "project_id": project.id,
                    "reason": "anchor_root_missing",
                    "detail": root_err,
                }),
            );
        }
        return Ok(true);
    }
    // M7 exec 策略闸（项目收口同款：默认关闭 = 跳过注记）。
    let exec_policy = if cfg.review.exec_anchors_enabled {
        nemesis_board::ExecAnchorPolicy::Enabled {
            timeout_secs: cfg.review.exec_anchor_timeout_secs,
        }
    } else {
        nemesis_board::ExecAnchorPolicy::Disabled
    };
    let (anchor_results, anchor_skipped) = run_anchors_dual_ext_blocking(
        &anchors,
        resolve_anchor_root(deps),
        project_fallback.as_deref(),
        &summary,
        &exec_policy,
    )
    .await;
    if !anchor_skipped.is_empty() {
        prompt.push_str(&format!(
            "\n## 客观锚点检查（部分未执行）\n以下 exec: 锚点本轮未执行（开关关闭或环境依赖缺失，见各条注记），其验收项**未经系统核验**，请按语义项人工评判，不得视为已通过：\n{}",
            anchor_skipped
                .iter()
                .map(|s| format!("- {s}"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    let output = if !anchor_results.is_empty() && !nemesis_board::all_passed(&anchor_results) {
        warn!("[BoardReview] 项目 {project_id} 客观锚点检查失败 → 短路 FAIL（跳过 LLM 语义评审）");
        nemesis_board::ReviewOutput {
            verdict: nemesis_board::ReviewVerdict::Fail,
            reasons: anchor_results
                .iter()
                .filter(|r| !r.passed)
                .map(|r| format!("锚点失败: {} — {}", r.anchor.raw, r.detail))
                .collect(),
            // AOR 批一c §③：失败锚点自动成 FAIL 条目（同子单短路路径）。
            criteria: anchor_results
                .iter()
                .filter(|r| !r.passed)
                .map(|r| nemesis_board::CriterionVerdict {
                    criterion: format!("[anchor] {}", r.anchor.raw),
                    verdict: nemesis_board::ReviewVerdict::Fail,
                    evidence: r.detail.clone(),
                })
                .collect(),
            gap: format!(
                "客观锚点检查失败（确定性核验，未进入 AI 语义评审）：\n{}",
                nemesis_board::render_anchor_failures(&anchor_results)
            ),
            experience: None,
            need_evidence: None,
            evidence_request: None,
            unknowns: Vec::new(),
            gap_closure: Vec::new(),
        }
    } else {
        if !anchor_results.is_empty() {
            prompt.push_str(&format!(
                "\n## 客观锚点检查（已通过）\n以下客观验收锚点已由系统核验通过，无需重复核验，请专注评审其余语义项：\n{}",
                nemesis_board::render_anchor_summary(&anchor_results)
            ));
        }
        let Some(agent_loop) = deps.moderator_loop.get() else {
            warn!("[BoardReview] 项目 {project_id} 主 agent 未就绪，跳过收口验收（人工兜底）");
            return Ok(false);
        };
        // PR4：项目收口预算块（恒纯文本单轮——同父单边界）。
        prompt.push_str(&render_review_budget_block(ReviewToolMode::NoTools));
        // 项目级恒纯文本（同父单边界：工件分散在各 worker 会话）。
        // C2：项目收口无职能分镜（lens=None），多检查员镜 1 消费预取禁令节
        // （项目目录 fallback 优先——架构工件就住在项目档案里）。
        let lens_reference = if cfg.review.checkers > 1 {
            prefetch_ban_section(resolve_anchor_root(deps), project_fallback.as_deref())
        } else {
            None
        };
        match run_review_panel(
            agent_loop,
            &prompt,
            ReviewToolMode::NoTools,
            cfg.review.checkers,
            nemesis_prompts::board::parse_review_tier(&cfg.review.tier),
            None,
            lens_reference.as_deref(),
            // 项目收口不走跨轮缺口清账（同父单口径）。
            &[],
        )
        .await
        {
            Ok(ok) => {
                // U-B：评审成功 = 端点活证据（入口闸保证封存期到不了这里）。
                nemesis_web::handlers::board_unavailable::record_llm_success(&cfg.unavailable);
                ok
            }
            Err(last_err) => {
                // U-B：失败喂封存判定；恰好触发封存时在途拆解链逐一写中断
                // 档案（focus=None），项目档案单独落（issue_key=project-<id>，
                // 项目无 comment 载体，档案是其唯一现场）。
                let tripped = nemesis_web::handlers::board_unavailable::record_llm_failure(
                    &cfg.unavailable,
                    &last_err,
                );
                if tripped {
                    nemesis_web::handlers::board_unavailable::write_trip_interruptions(
                        Some(deps.workspace.as_path()),
                        store,
                        None,
                        &last_err,
                    );
                    nemesis_web::handlers::board_unavailable::write_interruption_doc(
                        &deps.workspace,
                        &format!("project-{}", project.id),
                        project.id,
                        "project",
                        format!(
                            "项目「{}」收口验收进行中 LLM 端点不可用封存：恢复后探针自动续评",
                            project.name
                        ),
                        serde_json::json!({
                            "project_id": project.id,
                            "project_name": project.name,
                            "review_tier": cfg.review.tier,
                        }),
                    );
                }
                // 解析失败 → 全部顶层父单写转人工评论（项目无评论表，
                // 落在父单线程里人必然看得见）。U-A：停车档案随父单落盘。
                let comment_head = format!(
                    "🏛 项目「{}」收口验收无法完成（验收评审自身失败），请人工裁决。\n\n原因：{last_err}",
                    project.name
                );
                for p in &parents {
                    let handoff_note = park_with_handoff(
                        deps,
                        &p.number,
                        p.id,
                        nemesis_board::handoff::HandoffSpec {
                            stop_kind: "retry_exhausted",
                            suggested_action: "retry",
                            hint: format!(
                                "项目「{}」收口验收评审自身失败（非交付质量问题）：核查模型端点/评审配置后复评",
                                project.name
                            ),
                            resume_entry: "review.retrigger（number=NB-x，环境修复后复评）".into(),
                            environment: serde_json::json!({ "project_id": project.id, "review_tier": cfg.review.tier }),
                            attempted: vec![serde_json::json!({
                                "action": "review_panel",
                                "result": last_err.to_string(),
                            })],
                        },
                    );
                    let _ = post_review_comment(
                        store,
                        p.id,
                        &deps.cluster,
                        &format!("{comment_head}{handoff_note}"),
                    );
                }
                return Ok(true);
            }
        }
    };

    store_experience(
        store,
        &format!("project:{}", project.name),
        deps.cluster.node_id(),
        &output.experience,
    );

    apply_project_review_outcome(deps, &project, &parents, &output)?;
    Ok(true)
}

/// F3 三态落盘（独立 fn 供单测直测，不经 LLM）：
/// - PASS → 项目 completed（`board.review.auto_close_project` 本身即收口
///   授权，评论注明）。
/// - FAIL/UNSURE → completed 回滚 in_progress（F2 状态机合法转移，P3
///   预留）；已在 in_progress 的不动；archived 完全不动（只 warn）。缺口
///   评论落全部顶层父单（@各自创建者；LLM gap 无法可靠映射到具体父单，
///   诚实全量留痕），**不自动重开父单、不重派**（F3 边界）。
pub(super) fn apply_project_review_outcome(
    deps: &BoardReviewDeps,
    project: &nemesis_board::models::Project,
    parents: &[nemesis_board::Issue],
    output: &nemesis_board::ReviewOutput,
) -> Result<(), String> {
    let store = &deps.store;
    let reviewer = Actor::agent(deps.cluster.node_id());
    match output.verdict {
        nemesis_board::ReviewVerdict::Pass => {
            // 评审窗口内人工可能已动过项目状态（评审可跑数分钟）——按**新鲜
            // 状态**裁决。此前用评审前快照 + `?` 上抛：人工已 completed 时
            // 状态机 loud 拒绝把整个 PASS verdict 无声丢掉（无评论、无
            // auto_decide、无停车档案；与 FAIL/UNSURE 臂的 archived 只 warn
            // 降级不对称）。任一转移失败都只 warn 不丢结论。
            let fresh_status = store
                .get_project(project.id)
                .map(|p| p.status)
                .unwrap_or_else(|_| project.status.clone());
            let mut closed = false;
            match fresh_status.as_str() {
                "completed" => {
                    info!(
                        "[BoardReview] 项目 {}「{}」评审窗口内已被人工收口（completed）——PASS 结论留档，不重复转移",
                        project.id, project.name
                    );
                    closed = true;
                }
                "archived" => {
                    warn!(
                        "[BoardReview] 项目 {} 已 archived（评审窗口内人工归档）——PASS 结论只留档不改状态",
                        project.id
                    );
                }
                _ => {
                    // active 不可直达 completed（状态机只认 in_progress→completed）：
                    // 先补 in_progress 中转（两跳都合法）；中转失败不阻塞下一跳
                    //（人工同刻挪过状态时由下一跳按真实状态裁决）。
                    if fresh_status == "active"
                        && let Err(e) = store.update_project(
                            project.id,
                            &nemesis_board::models::ProjectPatch {
                                status: Some("in_progress".to_string()),
                                ..Default::default()
                            },
                        )
                    {
                        warn!(
                            "[BoardReview] 项目 {} PASS 收口 in_progress 中转失败（不阻塞）：{e}",
                            project.id
                        );
                    }
                    match store.update_project(
                        project.id,
                        &nemesis_board::models::ProjectPatch {
                            status: Some("completed".to_string()),
                            ..Default::default()
                        },
                    ) {
                        Ok(_) => {
                            info!(
                                "[BoardReview] 项目 {}「{}」收口验收 PASS → completed（review.auto_close_project）",
                                project.id, project.name
                            );
                            closed = true;
                        }
                        Err(e) => {
                            warn!(
                                "[BoardReview] 项目 {} PASS 收口 completed 落定失败（项目保持 {fresh_status}，请人工核查）：{e}",
                                project.id
                            );
                        }
                    }
                }
            }
            for p in parents {
                record_auto_decide(
                    deps,
                    p.id,
                    "project_complete",
                    output.verdict.as_str(),
                    serde_json::json!({
                        "project_id": project.id,
                        "reasons": render_reasons(&output.reasons),
                    }),
                );
            }
            if closed {
                // F9 收口总结（P6）：completed 落定后异步生成档案 summary.md
                //（失败诚实留痕，不影响已落定的收口状态）。
                crate::board_review::spawn_project_summary(deps.clone(), project.id);
                // 任务档案自动导出（2026-09-29 goal）：同源触发，内部自守门
                //（auto_on_complete 旗标默认开；失败父单评论留痕不阻塞收口）。
                crate::board_review::spawn_project_dossier(deps.clone(), project.id);
            }
        }
        verdict @ (nemesis_board::ReviewVerdict::Fail | nemesis_board::ReviewVerdict::Unsure) => {
            // completed → in_progress 回滚（合法转移）；in_progress 保持；
            // archived 不动。
            if project.status == "completed" {
                store.update_project(
                    project.id,
                    &nemesis_board::models::ProjectPatch {
                        status: Some("in_progress".to_string()),
                        ..Default::default()
                    },
                )?;
                info!(
                    "[BoardReview] 项目 {} 验收未定案 → completed 回滚 in_progress（F3）",
                    project.id
                );
            } else if project.status == "archived" {
                warn!(
                    "[BoardReview] 项目 {} 已 archived，验收未定案结论只留评论不回滚状态",
                    project.id
                );
            }
            let mut comment = format!(
                "🏛 项目「{name}」收口验收未定案（{ver}），请人工裁决。\n",
                name = project.name,
                ver = verdict.as_str()
            );
            if output.verdict == nemesis_board::ReviewVerdict::Fail {
                // AOR 批一c §③：人读评论用渲染后条目清单（旧格式回落 gap 原文）。
                comment.push_str(&format!(
                    "\n差距：\n{}\n",
                    nemesis_board::render_gap_with_criteria(output)
                ));
            }
            append_reasons_unless_gapped(&mut comment, output);
            comment.push_str(
                "\n（项目级验收不自动重开父单、不重派——请检查各任务结论后人工处置；\
处置完成后项目可人工设 completed）",
            );
            for p in parents {
                let human_mention = if p.creator.kind == "admin" {
                    format!("@{} ", p.creator.id)
                } else {
                    String::new()
                };
                // U-A：缺口落在哪个父单，停车档案就落在哪个父单（resume
                // 载体与评论载体一致）。
                let handoff_note = park_with_handoff(
                    deps,
                    &p.number,
                    p.id,
                    nemesis_board::handoff::HandoffSpec {
                        stop_kind: "project_unresolved",
                        suggested_action: "manual_check",
                        hint: format!(
                            "项目「{}」收口验收 {}：检查各任务结论后人工处置，完成后项目可人工设 completed",
                            project.name,
                            verdict.as_str()
                        ),
                        resume_entry: format!(
                            "review.retrigger（project_id={}，处置后复评）",
                            project.id
                        ),
                        environment: serde_json::json!({
                            "project_id": project.id,
                            "project_name": project.name,
                            "verdict": verdict.as_str(),
                            "gap": if output.verdict == nemesis_board::ReviewVerdict::Fail { output.gap.trim().to_string() } else { String::new() },
                        }),
                        attempted: vec![],
                    },
                );
                store.add_comment(NewComment {
                    issue_id: p.id,
                    author: reviewer.clone(),
                    content: format!("{human_mention}{comment}{handoff_note}"),
                    parent_id: None,
                    ctype: CommentType::Comment,
                })?;
            }
            info!(
                "[BoardReview] 项目 {} 收口验收 verdict={} → 缺口评论 {} 条父单，转人工",
                project.id,
                verdict.as_str(),
                parents.len()
            );
            for p in parents {
                record_auto_decide(
                    deps,
                    p.id,
                    "project_escalate_human",
                    verdict.as_str(),
                    serde_json::json!({
                        "project_id": project.id,
                        "reasons": render_reasons(&output.reasons),
                    }),
                );
            }
        }
    }
    Ok(())
}

/// AutoAccept 收货 + settle 联动（P2 #0 断链修复）：评论「自动收货」→
/// 落 done → **必须**走 `on_issue_settled`（父单状态同步 + 依赖子单补派）
/// ——与 WSAPI issue.status 落 done/cancelled 同一真相源；漏调即 T1-2
/// 断链（子单 done 后续依赖子单永不派发、父单永不收口）。独立成 fn 供
/// 回归测试直测联动（review_issue 主链需 LLM 才能到本分支）。
pub(super) fn auto_accept_and_settle(
    deps: &BoardReviewDeps,
    issue_id: i64,
    reasons: &[String],
) -> Result<(), String> {
    let store = &deps.store;
    let reviewer = Actor::agent(deps.cluster.node_id());
    store.add_comment(NewComment {
        issue_id,
        author: reviewer.clone(),
        content: format!(
            "✅ agent 验收通过（board.auto_accept 已开启，自动收货）\n\n{}",
            render_reasons(reasons)
        ),
        parent_id: None,
        ctype: CommentType::Comment,
    })?;
    store.transition_issue(issue_id, IssueStatus::Done, &reviewer)?;
    nemesis_web::handlers::board::on_issue_settled(store, Some(&deps.cluster), issue_id, &reviewer);
    info!("[BoardReview] issue {issue_id} PASS → done（auto_accept）+ settle 联动");
    Ok(())
}

/// estop 保险丝（全自动流转 P1/T1-6）：急停挂起 = true 并把 issue 停车
/// （release 后 [`spawn_estop_resume_watcher`] 复评恢复）。评论失败只
/// warn——冻结本身不依赖落库。重复停车按 **(kind, issue_id)** 去重（防多
/// 检查点重复入队/刷评论；BUG-6 根修 2026-10-13：旧键只看 issue_id，同一
/// 单先走评审停车（kind=Issue）、后走父单/项目收口停车（kind=Parent/
/// Project）会被吞号——后者永不复评）。
pub(super) fn estop_fuse_engaged(deps: &BoardReviewDeps, issue_id: i64, kind: ParkedKind) -> bool {
    if !deps.estop.is_engaged() {
        return false;
    }
    {
        let mut q = deps
            .estop_parked
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !q.iter().any(|&(k, id)| k == kind && id == issue_id) {
            q.push((kind, issue_id));
        }
    }
    warn!("[BoardReview] issue {issue_id} estop 冻结：自动验收/重派停车（急停释放后自动恢复）");
    let _ = post_review_comment(
        &deps.store,
        issue_id,
        &deps.cluster,
        "⛔ estop 急停中：自动验收/重派已冻结（保险丝）。急停释放后自动恢复。",
    );
    true
}

/// 停车场 sweep 触发闸（estop × 节流；集群完备性加固 2026-09-11 从
/// gateway 内联闭包抽取，可测）：急停挂起 → false 且**不消耗节流窗口**
/// —— 急停期间的 announce 刷新不吃掉释放后的首次重试机会。非急停时按
/// `min_interval` 节流（抗 announce 风暴），放行即盖章。gateway 装配的
/// sweep 回调把「本节点 announce」与「cluster 槽位未填」两条件留在调用
/// 侧（闭包捕获相关，与本闸正交）。
pub(crate) fn park_sweep_gate(
    estop_engaged: bool,
    last: &mut Option<std::time::Instant>,
    now: std::time::Instant,
    min_interval: std::time::Duration,
) -> bool {
    if estop_engaged {
        return false;
    }
    let ok = match *last {
        Some(t) => now.duration_since(t) >= min_interval,
        None => true,
    };
    if ok {
        *last = Some(now);
    }
    ok
}

/// estop 释放 watcher：订阅急停状态 watch，true→false 沿（释放）把停车
/// 队列逐条复评——无限模式循环从断点恢复（T1-6「release 后恢复」）；再
/// 按态补扫一轮 `replay_stuck_reviews`（R5-BUG-1：在 LLM 中途被打断的
/// 在途评审不进停车场，release 后靠扫态兜底复活）。gateway 装配期调用
/// 一次；四入口（CLI/托盘/Dashboard/WSAPI）最终都走同一
/// `EstopState::release()`，watch 全覆盖。
pub(crate) fn spawn_estop_resume_watcher(deps: BoardReviewDeps) {
    let mut rx = deps.estop.subscribe();
    tokio::spawn(async move {
        while rx.changed().await.is_ok() {
            if *rx.borrow_and_update() {
                continue; // 触发沿不动作（冻结在各检查点生效）
            }
            let parked: Vec<(ParkedKind, i64)> = deps
                .estop_parked
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .drain(..)
                .collect();
            for (kind, issue_id) in &parked {
                info!(
                    "[BoardReview] estop 释放 → 恢复{} {issue_id} 自动验收",
                    match kind {
                        ParkedKind::Issue => "",
                        ParkedKind::Parent => "父单",
                        ParkedKind::Project => "项目",
                    }
                );
                match kind {
                    ParkedKind::Issue => spawn_board_review(deps.clone(), *issue_id),
                    ParkedKind::Parent => spawn_parent_review(deps.clone(), *issue_id),
                    ParkedKind::Project => spawn_project_review(deps.clone(), *issue_id),
                }
            }
            // P4/E4（看板项目档案 goal）：急停挂起的合并补跑——PLACED 注册表
            // 里尚存的条目（estop 触发时合并被拒、条目保留）逐个重试；在途
            // 派发自然按 Waiting* 跳过。
            crate::board_archive_ingest::retry_placed_merges(&deps);
            // R5-BUG-1 根修：停车场只收「评审入口检查点被闸」的登记；已过
            // 检查点、在 LLM 中途被 estop 打断的评审不进队列（直接死亡），
            // drain 完停车场后按态补扫一遍兜底（复用启动重放/评审重放的
            // 守卫矩阵：无结论才重放、转人工 sticky、读失败 fail-closed）。
            // skip 刚唤醒的条目——入口竞态守卫拦不住并发双跑，必须去重。
            replay_stuck_reviews(&deps, &parked);
        }
    });
}

/// 启动重放守卫：该单据当前 in_review 态是否已有评审结论。判据 = 最新
/// `auto_decide` 活动 vs 最近一次**正常**进入 in_review 的转移，按
/// activity_log 插入序（append-only 单调 id，秒级时间戳同秒不可分）比先后：
/// - 无任何 auto_decide → 从未评审过（崩溃杀掉 spawn 的典型窗口）→ 重放；
/// - 结论在转移之后 → 当前态已定案（含转人工——stickiness 必须保持，重放
///   不得翻盘；含 audit.rollback 的人工纠错窗口——rollback 落的
///   `audit_rollback:…` 活动不是 JSON、不被计为正常转移，其 in_review
///   归属上一次已定案的转移 → 不重放）；
/// - 结论在转移之前 → 旧轮结论（如重派后再交付），本轮评审还没跑 → 重放。
///
/// 读失败 fail-closed 按「已定案」处理（不重放）。
pub(super) fn review_already_concluded(store: &nemesis_board::BoardStore, issue_id: i64) -> bool {
    let Ok(acts) = store.list_activity(issue_id) else {
        return true;
    };
    let Some(last_decide) = acts.iter().rposition(|a| a.action == "auto_decide") else {
        return false;
    };
    let last_enter = acts.iter().rposition(|a| {
        a.action == "status_changed"
            && a.details
                .as_deref()
                .and_then(|d| serde_json::from_str::<serde_json::Value>(d).ok())
                .and_then(|v| {
                    v.get("to")
                        .and_then(|t| t.as_str())
                        .map(|t| t == IssueStatus::InReview.as_str())
                })
                .unwrap_or(false)
    });
    match last_enter {
        Some(pos) => last_decide >= pos,
        // 有结论但查不到进入转移（旧数据/手工造数）→ 保守不重放。
        None => true,
    }
}

/// 项目级收口结论探测：任一顶层父单携带本项目的 auto_decide
///（`project_complete` / `project_escalate_human` 记在各父单名下，details
/// 带 `project_id`）= 项目收口评审已跑过（转人工 sticky 同样成立）→ 不重放。
pub(super) fn project_review_already_concluded(
    store: &nemesis_board::BoardStore,
    parent_ids: &[i64],
    project_id: i64,
) -> bool {
    parent_ids.iter().any(|pid| {
        store
            .list_activity(*pid)
            .map(|acts| {
                acts.iter().any(|a| {
                    a.action == "auto_decide"
                        && a.details
                            .as_deref()
                            .and_then(|d| serde_json::from_str::<serde_json::Value>(d).ok())
                            .and_then(|v| v.get("project_id").and_then(|p| p.as_i64()))
                            .map(|p| p == project_id)
                            .unwrap_or(false)
                })
            })
            .unwrap_or(true) // 读失败按已结论处理（不重放）
    })
}

/// 评审重放（2026-09-15 R4-BUG-2 根修；R5-BUG-1 扩展到 estop 释放沿）：
/// 评审 spawn 是纯内存 fire-and-forget 任务，in-flight 评审死于两种场景
/// 都无重触发——① master 重启杀掉（单据/父单永久卡 in_review、项目永久
/// 卡「验收中」，真机实证：父单收口验收 spawn 15s 后 master 被杀，重启后
/// NB-3 永卡 in_review）；② estop 在评审**入口检查点之后**打断（LLM 中途
/// 被杀）——此路不走 `estop_fuse_engaged` 停车登记，release 后 resume
/// watcher 的停车场队列是空的，单据同样永卡（真机实证：NB-16 收口评审
/// LLM 中被打断，release 后 10s 零复活）。与 estop resume watcher（冻结
/// 恢复）互补，这里管「评审死了没结论」的按态恢复：
///
/// - 叶子 in_review 且从未评审（有 Delivery = writeback 真发生过；人工
///   手动挪 in_review 的纯看板单无 Delivery，不扫）→ 重放子单验收；
/// - 父单 in_review 且全子单 done 且从未收口评审 → 重放收口（部分完成态
///   的父单 in_review 属人工/缺口语义，不自动收口）；
/// - 项目 active 且全顶层父单 done 且无项目级结论 → 重放项目收口。
///
/// 级联自愈：重放验收 PASS → on_issue_settled / notify_project_review 链
/// 逐层点火——只要最深的断链复活，上层自动跟上；重复触发由三个评审函数
/// 内建守卫（estop 保险丝 / auto_review 旗标现读 / status==InReview 竞态
/// 让位 / cancelled 缺口转人工）吸收，幂等安全。两个调用点：gateway
/// 装配期（三类钩子注册完成后，`skip` 传空）+ estop 释放沿（resume
/// watcher drain 完停车场后，`skip` 传刚唤醒的条目防同一单双重 spawn——
/// 入口竞态守卫拦不住并发双跑，两个 spawn 都能在写回前通过 in_review
/// 检查）。
pub(crate) fn replay_stuck_reviews(deps: &BoardReviewDeps, skip: &[(ParkedKind, i64)]) {
    let store = &deps.store;
    let parked_awake = |kind: ParkedKind, id: i64| skip.iter().any(|&(k, i)| k == kind && i == id);
    let mut replayed = 0usize;
    match store.list_issues(&nemesis_board::models::IssueFilter {
        status: Some(IssueStatus::InReview),
        ..Default::default()
    }) {
        Ok(issues) => {
            for issue in issues {
                if review_already_concluded(store, issue.id) {
                    continue;
                }
                let Ok(children) = store.list_children(issue.id) else {
                    continue;
                };
                if children.is_empty() {
                    if parked_awake(ParkedKind::Issue, issue.id) {
                        continue;
                    }
                    // 叶子单：writeback 已落定即重放。判据 = 派发行终态
                    //（done/failed）**或** Delivery 评论（兜底人工/旧数据形态）。
                    // 只认 Delivery 是 4a 修复前的缺口：write_back 的评论有
                    // 三形态（结构化=Delivery / 无格式降级 ✅=Comment /
                    // worker 失败 ⛔=Comment），降级与失败形态的单同样转
                    // in_review 进验收决策链（board_dispatch.rs 写回注释），
                    // 重启后却因无 Delivery 被重放漏掉——单据永久卡死无自愈
                    //（2026-09-30 17 场景观察点⑥）。dispatch 终态是更可靠的
                    // 单一判据：write_back 先 finish_dispatch 再 add_comment，
                    // 评论可能失败丢失，终态必落盘；人工改 in_review（无
                    // dispatch 行）仍由 Delivery 分支兜底语义（有交付才算）。
                    let dispatch_settled = store
                        .list_dispatches(issue.id)
                        .map(|ds| {
                            ds.iter().any(|d| {
                                matches!(
                                    d.state.as_str(),
                                    nemesis_board::models::dispatch_state::DONE
                                        | nemesis_board::models::dispatch_state::FAILED
                                )
                            })
                        })
                        .unwrap_or(false);
                    let delivered = dispatch_settled
                        || store
                            .list_comments(issue.id)
                            .map(|cs| cs.iter().any(|c| c.ctype == CommentType::Delivery))
                            .unwrap_or(false);
                    if !delivered {
                        continue;
                    }
                    info!(
                        "[BoardReview] 评审重放：issue {}（{}）in_review 无评审结论 → 重放验收",
                        issue.number, issue.title
                    );
                    spawn_board_review(deps.clone(), issue.id);
                    replayed += 1;
                } else if children.iter().all(|c| c.status == IssueStatus::Done) {
                    if parked_awake(ParkedKind::Parent, issue.id) {
                        continue;
                    }
                    info!(
                        "[BoardReview] 评审重放：父单 {}（{}）全子单 done 无收口结论 → 重放收口验收",
                        issue.number, issue.title
                    );
                    spawn_parent_review(deps.clone(), issue.id);
                    replayed += 1;
                }
            }
        }
        Err(e) => warn!("[BoardReview] 评审重放扫描 in_review 单据失败：{e}"),
    }
    // 项目级：全顶层父单已 done（父单若还卡 in_review，上面的重放链会
    // 在收口落 done 时经 notify_project_review 点火，这里不必抢跑）。
    if let Ok(projects) = store.list_projects() {
        for project in projects {
            if matches!(project.status.as_str(), "completed" | "archived") {
                continue;
            }
            let Ok(parents) = store.list_issues(&nemesis_board::models::IssueFilter {
                project_id: Some(project.id),
                ..Default::default()
            }) else {
                continue;
            };
            let tops: Vec<i64> = parents
                .iter()
                .filter(|i| i.parent_issue_id.is_none())
                .map(|i| i.id)
                .collect();
            if tops.is_empty()
                || parents
                    .iter()
                    .filter(|i| i.parent_issue_id.is_none())
                    .any(|p| p.status != IssueStatus::Done)
            {
                continue;
            }
            if project_review_already_concluded(store, &tops, project.id) {
                continue;
            }
            if parked_awake(ParkedKind::Project, project.id) {
                continue;
            }
            info!(
                "[BoardReview] 评审重放：项目 {}「{}」全父单 done 无收口结论 → 重放项目收口",
                project.id, project.name
            );
            spawn_project_review(deps.clone(), project.id);
            replayed += 1;
        }
    }
    if replayed > 0 {
        info!("[BoardReview] 评审重放完成：{replayed} 个评审/收口任务已重新触发");
    }
}

/// 验收评论落库（作者 = master 节点 agent；失败仅 warn，不炸评审流程）。
pub(super) fn post_review_comment(
    store: &nemesis_board::BoardStore,
    issue_id: i64,
    cluster: &nemesis_cluster::cluster::Cluster,
    content: &str,
) -> Result<(), String> {
    store
        .add_comment(NewComment {
            issue_id,
            author: Actor::agent(cluster.node_id()),
            content: content.to_string(),
            parent_id: None,
            ctype: CommentType::Comment,
        })
        .map(|_| ())
}

// ---------------------------------------------------------------------------
// 看板项目档案 P6（F9）：收口总结生成
//
// 项目 completed 后异步生成档案总结 `<archive>/docs/summary.md`（「AI 生成件」
// 标注 + 生成时间 + 模型名）。复用评审通道（detached 纯文本单轮）+ tier 闸
// （拒 mini，同冲突硬解——小模型写长文不可靠）。**生成失败诚实留痕，不阻塞
// 收口状态机**（completed 在调用前已落定，本函数只做锦上添花）。
// ---------------------------------------------------------------------------

/// 收口总结系统提示词（自由 Markdown，不走评审 verdict JSON——与评审
/// 通道共用的是 LLM 调用通道本身，不是输出格式）。
/// 文本单一真相源在 `nemesis-prompts`（M7 集中化）。
use nemesis_prompts::board::PROJECT_SUMMARY_SYSTEM_PROMPT;

/// F9 触发入口：项目收口（自动 PASS / 人工 project.update → completed）
/// 两条路径都汇到这里。异步不阻塞调用方；内部自守门（estop/tier/档案
/// 目录缺失全部诚实跳过）。
pub(crate) fn spawn_project_summary(deps: BoardReviewDeps, project_id: i64) {
    let store = deps.store.clone();
    let cluster = deps.cluster.clone();
    spawn_watched(store, cluster, None, "项目收口总结", async move {
        if let Err(e) = run_project_summary(&deps, project_id).await {
            warn!("[BoardReview] 项目 {project_id} 收口总结生成失败（不阻塞收口）: {e}");
            summarize_fail_note(&deps, project_id, &e);
        }
    });
}

/// 任务档案自动导出（2026-09-29 goal）：项目收口后把全项目任务记录整合
/// 成自包含目录（`<workspace>/logs/dossiers/`）。与 spawn_project_summary
/// 同源双路径触发（自动 PASS / 人工 project.update → completed 两处调用
/// 点）。纯本机整合（board.db + 项目档案 + 收件箱 + 账本），LLM 零参与
/// ——不受 estop/tier 闸（档案是保留证据，不是 agent 活动）；
/// `board.dossier.auto_on_complete`（默认开）关掉即静默跳过。失败 WARN +
/// 顶层父单系统评论留痕，不阻塞收口。
pub(crate) fn spawn_project_dossier(deps: BoardReviewDeps, project_id: i64) {
    let store = deps.store.clone();
    let cluster = deps.cluster.clone();
    spawn_watched(store, cluster, None, "项目档案导出", async move {
        let flags = match load_board_flags(&deps.home) {
            Ok(f) => f,
            Err(e) => {
                warn!("[BoardReview] 项目 {project_id} 档案导出旗标读取失败（跳过）: {e}");
                return;
            }
        };
        if !flags.dossier.auto_on_complete {
            return;
        }
        if let Err(e) = export_project_dossier(&deps, project_id) {
            warn!("[BoardReview] 项目 {project_id} 档案自动导出失败（不阻塞收口）: {e}");
            dossier_fail_note(&deps, project_id, &e);
        } else {
            info!(
                "[BoardReview] 项目 {project_id} 任务档案已自动导出到 {}/logs/dossiers/",
                deps.workspace.display()
            );
        }
    });
}

/// dossier 导出主体：DataStore 从 moderator_loop 后置解（未装配 = 无账，
/// usage.csv 缺行诚实），导出核心与 WSAPI/CLI 同源（nemesis-board dossier）。
fn export_project_dossier(
    deps: &BoardReviewDeps,
    project_id: i64,
) -> Result<nemesis_board::dossier::DossierOutcome, String> {
    let ds = deps.moderator_loop.get().and_then(|l| l.data_store());
    let usage_of = move |task_id: &str| {
        let ds = ds.as_ref()?;
        ds.aggregate_session_usage_by_task(task_id).ok().map(|agg| {
            nemesis_board::dossier::DossierUsage {
                input_tokens: agg.input_tokens,
                output_tokens: agg.output_tokens,
            }
        })
    };
    let out_root = deps.workspace.join("logs").join("dossiers");
    let outcome = nemesis_board::dossier::export_project(
        deps.store.as_ref(),
        project_id,
        resolve_anchor_root(deps),
        &out_root,
        &usage_of,
    )?;
    // P4-d：自动导出凭证入交付账本（dossier 行，run_id=None 项目级）。
    crate::review_ledger::dossier_delivery(deps.cluster.task_db().as_ref(), project_id, &outcome);
    Ok(outcome)
}

/// dossier 失败留痕：档案 timeline 能写则写 + 顶层父单系统评论（同
/// summarize_fail_note 的「能写哪个写哪个」姿态）。
fn dossier_fail_note(deps: &BoardReviewDeps, project_id: i64, err: &str) {
    if let Ok(project) = deps.store.get_project(project_id) {
        if let Ok(root) = project_archive_root(project.directory.as_deref()) {
            let _ = nemesis_board::archive::append_timeline(
                &root,
                "dossier",
                None,
                "board",
                &format!("任务档案自动导出失败：{err}"),
            );
        }
        if let Ok(issues) = deps.store.list_issues(&nemesis_board::models::IssueFilter {
            project_id: Some(project_id),
            ..Default::default()
        }) {
            let comment = format!(
                "📁 任务档案自动导出失败（不阻塞收口，可 `nemesisbot dossier export --project-id {project_id}` 手动补导）：{err}"
            );
            for p in issues.iter().filter(|i| i.parent_issue_id.is_none()) {
                let _ = deps.store.add_comment(NewComment {
                    issue_id: p.id,
                    author: Actor::agent(deps.cluster.node_id()),
                    content: comment.clone(),
                    parent_id: None,
                    ctype: CommentType::System,
                });
            }
        }
    }
}

/// 失败留痕：顶层父单评论 + 档案 timeline（能写哪个写哪个，都失败只 log）。
pub(super) fn summarize_fail_note(deps: &BoardReviewDeps, project_id: i64, err: &str) {
    let comment =
        format!("📝 AI 收口总结生成失败（不阻塞收口，可人工补写档案 docs/summary.md）：{err}");
    if let Ok(project) = deps.store.get_project(project_id) {
        if let Ok(root) = project_archive_root(project.directory.as_deref()) {
            let _ = nemesis_board::archive::append_timeline(
                &root,
                "summary",
                None,
                "board",
                &format!("收口总结生成失败：{err}"),
            );
        }
        if let Ok(issues) = deps.store.list_issues(&nemesis_board::models::IssueFilter {
            project_id: Some(project_id),
            ..Default::default()
        }) {
            for p in issues.iter().filter(|i| i.parent_issue_id.is_none()) {
                let _ = deps.store.add_comment(NewComment {
                    issue_id: p.id,
                    author: Actor::agent(deps.cluster.node_id()),
                    content: comment.clone(),
                    parent_id: None,
                    ctype: CommentType::System,
                });
            }
        }
    }
}

/// 档案目录解析：Project.directory（绝对路径）存在才可用；缺失 = 存量
/// 项目/目录被手删——诚实跳过（None 语义）。pub(crate)：board_evidence_read
/// 工具按单号解析档案根共用此单点（根语义漂移一处改）。
pub(crate) fn project_archive_root(directory: Option<&str>) -> Result<std::path::PathBuf, String> {
    let raw = directory.ok_or("项目未绑定档案目录（存量项目不参与档案管线）")?;
    let root = std::path::PathBuf::from(raw);
    if !root.is_dir() {
        return Err(format!("档案目录不存在: {}", root.display()));
    }
    Ok(root)
}

/// F9 主体。前置检查全部通过才调 LLM；任何失败向上返回（由调用方留痕）。
pub(super) async fn run_project_summary(
    deps: &BoardReviewDeps,
    project_id: i64,
) -> Result<(), String> {
    // 急停中不跑 LLM（ cosmetic 件不值得占保险丝语义，跳过即可）。
    if deps.estop.is_engaged() {
        return Err("急停（E-STOP）生效中，收口总结跳过".to_string());
    }
    let project = deps.store.get_project(project_id)?;
    if project.status != "completed" {
        // 触发与落定之间的竞态（人工又改回）：诚实跳过。
        return Err(format!(
            "项目状态已非 completed（{}），收口总结跳过",
            project.status
        ));
    }
    let root = project_archive_root(project.directory.as_deref())?;
    let agent_loop = deps
        .moderator_loop
        .get()
        .cloned()
        .ok_or("主 agent 未就绪，收口总结跳过")?;
    // tier 闸拒 mini（同冲突硬解；提示切模型后……无自动重试入口——下次
    // 收口的项目会再试，本单诚实留痕）。
    if matches!(
        agent_loop.tier(),
        nemesis_types::capability::ModelTier::Mini
    ) {
        return Err("当前模型能力档为 mini，收口总结需要 normal/big 档模型".to_string());
    }

    let issues = deps
        .store
        .list_issues(&nemesis_board::models::IssueFilter {
            project_id: Some(project_id),
            ..Default::default()
        })?;
    if issues.is_empty() {
        return Err("项目无任务，无可总结内容".to_string());
    }
    let decisions = deps
        .store
        .list_recent_activity(500, Some("auto_decide"))
        .unwrap_or_default();
    let facts = build_summary_facts(&deps.store, &project, &issues, &decisions);

    let prompt = format!(
        "# 项目「{name}」收口事实清单\n\n{facts}",
        name = project.name
    );
    let raw = agent_loop
        .run_detached(
            &prompt,
            nemesis_agent::r#loop::DetachedOpts {
                system_prompt: Some(PROJECT_SUMMARY_SYSTEM_PROMPT),
                no_tools: true,
                max_turns: 1,
                label: Some("project-summary"),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| format!("LLM 调用失败：{e}"))?;
    let text = raw.trim();
    if text.is_empty() {
        return Err("LLM 返回空内容".to_string());
    }

    let model = agent_loop.active_model();
    let header = format!(
        "> 本文件为 **AI 生成件**（NemesisBot 看板收口总结）。\n\
         > 生成时间：{} ｜ 模型：{model} ｜ 内容由 AI 归纳，请以 records/ 执行档案为准。\n",
        chrono::Local::now().to_rfc3339(),
    );
    let appendix = format!(
        "\n\n---\n\n## 附录：档案结构\n\n{}",
        render_archive_tree(&root, 2)
    );
    let doc = format!("{header}\n{text}{appendix}\n");
    let target = root.join("docs").join("summary.md");
    std::fs::create_dir_all(root.join("docs")).map_err(|e| format!("创建 docs/ 失败: {e}"))?;
    std::fs::write(&target, doc).map_err(|e| format!("写 summary.md 失败: {e}"))?;
    let _ = nemesis_board::archive::append_timeline(
        &root,
        "summary",
        None,
        "board",
        &format!("收口总结生成（模型 {model}）"),
    );
    info!(
        "[BoardReview] 项目 {}「{}」收口总结已生成 → {}",
        project_id,
        project.name,
        target.display()
    );
    Ok(())
}

/// F9 事实清单（纯函数，单测直测）：项目行 + 逐单状态/末评摘录 + 决策流
/// 压缩行。LLM 只看这份清单——不读盘、不猜。
pub(super) fn build_summary_facts(
    store: &nemesis_board::BoardStore,
    project: &nemesis_board::models::Project,
    issues: &[nemesis_board::models::Issue],
    decisions: &[nemesis_board::models::AuditDecisionRow],
) -> String {
    let ids: std::collections::HashSet<i64> = issues.iter().map(|i| i.id).collect();
    let mut out = String::new();
    out.push_str(&format!(
        "- 项目：{}（状态 {}）{}\n",
        project.name,
        project.status,
        if project.description.is_empty() {
            String::new()
        } else {
            format!("：{}", project.description)
        }
    ));
    out.push_str("\n## 任务清单\n");
    for i in issues {
        let depth = if i.parent_issue_id.is_some() {
            "  - "
        } else {
            "- "
        };
        out.push_str(&format!(
            "{depth}{} {}（状态 {}）\n",
            i.number,
            i.title,
            i.status.as_str()
        ));
        // 末条评论摘录（交付/评审结论落点；取 300 字防清单爆炸）。
        if let Ok(comments) = store.list_comments(i.id)
            && let Some(last) = comments.last()
        {
            let excerpt: String = last.content.chars().take(300).collect();
            out.push_str(&format!(
                "    末评[{}/{}]: {excerpt}\n",
                last.author.kind, last.author.id
            ));
        }
    }
    out.push_str("\n## 决策流（自动化决策）\n");
    let mut n = 0usize;
    for d in decisions {
        if !ids.contains(&d.activity.issue_id) {
            continue;
        }
        let kind =
            serde_json::from_str::<serde_json::Value>(d.activity.details.as_deref().unwrap_or(""))
                .ok()
                .and_then(|v| {
                    v.get("decision")
                        .and_then(|x| x.as_str())
                        .map(str::to_string)
                })
                .unwrap_or_else(|| "auto_decide".to_string());
        out.push_str(&format!("- {} {}：{kind}\n", d.issue_number, d.issue_title));
        n += 1;
    }
    if n == 0 {
        out.push_str("（无自动化决策记录）\n");
    }
    out
}

/// 档案结构树（确定性附录；depth 层目录 + 文件行，超深目录以 … 截断）。
pub(super) fn render_archive_tree(root: &std::path::Path, depth: usize) -> String {
    let mut out = String::new();
    fn walk(dir: &std::path::Path, prefix: &str, depth: usize, out: &mut String) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') {
                continue;
            }
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            out.push_str(&format!(
                "{prefix}{}{}\n",
                name,
                if is_dir { "/" } else { "" }
            ));
            if is_dir && depth > 1 {
                walk(&e.path(), &format!("{prefix}  "), depth - 1, out);
            }
        }
    }
    out.push_str("archive/\n");
    walk(root, "  ", depth, &mut out);
    out
}
