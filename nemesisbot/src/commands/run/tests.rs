//! K1 纯函数测试：参数校验/归一 + 文本模式事件折叠。
//! （装配与 e2e 属于门 4 的 TestAIServer 实机验收，不在此重复。）

use super::*;

// ---------------------------------------------------------------------------
// validate
// ---------------------------------------------------------------------------

#[test]
fn validate_defaults_build_mode_text_format() {
    let v = validate(Some("do X".into()), None, None, None, None, None).unwrap();
    assert_eq!(v.task, TaskSource::Arg("do X".into()));
    assert_eq!(v.mode, nemesis_agent::types::AgentMode::Build);
    assert_eq!(v.format, OutputFormat::Text);
    assert_eq!(v.max_turns, 0, "缺省 = 0 = 用配置默认");
    assert!(v.model.is_none());
    assert!(v.workspace.is_none());
}

#[test]
fn validate_dash_and_missing_task_both_mean_stdin() {
    let v = validate(None, None, None, None, None, None).unwrap();
    assert_eq!(v.task, TaskSource::Stdin);
    let v = validate(Some("-".into()), None, None, None, None, None).unwrap();
    assert_eq!(v.task, TaskSource::Stdin);
}

#[test]
fn validate_mode_is_case_tolerant() {
    let v = validate(None, None, Some("PLAN"), None, None, None).unwrap();
    assert_eq!(v.mode, nemesis_agent::types::AgentMode::Plan);
    let v = validate(None, None, Some("build"), None, None, None).unwrap();
    assert_eq!(v.mode, nemesis_agent::types::AgentMode::Build);
}

#[test]
fn validate_unknown_mode_rejected_with_remedy() {
    let err = validate(None, None, Some("yolo"), None, None, None).unwrap_err();
    assert!(err.contains("yolo"), "错误要回显原值: {err}");
    assert!(
        err.contains("plan") && err.contains("build"),
        "错误要给合法值: {err}"
    );
}

#[test]
fn validate_format_json_lands_k2() {
    let v = validate(None, None, None, Some("json"), None, None).unwrap();
    assert_eq!(v.format, OutputFormat::Json, "K2 已落地：json 合法");
}

#[test]
fn validate_unknown_format_rejected() {
    let err = validate(None, None, None, Some("yaml"), None, None).unwrap_err();
    assert!(err.contains("yaml") && err.contains("text"), "{err}");
}

#[test]
fn validate_carries_workspace_model_and_turn_budget() {
    let v = validate(
        None,
        Some(PathBuf::from("/tmp/wsx")),
        None,
        None,
        Some(42),
        Some("test/testai-1.1".into()),
    )
    .unwrap();
    assert_eq!(v.workspace, Some(PathBuf::from("/tmp/wsx")));
    assert_eq!(v.max_turns, 42);
    assert_eq!(v.model.as_deref(), Some("test/testai-1.1"));
}

// ---------------------------------------------------------------------------
// fold_text
// ---------------------------------------------------------------------------

use nemesis_agent::types::AgentEvent;

#[test]
fn fold_text_prefers_done_over_error() {
    let events = vec![
        AgentEvent::Message("intermediate".into()),
        AgentEvent::Error("stale".into()),
        AgentEvent::Done("final answer".into()),
    ];
    let (done, error) = fold_text(&events);
    assert_eq!(done.as_deref(), Some("final answer"));
    // error 记录最后一条 Error（与 run_detached 折叠同构）；调用方先看
    // done —— (Some, _) 臂即成功，error 此时无语义。
    assert_eq!(error.as_deref(), Some("stale"));
}

#[test]
fn fold_text_error_without_done_is_the_failure() {
    let events = vec![AgentEvent::Error("boom".into())];
    let (done, error) = fold_text(&events);
    assert_eq!(done, None);
    assert_eq!(error.as_deref(), Some("boom"));
}

#[test]
fn fold_text_no_terminal_event_is_honest_empty() {
    let events = vec![AgentEvent::Message("partial".into())];
    let (done, error) = fold_text(&events);
    assert_eq!(done, None);
    assert_eq!(error, None);
}

