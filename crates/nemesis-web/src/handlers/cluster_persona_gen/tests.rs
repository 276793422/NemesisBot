use super::*;

#[test]
fn sanitize_rejects_short_and_strips_controls() {
    assert!(sanitize_input("太短").is_err());
    let r = sanitize_input(
            "这是一段足够长的有效岗位描述文本，用于通过最小长度校验门槛，这里再多写一些内容确保超过四十个字符 abcdefgh",
        )
        .unwrap();
    assert!(!r.contains('\u{7f}'));
    assert!(!r.contains('\r'));
}

#[test]
fn validate_enforces_role_enum() {
    let mut pkg = PersonaPackage {
        node_name: "x".into(),
        display_name: "X".into(),
        emoji: "🤖".into(),
        role: "admin".into(),
        category: "dev".into(),
        tags: vec![" a ".into()],
        identity_md: "# X\nwho".into(),
        soul_md: "# Rules\n- a".into(),
        expertise_md: String::new(),
        coverage: None,
    };
    assert!(validate(&mut pkg).is_err());
    pkg.role = "worker".into();
    assert!(validate(&mut pkg).is_ok());
    assert_eq!(pkg.tags, vec!["a".to_string()]);
}

#[test]
fn unwrap_single_key_handles_wrapped_args() {
    let wrapped =
        serde_json::json!({ "emit_cluster_persona": { "identity_md": "# x", "soul_md": "# y" } });
    let v = unwrap_single_key(wrapped);
    assert!(v.get("identity_md").is_some());
}

#[test]
fn extract_json_span_and_fence() {
    assert_eq!(extract_json_span("noise {\"a\":1} tail"), Some("{\"a\":1}"));
    let stripped = strip_code_fence("```json\n{\"a\":1}\n```");
    assert_eq!(stripped, "{\"a\":1}");
}

// ---- 程序确定性校验（机制硬骨架）单测 ----

fn sample_unit(id: &str, disposition: &str, entities: &[&str]) -> InformationUnit {
    InformationUnit {
        id: id.into(),
        content: format!("unit {id}"),
        unit_type: "tech_decision".into(),
        relevance: "high".into(),
        disposition: disposition.into(),
        drop_reason: None,
        key_entities: entities.iter().map(|s| s.to_string()).collect(),
    }
}

fn sample_pkg(identity: &str, soul: &str, expertise: &str) -> PersonaPackage {
    PersonaPackage {
        node_name: "x".into(),
        display_name: "X".into(),
        emoji: "🤖".into(),
        role: "worker".into(),
        category: "dev".into(),
        tags: vec![],
        identity_md: identity.into(),
        soul_md: soul.into(),
        expertise_md: expertise.into(),
        coverage: None,
    }
}

#[test]
fn entity_coverage_covered_when_all_entities_present() {
    let units = vec![sample_unit("u1", "expertise", &["RocketMQ", "事务消息"])];
    let pkg = sample_pkg("", "", "用 RocketMQ 事务消息做分布式事务");
    let entries = check_entity_coverage(&units, &pkg);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].status, CoverageStatus::Covered);
}

#[test]
fn entity_coverage_missing_when_entity_absent() {
    let units = vec![sample_unit("u1", "soul", &["分库分表"])];
    let pkg = sample_pkg("", "没有提到相关内容", "");
    let entries = check_entity_coverage(&units, &pkg);
    assert_eq!(entries[0].status, CoverageStatus::Missing);
    assert!(entries[0].reason.as_ref().unwrap().contains("分库分表"));
}

#[test]
fn entity_coverage_case_insensitive() {
    let units = vec![sample_unit("u1", "identity", &["kafka"])];
    let pkg = sample_pkg("我用 Kafka 做过消息系统", "", "");
    let entries = check_entity_coverage(&units, &pkg);
    assert_eq!(entries[0].status, CoverageStatus::Covered);
}

#[test]
fn segment_gap_reported_when_unit_count_zero() {
    let units = InformationUnits {
        units: vec![],
        segments: vec![
            InputSegment {
                id: "s1".into(),
                label: "技能".into(),
                unit_count: 3,
            },
            InputSegment {
                id: "s2".into(),
                label: "教育".into(),
                unit_count: 0,
            },
        ],
    };
    let gaps = check_segment_coverage(&units);
    assert_eq!(gaps.len(), 1);
    assert!(gaps[0].contains("教育"));
}

