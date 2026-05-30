//! Unified memory manager that combines all stores.
//!
//! `MemoryManager` is the main entry point for the memory subsystem. It owns
//! a general `MemoryStore` (entries), an `EpisodicStore` (conversation
//! episodes), a `GraphStore` (knowledge graph), and an optional `VectorStore`
//! (semantic search). It exposes high-level operations mirroring the Go
//! `Manager`: `store`, `query`, `get`, `delete`, `close`, `init_vector_store`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::RwLock;

use crate::episodic::{EpisodicStore, Episode, FileEpisodicStore};
use crate::graph::{GraphEntity, GraphQueryResult, GraphStore, GraphTriple, InMemoryGraphStore};
use crate::local_store::TfIdfLocalStore;
use crate::store::{LocalStore, MemoryStore};
use crate::types::{Entry, MemoryType, SearchResult, ScoredEntry, VectorConfig};
use crate::vector::{VectorStore, StoreConfig};
use crate::vector::embedding_config;

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Configuration for the memory manager.
#[derive(Debug, Clone)]
pub struct Config {
    /// Root directory for file-based stores (episodic, etc.).
    pub data_dir: PathBuf,
    /// Optional vector search configuration.
    pub vector: VectorConfig,
}

impl Config {
    /// Create config pointing at the given data directory.
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            vector: VectorConfig::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// MemoryManager
// ---------------------------------------------------------------------------

/// Unified memory manager combining all storage backends.
///
/// Wraps a general-purpose `MemoryStore`, an `EpisodicStore`, a `GraphStore`,
/// and an optional `VectorStore`. The `enabled` flag controls whether most
/// operations are allowed; when disabled, reads return empty results and
/// writes are silently ignored -- matching the Go `Manager` semantics.
pub struct MemoryManager {
    /// General-purpose memory store (entries with TF-IDF / keyword search).
    store: Arc<dyn MemoryStore>,
    /// Episodic conversation store.
    episodic: Arc<dyn EpisodicStore>,
    /// Knowledge graph store.
    graph: Arc<dyn GraphStore>,
    /// Optional vector store for semantic search.
    vector_store: RwLock<Option<Arc<VectorStore>>>,
    /// Whether vector search is active (may differ from vector_store being Some).
    /// When false, search/store skip the vector store even if it is initialized.
    /// This allows disabling at runtime without dropping the ONNX plugin.
    vector_enabled: RwLock<bool>,
    /// Whether the memory system is active.
    enabled: RwLock<bool>,
    /// Root data directory (used for vector store paths).
    data_dir: PathBuf,
}

impl MemoryManager {
    /// Build a `MemoryManager` with default in-memory / file-backed stores.
    ///
    /// The general store is an in-memory `LocalStore` with word-overlap scoring.
    /// Use `new_with_jsonl` for a JSONL-persisted + TF-IDF-backed store.
    ///
    /// The graph store is persisted to `{data_dir}/graph/` when the directory
    /// exists or can be created.
    pub fn new(config: &Config) -> Self {
        let episodic_dir = config.data_dir.join("episodic");
        let graph_dir = config.data_dir.join("graph");
        let graph = InMemoryGraphStore::new().with_persistence(graph_dir);
        Self {
            store: Arc::new(LocalStore::new()),
            episodic: Arc::new(FileEpisodicStore::new(episodic_dir)),
            graph: Arc::new(graph),
            vector_store: RwLock::new(None),
            vector_enabled: RwLock::new(false),
            enabled: RwLock::new(true),
            data_dir: config.data_dir.clone(),
        }
    }

