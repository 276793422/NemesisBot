//! Memory handler — status/documents/vector operations + enhanced memory management.
//!
//! Commands: status, documents, document.get, document.save,
//!           env.check, env.setup, config.get, config.set,
//!           stats, entries.list, entries.search, entries.store,
//!           model.install

use crate::handlers::{
    get_opt_bool_loud, get_opt_str_loud, get_opt_u64_loud, read_workspace_file, require_home,
    require_workspace, resolve_path, write_workspace_file,
};
use crate::ws_router::{ModuleHandler, RequestContext};
use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub struct MemoryHandler;

#[async_trait::async_trait]
impl ModuleHandler for MemoryHandler {
    fn module_name(&self) -> &str {
        "memory"
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let workspace = require_workspace(ctx)?;
        let home = require_home(ctx)?;
        let config_dir = nemesis_path::workspace_config_dir(Path::new(workspace));

        match cmd {
            // --- Document memory (original) ---
            "status" => self.status(workspace, home),
            "documents" => self.documents(workspace),
            "document.get" => {
                let data = data.ok_or("missing data")?;
                let path = crate::handlers::get_str(&data, "path")?;
                self.document_get(workspace, &path)
            }
            "document.save" => {
                let data = data.ok_or("missing data")?;
                let path = crate::handlers::get_str(&data, "path")?;
                let content = crate::handlers::get_str(&data, "content")?;
                self.document_save(workspace, &path, &content)
            }

            // --- Enhanced memory: environment ---
            "env.check" => self.env_check(&config_dir, home),
            "env.setup" => self.env_setup(&config_dir, home, ctx).await,

            // --- Enhanced memory: configuration ---
            "config.get" => self.config_get(&config_dir, home),
            "config.set" => {
                let data = data.ok_or("missing data")?;
                self.config_set(&config_dir, home, &data, ctx)
            }

            // --- Enhanced memory: statistics & entries ---
            "stats" => self.stats(&config_dir, workspace),
            // 分页：缺省 offset=0 / limit=100（旧调用方零改动兼容）。
            "entries.list" => {
                let offset = data
                    .as_ref()
                    .and_then(|d| d.get("offset"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize;
                let limit = data
                    .as_ref()
                    .and_then(|d| d.get("limit"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(100) as usize;
                self.entries_list(workspace, offset, limit)
            }
            "entries.search" => {
                let data = data.ok_or("missing data")?;
                let query = crate::handlers::get_str(&data, "query")?;
                let limit = data.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
                self.entries_search(workspace, &query, limit, ctx).await
            }
            "entries.store" => {
                let data = data.ok_or("missing data")?;
                let content = crate::handlers::get_str(&data, "content")?;
                self.entries_store(workspace, &content, ctx).await
            }
            // 条目管理（自动记忆注入 TAB）：编辑必须取全量内容（list 截断
            // 200 字符），删除/更新配合前端的行内管理操作。
            "entries.get" => {
                let data = data.ok_or("missing data")?;
                let id = crate::handlers::get_str(&data, "id")?;
                self.entries_get(workspace, &id, ctx).await
            }
            "entries.delete" => {
                let data = data.ok_or("missing data")?;
                let id = crate::handlers::get_str(&data, "id")?;
                self.entries_delete(workspace, &id, ctx).await
            }
            "entries.update" => {
                let data = data.ok_or("missing data")?;
                let id = crate::handlers::get_str(&data, "id")?;
                let content = crate::handlers::get_str(&data, "content")?;
                self.entries_update(workspace, &id, &content, ctx).await
            }

            // --- Enhanced memory: model management ---
            "model.install" => {
                let data = data.ok_or("missing data")?;
                let tier = crate::handlers::get_str(&data, "tier")?;
                self.model_install(&config_dir, &tier, ctx).await
            }

            // --- Legacy (kept for compatibility) ---
            "vector.status" => self.vector_status(workspace),
            "vector.search" => {
                let data = data.ok_or("missing data")?;
                let query = crate::handlers::get_str(&data, "query")?;
                self.entries_search(workspace, &query, 10, ctx).await
            }

            _ => Err(format!("unknown command: memory.{}", cmd)),
        }
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Auto-detect plugin library path next to the current executable.
fn detect_plugin_path() -> Option<String> {
    nemesis_utils::find_plugin_library("plugin_onnx").map(|p| p.to_string_lossy().to_string())
}

/// Vector-store JSONL path — the MemoryManager's own persistence file.
///
/// MUST stay in lockstep with the gateway's manager construction
/// (`gateway.rs`: `memory_data_dir = <workspace>/memory_vector`, store
/// persists at `data_dir/vector/vector_store.jsonl`). The pre-fix handler
/// read/wrote `<workspace>/memory/vector/` instead — a parallel tree no
/// reader ever loaded, so Dashboard-stored entries were invisible to the
/// agent's memory_search and the auto-inject prefetch.
fn vector_store_jsonl_path(workspace: &str) -> PathBuf {
    PathBuf::from(workspace)
        .join("memory_vector")
        .join("vector")
        .join("vector_store.jsonl")
}

/// One-time migration: entries stored through the pre-fix Dashboard wrote to
/// `<workspace>/memory/vector/vector_store.jsonl`. Copy them to the manager's
/// load path (only when the target doesn't exist yet) so the path fix doesn't
/// orphan them — without this the fixed entries.list would show an empty list
/// for data the user can still see under the old tree.
fn migrate_legacy_vector_store(workspace: &str) {
    let legacy = PathBuf::from(workspace)
        .join("memory")
        .join("vector")
        .join("vector_store.jsonl");
    let target = vector_store_jsonl_path(workspace);
    if !legacy.is_file() || target.exists() {
        return;
    }
    let Some(parent) = target.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    match std::fs::copy(&legacy, &target) {
        Ok(_) => tracing::info!(
            "[Memory] migrated legacy vector store {} -> {}",
            legacy.display(),
            target.display()
        ),
        Err(e) => tracing::warn!("[Memory] legacy vector store migration failed: {}", e),
    }
}

/// Read the `memory.enabled` field from the main config.json.
fn read_main_switch(home: &str) -> bool {
    let cfg_path = PathBuf::from(home).join("config.json");
    if !cfg_path.exists() {
        return false;
    }
    std::fs::read_to_string(&cfg_path)
        .ok()
        .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
        .and_then(|v| {
            v.get("memory")
                .and_then(|m| m.get("enabled"))
                .and_then(|e| e.as_bool())
        })
        .unwrap_or(false)
}

/// Write the `memory.enabled` field in the main config.json.
fn set_main_switch(home: &str, enabled: bool) -> Result<(), String> {
    let cfg_path = PathBuf::from(home).join("config.json");
    if !cfg_path.exists() {
        return Err(format!("config.json not found at {}", cfg_path.display()));
    }
    let content =
        std::fs::read_to_string(&cfg_path).map_err(|e| format!("failed to read config: {}", e))?;
    let mut cfg: serde_json::Value =
        serde_json::from_str(&content).map_err(|e| format!("failed to parse config: {}", e))?;
    if cfg.get("memory").is_none() {
        cfg.as_object_mut()
            .map(|o| o.insert("memory".to_string(), serde_json::json!({})));
    }
    if let Some(mem) = cfg.get_mut("memory").and_then(|m| m.as_object_mut()) {
        mem.insert("enabled".to_string(), serde_json::Value::Bool(enabled));
    }
    let updated = serde_json::to_string_pretty(&cfg)
        .map_err(|e| format!("failed to serialize config: {}", e))?;
    std::fs::write(&cfg_path, updated).map_err(|e| format!("failed to write config: {}", e))?;
    Ok(())
}

/// Per-model install lock to prevent concurrent downloads.
fn install_locks() -> &'static std::sync::Mutex<HashSet<String>> {
    static INSTANCE: OnceLock<std::sync::Mutex<HashSet<String>>> = OnceLock::new();
    INSTANCE.get_or_init(|| std::sync::Mutex::new(HashSet::new()))
}

/// Global Mutex for JSONL append writes.
fn jsonl_write_lock() -> &'static std::sync::Mutex<()> {
    static INSTANCE: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    INSTANCE.get_or_init(|| std::sync::Mutex::new(()))
}

// ---------------------------------------------------------------------------
// Document memory (original commands)
// ---------------------------------------------------------------------------

impl MemoryHandler {
    fn status(&self, workspace: &str, home: &str) -> Result<Option<serde_json::Value>, String> {
        let memory_dir = PathBuf::from(workspace).join("memory");
        let doc_count = if memory_dir.exists() {
            count_files_recursive(&memory_dir)
        } else {
            0
        };

        let em_config_path =
            nemesis_path::resolve_enhanced_memory_config_path_in_workspace(Path::new(workspace));
        let vector_enabled = if em_config_path.exists() {
            nemesis_memory::vector::embedding_config::load_embedding_config(
                &PathBuf::from(workspace).join("config"),
            )
            .enabled
        } else {
            false
        };

        let main_enabled = read_main_switch(home);

        Ok(Some(serde_json::json!({
            "document_memory": {
                "enabled": true,
                "document_count": doc_count,
                "directory_exists": memory_dir.exists(),
            },
            "vector_memory": {
                "enabled": vector_enabled,
                "main_enabled": main_enabled,
            },
        })))
    }

    fn documents(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let memory_dir = PathBuf::from(workspace).join("memory");
        if !memory_dir.exists() {
            return Ok(Some(serde_json::json!({ "documents": [] })));
        }

        let mut docs = Vec::new();
        collect_files(workspace, "memory", &mut docs)?;
        Ok(Some(serde_json::json!({ "documents": docs })))
    }

    fn document_get(
        &self,
        workspace: &str,
        path: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        let content = read_workspace_file(workspace, path)?;
        Ok(Some(serde_json::json!({
            "path": path,
            "content": content,
        })))
    }

    fn document_save(
        &self,
        workspace: &str,
        path: &str,
        content: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        write_workspace_file(workspace, path, content)?;
        Ok(Some(serde_json::json!({ "saved": true, "path": path })))
    }

    fn vector_status(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let config_dir = nemesis_path::workspace_config_dir(Path::new(workspace));
        let emb_cfg = nemesis_memory::vector::embedding_config::load_embedding_config(&config_dir);
        Ok(Some(serde_json::json!({ "enabled": emb_cfg.enabled })))
    }
}

// ---------------------------------------------------------------------------
// Enhanced memory: environment
// ---------------------------------------------------------------------------

impl MemoryHandler {
    fn env_check(
        &self,
        config_dir: &Path,
        home: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        let plugin = detect_plugin_path();
        let main_switch = read_main_switch(home);

        // Load unified embedding config (contains enabled + models)
        let emb_cfg = nemesis_memory::vector::embedding_config::load_embedding_config(config_dir);
        let sub_switch = emb_cfg.enabled;
        let active_tier = emb_cfg.active.clone();
        let emb_data_dir = nemesis_memory::vector::embedding_config::embedding_data_dir(config_dir);

        let mut models = serde_json::Map::new();
        for tier in &["large", "medium", "small"] {
            if let Some(mc) = emb_cfg.models.get(tier) {
                let model_dir = emb_data_dir.join(&mc.name);
                let model_file = model_dir.join("model.onnx");
                let tokenizer_file = model_dir.join("tokenizer.json");

                // Also check local_model_path if set
                let model_ready = if !mc.local_model_path.is_empty()
                    && std::path::Path::new(&mc.local_model_path).exists()
                {
                    true
                } else {
                    model_file.exists()
                };
                let tokenizer_ready = if !mc.local_tokenizer_path.is_empty()
                    && std::path::Path::new(&mc.local_tokenizer_path).exists()
                {
                    true
                } else {
                    tokenizer_file.exists()
                };

                models.insert(
                    tier.to_string(),
                    serde_json::json!({
                        "name": mc.name,
                        "dimension": mc.dimension,
                        "model_ready": model_ready,
                        "tokenizer_ready": tokenizer_ready,
                        "model_size": mc.model_size,
                    }),
                );
            }
        }

        // Overall status
        let active_model_ready = emb_cfg
            .models
            .get(&active_tier)
            .map(|mc| {
                if !mc.local_model_path.is_empty()
                    && std::path::Path::new(&mc.local_model_path).exists()
                {
                    true
                } else {
                    emb_data_dir.join(&mc.name).join("model.onnx").exists()
                }
            })
            .unwrap_or(false);

        let overall = if !main_switch {
            "disabled"
        } else if !sub_switch || plugin.is_none() || !active_model_ready {
            "degraded"
        } else {
            "ready"
        };

        Ok(Some(serde_json::json!({
            "plugin": {
                "found": plugin.is_some(),
                "path": plugin.unwrap_or_default(),
            },
            "main_switch": main_switch,
            "sub_switch": sub_switch,
            "active_tier": active_tier,
            "models": models,
            "overall": overall,
        })))
    }

    async fn env_setup(
        &self,
        config_dir: &Path,
        home: &str,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let hub = ctx.state.event_hub.clone();
        let config_dir_clone = config_dir.to_path_buf();
        let home_owned = home.to_string();

        let result = tokio::task::spawn_blocking(move || {
            // 1. Check plugin
            let _plugin_path = detect_plugin_path().ok_or_else(|| {
                hub.publish(
                    "memory-setup",
                    serde_json::json!({
                        "status": "error", "message": "Plugin not found"
                    }),
                );
                let filename = nemesis_utils::plugin_library_filename("plugin_onnx");
                format!("Plugin not found at {{exe_dir}}/plugins/{}", filename)
            })?;

            hub.publish(
                "memory-setup",
                serde_json::json!({
                    "status": "starting", "message": "正在准备模型文件..."
                }),
            );

            // 2. Download model files
            let mut emb_cfg =
                nemesis_memory::vector::embedding_config::load_embedding_config(&config_dir_clone);
            let (_model_dir, _dim) =
                nemesis_memory::vector::embedding_config::download_model_files(
                    &mut emb_cfg,
                    &config_dir_clone,
                )
                .map_err(|e| {
                    hub.publish(
                        "memory-setup",
                        serde_json::json!({
                            "status": "error", "message": format!("模型下载失败: {}", e)
                        }),
                    );
                    e
                })?;
            nemesis_memory::vector::embedding_config::save_embedding_config(
                &emb_cfg,
                &config_dir_clone,
            );

            // 3. Write enabled=true to unified config
            let mut emb_cfg =
                nemesis_memory::vector::embedding_config::load_embedding_config(&config_dir_clone);
            emb_cfg.enabled = true;
            nemesis_memory::vector::embedding_config::save_embedding_config(
                &emb_cfg,
                &config_dir_clone,
            );

            hub.publish(
                "memory-setup",
                serde_json::json!({
                    "status": "complete", "message": "一键安装完成"
                }),
            );

            Ok::<(), String>(())
        })
        .await
        .map_err(|e| format!("setup task panicked: {}", e))?;

        result?;

        // 4. Set main switch
        set_main_switch(&home_owned, true)?;

        Ok(Some(serde_json::json!({ "success": true })))
    }
}

// ---------------------------------------------------------------------------
// Enhanced memory: configuration
// ---------------------------------------------------------------------------

impl MemoryHandler {
    fn config_get(
        &self,
        config_dir: &Path,
        home: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        let main_enabled = read_main_switch(home);

        // Load unified config (contains enabled + models + active tier)
        let emb_cfg = nemesis_memory::vector::embedding_config::load_embedding_config(config_dir);
        let sub_enabled = emb_cfg.enabled;
        let active_tier = emb_cfg.active.clone();

        // Read raw config file content
        let emb_path = config_dir.join("config.enhanced_memory.json");
        let embedding_config_content = std::fs::read_to_string(&emb_path).unwrap_or_default();

        Ok(Some(serde_json::json!({
            "main_enabled": main_enabled,
            "sub_enabled": sub_enabled,
            "active_tier": active_tier,
            "similarity_threshold": 0.7,
            "max_results": 10,
            // P1-1 (2026-08-24 UI entry gap): auto-inject flags for the
            // "自动记忆注入" card. Read by agent_factory at AgentLoop build
            // time — changes take effect after the Agent restarts.
            "auto_inject": emb_cfg.auto_inject,
            "auto_inject_top_k": emb_cfg.auto_inject_top_k,
            "embedding_config_content": embedding_config_content,
        })))
    }

    fn config_set(
        &self,
        config_dir: &Path,
        home: &str,
        data: &serde_json::Value,
        ctx: &crate::ws_router::RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        // 2026-08-24 re-review: every switch key below validates loudly
        // (present-but-mistyped → Err, absent/null → unchanged) instead of
        // silently ignoring wrong types — sandbox `set_config` convention.
        // Main switch
        if let Some(enabled) = get_opt_bool_loud(data, "main_enabled")? {
            set_main_switch(home, enabled)?;
            if !enabled && let Some(mgr) = ctx.state.memory_manager.as_ref() {
                mgr.set_vector_enabled(false);
            }
        }

        // Sub switch (write via unified config + runtime control)
        if let Some(enabled) = get_opt_bool_loud(data, "sub_enabled")? {
            if enabled {
                // Check model files before enabling
                let emb_cfg =
                    nemesis_memory::vector::embedding_config::load_embedding_config(config_dir);
                let emb_data_dir =
                    nemesis_memory::vector::embedding_config::embedding_data_dir(config_dir);
                let model_ready = emb_cfg
                    .models
                    .get(&emb_cfg.active)
                    .map(|mc| {
                        if !mc.local_model_path.is_empty()
                            && std::path::Path::new(&mc.local_model_path).exists()
                        {
                            true
                        } else {
                            emb_data_dir.join(&mc.name).join("model.onnx").exists()
                        }
                    })
                    .unwrap_or(false);
                if !model_ready {
                    return Err("当前激活的模型尚未下载，请先安装模型后再启用强化记忆".to_string());
                }
            }
            let mut emb_cfg =
                nemesis_memory::vector::embedding_config::load_embedding_config(config_dir);
            emb_cfg.enabled = enabled;
            nemesis_memory::vector::embedding_config::save_embedding_config(&emb_cfg, config_dir);
            // Runtime control
            if let Some(mgr) = ctx.state.memory_manager.as_ref() {
                if enabled {
                    if let Err(e) = mgr.init_vector_store_from_config(config_dir) {
                        // Init failed → rollback config
                        let mut emb_cfg =
                            nemesis_memory::vector::embedding_config::load_embedding_config(
                                config_dir,
                            );
                        emb_cfg.enabled = false;
                        nemesis_memory::vector::embedding_config::save_embedding_config(
                            &emb_cfg, config_dir,
                        );
                        return Err(format!("向量存储初始化失败: {}", e));
                    }
                } else {
                    mgr.set_vector_enabled(false);
                }
            }
        }

        // Active tier
        if let Some(tier) = get_opt_str_loud(data, "active_tier")? {
            let mut emb_cfg =
                nemesis_memory::vector::embedding_config::load_embedding_config(config_dir);
            emb_cfg.active = tier.to_string();
            nemesis_memory::vector::embedding_config::save_embedding_config(&emb_cfg, config_dir);
        }

        // P1-1 (2026-08-24 UI entry gap): auto-inject flags. Persisted into
        // config.enhanced_memory.json (same EmbeddingConfig the factory
        // reads). NOT runtime-hot: agent_factory reads them once at
        // AgentLoop build time (set_memory_inject), so the UI card tells the
        // user to restart the Agent (agent.stop → agent.start) after saving.
        if let Some(v) = get_opt_bool_loud(data, "auto_inject")? {
            let mut emb_cfg =
                nemesis_memory::vector::embedding_config::load_embedding_config(config_dir);
            emb_cfg.auto_inject = v;
            nemesis_memory::vector::embedding_config::save_embedding_config(&emb_cfg, config_dir);
        }
        if let Some(v) = get_opt_u64_loud(data, "auto_inject_top_k")? {
            // Range check (1..=10): values outside would either disable the
            // feature (0) or blow the prompt budget (huge). Wrong types are
            // already rejected by get_opt_u64_loud above.
            if !(1..=10).contains(&v) {
                return Err(format!("auto_inject_top_k 必须在 1-10 之间（收到 {}）", v));
            }
            let mut emb_cfg =
                nemesis_memory::vector::embedding_config::load_embedding_config(config_dir);
            emb_cfg.auto_inject_top_k = v as usize;
            nemesis_memory::vector::embedding_config::save_embedding_config(&emb_cfg, config_dir);
        }

        // Embedding config content (full overwrite of config.enhanced_memory.json)
        if let Some(content) = get_opt_str_loud(data, "embedding_config_content")? {
            let emb_path = config_dir.join("config.enhanced_memory.json");
            std::fs::write(&emb_path, content)
                .map_err(|e| format!("write embedding config error: {}", e))?;
        }

        Ok(Some(serde_json::json!({ "updated": true })))
    }
}

// ---------------------------------------------------------------------------
// Enhanced memory: statistics & entries
// ---------------------------------------------------------------------------

impl MemoryHandler {
    fn stats(
        &self,
        config_dir: &Path,
        workspace: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        let memory_dir = PathBuf::from(workspace).join("memory");

        // Vector/episodic/graph live under the MemoryManager's data_dir
        // (`<workspace>/memory_vector/`, see gateway.rs) — NOT under
        // `<workspace>/memory/`. Pre-fix stats counted the wrong tree, so
        // real agent-written episodic/graph data showed as 0.
        migrate_legacy_vector_store(workspace);
        let mgr_dir = PathBuf::from(workspace).join("memory_vector");

        // Vector entries: count lines in vector_store.jsonl
        let vector_jsonl = mgr_dir.join("vector").join("vector_store.jsonl");
        let vector_entries = count_jsonl_lines(&vector_jsonl);

        // Episodic: count files under episodic/
        let episodic_dir = mgr_dir.join("episodic");
        let (episodic_sessions, episodic_episodes) = count_episodic(&episodic_dir);

        // Graph: count lines in entities.jsonl and triples.jsonl
        let graph_dir = mgr_dir.join("graph");
        let graph_entities = count_jsonl_lines(&graph_dir.join("entities.jsonl"));
        let graph_triples = count_jsonl_lines(&graph_dir.join("triples.jsonl"));

        // Memory entries: total files in memory/
        let memory_entries = if memory_dir.exists() {
            count_files_recursive(&memory_dir)
        } else {
            0
        };

        // Active tier and dimension from embedding config
        let emb_cfg = nemesis_memory::vector::embedding_config::load_embedding_config(config_dir);
        let active_tier = emb_cfg.active.clone();
        let vector_dimension = emb_cfg
            .models
            .get(&active_tier)
            .map(|mc| mc.dimension)
            .unwrap_or(0);

        Ok(Some(serde_json::json!({
            "memory_entries": memory_entries,
            "episodic_sessions": episodic_sessions,
            "episodic_episodes": episodic_episodes,
            "graph_entities": graph_entities,
            "graph_triples": graph_triples,
            "vector_entries": vector_entries,
            "vector_dimension": vector_dimension,
            "active_tier": active_tier,
        })))
    }

    fn entries_list(
        &self,
        workspace: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Option<serde_json::Value>, String> {
        migrate_legacy_vector_store(workspace);
        let jsonl_path = vector_store_jsonl_path(workspace);
        if !jsonl_path.exists() {
            return Ok(Some(serde_json::json!({ "entries": [], "total": 0 })));
        }

        let content = std::fs::read_to_string(&jsonl_path)
            .map_err(|e| format!("failed to read vector store: {}", e))?;

        let mut entries = Vec::new();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) {
                entries.push(truncate_entry_content(entry));
            }
        }

        let total = entries.len();
        // Return most recent first (last in file), then page.
        entries.reverse();
        let page: Vec<serde_json::Value> = entries.into_iter().skip(offset).take(limit).collect();

        Ok(Some(
            serde_json::json!({ "entries": page, "total": total, "offset": offset }),
        ))
    }

    async fn entries_search(
        &self,
        workspace: &str,
        query: &str,
        limit: usize,
        ctx: &crate::ws_router::RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        // Live MemoryManager with vector store active → semantic search
        // through the same index the agent's memory_search / auto-inject
        // prefetch use (single truth source). Falls back to the keyword
        // substring scan over the persisted JSONL below when no manager
        // exists (memory.enabled=false) or the vector store is off.
        #[cfg(feature = "memory")]
        if let Some(mgr) = ctx.state.memory_manager.as_ref()
            && mgr.is_vector_enabled()
        {
            let result = mgr
                .search(query, None, limit)
                .await
                .map_err(|e| format!("search error: {}", e))?;
            let results: Vec<serde_json::Value> = result
                .entries
                .iter()
                .map(|se| {
                    truncate_entry_content(serde_json::json!({
                        "id": se.entry.id,
                        "type": se.entry.typ.to_string(),
                        "content": se.entry.content,
                        "metadata": se.entry.metadata,
                        "tags": se.entry.tags,
                        "score": se.score,
                        "created_at": se.entry.created_at.to_rfc3339(),
                        "updated_at": se.entry.updated_at.to_rfc3339(),
                    }))
                })
                .collect();
            let total = results.len();
            return Ok(Some(serde_json::json!({
                "query": query, "results": results, "total": total, "search_type": "semantic"
            })));
        }

        migrate_legacy_vector_store(workspace);
        let jsonl_path = vector_store_jsonl_path(workspace);
        if !jsonl_path.exists() {
            return Ok(Some(serde_json::json!({
                "query": query, "results": [], "total": 0, "search_type": "keyword"
            })));
        }

        let content = std::fs::read_to_string(&jsonl_path)
            .map_err(|e| format!("failed to read vector store: {}", e))?;

        let query_lower = query.to_lowercase();
        let mut results: Vec<serde_json::Value> = Vec::new();

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) {
                let text = entry
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_lowercase();

                if text.contains(&query_lower) {
                    results.push(truncate_entry_content(entry));
                }
            }
        }

        let total = results.len();
        results.truncate(limit);

        Ok(Some(serde_json::json!({
            "query": query, "results": results, "total": total, "search_type": "keyword"
        })))
    }

    async fn entries_store(
        &self,
        workspace: &str,
        content: &str,
        ctx: &crate::ws_router::RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        // Live MemoryManager with vector store active → single truth source:
        // store_entry embeds the content, adds it to the in-memory index
        // (immediately visible to the agent's memory_search AND the auto-inject
        // prefetch) and persists via the adapter at the manager's own JSONL.
        // Pre-fix this command raw-appended to <workspace>/memory/vector/ —
        // a path no reader ever loaded.
        #[cfg(feature = "memory")]
        if let Some(mgr) = ctx.state.memory_manager.as_ref()
            && mgr.is_vector_enabled()
        {
            let entry = nemesis_memory::types::Entry::new(
                nemesis_memory::types::MemoryType::LongTerm,
                content.to_string(),
            );
            let id = mgr
                .store_entry(entry)
                .await
                .map_err(|e| format!("store entry error: {}", e))?;
            return Ok(Some(serde_json::json!({ "id": id, "stored": true })));
        }
        // Manager present but vector off → fall through to the raw
        // append: the in-memory general store (LocalStore) is not
        // persisted, so the JSONL at the load path is the only durable
        // copy — it is re-embedded when the sub-switch re-initializes
        // the vector store.

        migrate_legacy_vector_store(workspace);
        let jsonl_path = vector_store_jsonl_path(workspace);

        // Ensure directory exists
        if let Some(parent) = jsonl_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("failed to create dir: {}", e))?;
        }

        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Local::now().to_rfc3339();

