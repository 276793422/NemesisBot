use super::*;
use crate::types::ToolCallInfo;

fn resp_with_tool(name: &str, args: &str) -> LlmResponse {
    LlmResponse {
        content: String::new(),
        tool_calls: vec![ToolCallInfo {
            id: "tc".to_string(),
            name: name.to_string(),
            arguments: args.to_string(),
        }],
        finished: false,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }
}

fn read_file_task() -> ProbeTask {
    probe_tasks()
        .into_iter()
        .find(|t| t.expected_tool == "read_file")
        .unwrap()
}

#[test]
fn score_no_tool_call_is_all_zero() {
    let resp = LlmResponse {
        content: "I refuse to use tools.".to_string(),
        tool_calls: vec![],
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    };
    let s = score_response(&resp, &read_file_task());
    assert_eq!(s, ProbeScore::default());
}

#[test]
fn score_correct_tool_valid_args_is_full_marks() {
    let resp = resp_with_tool("read_file", r#"{"path":"README.md"}"#);
    let s = score_response(&resp, &read_file_task());
    assert_eq!(
        s,
        ProbeScore {
            format: 1.0,
            selection: 1.0,
            schema: 1.0
        }
    );
}

#[test]
fn score_wrong_tool_is_zero_selection() {
    let resp = resp_with_tool("exec", r#"{"command":"cat README.md"}"#);
    let s = score_response(&resp, &read_file_task());
    assert_eq!(s.selection, 0.0);
    assert_eq!(s.format, 1.0); // still used the channel
}

#[test]
fn score_autofixable_args_is_half_schema() {
    // "patch" is edit-distance 1 from "path" → autofixed → 0.5
    let resp = resp_with_tool("read_file", r#"{"patch":"README.md"}"#);
    let s = score_response(&resp, &read_file_task());
    assert_eq!(s.schema, 0.5);
    assert_eq!(s.selection, 1.0);
}

#[test]
fn score_missing_required_is_zero_schema() {
    let resp = resp_with_tool("read_file", r#"{}"#);
    let s = score_response(&resp, &read_file_task());
    assert_eq!(s.schema, 0.0);
}

#[test]
fn tier_mapping() {
    assert_eq!(tier_from_scores(1.0, 1.0, 1.0), ModelTier::Big);
    assert_eq!(tier_from_scores(0.9, 0.85, 0.7), ModelTier::Normal);
    assert_eq!(tier_from_scores(0.3, 0.3, 0.3), ModelTier::Mini);
    assert_eq!(tier_from_scores(0.0, 0.0, 0.0), ModelTier::Mini);
}

#[test]
fn probe_tasks_has_seven_including_cluster() {
    let tasks = probe_tasks();
    assert_eq!(tasks.len(), 7);
    assert!(tasks.iter().any(|t| t.expected_tool == "cluster_rpc"));
}

#[test]
fn probe_tool_defs_dedupes() {
    let defs = probe_tool_defs();
    // 7 tasks but several share read_file/write_file/etc tools; dedup by name.
    let names: Vec<&str> = defs.iter().map(|d| d.function.name.as_str()).collect();
    let mut sorted = names.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(names.len(), sorted.len()); // no dupes
    assert!(names.contains(&"read_file"));
    assert!(names.contains(&"cluster_rpc"));
}

// --- W3a: run() 电池测试（mock provider 按 prompt 分发）---

use crate::types::{ChatOptions, ToolDefinition};
use std::sync::atomic::{AtomicUsize, Ordering};

/// 每个探针任务的合法参数（schema Valid）。
fn valid_args_for(tool: &str) -> &'static str {
    match tool {
        "exec" => r#"{"command":"date"}"#,
        "read_file" => r#"{"path":"README.md"}"#,
        "create_dir" => r#"{"path":"test"}"#,
        "grep" => r#"{"pattern":"TODO"}"#,
        "write_file" => r#"{"path":"note.md","content":"x"}"#,
        "edit_file" => r#"{"path":"note.md","old_text":"foo","new_text":"bar"}"#,
        "cluster_rpc" => r#"{"target_node":"n1","message":"你好"}"#,
        _ => r#"{}"#,
    }
}

fn empty_resp() -> LlmResponse {
    LlmResponse {
        content: "I'll just answer directly.".to_string(),
        tool_calls: vec![],
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }
}

/// 按 user prompt 找到当前任务，再按 mode 生成响应。
struct MockProbeProvider {
    mode: &'static str, // "perfect" | "normal" | "mini" | "fail"
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl crate::r#loop::LlmProvider for MockProbeProvider {
    async fn chat(
        &self,
        _model: &str,
        messages: Vec<crate::r#loop::LlmMessage>,
        _options: Option<ChatOptions>,
        _tools: Vec<ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.mode == "fail" {
            return Err("provider down".to_string());
        }
        let prompt = messages
            .last()
            .map(|m| m.content.clone())
            .unwrap_or_default();
        let task = probe_tasks()
            .into_iter()
            .find(|t| t.prompt == prompt.as_str())
            .expect("prompt must match a probe task");

        match self.mode {
            "perfect" => Ok(resp_with_tool(task.expected_tool, valid_args_for(task.expected_tool))),
            "normal" => {
                if task.expected_tool == "edit_file" {
                    Ok(empty_resp()) // format 失败一题
                } else if task.expected_tool == "grep" {
                    // 用了工具通道但选错工具（args 对 grep 任务仍合法）
                    Ok(resp_with_tool("exec", valid_args_for("grep")))
                } else {
                    Ok(resp_with_tool(task.expected_tool, valid_args_for(task.expected_tool)))
                }
            }
            "mini" => {
                // 前 4 题完美，后 3 题不使用工具 → 三轴 4/7 ≈ 0.571 → Mini
                match task.expected_tool {
                    "exec" | "read_file" | "create_dir" | "grep" => Ok(resp_with_tool(
                        task.expected_tool,
                        valid_args_for(task.expected_tool),
                    )),
                    _ => Ok(empty_resp()),
                }
            }
            _ => unreachable!("unknown mode"),
        }
    }
}

#[tokio::test]
async fn run_all_perfect_maps_to_big() {
    let p = MockProbeProvider {
        mode: "perfect",
        calls: AtomicUsize::new(0),
    };
    let report = run(&p, "test-model").await.expect("probe run ok");
    assert_eq!(report.format_score, 1.0);
    assert_eq!(report.selection_score, 1.0);
    assert_eq!(report.schema_score, 1.0);
    assert_eq!(report.tier, ModelTier::Big);
    assert_eq!(report.per_task.len(), 7);
    assert_eq!(report.per_task[0].0, "exec");
    assert!(report.per_task.iter().all(|(_, s)| s.format == 1.0));
    assert_eq!(p.calls.load(Ordering::SeqCst), 7, "one LLM call per task");
}

#[tokio::test]
async fn run_mixed_performance_maps_to_normal() {
    // fmt=6/7≈0.857 sel=5/7≈0.714 schema=6/7≈0.857 → 不满足 Big(sel<0.8)，
    // 满足 Normal(schema>=0.6 且 fmt/sel>=0.5)。
    let p = MockProbeProvider {
        mode: "normal",
        calls: AtomicUsize::new(0),
    };
    let report = run(&p, "test-model").await.expect("probe run ok");
    assert!(report.format_score > 0.8 && report.format_score < 1.0);
    assert!(report.selection_score < 0.8);
    assert_eq!(report.tier, ModelTier::Normal);
}

#[tokio::test]
async fn run_majority_format_failure_maps_to_mini() {
    // 4/7 三轴 ≈ 0.571：Big 不满足，Normal 的 schema>=0.6 不满足 → Mini。
    let p = MockProbeProvider {
        mode: "mini",
        calls: AtomicUsize::new(0),
    };
    let report = run(&p, "test-model").await.expect("probe run ok");
    let expected = 4.0 / 7.0;
    assert!((report.format_score - expected).abs() < 1e-9);
    assert!((report.selection_score - expected).abs() < 1e-9);
    assert_eq!(report.tier, ModelTier::Mini);
}

#[tokio::test]
async fn run_provider_error_propagates_with_task_name() {
    let p = MockProbeProvider {
        mode: "fail",
        calls: AtomicUsize::new(0),
    };
    let err = run(&p, "test-model").await.expect_err("must fail");
    assert!(err.contains("LLM chat failed"), "err: {err}");
    assert!(err.contains("exec"), "first task name in err: {err}");
    assert!(err.contains("provider down"), "err: {err}");
    assert_eq!(p.calls.load(Ordering::SeqCst), 1, "fails on first task");
}
