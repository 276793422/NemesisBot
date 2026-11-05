//! AOR 批三 5.3：重规划器（replanner）编排层。
//!
//! 纯逻辑层（schema/解析/校验/渲染）在 `nemesis_board::replan`（与
//! planner/review 同构）；本模块负责 LLM 调用编排、触发点挂接与
//! autonomous 执行：
//!
//! - **触发点**（事件驱动四类，全部 fire-and-forget 起草）：
//!   ① 评审链重派无意义停车（fail_class 冻结 / 能力类 / 预算超限 /
//!      EscalateHuman——`board_review.rs` 四挂点）；
//!   ② 父单收口评审 FAIL 转人工（`review_parent_issue`）；
//!   ③ sweep 冻结类停车（`sweep_freeze_park`，unknown_side_effect 等）；
//!   ④ 子树全终态但 FINISH 谓词被未裁决 unknown 阻塞超时（sweep ticker
//!      周期体检兜底——终态后无事件可挂，[`sweep_unknown_block_candidates`]）。
//! - **建议 ≠ 授权**：assisted（默认）只落看板系统评论 @创建者审批；
//!   autonomous 过策略闸（动作数/取消在途/目标解析三闸，任一不过 =
//!   冻结 + 审计评论 + 站内通知——宁可停不可闯）后才执行。
//! - **执行单一真相源**：cancel 走 store 状态机 + `spawn_task_cancel` /
//!   `on_issue_settled`（级联取消/补派/父单联动同款）；modify 走
//!   `update_issue`；add 走 `confirm_plan`（依赖映射/发波/父单 sync 全
//!   复用）——不另搓第二套。
//!
//! LLM 调用形态与 `run_review_llm` 同构：裸提示词 detached（no_tools +
//! max_turns 1）、传输失败即时 Err 不重试、系统终态文本不重试防污染、
//! 解析失败回灌自纠 ≤2 次（共 3 轮）。

use std::collections::HashSet;
use std::sync::{Arc, LazyLock};

use parking_lot::Mutex;
use tracing::{info, warn};

use crate::board_review::{BoardReviewDeps, is_system_terminal_text};

/// 策略闸拒绝的审计评论前缀（系统评论识别标记）：与建议评论同为
/// 「重规划在声」标记——人工评论其后出现即接管（去重闸放行下一轮）。
const BLOCKED_COMMENT_PREFIX: &str = "🛑 重规划建议被拒绝：";

/// 差距评论正文截取上限（证据材料里逐单差距引用的截断）。
const GAP_EXCERPT_BYTES: usize = 1200;
/// 根任务背景/验收标准截断。
const FIELD_EXCERPT_BYTES: usize = 2000;

/// 重规划审计评论作者。
fn replan_actor() -> nemesis_board::Actor {
    nemesis_board::Actor::system("board-replan")
}

fn op_label(op: Option<nemesis_board::replan::ReplanOp>) -> &'static str {
    match op {
        Some(nemesis_board::replan::ReplanOp::Cancel) => "取消",
        Some(nemesis_board::replan::ReplanOp::Modify) => "修改",
        Some(nemesis_board::replan::ReplanOp::Add) => "新增",
        None => "?",
    }
}

/// 建议评论在声状态（去重闸 + 版本号单一出处）。
struct ProposalState {
    /// 最新建议/拒绝类系统评论比最新人工评论新 = 建议仍在等人工表态
    /// （或已被闸拒绝待人工接管）→ 跳过起草。
    pending: bool,
    /// 下一个建议版本号 = 既有建议评论数 + 1。
    version: u64,
}

/// 剥离 assisted 建议评论的 @提及前缀（BUG-20261105-010）：assisted 模式
/// 落盘形态是 `@<creator> {PROPOSAL_COMMENT_PREFIX}…`（mention 必须在
/// 开头才有提醒语义），识别处 `starts_with` 对其直接失配 → 去重闸永久
/// 失效 + 版本号恒 v1。只剥一次 `@<id> ` 形态，无前缀原样返回。
fn strip_assisted_mention(content: &str) -> &str {
    if let Some((_, rest)) = content.strip_prefix('@').and_then(|s| s.split_once(' ')) {
        return rest;
    }
    content
}