        let entry = serde_json::json!({
            "id": id,
            "type": "long_term",
            "content": content,
            "metadata": {},
            "tags": [],
            "score": 0.0,
            "created_at": now,
            "updated_at": now,
        });

        let mut line =
            serde_json::to_string(&entry).map_err(|e| format!("serialize error: {}", e))?;
        line.push('\n');

        // Lock to prevent concurrent appends
        let _guard = jsonl_write_lock().lock().unwrap();
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&jsonl_path)
            .map_err(|e| format!("failed to open file: {}", e))?;
        file.write_all(line.as_bytes())
            .map_err(|e| format!("failed to write: {}", e))?;

        Ok(Some(serde_json::json!({ "id": id, "stored": true })))
    }

    /// Fetch ONE entry with FULL content (`entries.list` truncates content to
    /// 200 chars for display — editing must work on the true bytes).
    /// Manager online → `mgr.get`; otherwise read the JSONL directly (same
    /// durability reasoning as `entries_store`'s fallback path).
    async fn entries_get(
        &self,
        workspace: &str,
        id: &str,
        ctx: &crate::ws_router::RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        #[cfg(feature = "memory")]
        if let Some(mgr) = ctx.state.memory_manager.as_ref()
            && mgr.is_vector_enabled()
        {
            let entry = mgr
                .get(id)
                .await
                .map_err(|e| format!("get entry error: {}", e))?;
            let json = entry
                .map(|e| serde_json::to_value(&e))
                .transpose()
                .map_err(|e| format!("serialize entry error: {}", e))?;
            return Ok(Some(serde_json::json!({ "entry": json })));
        }

        migrate_legacy_vector_store(workspace);
        let jsonl_path = vector_store_jsonl_path(workspace);
        if !jsonl_path.exists() {
            return Ok(Some(serde_json::json!({ "entry": null })));
        }
        let content = std::fs::read_to_string(&jsonl_path)
            .map_err(|e| format!("failed to read vector store: {}", e))?;
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Ok(entry) = serde_json::from_str::<serde_json::Value>(trimmed)
                && entry.get("id").and_then(|v| v.as_str()) == Some(id)
            {
                return Ok(Some(serde_json::json!({ "entry": entry })));
            }
        }
        Ok(Some(serde_json::json!({ "entry": null })))
    }

    /// Delete one entry by id. Manager online with vector enabled → route
    /// through the manager (in-memory index + persistence, immediately
    /// invisible to memory_search AND the auto-inject prefetch); otherwise
    /// line-level rewrite of the JSONL (tmp + rename, same as the raw-append
    /// fallback's durability level).
    async fn entries_delete(
        &self,
        workspace: &str,
        id: &str,
        ctx: &crate::ws_router::RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        #[cfg(feature = "memory")]
        if let Some(mgr) = ctx.state.memory_manager.as_ref()
            && mgr.is_vector_enabled()
        {
            let deleted = mgr
                .delete(id)
                .await
                .map_err(|e| format!("delete entry error: {}", e))?;
            return Ok(Some(serde_json::json!({ "id": id, "deleted": deleted })));
        }

        migrate_legacy_vector_store(workspace);
        let jsonl_path = vector_store_jsonl_path(workspace);
        if !jsonl_path.exists() {
            return Ok(Some(serde_json::json!({ "id": id, "deleted": false })));
        }
        let content = std::fs::read_to_string(&jsonl_path)
            .map_err(|e| format!("failed to read vector store: {}", e))?;
        let mut removed = false;
        let mut kept = String::new();
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let is_target = serde_json::from_str::<serde_json::Value>(trimmed)
                .ok()
                .and_then(|e| e.get("id").and_then(|v| v.as_str()).map(|s| s == id))
                .unwrap_or(false);
            if is_target {
                removed = true;
                continue;
            }
            kept.push_str(trimmed);
            kept.push('\n');
        }
        if removed {
            let _guard = jsonl_write_lock().lock().unwrap();
            let tmp = jsonl_path.with_extension("jsonl.tmp");
            std::fs::write(&tmp, kept).map_err(|e| format!("failed to write tmp: {}", e))?;
            std::fs::rename(&tmp, &jsonl_path).map_err(|e| format!("failed to rename: {}", e))?;
        }
        Ok(Some(serde_json::json!({ "id": id, "deleted": removed })))
    }

    /// Update an entry's content = delete + re-store (the stored vector must
    /// be regenerated from the new content, so this REQUIRES the embedding
    /// pipeline). Editing the offline JSONL would write a stale vector that
    /// silently poisons semantic search — loud error instead.
    async fn entries_update(
        &self,
        workspace: &str,
        id: &str,
        content: &str,
        ctx: &crate::ws_router::RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        #[cfg(feature = "memory")]
        if let Some(mgr) = ctx.state.memory_manager.as_ref()
            && mgr.is_vector_enabled()
        {
            let deleted = mgr
                .delete(id)
                .await
                .map_err(|e| format!("update entry error: {}", e))?;
            if !deleted {
                return Err(format!("entry not found: {}", id));
            }
            let entry = nemesis_memory::types::Entry::new(
                nemesis_memory::types::MemoryType::LongTerm,
                content.to_string(),
            );
            let new_id = mgr
                .store_entry(entry)
                .await
                .map_err(|e| format!("store entry error: {}", e))?;
            return Ok(Some(serde_json::json!({ "id": new_id, "updated": true })));
        }
        let _ = workspace;
        Err("强化记忆未启用，无法编辑条目（需要重新生成向量）；可删除后重新添加".to_string())
    }
}

