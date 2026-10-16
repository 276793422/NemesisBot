//! Skill 行为回归测试（自进化升级 A 线，2026-10-14）。
//!
//! SKILL.md frontmatter 扩展 `tests` 段（YAML）——行为用例随 skill 分发，
//! registry 安装时自动带入回归网。四层确定性断言（工具/文本/效果/预算）
//! + 可选 judge 层（LLM 评审 rubric 评分）。
//!
//! 职责分界：
//! - 本模块 = **纯 schema + 纯断言求值**（不执行、不 IO）；
//! - [`SkillTestRunner`] trait = 执行抽象（跑 turns、写 fixtures、收集
//!   transcript），由集成层（nemesisbot）实现注入——与 [`crate::reflector_llm::LLMCaller`]
//!   同款依赖倒置，forge 不反依赖执行环境。
//!
//! 格式约束（定稿数值，见 docs/PLAN/2026-10-14_自进化升级计划.md §四）：
//! turns ≤ 6；fixtures ≤ 8 个且单文件 ≤ 64KB；每条用例至少一个断言；
//! 违反 = 解析错误（该 skill 回归不通过，诚实报错）。

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use std::path::Path;

/// 单文件 fixture 大小上限（64KB）。
pub const FIXTURE_MAX_BYTES: usize = 65536;
/// fixtures 数量上限。
pub const FIXTURE_MAX_COUNT: usize = 8;
/// turns 脚本轮数上限。
pub const TURNS_MAX: usize = 6;

/// 多轮脚本中的一轮用户消息。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TestTurn {
    pub user: String,
}

/// 预置工作区文件（沙盒副本内，执行前写入）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TestFixture {
    pub path: String,
    pub content: String,
}

/// 确定性断言集（全部满足才过；至少声明一个断言）。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TestExpectations {
    /// 期望调用过的工具（子集语义：transcript 工具集须包含全部列名）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools_called: Vec<String>,
    /// 禁止调用的工具。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools_forbidden: Vec<String>,
    /// 回复包含（多轮 = 全部 final 文本拼接后判）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub contains: Vec<String>,
    /// 回复禁止包含。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub not_contains: Vec<String>,
    /// 回复匹配正则（拼接文本）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub matches_regex: Vec<String>,
    /// 执行后应存在的文件（相对 workspace 根）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files_exist: Vec<String>,
    /// LLM 轮数上限。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_llm_turns: Option<u32>,
    /// 墙钟上限（秒）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_duration_secs: Option<u64>,
}

impl TestExpectations {
    fn assertion_count(&self) -> usize {
        let mut n = 0;
        if !self.tools_called.is_empty() {
            n += 1;
        }
        if !self.tools_forbidden.is_empty() {
            n += 1;
        }
        if !self.contains.is_empty() {
            n += 1;
        }
        if !self.not_contains.is_empty() {
            n += 1;
        }
        if !self.matches_regex.is_empty() {
            n += 1;
        }
        if !self.files_exist.is_empty() {
            n += 1;
        }
        if self.max_llm_turns.is_some() {
            n += 1;
        }
        if self.max_duration_secs.is_some() {
            n += 1;
        }
        n
    }
}

/// LLM 评审断言（模糊行为；可选）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TestJudge {
    /// 评审 rubric（自然语言判据）。
    pub rubric: String,
    /// 评审分阈值（0.0-1.0）；None = 执行层配置默认
    /// （forge.test.judge_threshold，缺省 0.7）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f64>,
}

/// 单条行为用例。`deny_unknown_fields` 钉拼写（`jduge:` 这类 typo 静默
/// 丢弃 judge 层 = 断言悄悄少跑一层，与 TestExpectations 同纪律）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TestCase {
    pub name: String,
    /// 多轮脚本（缺省单轮：单条 user 消息 = 整个任务文本）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub turns: Vec<TestTurn>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixtures: Vec<TestFixture>,
    #[serde(default)]
    pub expect: TestExpectations,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judge: Option<TestJudge>,
}

/// 一个 skill 的全部行为用例（frontmatter `tests` 段）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SkillTestsSpec {
    pub tests: Vec<TestCase>,
}