    /// Build a `MemoryManager` backed by a JSONL-persisted TF-IDF store.
    ///
    /// The store file is `{data_dir}/memory/store.jsonl`. This mirrors the Go
    /// constructor which always creates a `localStore`.
    ///
    /// The graph store is persisted to `{data_dir}/graph/`.
    pub async fn new_with_jsonl(config: &Config) -> Result<Self, String> {
        let episodic_dir = config.data_dir.join("episodic");
        let store_path = config.data_dir.join("memory").join("store.jsonl");
        let graph_dir = config.data_dir.join("graph");

        let tfidf_store = TfIdfLocalStore::new(&store_path).await?;
        let graph = InMemoryGraphStore::new().with_persistence(graph_dir);

        Ok(Self {
            store: Arc::new(tfidf_store),
            episodic: Arc::new(FileEpisodicStore::new(episodic_dir)),
            graph: Arc::new(graph),
            vector_store: RwLock::new(None),
            vector_enabled: RwLock::new(false),
            enabled: RwLock::new(true),
            data_dir: config.data_dir.clone(),
        })
    }

    /// Create a `MemoryManager` with config-based enhanced memory auto-detection.
    ///
    /// Reads `config.enhanced_memory.json` from `config_dir` for the unified
    /// configuration (enabled flag + model definitions), then tries to
    /// auto-detect and load the ONNX plugin.
    ///
    /// Flow:
    /// - No config file → basic memory (no vector store)
    /// - Config `enabled: false` → basic memory (previously failed or manually disabled)
    /// - Config `enabled: true` + plugin DLL found → vector store with semantic search
    /// - Config `enabled: true` + plugin missing/fails → writes `enabled: false` to disk, basic memory
    ///
    /// Never panics or returns an error.
    pub fn with_config_dir(data_dir: &Path, config_dir: &Path) -> Self {
        let mut cfg = Config::new(data_dir);
        cfg.vector.config_dir = Some(config_dir.to_string_lossy().to_string());

        // 1. Load unified embedding config (contains enabled + models)
        let emb_config = embedding_config::load_embedding_config(config_dir);

        // 2. Create MemoryManager (basic memory always available)
        let mgr = Self::new(&cfg);

        // 3. Attempt vector store init only when config says enabled
        if !emb_config.enabled {
            tracing::info!(
                "[Memory] Enhanced memory disabled (config.enhanced_memory.json: enabled = false)"
            );
            return mgr;
        }

        // Auto-detect plugin — it is always at {exe_dir}/plugins/plugin_onnx.dll
        let plugin_path = match Self::detect_plugin_path() {
            Some(p) => p,
            None => {
                tracing::warn!(
                    "[Memory] Plugin DLL not found at {{exe_dir}}/plugins/plugin_onnx.dll. \
                     Disabling enhanced memory."
                );
                Self::disable_enhanced_memory_config(config_dir, &emb_config);
                return mgr;
            }
        };

        cfg.vector.plugin_path = Some(plugin_path.clone());

        let storage_path = data_dir.join("vector").join("vector_store.jsonl");
        let store_config = StoreConfig {
            embedding_tier: "plugin".into(),
            plugin_path: Some(plugin_path),
            config_dir: cfg.vector.config_dir.clone(),
            max_results: 10,
            similarity_threshold: 0.7,
            storage_path: storage_path.to_string_lossy().to_string(),
        };

        match mgr.init_vector_store(Some(store_config)) {
            Ok(()) => {
                *mgr.vector_enabled.write() = true;
                tracing::info!("[Memory] Vector store initialized");
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "[Memory] Vector store init failed, disabling enhanced memory"
                );
                Self::disable_enhanced_memory_config(config_dir, &emb_config);
            }
        }

        mgr
    }

    /// Write `enabled: false` to config.enhanced_memory.json so the next
    /// restart skips the vector store init attempt entirely.
    /// Preserves the rest of the config (models, active tier).
    fn disable_enhanced_memory_config(
        config_dir: &Path,
        emb_config: &embedding_config::EmbeddingConfig,
    ) {
        let mut disabled = emb_config.clone();
        disabled.enabled = false;
        embedding_config::save_embedding_config(&disabled, config_dir);
    }

    /// Auto-detect plugin DLL path next to the current executable.
    ///
    /// Checks `{exe_dir}/plugins/plugin_onnx.dll`.
    pub fn detect_plugin_path() -> Option<String> {
        let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
        let plugin_dll = exe_dir.join("plugins").join("plugin_onnx.dll");
        if plugin_dll.exists() {
            Some(plugin_dll.to_string_lossy().to_string())
        } else {
            None
        }
    }

