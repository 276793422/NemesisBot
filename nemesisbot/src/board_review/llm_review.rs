// ---------------------------------------------------------------------------
// 机械拆分：本文件内容由 board_review.rs 原样搬入（纯代码搬家，行为零变更）。
// ---------------------------------------------------------------------------

use super::*;

/// 模式选择（纯函数；单测钉死）：仅当配置轮数 >1（或 **0=不限**）才开
/// 执行型取证。最近派发 worker 是本机节点 → `ReadOnly`（全只读白名单）；
/// 远端 worker → `EvidenceOnly`（档案围栏工具单件——工件虽在远端执行，
/// 交付档案已合并到本机，可取证）；无派发历史（人工转入 in_review 的手工
/// 单）保持 `NoTools` 历史形态（零派发=零档案合并记录，工具无输入）。
/// `max_turns=1` 恒为 `NoTools`（与历史行为字节等价）。
///
/// `cfg_max_turns=0` = 不限取证轮数（对齐 max_tool_iterations /
/// llm_timeout_secs 的 0=不限惯例； web-FPS 评审预算耗尽事故
/// 根）：映射 `u32::MAX` 哨兵——`DetachedOpts.max_turns` 的
/// 0 语义是「回落主配置默认」而非不限（`loop/turn.rs`），不能直传。不限档
/// 下 agent 层守卫照常生效（turn_guard doom-loop 检测 / estop / 取消），
/// 摘掉的只是人为轮数顶棚。
pub(crate) fn pick_review_tool_mode(
    last_worker: Option<&str>,
    local_node: &str,
    cfg_max_turns: u32,
) -> ReviewToolMode {
    let unlimited = cfg_max_turns == 0;
    if !(cfg_max_turns > 1 || unlimited) {
        return ReviewToolMode::NoTools;
    }
    let max_turns = if unlimited { u32::MAX } else { cfg_max_turns };
    match last_worker {
        Some(w) if w == local_node => ReviewToolMode::ReadOnly { max_turns },
        Some(_) => ReviewToolMode::EvidenceOnly { max_turns },
        None => ReviewToolMode::NoTools,
    }
}

/// （机制加固）：评审预算感知注入块——告知评审员可用取证
/// 轮数，预算内证据仍不足只能 UNSURE（转人工），严禁猜 PASS。 事故
/// （评审 2 轮预算耗尽无结论 → 合格交付被反复重派）的提示词
/// 臂：评审员不知道自己有几轮，烧完预算被强停时连「证据不足」的结论都
/// 没来得及出；知道预算才会边取证边收敛。
pub(crate) fn render_review_budget_block(tool_mode: ReviewToolMode) -> String {
    match tool_mode {
        ReviewToolMode::NoTools => "\n## 评审预算（重要）\n本次评审没有工具、只有这一轮：你看到的材料就是全部证据。证据不足以客观判定时，结论只能是 UNSURE（转人工裁决），严禁凭推测给出 PASS。\n".to_string(),
        ReviewToolMode::ReadOnly { max_turns } if max_turns == u32::MAX => "\n## 评审预算（重要）\n本次评审为执行型：你可用只读工具就地取证，轮数不限（但仍须克制，证据足够即定案，不得为取证而取证）。穷尽合理取证仍无法客观判定时，结论只能是 UNSURE（转人工裁决），严禁凭推测给出 PASS。\n".to_string(),
        ReviewToolMode::ReadOnly { max_turns } => format!(
            "\n## 评审预算（重要）\n本次评审为执行型：你可用只读工具就地取证，预算 **{max_turns} 轮**。请把预算花在验收标准里最关键、可客观判定的点上，边取证边收敛；预算内证据仍不足以定案时，结论只能是 UNSURE（转人工裁决），严禁在预算将尽时凭推测抢答 PASS。\n"
        ),
        ReviewToolMode::EvidenceOnly { max_turns } if max_turns == u32::MAX => "\n## 评审预算（重要）\n本次评审为执行型：交付档案已由系统合并到本机，你可用 board_evidence_read 工具按档案内相对路径读取交付文件内容取证，轮数不限（但仍须克制，证据足够即定案，不得为取证而取证）。穷尽合理取证仍无法客观判定时，结论只能是 UNSURE（转人工裁决），严禁凭推测给出 PASS。\n".to_string(),
        ReviewToolMode::EvidenceOnly { max_turns } => format!(
            "\n## 评审预算（重要）\n本次评审为执行型：交付档案已由系统合并到本机，你可用 board_evidence_read 工具按档案内相对路径读取交付文件内容取证，预算 **{max_turns} 轮**。请把预算花在验收标准里最关键、可客观判定的点上，边取证边收敛；预算内证据仍不足以定案时，结论只能是 UNSURE（转人工裁决），严禁在预算将尽时凭推测抢答 PASS。\n"
        ),
    }
}