#[test]
fn report_program_missing_wins_over_audit() {
    // 程序判 Missing（实体没出现）→ 报告 Missing，即使审计说 covered 也不信。
    let units = vec![sample_unit("u1", "expertise", &["Redis"])];
    let prog = vec![CoverageEntry {
        unit_id: "u1".into(),
        status: CoverageStatus::Missing,
        location: Some("expertise".into()),
        reason: Some("缺 Redis".into()),
    }];
    let audit = vec![CoverageEntry {
        unit_id: "u1".into(),
        status: CoverageStatus::Covered,
        location: Some("expertise".into()),
        reason: None,
    }];
    let report = build_coverage_report(&units, prog, audit, vec![]);
    assert_eq!(report.missing, 1);
    assert!(!report.is_complete());
}

#[test]
fn report_covered_but_audit_suspect_becomes_suspect_non_blocking() {
    // 程序 Covered（词在）+ 审计 Suspect（意思存疑）→ Suspect；suspect 不阻断完整性。
    let units = vec![sample_unit("u1", "expertise", &["Redis"])];
    let prog = vec![CoverageEntry {
        unit_id: "u1".into(),
        status: CoverageStatus::Covered,
        location: Some("expertise".into()),
        reason: None,
    }];
    let audit = vec![CoverageEntry {
        unit_id: "u1".into(),
        status: CoverageStatus::Suspect,
        location: None,
        reason: Some("词在但意思没到位".into()),
    }];
    let report = build_coverage_report(&units, prog, audit, vec![]);
    assert_eq!(report.covered, 0);
    assert_eq!(report.suspect, 1);
    assert!(report.is_complete());
}

#[test]
fn report_complete_when_no_missing_and_no_gaps() {
    let units = vec![sample_unit("u1", "expertise", &["Redis"])];
    let prog = vec![CoverageEntry {
        unit_id: "u1".into(),
        status: CoverageStatus::Covered,
        location: Some("expertise".into()),
        reason: None,
    }];
    let report = build_coverage_report(&units, prog, vec![], vec![]);
    assert_eq!(report.coverage_rate, 1.0);
    assert!(report.is_complete());
}

#[test]
fn report_skipped_counts_archive_and_drop() {
    let units = vec![
        sample_unit("u1", "expertise", &["Redis"]),
        InformationUnit {
            id: "u2".into(),
            content: "无关信息".into(),
            unit_type: "skill".into(),
            relevance: "none".into(),
            disposition: "drop".into(),
            drop_reason: Some("与人格无关".into()),
            key_entities: vec![],
        },
    ];
    let prog = vec![CoverageEntry {
        unit_id: "u1".into(),
        status: CoverageStatus::Covered,
        location: Some("expertise".into()),
        reason: None,
    }];
    let report = build_coverage_report(&units, prog, vec![], vec![]);
    assert_eq!(report.total, 2);
    assert_eq!(report.skipped, 1);
    assert_eq!(report.covered, 1);
}

// ===========================================================================
// P3-web3（2026-08-25）：schema/prompt/提取边臂 + generate_persona 三阶段流。
//
// LLM 后端用 wiremock 假 OpenAI 兼容服务，按 system prompt 的阶段特征串区分
// 三个阶段的请求（“分析师”=阶段1、“人格设计师”=阶段2、“完整性审计员”=
// 阶段3）；重试轮的 author 请求带“把它们补进”补全提示（覆盖缺口 / 校验失败
// / 解析失败三类重试共用同一模板），可用更具体的 matcher + with_priority(1)
// 区分首轮与重试轮，全程进程内无真实 LLM 依赖。
// ===========================================================================

use nemesis_providers::http_provider::HttpProviderConfig;
use nemesis_providers::types::{FunctionCall, ToolCall};

const JD_TEXT: &str = "这是一段用于测试的足够长的岗位描述文本，描述一个基于消息队列的后端架构师岗位，要求熟悉 RocketMQ 事务消息与分布式事务一致性方案，超过四十个字符。";

/// 构造 OpenAI 兼容非流式 chat completion 响应：tool_call arguments 为 JSON
/// 字符串（HttpProvider 的标准解析路径）。
fn llm_tool_args_response(args: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "id": "call-1",
                    "type": "function",
                    "function": { "name": "stage_tool", "arguments": args.to_string() }
                }]
            },
            "finish_reason": "tool_calls"
        }]
    })
}

