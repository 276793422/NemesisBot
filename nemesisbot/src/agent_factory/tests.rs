//! Factory wiring tests (2026-08-24 arch review, U-list D1/D2).
//!
//! D1 — `build_cluster_agent_loop` resolves the startup capability tier from
//!      config.json exactly like `build_agent_loop`: a mini-tier active model
//!      must yield the mini tier on the cluster loop (previously the loop
//!      always ran the AgentLoop::new default = Big = full 42-tool set even
//!      for small models; tier filtering reads `self.tier` at tool-def build
//!      time, so a correct tier is the whole guarantee).
//! D2 — the cluster loop gets a spill root (`<home>/logs/spill`) so oversized
//!      tool results spill whole to disk instead of degrading to the prune
//!      profile. It shares the main agent's root; the daily cleanup task is
//!      only spawned by the main factory (asserted by wiring, not here).
//! D3 — comment-only (no behavior change); not asserted.

// The cluster factory is feature-gated; mirror the gate so the module is
// empty in trimmed builds (--no-default-features must compile clean).
#![cfg(feature = "cluster")]

use std::sync::Arc;

use nemesis_cluster::types::ClusterConfig;

use super::*;

/// Write a config.json whose active model is tagged `model_tier: "mini"`.
///
/// Shape mirrors what `model add` writes (model_name = post-slash segment,
/// model = full vendor/name), so the resolution chain
/// get_effective_llm → resolve_model_config → resolve_active_tier
/// matches production entries exactly.
fn write_mini_model_config(home: &std::path::Path) {
    let cfg = serde_json::json!({
        "agents": { "defaults": { "llm": "mini-model", "max_tool_iterations": 5 } },
        "model_list": [{
            "model_name": "mini-model",
            "model": "testai/mini-model",
            "api_key": "test-key",
            "api_base": "http://127.0.0.1:9",
            "model_tier": "mini"
        }]
    });
    std::fs::create_dir_all(home).unwrap();
    std::fs::write(home.join("config.json"), cfg.to_string()).unwrap();
}

#[tokio::test]
async fn cluster_agent_resolves_tier_and_spill_root_from_config() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().to_path_buf();
    write_mini_model_config(&home);

    let (outbound_tx, _outbound_rx) = tokio::sync::mpsc::channel(16);
    let shared = Arc::new(SharedResources {
        home: home.clone(),
        agent_outbound_tx: outbound_tx,
        cron_service: Arc::new(std::sync::Mutex::new(nemesis_cron::service::CronService::new(
            "",
        ))),
        mcp_config_path: home.join("nonexistent-mcp.json"),
        ..Default::default()
    });
    // Cluster::new only builds in-memory state (registry / task manager /
    // continuation store paths) — no sockets, safe in tests.
    let cluster = Arc::new(nemesis_cluster::cluster::Cluster::new(ClusterConfig {
        node_id: "test-node".to_string(),
        bind_address: "127.0.0.1:0".to_string(),
        peers: Vec::new(),
    }));

    let (agent_loop, _config, _observer) =
        build_cluster_agent_loop(&shared, cluster).expect(
            "build_cluster_agent_loop must succeed with a mini-tier config (provider construction is offline: HttpCompat + dummy key)",
        );

    // D1: startup tier comes from config.json (was: always the Big default).
    assert!(
        matches!(
            agent_loop.tier(),
            nemesis_types::capability::ModelTier::Mini
        ),
        "cluster agent must resolve model_tier=mini from config.json, got {:?}",
        agent_loop.tier()
    );
    // Tools are still registered for the loop (tier filtering happens at
    // tool-def build time inside the loop, reading the tier asserted above).
    assert!(agent_loop.tool_count() > 0);

    // D2: spill root wired to the shared <home>/logs/spill.
    assert_eq!(
        agent_loop.spill_root_path(),
        Some(home.join("logs").join("spill"))
    );
}