/// 未附内容文件的注记（纯函数，单测钉死；两臂分流
/// evidence-on-demand）。有工具时指路 `board_evidence_read`（快路径没覆盖
/// 到的内容由评审员自取——正确性通路），NoTools 保持历史文本（测试
/// 4188 钉「未附内容」子串）。
pub(crate) fn render_unattached_note(unattached: usize, tool_available: bool) -> String {
    if tool_available {
        format!(
            "（其余 {unattached} 个文件未附内容：二进制 / 超大 / 内容节选名额耗尽——清单仅证实落盘；需要内容时用 board_evidence_read 工具自行读取：issue 传本单单号，path 传档案内相对路径（上面清单里的路径原样），支持 offset/limit 字符分段）\n"
        )
    } else {
        format!(
            "（其余 {unattached} 个文件未附内容：二进制 / 超大 / 单文件节选名额耗尽——仅以清单证实落盘）\n"
        )
    }
}

/// 裸提示词 detached 评审调用：首跑 + `parse_review` 失败回灌重试 ≤2 次
/// （共 3 轮）。成功返回解析输出；3 轮全败返回末次错误。`mode` 控制
/// 工具面（NoTools = 历史行为字节等价）。
///
/// 发现 D（措辞精度）：LLM **调用**失败（网络/超时/上游错误）经
/// `run_detached` 的 Err 在首轮立即返回——不消耗解析重试轮，与本函数内
/// **解析**失败的「连续 3 轮」是两类故障。返回值文本分别带前缀区分，
/// 下游评论不再把调用失败误报成「连续 3 次无法解析」。
///
/// 防污染（压测①b）：`raw` 是 agent 引擎的系统终态文本
/// （急停/取消/轮次预算暂停）时不重试——系统文本解析必败，回灌只会让
/// 下一轮模型围绕「上一轮为何暂停」作答（实证：理由=「上一轮输出
/// 为定时任务预算暂停的系统提示文本」→ UNSURE 转人工，污染评审记录）。
/// 直接 Err 诚实转人工，不烧重试轮。
pub(super) async fn run_review_llm(
    agent_loop: &Arc<nemesis_agent::r#loop::AgentLoop>,
    prompt: &mut String,
    mode: ReviewToolMode,
    tier: nemesis_prompts::board::ReviewTier,
    // /（profession-capability-evolution）：评审分镜 + 服务
    // 端预取的禁令节引用（只进 constraint 镜；None = 历史行为字节等价）。
    lens: Option<&'static nemesis_prompts::board::ReviewLens>,
    lens_reference: Option<&str>,
    // 缺口清账机械闸：上一轮落下的编号清单（空 = 首评，只查虚构编号）。
    prior_gaps: &[nemesis_board::LedgerItem],
) -> Result<nemesis_board::ReviewOutput, String> {
    // 评审分层（gap ②）：Thorough = 历史 REVIEW_SYSTEM_PROMPT 字节一致；
    // Fast = 精简档（保留注入防线与三态 JSON 契约，关取证/经验）。
    // 带分镜走 with_lens（无分镜时 String 与原文同字节；借用进
    // DetachedOpts——无分镜路径经 with_lens 也返回 String，行为不变）。
    let system_prompt =
        nemesis_prompts::board::render_review_system_prompt_with_lens(tier, lens, lens_reference);
    let mut last_err = String::new();
    for _ in 0..=2 {
        let opts = match mode {
            ReviewToolMode::NoTools => nemesis_agent::r#loop::DetachedOpts {
                system_prompt: Some(system_prompt.as_str()),
                no_tools: true,
                max_turns: 1,
                label: Some("board-review"),
                ..Default::default()
            },
            ReviewToolMode::ReadOnly { max_turns } => nemesis_agent::r#loop::DetachedOpts {
                system_prompt: Some(system_prompt.as_str()),
                no_tools: false,
                allowed_tools: Some(READONLY_REVIEW_TOOLS),
                max_turns,
                label: Some("board-review"),
                ..Default::default()
            },
            // evidence-on-demand：远端 worker 评审的取证面 =
            // 档案围栏工具单件。工具未注册（store 缺席等装配残缺态）时白名
            // 单过滤为零供给——诚实退化为多轮纯文本，不会放大工具面。
            ReviewToolMode::EvidenceOnly { max_turns } => nemesis_agent::r#loop::DetachedOpts {
                system_prompt: Some(system_prompt.as_str()),
                no_tools: false,
                allowed_tools: Some(EVIDENCE_ONLY_TOOLS),
                max_turns,
                label: Some("board-review"),
                ..Default::default()
            },
        };
        let raw = match agent_loop.run_detached(prompt, opts).await {
            Ok(raw) => raw,
            Err(e) => return Err(format!("LLM 调用失败：{e}")),
        };
        if is_system_terminal_text(&raw) {
            return Err(format!(
                "评审输出为系统终态文本（非评审结论，不重试防污染）：{raw}"
            ));
        }
        match nemesis_board::parse_review(&raw) {
            Ok(out) => {
                // 缺口清账机械闸（改进 1）：清单非空时上一轮缺口必须逐条
                // 声明归宿、closed/obsolete 必须附证据、PASS 与未清账缺口
                // 互斥——违例与解析失败同通道回灌重试；重试 prompt 显式
                // 带上清单（错误消息只报编号不带条目原文，注入段已被重试
                // 文本替换，没有清单模型无法对着内容自纠）；3 轮耗尽 =
                // 评审自身失败 → 转人工。
                if let Err(e) = nemesis_board::check_gap_closure(&out, prior_gaps) {
                    *prompt =
                        nemesis_board::review::build_gap_closure_retry_prompt(&raw, &e, prior_gaps);
                    last_err = e.message;
                    continue;
                }
                return Ok(out);
            }
            Err(e) => {
                // 解析失败回灌同样带清单语境：清单非空时 gap_closure 是
                // JSON 契约的一部分，重试需要契约要求文本与条目原文才能
                // 一次性自纠（prior 为空时与旧 build_retry_prompt 字节一致）。
                *prompt =
                    nemesis_board::review::build_gap_closure_retry_prompt(&raw, &e, prior_gaps);
                last_err = e.message;
            }
        }
    }
    Err(format!("评审输出连续 3 轮无法解析：{last_err}"))
}