    /// Enable or disable vector search at runtime without dropping the store.
    ///
    /// When disabled, `search()` and `store_entry_to_vector()` skip the vector
    /// store entirely. The ONNX plugin stays alive in memory for instant re-enable.
    pub fn set_vector_enabled(&self, enabled: bool) {
        *self.vector_enabled.write() = enabled;
        tracing::info!("[Memory] Vector search {}", if enabled { "enabled" } else { "disabled" });
    }

    /// Initialize the vector store at runtime (e.g. from a Dashboard toggle).
    ///
    /// If the vector store is already initialized, just enables it.
    /// Otherwise creates a new one from the current config.
    /// Returns `Err` if the plugin DLL or model files are missing.
    pub fn init_vector_store_from_config(&self, config_dir: &Path) -> Result<(), String> {
        // Already initialized → just enable
        if self.vector_store.read().is_some() {
            *self.vector_enabled.write() = true;
            tracing::info!("[Memory] Vector store already initialized, enabled");
            return Ok(());
        }

        // Detect plugin
        let plugin_path = Self::detect_plugin_path()
            .ok_or("plugin_onnx.dll not found")?;

        let storage_path = self.data_dir.join("vector").join("vector_store.jsonl");
        let store_config = StoreConfig {
            embedding_tier: "plugin".into(),
            plugin_path: Some(plugin_path),
            config_dir: Some(config_dir.to_string_lossy().to_string()),
            max_results: 10,
            similarity_threshold: 0.7,
            storage_path: storage_path.to_string_lossy().to_string(),
        };

        self.init_vector_store(Some(store_config))?;
        *self.vector_enabled.write() = true;
        tracing::info!("[Memory] Vector store created and enabled at runtime");
        Ok(())
    }

    /// Build a `MemoryManager` with custom store implementations (for testing).
    pub fn with_backends(
        store: Arc<dyn MemoryStore>,
        episodic: Arc<dyn EpisodicStore>,
        graph: Arc<dyn GraphStore>,
    ) -> Self {
        Self {
            store,
            episodic,
            graph,
            vector_store: RwLock::new(None),
            vector_enabled: RwLock::new(false),
            enabled: RwLock::new(true),
            data_dir: PathBuf::new(),
        }
    }

    // -- Lifecycle ----------------------------------------------------------

    /// Reports whether the memory system is active.
    pub fn is_enabled(&self) -> bool {
        *self.enabled.read()
    }

    /// Initialize the vector store and replace the general store with a
    /// `VectorStoreAdapter` so that all entries go through the vector backend.
    ///
    /// After this call `query_semantic` will delegate to the vector store for
    /// embedding-based similarity search.
    ///
    /// If a JSONL persistence file exists at the configured path, previously
    /// saved entries are loaded into memory so they are available for search.
    pub fn init_vector_store(&self, config: Option<StoreConfig>) -> Result<(), String> {
        let store_cfg = config.unwrap_or_else(|| {
            let default_path = self.data_dir
                .join("memory")
                .join("vector")
                .join("vector_store.jsonl");
            StoreConfig {
                storage_path: default_path.to_string_lossy().to_string(),
                ..Default::default()
            }
        });

        let vs = Arc::new(VectorStore::new(store_cfg).map_err(|e| e)?);

        // Load previously persisted entries
        if let Err(e) = vs.load_persisted_sync() {
            tracing::warn!(error = %e, "[Memory] Failed to load persisted vector entries");
        }

        *self.vector_store.write() = Some(vs);
        Ok(())
    }

    /// Initialize the vector store with a pre-built embedding function.
    ///
    /// This is a test-only method that allows sharing a single ONNX plugin
    /// across multiple MemoryManager instances via the shared test fixture.
    #[cfg(any(test, feature = "test-fixture"))]
    pub fn init_vector_store_with_embed(
        &self,
        embed: crate::vector::EmbeddingFunc,
        config: StoreConfig,
    ) -> Result<(), String> {
        let vs = Arc::new(VectorStore::new_from_embed(embed, config));
        *self.vector_store.write() = Some(vs);
        Ok(())
    }