fn full_pkg() -> PersonaPackage {
    let mut p = sample_pkg("# I\n定位", "# S\n哲学", "# E\n方案");
    p.node_name = "n".into();
    p.display_name = "N".into();
    p.role = "worker".into();
    p
}

#[test]
fn sanitize_truncates_over_long_input() {
    // MAX_INPUT_CHARS=20000：超长输入截断而非报错。
    let long: String = "字".repeat(20_050);
    let out = sanitize_input(&long).unwrap();
    assert_eq!(out.chars().count(), 20_000);
}

#[test]
fn validate_rejects_empty_required_fields_and_defaults_emoji() {
    let mut p = full_pkg();
    p.node_name = "  ".into();
    assert!(validate(&mut p).is_err());
    let mut p = full_pkg();
    p.display_name = "".into();
    assert!(validate(&mut p).is_err());
    let mut p = full_pkg();
    p.identity_md = " \n".into();
    assert!(validate(&mut p).is_err());
    let mut p = full_pkg();
    p.soul_md = "".into();
    assert!(validate(&mut p).is_err());
    // emoji 空 → 默认 🤖
    let mut p = full_pkg();
    p.emoji = "".into();
    assert!(validate(&mut p).is_ok());
    assert_eq!(p.emoji, "🤖");
    // emoji 超长 → 截到 4 个 char
    let mut p = full_pkg();
    p.emoji = "🚀🔥💧⚡🌈✨".into();
    assert!(validate(&mut p).is_ok());
    assert_eq!(p.emoji.chars().count(), 4);
}

#[test]
fn tool_schemas_and_defs_are_wellformed() {
    let s = persona_tool_schema();
    assert_eq!(s["type"], "object");
    assert_eq!(s["properties"]["identity_md"]["type"], "string");
    assert_eq!(s["required"].as_array().unwrap().len(), 9);

    let u = units_tool_schema();
    assert_eq!(u["properties"]["units"]["type"], "array");
    assert!(u["required"].as_array().unwrap().contains(&serde_json::json!("units")));

    let a = audit_tool_schema();
    assert!(a["required"].as_array().unwrap().contains(&serde_json::json!("entries")));

    assert_eq!(persona_tool_def().function.name, "emit_cluster_persona");
    assert_eq!(units_tool_def().function.name, "extract_information_units");
    assert_eq!(audit_tool_def().function.name, "audit_coverage");
    assert!(!persona_tool_def().function.description.is_empty());
}

#[test]
fn prompts_switch_by_kind_and_hint() {
    assert!(extract_prompt("resume").contains("简历"));
    assert!(extract_prompt("jd").contains("JD"));
    // jd 不走 resume 分支的措辞
    assert!(!extract_prompt("jd").contains("用户给你一份简历"));

    let with_hint = author_prompt("jd", Some("补上 RocketMQ"));
    assert!(with_hint.contains("必须】把它们补进"));
    assert!(with_hint.contains("补上 RocketMQ"));
    let resume_no_hint = author_prompt("resume", None);
    assert!(resume_no_hint.contains("简历"));
    assert!(!resume_no_hint.contains("把它们补进"));

    assert!(audit_prompt().contains("完整性审计员"));

    let m = mk_msg("system", "hello".to_string());
    assert_eq!(m.role, "system");
    assert_eq!(m.content, "hello");
    assert!(m.tool_calls.is_empty());
}

#[test]
fn unwrap_single_key_only_for_known_shapes() {
    let v = unwrap_single_key(serde_json::json!({
        "emit_cluster_persona": { "identity_md": "x", "soul_md": "y" }
    }));
    assert_eq!(v["identity_md"], "x");

    let v = unwrap_single_key(serde_json::json!({
        "extract_information_units": { "units": [1] }
    }));
    assert!(v["units"].is_array());

    let v = unwrap_single_key(serde_json::json!({
        "audit_coverage": { "entries": [] }
    }));
    assert!(v["entries"].is_array());

    // 单 key 但内层不认识 → 原样返回
    let v = unwrap_single_key(serde_json::json!({ "unknown": { "foo": 1 } }));
    assert!(v.get("unknown").is_some());
    // 多 key → 原样返回
    let v = unwrap_single_key(serde_json::json!({ "a": 1, "b": { "units": [] } }));
    assert!(v.get("a").is_some() && v.get("b").is_some());
    // 非对象 → 原样返回
    let v = unwrap_single_key(serde_json::json!([1, 2]));
    assert!(v.is_array());
}

