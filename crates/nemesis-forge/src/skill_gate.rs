//! skill 装前 tests 闸（自学习铺路批次四）。
//!
//! 对话产出的 skill 草稿 → A 线 tests 实跑（[`SkillTestRunner`]）→
//! refine ≤ [`REFINE_MAX_ROUNDS`] → 过闸落
//! `<workspace>/skills/{slug}-forge/`。「LLM 当脑子、Rust 当路」分工：
//! 本模块是 Rust 路面——实跑、判分、落盘、记录全部确定性；refine 是
//! 唯一 LLM 参与点（喂失败明细要修正稿，system prompt =
//! [`nemesis_prompts::forge::SKILL_FIXER_SYSTEM_PROMPT`]）。
//!
//! 纪律（fail-closed）：
//! - 执行器缺席 = 全拒（草稿只在 outcome 留痕，不落 workspace）；
//! - 无 tests 段 / 段非法 = 拒（装前闸要求 tests；预探测不信任，闸内
//!   以 [`parse_skill_tests`] 现解析为准——单一真相源）；
//! - refine ≤N 轮仍败 = 弃稿不落盘（诚实记 FailedAfterRefine）；
//! - estop 在 refine 轮间检查，中止即弃稿。

use std::path::Path;
use std::sync::Arc;

use nemesis_utils::sanitize_path_segment;

use crate::learn_dialogue::LearnedSkillDraft;
use crate::reflector_llm::LLMCaller;
use crate::skill_tests::{
    ExecutionTranscript, SkillTestRunner, evaluate_case_full, parse_skill_tests,
};

/// refine 最大轮数（schema 常量，刻意不 config 化——同 skill 装闸门槛
/// 跨实例一致性先例）。每轮 = 一次修正调用 + 一轮全量重跑。
pub const REFINE_MAX_ROUNDS: u32 = 2;

/// 单稿闸终局。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "verdict", content = "detail")]
pub enum GateVerdict {
    /// 过闸已落 `<workspace>/skills/{slug}-forge/SKILL.md`。
    Installed,
    /// 未进实跑即拒（无 tests / 非法 / 执行器缺席 / slug 空 / estop）。
    Rejected(String),
    /// 实跑过但 refine ≤N 轮仍败——弃稿不落盘。
    FailedAfterRefine,
}

/// 逐稿闸记录（落 `outcome.json` 的 `gate` 数组）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DraftGateRecord {
    /// 落盘目录名（消毒形态，含 `-forge` 后缀）；拒绝场景为消毒形态或
    /// 原始 slug（slug 为空时原样）。
    pub slug: String,
    pub verdict: GateVerdict,
    /// 实际消耗的 refine 轮数（0 = 首跑即过或未进 refine）。
    pub refine_rounds: u32,
    /// 最后一次失败明细（封顶 10 条，供回查）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub last_failures: Vec<String>,
}

/// 失败明细封顶（记录面，防垃圾稿把 outcome.json 撑爆）。
const MAX_RECORDED_FAILURES: usize = 10;

/// 对一批草稿跑装前闸，回传逐稿记录（顺序与 `drafts` 一致）。
///
/// `runner` 缺席 = fail-closed 全拒；`caller` 缺席时带 judge 断言的用例
/// fail-closed 失败（[`evaluate_case_full`] 语义），且 refine 不可用
/// （首跑失败即 FailedAfterRefine）。
#[allow(clippy::too_many_arguments)]
pub async fn gate_skill_drafts(
    runner: Option<&Arc<dyn SkillTestRunner>>,
    caller: Option<&Arc<dyn LLMCaller>>,
    workspace: &Path,
    drafts: &[LearnedSkillDraft],
    is_stopped: &(dyn Fn() -> bool + Send + Sync),
    budget_tokens: u32,
    judge_threshold: f64,
) -> Vec<DraftGateRecord> {
    let Some(runner) = runner else {
        return drafts
            .iter()
            .map(|d| DraftGateRecord {
                slug: dir_name_for(&d.slug),
                verdict: GateVerdict::Rejected(
                    "A 线执行器未装配（set_skill_test_runner），草稿不落盘".into(),
                ),
                refine_rounds: 0,
                last_failures: Vec::new(),
            })
            .collect();
    };
    let mut records = Vec::with_capacity(drafts.len());
    for draft in drafts {
        let record = gate_one(
            runner.as_ref(),
            caller,
            workspace,
            draft,
            is_stopped,
            budget_tokens,
            judge_threshold,
        )
        .await;
        records.push(record);
    }
    records
}