    /// Gracefully shut down all stores and disable the manager.
    ///
    /// Mirrors Go `Manager.Close`: sets `enabled = false`, then closes each
    /// backend in order (vector -> episodic -> graph -> general). If multiple
    /// errors occur they are concatenated.
    pub async fn close(&self) -> Result<(), String> {
        // Disable first.
        *self.enabled.write() = false;

        let mut errors: Vec<String> = Vec::new();

        // Close vector store.
        // ONNX plugin drop performs blocking I/O; must not run inside an
        // async context (tokio panics "Cannot drop a runtime where blocking
        // is not allowed").  Move the drop to a blocking thread.
        {
            let vs = self.vector_store.write().take();
            if vs.is_some() {
                tokio::task::spawn_blocking(move || drop(vs)).await.map_err(|e| e.to_string())?;
            }
        }

        // Close episodic store -- no async close in the trait, so just drop.
        // The FileEpisodicStore has no resources to release.

        // Close graph store -- InMemoryGraphStore needs no cleanup.

        // Close general store.
        if let Err(e) = self.store.close().await {
            errors.push(format!("store: {e}"));
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(format!("memory close errors: {}", errors.join("; ")))
        }
    }

    // -- General store operations ------------------------------------------

    /// Store a new memory entry and return its ID.
    ///
    /// Silently succeeds when disabled (mirrors Go behaviour).
    /// When the vector store is initialized, the entry is also stored in the
    /// vector store for semantic search (mirrors Go's vectorStoreAdapter).
    pub async fn store_entry(&self, entry: Entry) -> Result<String, String> {
        if !self.is_enabled() {
            return Ok(String::new());
        }
        let id = self.store.store(entry.clone()).await?;

        // Also store in the vector store if initialized (adapter pattern)
        self.store_entry_to_vector(&entry, &id);

        Ok(id)
    }

    /// Store a memory entry (Go-style `Manager.Store`).
    ///
    /// Fills in ID/timestamps if missing, then delegates to the backing store.
    /// When the vector store is initialized, the entry is also stored in the
    /// vector store for semantic search (mirrors Go's vectorStoreAdapter).
    pub async fn store(&self, entry: Entry) -> Result<String, String> {
        if !self.is_enabled() {
            return Ok(String::new());
        }
        let id = self.store.store(entry.clone()).await?;

        // Also store in the vector store if initialized (adapter pattern)
        self.store_entry_to_vector(&entry, &id);

        Ok(id)
    }

    /// Helper: store an entry in the vector store if it is initialized.
    /// Also persists to disk so data survives restarts.
    fn store_entry_to_vector(&self, entry: &Entry, id: &str) {
        if !*self.vector_enabled.read() {
            return;
        }
        let vs_guard = self.vector_store.read();
        if let Some(ref vs) = *vs_guard {
            let ve = crate::vector::VectorEntry {
                id: id.to_string(),
                entry_type: format!("{:?}", entry.typ).to_lowercase(),
                content: entry.content.clone(),
                metadata: entry.metadata.clone(),
                tags: entry.tags.clone(),
                score: entry.score.unwrap_or(0.0),
                created_at: entry.created_at.to_rfc3339(),
                updated_at: entry.updated_at.to_rfc3339(),
            };
            if let Err(e) = vs.store_entry(&ve) {
                tracing::debug!("[Memory] Failed to store entry in vector store: {}", e);
            }
            if let Err(e) = vs.persist_entry_sync(&ve) {
                tracing::debug!("[Memory] Failed to persist entry to disk: {}", e);
            }
        }
    }