#[test]
fn json_span_and_fence_edge_cases() {
    assert_eq!(extract_json_span("no braces here"), None);
    assert_eq!(extract_json_span(""), None);
    // fence 无换行 → 原样返回
    assert_eq!(strip_code_fence("```json"), "```json");
    // fence 有内容但没有闭合标记
    assert_eq!(strip_code_fence("```\n{\"a\":1}"), "{\"a\":1}");
    // 标准闭合 fence
    assert_eq!(strip_code_fence("```json\n{\"a\":1}\n```"), "{\"a\":1}");
    // 无 fence 原样 trim
    assert_eq!(strip_code_fence("  {\"a\":1}  "), "{\"a\":1}");
}

fn resp_with(content: &str, tool_calls: Vec<ToolCall>) -> LLMResponse {
    LLMResponse {
        content: content.to_string(),
        tool_calls,
        finish_reason: "stop".to_string(),
        usage: None,
        reasoning_content: None,
        extra: std::collections::HashMap::new(),
        raw_request_body: None,
        raw_response_body: None,
    }
}

#[test]
fn extract_response_json_paths() {
    // 1) tool_call.function.arguments JSON 字符串（含 emit 包裹解包）
    let r = resp_with(
        "",
        vec![ToolCall {
            id: "1".into(),
            call_type: Some("function".into()),
            function: Some(FunctionCall {
                name: "t".into(),
                arguments: "{\"emit_cluster_persona\":{\"identity_md\":\"x\",\"soul_md\":\"y\"}}"
                    .into(),
            }),
            name: None,
            arguments: None,
        }],
    );
    let v = extract_response_json(&r).unwrap();
    assert_eq!(v["identity_md"], "x");

    // 2) arguments 字符串为空白 → 退化到 tool_call.arguments map
    let mut map = std::collections::HashMap::new();
    map.insert("units".to_string(), serde_json::json!([]));
    let r = resp_with(
        "",
        vec![ToolCall {
            id: "1".into(),
            call_type: None,
            function: Some(FunctionCall { name: "t".into(), arguments: "   ".into() }),
            name: None,
            arguments: Some(map),
        }],
    );
    let v = extract_response_json(&r).unwrap();
    assert!(v["units"].is_array());

    // 3) content 带 fence 的 JSON
    let r = resp_with("```json\n{\"audit_coverage\":{\"entries\": []}}\n```", vec![]);
    let v = extract_response_json(&r).unwrap();
    assert!(v["entries"].is_array());

    // 4) content 纯文本夹 JSON（span 提取）
    let r = resp_with("前缀说明 {\"a\": 5} 后缀噪音", vec![]);
    let v = extract_response_json(&r).unwrap();
    assert_eq!(v["a"], 5);

    // 5) 哪里都没有 JSON → Err
    let r = resp_with("这只是一段纯文本回复，没有任何结构化数据", vec![]);
    assert!(extract_response_json(&r).is_err());
}

#[test]
fn entity_coverage_skips_archive_drop_and_empty_entities() {
    // archive/drop 不查字面；空 key_entities 跳过 → 无任何条目。
    let units = vec![
        sample_unit("u1", "archive", &[]),
        sample_unit("u2", "identity", &[]),
        sample_unit("u3", "drop", &["Redis"]),
    ];
    let pkg = sample_pkg("", "", "");
    let entries = check_entity_coverage(&units, &pkg);
    assert!(entries.is_empty());
}

#[test]
fn report_audit_covered_stays_covered() {
    let units = vec![sample_unit("u1", "expertise", &["Redis"])];
    let prog = vec![CoverageEntry {
        unit_id: "u1".into(),
        status: CoverageStatus::Covered,
        location: Some("expertise".into()),
        reason: None,
    }];
    let audit = vec![CoverageEntry {
        unit_id: "u1".into(),
        status: CoverageStatus::Covered,
        location: Some("expertise".into()),
        reason: None,
    }];
    let report = build_coverage_report(&units, prog, audit, vec![]);
    assert_eq!(report.covered, 1);
    assert_eq!(report.coverage_rate, 1.0);
    assert!(report.is_complete());
}