/// 目录名：`{消毒后 slug}-forge`（slug 空 = 原样返回，上游按空拒）。
fn dir_name_for(slug: &str) -> String {
    let s = slug.trim();
    if s.is_empty() {
        return String::new();
    }
    format!("{}-forge", sanitize_path_segment(s))
}

/// 单稿闸：解析 → 首跑 →（失败时）refine 循环 → 落盘或弃稿。
#[allow(clippy::too_many_arguments)]
async fn gate_one(
    runner: &dyn SkillTestRunner,
    caller: Option<&Arc<dyn LLMCaller>>,
    workspace: &Path,
    draft: &LearnedSkillDraft,
    is_stopped: &(dyn Fn() -> bool + Send + Sync),
    budget_tokens: u32,
    judge_threshold: f64,
) -> DraftGateRecord {
    let dir_name = dir_name_for(&draft.slug);
    let mut record = DraftGateRecord {
        slug: if dir_name.is_empty() {
            draft.slug.clone()
        } else {
            dir_name
        },
        verdict: GateVerdict::Rejected(String::new()),
        refine_rounds: 0,
        last_failures: Vec::new(),
    };

    // ① slug / 内容非空（引擎侧已挡一轮，闸内再守一次——边界各自诚实）。
    if draft.slug.trim().is_empty() || draft.skill_md.trim().is_empty() {
        record.verdict = GateVerdict::Rejected("slug 或 SKILL.md 为空".into());
        return record;
    }

    // ② tests 段权威解析（单一真相源；预探测不信任）。
    let spec = match parse_skill_tests(&draft.skill_md) {
        Ok(Some(spec)) => spec,
        Ok(None) => {
            record.verdict = GateVerdict::Rejected("无 tests 段（装前闸要求行为用例）".into());
            return record;
        }
        Err(e) => {
            record.verdict = GateVerdict::Rejected(format!("tests 段非法：{e}"));
            return record;
        }
    };

    // ③ 首跑 + refine 循环。`current` 是本轮被测内容（refine 后替换）。
    let mut current = draft.skill_md.clone();
    loop {
        let failures = run_all_cases(runner, caller, &current, &spec, judge_threshold).await;
        if failures.is_empty() {
            match install(workspace, &record.slug, &current) {
                Ok(()) => {
                    record.verdict = GateVerdict::Installed;
                    break;
                }
                Err(e) => {
                    record.verdict = GateVerdict::Rejected(format!("落盘失败：{e}"));
                    return record;
                }
            }
        }
        record.last_failures = cap_failures(&failures);
        if record.refine_rounds >= REFINE_MAX_ROUNDS {
            record.verdict = GateVerdict::FailedAfterRefine;
            return record;
        }
        if is_stopped() {
            record.verdict = GateVerdict::Rejected("急停中：refine 中止，草稿不落盘".into());
            return record;
        }
        let Some(caller) = caller else {
            record.verdict = GateVerdict::FailedAfterRefine;
            return record;
        };
        record.refine_rounds += 1;
        match refine_once(caller, &current, &failures, budget_tokens).await {
            Ok(fixed) => current = fixed,
            Err(e) => {
                record.verdict = GateVerdict::FailedAfterRefine;
                record
                    .last_failures
                    .insert(0, format!("refine 调用失败：{e}"));
                return record;
            }
        }
    }
    record
}

