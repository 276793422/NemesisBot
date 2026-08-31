//! Tests for the models handler — P3-2 (2026-08-24 UI entry gap) attribute
//! editor + catalog glue.
//!
//! Declared from `models.rs` (not the flat `handlers/mod.rs` extra-test list)
//! so the private `update_field`/`catalog_info`/`list` methods are reachable
//! without constructing an AppState — same pattern as `sandbox/tests.rs`.
//! Consequence: these run under EVERY feature combo including
//! `--no-default-features` (the models handler is not feature-gated).

use super::*;

fn write_config(home: &std::path::Path, body: &str) {
    std::fs::write(home.join("config.json"), body).unwrap();
}

fn read_config_raw(home: &std::path::Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(home.join("config.json")).unwrap()).unwrap()
}

fn home_str(dir: &tempfile::TempDir) -> String {
    dir.path().to_string_lossy().to_string()
}

/// Two models: m1 carries raw extras (tier + real_name + an unmodeled key),
/// m2 is bare. Both shapes occur in real configs (`model add --default`
/// writes bare entries; the CLI `set-*` commands write the extras).
const SEED: &str = r#"{
  "model_list": [
    {
      "model_name": "m1",
      "model": "qwen/qwen3-30b",
      "api_key": "sk-secret",
      "model_tier": "auto",
      "real_name": "Qwen3-30B",
      "custom_note": "keep-me"
    },
    { "model_name": "m2", "model": "test/testai-1.1" }
  ]
}"#;

/// The whole point of the raw-JSON RMW: tier/size/real_name/context_window
/// are extra keys the typed ModelConfig does not model, so a typed save would
/// DROP them. Editing one field must preserve every other key on disk.
#[test]
fn update_field_tier_roundtrip_preserves_raw_extras() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), SEED);
    let h = ModelsHandler::new();

    let out = h
        .update_field(
            &home_str(&dir),
            &serde_json::json!({ "name": "m1", "field": "model_tier", "value": "mini" }),
        )
        .unwrap()
        .unwrap();
    assert_eq!(out["updated"], true);
    assert_eq!(out["value"], "mini");

    let cfg = read_config_raw(dir.path());
    let m1 = &cfg["model_list"][0];
    assert_eq!(m1["model_tier"], "mini");
    assert_eq!(m1["real_name"], "Qwen3-30B", "sibling extra must survive");
    assert_eq!(m1["custom_note"], "keep-me", "unmodeled key must survive");
    assert_eq!(m1["api_key"], "sk-secret", "typed fields untouched");
    // m2 untouched (no tier key invented).
    assert_eq!(cfg["model_list"][1]["model_name"], "m2");
    assert!(cfg["model_list"][1].get("model_tier").is_none());
}

/// Matching by the `model` identifier (vendor/model string) must work too —
/// the dashboard list shows both, and set_default accepts either.
#[test]
fn update_field_matches_model_key_too() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), SEED);
    let h = ModelsHandler::new();

    h.update_field(
        &home_str(&dir),
        &serde_json::json!({ "name": "qwen/qwen3-30b", "field": "model_tier", "value": "big" }),
    )
    .unwrap()
    .unwrap();
    assert_eq!(read_config_raw(dir.path())["model_list"][0]["model_tier"], "big");
}

#[test]
fn update_field_validation_rejects_bad_values() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), SEED);
    let h = ModelsHandler::new();
    let home = home_str(&dir);

    let cases: Vec<(&str, &str, serde_json::Value, &str)> = vec![
        ("m1", "model_tier", serde_json::json!("huge"), "Invalid tier"),
        ("m1", "reasoning_effort", serde_json::json!("turbo"), "Invalid effort"),
        ("m1", "model_size_b", serde_json::json!(0), "> 0"),
        ("m1", "context_window", serde_json::json!(-5), "positive number"),
        ("m1", "model_tier", serde_json::json!(3), "must be a string"),
        ("m1", "no_such_field", serde_json::json!("x"), "unknown field"),
        ("ghost", "model_tier", serde_json::json!("mini"), "not found"),
        ("m1", "real_name", serde_json::json!("  "), "not be empty"),
    ];
    for (name, field, value, want) in cases {
        let err = h
            .update_field(&home, &serde_json::json!({ "name": name, "field": field, "value": value }))
            .unwrap_err();
        assert!(
            err.contains(want),
            "field={field} value={value}: expected error containing '{want}', got '{err}'"
        );
    }
    // Every rejection above must have left the file untouched.
    assert_eq!(read_config_raw(dir.path())["model_list"][0]["model_tier"], "auto");
}