fn no_entity_unit(id: &str, disposition: &str) -> InformationUnit {
    InformationUnit {
        id: id.into(),
        content: format!("unit {id}"),
        unit_type: "skill".into(),
        relevance: "high".into(),
        disposition: disposition.into(),
        drop_reason: None,
        key_entities: vec![],
    }
}

#[test]
fn report_trusts_audit_when_program_has_no_entry() {
    // 无 key_entities 的 target unit：程序不产出条目 → 信审计；审计也没有则
    // Suspect（两臂一次覆盖）。
    let units = vec![no_entity_unit("u1", "soul"), no_entity_unit("u2", "identity")];
    let audit = vec![CoverageEntry {
        unit_id: "u1".into(),
        status: CoverageStatus::Covered,
        location: None,
        reason: None,
    }];
    let report = build_coverage_report(&units, vec![], audit, vec![]);
    assert_eq!(report.covered, 1);
    assert_eq!(report.suspect, 1);
}

#[test]
fn report_audit_skipped_entry_counts_nothing() {
    // 审计给出 skipped → Skipped 状态不进任何计数。
    let units = vec![no_entity_unit("u1", "identity")];
    let audit = vec![CoverageEntry {
        unit_id: "u1".into(),
        status: CoverageStatus::Skipped,
        location: None,
        reason: None,
    }];
    let report = build_coverage_report(&units, vec![], audit, vec![]);
    assert_eq!(report.covered, 0);
    assert_eq!(report.missing, 0);
    assert_eq!(report.suspect, 0);
    assert_eq!(report.entries[0].status, CoverageStatus::Skipped);
}

#[test]
fn report_rate_is_one_when_no_target_units() {
    // 全部 archive/drop → target_count=0 → 覆盖率约定为 1.0。
    let units = vec![sample_unit("u1", "drop", &[]), sample_unit("u2", "archive", &[])];
    let report = build_coverage_report(&units, vec![], vec![], vec![]);
    assert_eq!(report.coverage_rate, 1.0);
    assert_eq!(report.total, 2);
    assert_eq!(report.skipped, 2);
}

// ---------------------------------------------------------------------------
// generate_persona 三阶段流（wiremock 假 LLM）
// ---------------------------------------------------------------------------

use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn mock_provider_at(uri: String) -> Arc<HttpProvider> {
    Arc::new(HttpProvider::new(HttpProviderConfig {
        name: "mock".into(),
        base_url: uri,
        api_key: "test-key".into(),
        default_model: "test-model".into(),
        timeout_secs: 15,
        headers: std::collections::HashMap::new(),
        proxy: None,
        preserve_prefix: false,
    }))
}

/// 挂一个阶段 mock：按请求体里的阶段特征串匹配；`priority=1` 供重试轮的
/// 更具体 mock 抢在通用阶段 mock 前面。
async fn mount_stage_mock(
    server: &MockServer,
    body_marker: &str,
    args: serde_json::Value,
    status: u16,
    hits: u64,
    priority: u8,
) {
    let template = if status == 200 {
        ResponseTemplate::new(200).set_body_json(llm_tool_args_response(&args))
    } else {
        ResponseTemplate::new(status)
    };
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(body_string_contains(body_marker.to_string()))
        .respond_with(template)
        .expect(hits)
        .with_priority(priority)
        .mount(server)
        .await;
}

fn units_args(u1_disposition: &str, with_segment_gap: bool) -> serde_json::Value {
    let segments = if with_segment_gap {
        serde_json::json!([
            { "id": "s1", "label": "项目", "unit_count": 2 },
            { "id": "s2", "label": "兴趣爱好", "unit_count": 0 },
        ])
    } else {
        serde_json::json!([{ "id": "s1", "label": "项目", "unit_count": 2 }])
    };
    serde_json::json!({
        "units": [
            {
                "id": "u1",
                "content": "RocketMQ 事务消息方案",
                "unit_type": "tech_decision",
                "relevance": "high",
                "disposition": u1_disposition,
                "key_entities": ["RocketMQ"],
            },
            {
                "id": "u2",
                "content": "与人格无关的信息",
                "unit_type": "skill",
                "relevance": "none",
                "disposition": "drop",
                "drop_reason": "与人格无关",
                "key_entities": [],
            },
        ],
        "segments": segments,
    })
}