/// agent 引擎系统终态文本判定（run_loop.rs / recovery.rs / tool_batch.rs /
/// turn_guard.rs 的 Done 文本）：急停 / 取消 / 轮次预算暂停 / LLM hook 拦截 /
/// 退化输出放弃。评审链拿这类文本当模型输出是污染源（压测①），回灌重试
/// 只会放大——检测到直接诚实失败。特征串均为引擎独有措辞，评审 JSON 正常
/// 内容不可能逐字命中；「已取消」引擎产出恒为整串，用整串比对避免理由段
/// 偶含同词的误伤。
///
/// A起对交付写入路径开放（pub(crate)）：
/// `write_back_board_dispatch`（board_dispatch.rs）在落库终态判定前用本
/// 谓词守卫——系统暂停/取消文本不是 worker 汇报，不得当交付收货。
pub(crate) fn is_system_terminal_text(raw: &str) -> bool {
    if raw.trim() == "已取消" {
        return true;
    }
    const MARKERS: [&str; 4] = [
        "(E-STOP)",                             // 急停三形态（任务/LLM 调用/工具调用）
        "轮工具调用后暂停，已完成的工作已保存", // max_turns 暂停（预算/非预算两分支）
        "HOOK BLOCKED",                         // LLM hook 拦截终局
        "模型多次未给出有效答复",               // turn_guard 退化输出 GiveUp
    ];
    MARKERS.iter().any(|m| raw.contains(m))
}

/// 多检查员面板并发上限（/：N 路评审同时最多 4 路在飞，多余排队）。
pub(super) const PANEL_CONCURRENCY: usize = 4;
/// 检查员路数收敛上限（配置面同步 clamp；防手滑写 5000 路烧穿 LLM 配额）。
pub(crate) const MAX_REVIEW_CHECKERS: u32 = 5;
/// 手法3：检查员面板 retained input 字节上限（prompt 总账）。8 MiB
/// 刻意宽于任何现实评审 prompt，只有病态大包才被诚实拒绝。
pub(super) const PANEL_MAX_INPUT_BYTES: usize = 8 * 1024 * 1024;

