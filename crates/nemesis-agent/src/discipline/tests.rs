//! 纪律闭环测试（件4）。覆盖：声明校验四态 / 路径豁免 / 闸五路径 /
//! 证伪通过-失败-预算耗尽 / estop fail-open / marker 参与 / waive 审计 /
//! `/discipline` gate 臂。全部真子进程（exit 0/2），与闸/证伪执行协议一致。

use std::sync::Arc;

use super::{
    DISCIPLINE_DIR, Declaration, DisciplineFalsificationHook, DisciplineGateHook, DisciplineState,
    MAX_FALSIFICATION_RUNS,
};
use crate::hooks::{
    HookDecision, HookPrompt, HookToolCall, LifecycleHook, ToolHook, TurnEndDecision,
};
use crate::r#loop::{GateOutcome, LlmMessage, LlmProvider, LlmResponse};

// ---------------------------------------------------------------------------
// 助手
// ---------------------------------------------------------------------------

fn tempdir() -> std::path::PathBuf {
    tempfile::tempdir().expect("tempdir").keep() // 测试进程内自管生命周期
}

fn declaration_doc(cmd: &str) -> String {
    serde_json::json!({
        "root_cause": "src/parser.rs:42 未判空",
        "truth_source": "issue #12 复现栈",
        "invariant": "既有签名不破坏",
        "impact": "仅 parser 模块",
        "single_variable": "只改 parse_expr 判空分支",
        "falsification_cmd": cmd,
    })
    .to_string()
}

fn write_declaration(ws: &std::path::Path, cmd: &str) {
    let dir = ws.join(DISCIPLINE_DIR);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("declaration.json"), declaration_doc(cmd)).unwrap();
}

fn state(ws: &std::path::Path) -> Arc<DisciplineState> {
    let st = DisciplineState::new(true, ws.to_path_buf(), false);
    st.set_interactive("sk-1");
    st
}

fn call(name: &str, path: &str) -> HookToolCall {
    HookToolCall {
        name: name.to_string(),
        arguments: serde_json::json!({ "path": path, "content": "x" }).to_string(),
        channel: "web".to_string(),
        chat_id: "1".to_string(),
        session_key: "sk-1".to_string(),
    }
}

fn gate(st: Arc<DisciplineState>) -> DisciplineGateHook {
    DisciplineGateHook::new(st)
}

fn falsifier(st: Arc<DisciplineState>) -> DisciplineFalsificationHook {
    DisciplineFalsificationHook::new(st)
}

fn turn_end() -> crate::hooks::HookTurnEnd {
    crate::hooks::HookTurnEnd {
        session_key: "sk-1".to_string(),
        channel: "web".to_string(),
        chat_id: "1".to_string(),
        final_content: "done".to_string(),
        stop_hook_active: false,
    }
}

fn pass_cmd() -> &'static str {
    if cfg!(windows) { "exit 0" } else { "true" }
}

fn fail_cmd() -> &'static str {
    // 注意不能用 `exit /b 2`：batch-context 退出不透过 `cmd /C` 包装（对
    // 外退出码变 0）；裸 `exit 2` 在 cmd /C 与 sh -c 下同形。
    "exit 2"
}

fn falsification_record(ws: &std::path::Path, run: u32) -> serde_json::Value {
    let text = std::fs::read_to_string(
        ws.join(DISCIPLINE_DIR)
            .join(format!("falsification-{run}.json")),
    )
    .expect("record exists");
    serde_json::from_str(&text).expect("record is json")
}

// ---------------------------------------------------------------------------
// 声明校验
// ---------------------------------------------------------------------------

#[test]
fn declaration_missing_file_bad_json_and_fields() {
    let ws = tempdir();
    // 缺失
    let err = Declaration::from_workspace(&ws).expect_err("missing");
    assert!(err.contains("声明缺失"), "err={err}");
    // 坏 JSON
    let dir = ws.join(DISCIPLINE_DIR);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("declaration.json"), "{nope").unwrap();
    let err = Declaration::from_workspace(&ws).expect_err("bad json");
    assert!(err.contains("JSON"), "err={err}");
    // 字段空
    std::fs::write(
        dir.join("declaration.json"),
        serde_json::json!({ "root_cause": "", }).to_string(),
    )
    .unwrap();
    let err = Declaration::from_workspace(&ws).expect_err("empty field");
    assert!(err.contains("root_cause"), "err={err}");
}

#[test]
fn declaration_valid_roundtrip() {
    let ws = tempdir();
    write_declaration(&ws, pass_cmd());
    let d = Declaration::from_workspace(&ws).expect("valid");
    assert_eq!(d.falsification_cmd, pass_cmd());
    assert!(d.root_cause.contains("parser.rs:42"));
}

// ---------------------------------------------------------------------------
// 路径豁免
// ---------------------------------------------------------------------------

