//! Memory handler — status/documents/vector operations + enhanced memory management.
//!
//! Commands: status, documents, document.get, document.save,
//!           env.check, env.setup, config.get, config.set,
//!           stats, entries.list, entries.search, entries.store,
//!           model.install

use crate::handlers::{read_workspace_file, require_home, require_workspace, resolve_path, write_workspace_file};
use crate::ws_router::{ModuleHandler, RequestContext};
use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
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
        let config_dir = PathBuf::from(workspace).join("config");

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
            "entries.list" => self.entries_list(workspace),
            "entries.search" => {
                let data = data.ok_or("missing data")?;
                let query = crate::handlers::get_str(&data, "query")?;
                let limit = data.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
                self.entries_search(workspace, &query, limit)
            }
            "entries.store" => {
                let data = data.ok_or("missing data")?;
                let content = crate::handlers::get_str(&data, "content")?;
                self.entries_store(workspace, &content)
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
                self.entries_search(workspace, &query, 10)
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
    nemesis_utils::find_plugin_library("plugin_onnx")
        .map(|p| p.to_string_lossy().to_string())
}

/// Read the `memory.enabled` field from the main config.json.
fn read_main_switch(home: &str) -> bool {
    let cfg_path = PathBuf::from(home).join("config.json");
    if !cfg_path.exists() {
        return false;
    }
    std::fs::read_to_string(&cfg_path).ok()
        .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
        .and_then(|v| v.get("memory").and_then(|m| m.get("enabled")).and_then(|e| e.as_bool()))
        .unwrap_or(false)
}

/// Write the `memory.enabled` field in the main config.json.
fn set_main_switch(home: &str, enabled: bool) -> Result<(), String> {
    let cfg_path = PathBuf::from(home).join("config.json");
    if !cfg_path.exists() {
        return Err(format!("config.json not found at {}", cfg_path.display()));
    }
    let content = std::fs::read_to_string(&cfg_path)
        .map_err(|e| format!("failed to read config: {}", e))?;
    let mut cfg: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| format!("failed to parse config: {}", e))?;
    if cfg.get("memory").is_none() {
        cfg.as_object_mut().map(|o| o.insert("memory".to_string(), serde_json::json!({})));
    }
    if let Some(mem) = cfg.get_mut("memory").and_then(|m| m.as_object_mut()) {
        mem.insert("enabled".to_string(), serde_json::Value::Bool(enabled));
    }
    let updated = serde_json::to_string_pretty(&cfg)
        .map_err(|e| format!("failed to serialize config: {}", e))?;
    std::fs::write(&cfg_path, updated)
        .map_err(|e| format!("failed to write config: {}", e))?;
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

        let em_config_path = PathBuf::from(workspace).join("config/config.enhanced_memory.json");
        let vector_enabled = if em_config_path.exists() {
            nemesis_memory::vector::embedding_config::load_embedding_config(
                &PathBuf::from(workspace).join("config"),
            ).enabled
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

    fn document_get(&self, workspace: &str, path: &str) -> Result<Option<serde_json::Value>, String> {
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
        let config_dir = PathBuf::from(workspace).join("config");
        let emb_cfg = nemesis_memory::vector::embedding_config::load_embedding_config(&config_dir);
        Ok(Some(serde_json::json!({ "enabled": emb_cfg.enabled })))
    }
}

// ---------------------------------------------------------------------------
// Enhanced memory: environment
// ---------------------------------------------------------------------------