/// 扫描根单评论得出建议在声状态。两类识别标记：建议评论
///（`PROPOSAL_COMMENT_PREFIX`）与策略闸拒绝评论（[`BLOCKED_COMMENT_PREFIX`]），
/// 都是 system 评论（建议评论允许带 assisted @提及前缀，剥掉后比对）；
/// admin 评论（按插入序）出现即接管。
fn proposal_state(
    store: &nemesis_board::BoardStore,
    root_id: i64,
) -> Result<ProposalState, String> {
    let comments = store.list_comments(root_id)?;
    let mut last_replan: Option<i64> = None;
    let mut last_admin: Option<i64> = None;
    let mut proposal_count: u64 = 0;
    for c in &comments {
        let body = strip_assisted_mention(&c.content);
        let is_replan = c.ctype == nemesis_board::CommentType::System
            && (body.starts_with(nemesis_board::replan::PROPOSAL_COMMENT_PREFIX)
                || body.starts_with(BLOCKED_COMMENT_PREFIX));
        if is_replan {
            last_replan = Some(c.id);
            if body.starts_with(nemesis_board::replan::PROPOSAL_COMMENT_PREFIX) {
                proposal_count += 1;
            }
        }
        if c.author.kind == "admin" {
            last_admin = Some(c.id);
        }
    }
    let pending = match (last_replan, last_admin) {
        (Some(r), Some(a)) => r > a,
        (Some(_), None) => true,
        (None, _) => false,
    };
    Ok(ProposalState {
        pending,
        version: proposal_count + 1,
    })
}

// ---------- 起草 in-flight 去重（BUG-20261105-011）----------

/// 起草中根单集合（进程内）。去重闸检查与建议落盘之间隔着分钟级 LLM
/// 起草，评审 FAIL / sweep ticker / 收口评审等多触发源可在窗口内对同一
/// 根单双过闸，各跑一次完整 LLM 后落两份重复建议（版本号同为 v1）。
/// claim 在 root 解析后立刻占位（spawn 层只有 trigger_issue_id 拿不到
/// root.id，必须放 draft 主体内），guard RAII 释放（`?` 早退路径覆盖）。
static REPLAN_INFLIGHT: LazyLock<Mutex<HashSet<i64>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

struct ReplanInflightGuard(i64);

impl Drop for ReplanInflightGuard {
    fn drop(&mut self) {
        REPLAN_INFLIGHT.lock().remove(&self.0);
    }
}

/// 尝试占位：插入成功返回 guard，已有在飞起草返回 None。
fn claim_replan_inflight(root_id: i64) -> Option<ReplanInflightGuard> {
    let mut g = REPLAN_INFLIGHT.lock();
    if g.insert(root_id) {
        Some(ReplanInflightGuard(root_id))
    } else {
        None
    }
}

