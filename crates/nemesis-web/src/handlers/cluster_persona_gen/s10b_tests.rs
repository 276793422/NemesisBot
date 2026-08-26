//! S10b (quality-hardening goal 冲刺, web 批次 2): drive the full
//! `generate_persona` three-stage pipeline against a wiremock OpenAI-compatible
//! backend (process-local, no real network) — the existing tests only cover the
//! pure helpers (extract/unwrap/check_*), never the retry loop:
//!
//! - clean run: units → persona → audit → complete report → Ok(pkg)
//! - never-covered entity: retry with missing_hint, then exhausted →
//!   Ok(pkg with incomplete coverage report)
//! - stage-2 parse failure (`continue` arm) followed by a good retry + audit
//!   HTTP failure (program-check fallback arm)
//! - validate failure (`continue` arm) followed by a good retry

use crate::handlers::cluster_persona_gen::generate_persona;
use nemesis_providers::http_provider::{HttpProvider, HttpProviderConfig};
use std::collections::HashMap;
use std::sync::Arc;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// --- wire helpers -----------------------------------------------------------

fn tool_args_response(args: serde_json::Value) -> ResponseTemplate {
    let arguments = serde_json::to_string(&args).unwrap();
    ResponseTemplate::new(200).set_body_json(serde_json::json!({
        "id": "chatcmpl-s10b",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "id": "call-1",
                    "type": "function",
                    "function": { "name": "emit", "arguments": arguments }
                }]
            },
            "finish_reason": "tool_calls"
        }],
        "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
    }))
}

fn text_response(content: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(serde_json::json!({
        "id": "chatcmpl-s10b",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": content },
            "finish_reason": "stop"
        }]
    }))
}

/// The system prompt embedded in each stage's request body carries a unique
/// marker — match on it so mocks bind to stages, not call order.
fn stage_mock(marker: &str, resp: ResponseTemplate) -> Mock {
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(body_string_contains(marker))
        .respond_with(resp)
}

fn units_args(identity_entities: &[&str], soul_entities: &[&str]) -> serde_json::Value {
    let mut units = Vec::new();
    for (i, e) in identity_entities.iter().enumerate() {
        units.push(serde_json::json!({
            "id": format!("ui{}", i),
            "content": "身份单元",
            "unit_type": "tech_decision",
            "relevance": "high",
            "disposition": "identity",
            "key_entities": [e],
        }));
    }
    for (i, e) in soul_entities.iter().enumerate() {
        units.push(serde_json::json!({
            "id": format!("us{}", i),
            "content": "灵魂单元",
            "unit_type": "methodology",
            "relevance": "high",
            "disposition": "soul",
            "key_entities": [e],
        }));
    }
    serde_json::json!({
        "units": units,
        "segments": [
            { "id": "s1", "label": "技能", "unit_count": units.len() },
        ]
    })
}

fn pkg_args(identity_md: &str, soul_md: &str) -> serde_json::Value {
    pkg_args_with_role(identity_md, soul_md, "worker")
}

fn pkg_args_with_role(identity_md: &str, soul_md: &str, role: &str) -> serde_json::Value {
    serde_json::json!({
        "node_name": "node-s10b",
        "display_name": "S10b 节点",
        "emoji": "🤖",
        "role": role,
        "category": "development",
        "tags": ["Go"],
        "identity_md": identity_md,
        "soul_md": soul_md,
    })
}

fn provider_for(server: &MockServer) -> Arc<HttpProvider> {
    Arc::new(HttpProvider::new(HttpProviderConfig {
        name: "s10b-mock".to_string(),
        base_url: server.uri(),
        api_key: "test-key".to_string(),
        default_model: "mock-model".to_string(),
        timeout_secs: 30,
        headers: HashMap::new(),
        proxy: None,
        preserve_prefix: false,
    }))
}

const INPUT: &str = "后端工程师岗位，负责订单中台微服务架构，要求熟悉 Go 语言、分布式事务消息、\
分库分表治理与可观测性建设，五年以上经验，能带小团队推进技术方案落地。";

// --- tests ------------------------------------------------------------------