// ---------------------------------------------------------------------------
// Enhanced memory: model management
// ---------------------------------------------------------------------------

impl MemoryHandler {
    async fn model_install(
        &self,
        config_dir: &Path,
        tier: &str,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        // Validate tier
        if !["large", "medium", "small"].contains(&tier) {
            return Err(format!(
                "unknown tier: '{}'. Must be large, medium, or small.",
                tier
            ));
        }

        // Acquire per-tier install lock
        {
            let mut locks = install_locks().lock().unwrap();
            if locks.contains(tier) {
                return Err(format!("{}模型正在安装中，请稍候", tier));
            }
            locks.insert(tier.to_string());
        }

        let hub = ctx.state.event_hub.clone();
        let config_dir_clone = config_dir.to_path_buf();
        let tier_owned = tier.to_string();

        let result = tokio::task::spawn_blocking(move || {
            hub.publish(
                "memory-setup",
                serde_json::json!({
                    "status": "starting",
                    "message": format!("正在下载{}模型...", tier_owned)
                }),
            );

            let mut emb_cfg =
                nemesis_memory::vector::embedding_config::load_embedding_config(&config_dir_clone);

            // Temporarily set active to the requested tier
            let original_active = emb_cfg.active.clone();
            emb_cfg.active = tier_owned.clone();

            match nemesis_memory::vector::embedding_config::download_model_files(
                &mut emb_cfg,
                &config_dir_clone,
            ) {
                Ok((_model_dir, dim)) => {
                    // Restore original active and save
                    emb_cfg.active = original_active;
                    nemesis_memory::vector::embedding_config::save_embedding_config(
                        &emb_cfg,
                        &config_dir_clone,
                    );

                    hub.publish(
                        "memory-setup",
                        serde_json::json!({
                            "status": "complete",
                            "message": format!("{}模型安装完成 (dim={})", tier_owned, dim)
                        }),
                    );

                    Ok(serde_json::json!({ "success": true, "tier": tier_owned, "dimension": dim }))
                }
                Err(e) => {
                    // Restore and save even on failure
                    emb_cfg.active = original_active;
                    nemesis_memory::vector::embedding_config::save_embedding_config(
                        &emb_cfg,
                        &config_dir_clone,
                    );

                    hub.publish(
                        "memory-setup",
                        serde_json::json!({
                            "status": "error",
                            "message": format!("{}模型安装失败: {}", tier_owned, e)
                        }),
                    );
                    Err(format!("model install failed: {}", e))
                }
            }
        })
        .await
        .map_err(|e| format!("install task panicked: {}", e))?;

        // Release lock
        {
            let mut locks = install_locks().lock().unwrap();
            locks.remove(tier);
        }

        result.map(Some)
    }
}