#[test]
fn is_discipline_path_matches_component_anywhere() {
    use super::is_discipline_path;
    assert!(is_discipline_path(".discipline/declaration.json"));
    assert!(is_discipline_path(".discipline"));
    assert!(is_discipline_path("ws/.discipline/falsification-1.json"));
    assert!(!is_discipline_path("src/main.rs"));
    assert!(!is_discipline_path("src/discipline_notes/x.txt"));
    // Windows 反斜杠形态
    assert!(is_discipline_path("ws\\.discipline\\falsification-1.json"));
}

// ---------------------------------------------------------------------------
// 声明闸
// ---------------------------------------------------------------------------

#[tokio::test]
async fn gate_allows_non_gated_tool_and_non_participating_session() {
    let ws = tempdir();
    let st = DisciplineState::new(true, ws.clone(), false); // 未参与
    let g = gate(st);
    // 非闸面工具（exec 有意不闸——诚实边界）
    assert_eq!(
        g.pre_tool_use(&call("exec", "src/main.rs")).await,
        HookDecision::Allow
    );
    // 闸面工具但未参与
    assert_eq!(
        g.pre_tool_use(&call("write_file", "src/main.rs")).await,
        HookDecision::Allow
    );
}

#[tokio::test]
async fn gate_blocks_without_declaration_and_allows_discipline_path() {
    let ws = tempdir();
    let g = gate(state(&ws));
    let decision = g.pre_tool_use(&call("write_file", "src/main.rs")).await;
    match decision {
        HookDecision::Block { reason } => {
            assert!(reason.contains("纪律闸"), "reason={reason}");
            assert!(reason.contains("root_cause"), "schema 提示必须在文案里");
            assert!(reason.contains(".discipline/declaration.json"));
        }
        other => panic!("expected block, got {other:?}"),
    }
    // `.discipline/**` 豁免：无声明也放行（写声明本身不能被闸）。
    assert_eq!(
        g.pre_tool_use(&call("write_file", ".discipline/declaration.json"))
            .await,
        HookDecision::Allow
    );
    // 四工具全闸面（edit/append/delete 同语义——抽两个代表）。
    for name in ["edit_file", "delete_file"] {
        assert!(
            matches!(
                g.pre_tool_use(&call(name, "src/main.rs")).await,
                HookDecision::Block { .. }
            ),
            "{name} must be gated"
        );
    }
}

#[tokio::test]
async fn gate_allows_with_valid_declaration() {
    let ws = tempdir();
    write_declaration(&ws, pass_cmd());
    let g = gate(state(&ws));
    assert_eq!(
        g.pre_tool_use(&call("write_file", "src/main.rs")).await,
        HookDecision::Allow
    );
}

#[tokio::test]
async fn gate_inert_when_state_disabled() {
    let ws = tempdir();
    let st = DisciplineState::new(false, ws.clone(), false);
    st.set_interactive("sk-1");
    let g = gate(st);
    assert_eq!(
        g.pre_tool_use(&call("write_file", "src/main.rs")).await,
        HookDecision::Allow
    );
}

// ---------------------------------------------------------------------------
// 证伪执行
// ---------------------------------------------------------------------------

#[tokio::test]
async fn falsification_pass_then_no_more_continue() {
    let ws = tempdir();
    write_declaration(&ws, pass_cmd());
    let f = falsifier(state(&ws));
    let first = f.on_turn_end(&turn_end()).await;
    match first {
        TurnEndDecision::Continue { feedback } => {
            assert!(feedback.contains("证伪通过"), "feedback={feedback}");
        }
        other => panic!("expected continue, got {other:?}"),
    }
    let rec = falsification_record(&ws, 1);
    assert_eq!(rec["run"], 1);
    assert_eq!(rec["passed"], true);
    // 通过后条件「未跑或上次未过」转假 → 放行收尾。
    assert_eq!(f.on_turn_end(&turn_end()).await, TurnEndDecision::Stop);
}

#[tokio::test]
async fn falsification_fail_budget_exhaustion_escalates() {
    let ws = tempdir();
    write_declaration(&ws, fail_cmd());
    let f = falsifier(state(&ws));
    for run in 1..=MAX_FALSIFICATION_RUNS {
        match f.on_turn_end(&turn_end()).await {
            TurnEndDecision::Continue { feedback } => {
                assert!(
                    feedback.contains("证伪失败"),
                    "run={run} feedback={feedback}"
                );
                assert!(feedback.contains("剩余证伪预算"), "run={run}");
            }
            other => panic!("run {run}: expected continue, got {other:?}"),
        }
        let rec = falsification_record(&ws, run);
        assert_eq!(rec["passed"], false, "run={run}");
    }
    // 预算耗尽（D4）：停车升级不静默——Stop（warn 已发，产物留盘供评审注记）。
    assert_eq!(f.on_turn_end(&turn_end()).await, TurnEndDecision::Stop);
}