/// Normalization parity with the CLI `model set-*` commands:
/// effort "off" clears to empty string, case-insensitive accepted;
/// numeric fields accept JSON strings and store as numbers.
#[test]
fn update_field_effort_and_size_normalization() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), SEED);
    let h = ModelsHandler::new();
    let home = home_str(&dir);

    h.update_field(&home, &serde_json::json!({ "name": "m1", "field": "reasoning_effort", "value": "HIGH" }))
        .unwrap()
        .unwrap();
    assert_eq!(read_config_raw(dir.path())["model_list"][0]["reasoning_effort"], "high");

    h.update_field(&home, &serde_json::json!({ "name": "m1", "field": "reasoning_effort", "value": "off" }))
        .unwrap()
        .unwrap();
    assert_eq!(read_config_raw(dir.path())["model_list"][0]["reasoning_effort"], "");

    h.update_field(&home, &serde_json::json!({ "name": "m1", "field": "model_size_b", "value": "30" }))
        .unwrap()
        .unwrap();
    let size = &read_config_raw(dir.path())["model_list"][0]["model_size_b"];
    assert_eq!(size.as_u64(), Some(30), "string input must be stored as a number");

    h.update_field(&home, &serde_json::json!({ "name": "m1", "field": "context_window", "value": 131072 }))
        .unwrap()
        .unwrap();
    assert_eq!(read_config_raw(dir.path())["model_list"][0]["context_window"].as_u64(), Some(131072));
}

#[test]
fn catalog_info_reports_cache_state() {
    let dir = tempfile::tempdir().unwrap();
    let h = ModelsHandler::new();
    let home = home_str(&dir);

    // No cache file → exists=false, not an error (fresh installs).
    let info = h.catalog_info(&home).unwrap().unwrap();
    assert_eq!(info["exists"], false);
    assert_eq!(info["entries"], 0);

    // 真相源 = nemesis_path::models_catalog_cache_path（与 CLI 写盘位置同源）。
    let seeded = nemesis_path::models_catalog_cache_path(dir.path());
    std::fs::create_dir_all(seeded.parent().unwrap()).unwrap();
    std::fs::write(
        seeded,
        r#"{ "version": 1, "fetched_at": "2026-08-24T00:00:00Z",
            "entries": [ { "key": "a" }, { "key": "b" } ] }"#,
    )
    .unwrap();
    let info = h.catalog_info(&home).unwrap().unwrap();
    assert_eq!(info["exists"], true);
    assert_eq!(info["entries"], 2);
    assert_eq!(info["fetched_at"], "2026-08-24T00:00:00Z");
}

/// 2026-08-28 布局迁移：home 根的 legacy 缓存在首次读取时自动 rename 进
/// workspace/data（零重新下载），与 CLI 侧共用 nemesis-path 的迁移函数同契约。
#[test]
fn catalog_info_migrates_legacy_home_root_cache() {
    let dir = tempfile::tempdir().unwrap();
    let h = ModelsHandler::new();
    let home = home_str(&dir);

    // legacy 位置种缓存 → 读一次 → 搬到新位置且内容可读。
    std::fs::write(
        nemesis_path::legacy_models_catalog_cache_path(dir.path()),
        r#"{ "version": 1, "fetched_at": "2026-08-27T00:00:00Z",
            "entries": [ { "key": "legacy/a" } ] }"#,
    )
    .unwrap();
    let info = h.catalog_info(&home).unwrap().unwrap();
    assert_eq!(info["exists"], true);
    assert_eq!(info["entries"], 1);
    assert!(
        !nemesis_path::legacy_models_catalog_cache_path(dir.path()).exists(),
        "legacy 文件已搬走"
    );
    assert!(nemesis_path::models_catalog_cache_path(dir.path()).exists());
}