// ---------------------------------------------------------------------------
// File system utilities
// ---------------------------------------------------------------------------

/// Recursively collect files under a directory.
fn collect_files(
    workspace: &str,
    base_relative: &str,
    output: &mut Vec<serde_json::Value>,
) -> Result<(), String> {
    let dir = resolve_path(workspace, base_relative)?;
    if !dir.exists() {
        return Ok(());
    }
    let read_dir = std::fs::read_dir(&dir).map_err(|e| format!("failed to read dir: {}", e))?;
    for entry in read_dir {
        let entry = entry.map_err(|e| format!("failed to read entry: {}", e))?;
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        let relative = if base_relative.is_empty() {
            name.clone()
        } else {
            format!("{}/{}", base_relative, name)
        };
        if path.is_dir() {
            collect_files(workspace, &relative, output)?;
        } else {
            let size = path.metadata().map(|m| m.len()).unwrap_or(0);
            output.push(serde_json::json!({
                "path": relative,
                "size": size,
                "type": "file",
            }));
        }
    }
    Ok(())
}

/// Count files recursively in a directory.
fn count_files_recursive(dir: &std::path::Path) -> usize {
    let mut count = 0;
    if let Ok(read_dir) = std::fs::read_dir(dir) {
        for entry in read_dir.flatten() {
            let path = entry.path();
            if path.is_dir() {
                count += count_files_recursive(&path);
            } else {
                count += 1;
            }
        }
    }
    count
}