/// 多检查员面板评审：`checkers > 1` 时并行发出 N 路独立 `run_review_llm`
/// （每路独立 prompt 副本 + 编号后缀去相关，WorkPool 限并发（permits =
/// [`PANEL_CONCURRENCY`]， spawn 形态），join 保序），`aggregate_verdicts`
/// 多数票聚合。`checkers = 1`（默认）直接单路调用，与历史行为字节等价。
/// 全路解析失败 → 返回末路错误（与单路 3 轮全败语义一致）。
///
/// （profession-capability-evolution）：单路 = 职能分镜
/// （`profession_lens`，按 issue.required_profession 选）；多路 = 异构
/// 三镜按 index 轮换（`checker_lens_for_index`，不再 N 个同质评委，「第
/// N 号检查员」编号去相关保留）——预取的禁令节引用只注入 constraint 镜
/// （[`nemesis_prompts::board:CONSTRAINT_LENS_ID`]）。
///
/// 手法3：双上限背压——pending 上限 =
/// [`MAX_REVIEW_CHECKERS`]（clamp 后全批可提交）、retained input 字节上限 =
/// [`PANEL_MAX_INPUT_BYTES`]。超限 = 类型化 overloaded **全批诚实拒绝**
/// （已提交任务取消释放，不部分执行——与 并行批同款批级原子性）。
pub(super) async fn run_review_panel(
    agent_loop: &Arc<nemesis_agent::r#loop::AgentLoop>,
    base_prompt: &str,
    mode: ReviewToolMode,
    checkers: u32,
    tier: nemesis_prompts::board::ReviewTier,
    profession_lens: Option<&'static nemesis_prompts::board::ReviewLens>,
    lens_reference: Option<&str>,
    // 缺口清账机械闸的清单来源（透传给每路检查员；空 = 首评）。
    prior_gaps: &[nemesis_board::LedgerItem],
) -> Result<nemesis_board::ReviewOutput, String> {
    let checkers = checkers.clamp(1, MAX_REVIEW_CHECKERS) as usize;
    if checkers == 1 {
        let mut prompt = base_prompt.to_string();
        return run_review_llm(
            agent_loop,
            &mut prompt,
            mode,
            tier,
            profession_lens,
            lens_reference,
            prior_gaps,
        )
        .await;
    }
    let pool = nemesis_utils::workpool::WorkPool::new(nemesis_utils::workpool::PoolLimits {
        permits: PANEL_CONCURRENCY,
        max_pending: MAX_REVIEW_CHECKERS as usize,
        max_input_bytes: PANEL_MAX_INPUT_BYTES,
    });
    let mut tasks = Vec::with_capacity(checkers);
    // 'static 闭包约束：清单按路克隆进任务（条目各 ≤512B，量级可忽略）。
    let prior_owned = prior_gaps.to_vec();
    for i in 0..checkers {
        let lens = nemesis_prompts::board::checker_lens_for_index(i);
        let reference = if lens.id == nemesis_prompts::board::CONSTRAINT_LENS_ID {
            lens_reference.map(str::to_string)
        } else {
            None
        };
        let prior = prior_owned.clone();
        let prompt = format!(
            "{base_prompt}\n\n（你是第 {} 号检查员：请独立评审，不受其他检查员意见影响）",
            i + 1
        );
        let input_bytes = prompt.len();
        let agent_loop = Arc::clone(agent_loop);
        let submitted = pool.try_submit(input_bytes, async move {
            let mut prompt = prompt;
            run_review_llm(
                &agent_loop,
                &mut prompt,
                mode,
                tier,
                Some(lens),
                reference.as_deref(),
                &prior,
            )
            .await
        });
        match submitted {
            Ok(t) => tasks.push(t),
            Err(err) => {
                // 超限 = 全批诚实拒绝（已提交任务 Drop=取消 → worker 下一
                // 悬挂点停止释放；不部分执行、不挂起排队）。
                drop(tasks);
                warn!("[BoardReview] Reviewer panel rejected by pool: {err}");
                return Err(format!("评审检查员面板过载，已拒绝执行（{err}）"));
            }
        }
    }
    let mut oks = Vec::new();
    let mut last_err = String::new();
    for task in tasks {
        match task.join().await {
            Ok(Ok(out)) => oks.push(out),
            Ok(Err(e)) => last_err = e,
            Err(_cancelled) => last_err = "检查员任务被取消".to_string(),
        }
    }
    if oks.is_empty() {
        return Err(last_err);
    }
    Ok(aggregate_verdicts(checkers, oks))
}