/// 过闸落盘：`<workspace>/skills/{slug}/SKILL.md` 原子写（同名 skill =
/// 对话再学习覆盖更新）。写失败 = 拒（fail-closed，不假装装好）。
fn install(workspace: &Path, dir_name: &str, skill_md: &str) -> std::io::Result<()> {
    nemesis_utils::write_file_atomic(
        &workspace
            .join("skills")
            .join(dir_name)
            .join("SKILL.md")
            .to_string_lossy(),
        skill_md.as_bytes(),
        0o600,
    )
    .map_err(std::io::Error::other)
}

/// 全量用例实跑：逐条 run_case + 纯断言求值 + judge 层（caller 在场时）。
/// 回传全部失败明细（空 = 全过）。
async fn run_all_cases(
    runner: &dyn SkillTestRunner,
    caller: Option<&Arc<dyn LLMCaller>>,
    skill_md: &str,
    spec: &crate::skill_tests::SkillTestsSpec,
    judge_threshold: f64,
) -> Vec<String> {
    let mut failures = Vec::new();
    for case in &spec.tests {
        let result = match runner.run_case(skill_md, case).await {
            Ok(transcript) => {
                evaluate_case_full(case, &transcript, caller.cloned(), judge_threshold).await
            }
            Err(e) => {
                // 执行器拒绝执行（装配失败等）：构造失败结果，断言无从谈起。
                crate::skill_tests::CaseResult {
                    name: case.name.clone(),
                    passed: false,
                    failures: vec![format!("执行器错误：{e}")],
                    transcript: ExecutionTranscript::default(),
                    judge: None,
                }
            }
        };
        if !result.passed {
            failures.extend(result.failures);
        }
    }
    failures
}

/// 单轮 refine：喂当前稿 + 失败明细，取回修正后的完整 SKILL.md。
/// 回复剥 markdown 代码围栏（``` 包裹形态）；空回复 = Err。
async fn refine_once(
    caller: &Arc<dyn LLMCaller>,
    current: &str,
    failures: &[String],
    budget_tokens: u32,
) -> Result<String, String> {
    let mut user = String::from(
        "以下自学习对话产出的 skill 草稿未通过行为回归测试。\
         请修复 skill 内容（保持 frontmatter 与 tests 段契约不变），\
         只回复修正后的完整 SKILL.md，不要输出任何其他文本。\n\n\
         ## 当前 SKILL.md\n",
    );
    user.push_str(current.trim());
    user.push_str("\n\n## 失败明细\n");
    for f in failures.iter().take(MAX_RECORDED_FAILURES) {
        user.push_str(&format!("- {f}\n"));
    }
    user.push_str(
        "\n## 格式约束\n- frontmatter tests：turns ≤ 6；fixtures ≤ 8 且单文件 ≤ 64KB；\
         每条用例至少一个断言。\n",
    );
    let raw = caller
        .chat(
            nemesis_prompts::forge::SKILL_FIXER_SYSTEM_PROMPT,
            &user,
            Some(budget_tokens as i64),
        )
        .await?;
    let stripped = strip_code_fence(&raw);
    if stripped.trim().is_empty() {
        return Err("refine 回复为空".into());
    }
    Ok(stripped)
}

/// 剥一层 markdown 代码围栏（```/```lang 包裹形态）；无围栏 = 原样返回。
/// 实现单一真相源 = [`nemesis_utils::sanitize::strip_outer_code_fence`]
/// （BUG-20261128-022：旧实现按「首个独立 ``` 行」截断——修正稿正文含
/// fenced 代码块时内层收尾被当外壳，其后正文静默丢失且截断稿照常过闸
/// 落盘；SKILL.md 合法形态首行恒为 frontmatter `---`，包裹歧义角落对
/// 本消费点不可达）。
fn strip_code_fence(raw: &str) -> String {
    nemesis_utils::sanitize::strip_outer_code_fence(raw)
}

/// 失败明细封顶记录。
fn cap_failures(failures: &[String]) -> Vec<String> {
    failures
        .iter()
        .take(MAX_RECORDED_FAILURES)
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests;
