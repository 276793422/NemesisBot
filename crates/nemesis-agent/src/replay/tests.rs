//! Tests for the projection-ledger replay (T8 / U9 ②).
//!
//! Isolation note: the ledger sidecar resolves through the process-global
//! `default_path_manager()` boundary dir. Every test uses a unique session
//! key and removes its own `<key>.replay.jsonl` (+ boundary sidecar) at the
//! end — same precedent as the T6 history_search tests.

use super::*;
use crate::context::RequestContext;
use crate::instance::AgentInstance;
use crate::r#loop::{
    AgentLoop, LlmMessage, LlmProvider, LlmResponse, Tool as LoopTool, VOICE_PLAYBACK_SUFFIX,
};
use crate::session::SessionStore;
use crate::types::{AgentConfig, AgentEvent, ConversationTurn, ToolCallInfo};
use async_trait::async_trait;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

fn test_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        max_turns: 5,
        tools: vec!["calculator".to_string()],
        models: std::collections::HashMap::new(),
    }
}

fn turn(role: &str, content: &str) -> ConversationTurn {
    ConversationTurn {
        role: role.to_string(),
        content: content.to_string(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        timestamp: chrono::Local::now().to_rfc3339(),
        reasoning_content: None,
        tool_name: None,
        tool_result_projection: None,
    }
}

static KEY_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Unique-per-process session key so parallel test runs never share a ledger.
fn unique_key(prefix: &str) -> String {
    format!(
        "{}_{}_{}",
        prefix,
        std::process::id(),
        KEY_COUNTER.fetch_add(1, Ordering::SeqCst)
    )
}

/// Best-effort removal of this test's sidecars from the global boundary dir.
fn cleanup_sidecars(key: &str) {
    let dir = nemesis_path::default_path_manager().boundary_events_dir();
    let safe = key.replace(':', "_");
    let _ = std::fs::remove_file(dir.join(format!("{}.replay.jsonl", safe)));
    let _ = std::fs::remove_file(dir.join(format!("{}.jsonl", safe)));
}

fn temp_store(key: &str) -> (SessionStore, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("t8_replay_{}_{}", key, std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let store = SessionStore::new_with_storage(&dir);
    store.get_or_create(key);
    (store, dir)
}

/// Never-called provider (unit builds only; the full-loop test uses
/// [`CapturingProvider`]).
struct NoopProvider;

#[async_trait]
impl LlmProvider for NoopProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Err("not supposed to be called".to_string())
    }
}

/// Provider that records the exact message list (as `serde_json::Value`, the
/// same serialization the request_logger's raw.json uses) for every chat
/// call, then plays back scripted responses. Shared via Arc so the test keeps
/// a handle after the AgentLoop takes ownership of a forwarding wrapper.
struct CapturingProvider {
    responses: Mutex<Vec<LlmResponse>>,
    captured: Mutex<Vec<Vec<serde_json::Value>>>,
}

#[async_trait]
impl LlmProvider for CapturingProvider {
    async fn chat(
        &self,
        _model: &str,
        messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        self.captured
            .lock()
            .unwrap()
            .push(messages.iter().filter_map(|m| serde_json::to_value(m).ok()).collect());
        let mut responses = self.responses.lock().unwrap();
        if responses.is_empty() {
            Ok(LlmResponse {
                content: "No more responses".to_string(),
                tool_calls: Vec::new(),
                finished: true,
                reasoning_content: None,
                usage: None,
                raw_request_body: None,
                raw_response_body: None,
            })
        } else {
            Ok(responses.remove(0))
        }
    }
}

/// Thin forwarder so the test keeps the [`CapturingProvider`] handle.
struct ForwardingProvider {
    inner: Arc<CapturingProvider>,
}

#[async_trait]
impl LlmProvider for ForwardingProvider {
    async fn chat(
        &self,
        model: &str,
        messages: Vec<LlmMessage>,
        options: Option<crate::types::ChatOptions>,
        tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        self.inner.chat(model, messages, options, tools).await
    }
}

struct EchoTool;

#[async_trait]
impl LoopTool for EchoTool {
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok("4".to_string())
    }
}