    /// Search all general memory entries by free-text query.
    ///
    /// When the vector store is initialized, tries semantic search first
    /// and falls back to keyword search (mirrors Go's vectorStoreAdapter).
    pub async fn search(
        &self,
        query: &str,
        memory_type: Option<MemoryType>,
        limit: usize,
    ) -> Result<SearchResult, String> {
        if !self.is_enabled() {
            return Ok(SearchResult {
                entries: Vec::new(),
                total: 0,
            });
        }

        // Try vector store first for semantic search (adapter pattern)
        if *self.vector_enabled.read() {
            let vs_guard = self.vector_store.read();
            if let Some(ref vs) = *vs_guard {
                let type_filter: Vec<String> = memory_type
                    .map(|mt| format!("{:?}", mt).to_lowercase())
                    .into_iter()
                    .collect();
                let result = vs.query(query, limit, &type_filter)
                    .map_err(|e| e.to_string())?;

                if !result.entries.is_empty() {
                    let entries: Vec<ScoredEntry> = result
                        .entries
                        .into_iter()
                        .map(|ve| {
                            let entry = Entry {
                                id: ve.id,
                                typ: parse_memory_type_from_str(&ve.entry_type),
                                content: ve.content,
                                metadata: ve.metadata,
                                tags: ve.tags,
                                score: Some(ve.score),
                                created_at: chrono::DateTime::parse_from_rfc3339(&ve.created_at)
                                    .map(|dt| dt.with_timezone(&chrono::Utc))
                                    .unwrap_or_else(|_| chrono::Utc::now()),
                                updated_at: chrono::DateTime::parse_from_rfc3339(&ve.updated_at)
                                    .map(|dt| dt.with_timezone(&chrono::Utc))
                                    .unwrap_or_else(|_| chrono::Utc::now()),
                            };
                            ScoredEntry {
                                entry,
                                score: ve.score,
                            }
                        })
                        .collect();

                    let total = entries.len();
                    return Ok(SearchResult { entries, total });
                }
            }
        }

        // Fallback to keyword search
        self.store.query(query, memory_type, limit).await
    }

    /// Query memories matching the text query with optional type filter.
    ///
    /// Mirrors Go `Manager.Query`. When the vector store is initialized,
    /// tries semantic search first and falls back to keyword search.
    pub async fn query(
        &self,
        query: &str,
        memory_type: Option<MemoryType>,
        limit: usize,
    ) -> Result<SearchResult, String> {
        // Delegate to search which handles the vector store adapter pattern
        self.search(query, memory_type, limit).await
    }

