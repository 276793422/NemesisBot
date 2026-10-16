use super::*;
use crate::reflector::{PatternInsight, RetryPattern, ToolChainPattern};
use std::collections::HashMap;

/// A mock LLM caller that echoes back the user prompt.
struct MockLLMCaller;

#[async_trait]
impl LLMCaller for MockLLMCaller {
    async fn chat(
        &self,
        _system_prompt: &str,
        user_prompt: &str,
        _max_tokens: Option<i64>,
    ) -> Result<String, String> {
        Ok(format!("Mock LLM response for: {}", user_prompt.len()))
    }
}

/// A mock LLM caller that returns bullet-point insights.
struct InsightMockLLMCaller;

#[async_trait]
impl LLMCaller for InsightMockLLMCaller {
    async fn chat(
        &self,
        _system_prompt: &str,
        _user_prompt: &str,
        _max_tokens: Option<i64>,
    ) -> Result<String, String> {
        Ok("- First insight\n- Second insight\n- Third insight".to_string())
    }
}

/// A mock LLM caller that returns an error.
struct ErrorMockLLMCaller;

#[async_trait]
impl LLMCaller for ErrorMockLLMCaller {
    async fn chat(
        &self,
        _system_prompt: &str,
        _user_prompt: &str,
        _max_tokens: Option<i64>,
    ) -> Result<String, String> {
        Err("LLM service unavailable".to_string())
    }
}

fn make_stats() -> ExperienceStats {
    ExperienceStats {
        total_count: 100,
        success_count: 85,
        failure_count: 15,
        avg_duration_ms: 250.0,
        tool_counts: {
            let mut m = HashMap::new();
            m.insert(
                "file_read".into(),
                crate::types::ToolStats {
                    count: 50,
                    success_count: 48,
                    avg_duration_ms: 100.0,
                },
            );
            m
        },
    }
}

fn make_reflection_stats() -> ReflectionStats {
    ReflectionStats {
        total_records: 200,
        unique_patterns: 15,
        avg_success_rate: 0.85,
        top_patterns: vec![PatternInsight {
            tool_name: "file_read".to_string(),
            count: 100,
            avg_duration_ms: 120,
            success_rate: 0.95,
            suggestion: "High frequency pattern".to_string(),
        }],
        low_success: vec![PatternInsight {
            tool_name: "flaky_tool".to_string(),
            count: 20,
            avg_duration_ms: 500,
            success_rate: 0.3,
            suggestion: "Investigate failures".to_string(),
        }],
        tool_frequency: {
            let mut m = HashMap::new();
            m.insert("file_read".to_string(), 100);
            m.insert("file_write".to_string(), 50);
            m
        },
    }
}

fn make_trace_stats() -> TraceStats {
    TraceStats {
        total_traces: 50,
        avg_rounds: 3.5,
        avg_duration_ms: 2500,
        efficiency_score: 0.75,
        tool_chain_patterns: vec![ToolChainPattern {
            chain: "read->edit->exec".to_string(),
            count: 15,
            avg_rounds: 3.0,
            success_rate: 0.8,
        }],
        retry_patterns: vec![RetryPattern {
            tool_name: "exec".to_string(),
            retry_count: 5,
            success_rate: 0.6,
        }],
        signal_summary: {
            let mut m = HashMap::new();
            m.insert("retry".to_string(), 10);
            m.insert("backtrack".to_string(), 3);
            m
        },
    }
}

#[test]
fn test_build_analysis_prompt() {
    let stats = make_stats();
    let prompt = build_analysis_prompt(&stats, 5);
    assert!(prompt.contains("100"));
    assert!(prompt.contains("file_read"));
    assert!(prompt.contains("85.0%"));
}

#[test]
fn test_build_full_analysis_prompt_basic() {
    let stats = make_reflection_stats();
    let prompt = build_full_analysis_prompt(&stats, &[], None, None);
    assert!(prompt.contains("200"));
    assert!(prompt.contains("15"));
    assert!(prompt.contains("85.0%"));
    assert!(prompt.contains("file_read"));
    assert!(prompt.contains("flaky_tool"));
    assert!(prompt.contains("低成功率模式"));
}