#[test]
fn fold_text_ignores_tool_traffic() {
    let events = vec![
        AgentEvent::ToolCall(Vec::new()),
        AgentEvent::ToolResult(nemesis_agent::types::ToolCallResult {
            tool_name: "exec".into(),
            result: "out".into(),
            is_error: false,
        }),
        AgentEvent::Done("ok".into()),
    ];
    let (done, _) = fold_text(&events);
    assert_eq!(done.as_deref(), Some("ok"));
}

// ---------------------------------------------------------------------------
// K2 NDJSON 序列化
// ---------------------------------------------------------------------------

use nemesis_types::agent::AgentEvent as LiveEvent;

#[test]
fn live_tool_start_serializes_with_schema_fields() {
    let ev = LiveEvent::ToolStarted {
        session_key: "subagent:s".into(),
        chat_id: "subagent:s".into(),
        call_id: "c1".into(),
        tool: "exec".into(),
        args_preview: "{\"command\":\"ls\"}".into(),
    };
    let v: serde_json::Value = serde_json::from_str(&serialize_live_event(&ev).unwrap()).unwrap();
    assert_eq!(v["type"], "tool_start");
    assert_eq!(v["call_id"], "c1");
    assert_eq!(v["tool"], "exec");
    assert_eq!(v["args_preview"], "{\"command\":\"ls\"}");
}

#[test]
fn live_tool_end_serializes_with_result_and_ok() {
    let ev = LiveEvent::ToolFinished {
        session_key: "subagent:s".into(),
        chat_id: "subagent:s".into(),
        call_id: "c1".into(),
        tool: "exec".into(),
        duration_ms: 42,
        ok: false,
        result_preview: "Tool error: boom".into(),
    };
    let v: serde_json::Value = serde_json::from_str(&serialize_live_event(&ev).unwrap()).unwrap();
    assert_eq!(v["type"], "tool_end");
    assert_eq!(v["duration_ms"], 42);
    assert_eq!(v["ok"], false);
    assert_eq!(v["result_preview"], "Tool error: boom");
}

#[test]
fn live_events_outside_v1_contract_are_skipped() {
    // TodoUpdated / ModeChanged 不在 v1 固定 5-type 契约内——不发未知 type。
    let todo = LiveEvent::TodoUpdated {
        session_key: "s".into(),
        chat_id: "s".into(),
        todos: vec![],
    };
    let mode = LiveEvent::ModeChanged {
        session_key: "s".into(),
        chat_id: "s".into(),
        mode: "plan".into(),
    };
    assert_eq!(serialize_live_event(&todo), None);
    assert_eq!(serialize_live_event(&mode), None);
}

#[test]
fn terminal_events_serialize_turn_final_error_in_order() {
    let events = vec![
        AgentEvent::Message("thinking out loud".into()),
        AgentEvent::ToolCall(Vec::new()),
        AgentEvent::ToolResult(nemesis_agent::types::ToolCallResult {
            tool_name: "exec".into(),
            result: "out".into(),
            is_error: false,
        }),
        AgentEvent::Done("the answer".into()),
    ];
    let lines = serialize_terminal_events(&events);
    assert_eq!(lines.len(), 2, "工具流量不产生终结行");
    let t0: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    let t1: serde_json::Value = serde_json::from_str(&lines[1]).unwrap();
    assert_eq!(t0["type"], "turn");
    assert_eq!(t0["text"], "thinking out loud");
    assert_eq!(t1["type"], "final");
    assert_eq!(t1["text"], "the answer");
}

#[test]
fn terminal_error_serializes_without_final() {
    let lines = serialize_terminal_events(&[AgentEvent::Error("boom".into())]);
    assert_eq!(lines.len(), 1);
    let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    assert_eq!(v["type"], "error");
    assert_eq!(v["message"], "boom");
}

#[test]
fn terminal_empty_is_empty_lines() {
    assert!(serialize_terminal_events(&[]).is_empty());
}