/// Unit: annotated build + ledger round-trip rebuilds BYTE-EXACTLY — including
/// when the store's history grew AFTER the recorded round (the round's
/// `history_len_at_build` truncates the tail).
#[test]
fn test_rebuild_byte_exact_with_ledger_and_later_history() {
    let key = unique_key("t8_unit_exact");
    let agent_loop = AgentLoop::new(Box::new(NoopProvider), test_config());
    let instance = AgentInstance::new(test_config());
    instance.set_history(vec![
        turn("system", "You are a test assistant."),
        turn("user", "first question"),
        turn("assistant", "first answer"),
        turn("user", "second question"),
    ]);

    let (messages, annotation) = agent_loop.build_messages_with_memory_annotated(&instance, None);
    // Preconditions the test depends on: system at [0] + a trailing user
    // message ⇒ the merged context digest IS injected mid-vec.
    let digest_index = annotation
        .digest_index
        .expect("digest must be injected with system[0] + trailing user");
    assert_eq!(annotation.history_len, 4, "no summary cache: full history");
    assert!(annotation.summary_as_of.is_none());
    assert!(
        messages[digest_index].content.contains("# Current Time"),
        "digest content expected at {}",
        digest_index
    );

    let recorded: Vec<serde_json::Value> = messages
        .iter()
        .filter_map(|m| serde_json::to_value(m).ok())
        .collect();

    append_projection_record(&RequestProjectionRecord {
        trace_id: "unit-exact".to_string(),
        session_key: key.clone(),
        round: 1,
        ts: now_rfc3339(),
        messages_count: messages.len(),
        roles: messages.iter().map(|m| m.role.clone()).collect(),
        history_len_at_build: annotation.history_len,
        injections: vec![InjectionRecord {
            index: digest_index,
            role: messages[digest_index].role.clone(),
            source: INJECTION_CONTEXT_DIGEST.to_string(),
            content: messages[digest_index].content.clone(),
        }],
        voice_append: None,
        summary_as_of: annotation.summary_as_of.clone(),
    });

    // Store history has grown past the round (post-round assistant + new user).
    let (store, dir) = temp_store(&key);
    let mut final_history = instance.get_history();
    final_history.push(turn("assistant", "second answer (after the recorded round)"));
    final_history.push(turn("user", "third question (after the recorded round)"));
    store.set_history(
        &key,
        final_history.iter().map(|t| t.into()).collect(),
    );

    match rebuild_request_messages(&store, &key, 1).expect("rebuild must not error") {
        RebuildOutcome::Rebuilt(rebuilt) => {
            assert!(
                verify_request_replay(&rebuilt, &recorded).is_ok(),
                "rebuild must be byte-exact against the build output even though \
                 the store's history grew after the round"
            );
        }
        other => panic!("expected Rebuilt, got {:?}", other),
    }

    cleanup_sidecars(&key);
    let _ = std::fs::remove_dir_all(dir);
}

/// Unit: an injection whose recorded content changed (or any message diff)
/// is LOCATED — first-difference index + kind, not a bare "mismatch".
#[test]
fn test_verify_locates_injection_diff() {
    let agent_loop = AgentLoop::new(Box::new(NoopProvider), test_config());
    let instance = AgentInstance::new(test_config());
    instance.set_history(vec![
        turn("system", "You are a test assistant."),
        turn("user", "hello"),
    ]);

    let (messages, annotation) = agent_loop.build_messages_with_memory_annotated(&instance, None);
    let digest_index = annotation.digest_index.expect("digest injected");

    let recorded: Vec<serde_json::Value> = messages
        .iter()
        .filter_map(|m| serde_json::to_value(m).ok())
        .collect();
    // Simulate a drifted injection (e.g. ledger recorded an older digest).
    let mut tampered = recorded.clone();
    tampered[digest_index]["content"] =
        serde_json::json!("<system-reminder>\nstale digest\n</system-reminder>");

    let diff = verify_request_replay(&messages, &tampered)
        .expect_err("a tampered digest content must be caught");
    assert_eq!(diff.index, digest_index, "diff located at the digest position");
    assert_eq!(diff.kind, "content");
    assert!(
        diff.detail.contains("content differs"),
        "detail should describe the content diff: {}",
        diff.detail
    );

    // Role drift is classified separately.
    let mut tampered_role = recorded.clone();
    tampered_role[digest_index]["role"] = serde_json::json!("system");
    let diff = verify_request_replay(&messages, &tampered_role)
        .expect_err("role drift must be caught");
    assert_eq!(diff.kind, "role");

    // Count mismatch is classified separately.
    let truncated = &recorded[..recorded.len() - 1];
    let diff =
        verify_request_replay(&messages, truncated).expect_err("count mismatch must be caught");
    assert_eq!(diff.kind, "count");
}