/// `list` must surface the raw extras (null when absent) and the exact-key
/// catalog hit — the data the attribute editor renders. Skips when a
/// process-global ConfigStore is installed (load_live would shadow the file),
/// mirroring the sandbox test-isolation stance.
#[test]
fn list_attaches_extras_and_catalog_match() {
    if nemesis_config::global().is_some() {
        eprintln!("skip list_attaches_extras: process-global ConfigStore installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let home = home_str(&dir);

    // Seed a typed-valid config via the real serializer, then raw-inject the
    // extras exactly like the CLI does.
    let mut cfg = nemesis_config::Config::default();
    cfg.model_list.push(nemesis_config::ModelConfig {
        model_name: "m1".to_string(),
        model: "qwen/qwen3-30b".to_string(),
        api_base: String::new(),
        api_key: "sk-secret".to_string(),
        proxy: String::new(),
        auth_method: String::new(),
        connect_mode: String::new(),
        workspace: String::new(),
        reasoning_effort: String::new(),
    });
    cfg.model_list.push(nemesis_config::ModelConfig {
        model_name: "m2".to_string(),
        model: "test/testai-1.1".to_string(),
        api_base: String::new(),
        api_key: String::new(),
        proxy: String::new(),
        auth_method: String::new(),
        connect_mode: String::new(),
        workspace: String::new(),
        reasoning_effort: String::new(),
    });
    nemesis_config::save_config(&dir.path().join("config.json"), &mut cfg).unwrap();
    {
        let mut v = read_config_raw(dir.path());
        v["model_list"][0]["model_tier"] = serde_json::json!("mini");
        v["model_list"][0]["real_name"] = serde_json::json!("Qwen3-30B");
        std::fs::write(
            dir.path().join("config.json"),
            serde_json::to_string_pretty(&v).unwrap(),
        )
        .unwrap();
    }

    // Catalog cache with an entry whose key EXACTLY matches m1's `model`.
    // 真相源 = nemesis_path::models_catalog_cache_path（与 CLI 写盘位置同源）。
    let seeded = nemesis_path::models_catalog_cache_path(dir.path());
    std::fs::create_dir_all(seeded.parent().unwrap()).unwrap();
    std::fs::write(
        seeded,
        r#"{ "fetched_at": "2026-08-24T00:00:00Z", "entries": [
            { "key": "qwen/qwen3-30b", "context_window": 131072,
              "max_output_tokens": 8192, "family": "qwen" },
            { "key": "other/model", "context_window": 4096 }
        ] }"#,
    )
    .unwrap();

    let h = ModelsHandler::new();
    let out = h.list(&home).unwrap().unwrap();
    let models = out["models"].as_array().unwrap();
    assert_eq!(models.len(), 2);

    let m1 = &models[0];
    assert_eq!(m1["model_name"], "m1");
    assert_eq!(m1["model_tier"], "mini");
    assert_eq!(m1["real_name"], "Qwen3-30B");
    assert_eq!(m1["catalog_match"]["context_window"], 131072);
    assert_eq!(m1["catalog_match"]["max_output_tokens"], 8192);
    assert_eq!(m1["catalog_match"]["family"], "qwen");

    // Bare entry: extras null, no catalog hit, api_key masked.
    let m2 = &models[1];
    assert_eq!(m2["model_tier"], serde_json::Value::Null);
    assert_eq!(m2["model_size_b"], serde_json::Value::Null);
    assert_eq!(m2["context_window"], serde_json::Value::Null);
    assert_eq!(m2["catalog_match"], serde_json::Value::Null);
}

// ---------------------------------------------------------------------------
// Raw-RMW migration (P3-2): add/delete/set_default must NOT erase the raw
// extras — the reason they no longer go through a typed save.
// ---------------------------------------------------------------------------

/// RequestContext for set_default (agent_loop None → no runtime swap; only
/// the file effect is asserted). Same dual-decl pattern as
/// handlers/logs/history_tests.rs make_ctx.
fn make_ctx(dir: &tempfile::TempDir) -> crate::ws_router::RequestContext {
    use crate::api_handlers::AppState;
    use crate::events::EventHub;
    use crate::session::SessionManager;
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::time::Instant;

    let ws = dir.path().to_string_lossy().to_string();
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
        memory_manager: None,
        forge: None,
        agent_loop: Arc::new(parking_lot::RwLock::new(None)),
        cluster: None,
        cluster_service: None,
        cluster_log_dir: None,
        workflow_engine: None,
        #[cfg(feature = "workflow")]
        chat_secret_store: std::sync::Arc::new(
            nemesis_workflow::chat_secrets::ChatSecretStore::in_memory(),
        ),
        #[cfg(not(feature = "workflow"))]
        chat_secret_store: std::sync::Arc::new(()),
        #[cfg(feature = "workflow")]
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        #[cfg(not(feature = "workflow"))]
        webhook_rate_limiter: Arc::new(()),
        internal_cmd_tx: None,
        estop: None,
        cron: None,
        board: None,
    });
    crate::ws_router::RequestContext {
        session_id: "test-session".to_string(),
        chat_id: "test-chat".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

fn write_catalog(home: &std::path::Path, entries_json: &str) {
    // 真相源 = nemesis_path::models_catalog_cache_path（与 CLI 共用，见
    // models.rs catalog_info 注释）。
    let path = nemesis_path::models_catalog_cache_path(home);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        path,
        format!("{{ \"fetched_at\": \"t\", \"entries\": {entries_json} }}"),
    )
    .unwrap();
}