#[test]
fn test_build_full_analysis_prompt_with_artifacts() {
    let stats = make_reflection_stats();
    let artifacts = vec![Artifact {
        id: "art-1".to_string(),
        name: "test-skill".to_string(),
        kind: nemesis_types::forge::ArtifactKind::Skill,
        version: "1.0".to_string(),
        status: nemesis_types::forge::ArtifactStatus::Active,
        content: "skill content".to_string(),
        tool_signature: vec!["file_read".to_string()],
        usage_count: 25,
        last_degraded_at: None,
        success_rate: 0.0,
        consecutive_observing_rounds: 0,
        deployed_at: None,
        last_efficacy: None,
        created_at: chrono::Local::now().to_rfc3339(),
        updated_at: chrono::Local::now().to_rfc3339(),
    }];
    let prompt = build_full_analysis_prompt(&stats, &artifacts, None, None);
    assert!(prompt.contains("test-skill"));
    assert!(prompt.contains("Skill"));
    assert!(prompt.contains("1.0"));
}

#[test]
fn test_build_full_analysis_prompt_with_trace_stats() {
    let stats = make_reflection_stats();
    let trace = make_trace_stats();
    let prompt = build_full_analysis_prompt(&stats, &[], Some(&trace), None);
    assert!(prompt.contains("会话级轨迹洞察"));
    assert!(prompt.contains("50"));
    assert!(prompt.contains("3.5"));
    assert!(prompt.contains("read->edit->exec"));
    assert!(prompt.contains("重试模式"));
    assert!(prompt.contains("会话信号"));
    assert!(prompt.contains("retry"));
}

#[test]
fn test_build_full_analysis_prompt_with_learning_cycle() {
    let stats = make_reflection_stats();
    let cycle = nemesis_types::forge::LearningCycle {
        id: "lc-1".to_string(),
        started_at: chrono::Local::now().to_rfc3339(),
        completed_at: None,
        patterns_found: 5,
        actions_taken: 3,
        status: nemesis_types::forge::CycleStatus::Completed,
    };
    let prompt = build_full_analysis_prompt(&stats, &[], None, Some(&cycle));
    assert!(prompt.contains("闭环学习状态"));
    assert!(prompt.contains("5"));
    assert!(prompt.contains("3"));
}

#[tokio::test]
async fn test_semantic_analysis_success() {
    let caller = MockLLMCaller;
    let stats = make_reflection_stats();
    let result = semantic_analysis(&caller, &stats, &[], None, None, Some(500)).await;
    assert!(result.is_ok());
    let response = result.unwrap();
    assert!(response.contains("Mock LLM response"));
}

#[tokio::test]
async fn test_semantic_analysis_with_insights() {
    let caller = InsightMockLLMCaller;
    let stats = make_reflection_stats();
    let result = semantic_analysis(&caller, &stats, &[], None, None, None).await;
    let response = result.unwrap();
    let insights = parse_insights(&response);
    assert_eq!(insights.len(), 3);
    assert_eq!(insights[0], "First insight");
}

#[tokio::test]
async fn test_semantic_analysis_error() {
    let caller = ErrorMockLLMCaller;
    let stats = make_reflection_stats();
    let result = semantic_analysis(&caller, &stats, &[], None, None, None).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("unavailable"));
}

#[tokio::test]
async fn test_semantic_analysis_with_all_contexts() {
    let caller = MockLLMCaller;
    let stats = make_reflection_stats();
    let trace = make_trace_stats();
    let cycle = nemesis_types::forge::LearningCycle {
        id: "lc-2".to_string(),
        started_at: chrono::Local::now().to_rfc3339(),
        completed_at: None,
        patterns_found: 2,
        actions_taken: 1,
        status: nemesis_types::forge::CycleStatus::Running,
    };
    let result =
        semantic_analysis(&caller, &stats, &[], Some(&trace), Some(&cycle), Some(1000)).await;
    assert!(result.is_ok());
}