fn persona_args(include_entity: bool, valid_role: bool, with_expertise: bool) -> serde_json::Value {
    serde_json::json!({
        "node_name": "mq-architect",
        "display_name": "MQ 架构师",
        "emoji": "🚀",
        "role": if valid_role { "worker" } else { "admin" },
        "category": "development",
        "tags": ["RocketMQ"],
        "identity_md": if include_entity {
            "# 定位\n熟悉 RocketMQ 事务消息的后端架构师"
        } else {
            "# 定位\n一个通用后端架构师"
        },
        "soul_md": "# 工作哲学\n以消息可靠性为锚点",
        "expertise_md": if with_expertise {
            "# RocketMQ 方案\nRocketMQ 事务消息半消息机制"
        } else {
            ""
        },
    })
}

fn audit_args() -> serde_json::Value {
    serde_json::json!({
        "entries": [
            { "unit_id": "u1", "status": "covered", "location": "identity/定位" },
        ]
    })
}

#[tokio::test]
async fn generate_persona_full_flow_complete() {
    let server = MockServer::start().await;
    mount_stage_mock(&server, "分析师", units_args("identity", false), 200, 1, 5).await;
    mount_stage_mock(&server, "人格设计师", persona_args(true, true, false), 200, 1, 5).await;
    mount_stage_mock(&server, "完整性审计员", audit_args(), 200, 1, 5).await;
    let provider = mock_provider_at(server.uri()).await;

    let pkg = generate_persona(&provider, "test-model", "jd", JD_TEXT, 2).await.unwrap();
    assert_eq!(pkg.node_name, "mq-architect");
    assert_eq!(pkg.role, "worker");
    let cov = pkg.coverage.expect("complete flow must attach coverage");
    assert!(cov.is_complete());
    assert_eq!(cov.covered, 1);
    assert_eq!(cov.skipped, 1); // u2 dropped
    assert_eq!(cov.coverage_rate, 1.0);
}

#[tokio::test]
async fn generate_persona_retry_after_entity_missing() {
    // u1 去向 expertise：首轮 expertise_md 为空 → 实体 Missing → 补全提示
    // 重试；第二轮带上实体 → 完整通过。
    let server = MockServer::start().await;
    mount_stage_mock(&server, "分析师", units_args("expertise", false), 200, 1, 5).await;
    // 重试轮（带补全提示，更具体 matcher + 最高优先级）
    mount_stage_mock(&server, "把它们补进", persona_args(true, true, true), 200, 1, 1).await;
    // 首轮（无补全提示；expect(1) 耗尽后不再匹配，重试轮只会落到上面的 mock）
    mount_stage_mock(&server, "人格设计师", persona_args(false, true, false), 200, 1, 5).await;
    mount_stage_mock(&server, "完整性审计员", audit_args(), 200, 2, 5).await;
    let provider = mock_provider_at(server.uri()).await;

    let pkg = generate_persona(&provider, "test-model", "resume", JD_TEXT, 2).await.unwrap();
    assert!(pkg.expertise_md.contains("RocketMQ"));
    let cov = pkg.coverage.unwrap();
    assert!(cov.is_complete());
}

#[tokio::test]
async fn generate_persona_exhausted_returns_pkg_with_report() {
    // 两轮实体都缺失 → 耗尽后不硬 Err，返回带缺口报告的最后一版 pkg。
    let server = MockServer::start().await;
    mount_stage_mock(&server, "分析师", units_args("identity", false), 200, 1, 5).await;
    mount_stage_mock(&server, "人格设计师", persona_args(false, true, false), 200, 2, 5).await;
    mount_stage_mock(&server, "完整性审计员", audit_args(), 200, 2, 5).await;
    let provider = mock_provider_at(server.uri()).await;

    let pkg = generate_persona(&provider, "test-model", "jd", JD_TEXT, 2).await.unwrap();
    let cov = pkg.coverage.expect("exhausted flow still attaches report");
    assert!(!cov.is_complete());
    assert_eq!(cov.missing, 1);
}

#[tokio::test]
async fn generate_persona_all_author_attempts_fail() {
    // 阶段2 全部解析失败 → 从未产出 pkg → 耗尽后硬 Err。
    let server = MockServer::start().await;
    mount_stage_mock(&server, "分析师", units_args("identity", false), 200, 1, 5).await;
    mount_stage_mock(&server, "人格设计师", serde_json::json!({ "garbage": true }), 200, 2, 5).await;
    let provider = mock_provider_at(server.uri()).await;

    let err = generate_persona(&provider, "test-model", "jd", JD_TEXT, 2)
        .await
        .unwrap_err();
    assert!(err.contains("生成失败"), "{err}");
}