#[test]
fn add_preserves_sibling_extras_and_autofills_catalog() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), SEED);
    write_catalog(
        dir.path(),
        r#"[{ "key": "openai/gpt-5", "context_window": 400000, "max_output_tokens": 128000 }]"#,
    );
    let h = ModelsHandler::new();

    h.add(
        &home_str(&dir),
        &serde_json::json!({ "name": "gpt5", "model": "openai/gpt-5", "key": "sk-5" }),
    )
    .unwrap()
    .unwrap();

    let cfg = read_config_raw(dir.path());
    let list = cfg["model_list"].as_array().unwrap();
    assert_eq!(list.len(), 3);
    // m1's raw extras survived the add (the old typed save dropped them).
    let m1 = &list[0];
    assert_eq!(m1["model_tier"], "auto");
    assert_eq!(m1["real_name"], "Qwen3-30B");
    assert_eq!(m1["custom_note"], "keep-me");
    // New entry: CLI parity — explicit auto tier + catalog auto-fill.
    let new = &list[2];
    assert_eq!(new["model_name"], "gpt5");
    assert_eq!(new["model_tier"], "auto");
    assert_eq!(new["context_window"], 400000);
    assert_eq!(new["max_output_tokens"], 128000);
    assert_eq!(new["api_key"], "sk-5");

    // Duplicate model_name still rejected.
    assert!(h
        .add(
            &home_str(&dir),
            &serde_json::json!({ "name": "gpt5", "model": "openai/gpt-5", "key": "x" })
        )
        .is_err());
}

#[test]
fn delete_preserves_survivor_extras() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), SEED);
    let h = ModelsHandler::new();

    h.delete(&home_str(&dir), "m2").unwrap().unwrap();
    let cfg = read_config_raw(dir.path());
    let list = cfg["model_list"].as_array().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["model_name"], "m1");
    assert_eq!(list[0]["model_tier"], "auto");
    assert_eq!(list[0]["real_name"], "Qwen3-30B");
    assert_eq!(list[0]["custom_note"], "keep-me");
}

#[test]
fn set_default_reorders_and_preserves_extras() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), SEED);
    let ctx = make_ctx(&dir);
    let h = ModelsHandler::new();

    h.set_default(&home_str(&dir), "m2", &ctx).unwrap().unwrap();
    let cfg = read_config_raw(dir.path());
    let list = cfg["model_list"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0]["model_name"], "m2", "default moves to head");
    assert_eq!(cfg["agents"]["defaults"]["llm"], "m2");
    // m1's extras survive the reorder (the old typed save dropped them).
    assert_eq!(list[1]["model_tier"], "auto");
    assert_eq!(list[1]["real_name"], "Qwen3-30B");
    assert_eq!(list[1]["custom_note"], "keep-me");
}

// ============================================================
// Phase 3 覆盖率补测（2026-08-25）：delete 默认模型拒绝臂、
// set_default 的运行时 provider 热切（含 api_base 推断/显式/失败
// 三臂 + Forge 桥）、real_name trim 落盘、catalog 无 key 条目跳过。
// ============================================================