    /// Semantic search using vector embeddings if available, otherwise
    /// falls back to keyword-frequency scoring.
    ///
    /// When the vector store is initialised, the query text is embedded and
    /// compared against stored vectors using cosine similarity. Without a
    /// vector store the general keyword store is used instead.
    pub async fn query_semantic(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<SearchResult, String> {
        if !self.is_enabled() {
            return Ok(SearchResult {
                entries: Vec::new(),
                total: 0,
            });
        }

        let limit = if limit == 0 { 5 } else { limit };

        // Try the vector store first.
        let vs_guard = self.vector_store.read();
        if let Some(ref vs) = *vs_guard {
            let result = vs.query(query, limit, &[])
                .map_err(|e| e.to_string())?;

            // Convert VectorEntry results to SearchResult.
            let entries: Vec<ScoredEntry> = result
                .entries
                .into_iter()
                .map(|ve| {
                    let entry = Entry {
                        id: ve.id,
                        typ: parse_memory_type_from_str(&ve.entry_type),
                        content: ve.content,
                        metadata: ve.metadata,
                        tags: ve.tags,
                        score: Some(ve.score),
                        created_at: chrono::DateTime::parse_from_rfc3339(&ve.created_at)
                            .map(|dt| dt.with_timezone(&chrono::Utc))
                            .unwrap_or_else(|_| chrono::Utc::now()),
                        updated_at: chrono::DateTime::parse_from_rfc3339(&ve.updated_at)
                            .map(|dt| dt.with_timezone(&chrono::Utc))
                            .unwrap_or_else(|_| chrono::Utc::now()),
                    };
                    ScoredEntry {
                        entry,
                        score: ve.score,
                    }
                })
                .collect();

            let total = entries.len();
            return Ok(SearchResult { entries, total });
        }

        // Fallback: keyword search over the general store.
        drop(vs_guard);
        self.store.query(query, None, limit).await
    }

    /// Retrieve a memory entry by ID.
    ///
    /// Checks the keyword store first, then falls back to the vector store
    /// if initialized (mirrors Go's vectorStoreAdapter).
    pub async fn get(&self, id: &str) -> Result<Option<Entry>, String> {
        if !self.is_enabled() {
            return Ok(None);
        }

        // Try keyword store first
        if let Some(entry) = self.store.get(id).await? {
            return Ok(Some(entry));
        }

        // Fall back to vector store if initialized
        let vs_guard = self.vector_store.read();
        if let Some(ref vs) = *vs_guard {
            if let Some(ve) = vs.get_by_id(id) {
                return Ok(Some(Entry {
                    id: ve.id,
                    typ: parse_memory_type_from_str(&ve.entry_type),
                    content: ve.content,
                    metadata: ve.metadata,
                    tags: ve.tags,
                    score: Some(ve.score),
                    created_at: chrono::DateTime::parse_from_rfc3339(&ve.created_at)
                        .map(|dt| dt.with_timezone(&chrono::Utc))
                        .unwrap_or_else(|_| chrono::Utc::now()),
                    updated_at: chrono::DateTime::parse_from_rfc3339(&ve.updated_at)
                        .map(|dt| dt.with_timezone(&chrono::Utc))
                        .unwrap_or_else(|_| chrono::Utc::now()),
                }));
            }
        }

        Ok(None)
    }

    /// Delete ("forget") a memory entry by ID.
    pub async fn forget(&self, id: &str) -> Result<bool, String> {
        if !self.is_enabled() {
            return Ok(false);
        }
        self.store.delete(id).await
    }

    /// Delete a memory entry by ID (alias matching Go `Manager.Delete`).
    pub async fn delete(&self, id: &str) -> Result<bool, String> {
        self.forget(id).await
    }

    /// List general memory entries with optional type filter and pagination.
    pub async fn list(
        &self,
        memory_type: Option<MemoryType>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Entry>, String> {
        if !self.is_enabled() {
            return Ok(Vec::new());
        }
        self.store.list(memory_type, limit, offset).await
    }

    // -- Convenience helpers -----------------------------------------------

    /// Store an episodic memory entry (conversation experience).
    /// Creates an Entry of type Episodic with session metadata.
    ///
    /// Routes through the vector store adapter so data is available for
    /// semantic search when the ONNX plugin is loaded.
    pub async fn store_episodic(
        &self,
        session_key: &str,
        role: &str,
        content: &str,
    ) -> Result<String, String> {
        let entry = Entry::new(MemoryType::Episodic, content.to_string())
            .with_metadata({
                let mut meta = HashMap::new();
                meta.insert("session_key".to_string(), session_key.to_string());
                meta.insert("role".to_string(), role.to_string());
                meta
            })
            .with_tags(vec!["conversation".to_string(), role.to_string()]);
        self.store_entry(entry).await
    }

    /// Store a long-term factual memory.
    ///
    /// Routes through the vector store adapter so data is available for
    /// semantic search when the ONNX plugin is loaded.
    pub async fn store_fact(
        &self,
        content: &str,
        tags: Vec<String>,
    ) -> Result<String, String> {
        let entry = Entry::new(MemoryType::LongTerm, content.to_string())
            .with_tags(tags);
        self.store_entry(entry).await
    }

    // -- Episodic operations -----------------------------------------------

    /// Append a conversation episode.
    pub async fn append_episode(&self, episode: Episode) -> Result<String, String> {
        let id = self.episodic.append(episode.clone()).await?;

        // Also store in vector store for semantic search
        let entry = Entry {
            id: id.clone(),
            typ: MemoryType::Episodic,
            content: episode.content.clone(),
            metadata: episode.metadata.clone(),
            tags: episode.tags.clone(),
            score: None,
            created_at: episode.timestamp,
            updated_at: episode.timestamp,
        };
        self.store_entry_to_vector(&entry, &id);

        Ok(id)
    }

    /// Retrieve all episodes for a session.
    pub async fn get_session(&self, session_key: &str) -> Result<Vec<Episode>, String> {
        self.episodic.get_session(session_key).await
    }

    /// Get recent episodes for a session with a limit.
    pub async fn get_recent_episodes(
        &self,
        session_key: &str,
        limit: usize,
    ) -> Result<Vec<Episode>, String> {
        self.episodic.get_recent(session_key, limit).await
    }

    /// Search episodic memories by text query.
    pub async fn search_episodic(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<Episode>, String> {
        self.episodic.search(query, limit).await
    }

    /// Delete all episodes for a session.
    pub async fn delete_episode_session(
        &self,
        session_key: &str,
    ) -> Result<usize, String> {
        self.episodic.delete_session(session_key).await
    }

    /// Cleanup episodes older than the given number of days.
    pub async fn cleanup_episodic(&self, older_than_days: usize) -> Result<usize, String> {
        self.episodic.cleanup(older_than_days).await
    }

    /// Get episodic store stats (session_count, episode_count).
    pub async fn episodic_stats(&self) -> Result<(usize, usize), String> {
        let sessions = self.episodic.session_count().await?;
        let episodes = self.episodic.episode_count().await?;
        Ok((sessions, episodes))
    }

    // -- Graph operations --------------------------------------------------

    /// Add a knowledge-graph triple.
    pub async fn add_triple(&self, triple: GraphTriple) -> Result<(), String> {
        self.graph.add_triple(triple).await
    }

    /// Add or update a graph entity.
    pub async fn upsert_entity(&self, entity: GraphEntity) -> Result<(), String> {
        self.graph.upsert_entity(entity).await
    }

    /// Query the knowledge graph using BFS from a given entity.
    pub async fn query_graph(
        &self,
        start_entity: &str,
        max_depth: usize,
    ) -> Result<GraphQueryResult, String> {
        self.graph.query_bfs(start_entity, max_depth).await
    }

    /// Search the knowledge graph by text query.
    pub async fn search_graph(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<GraphTriple>, String> {
        self.graph.search(query, limit).await
    }

    /// Delete a graph entity and all related triples.
    pub async fn delete_graph_entity(&self, name: &str) -> Result<(), String> {
        self.graph.delete_entity(name).await
    }

    /// Get a graph entity by name.
    pub async fn get_graph_entity(
        &self,
        name: &str,
    ) -> Result<Option<GraphEntity>, String> {
        self.graph.get_entity(name).await
    }

    /// Query triples matching subject/predicate/object filters.
    pub async fn query_graph_triples(
        &self,
        subject: &str,
        predicate: &str,
        object: &str,
    ) -> Result<Vec<GraphTriple>, String> {
        self.graph.query_triples(subject, predicate, object).await
    }

    /// Get all related triples within depth hops via BFS.
    pub async fn get_related_triples(
        &self,
        entity_name: &str,
        depth: usize,
    ) -> Result<Vec<GraphTriple>, String> {
        self.graph.get_related(entity_name, depth).await
    }

    /// Get a reference to the underlying episodic store.
    pub fn get_episodic_store(&self) -> &Arc<dyn EpisodicStore> {
        &self.episodic
    }

    /// Get a reference to the underlying graph store.
    pub fn get_graph_store(&self) -> &Arc<dyn GraphStore> {
        &self.graph
    }

    /// Get graph store stats (entity_count, triple_count).
    pub async fn graph_stats(&self) -> Result<(usize, usize), String> {
        let entities = self.graph.entity_count().await?;
        let triples = self.graph.triple_count().await?;
        Ok((entities, triples))
    }
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

/// Parse a memory type string, defaulting to LongTerm for unknown values.
fn parse_memory_type_from_str(s: &str) -> MemoryType {
    match s {
        "short_term" => MemoryType::ShortTerm,
        "long_term" | "" => MemoryType::LongTerm,
        "episodic" => MemoryType::Episodic,
        "graph" => MemoryType::Graph,
        "daily" => MemoryType::Daily,
        _ => MemoryType::LongTerm,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