/// Unit: the voice-playback suffix is a MUTATION of a persisted-derived
/// message; the ledger's `voice_append` replays it after all inserts.
/// Expected bytes are hand-authored (no circularity).
#[test]
fn test_voice_append_replays_on_top_of_digest_insert() {
    let key = unique_key("t8_unit_voice");
    let (store, dir) = temp_store(&key);
    store.set_history(
        &key,
        [turn("system", "You are a test assistant."),
            turn("user", "hello")]
        .iter()
        .map(|t| t.into())
        .collect(),
    );
    let digest_body = "<system-reminder>\n# Current Time / Environment snapshot\nfake\n</system-reminder>";
    append_projection_record(&RequestProjectionRecord {
        trace_id: "unit-voice".to_string(),
        session_key: key.clone(),
        round: 1,
        ts: now_rfc3339(),
        messages_count: 3,
        roles: vec!["system".into(), "user".into(), "user".into()],
        history_len_at_build: 2,
        injections: vec![InjectionRecord {
            index: 1,
            role: "user".to_string(),
            source: INJECTION_CONTEXT_DIGEST.to_string(),
            content: digest_body.to_string(),
        }],
        voice_append: Some(VoiceAppend {
            index: 2,
            suffix: VOICE_PLAYBACK_SUFFIX.to_string(),
        }),
        summary_as_of: None,
    });

    // Hand-authored expected request bytes: system, digest, user+suffix.
    // Built via LlmMessage serialization (not json! literals) because
    // `to_value(LlmMessage)` emits `tool_calls:null`/`tool_call_id:null` —
    // the exact bytes the provider/ request_logger see.
    let lm = |role: &str, content: String| LlmMessage {
        role: role.to_string(),
        content,
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
    };
    let recorded: Vec<serde_json::Value> = [
        lm("system", "You are a test assistant.".to_string()),
        lm("user", digest_body.to_string()),
        lm("user", format!("hello{}", VOICE_PLAYBACK_SUFFIX)),
    ]
    .iter()
    .filter_map(|m| serde_json::to_value(m).ok())
    .collect();

    match rebuild_request_messages(&store, &key, 1).expect("rebuild must not error") {
        RebuildOutcome::Rebuilt(rebuilt) => {
            assert!(verify_request_replay(&rebuilt, &recorded).is_ok(),
                "digest insert + voice suffix must replay to the hand-authored bytes");
        }
        other => panic!("expected Rebuilt, got {:?}", other),
    }

    cleanup_sidecars(&key);
    let _ = std::fs::remove_dir_all(dir);
}

/// Unit: equal `round` numbers across turns (round restarts at 1 every turn)
/// are disambiguated by `trace_id` — `rebuild_request_messages_in(..,
/// Some(t))` selects THAT turn's record; `None` keeps last-record-wins.
/// (Dashboard `replay_verify` regression: verifying round 1 of an older turn
/// used to always hit the newest turn's round 1.)
#[test]
fn test_trace_id_disambiguates_equal_rounds_across_turns() {
    let key = unique_key("t8_unit_tracedisamb");
    let (store, dir) = temp_store(&key);
    store.set_history(
        &key,
        [turn("system", "You are a test assistant."),
            turn("user", "hello")]
        .iter()
        .map(|t| t.into())
        .collect(),
    );

    // Turn A (older) and turn B (newer) both log round 1 — same shape,
    // different digest bodies, so the rebuilt bytes differ per trace.
    let mk_record = |trace_id: &str, digest_body: String| RequestProjectionRecord {
        trace_id: trace_id.to_string(),
        session_key: key.clone(),
        round: 1,
        ts: now_rfc3339(),
        messages_count: 3,
        roles: vec!["system".into(), "user".into(), "user".into()],
        history_len_at_build: 2,
        injections: vec![InjectionRecord {
            index: 1,
            role: "user".to_string(),
            source: INJECTION_CONTEXT_DIGEST.to_string(),
            content: digest_body,
        }],
        voice_append: None,
        summary_as_of: None,
    };
    let digest_a = "<system-reminder>\nTRACE-A digest\n</system-reminder>".to_string();
    let digest_b = "<system-reminder>\nTRACE-B digest\n</system-reminder>".to_string();
    append_projection_record(&mk_record("trace-a", digest_a.clone()));
    append_projection_record(&mk_record("trace-b", digest_b.clone()));

    let ledger = replay_ledger_path(&key);
    let lm = |role: &str, content: String| LlmMessage {
        role: role.to_string(),
        content,
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
    };
    let recorded_for = |digest: &str| -> Vec<serde_json::Value> {
        [
            lm("system", "You are a test assistant.".to_string()),
            lm("user", digest.to_string()),
            lm("user", "hello".to_string()),
        ]
        .iter()
        .filter_map(|m| serde_json::to_value(m).ok())
        .collect()
    };

    match rebuild_request_messages_in(&store, &ledger, 1, Some("trace-a")).expect("rebuild a") {
        RebuildOutcome::Rebuilt(rebuilt) => {
            assert!(
                verify_request_replay(&rebuilt, &recorded_for(&digest_a)).is_ok(),
                "trace-a selection must rebuild turn A's bytes"
            );
            assert!(
                verify_request_replay(&rebuilt, &recorded_for(&digest_b)).is_err(),
                "turn A's rebuild must NOT match turn B's bytes"
            );
        }
        other => panic!("expected Rebuilt for trace-a, got {:?}", other),
    }
    match rebuild_request_messages_in(&store, &ledger, 1, Some("trace-b")).expect("rebuild b") {
        RebuildOutcome::Rebuilt(rebuilt) => {
            assert!(
                verify_request_replay(&rebuilt, &recorded_for(&digest_b)).is_ok(),
                "trace-b selection must rebuild turn B's bytes"
            );
        }
        other => panic!("expected Rebuilt for trace-b, got {:?}", other),
    }
    // `None` keeps the documented last-record-wins behavior (trace-b is newer).
    match rebuild_request_messages_in(&store, &ledger, 1, None).expect("rebuild none") {
        RebuildOutcome::Rebuilt(rebuilt) => {
            assert!(
                verify_request_replay(&rebuilt, &recorded_for(&digest_b)).is_ok(),
                "None must keep last-record-wins (trace-b)"
            );
        }
        other => panic!("expected Rebuilt for None, got {:?}", other),
    }
    // Unknown trace_id → explicit NoLedger (with the trace named), never a
    // silent fallback to a different turn's record.
    match rebuild_request_messages_in(&store, &ledger, 1, Some("trace-x")).expect("no err") {
        RebuildOutcome::NoLedger { note } => {
            assert!(
                note.contains("trace-x"),
                "note must name the requested trace: {note}"
            );
        }
        other => panic!("expected NoLedger for unknown trace, got {:?}", other),
    }

    cleanup_sidecars(&key);
    let _ = std::fs::remove_dir_all(dir);
}