#[test]
fn delete_default_model_refused() {
    let dir = tempfile::tempdir().unwrap();
    // SEED + agents.defaults.llm 指向 m1 → 删除 m1 必须被拒。
    let mut v: serde_json::Value = serde_json::from_str(SEED).unwrap();
    v["agents"]["defaults"]["llm"] = serde_json::json!("m1");
    std::fs::write(
        dir.path().join("config.json"),
        serde_json::to_string_pretty(&v).unwrap(),
    )
    .unwrap();
    let h = ModelsHandler::new();

    let err = h.delete(&home_str(&dir), "m1").unwrap_err();
    assert!(
        err.contains("cannot delete default model"),
        "got: {err}"
    );
    // 文件不动。
    let cfg = read_config_raw(dir.path());
    assert_eq!(cfg["model_list"].as_array().unwrap().len(), 2);
}

// --- runtime swap：真 AgentLoop ---

struct SwapProvider;
#[async_trait::async_trait]
impl nemesis_agent::r#loop::LlmProvider for SwapProvider {
    async fn chat(
        &self,
        _: &str,
        _: Vec<nemesis_agent::r#loop::LlmMessage>,
        _: Option<nemesis_agent::types::ChatOptions>,
        _: Vec<nemesis_agent::types::ToolDefinition>,
    ) -> Result<nemesis_agent::r#loop::LlmResponse, String> {
        Ok(nemesis_agent::r#loop::LlmResponse {
            content: String::new(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

// 注意：这里不接 forge 参数 —— nemesis-forge 是可选依赖，窄 feature 构建下
// `nemesis_forge` 命名空间不存在，签名一旦引用类型就编不过。forge 专属测试
// 自行组 ctx（见 set_default_syncs_forge_provider_bridge）。
fn ctx_with_loop(dir: &tempfile::TempDir) -> crate::ws_router::RequestContext {
    let mut ctx = make_ctx(dir);
    let al = nemesis_agent::r#loop::AgentLoop::new(
        Box::new(SwapProvider),
        nemesis_agent::types::AgentConfig::default(),
    );
    let state = crate::api_handlers::AppState {
        agent_loop: Arc::new(parking_lot::RwLock::new(Some(Arc::new(al)))),
        ..(*ctx.state).clone()
    };
    ctx.state = Arc::new(state);
    ctx
}

#[test]
fn set_default_with_live_loop_swaps_runtime_provider() {
    let dir = tempfile::tempdir().unwrap();
    write_config(
        dir.path(),
        r#"{ "model_list": [
            { "model_name": "zm", "model": "zhipu/glm-4.7-flash", "api_key": "sk" },
            { "model_name": "em", "model": "x/y", "api_key": "sk", "api_base": "http://127.0.0.1:9" }
        ] }"#,
    );
    let h = ModelsHandler::new();

    // 臂 1：api_base 空 → infer_provider("zhipu/...") 推默认 base。
    let ctx = ctx_with_loop(&dir);
    h.set_default(&home_str(&dir), "zm", &ctx).unwrap().unwrap();
    assert_eq!(
        read_config_raw(dir.path())["agents"]["defaults"]["llm"],
        "zm"
    );

    // 臂 2：显式 api_base 原样使用。
    let ctx = ctx_with_loop(&dir);
    h.set_default(&home_str(&dir), "em", &ctx).unwrap().unwrap();
    assert_eq!(
        read_config_raw(dir.path())["agents"]["defaults"]["llm"],
        "em"
    );

    // 臂 3：create_provider 失败（未知 provider 无 base）→ warn 但配置已保存。
    write_config(
        dir.path(),
        r#"{ "model_list": [
            { "model_name": "zm", "model": "zhipu/glm-4.7-flash", "api_key": "sk" },
            { "model_name": "em", "model": "x/y", "api_key": "sk", "api_base": "http://127.0.0.1:9" },
            { "model_name": "uk", "model": "??/totally-unknown", "api_key": "sk" }
        ] }"#,
    );
    let ctx = ctx_with_loop(&dir);
    h.set_default(&home_str(&dir), "uk", &ctx).unwrap().unwrap();
    assert_eq!(
        read_config_raw(dir.path())["agents"]["defaults"]["llm"],
        "uk",
        "provider 创建失败不能吞掉配置写入"
    );
}