/// 执行层回传的运行记录（runner 填充，纯求值消费）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecutionTranscript {
    /// 实际执行的轮数。
    pub turns_executed: u32,
    /// 墙钟（秒）。
    pub duration_secs: u64,
    /// 调用过的工具名（去重，按首现序）。
    pub tools_called: Vec<String>,
    /// 每轮 assistant 最终文本。
    pub finals: Vec<String>,
    /// 执行期错误（非断言失败；出现即本用例失败）。
    #[serde(default)]
    pub errors: Vec<String>,
    /// 执行后 workspace 内存在的文件（相对路径，供 files_exist 判定）。
    #[serde(default)]
    pub files: Vec<String>,
}

impl ExecutionTranscript {
    /// 全部 assistant 文本拼接（finals + errors，供文本断言）。
    pub fn joined_text(&self) -> String {
        let mut s = self.finals.join("\n");
        s.push('\n');
        s.push_str(&self.errors.join("\n"));
        s
    }
}

/// judge 层结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JudgeOutcome {
    pub score: f64,
    pub threshold: f64,
    pub passed: bool,
    /// 评审摘要（模型返回的说明，截断存储）。
    #[serde(default)]
    pub summary: String,
}

/// 单条用例结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseResult {
    pub name: String,
    pub passed: bool,
    /// 失败明细（断言失败 / 执行错误 / judge 未过）。
    #[serde(default)]
    pub failures: Vec<String>,
    /// 确定性断言层的执行统计。
    pub transcript: ExecutionTranscript,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judge: Option<JudgeOutcome>,
}

/// 行为测试执行器抽象（集成层实现；forge 侧只在 pipeline 调用）。
#[async_trait::async_trait]
pub trait SkillTestRunner: Send + Sync {
    /// 在隔离沙盒 workspace 中执行一条用例，回传运行记录。
    ///
    /// `skill_content` = 完整 SKILL.md（含 frontmatter）；执行器负责把它
    /// 写入沙盒 workspace 的 skills 目录并保证 skill 被装载。
    async fn run_case(
        &self,
        skill_content: &str,
        case: &TestCase,
    ) -> Result<ExecutionTranscript, String>;
}

/// 从 SKILL.md 全文解析 `tests` 段。
///
/// 返回：`Ok(None)` = 无 tests 段（存量 skill，诚实跳过）；`Ok(Some)` =
/// 解析并校验通过；`Err` = 段存在但格式非法（该 skill 回归不通过）。
pub fn parse_skill_tests(content: &str) -> Result<Option<SkillTestsSpec>, String> {
    let fm = extract_frontmatter(content);
    if fm.is_empty() {
        return Ok(None);
    }
    let yaml: JsonValue =
        serde_yaml::from_str(&fm).map_err(|e| format!("frontmatter YAML parse failed: {e}"))?;
    let Some(tests_val) = yaml.get("tests") else {
        return Ok(None);
    };
    if tests_val.is_null() {
        return Ok(None);
    }
    // tests_val 就是数组本身——直接反序列化 Vec<TestCase>（不要套
    // SkillTestsSpec 外壳：外壳期望 {tests:[...]}，喂数组必报 map/sequence）。
    let tests: Vec<TestCase> = serde_json::from_value(tests_val.clone())
        .map_err(|e| format!("tests section malformed: {e}"))?;
    let spec = SkillTestsSpec { tests };
    validate_spec(&spec)?;
    Ok(Some(spec))
}

/// frontmatter 提取（`---` 围栏；无围栏 = 空）。
fn extract_frontmatter(content: &str) -> String {
    let mut lines = content.lines();
    if lines.next().map(str::trim) != Some("---") {
        return String::new();
    }
    let mut out = Vec::new();
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        out.push(line);
    }
    out.join("\n")
}