#[tokio::test]
async fn falsification_noop_without_participation_or_declaration() {
    let ws = tempdir();
    // 未参与
    let st = DisciplineState::new(true, ws.clone(), false);
    assert_eq!(
        falsifier(st).on_turn_end(&turn_end()).await,
        TurnEndDecision::Stop
    );
    // 参与但无声明（闸之下没有文件变更 → 无可证伪）。
    assert_eq!(
        falsifier(state(&ws)).on_turn_end(&turn_end()).await,
        TurnEndDecision::Stop
    );
}

// ---------------------------------------------------------------------------
// marker 参与 + estop fail-open + waive 审计
// ---------------------------------------------------------------------------

#[tokio::test]
async fn marker_prompt_enables_participation() {
    let ws = tempdir();
    let st = DisciplineState::new(true, ws.clone(), false);
    let f = falsifier(st.clone());
    // 无 marker 的 prompt 不参与。
    f.on_user_prompt(&HookPrompt {
        session_key: "sk-1".to_string(),
        channel: "web".to_string(),
        chat_id: "1".to_string(),
        prompt: "fix the parser bug".to_string(),
    })
    .await;
    assert!(!st.active("sk-1"));
    // 带 marker 即参与。
    f.on_user_prompt(&HookPrompt {
        session_key: "sk-1".to_string(),
        channel: "web".to_string(),
        chat_id: "1".to_string(),
        prompt: "[discipline:bugfix] fix the parser bug".to_string(),
    })
    .await;
    assert!(st.active("sk-1"));
}

#[tokio::test]
async fn estop_engaged_fails_open() {
    let ws = tempdir();
    let st = state(&ws);
    let estop = Arc::new(crate::estop::EstopState::new());
    st.set_estop(estop.clone());
    estop.trigger();
    let g = gate(st.clone());
    // 无声明 + 参与 + estop 触发 → 闸放行（fail-open）。
    assert_eq!(
        g.pre_tool_use(&call("write_file", "src/main.rs")).await,
        HookDecision::Allow
    );
    // 证伪钩子同停用。
    write_declaration(&ws, pass_cmd());
    assert_eq!(
        falsifier(st).on_turn_end(&turn_end()).await,
        TurnEndDecision::Stop
    );
}

#[test]
fn waive_audit_written_with_reason_only() {
    let ws = tempdir();
    let st = state(&ws);
    // 无理由退出：不留审计。
    st.clear_interactive("sk-1", None);
    assert!(!ws.join(DISCIPLINE_DIR).join("waive-audit.jsonl").exists());
    // 重新参与 + 带理由退出：留痕。
    st.set_interactive("sk-1");
    st.clear_interactive("sk-1", Some("用户明确放弃"));
    let audit = std::fs::read_to_string(ws.join(DISCIPLINE_DIR).join("waive-audit.jsonl")).unwrap();
    assert!(audit.contains("用户明确放弃"));
    assert!(audit.contains("sk-1"));
    assert!(!st.active("sk-1"));
}

// ---------------------------------------------------------------------------
// `/discipline` gate 臂（admission）
// ---------------------------------------------------------------------------

fn inbound(content: &str) -> nemesis_types::channel::InboundMessage {
    nemesis_types::channel::InboundMessage {
        channel: "web".to_string(),
        sender_id: String::new(),
        chat_id: "1".to_string(),
        content: content.to_string(),
        media: Vec::new(),
        session_key: String::new(),
        correlation_id: String::new(),
        metadata: Default::default(),
        voice_playback: None,
    }
}

struct NoopProvider;

#[async_trait::async_trait]
impl LlmProvider for NoopProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Err("NoopProvider must not be called".to_string())
    }
}

#[test]
fn gate_inbound_discipline_arm_toggles_and_reports_disabled() {
    let agent_loop = crate::r#loop::AgentLoop::new(
        Box::new(NoopProvider),
        crate::types::AgentConfig {
            model: "test-model".to_string(),
            system_prompt: None,
            max_turns: 5,
            tools: Vec::new(),
            models: std::collections::HashMap::new(),
        },
    );
    // 未启用：诚实提示，不改状态。
    let GateOutcome::Immediate { response, .. } =
        agent_loop.gate_inbound(&inbound("/discipline on"))
    else {
        panic!("expected immediate");
    };
    assert!(response.contains("未启用"), "response={response}");

    // 启用后：on 进入参与态（幂等再 on 不新增）、off 带理由退出并留痕。
    let st = DisciplineState::new(true, tempdir(), false);
    agent_loop.set_discipline(st.clone());
    let GateOutcome::Immediate { response, .. } =
        agent_loop.gate_inbound(&inbound("/discipline on"))
    else {
        panic!("expected immediate");
    };
    assert!(response.contains("已开启"), "response={response}");
    assert_eq!(st.participating_len(), 1);
    let GateOutcome::Immediate { response, .. } =
        agent_loop.gate_inbound(&inbound("/discipline off 用户收工"))
    else {
        panic!("expected immediate");
    };
    assert!(response.contains("用户收工"), "response={response}");
    assert_eq!(st.participating_len(), 0);
}