#[test]
fn test_parse_insights() {
    let response = "Here are some insights:\n- First insight\n- Second insight\n* Third insight\n• Fourth insight\nSome other text";
    let insights = parse_insights(response);
    assert_eq!(insights.len(), 4);
    assert_eq!(insights[0], "First insight");
    assert_eq!(insights[3], "Fourth insight");
}

#[test]
fn test_parse_insights_empty() {
    let response = "No insights here.\nJust regular text.";
    let insights = parse_insights(response);
    assert!(insights.is_empty());
}

#[test]
fn test_extract_json_found() {
    let response = "Here is the JSON: {\"key\": \"value\"} and some more text";
    let json = extract_json(response).unwrap();
    assert_eq!(json["key"], "value");
}

#[test]
fn test_extract_json_not_found() {
    let response = "No JSON here";
    assert!(extract_json(response).is_none());
}

#[test]
fn test_extract_json_nested() {
    let response = r#"Result: {"a": 1, "b": {"c": 2}} end"#;
    let json = extract_json(response).unwrap();
    assert_eq!(json["a"], 1);
    assert_eq!(json["b"]["c"], 2);
}

#[test]
fn test_build_analysis_prompt_empty_stats() {
    let stats = ExperienceStats {
        total_count: 0,
        success_count: 0,
        failure_count: 0,
        avg_duration_ms: 0.0,
        tool_counts: HashMap::new(),
    };
    let prompt = build_analysis_prompt(&stats, 0);
    assert!(prompt.contains("0"));
    assert!(prompt.contains("0.0%"));
}

#[test]
fn test_parse_insights_skip_empty() {
    let response = "- \n- valid insight\n- ";
    let insights = parse_insights(response);
    assert_eq!(insights.len(), 1);
    assert_eq!(insights[0], "valid insight");
}

#[test]
fn test_parse_insights_mixed_prefixes() {
    let response = "- dash insight\n* star insight\n- bullet insight";
    let insights = parse_insights(response);
    assert_eq!(insights.len(), 3);
}

#[test]
fn test_extract_json_invalid_json() {
    let response = "{not valid json}";
    assert!(extract_json(response).is_none());
}

#[test]
fn test_extract_json_array_not_supported() {
    let response = "[1, 2, 3]";
    assert!(extract_json(response).is_none());
}

#[test]
fn test_build_analysis_prompt_multiple_tools() {
    let mut tool_counts = HashMap::new();
    tool_counts.insert(
        "file_read".into(),
        crate::types::ToolStats {
            count: 30,
            success_count: 28,
            avg_duration_ms: 50.0,
        },
    );
    tool_counts.insert(
        "file_write".into(),
        crate::types::ToolStats {
            count: 20,
            success_count: 18,
            avg_duration_ms: 80.0,
        },
    );
    tool_counts.insert(
        "exec".into(),
        crate::types::ToolStats {
            count: 10,
            success_count: 5,
            avg_duration_ms: 200.0,
        },
    );
    let stats = ExperienceStats {
        total_count: 60,
        success_count: 51,
        failure_count: 9,
        avg_duration_ms: 100.0,
        tool_counts,
    };
    let prompt = build_analysis_prompt(&stats, 3);
    assert!(prompt.contains("file_read"));
    assert!(prompt.contains("file_write"));
    assert!(prompt.contains("exec"));
    assert!(prompt.contains("60"));
}

#[test]
fn test_build_full_analysis_prompt_empty_artifacts() {
    let stats = make_reflection_stats();
    let prompt = build_full_analysis_prompt(&stats, &[], None, None);
    assert!(prompt.contains("现有 Forge 产物"));
}