/// 触发点④ 候选扫描（sweep ticker 周期体检）：非终态单满足「直接子单
/// 全终态 + 子单存在未裁决 unknown 批 + 最老批龄 ≥ timeout」→ 返回该单
/// id（起草路径内还有去重/开关/评审引擎闸，此处只做粗筛）。estop 生效中 /
/// timeout=0 恒空。
pub(crate) fn sweep_unknown_block_candidates(
    deps: &BoardReviewDeps,
    timeout_secs: u64,
) -> Vec<i64> {
    if timeout_secs == 0 || deps.estop.is_engaged() {
        return Vec::new();
    }
    let store = &deps.store;
    let issues = match store.list_issues(&nemesis_board::IssueFilter::default()) {
        Ok(v) => v,
        Err(e) => {
            warn!("[BoardReplan] 候选扫描 list_issues 失败：{e}");
            return Vec::new();
        }
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut out = Vec::new();
    for issue in &issues {
        if issue.status.is_terminal() {
            continue;
        }
        let children = match store.list_children(issue.id) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if children.is_empty() {
            continue;
        }
        // 直接子单层全终态——在途子单有事件链兜底，不需要本扫描。
        if children.iter().any(|c| !c.status.is_terminal()) {
            continue;
        }
        let batches = nemesis_board::finish::pending_unknown_batches(store, &children);
        if batches.is_empty() {
            continue;
        }
        let oldest = batches
            .iter()
            .map(|b| b.oldest_created_at.max(0) as u64)
            .min()
            .unwrap_or(0);
        if oldest == 0 || now.saturating_sub(oldest) < timeout_secs {
            continue;
        }
        out.push(issue.id);
    }
    out
}

/// 证据材料：根任务 + 子单现状（失败分类/差距/未裁决 unknown/[TOUCH]）
/// + 资源冲突（同一 [TOUCH] 声明被多单重复声明）。
fn build_replan_user_prompt(
    root: &nemesis_board::Issue,
    children: &[nemesis_board::Issue],
    store: &nemesis_board::BoardStore,
    trigger_note: &str,
) -> String {
    use nemesis_board::review::truncate_bytes;
    let mut p = String::new();
    p.push_str(&format!(
        "{} {}\n",
        nemesis_board::report::TASK_CARD_HEADER,
        root.number
    ));
    p.push_str(&format!("触发源：{trigger_note}\n"));
    p.push_str(&format!("标题：{}\n", root.title));
    p.push_str(&format!(
        "\n## 根任务背景\n{}\n",
        if root.description.trim().is_empty() {
            "（未提供）".to_string()
        } else {
            truncate_bytes(root.description.trim(), FIELD_EXCERPT_BYTES)
        }
    ));
    let ac = root
        .acceptance_criteria
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| truncate_bytes(s, FIELD_EXCERPT_BYTES))
        .unwrap_or_else(|| "（未提供）".to_string());
    p.push_str(&format!("\n## 根任务验收标准\n{ac}\n"));

    p.push_str("\n## 子单现状\n");
    for c in children {
        p.push_str(&format!(
            "\n### {}「{}」— 状态 {}\n",
            c.number,
            c.title,
            c.status.as_str()
        ));
        let comments = store.list_comments(c.id).unwrap_or_default();
        if let Some(fc) = crate::board_review::latest_fail_class(&comments) {
            p.push_str(&format!("- 最近失败分类：{fc}\n"));
        }
        // 最新差距引用（评审 FAIL 评论的「## 差距」段取尾；`Split<&str>`
        // 非 DoubleEnded，取尾段用 rsplit 正向 first）。
        if let Some(gap) = comments
            .iter()
            .rev()
            .find(|cm| cm.content.contains("## 差距"))
            .and_then(|cm| {
                cm.content
                    .rsplit("## 差距")
                    .next()
                    .map(|s| s.trim().to_string())
            })
            .filter(|s| !s.is_empty())
        {
            p.push_str(&format!(
                "- 最新差距：{}\n",
                truncate_bytes(&gap, GAP_EXCERPT_BYTES)
            ));
        }
        // 未裁决 unknown 批次（触发点④素材）。
        for b in nemesis_board::finish::pending_unknown_batches(store, std::slice::from_ref(c)) {
            p.push_str(&format!("- 未裁决 unknown：{}\n", b.items.join("；")));
        }
        // [TOUCH] 资源声明。
        if let Some(ac) = c.acceptance_criteria.as_deref() {
            let touches = nemesis_board::planner::parse_touch_paths(ac);
            if !touches.is_empty() {
                p.push_str(&format!("- [TOUCH] 声明：{}\n", touches.join(", ")));
            }
        }
    }

    // [TOUCH] 冲突：同一声明出现在多个子单。
    let mut touch_owners: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for c in children {
        if let Some(ac) = c.acceptance_criteria.as_deref() {
            for t in nemesis_board::planner::parse_touch_paths(ac) {
                touch_owners.entry(t).or_default().push(c.number.clone());
            }
        }
    }
    let conflicts: Vec<String> = touch_owners
        .into_iter()
        .filter(|(_, owners)| owners.len() > 1)
        .map(|(path, owners)| format!("- {path}：{}", owners.join("、")))
        .collect();
    if !conflicts.is_empty() {
        p.push_str("\n## [TOUCH] 资源冲突（同一写资源被多单声明）\n");
        p.push_str(&conflicts.join("\n"));
        p.push('\n');
    }

    p.push_str(
        "\n请对照上述证据评估当前计划是否需要调整（保已验证工作、只增删/改证据必要的部分、\
         修依赖与 Touch、每项建议附证据引用），只输出 JSON 对象。",
    );
    p
}