/// 多数票聚合（纯函数，穷举可测）：多数 PASS → Pass；多数 FAIL → Fail；
/// 其余（平票/全 Unsure）→ Unsure（转人工，fail-safe 不偏向自动收货）。
/// reasons 首条注入投票注记（含无效路注记），其余按路合并去重保序；gap
/// 取首个非空；criteria 取首个非空、unknowns 按序去重合并（c §③）；
/// experience 取首个非空；need_evidence/evidence_request 仅在
/// 聚合结论为 Unsure 时保留——多数已定案的结论不被个别证据请求改写
/// （挂起只服务"评不了"，不推翻"评得了"）。
/// c §③ unknowns 落盘（评审存疑事项 → issue 系统评论的
/// `unknown: <事项>` 结构化标记行；b ⑥ FINISH 谓词消费此标记，
/// 人工评论回应即裁决）。去重：历史轮已落的 `unknown: <同文>` 行不
/// 重复落，本轮有新增盲区才落一条聚合评论。失败只 warn 不炸评审
/// （评论是可观测面，非语义闸）。
pub(super) fn persist_unknowns(
    store: &nemesis_board::BoardStore,
    issue_id: i64,
    unknowns: &[String],
) {
    if unknowns.is_empty() {
        return;
    }
    let seen: Vec<String> = store
        .list_comments(issue_id)
        .unwrap_or_default()
        .into_iter()
        .map(|c| c.content)
        .collect();
    let fresh: Vec<&str> = unknowns
        .iter()
        .map(|u| u.trim())
        .filter(|u| {
            if u.is_empty() {
                return false;
            }
            let marker = format!("unknown: {u}");
            !seen.iter().any(|c| c.contains(&marker))
        })
        .collect();
    if fresh.is_empty() {
        return;
    }
    let mut body = String::from("❓ 评审存疑事项（unknown 清单，自动收口检查会核对裁决状态）：");
    for u in fresh {
        body.push_str(&format!("\nunknown: {u}"));
    }
    if let Err(e) = store.add_comment(nemesis_board::NewComment {
        issue_id,
        author: nemesis_board::Actor::system("board-review"),
        content: body,
        parent_id: None,
        ctype: CommentType::System,
    }) {
        warn!("[BoardReview] issue {issue_id} unknown 清单落盘失败（不阻塞评审）: {e}");
    }
}

pub(super) fn aggregate_verdicts(
    requested: usize,
    outputs: Vec<nemesis_board::ReviewOutput>,
) -> nemesis_board::ReviewOutput {
    let n = outputs.len();
    let pass = outputs
        .iter()
        .filter(|o| o.verdict == nemesis_board::ReviewVerdict::Pass)
        .count();
    let fail = outputs
        .iter()
        .filter(|o| o.verdict == nemesis_board::ReviewVerdict::Fail)
        .count();
    let unsure = n.saturating_sub(pass + fail);
    let verdict = if pass * 2 > n {
        nemesis_board::ReviewVerdict::Pass
    } else if fail * 2 > n {
        nemesis_board::ReviewVerdict::Fail
    } else {
        nemesis_board::ReviewVerdict::Unsure
    };

    let mut reasons = vec![format!(
        "{} 位检查员投票：PASS×{pass} FAIL×{fail} UNSURE×{unsure}（多数票裁决）",
        requested
    )];
    if n < requested {
        reasons.push(format!("另有 {} 位检查员输出无效未计票", requested - n));
    }
    for o in &outputs {
        for r in &o.reasons {
            let r = r.trim();
            if !r.is_empty() && !reasons.iter().any(|x| x.trim() == r) {
                reasons.push(r.to_string());
            }
        }
    }

    let gap = outputs
        .iter()
        .map(|o| o.gap.trim())
        .find(|g| !g.is_empty())
        .unwrap_or("")
        .to_string();
    let experience = outputs.iter().find_map(|o| o.experience.clone());
    // c §③ 聚合语义：criteria 取首个非空（条目文本难以逐字对齐，
    // 按条目多数决脆弱——取最完整的一份比逐条投票稳健）；unknowns 按序
    // 去重合并（保守收集全部检查员的盲区）。聚合 verdict==Pass 但 criteria
    // 带 FAIL 条（少数意）不在此强制改判——渲染层照实列条目（parse_review
    // 的互斥校验只管单路 LLM 输出，聚合是系统合成）。
    let criteria = outputs
        .iter()
        .find(|o| !o.criteria.is_empty())
        .map(|o| o.criteria.clone())
        .unwrap_or_default();
    // 缺口清账聚合：同 criteria 策略取首个非空（对账是逐编号声明，按条
    // 多数决脆弱；多数票下取最完整一份与条目清单同源一致）。
    let gap_closure = outputs
        .iter()
        .find(|o| !o.gap_closure.is_empty())
        .map(|o| o.gap_closure.clone())
        .unwrap_or_default();
    let mut unknowns = Vec::new();
    for o in &outputs {
        for u in &o.unknowns {
            let u = u.trim();
            if !u.is_empty() && !unknowns.iter().any(|x: &String| x.trim() == u) {
                unknowns.push(u.to_string());
            }
        }
    }
    let (need_evidence, evidence_request) = if verdict == nemesis_board::ReviewVerdict::Unsure {
        let need = outputs.iter().any(|o| o.need_evidence == Some(true));
        let req = outputs
            .iter()
            .find_map(|o| o.evidence_request.clone().filter(|s| !s.trim().is_empty()));
        (need.then_some(true), req)
    } else {
        (None, None)
    };

    nemesis_board::ReviewOutput {
        verdict,
        reasons,
        gap,
        experience,
        need_evidence,
        evidence_request,
        criteria,
        unknowns,
        gap_closure,
    }
}