#[test]
fn test_build_full_analysis_prompt_no_low_success() {
    let stats = ReflectionStats {
        total_records: 100,
        unique_patterns: 5,
        avg_success_rate: 0.99,
        top_patterns: vec![],
        low_success: vec![],
        tool_frequency: HashMap::new(),
    };
    let prompt = build_full_analysis_prompt(&stats, &[], None, None);
    // Should still contain the section header but no entries
    assert!(prompt.contains("100"));
}

#[test]
fn test_parse_insights_single_dash() {
    let insights = parse_insights("- only one insight");
    assert_eq!(insights.len(), 1);
    assert_eq!(insights[0], "only one insight");
}

#[test]
fn test_parse_insights_unicode_content() {
    let insights = parse_insights("- Unicode test content");
    assert_eq!(insights.len(), 1);
}

#[test]
fn test_extract_json_multiple_braces() {
    // extract_json extracts from first { to last } and tries to parse.
    // With two separate JSON objects, the combined string is invalid,
    // so extract_json returns None.
    let response = r#"first {"a":1} second {"b":2}"#;
    assert!(extract_json(response).is_none());

    // A single valid JSON object should still work
    let response2 = r#"text before {"a":1} text after"#;
    let json = extract_json(response2).unwrap();
    assert_eq!(json["a"], 1);
}

#[test]
fn test_build_full_analysis_prompt_trace_signals() {
    let stats = make_reflection_stats();
    let mut trace = TraceStats::default();
    trace.total_traces = 10;
    trace.signal_summary.insert("retry".to_string(), 5);
    trace.signal_summary.insert("backtrack".to_string(), 2);
    let prompt = build_full_analysis_prompt(&stats, &[], Some(&trace), None);
    assert!(prompt.contains("retry"));
    assert!(prompt.contains("backtrack"));
}

#[test]
fn test_build_full_analysis_prompt_caps_top_patterns_at_five() {
    // Only the first 5 top patterns are included in the prompt.
    let mut stats = make_reflection_stats();
    stats.top_patterns = (1..=7)
        .map(|i| PatternInsight {
            tool_name: format!("tp{}", i),
            count: 100 - i,
            avg_duration_ms: 100,
            success_rate: 0.9,
            suggestion: format!("suggestion {}", i),
        })
        .collect();

    let prompt = build_full_analysis_prompt(&stats, &[], None, None);
    assert!(prompt.contains("tp5"), "5th pattern must be included");
    assert!(!prompt.contains("tp6"), "6th pattern must be excluded");
    assert!(!prompt.contains("tp7"), "7th pattern must be excluded");
}

#[test]
fn test_extract_json_end_before_start_returns_none() {
    // '}' occurs before '{' so the slice bounds are inverted -> None.
    assert!(extract_json("}{").is_none());
    assert!(extract_json("no braces at all").is_none());
}

// =========================================================================
// auto_skill 蒸馏（2026-10-15 W2）：digest 构造 / 响应解析 / 蒸馏入口
// =========================================================================

fn make_distill_experience(
    tool: &str,
    success: bool,
    input: &str,
    output: &str,
) -> CollectedExperience {
    CollectedExperience {
        dedup_hash: format!("dh-{tool}"),
        experience: crate::types::Experience {
            id: format!("exp-{tool}"),
            tool_name: tool.to_string(),
            input_summary: input.to_string(),
            output_summary: output.to_string(),
            success,
            duration_ms: 42,
            timestamp: "2026-10-15T00:00:00+08:00".to_string(),
            session_key: "sess:1".to_string(),
        },
    }
}

/// 返回固定合法蒸馏 JSON 的 mock（满足 parse_distill_response 契约）。
struct DistillOkMockLLM;

#[async_trait]
impl LLMCaller for DistillOkMockLLM {
    async fn chat(
        &self,
        _system_prompt: &str,
        _user_prompt: &str,
        _max_tokens: Option<i64>,
    ) -> Result<String, String> {
        Ok(
            r#"{"has_skill": true, "name": "distill-entry", "skill_md": "---\nname: distill-entry\n---\nsteps", "rationale": "r"}"#
                .to_string(),
        )
    }
}