impl MemoryHandler {
    fn env_check(&self, config_dir: &PathBuf, home: &str) -> Result<Option<serde_json::Value>, String> {
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
                let model_ready = if !mc.local_model_path.is_empty() && std::path::Path::new(&mc.local_model_path).exists() {
                    true
                } else {
                    model_file.exists()
                };
                let tokenizer_ready = if !mc.local_tokenizer_path.is_empty() && std::path::Path::new(&mc.local_tokenizer_path).exists() {
                    true
                } else {
                    tokenizer_file.exists()
                };

                models.insert(tier.to_string(), serde_json::json!({
                    "name": mc.name,
                    "dimension": mc.dimension,
                    "model_ready": model_ready,
                    "tokenizer_ready": tokenizer_ready,
                    "model_size": mc.model_size,
                }));
            }
        }

        // Overall status
        let active_model_ready = emb_cfg.models.get(&active_tier)
            .map(|mc| {
                let model_ready = if !mc.local_model_path.is_empty() && std::path::Path::new(&mc.local_model_path).exists() {
                    true
                } else {
                    emb_data_dir.join(&mc.name).join("model.onnx").exists()
                };
                model_ready
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
        config_dir: &PathBuf,
        home: &str,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let hub = ctx.state.event_hub.clone();
        let config_dir_clone = config_dir.clone();
        let home_owned = home.to_string();

        let result = tokio::task::spawn_blocking(move || {
            // 1. Check plugin
            let _plugin_path = detect_plugin_path().ok_or_else(|| {
                hub.publish("memory-setup", serde_json::json!({
                    "status": "error", "message": "Plugin not found"
                }));
                let filename = nemesis_utils::plugin_library_filename("plugin_onnx");
                format!("Plugin not found at {{exe_dir}}/plugins/{}", filename)
            })?;

            hub.publish("memory-setup", serde_json::json!({
                "status": "starting", "message": "正在准备模型文件..."
            }));

            // 2. Download model files
            let mut emb_cfg = nemesis_memory::vector::embedding_config::load_embedding_config(&config_dir_clone);
            let (_model_dir, _dim) = nemesis_memory::vector::embedding_config::download_model_files(
                &mut emb_cfg, &config_dir_clone,
            ).map_err(|e| {
                hub.publish("memory-setup", serde_json::json!({
                    "status": "error", "message": format!("模型下载失败: {}", e)
                }));
                e
            })?;
            nemesis_memory::vector::embedding_config::save_embedding_config(&emb_cfg, &config_dir_clone);

            // 3. Write enabled=true to unified config
            let mut emb_cfg = nemesis_memory::vector::embedding_config::load_embedding_config(&config_dir_clone);
            emb_cfg.enabled = true;
            nemesis_memory::vector::embedding_config::save_embedding_config(&emb_cfg, &config_dir_clone);

            hub.publish("memory-setup", serde_json::json!({
                "status": "complete", "message": "一键安装完成"
            }));

            Ok::<(), String>(())
        }).await
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
    fn config_get(&self, config_dir: &PathBuf, home: &str) -> Result<Option<serde_json::Value>, String> {
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
            "embedding_config_content": embedding_config_content,
        })))
    }

    fn config_set(&self, config_dir: &PathBuf, home: &str, data: &serde_json::Value, ctx: &crate::ws_router::RequestContext) -> Result<Option<serde_json::Value>, String> {
        // Main switch
        if let Some(enabled) = data.get("main_enabled").and_then(|v| v.as_bool()) {
            set_main_switch(home, enabled)?;
            if !enabled {
                if let Some(mgr) = ctx.state.memory_manager.as_ref() {
                    mgr.set_vector_enabled(false);
                }
            }
        }

        // Sub switch (write via unified config + runtime control)
        if let Some(enabled) = data.get("sub_enabled").and_then(|v| v.as_bool()) {
            if enabled {
                // Check model files before enabling
                let emb_cfg = nemesis_memory::vector::embedding_config::load_embedding_config(config_dir);
                let emb_data_dir = nemesis_memory::vector::embedding_config::embedding_data_dir(config_dir);
                let model_ready = emb_cfg.models.get(&emb_cfg.active)
                    .map(|mc| {
                        if !mc.local_model_path.is_empty() && std::path::Path::new(&mc.local_model_path).exists() {
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
            let mut emb_cfg = nemesis_memory::vector::embedding_config::load_embedding_config(config_dir);
            emb_cfg.enabled = enabled;
            nemesis_memory::vector::embedding_config::save_embedding_config(&emb_cfg, config_dir);
            // Runtime control
            if let Some(mgr) = ctx.state.memory_manager.as_ref() {
                if enabled {
                    if let Err(e) = mgr.init_vector_store_from_config(config_dir) {
                        // Init failed → rollback config
                        let mut emb_cfg = nemesis_memory::vector::embedding_config::load_embedding_config(config_dir);
                        emb_cfg.enabled = false;
                        nemesis_memory::vector::embedding_config::save_embedding_config(&emb_cfg, config_dir);
                        return Err(format!("向量存储初始化失败: {}", e));
                    }
                } else {
                    mgr.set_vector_enabled(false);
                }
            }
        }

        // Active tier
        if let Some(tier) = data.get("active_tier").and_then(|v| v.as_str()) {
            let mut emb_cfg = nemesis_memory::vector::embedding_config::load_embedding_config(config_dir);
            emb_cfg.active = tier.to_string();
            nemesis_memory::vector::embedding_config::save_embedding_config(&emb_cfg, config_dir);
        }

        // Embedding config content (full overwrite of config.enhanced_memory.json)
        if let Some(content) = data.get("embedding_config_content").and_then(|v| v.as_str()) {
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
    fn stats(&self, config_dir: &PathBuf, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let memory_dir = PathBuf::from(workspace).join("memory");

        // Vector entries: count lines in vector_store.jsonl
        let vector_jsonl = memory_dir.join("vector").join("vector_store.jsonl");
        let vector_entries = count_jsonl_lines(&vector_jsonl);

        // Episodic: count files under episodic/
        let episodic_dir = memory_dir.join("episodic");
        let (episodic_sessions, episodic_episodes) = count_episodic(&episodic_dir);

        // Graph: count lines in entities.jsonl and triples.jsonl
        let graph_dir = memory_dir.join("graph");
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
        let vector_dimension = emb_cfg.models.get(&active_tier)
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

    fn entries_list(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let jsonl_path = PathBuf::from(workspace).join("memory").join("vector").join("vector_store.jsonl");
        if !jsonl_path.exists() {
            return Ok(Some(serde_json::json!({ "entries": [], "total": 0 })));
        }

        let content = std::fs::read_to_string(&jsonl_path)
            .map_err(|e| format!("failed to read vector store: {}", e))?;

        let mut entries = Vec::new();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() { continue; }
            if let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) {
                entries.push(truncate_entry_content(entry));
            }
        }

        let total = entries.len();
        // Return most recent first (last in file)
        entries.reverse();
        entries.truncate(100);

        Ok(Some(serde_json::json!({ "entries": entries, "total": total })))
    }

    fn entries_search(&self, workspace: &str, query: &str, limit: usize) -> Result<Option<serde_json::Value>, String> {
        let jsonl_path = PathBuf::from(workspace).join("memory").join("vector").join("vector_store.jsonl");
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
            if line.is_empty() { continue; }
            if let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) {
                let text = entry.get("content")
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

    fn entries_store(&self, workspace: &str, content: &str) -> Result<Option<serde_json::Value>, String> {
        let jsonl_path = PathBuf::from(workspace).join("memory").join("vector").join("vector_store.jsonl");

        // Ensure directory exists
        if let Some(parent) = jsonl_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create dir: {}", e))?;
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

        let mut line = serde_json::to_string(&entry)
            .map_err(|e| format!("serialize error: {}", e))?;
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
}

// ---------------------------------------------------------------------------
// Enhanced memory: model management
// ---------------------------------------------------------------------------

impl MemoryHandler {
    async fn model_install(
        &self,
        config_dir: &PathBuf,
        tier: &str,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        // Validate tier
        if !["large", "medium", "small"].contains(&tier) {
            return Err(format!("unknown tier: '{}'. Must be large, medium, or small.", tier));
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
        let config_dir_clone = config_dir.clone();
        let tier_owned = tier.to_string();

        let result = tokio::task::spawn_blocking(move || {
            hub.publish("memory-setup", serde_json::json!({
                "status": "starting",
                "message": format!("正在下载{}模型...", tier_owned)
            }));

            let mut emb_cfg = nemesis_memory::vector::embedding_config::load_embedding_config(&config_dir_clone);

            // Temporarily set active to the requested tier
            let original_active = emb_cfg.active.clone();
            emb_cfg.active = tier_owned.clone();

            match nemesis_memory::vector::embedding_config::download_model_files(&mut emb_cfg, &config_dir_clone) {
                Ok((_model_dir, dim)) => {
                    // Restore original active and save
                    emb_cfg.active = original_active;
                    nemesis_memory::vector::embedding_config::save_embedding_config(&emb_cfg, &config_dir_clone);

                    hub.publish("memory-setup", serde_json::json!({
                        "status": "complete",
                        "message": format!("{}模型安装完成 (dim={})", tier_owned, dim)
                    }));

                    Ok(serde_json::json!({ "success": true, "tier": tier_owned, "dimension": dim }))
                }
                Err(e) => {
                    // Restore and save even on failure
                    emb_cfg.active = original_active;
                    nemesis_memory::vector::embedding_config::save_embedding_config(&emb_cfg, &config_dir_clone);

                    hub.publish("memory-setup", serde_json::json!({
                        "status": "error",
                        "message": format!("{}模型安装失败: {}", tier_owned, e)
                    }));
                    Err(format!("model install failed: {}", e))
                }
            }
        }).await
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
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
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
    let content = entry.get("content").and_then(|v| v.as_str()).map(|s| s.to_string());
    if let Some(c) = content {
        if c.len() > 200 {
            // Truncate at the nearest char boundary ≤ 200 bytes. Slicing at a
            // fixed byte index lands inside multibyte UTF-8 chars (e.g. Chinese
            // in memory content) and panics.
            let mut end = 200;
            while !c.is_char_boundary(end) {
                end -= 1;
            }
            entry.as_object_mut().map(|o| o.insert(
                "content".to_string(),
                serde_json::Value::String(format!("{}...", &c[..end])),
            ));
        }
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