/// 格式约束校验（定稿数值）。
fn validate_spec(spec: &SkillTestsSpec) -> Result<(), String> {
    if spec.tests.is_empty() {
        return Err("tests section is empty".into());
    }
    for case in &spec.tests {
        if case.name.trim().is_empty() {
            return Err("test case has empty name".into());
        }
        if case.turns.len() > TURNS_MAX {
            return Err(format!(
                "test '{}': turns {} > max {TURNS_MAX}",
                case.name,
                case.turns.len()
            ));
        }
        if case.fixtures.len() > FIXTURE_MAX_COUNT {
            return Err(format!(
                "test '{}': fixtures {} > max {FIXTURE_MAX_COUNT}",
                case.name,
                case.fixtures.len()
            ));
        }
        for fx in &case.fixtures {
            // 路径闸（R18 收紧）：拒绝 `..`、绝对路径与 Windows 盘符形态。
            // 此前只挡 `..`——`C:evil` 这类盘符相对路径 is_absolute()==false
            // 且无 `..`，但 Path::join 会整段换基座（写出到盘符当前目录），
            // 沙盒夹具越出临时工作区。合法夹具一律是沙盒内的相对段。
            let p = Path::new(fx.path.trim());
            if fx.path.trim().is_empty()
                || fx.path.contains("..")
                || p.is_absolute()
                || fx.path.contains(':')
                || fx.path.starts_with('\\')
                || fx.path.starts_with('/')
            {
                return Err(format!(
                    "test '{}': invalid fixture path {:?}",
                    case.name, fx.path
                ));
            }
            if fx.content.len() > FIXTURE_MAX_BYTES {
                return Err(format!(
                    "test '{}': fixture {} exceeds {FIXTURE_MAX_BYTES} bytes",
                    case.name, fx.path
                ));
            }
        }
        let has_judge = case.judge.is_some();
        if case.expect.assertion_count() == 0 && !has_judge {
            return Err(format!(
                "test '{}': no assertions declared (expect empty and no judge)",
                case.name
            ));
        }
        if let Some(j) = &case.judge {
            if j.rubric.trim().is_empty() {
                return Err(format!("test '{}': judge rubric is empty", case.name));
            }
            if let Some(t) = j.threshold
                && !(0.0..=1.0).contains(&t)
            {
                return Err(format!(
                    "test '{}': judge threshold {} outside [0,1]",
                    case.name, t
                ));
            }
        }
    }
    Ok(())
}

/// 纯断言求值：transcript vs 用例期望。不执行 judge 层（调用方负责）。
pub fn evaluate_case(case: &TestCase, transcript: &ExecutionTranscript) -> CaseResult {
    let mut failures = Vec::new();

    // 执行期错误：出现即失败（断言无从谈起）。
    for e in &transcript.errors {
        failures.push(format!("execution error: {e}"));
    }

    let joined = transcript.joined_text();

    // 工具面。
    for t in &case.expect.tools_called {
        if !transcript.tools_called.iter().any(|c| c == t) {
            failures.push(format!("tools_called: expected tool '{t}' was not called"));
        }
    }
    for t in &case.expect.tools_forbidden {
        if transcript.tools_called.iter().any(|c| c == t) {
            failures.push(format!("tools_forbidden: forbidden tool '{t}' was called"));
        }
    }

    // 文本面。
    for s in &case.expect.contains {
        if !joined.contains(s.as_str()) {
            failures.push(format!("contains: expected {s:?} in response"));
        }
    }
    for s in &case.expect.not_contains {
        if joined.contains(s.as_str()) {
            failures.push(format!("not_contains: forbidden {s:?} found in response"));
        }
    }
    for pat in &case.expect.matches_regex {
        match regex::Regex::new(pat) {
            Ok(re) => {
                if !re.is_match(&joined) {
                    failures.push(format!("matches_regex: pattern {pat:?} did not match"));
                }
            }
            Err(e) => failures.push(format!("matches_regex: invalid pattern {pat:?}: {e}")),
        }
    }

    // 效果面。
    for f in &case.expect.files_exist {
        if !transcript.files.iter().any(|p| p == f) {
            failures.push(format!(
                "files_exist: expected file {f:?} not present (files: {:?})",
                transcript.files
            ));
        }
    }

    // 预算面。
    if let Some(max_turns) = case.expect.max_llm_turns
        && transcript.turns_executed > max_turns
    {
        failures.push(format!(
            "max_llm_turns: {} exceeded budget {max_turns}",
            transcript.turns_executed
        ));
    }
    if let Some(max_secs) = case.expect.max_duration_secs
        && transcript.duration_secs > max_secs
    {
        failures.push(format!(
            "max_duration_secs: {}s exceeded budget {max_secs}s",
            transcript.duration_secs
        ));
    }

    CaseResult {
        name: case.name.clone(),
        passed: failures.is_empty(),
        failures,
        transcript: transcript.clone(),
        judge: None,
    }
}