#[test]
fn test_parse_distill_response_valid() {
    let raw = r#"{"has_skill": true, "name": "install-http-mcp", "skill_md": "---\nname: install-http-mcp\n---\n1. discover\n2. add", "rationale": "收敛序列"}"#;
    let d = parse_distill_response(raw).expect("valid response must parse");
    assert_eq!(d.name, "install-http-mcp");
    assert!(d.skill_md.starts_with("---\n"));
    assert_eq!(d.rationale, "收敛序列");
}

#[test]
fn test_parse_distill_response_has_skill_false() {
    let raw = r#"{"has_skill": false, "name": "", "skill_md": ""}"#;
    assert!(parse_distill_response(raw).is_none());
}

#[test]
fn test_parse_distill_response_missing_frontmatter() {
    let raw = r#"{"has_skill": true, "name": "x", "skill_md": "no frontmatter body"}"#;
    assert!(parse_distill_response(raw).is_none());
}

#[test]
fn test_parse_distill_response_empty_fields() {
    let raw = r#"{"has_skill": true, "name": "", "skill_md": "---\nx"}"#;
    assert!(parse_distill_response(raw).is_none());
    let raw2 = r#"{"has_skill": true, "name": "x", "skill_md": "  "}"#;
    assert!(parse_distill_response(raw2).is_none());
}