/// 项目收口验收标准聚合（纯函数）：项目级 AC 排首段（【项目级】标注），
/// 各顶层父单非空 AC 依序拼接（【父单号】标注），段间 `---` 分隔。空白段
/// 跳过（无 AC 的项目/父单不产生空段）。
/// 【label】必须独立成行（根修）：旧实现拼进 AC 首行
/// （`【项目级】[CHECK] …`），首行不再以 `[CHECK]` 顶格 → 每段第一条
/// 锚点被 parse_anchors 静默吞掉回落语义项（双端真机 实证：项目级
/// AC 的 re:index.html 锚点从未参与客观核验；单行 AC 项目整个锚点面
/// 失效）。
pub(super) fn join_project_review_ac(
    project_ac: Option<&str>,
    parents: &[nemesis_board::Issue],
) -> String {
    let mut joined = String::new();
    let mut push = |label: String, ac: &str| {
        let ac = ac.trim();
        if ac.is_empty() {
            return;
        }
        if !joined.is_empty() {
            joined.push_str("\n---\n");
        }
        joined.push_str(&format!("【{label}】\n{ac}\n"));
    };
    if let Some(ac) = project_ac {
        push("项目级".to_string(), ac);
    }
    for p in parents {
        if let Some(ac) = p.acceptance_criteria.as_deref() {
            push(p.number.clone(), ac);
        }
    }
    joined
}

/// 从 home 读 board 段旗标（auto_review/auto_accept/max_redispatch）。
/// 配置读失败 → Err（评审诚实放弃，fail-closed 到人工验收——不拿默认值
/// 顶替用户配置）。 冲突漏斗（conflict_auto_resolve）同源消费。
pub(crate) fn load_board_flags(
    home: &std::path::Path,
) -> Result<nemesis_config::BoardFlagConfig, String> {
    let path = home.join("config.json");
    let cfg = nemesis_config::load_config(&path)
        .map_err(|e| format!("config.json 读取失败（{}）：{e}", path.display()))?;
    Ok(cfg.board.unwrap_or_default())
}

/// reasons 列表渲染成评论小节；空列表给诚实注记。
pub(super) fn render_reasons(reasons: &[String]) -> String {
    if reasons.is_empty() {
        return "（评审 agent 未给出理由）".to_string();
    }
    let mut s = String::from("理由：\n");
    for r in reasons {
        s.push_str(&format!("- {}\n", r.trim()));
    }
    s
}

/// gap 与 reasons 的内容重叠守卫（根修，横扫三处转人工/收口
/// 评论拼装）：锚点短路 FAIL 路径上 `output.gap` 与 `output.reasons` 同源
/// 同构（reasons 每条 = "锚点失败: …"，gap = render_anchor_failures 明细
/// 渲染），两处都拼进评论就是同一条失败锚点重复出现两遍（双端真机
/// 实证：项目收口转人工评论每条失败锚点列出两次）。gap 非空只拼 gap；
/// gap 空（LLM FAIL 未填差距原文的诚实降级）reasons 仍是唯一理由来源，
/// 照常渲染。
pub(super) fn append_reasons_unless_gapped(
    comment: &mut String,
    output: &nemesis_board::ReviewOutput,
) {
    if output.gap.trim().is_empty() {
        comment.push_str(&render_reasons(&output.reasons));
    }
}
