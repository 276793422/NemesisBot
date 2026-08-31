//! R4 覆盖率（2026-08-27）：episodic 富集两段 + 审计链 prev_hash mismatch。
//!
//! - `session_list` 的 episodic metadata 富集（model / triggerCluster）
//! - `session_detail` 的 episodic tags 富集（triggerCluster / toolCalls）
//! - `chain_list` 的 `prev_hash mismatch` 分支（与 wweb2_tests 的
//!   `hash mismatch` 互补：手工构造「自身 hash 自洽但 prev 断链」的事件）
//!
//! 夹具沿用 browse_tests 的 make_ctx / make_session_log / wweb2_tests 的
//! make_real_chain，另加 make_ctx_with_memory 注入真 MemoryManager。

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::RequestContext;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

// Need the memory feature for the enrichment blocks under test.
#[cfg(feature = "memory")]
fn make_ctx_with_memory(dir: &tempfile::TempDir) -> RequestContext {
    let ws = dir.path().to_string_lossy().to_string();
    let mem_cfg = nemesis_memory::manager::Config::new(dir.path());
    let mgr = Arc::new(nemesis_memory::manager::MemoryManager::new(&mem_cfg));
    let state = Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: Some(ws.clone()),
        home: Some(ws.clone()),
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new("test-model".to_string())),
        model_base: Arc::new(parking_lot::Mutex::new(String::new())),
        model_has_key: Arc::new(AtomicBool::new(false)),
        event_hub: Arc::new(EventHub::new()),
        running: Arc::new(AtomicBool::new(true)),
        session_manager: Arc::new(SessionManager::with_default_timeout()),
        inbound_tx: None,
        streaming_provider: None,
        ws_router: None,
        agent_service: None,
        data_store: None,
        memory_manager: Some(mgr),
        forge: None,
        agent_loop: Arc::new(parking_lot::RwLock::new(None)),
        cluster: None,
        cluster_service: None,
        cluster_log_dir: None,
        workflow_engine: None,
        chat_secret_store: std::sync::Arc::new(
            nemesis_workflow::chat_secrets::ChatSecretStore::in_memory(),
        ),
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        internal_cmd_tx: None,
        estop: None,
        cron: None,
        board: None,
    });
    RequestContext {
        session_id: "test-session".to_string(),
        chat_id: "test-chat".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

fn make_session_log(ws: &std::path::Path, id: &str, lines: &[(&str, &str)]) {
    let dir = ws.join("logs/session_logs");
    std::fs::create_dir_all(&dir).unwrap();
    let body: String = lines
        .iter()
        .map(|(role, content)| {
            serde_json::json!({
                "role": role,
                "content": content,
                "timestamp": "2026-08-27T07:00:00",
            })
            .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(dir.join(format!("{id}.jsonl")), body + "\n").unwrap();
}

#[cfg(feature = "memory")]
#[tokio::test]
async fn session_list_without_matching_episodic_entry_keeps_scan_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_memory(&dir);
    let ws = dir.path().to_string_lossy().to_string();

    make_session_log(dir.path(), "sess_a", &[("user", "hello"), ("assistant", "hi")]);

    // Episodic entry under an UNRELATED session key must not leak into sess_a.
    let mut ep = nemesis_memory::episodic::Episode::new(
        "web:chat_1".to_string(),
        "assistant".to_string(),
        "did the thing".to_string(),
    );
    ep.metadata.insert("model".to_string(), "probe-model".to_string());
    ep.tags.push("cluster".to_string());
    ctx.state
        .memory_manager
        .as_ref()
        .unwrap()
        .get_episodic_store()
        .append(ep)
        .await
        .unwrap();

    let out = LogsHandler
        .session_list(&ctx, &ws, None, 50, 0)
        .await
        .unwrap()
        .unwrap();
    let sessions = out["sessions"].as_array().unwrap();
    // Seed a session whose file stem matches the episodic key's colon-restored form.
    // The enrichment matches `sess_a` only if episodic has an entry under it; here we
    // assert the code path executes and the matching entry gets enriched.
    assert!(!sessions.is_empty());
    let hit = sessions.iter().find(|s| s["id"] == "sess_a").unwrap();
    // No episodic entry under "sess_a" / "sess:a" → stays at scan defaults
    // (scan_session_logs always seeds model="" / triggerCluster=false).
    assert_eq!(hit["model"], "");
    assert_eq!(hit["triggerCluster"], false);
}

#[cfg(feature = "memory")]
#[tokio::test]
async fn session_list_episodic_enrichment_matches_stem_and_colon_form() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_memory(&dir);
    let ws = dir.path().to_string_lossy().to_string();

    // chat_log stores ':' as '_' in the file name; episodic key uses ':'
    // (file stem = "web_chat_1", episodic key = "web:chat_1" and its stem form).
    make_session_log(dir.path(), "web_chat_1", &[("user", "q"), ("assistant", "a")]);

    let mut ep = nemesis_memory::episodic::Episode::new(
        "web:chat_1".to_string(),
        "assistant".to_string(),
        "cluster triggered".to_string(),
    );
    ep.metadata.insert("model".to_string(), "glm-4.7".to_string());
    ep.tags.push("cluster".to_string());
    ctx.state
        .memory_manager
        .as_ref()
        .unwrap()
        .get_episodic_store()
        .append(ep)
        .await
        .unwrap();

    let out = LogsHandler
        .session_list(&ctx, &ws, None, 50, 0)
        .await
        .unwrap()
        .unwrap();
    let sessions = out["sessions"].as_array().unwrap();
    let hit = sessions
        .iter()
        .find(|s| s["id"] == "web_chat_1")
        .expect("seeded session must be listed");
    assert_eq!(hit["model"], "glm-4.7");
    assert_eq!(hit["triggerCluster"], true);
}

#[cfg(feature = "memory")]
#[tokio::test]
async fn session_detail_enriches_messages_with_episodic_tags() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_memory(&dir);
    let ws = dir.path().to_string_lossy().to_string();

    make_session_log(
        dir.path(),
        "web_chat_2",
        &[("user", "run it"), ("assistant", "ran")],
    );

    // Clustered episode carries ONLY the tag; the other carries ONLY
    // tool_calls metadata → each message gets exactly one enrichment field.
    let mut clustered = nemesis_memory::episodic::Episode::new(
        "web:chat_2".to_string(),
        "user".to_string(),
        "content".to_string(),
    );
    clustered.tags.push("cluster".to_string());
    let mut counted = nemesis_memory::episodic::Episode::new(
        "web:chat_2".to_string(),
        "assistant".to_string(),
        "content".to_string(),
    );
    counted.metadata.insert("tool_calls".to_string(), "3".to_string());
    let store = ctx.state.memory_manager.as_ref().unwrap().get_episodic_store();
    store.append(clustered).await.unwrap();
    store.append(counted).await.unwrap();

    let out = LogsHandler
        .session_detail(&ctx, &ws, "web_chat_2")
        .await
        .unwrap()
        .unwrap();
    let msgs = out["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0]["triggerCluster"], true);
    assert_eq!(msgs[1]["toolCalls"], 3);
    // Untagged message carries neither field.
    assert!(msgs[0].get("toolCalls").is_none());
    assert!(msgs[1].get("triggerCluster").is_none());
}

#[cfg(feature = "security")]
#[tokio::test]
async fn chain_list_prev_hash_break_reports_prev_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let main = {
        let sec_dir = dir.path().join("logs/security_logs");
        std::fs::create_dir_all(&sec_dir).unwrap();
        let config = nemesis_security::integrity::AuditChainConfig {
            storage_path: sec_dir.join("audit_chain.jsonl"),
            max_events_per_segment: 10,
            ..Default::default()
        };
        let chain = nemesis_security::integrity::AuditChain::new(config);
        for i in 0..3 {
            chain
                .append(
                    &format!("op-{i}"),
                    &format!("tool-{i}"),
                    "tester",
                    "ws",
                    &format!("target-{i}"),
                    "allowed",
                    "ok",
                )
                .unwrap();
        }
        sec_dir.join("audit_chain.jsonl")
    };

    // Rewrite event #1 with a foreign prev_hash and a freshly computed own hash:
    // own hash verifies (hash_match=true) but prev does not chain to event #0
    // → exercises the `prev_hash mismatch` breakReason branch.
    let lines: Vec<String> = std::fs::read_to_string(&main).unwrap().lines().map(String::from).collect();
    let mut ev: nemesis_security::integrity::AuditEvent =
        serde_json::from_str(&lines[1]).unwrap();
    ev.prev_hash = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".to_string();
    ev.hash = compute_audit_hash(&ev);
    std::fs::write(&main, format!("{}\n{}\n{}\n", lines[0], serde_json::to_string(&ev).unwrap(), lines[2])).unwrap();

    let ctx = {
        let ws = dir.path().to_string_lossy().to_string();
        let state = Arc::new(AppState {
            auth_token: String::new(),
            session_count: Arc::new(AtomicUsize::new(0)),
            workspace: Some(ws.clone()),
            home: Some(ws.clone()),
            version: "test".to_string(),
            start_time: Instant::now(),
            model_name: Arc::new(parking_lot::Mutex::new("m".to_string())),
            model_base: Arc::new(parking_lot::Mutex::new(String::new())),
            model_has_key: Arc::new(AtomicBool::new(false)),
            event_hub: Arc::new(EventHub::new()),
            running: Arc::new(AtomicBool::new(true)),
            session_manager: Arc::new(SessionManager::with_default_timeout()),
            inbound_tx: None,
            streaming_provider: None,
            ws_router: None,
            agent_service: None,
            data_store: None,
            memory_manager: None,
            forge: None,
            agent_loop: Arc::new(parking_lot::RwLock::new(None)),
            cluster: None,
            cluster_service: None,
            cluster_log_dir: None,
            workflow_engine: None,
            chat_secret_store: std::sync::Arc::new(
                nemesis_workflow::chat_secrets::ChatSecretStore::in_memory(),
            ),
            webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
            internal_cmd_tx: None,
            estop: None,
            cron: None,
            board: None,
        });
        RequestContext {
            session_id: "t".to_string(),
            chat_id: "t".to_string(),
            workspace: Some(ws.clone()),
            home: Some(ws),
            state,
            auth_method: crate::session::AuthMethod::default(),
        }
    };

    let r = LogsHandler
        .handle_cmd("chain_verify", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    // Per-event check model: breaking event #1's prev link orphans event #2
    // too (its prev_hash still points at event #1's ORIGINAL hash) → single
    // tamper, two broken segments. Cascade semantics pinned here.
    assert_eq!(r["valid"], false);
    assert_eq!(r["first_broken_index"], 1);
    assert_eq!(r["broken_count"], 2);
}