#[tokio::test]
async fn generate_persona_validate_fail_then_success() {
    // 首轮 role 非法（validate 失败）；重试轮合法且实体齐全。
    let server = MockServer::start().await;
    mount_stage_mock(&server, "分析师", units_args("identity", false), 200, 1, 5).await;
    mount_stage_mock(&server, "把它们补进", persona_args(true, true, false), 200, 1, 1).await;
    mount_stage_mock(&server, "人格设计师", persona_args(true, false, false), 200, 1, 5).await;
    mount_stage_mock(&server, "完整性审计员", audit_args(), 200, 1, 5).await;
    let provider = mock_provider_at(server.uri()).await;

    let pkg = generate_persona(&provider, "test-model", "jd", JD_TEXT, 2).await.unwrap();
    assert_eq!(pkg.role, "worker");
    assert!(pkg.coverage.unwrap().is_complete());
}

#[tokio::test]
async fn generate_persona_segment_gap_hint_and_exhausted() {
    // 段落缺口（unit_count=0）不阻断重试，但报告带 segment_gaps；missing
    // 为空时补全提示走“段落缺口”分支。
    let server = MockServer::start().await;
    mount_stage_mock(&server, "分析师", units_args("identity", true), 200, 1, 5).await;
    mount_stage_mock(&server, "人格设计师", persona_args(true, true, false), 200, 2, 5).await;
    mount_stage_mock(&server, "完整性审计员", audit_args(), 200, 2, 5).await;
    let provider = mock_provider_at(server.uri()).await;

    let pkg = generate_persona(&provider, "test-model", "jd", JD_TEXT, 2).await.unwrap();
    let cov = pkg.coverage.unwrap();
    assert!(!cov.is_complete());
    assert_eq!(cov.missing, 0);
    assert_eq!(cov.segment_gaps.len(), 1);
    assert!(cov.segment_gaps[0].contains("兴趣爱好"));
}

#[tokio::test]
async fn generate_persona_tolerates_audit_failure() {
    // 阶段3 审计解析失败 → 退化为空审计，程序校验兜底，仍能完整通过。
    let server = MockServer::start().await;
    mount_stage_mock(&server, "分析师", units_args("identity", false), 200, 1, 5).await;
    mount_stage_mock(&server, "人格设计师", persona_args(true, true, false), 200, 1, 5).await;
    mount_stage_mock(&server, "完整性审计员", serde_json::json!({ "nope": 1 }), 200, 1, 5).await;
    let provider = mock_provider_at(server.uri()).await;

    let pkg = generate_persona(&provider, "test-model", "jd", JD_TEXT, 2).await.unwrap();
    let cov = pkg.coverage.unwrap();
    assert!(cov.is_complete());
    assert_eq!(cov.covered, 1);
}

#[tokio::test]
async fn generate_persona_units_parse_fail_errors() {
    let server = MockServer::start().await;
    mount_stage_mock(&server, "分析师", serde_json::json!({ "foo": 1 }), 200, 1, 5).await;
    let provider = mock_provider_at(server.uri()).await;

    let err = generate_persona(&provider, "test-model", "jd", JD_TEXT, 2)
        .await
        .unwrap_err();
    assert!(err.contains("解析信息单元失败"), "{err}");
}

#[tokio::test]
async fn generate_persona_llm_http_error_errors() {
    let server = MockServer::start().await;
    mount_stage_mock(&server, "分析师", serde_json::json!({}), 500, 1, 5).await;
    let provider = mock_provider_at(server.uri()).await;

    let err = generate_persona(&provider, "test-model", "jd", JD_TEXT, 2)
        .await
        .unwrap_err();
    assert!(err.contains("LLM 调用失败"), "{err}");
}

#[tokio::test]
async fn generate_persona_rejects_short_input_without_llm() {
    let server = MockServer::start().await;
    let provider = mock_provider_at(server.uri()).await;

    let err = generate_persona(&provider, "test-model", "jd", "太短", 2)
        .await
        .unwrap_err();
    assert!(err.contains("内容太短"), "{err}");
}