/// 裸提示词 detached 重规划调用：首跑 + `parse_replan` 失败回灌重试 ≤2 次
/// （共 3 轮）。传输失败即时 Err 不重试；系统终态文本直接 Err 不重试防
/// 污染（与 `run_review_llm` 同款纪律）。
async fn run_replan_llm(
    agent_loop: &Arc<nemesis_agent::r#loop::AgentLoop>,
    prompt: &mut String,
) -> Result<nemesis_board::replan::ReplanOutput, String> {
    let system_prompt = nemesis_board::replan::replan_system_prompt();
    let mut last_err = String::new();
    for _ in 0..=2 {
        let opts = nemesis_agent::r#loop::DetachedOpts {
            system_prompt: Some(system_prompt),
            no_tools: true,
            max_turns: 1,
            label: Some("board-replan"),
            ..Default::default()
        };
        let raw = match agent_loop.run_detached(prompt, opts).await {
            Ok(raw) => raw,
            Err(e) => return Err(format!("LLM 调用失败：{e}")),
        };
        if is_system_terminal_text(&raw) {
            return Err(format!(
                "重规划输出为系统终态文本（非评审结论，不重试防污染）：{raw}"
            ));
        }
        match nemesis_board::replan::parse_replan(&raw) {
            Ok(out) => return Ok(out),
            Err(e) => {
                *prompt = nemesis_board::replan::build_replan_retry_prompt(&raw, &e);
                last_err = e.message;
            }
        }
    }
    Err(format!("重规划输出连续 3 轮无法解析：{last_err}"))
}

/// 触发点挂点（fire-and-forget 起草）：panic 看护 + 失败 warn（人工兜底
/// ——重规划是增强件，起草失败不阻塞主流程）。无活动 tokio runtime 的
/// 同步调用方（sweep_freeze_park 直调测试等）诚实跳过——spawn panic
/// 反而炸测试进程，宁可静默降级。
pub(crate) fn spawn_replan_draft(
    deps: BoardReviewDeps,
    trigger_issue_id: i64,
    trigger_source: &'static str,
) {
    if tokio::runtime::Handle::try_current().is_err() {
        warn!(
            "[BoardReplan] 无活动 runtime，跳过起草（issue {trigger_issue_id}，{trigger_source}）"
        );
        return;
    }
    let store = deps.store.clone();
    let cluster = deps.cluster.clone();
    crate::board_review::spawn_watched(
        store,
        cluster,
        Some(trigger_issue_id),
        "重规划起草",
        async move {
            match draft_replan_proposal(&deps, trigger_issue_id, trigger_source).await {
                Ok(_) => {} // 跳过/闭环结果已在 draft_replan_proposal 内记日志
                Err(e) => warn!(
                    "[BoardReplan] issue {trigger_issue_id} 起草失败（{trigger_source}）：{e}（人工兜底）"
                ),
            }
        },
    );
}