#[cfg(feature = "forge")]
#[test]
fn set_default_syncs_forge_provider_bridge() {
    let dir = tempfile::tempdir().unwrap();
    write_config(
        dir.path(),
        r#"{ "model_list": [
            { "model_name": "zm", "model": "zhipu/glm-4.7-flash", "api_key": "sk" }
        ] }"#,
    );
    let forge = Arc::new(nemesis_forge::forge::Forge::new(
        <nemesis_forge::config::ForgeConfig as Default>::default(),
        dir.path().to_path_buf(),
    ));
    // forge 测试自组 ctx（helper 不接 forge 参数，见其注释）。
    let mut ctx = make_ctx(&dir);
    let al = nemesis_agent::r#loop::AgentLoop::new(
        Box::new(SwapProvider),
        nemesis_agent::types::AgentConfig::default(),
    );
    let state = crate::api_handlers::AppState {
        agent_loop: Arc::new(parking_lot::RwLock::new(Some(Arc::new(al)))),
        forge: Some(forge),
        ..(*ctx.state).clone()
    };
    ctx.state = Arc::new(state);
    let h = ModelsHandler::new();
    // 走完 Forge 桥分支不 panic + 配置照常落盘。
    h.set_default(&home_str(&dir), "zm", &ctx).unwrap().unwrap();
    assert_eq!(
        read_config_raw(dir.path())["agents"]["defaults"]["llm"],
        "zm"
    );
}

#[test]
fn real_name_is_trimmed_on_update() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), SEED);
    let h = ModelsHandler::new();
    h.update_field(
        &home_str(&dir),
        &serde_json::json!({ "name": "m1", "field": "real_name", "value": "  Qwen3-30B  " }),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        read_config_raw(dir.path())["model_list"][0]["real_name"],
        "Qwen3-30B"
    );
}

#[test]
fn catalog_entries_without_key_are_skipped() {
    if nemesis_config::global().is_some() {
        eprintln!("skip catalog_entries_without_key: process-global ConfigStore installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), SEED);
    // 无 key 条目必须被 read_catalog 跳过，不影响同批合法条目命中。
    write_catalog(
        dir.path(),
        r#"[
            { "context_window": 999 },
            { "key": "qwen/qwen3-30b", "context_window": 131072 }
        ]"#,
    );
    let h = ModelsHandler::new();
    let out = h.list(&home_str(&dir)).unwrap().unwrap();
    let m1 = &out["models"][0];
    assert_eq!(m1["catalog_match"]["context_window"], 131072);
}

// ---------------------------------------------------------------------------
// G4 (U15)：key_source 来源徽标 —— list 必须带 key_source（kind + ref），
// 四种来源各就位，且响应中不泄露明文 key。
// ---------------------------------------------------------------------------

/// G4：env:/yaml:/inline/空 四种 api_key 形态 → classify_key_source 映射正确，
/// ref 只携带引用名（VAR 名 / alias），绝不携带明文。
#[test]
fn list_key_source_covers_all_four_kinds() {
    if nemesis_config::global().is_some() {
        eprintln!("skip list_key_source: process-global ConfigStore installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    write_config(
        dir.path(),
        r#"{
          "model_list": [
            { "model_name": "m-env", "model": "a/one", "api_key": "env:ZHIPU_API_KEY" },
            { "model_name": "m-yaml", "model": "a/two", "api_key": "yaml:zhipu" },
            { "model_name": "m-inline", "model": "a/three", "api_key": "sk-plain-secret-9999" },
            { "model_name": "m-none", "model": "a/four", "api_key": "" }
          ]
        }"#,
    );
    let h = ModelsHandler::new();
    let out = h.list(&home_str(&dir)).unwrap().unwrap();
    let models = out["models"].as_array().unwrap();
    assert_eq!(models.len(), 4);

    assert_eq!(models[0]["key_source"]["kind"], "env");
    assert_eq!(models[0]["key_source"]["ref"], "ZHIPU_API_KEY");
    assert_eq!(models[1]["key_source"]["kind"], "yaml");
    assert_eq!(models[1]["key_source"]["ref"], "zhipu");
    assert_eq!(models[2]["key_source"]["kind"], "inline");
    // inline 无引用名；且响应整体不泄露明文（api_key 已 mask）。
    assert_eq!(models[2]["key_source"]["ref"], "");
    assert_eq!(models[3]["key_source"]["kind"], "none");

    let out_str = serde_json::to_string(&out).unwrap();
    assert!(!out_str.contains("sk-plain-secret-9999"));
    // ref 字段不能叫 "reference"（serde rename 钉住 wire 契约）。
    assert!(models[0]["key_source"].get("ref").is_some());
    assert!(models[0]["key_source"].get("reference").is_none());
}