/// Count non-empty lines in a JSONL file.
fn count_jsonl_lines(path: &std::path::Path) -> usize {
    if !path.exists() {
        return 0;
    }
    std::fs::read_to_string(path)
        .map(|c| c.lines().filter(|l| !l.trim().is_empty()).count())
        .unwrap_or(0)
}

/// Truncate the content field of an entry to 200 chars for listing.
fn truncate_entry_content(mut entry: serde_json::Value) -> serde_json::Value {
    let content = entry
        .get("content")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    if let Some(c) = content
        && c.len() > 200
    {
        // Truncate at the nearest char boundary ≤ 200 bytes. Slicing at a
        // fixed byte index lands inside multibyte UTF-8 chars (e.g. Chinese
        // in memory content) and panics.
        let mut end = 200;
        while !c.is_char_boundary(end) {
            end -= 1;
        }
        entry.as_object_mut().map(|o| {
            o.insert(
                "content".to_string(),
                serde_json::Value::String(format!("{}...", &c[..end])),
            )
        });
    }
    entry
}

/// Count episodic sessions and episodes.
fn count_episodic(dir: &std::path::Path) -> (usize, usize) {
    if !dir.exists() {
        return (0, 0);
    }
    let mut sessions = 0;
    let mut episodes = 0;
    if let Ok(read_dir) = std::fs::read_dir(dir) {
        for entry in read_dir.flatten() {
            let path = entry.path();
            if path.is_dir() {
                sessions += 1;
                if let Ok(files) = std::fs::read_dir(&path) {
                    for f in files.flatten() {
                        if f.path().is_file() {
                            episodes += 1;
                        }
                    }
                }
            } else if path.is_file() {
                // Flat file in episodic dir also counts as an episode
                episodes += 1;
                sessions += 1;
            }
        }
    }
    (sessions, episodes)
}

#[cfg(all(test, feature = "memory"))]
mod tests;

// S10b (2026-08-26, quality-hardening goal 冲刺 web 批次 2): manager-dependent
// arms + offline model-install success + helper edge cases.
#[cfg(all(test, feature = "memory"))]
mod s10b_tests;