#[tokio::test]
async fn generate_persona_clean_run_completes_with_full_coverage() {
    let server = MockServer::start().await;
    stage_mock("你是简历/JD 分析师", tool_args_response(units_args(&["Go", "微服务"], &["简洁"])))
        .mount(&server).await;
    stage_mock(
        "你是集群节点人格设计师",
        tool_args_response(pkg_args("## 定位\nGo 微服务老兵，主导过订单中台。", "## 工作哲学\n一切从简洁出发。")),
    )
    .mount(&server)
    .await;
    stage_mock("你是完整性审计员", tool_args_response(serde_json::json!({ "entries": [] })))
        .mount(&server)
        .await;

    let provider = provider_for(&server);
    let pkg = generate_persona(&provider, "mock-model", "jd", INPUT, 2)
        .await
        .expect("clean run succeeds");
    assert_eq!(pkg.node_name, "node-s10b");
    assert!(pkg.identity_md.contains("Go"));
    let report = pkg.coverage.expect("complete run attaches report");
    assert!(report.is_complete(), "no missing / gaps: {:?}", report.entries);
    // 3 target units (Go / 微服务 / 简洁 — one per key entity), all covered.
    assert_eq!(report.covered, 3);
}

#[tokio::test]
async fn generate_persona_retries_then_returns_pkg_with_gaps_report() {
    let server = MockServer::start().await;
    stage_mock("你是简历/JD 分析师", tool_args_response(units_args(&["XyzzyMissing"], &[])))
        .mount(&server).await;
    // Persona never contains the entity → both attempts report missing.
    stage_mock(
        "你是集群节点人格设计师",
        tool_args_response(pkg_args("## 定位\n通用后端。", "## 工作哲学\n务实。")),
    )
    .mount(&server)
    .await;
    stage_mock("你是完整性审计员", tool_args_response(serde_json::json!({ "entries": [] })))
        .mount(&server)
        .await;

    let provider = provider_for(&server);
    let pkg = generate_persona(&provider, "mock-model", "jd", INPUT, 2)
        .await
        .expect("exhausted retries still return the last pkg, not Err");
    let report = pkg.coverage.expect("incomplete run attaches gap report");
    assert!(!report.is_complete());
    assert!(report.missing >= 1, "the uncovered entity is reported: {:?}", report.entries);
    assert!(report.coverage_rate < 1.0);
}

#[tokio::test]
async fn generate_persona_author_parse_failure_retries_and_audit_http_failure_falls_back() {
    let server = MockServer::start().await;
    stage_mock("你是简历/JD 分析师", tool_args_response(units_args(&["Go"], &[])))
        .mount(&server).await;
    // First author call: plain text, no JSON → parse Err → continue arm.
    stage_mock("你是集群节点人格设计师", text_response("抱歉，我无法以结构化形式输出。"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    // Second author call carries the failure hint in its prompt → good pkg.
    stage_mock(
        "上轮创作/解析失败",
        tool_args_response(pkg_args("## 定位\nGo 微服务老兵。", "## 工作哲学\n务实。")),
    )
    .mount(&server)
    .await;
    // Audit stage fails with HTTP 500 → program-check fallback arm.
    stage_mock("你是完整性审计员", ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&server)
        .await;

    let provider = provider_for(&server);
    let pkg = generate_persona(&provider, "mock-model", "jd", INPUT, 2)
        .await
        .expect("second attempt completes via program check");
    let report = pkg.coverage.expect("report attached");
    assert!(report.is_complete(), "audit failure falls back to program check: {:?}", report.entries);
}

#[tokio::test]
async fn generate_persona_validate_failure_retries_with_good_pkg() {
    let server = MockServer::start().await;
    stage_mock("你是简历/JD 分析师", tool_args_response(units_args(&["Go"], &[])))
        .mount(&server).await;
    // First author call: role=boss → validate Err → continue arm.
    stage_mock(
        "你是集群节点人格设计师",
        tool_args_response(pkg_args_with_role("## 定位\nGo。", "## 工作哲学\n务实。", "boss")),
    )
    .up_to_n_times(1)
    .mount(&server)
    .await;
    // Second call prompt contains the validation-failure hint.
    stage_mock(
        "上轮校验失败",
        tool_args_response(pkg_args("## 定位\nGo 微服务老兵。", "## 工作哲学\n务实。")),
    )
    .mount(&server)
    .await;
    stage_mock("你是完整性审计员", tool_args_response(serde_json::json!({ "entries": [] })))
        .mount(&server)
        .await;

    let provider = provider_for(&server);
    let pkg = generate_persona(&provider, "mock-model", "jd", INPUT, 2)
        .await
        .expect("retry after invalid pkg succeeds");
    assert_eq!(pkg.role, "worker");
    assert!(pkg.coverage.as_ref().unwrap().is_complete());
}