#[test]
fn test_parse_distill_response_malformed() {
    assert!(parse_distill_response("not json at all").is_none());
    // has_skill 类型错（字符串非布尔）→ 不猜不救
    assert!(
        parse_distill_response(r#"{"has_skill": "yes", "name": "x", "skill_md": "---"}"#).is_none()
    );
}

#[test]
fn test_parse_distill_response_rationale_optional() {
    let raw = r#"{"has_skill": true, "name": "n1", "skill_md": "---\nbody"}"#;
    let d = parse_distill_response(raw).unwrap();
    assert_eq!(d.rationale, "");
    assert_eq!(d.name, "n1");
}

#[test]
fn test_parse_distill_response_wrapped_in_prose() {
    // thinking 系模型偶尔带前后缀文本——extract_json 兜底提取
    let raw = r#"好的：{"has_skill": true, "name": "wrap", "skill_md": "---\nbody"} 以上"#;
    let d = parse_distill_response(raw).unwrap();
    assert_eq!(d.name, "wrap");
}

#[test]
fn test_build_experience_digest_row_format() {
    let exps = vec![make_distill_experience(
        "exec",
        true,
        "run pet.exe",
        "started",
    )];
    let digest = build_experience_digest(&exps);
    assert!(
        digest.contains("#0 [exec] OK 42ms session=sess:1 input=run pet.exe output=started"),
        "row format mismatch; got: {digest}"
    );
    let exps2 = vec![make_distill_experience("mcp_add", false, "in", "err")];
    assert!(
        build_experience_digest(&exps2).contains("#0 [mcp_add] FAIL 42ms"),
        "failed rows must be marked FAIL"
    );
}

#[test]
fn test_build_experience_digest_sanitizes_secrets() {
    // data_1 实录形态：MCP 安装经验里带裸 hex token——第二道脱敏必须拦住
    let exps = vec![make_distill_experience(
        "mcp_add",
        true,
        r#"header "Authorization: Bearer 86c3d6290123456789abcdef0123456789""#,
        "added",
    )];
    let digest = build_experience_digest(&exps);
    assert!(
        digest.contains("[REDACTED]"),
        "hex secret must be redacted; got: {digest}"
    );
    assert!(
        !digest.contains("86c3d629"),
        "raw secret must not survive into the prompt"
    );
}

#[test]
fn test_build_experience_digest_truncates_long_cells() {
    let long = "x".repeat(300);
    let exps = vec![make_distill_experience("write_file", true, &long, "ok")];
    let digest = build_experience_digest(&exps);
    assert!(
        digest.contains("…[truncated]"),
        "oversized cell must be truncated"
    );
    assert!(
        !digest.contains(&"x".repeat(221)),
        "cell must be cut at 220 chars"
    );
}

#[test]
fn test_build_experience_digest_rows_window() {
    // >120 条时只取最近 MAX_DISTILL_ROWS 条，行号保留全局序号
    let exps: Vec<_> = (0..130)
        .map(|i| make_distill_experience(&format!("t{i}"), true, "i", "o"))
        .collect();
    let digest = build_experience_digest(&exps);
    assert!(digest.contains("#10 [t10]"), "oldest kept row is #10");
    assert!(digest.contains("#129 [t129]"));
    assert!(
        !digest.contains("#0 [t0]"),
        "rows beyond the window must be dropped"
    );
}

#[tokio::test]
async fn test_distill_skill_draft_success() {
    let exps = vec![make_distill_experience("exec", true, "i", "o")];
    let d = distill_skill_draft(&DistillOkMockLLM, &exps, &[], Some(8192))
        .await
        .unwrap()
        .expect("valid contract must yield a draft");
    assert_eq!(d.name, "distill-entry");
}

#[tokio::test]
async fn test_distill_skill_draft_empty_experiences_no_llm_call() {
    // ErrorMockLLMCaller：若实现真的发起调用就会 Err——Ok(None) 证明空序列在调用前短路
    let r = distill_skill_draft(&ErrorMockLLMCaller, &[], &[], None)
        .await
        .unwrap();
    assert!(r.is_none());
}

#[tokio::test]
async fn test_distill_skill_draft_llm_error_is_err() {
    let exps = vec![make_distill_experience("exec", true, "i", "o")];
    let r = distill_skill_draft(&ErrorMockLLMCaller, &exps, &[], None).await;
    assert!(r.is_err(), "call-face failure must surface as Err");
}

#[tokio::test]
async fn test_distill_skill_draft_contract_violation_is_none() {
    // has_skill=true 但 skill_md 无 frontmatter → 按无可蒸馏处理（Ok(None)）
    struct BadMockLLM;
    #[async_trait]
    impl LLMCaller for BadMockLLM {
        async fn chat(
            &self,
            _system_prompt: &str,
            _user_prompt: &str,
            _max_tokens: Option<i64>,
        ) -> Result<String, String> {
            Ok(
                r#"{"has_skill": true, "name": "x", "skill_md": "plain text no frontmatter"}"#
                    .to_string(),
            )
        }
    }
    let exps = vec![make_distill_experience("exec", true, "i", "o")];
    let r = distill_skill_draft(&BadMockLLM, &exps, &[], None)
        .await
        .unwrap();
    assert!(r.is_none(), "contract violation must degrade to no-draft");
}

#[test]
fn test_parse_dimension_score_clamps_and_rejects() {
    use super::parse_dimension_score;
    // 越界钳位（R18：u32 溢出/溢出分直接进加权会 panic 或回绕）。
    assert_eq!(parse_dimension_score(&serde_json::json!(1000)), Some(100));
    assert_eq!(parse_dimension_score(&serde_json::json!(-5)), Some(0));
    assert_eq!(parse_dimension_score(&serde_json::json!(87.6)), Some(87));
    // f64 形态 + 边界。
    assert_eq!(parse_dimension_score(&serde_json::json!(100.0)), Some(100));
    assert_eq!(parse_dimension_score(&serde_json::json!(0.0)), Some(0));
    // 非数值 / 非有限 = 拒绝（None），调用方 fail-open 到该维缺省。
    assert_eq!(parse_dimension_score(&serde_json::json!("92")), None);
    assert_eq!(parse_dimension_score(&serde_json::json!(null)), None);
    let nan = serde_json::from_str::<serde_json::Value>("{\"x\": NaN}")
        .ok()
        .and_then(|v| parse_dimension_score(v.get("x").unwrap()));
    assert_eq!(nan, None, "NaN must be rejected, not clamped");
}