/// 起草一次重规划建议（触发点①②③④ 共用主体）。返回 `Ok(false)` =
/// 条件不满足跳过（开关关/estop/去重闸/无评审引擎），`Ok(true)` = 建议
/// 已落盘或自治链已闭环。
pub(crate) async fn draft_replan_proposal(
    deps: &BoardReviewDeps,
    trigger_issue_id: i64,
    trigger_source: &str,
) -> Result<bool, String> {
    // estop 不变量：急停生效中不做任何自治动作。
    if deps.estop.is_engaged() {
        info!("[BoardReplan] estop 生效中，跳过起草（issue {trigger_issue_id}）");
        return Ok(false);
    }
    // 开关与模式：board.replan.* + board.autonomy（fail-closed 读失败 =
    // 关 + assisted）。
    let flags = match crate::board_review::load_board_flags(&deps.home) {
        Ok(f) => f,
        Err(e) => {
            info!("[BoardReplan] 配置读取失败（视为关闭）：{e}");
            return Ok(false);
        }
    };
    if !flags.replan.enabled {
        return Ok(false);
    }
    let cfg = &flags.replan;
    let assisted = flags.autonomy != nemesis_config::BoardAutonomy::Autonomous;
    // 评审引擎（主 loop 后置装配桥）：未就绪诚实跳过。
    let Some(agent_loop) = deps.moderator_loop.get() else {
        info!("[BoardReplan] 评审引擎未就绪，跳过起草（issue {trigger_issue_id}）");
        return Ok(false);
    };
    let store = &deps.store;
    let trigger = store.get_issue(trigger_issue_id)?;
    // 根 = 触发单的父单（子单触发）/ 触发单自身（父单触发）。
    let root = match trigger.parent_issue_id {
        Some(pid) => store.get_issue(pid)?,
        None => trigger.clone(),
    };
    // 去重闸：最新建议/拒绝类系统评论比最新人工评论新 = 建议仍在声，
    // 不重复起草（人工表态后自动放行下一轮）。
    // in-flight 去重（BUG-20261105-011）：去重闸读的是评论表，而本任务
    // 的建议要等分钟级 LLM 起草完才落盘——窗口内并发触发（评审 FAIL 与
    // sweep ticker 同根单齐发）会双双读到 pending=false。同根单同时只放
    // 一个起草在飞，guard 在所有返回路径上 RAII 释放。
    let _inflight = match claim_replan_inflight(root.id) {
        Some(g) => g,
        None => {
            info!(
                "[BoardReplan] {} 起草已在飞（并发触发去重），跳过本次",
                root.number
            );
            return Ok(false);
        }
    };
    let state = proposal_state(store, root.id)?;
    if state.pending {
        info!(
            "[BoardReplan] {} 已有在声重规划建议（待人工表态），跳过起草",
            root.number
        );
        return Ok(false);
    }
    let version = state.version;
    let children = store.list_children(root.id)?;
    let trigger_note = format!("{trigger_source}（触发单 {}）", trigger.number);

    // ---- 起草 ----
    let mut prompt = build_replan_user_prompt(&root, &children, store, &trigger_note);
    let output = run_replan_llm(agent_loop, &mut prompt).await?;

    // ---- 建议评论落盘（assisted/autonomous 共同面）----
    let proposal_comment = nemesis_board::replan::render_proposal_comment(
        version,
        &output.analysis,
        &output.actions,
        &trigger_note,
    );
    let mention = if root.creator.kind == "admin" && assisted {
        format!("@{} ", root.creator.id)
    } else {
        String::new()
    };
    let full_comment = if assisted {
        format!(
            "{mention}{proposal_comment}\n（assisted 模式：本建议仅为提案，请人工审阅后处置——\
             同意取消/修改可直接操作看板；同意新增可人工拆解，或切 autonomy=autonomous \
             后由系统执行。）"
        )
    } else {
        proposal_comment.clone()
    };
    store.add_comment(nemesis_board::NewComment {
        issue_id: root.id,
        author: replan_actor(),
        content: full_comment,
        parent_id: None,
        ctype: nemesis_board::CommentType::System,
    })?;
    crate::board_review::record_auto_decide(
        deps,
        root.id,
        "replan_propose",
        trigger_source,
        serde_json::json!({
            "version": version,
            "actions": output.actions.len(),
            "trigger_issue": trigger.number,
            "assisted": assisted,
        }),
    );
    info!(
        "[BoardReplan] {} 重规划建议 v{version} 已落盘（{} 动作，{trigger_source}，{}）",
        root.number,
        output.actions.len(),
        if assisted {
            "assisted 待审批"
        } else {
            "autonomous 执行中"
        }
    );

    if assisted {
        return Ok(true);
    }

    // ---- autonomous：策略闸（宁可停不可闯）+ 三段执行——主体提取在
    // autonomous_execute（LLM 无关，单测直驱覆盖执行链）。
    autonomous_execute(
        deps,
        &root,
        &children,
        &output,
        version,
        trigger_source,
        cfg,
    )?;
    Ok(true)
}