/// Unit: ledger ABSENT for the round (pre-feature session / exempted turn) ⇒
/// `verify_session_round` degrades EXPLICITLY to the role-subsequence anchor,
/// with a note saying so — never silently claims byte-exactness.
#[test]
fn test_no_ledger_degrades_to_subsequence() {
    let key = unique_key("t8_unit_noledger");
    let (store, dir) = temp_store(&key);
    store.set_history(
        &key,
        [turn("system", "You are a test assistant."),
            turn("user", "hello"),
            turn("assistant", "hi")]
        .iter()
        .map(|t| t.into())
        .collect(),
    );
    // No append_projection_record — ledger absent.

    let recorded = vec![
        serde_json::json!({"role": "system", "content": "You are a test assistant."}),
        serde_json::json!({"role": "user", "content": "hello"}),
        serde_json::json!({"role": "assistant", "content": "hi"}),
    ];

    match verify_session_round(&store, &key, 1, &recorded) {
        Ok(ReplayCheck::DegradedSubsequence { note, verdict }) => {
            assert!(!note.is_empty(), "degradation must be explained");
            assert!(
                note.contains("no projection ledger record"),
                "note should say why: {}",
                note
            );
            // All persisted roles appear in order in the recording → anchor Ok.
            assert!(verdict.is_ok(), "subsequence anchor should hold here");
        }
        other => panic!("expected DegradedSubsequence, got {:?}", other),
    }

    cleanup_sidecars(&key);
    let _ = std::fs::remove_dir_all(dir);
}

/// Unit: ledger exists but the history the round needed was trimmed away
/// (`MAX_STORED_MESSAGES` / store reset) ⇒ `Unavailable` with the numbers —
/// replay never fabricates messages.
#[test]
fn test_trimmed_history_reports_unavailable() {
    let key = unique_key("t8_unit_trimmed");
    let (store, dir) = temp_store(&key);
    store.set_history(
        &key,
        [turn("system", "You are a test assistant."),
            turn("user", "hello")]
        .iter()
        .map(|t| t.into())
        .collect(),
    );
    append_projection_record(&RequestProjectionRecord {
        trace_id: "unit-trim".to_string(),
        session_key: key.clone(),
        round: 1,
        ts: now_rfc3339(),
        messages_count: 10,
        roles: vec!["system".to_string(); 10],
        history_len_at_build: 10, // the round saw 10; the store kept 2
        injections: vec![],
        voice_append: None,
        summary_as_of: None,
    });

    match rebuild_request_messages(&store, &key, 1).expect("rebuild must not error") {
        RebuildOutcome::Unavailable { needed, available } => {
            assert_eq!(needed, 10);
            assert_eq!(available, 2);
        }
        other => panic!("expected Unavailable, got {:?}", other),
    }

    let recorded = vec![serde_json::json!({"role": "system", "content": "x"})];
    match verify_session_round(&store, &key, 1, &recorded) {
        Ok(ReplayCheck::Unavailable { needed, available }) => {
            assert_eq!((needed, available), (10, 2));
        }
        other => panic!("expected ReplayCheck::Unavailable, got {:?}", other),
    }

    cleanup_sidecars(&key);
    let _ = std::fs::remove_dir_all(dir);
}