/// 完整求值 = 纯断言 + judge 层（声明了 judge 就必须可评：无 caller =
/// fail-closed 失败，不允许静默跳过声明的断言）。
///
/// `default_threshold` = 用例未显式声明 threshold 时的执行层配置默认
/// （forge.test.judge_threshold）。
pub async fn evaluate_case_full(
    case: &TestCase,
    transcript: &ExecutionTranscript,
    caller: Option<std::sync::Arc<dyn crate::reflector_llm::LLMCaller>>,
    default_threshold: f64,
) -> CaseResult {
    let mut result = evaluate_case(case, transcript);
    let Some(judge) = &case.judge else {
        return result;
    };
    let threshold = judge.threshold.unwrap_or(default_threshold);
    match caller {
        Some(caller) => {
            let prompt = judge_prompt(case, &result.transcript);
            match caller
                .chat(
                    "你是严格的行为评审。只输出 JSON。",
                    &prompt,
                    Some(crate::reflector_llm::AUX_FORGE_MAX_TOKENS),
                )
                .await
            {
                Ok(resp) => match parse_judge_response(&resp, threshold) {
                    Ok(outcome) => {
                        if !outcome.passed {
                            result.failures.push(format!(
                                "judge: score {} below threshold {} ({})",
                                outcome.score, outcome.threshold, outcome.summary
                            ));
                        }
                        result.judge = Some(outcome);
                    }
                    Err(e) => result.failures.push(format!("judge parse failed: {e}")),
                },
                Err(e) => result.failures.push(format!("judge LLM call failed: {e}")),
            }
        }
        None => result
            .failures
            .push("judge declared but no LLM caller configured (fail-closed)".into()),
    }
    result.passed = result.failures.is_empty();
    result
}

/// judge 评审 prompt（单一真相源；调用方拼 transcript 后调 LLMCaller）。
pub fn judge_prompt(case: &TestCase, transcript: &ExecutionTranscript) -> String {
    let judge = case
        .judge
        .as_ref()
        .expect("judge_prompt called without judge spec");
    format!(
        "你是对 agent 行为的严格评审。根据以下 rubric 对执行结果评分（0.0-1.0）。\n\
         \n rubric:\n{}\n\
         \n 执行记录:\n- 轮数: {}\n- 工具: {:?}\n- 回复:\n{}\n\
         \n 只输出 JSON: {{\"score\": <0.0-1.0>, \"summary\": \"<一句话评语>\"}}",
        judge.rubric,
        transcript.turns_executed,
        transcript.tools_called,
        transcript
            .joined_text()
            .chars()
            .take(4000)
            .collect::<String>()
    )
}

/// 解析 judge 回复（`{"score": x, "summary": "..."}`）。
pub fn parse_judge_response(response: &str, threshold: f64) -> Result<JudgeOutcome, String> {
    let parsed: JsonValue =
        crate::reflector_llm::extract_json(response).ok_or("judge response is not valid JSON")?;
    let score = parsed
        .get("score")
        .and_then(|v| v.as_f64())
        .ok_or("judge response missing numeric 'score'")?;
    if !(0.0..=1.0).contains(&score) {
        return Err(format!("judge score {score} outside [0,1]"));
    }
    let summary = parsed
        .get("summary")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .chars()
        .take(500)
        .collect();
    Ok(JudgeOutcome {
        score,
        threshold,
        passed: score >= threshold,
        summary,
    })
}

#[cfg(test)]
mod tests;