/// autonomous 策略闸 + 执行主体（闸拒绝 = 🛑 审计评论 + REPLAN_BLOCKED
/// 站内通知 + 审计漏斗，不部分执行——宁可停不可闯；全过 = cancel →
/// modify → add 三段执行，单动作失败 warn 继续、差异评论诚实记账）。
/// 与 LLM 完全无关：`draft_replan_proposal` 起草后调用；单测直驱本函数
/// 覆盖执行链。
fn autonomous_execute(
    deps: &BoardReviewDeps,
    root: &nemesis_board::Issue,
    children: &[nemesis_board::Issue],
    output: &nemesis_board::replan::ReplanOutput,
    version: u64,
    trigger_source: &str,
    cfg: &nemesis_config::BoardReplanConfig,
) -> Result<(), String> {
    // EST-03 同款长跑复查（2026-11-05 R528）：起草入口查过一次 estop，但
    // 起草到执行之间隔着分钟级评审 LLM——发起时未生效不代表结束时仍未
    // 生效。急停生效中不做任何自治动作（本模块不变量；config 侧
    // BoardAutonomy 文档同钉「estop 与外部熔断权不受此字段影响」）：建议
    // 评论已先行落盘（人工可见，形态等同 assisted 提案），此处放弃执行
    // 留给人工处置，不另发停车评论（急停期间操作员要的是安静）。
    if deps.estop.is_engaged() {
        info!(
            "[BoardReplan] {} 计划 v{version} 起草期间 estop 生效，放弃自治执行（建议已落盘待人工处置，{trigger_source}）",
            root.number
        );
        return Ok(());
    }
    let store = &deps.store;
    let violations = evaluate_policy_gates(root, children, output, cfg);
    if !violations.is_empty() {
        let msg = format!(
            "{BLOCKED_COMMENT_PREFIX}计划 v{version} 未执行。被以下约束拒绝：\n- {}\n\n\
             请人工裁决后处置（调整 board.replan.* 约束或人工执行建议）。",
            violations.join("\n- ")
        );
        store.add_comment(nemesis_board::NewComment {
            issue_id: root.id,
            author: replan_actor(),
            content: msg,
            parent_id: None,
            ctype: nemesis_board::CommentType::System,
        })?;
        crate::board_review::record_auto_decide(
            deps,
            root.id,
            "replan_blocked",
            trigger_source,
            serde_json::json!({
                "version": version,
                "violations": violations,
            }),
        );
        if let Err(e) = store.notify_dispatch_event(
            root.id,
            nemesis_board::notification_kind::REPLAN_BLOCKED,
            &format!(
                "重规划计划 v{version} 被策略闸拒绝：{}",
                violations.join("；")
            ),
        ) {
            warn!("[BoardReplan] notify replan_blocked 失败：{e}");
        }
        warn!(
            "[BoardReplan] {} 计划 v{version} 被策略闸拒绝（{} 项）：{}",
            root.number,
            violations.len(),
            violations.join("；")
        );
        return Ok(());
    }

    // ---- 执行（闸全过：cancel → modify → add 三段；单动作失败 warn 继续，
    // 差异评论诚实记账）----
    let actor = replan_actor();
    let mut cancelled: Vec<String> = Vec::new();
    let mut modified: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    for a in &output.actions {
        if a.op != Some(nemesis_board::replan::ReplanOp::Cancel) {
            continue;
        }
        let Some(child) = children.iter().find(|c| c.number == a.number.trim()) else {
            skipped.push(format!("取消 {}（编号不在直接子单集合）", a.number.trim()));
            continue;
        };
        if child.status.is_terminal() {
            skipped.push(format!(
                "{}（已终态 {}）",
                child.number,
                child.status.as_str()
            ));
            continue;
        }
        let _ = store.add_comment(nemesis_board::NewComment {
            issue_id: child.id,
            author: replan_actor(),
            content: format!(
                "🔁 重规划取消（计划 v{version}）：{}\n证据：{}",
                a.reason.trim(),
                a.evidence.trim()
            ),
            parent_id: None,
            ctype: nemesis_board::CommentType::System,
        });
        // 在途派发连带取消（竞态守卫赢才动派发行 + 下行 task_cancel）。
        if let Ok(Some(d)) = store.get_active_dispatch(child.id) {
            match store.cancel_dispatch(&d.task_id, &actor) {
                Ok(Some(_)) => {
                    nemesis_web::handlers::board::spawn_task_cancel(
                        store,
                        &deps.cluster,
                        child.id,
                        &d.worker_id,
                        &d.task_id,
                    );
                }
                Ok(None) => {} // 输竞态：transition 状态机会诚实拒绝
                Err(e) => warn!("[BoardReplan] {} 在途派发取消失败：{e}", child.number),
            }
        }
        match store.transition_issue(child.id, nemesis_board::IssueStatus::Cancelled, &actor) {
            Ok(_) => {
                cancelled.push(child.number.clone());
                // 落定联动单一出口（父单 sync + 级联取消 + 项目收口 notify）。
                nemesis_web::handlers::board::on_issue_settled(
                    store,
                    Some(&deps.cluster),
                    child.id,
                    &actor,
                );
            }
            Err(e) => {
                skipped.push(format!("{}（取消落状态失败：{e}）", child.number));
                warn!("[BoardReplan] {} 取消落状态失败（跳过）：{e}", child.number);
            }
        }
    }
    for a in &output.actions {
        if a.op != Some(nemesis_board::replan::ReplanOp::Modify) {
            continue;
        }
        let Some(child) = children.iter().find(|c| c.number == a.number.trim()) else {
            skipped.push(format!("修改 {}（编号不在直接子单集合）", a.number.trim()));
            continue;
        };
        let title = non_empty(&a.title);
        let description = non_empty(&a.description);
        let acceptance_criteria = non_empty(&a.acceptance_criteria);
        if title.is_none() && description.is_none() && acceptance_criteria.is_none() {
            continue;
        }
        match store.update_issue(
            child.id,
            &nemesis_board::IssuePatch {
                title,
                description,
                acceptance_criteria,
                ..Default::default()
            },
            &actor,
        ) {
            Ok(_) => modified.push(child.number.clone()),
            Err(e) => {
                skipped.push(format!("{}（修改失败：{e}）", child.number));
                warn!("[BoardReplan] {} 修改失败（跳过）：{e}", child.number);
            }
        }
    }
    let add_count = output
        .actions
        .iter()
        .filter(|a| a.op == Some(nemesis_board::replan::ReplanOp::Add))
        .count();
    let mut added_ok = false;
    if add_count > 0 {
        let adds = nemesis_board::replan::replan_adds_as_planned(output);
        match nemesis_web::handlers::board::confirm_plan(
            store,
            Some(&deps.cluster),
            root,
            adds,
            &actor,
            &[],
        ) {
            Ok(_) => added_ok = true,
            Err(e) => {
                skipped.push(format!("新增 {add_count} 条子单失败：{e}"));
                warn!("[BoardReplan] {} 新增子单失败：{e}", root.number);
            }
        }
    }

    // ---- 差异评论（每次重估追加「计划 vN 执行差异」）----
    let mut diff = format!("🔁 重规划执行差异（计划 v{version}，{trigger_source}）：\n");
    if cancelled.is_empty() && modified.is_empty() && !added_ok && skipped.is_empty() {
        diff.push_str("\n（无实际改动——建议为空或全部动作跳过）\n");
    } else {
        if !cancelled.is_empty() {
            diff.push_str(&format!("\n- 取消：{}\n", cancelled.join("、")));
        }
        if !modified.is_empty() {
            diff.push_str(&format!("\n- 修改：{}\n", modified.join("、")));
        }
        if added_ok {
            diff.push_str(&format!(
                "\n- 新增子单：{add_count} 条（已按依赖落库发波）\n"
            ));
        }
        if !skipped.is_empty() {
            diff.push_str(&format!("\n- 跳过/失败：{}\n", skipped.join("、")));
        }
    }
    store.add_comment(nemesis_board::NewComment {
        issue_id: root.id,
        author: replan_actor(),
        content: diff,
        parent_id: None,
        ctype: nemesis_board::CommentType::System,
    })?;
    crate::board_review::record_auto_decide(
        deps,
        root.id,
        "replan_execute",
        trigger_source,
        serde_json::json!({
            "version": version,
            "cancelled": cancelled,
            "modified": modified,
            "added": if added_ok { add_count } else { 0 },
            "skipped": skipped,
        }),
    );
    info!(
        "[BoardReplan] {} 计划 v{version} 已自动执行：取消 {}、修改 {}、新增 {}",
        root.number,
        cancelled.len(),
        modified.len(),
        if added_ok { add_count } else { 0 }
    );
    Ok(())
}