/// Acceptance (goal T8): a FULL turn with a real tool loop — the ledger
/// records every round, and each round replays from the session store +
/// ledger BYTE-EXACTLY against what the provider actually received
/// (`serde_json::to_value` per message — the same serialization the
/// request_logger's raw.json uses). Round 1's replay works even though the
/// turn's later rounds grew the history — the as-of truncation property.
#[tokio::test]
async fn test_full_turn_replay_byte_exact() {
    let key = unique_key("t8_full");
    let capturing = Arc::new(CapturingProvider {
        responses: Mutex::new(vec![
            LlmResponse {
                content: String::new(),
                tool_calls: vec![ToolCallInfo {
                    id: "tc_1".to_string(),
                    name: "calculator".to_string(),
                    arguments: r#"{"expr":"2+2"}"#.to_string(),
                }],
                finished: false,
                reasoning_content: None,
                usage: None,
                raw_request_body: None,
                raw_response_body: None,
            },
            LlmResponse {
                content: "The answer is 4.".to_string(),
                tool_calls: Vec::new(),
                finished: true,
                reasoning_content: None,
                usage: None,
                raw_request_body: None,
                raw_response_body: None,
            },
        ]),
        captured: Mutex::new(Vec::new()),
    });

    let dir = std::env::temp_dir().join(format!("t8_replay_{}_{}", key, std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    // ONE store object shared by the loop and the assertions — the store's
    // in-memory map is per-instance; a second `new_with_storage` on the same
    // dir would not see the loop's writes.
    let store = Arc::new(SessionStore::new_with_storage(&dir));
    store.get_or_create(&key);
    let mut agent_loop = AgentLoop::new(
        Box::new(ForwardingProvider {
            inner: capturing.clone(),
        }),
        test_config(),
    );
    agent_loop.register_tool("calculator".to_string(), Box::new(EchoTool));
    agent_loop.set_session_store(store.clone());

    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", &key);

    let events = agent_loop.run(&instance, "What is 2+2?", &context).await;
    assert!(
        events.iter().any(|e| matches!(e, AgentEvent::Done(_))),
        "turn must finish"
    );
    // system + user + assistant(tool_call) + tool + assistant(final)
    assert_eq!(instance.get_history().len(), 5);

    let records = load_projection_records(&key);
    assert_eq!(
        records.len(),
        2,
        "two LLM rounds must produce two ledger rows"
    );
    assert_eq!(records[0].round, 1);
    assert_eq!(records[1].round, 2);
    for r in &records {
        assert!(
            r.injections.iter().any(|i| i.source == INJECTION_CONTEXT_DIGEST),
            "every round's digest injection must be recorded"
        );
    }

    let captured = capturing.captured.lock().unwrap().clone();
    assert_eq!(captured.len(), 2, "provider must have seen two requests");

    // Persist to the store, mirroring run_agent_loop_internal's save path
    // (cache BEFORE history; same precedent as loop/tests.rs:4408) — `run()`
    // itself drives the LLM loop but not the session-store save wrapper.
    store.get_or_create(&key);
    store.set_summary(&key, "");
    store.set_summary_covers_up_to(&key, None);
    store.set_history(
        &key,
        instance
            .get_history()
            .iter()
            .map(crate::session::StoredMessage::from)
            .collect(),
    );
    assert_eq!(store.get_history(&key).len(), 5);

    // Byte-exact per-round replay against what the provider received.
    for (i, recorded_round) in captured.iter().enumerate() {
        let round = i + 1;
        match rebuild_request_messages(&store, &key, round).expect("rebuild ok") {
            RebuildOutcome::Rebuilt(rebuilt) => {
                assert!(
                    verify_request_replay(&rebuilt, recorded_round).is_ok(),
                    "round {} replay must be byte-exact against the provider's view",
                    round
                );
            }
            other => panic!("round {} expected Rebuilt, got {:?}", round, other),
        }
    }

    cleanup_sidecars(&key);
    let _ = std::fs::remove_dir_all(dir);
}

/// Acceptance (goal X1 / U3): a full turn whose tool result is OVERSIZED
/// (above the 8192-char inline budget, below the spill threshold). Three
/// properties must hold end-to-end:
///   1. HISTORY keeps the ORIGINAL bytes (recoverable mid-section);
///   2. the PROVIDER saw the bounded pruned form (never the original);
///   3. per-round replay rebuild — which RECOMPUTES the fold through
///      `project_history_for_request` with no injection-ledger entry for it
///      — is byte-exact against what the provider received.
#[tokio::test]
async fn test_x1_pruned_tool_result_replay_byte_exact() {
    let key = unique_key("x1_pruned");
    // 13,200 chars: > 8192 (inline budget, must fold) but < 65,536 (spill
    // threshold, must NOT spill — the recompute path, not a locator override).
    let original: String = {
        let head: String = "A".repeat(3_600);
        let mid: String = "B".repeat(6_000);
        let tail: String = "C".repeat(3_600);
        format!("{head}{mid}{tail}")
    };
    assert_eq!(original.chars().count(), 13_200);

    struct BigOutputTool {
        output: String,
    }

    #[async_trait]
    impl LoopTool for BigOutputTool {
        async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
            Ok(self.output.clone())
        }
    }

    let capturing = Arc::new(CapturingProvider {
        responses: Mutex::new(vec![
            LlmResponse {
                content: String::new(),
                tool_calls: vec![ToolCallInfo {
                    id: "tc_big".to_string(),
                    name: "big_output".to_string(),
                    arguments: "{}".to_string(),
                }],
                finished: false,
                reasoning_content: None,
                usage: None,
                raw_request_body: None,
                raw_response_body: None,
            },
            LlmResponse {
                content: "Done with the big output.".to_string(),
                tool_calls: Vec::new(),
                finished: true,
                reasoning_content: None,
                usage: None,
                raw_request_body: None,
                raw_response_body: None,
            },
        ]),
        captured: Mutex::new(Vec::new()),
    });

    let dir = std::env::temp_dir().join(format!("x1_replay_{}_{}", key, std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let store = Arc::new(SessionStore::new_with_storage(&dir));
    store.get_or_create(&key);
    let mut agent_loop = AgentLoop::new(
        Box::new(ForwardingProvider {
            inner: capturing.clone(),
        }),
        test_config(),
    );
    agent_loop.register_tool("big_output".to_string(), Box::new(BigOutputTool { output: original.clone() }));
    agent_loop.set_session_store(store.clone());

    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", &key);

    let events = agent_loop
        .run(&instance, "run the big thing", &context)
        .await;
    assert!(
        events.iter().any(|e| matches!(e, AgentEvent::Done(_))),
        "turn must finish"
    );

    // (1) History keeps the ORIGINAL — byte-for-byte, mid-section intact.
    let history = instance.get_history();
    let tool_turn = history
        .iter()
        .find(|t| t.role == "tool")
        .expect("tool turn in history");
    assert_eq!(tool_turn.content, original, "history must keep the original");
    assert_eq!(tool_turn.tool_name.as_deref(), Some("big_output"));
    assert!(
        tool_turn.tool_result_projection.is_none(),
        "below spill threshold with no guard nudge: no override recorded"
    );

    // (2) The provider's round-2 request saw the bounded pruned form.
    let captured = capturing.captured.lock().unwrap().clone();
    assert_eq!(captured.len(), 2);
    let round2_tool: Vec<&serde_json::Value> = captured[1]
        .iter()
        .filter(|m| m.get("role").and_then(|r| r.as_str()) == Some("tool"))
        .collect();
    assert_eq!(round2_tool.len(), 1);
    let seen = round2_tool[0]
        .get("content")
        .and_then(|c| c.as_str())
        .expect("tool content in round-2 request");
    assert!(
        seen.chars().count() < original.chars().count(),
        "provider must see the bounded form, got {} chars",
        seen.chars().count()
    );
    assert!(seen.contains("big_output"), "marker names the tool");
    assert!(!seen.contains(&"B".repeat(100)), "mid-section elided");

    // Persist (store round-trip must preserve the original + fields).
    store.get_or_create(&key);
    store.set_summary(&key, "");
    store.set_summary_covers_up_to(&key, None);
    store.set_history(
        &key,
        history
            .iter()
            .map(crate::session::StoredMessage::from)
            .collect(),
    );
    let reloaded: Vec<ConversationTurn> = store
        .get_history(&key)
        .into_iter()
        .map(|m| m.into())
        .collect();
    let reloaded_tool = reloaded
        .iter()
        .find(|t| t.role == "tool")
        .expect("tool turn after reload");
    assert_eq!(reloaded_tool.content, original, "store round-trip keeps original");
    assert_eq!(reloaded_tool.tool_name.as_deref(), Some("big_output"));

    // (3) Byte-exact replay for BOTH rounds: the rebuild recomputes the fold
    // (deterministic projection, no ledger entry for the prune itself).
    for (i, recorded_round) in captured.iter().enumerate() {
        let round = i + 1;
        match rebuild_request_messages(&store, &key, round).expect("rebuild ok") {
            RebuildOutcome::Rebuilt(rebuilt) => {
                assert!(
                    verify_request_replay(&rebuilt, recorded_round).is_ok(),
                    "round {} replay must be byte-exact (recomputed fold)",
                    round
                );
            }
            other => panic!("round {} expected Rebuilt, got {:?}", round, other),
        }
    }

    cleanup_sidecars(&key);
    let _ = std::fs::remove_dir_all(dir);
}

// ---------------------------------------------------------------------------
// W3a: 分支补漏（append 失败臂 / 越界注入 push / voice 变异 / verify 的
// tool_calls 与 field 臂 / char-boundary 帮助函数 / verify_session_round 的
// Rebuilt 路径 / read_raw_request_messages）
// ---------------------------------------------------------------------------

/// append_projection_record：账本路径被目录占位 → warn + 静默返回。
#[test]
fn append_projection_record_open_failure_warns() {
    let key = unique_key("t8_open_fail");
    let dir = nemesis_path::default_path_manager().boundary_events_dir();
    let safe = key.replace(':', "_");
    let _ = std::fs::create_dir_all(&dir);
    let ledger = dir.join(format!("{}.replay.jsonl", safe));
    let _ = std::fs::remove_file(&ledger);
    std::fs::create_dir_all(&ledger).unwrap();

    append_projection_record(&RequestProjectionRecord {
        trace_id: "t".to_string(),
        session_key: key.clone(),
        round: 1,
        ts: now_rfc3339(),
        messages_count: 1,
        roles: vec!["user".to_string()],
        history_len_at_build: 1,
        injections: vec![],
        voice_append: None,
        summary_as_of: None,
    });
    assert!(load_projection_records(&key).is_empty(), "nothing written");

    std::fs::remove_dir(&ledger).unwrap();
}

/// 注入 index 超出 view 末端 → push 到尾部；voice 变异落在 push 出来的
/// 消息上。
#[test]
fn rebuild_pushes_injection_beyond_view_end_and_applies_voice() {
    let key = unique_key("t8_push_inj");
    let (store, dir) = temp_store(&key);
    store.set_history(
        &key,
        vec![
            (&turn("system", "sys")).into(),
            (&turn("user", "q")).into(),
        ],
    );
    append_projection_record(&RequestProjectionRecord {
        trace_id: "t".to_string(),
        session_key: key.clone(),
        round: 1,
        ts: now_rfc3339(),
        messages_count: 3,
        roles: vec!["system".into(), "user".into(), "system".into()],
        history_len_at_build: 2,
        injections: vec![InjectionRecord {
            index: 5, // 越过 view.len()==2 → push 到尾部
            role: "system".to_string(),
            source: INJECTION_GRACE_NUDGE.to_string(),
            content: "wrap up".to_string(),
        }],
        voice_append: Some(VoiceAppend {
            index: 2, // push 之后注入消息正好在 idx 2
            suffix: " [voice]".to_string(),
        }),
        summary_as_of: None,
    });

    match rebuild_request_messages(&store, &key, 1).expect("rebuild") {
        RebuildOutcome::Rebuilt(view) => {
            assert_eq!(view.len(), 3);
            assert_eq!(view[0].role, "system");
            assert_eq!(view[1].content, "q");
            assert_eq!(view[2].content, "wrap up [voice]", "push + voice suffix");
        }
        other => panic!("expected Rebuilt, got {:?}", other),
    }

    cleanup_sidecars(&key);
    let _ = std::fs::remove_dir_all(dir);
}

/// verify_request_replay 的 tool_calls 臂与「role/content 之外的 field」臂。
#[test]
fn verify_request_replay_tool_calls_and_field_arms() {
    use crate::r#loop::LlmMessage;
    let msg = |role: &str, content: &str| LlmMessage {
        role: role.to_string(),
        content: content.to_string(),
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
    };

    // tool_calls 臂：rebuilt 无 tool_calls，recorded 带数组。
    let rebuilt = vec![msg("assistant", "calling")];
    let recorded = vec![serde_json::json!({
        "role": "assistant",
        "content": "calling",
        "tool_calls": [
            {"id": "1", "type": "function", "function": {"name": "t", "arguments": "{}"}}
        ]
    })];
    let diff = verify_request_replay(&rebuilt, &recorded).unwrap_err();
    assert_eq!(diff.kind, "tool_calls");
    assert_eq!(diff.index, 0);

    // field 臂：role/content/tool_calls 全一致，差异在 reasoning_content。
    let mut with_reasoning = msg("assistant", "ans");
    with_reasoning.reasoning_content = Some("thinking...".to_string());
    let recorded2 = vec![serde_json::json!({
        "role": "assistant",
        "content": "ans",
        "tool_calls": null,
    })];
    let diff2 = verify_request_replay(&[with_reasoning], &recorded2).unwrap_err();
    assert_eq!(diff2.kind, "field");
    assert!(diff2.detail.contains("beyond role/content/tool_calls"));
}

/// first_diff_offset 的 char-boundary 回落与 preview 的换行转义。
#[test]
fn first_diff_offset_char_boundary_and_preview_escape() {
    // 完全一致：i 走到 len → 早退支。
    assert_eq!(first_diff_offset("abc", "abc"), 3);
    assert_eq!(first_diff_offset("", ""), 0);

    // 差异落在一个多字节字符内部：回落到该字符的起始边界。
    // “文” = e6 96 87；U+6580 = e6 96 80 → 第 5 字节才不同（“文”的中间）。
    let a = "中文";
    let b = "中\u{6580}";
    assert_eq!(first_diff_offset(a, b), 3, "must floor to char start");

    // preview：换行转成 \\n 字面量。
    let p = preview("line1\nline2", 5);
    assert!(p.contains("\\n"), "newline escaped: {p}");
}

/// verify_session_round 的 Rebuilt→ByteExact 主路径（此前只测过 NoLedger
/// 与 Unavailable 两个退化支）。
#[test]
fn verify_session_round_rebuilt_path_byte_exact() {
    let key = unique_key("t8_round_exact");
    let (store, dir) = temp_store(&key);
    store.set_history(
        &key,
        vec![
            (&turn("system", "sys prompt")).into(),
            (&turn("user", "hello")).into(),
        ],
    );
    append_projection_record(&RequestProjectionRecord {
        trace_id: "t".to_string(),
        session_key: key.clone(),
        round: 1,
        ts: now_rfc3339(),
        messages_count: 2,
        roles: vec!["system".into(), "user".into()],
        history_len_at_build: 2,
        injections: vec![],
        voice_append: None,
        summary_as_of: None,
    });

    let recorded: Vec<serde_json::Value> = ["sys prompt", "hello"]
        .iter()
        .enumerate()
        .map(|(i, c)| {
            serde_json::json!({
                "role": if i == 0 { "system" } else { "user" },
                "content": c,
                "tool_calls": null,
                "tool_call_id": null,
            })
        })
        .collect();

    match verify_session_round(&store, &key, 1, &recorded) {
        Ok(ReplayCheck::ByteExact) => {}
        other => panic!("expected ByteExact, got {:?}", other),
    }

    cleanup_sidecars(&key);
    let _ = std::fs::remove_dir_all(dir);
}

/// read_raw_request_messages：完整 envelope、缺文件、垃圾、缺 round、
/// messages 非数组。
#[test]
fn read_raw_request_messages_variants() {
    let tmp = tempfile::tempdir().unwrap();

    // happy path。
    let p = tmp.path().join("01.raw.json");
    std::fs::write(
        &p,
        serde_json::json!({
            "round": 2,
            "body": { "messages": [
                {"role": "system", "content": "s"},
                {"role": "user", "content": "u"},
            ]}
        })
        .to_string(),
    )
    .unwrap();
    let (round, msgs) = read_raw_request_messages(&p).expect("parses");
    assert_eq!(round, 2);
    assert_eq!(msgs.len(), 2);

    // 缺文件。
    assert!(read_raw_request_messages(&tmp.path().join("nope.json")).is_none());
    // 垃圾内容。
    let g = tmp.path().join("garbage.json");
    std::fs::write(&g, "NOT JSON").unwrap();
    assert!(read_raw_request_messages(&g).is_none());
    // 缺 round。
    let nr = tmp.path().join("noround.json");
    std::fs::write(&nr, r#"{"body":{"messages":[]}}"#).unwrap();
    assert!(read_raw_request_messages(&nr).is_none());
    // messages 不是数组。
    let na = tmp.path().join("notarray.json");
    std::fs::write(&na, r#"{"round":1,"body":{"messages":42}}"#).unwrap();
    assert!(read_raw_request_messages(&na).is_none());
}
