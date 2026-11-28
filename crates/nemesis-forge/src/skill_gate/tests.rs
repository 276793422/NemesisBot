//! 装前 tests 闸行为测试：过闸落盘 / 无 tests 拒 / 非法拒 / refine 修复 /
//! refine 耗尽弃稿 / 执行器缺席 fail-closed / estop 中止 / slug 消毒。

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use super::*;
use crate::reflector_llm::LLMCaller;
use crate::skill_tests::{ExecutionTranscript, SkillTestRunner, TestCase};

/// 内容键控 runner：skill_md 含 `FIXED` = 全过；否则 contains 断言失败。
/// 记录每次被测内容供断言。
struct ContentKeyedRunner {
    seen: StdMutex<Vec<String>>,
    /// true = 修好也仍失败（refine 耗尽路径）。
    always_fail: bool,
}

impl ContentKeyedRunner {
    fn passing() -> Self {
        Self {
            seen: StdMutex::new(Vec::new()),
            always_fail: false,
        }
    }
    fn failing() -> Self {
        Self {
            seen: StdMutex::new(Vec::new()),
            always_fail: true,
        }
    }
    fn seen(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl SkillTestRunner for ContentKeyedRunner {
    async fn run_case(
        &self,
        skill_content: &str,
        _case: &TestCase,
    ) -> Result<ExecutionTranscript, String> {
        self.seen.lock().unwrap().push(skill_content.to_string());
        let pass = !self.always_fail && skill_content.contains("FIXED");
        Ok(ExecutionTranscript {
            turns_executed: 1,
            duration_secs: 0,
            tools_called: Vec::new(),
            finals: vec![if pass { "ok".into() } else { "nope".into() }],
            errors: Vec::new(),
            files: Vec::new(),
        })
    }
}

/// 脚本 caller：按序弹出 refine 回复。
struct ScriptedFixer {
    steps: StdMutex<VecDeque<Result<String, String>>>,
    calls: std::sync::atomic::AtomicUsize,
}

impl ScriptedFixer {
    fn new(steps: Vec<Result<String, String>>) -> Self {
        Self {
            steps: StdMutex::new(steps.into()),
            calls: std::sync::atomic::AtomicUsize::new(0),
        }
    }
    fn calls(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl LLMCaller for ScriptedFixer {
    async fn chat(
        &self,
        _system_prompt: &str,
        _user_prompt: &str,
        _max_tokens: Option<i64>,
    ) -> Result<String, String> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut q = self.steps.lock().unwrap();
        q.pop_front()
            .unwrap_or_else(|| Ok("{\"proposals\":[],\"empty_reason\":\"脚本耗尽\"}".into()))
    }
}

/// 合法 SKILL.md（含可解析 tests 段 + contains 断言）。
fn valid_md(marker: &str) -> String {
    format!(
        "---\nname: gate-demo\ndescription: demo\nassert_marker: {marker}\ntests:\n  - name: smoke\n    turns:\n      - user: hi\n    expect:\n      contains: [\"ok\"]\n---\nbody {marker}\n"
    )
}

fn draft(slug: &str, md: String) -> LearnedSkillDraft {
    LearnedSkillDraft {
        slug: slug.into(),
        skill_md: md,
        has_tests: true,
    }
}

fn stopped_fn(v: bool) -> impl Fn() -> bool {
    move || v
}

fn workspace() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    (dir, ws)
}

fn skill_file(ws: &Path, dir_name: &str) -> PathBuf {
    ws.join("skills").join(dir_name).join("SKILL.md")
}

#[tokio::test]
async fn test_gate_installs_passing_draft() {
    let (_g, ws) = workspace();
    // 首稿即含 FIXED（首跑全过，零 refine）。
    let runner = ContentKeyedRunner::passing();
    let records = gate_skill_drafts(
        Some(&(Arc::new(runner) as Arc<dyn SkillTestRunner>)),
        None,
        &ws,
        &[draft("demo", valid_md("FIXED"))],
        &stopped_fn(false),
        8192,
        0.7,
    )
    .await;
    assert_eq!(records.len(), 1);
    assert!(
        matches!(records[0].verdict, GateVerdict::Installed),
        "{:?}",
        records[0]
    );
    assert_eq!(records[0].refine_rounds, 0);
    assert!(records[0].slug.ends_with("-forge"));
    assert!(
        skill_file(&ws, &records[0].slug).exists(),
        "落盘文件必须存在"
    );
}

#[tokio::test]
async fn test_gate_rejects_missing_tests() {
    let (_g, ws) = workspace();
    let runner = Arc::new(ContentKeyedRunner::passing());
    let md = "---\nname: no-tests\ndescription: x\n---\nbody".to_string();
    let records = gate_skill_drafts(
        Some(&(runner.clone() as Arc<dyn SkillTestRunner>)),
        None,
        &ws,
        &[draft("no-tests", md)],
        &stopped_fn(false),
        8192,
        0.7,
    )
    .await;
    assert!(
        matches!(&records[0].verdict, GateVerdict::Rejected(r) if r.contains("无 tests")),
        "{:?}",
        records[0]
    );
    assert!(runner.seen().is_empty(), "拒绝不得触达执行器");
    assert!(!skill_file(&ws, &records[0].slug).exists());
}

#[tokio::test]
async fn test_gate_rejects_invalid_spec() {
    let (_g, ws) = workspace();
    // tests 段存在但用例无断言（validate_spec 拒绝）。
    let md = "---\nname: bad\ntests:\n  - name: empty\n---\nbody".to_string();
    let records = gate_skill_drafts(
        Some(&(Arc::new(ContentKeyedRunner::passing()) as Arc<dyn SkillTestRunner>)),
        None,
        &ws,
        &[draft("bad", md)],
        &stopped_fn(false),
        8192,
        0.7,
    )
    .await;
    assert!(
        matches!(&records[0].verdict, GateVerdict::Rejected(r) if r.contains("非法")),
        "{:?}",
        records[0]
    );
}

#[tokio::test]
async fn test_gate_refine_then_install() {
    let (_g, ws) = workspace();
    let runner = Arc::new(ContentKeyedRunner::passing());
    let fixed = format!("```markdown\n{}\n```", valid_md("FIXED"));
    let fixer = Arc::new(ScriptedFixer::new(vec![Ok(fixed)]));
    let records = gate_skill_drafts(
        Some(&(runner.clone() as Arc<dyn SkillTestRunner>)),
        Some(&(fixer.clone() as Arc<dyn LLMCaller>)),
        &ws,
        &[draft("demo", valid_md("BROKEN"))],
        &stopped_fn(false),
        8192,
        0.7,
    )
    .await;
    assert!(
        matches!(records[0].verdict, GateVerdict::Installed),
        "{:?}",
        records[0]
    );
    assert_eq!(records[0].refine_rounds, 1);
    assert_eq!(fixer.calls(), 1);
    // 首跑 BROKEN 一次 + refine 后 FIXED 重跑一次。
    let seen = runner.seen();
    assert_eq!(seen.len(), 2);
    assert!(seen[1].contains("FIXED"));
    let on_disk = std::fs::read_to_string(skill_file(&ws, &records[0].slug)).unwrap();
    assert!(on_disk.contains("FIXED"));
    // 围栏必须被剥掉——落盘的是纯 SKILL.md。
    assert!(!on_disk.contains("```"), "代码围栏不得落盘");
}

#[tokio::test]
async fn test_gate_refine_strip_preserves_inner_fence_and_tail() {
    // BUG-20261128-022：修正稿正文自带 fenced 代码块（skill 常见形态）且
    // 回复被外层围栏包裹时，旧剥壳按「首个独立 ``` 行」截断——内层代码块
    // 收尾被当外壳收尾，其后正文（含 FIXED 标记与收尾段）静默丢失，截断
    // 稿重跑必败 → refine 耗尽弃稿。剥外壳必须只动结尾：
    // nemesis_utils::sanitize::strip_outer_code_fence。
    let (_g, ws) = workspace();
    let runner = Arc::new(ContentKeyedRunner::passing());
    let fixed_body = "---\nname: gate-demo\ndescription: demo\nassert_marker: FIXED\ntests:\n  - name: smoke\n    turns:\n      - user: hi\n    expect:\n      contains: [\"ok\"]\n---\n## 用法示例\n\n```bash\nrun demo --help\n```\n\nbody FIXED\n\n## 收尾段\nTAIL-MARKER\n";
    let fixed = format!("```markdown\n{fixed_body}\n```");
    let fixer = Arc::new(ScriptedFixer::new(vec![Ok(fixed)]));
    let records = gate_skill_drafts(
        Some(&(runner.clone() as Arc<dyn SkillTestRunner>)),
        Some(&(fixer.clone() as Arc<dyn LLMCaller>)),
        &ws,
        &[draft("demo", valid_md("BROKEN"))],
        &stopped_fn(false),
        8192,
        0.7,
    )
    .await;
    assert!(
        matches!(records[0].verdict, GateVerdict::Installed),
        "{:?}",
        records[0]
    );
    let on_disk = std::fs::read_to_string(skill_file(&ws, &records[0].slug)).unwrap();
    assert!(on_disk.contains("```bash"), "内层代码块必须原样保留");
    assert!(
        on_disk.contains("TAIL-MARKER"),
        "内层围栏之后的正文不得被截断"
    );
    assert!(!on_disk.contains("```markdown"), "外层围栏必须剥净");
}

#[tokio::test]
async fn test_gate_refine_exhausts_discards() {
    let (_g, ws) = workspace();
    let runner = Arc::new(ContentKeyedRunner::failing());
    let fixed_but_still_failing = valid_md("FIXED");
    let fixer = Arc::new(ScriptedFixer::new(vec![
        Ok(fixed_but_still_failing.clone()),
        Ok(fixed_but_still_failing),
    ]));
    let records = gate_skill_drafts(
        Some(&(runner.clone() as Arc<dyn SkillTestRunner>)),
        Some(&(fixer.clone() as Arc<dyn LLMCaller>)),
        &ws,
        &[draft("demo", valid_md("BROKEN"))],
        &stopped_fn(false),
        8192,
        0.7,
    )
    .await;
    assert!(
        matches!(records[0].verdict, GateVerdict::FailedAfterRefine),
        "{:?}",
        records[0]
    );
    assert_eq!(records[0].refine_rounds, REFINE_MAX_ROUNDS);
    assert!(!records[0].last_failures.is_empty(), "弃稿必须留失败明细");
    assert!(
        runner.seen().len() == 1 + REFINE_MAX_ROUNDS as usize,
        "首跑 + 每轮 refine 重跑"
    );
    assert!(fixer.calls() == REFINE_MAX_ROUNDS as usize);
    // 弃稿不落盘。
    let skills_dir = ws.join("skills");
    if skills_dir.exists() {
        assert!(
            std::fs::read_dir(&skills_dir).unwrap().next().is_none(),
            "弃稿后 skills 目录必须为空"
        );
    }
}

#[tokio::test]
async fn test_gate_no_runner_fail_closed() {
    let (_g, ws) = workspace();
    let fixer = Arc::new(ScriptedFixer::new(vec![]));
    let records = gate_skill_drafts(
        None,
        Some(&(fixer.clone() as Arc<dyn LLMCaller>)),
        &ws,
        &[draft("demo", valid_md("FIXED"))],
        &stopped_fn(false),
        8192,
        0.7,
    )
    .await;
    assert!(
        matches!(&records[0].verdict, GateVerdict::Rejected(r) if r.contains("执行器未装配")),
        "{:?}",
        records[0]
    );
    assert_eq!(fixer.calls(), 0, "执行器缺席不得触发 refine");
    assert!(!skill_file(&ws, &records[0].slug).exists());
}

#[tokio::test]
async fn test_gate_estop_aborts_before_refine() {
    let (_g, ws) = workspace();
    let fixer = Arc::new(ScriptedFixer::new(vec![]));
    let records = gate_skill_drafts(
        Some(&(Arc::new(ContentKeyedRunner::failing()) as Arc<dyn SkillTestRunner>)),
        Some(&(fixer.clone() as Arc<dyn LLMCaller>)),
        &ws,
        &[draft("demo", valid_md("BROKEN"))],
        &stopped_fn(true),
        8192,
        0.7,
    )
    .await;
    assert!(
        matches!(&records[0].verdict, GateVerdict::Rejected(r) if r.contains("急停")),
        "{:?}",
        records[0]
    );
    assert_eq!(records[0].refine_rounds, 0);
    assert_eq!(fixer.calls(), 0);
}

#[tokio::test]
async fn test_gate_slug_sanitized_no_escape() {
    let (_g, ws) = workspace();
    let records = gate_skill_drafts(
        Some(&(Arc::new(ContentKeyedRunner::passing()) as Arc<dyn SkillTestRunner>)),
        None,
        &ws,
        &[draft("../evil", valid_md("FIXED"))],
        &stopped_fn(false),
        8192,
        0.7,
    )
    .await;
    assert!(
        matches!(records[0].verdict, GateVerdict::Installed),
        "{:?}",
        records[0]
    );
    assert!(!records[0].slug.contains(".."), "消毒后不得残留穿越段");
    // 文件必须落在 workspace 内（穿越形态落不进 workspace 外）。
    assert!(skill_file(&ws, &records[0].slug).exists());
    let canonical_ws = ws.canonicalize().unwrap();
    let canonical_file = skill_file(&ws, &records[0].slug).canonicalize().unwrap();
    assert!(canonical_file.starts_with(&canonical_ws));
}