/// Option 化非空 trim 文本（modify 臂字段映射）。
fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

/// autonomous 策略闸评估：返回违反清单（空 = 全过）。
/// - **闸 A**：add 动作数 > `max_new_subissues`；
/// - **闸 B**：cancel 目标是非终态子单且 `allow_cancel_active=false`；
/// - **闸 C**：cancel/modify 的 number 必须能解析为根单的直接子单
///   （cancel 目标已终态不算违反——执行时按 no-op 跳过）。
///
/// 任一违反 → 全部拒绝，不部分执行（宁可停不可闯）。
fn evaluate_policy_gates(
    root: &nemesis_board::Issue,
    children: &[nemesis_board::Issue],
    output: &nemesis_board::replan::ReplanOutput,
    cfg: &nemesis_config::BoardReplanConfig,
) -> Vec<String> {
    let mut violations: Vec<String> = Vec::new();
    let add_count = output
        .actions
        .iter()
        .filter(|a| a.op == Some(nemesis_board::replan::ReplanOp::Add))
        .count();
    if add_count > cfg.max_new_subissues as usize {
        violations.push(format!(
            "闸 A：新增子单 {add_count} 条超过上限 {}（board.replan.max_new_subissues）",
            cfg.max_new_subissues
        ));
    }
    for (i, a) in output.actions.iter().enumerate() {
        let op = match a.op {
            Some(
                op @ (nemesis_board::replan::ReplanOp::Cancel
                | nemesis_board::replan::ReplanOp::Modify),
            ) => op,
            _ => continue,
        };
        let num = a.number.trim();
        let Some(child) = children.iter().find(|c| c.number == num) else {
            violations.push(format!(
                "闸 C：actions[{i}] 的 {}（{num}）不在 {} 的直接子单集合\
                 （重规划作用域限定本树：已归档/他树/不存在的编号一律拒绝）",
                op_label(a.op),
                root.number
            ));
            continue;
        };
        if op == nemesis_board::replan::ReplanOp::Cancel
            && !child.status.is_terminal()
            && !cfg.allow_cancel_active
        {
            violations.push(format!(
                "闸 B：actions[{i}] 取消非终态子单 {num}（状态 {}）需要 \
                 board.replan.allow_cancel_active=true（当前 false）",
                child.status.as_str()
            ));
        }
    }
    violations
}

#[cfg(test)]
mod tests;
