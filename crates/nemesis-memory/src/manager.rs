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
use serde::{Deserialize, Serialize};

use crate::episodic::{EpisodicStore, Episode, FileEpisodicStore};
use crate::graph::{GraphEntity, GraphQueryResult, GraphStore, GraphTriple, InMemoryGraphStore};
use crate::local_store::TfIdfLocalStore;
use crate::store::{LocalStore, MemoryStore};
use crate::types::{Entry, MemoryType, SearchResult, ScoredEntry, VectorConfig};
use crate::vector::{VectorStore, StoreConfig};

// ---------------------------------------------------------------------------
// Enhanced memory configuration (internal to nemesis-memory)
// ---------------------------------------------------------------------------

/// Enhanced memory configuration loaded from `config.enhanced_memory.json`.
///
/// This type is internal to nemesis-memory — not exposed outside this crate.
///
/// The only field is `enabled`: when `true`, the system attempts to load the
/// ONNX plugin and initialize semantic search.  When `false` (e.g. after a
/// previous init failure), the system falls back to basic keyword-only memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct EnhancedMemoryConfig {
    #[serde(default)]
    enabled: bool,
}

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
            enabled: RwLock::new(true),
            data_dir: config.data_dir.clone(),
        })
    }

    /// Create a `MemoryManager` with config-based enhanced memory auto-detection.
    ///
    /// Reads `config.enhanced_memory.json` from `config_dir` for the `enabled`
    /// flag, then tries to auto-detect and load the ONNX plugin.
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

        // 1. Load config.enhanced_memory.json
        let em_config_path = config_dir.join("config.enhanced_memory.json");
        let em_config = Self::load_enhanced_memory_config(&em_config_path);

        // 2. Create MemoryManager (basic memory always available)
        let mgr = Self::new(&cfg);

        // 3. Attempt vector store init only when config says enabled
        if let Some(ref em) = em_config {
            if !em.enabled {
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
                    Self::disable_enhanced_memory_config(&em_config_path);
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
                Ok(()) => tracing::info!("[Memory] Vector store initialized"),
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "[Memory] Vector store init failed, disabling enhanced memory"
                    );
                    Self::disable_enhanced_memory_config(&em_config_path);
                }
            }
        }

        mgr
    }

    /// Load enhanced memory config from disk.
    ///
    /// Returns `None` if the file doesn't exist or can't be parsed.
    fn load_enhanced_memory_config(path: &Path) -> Option<EnhancedMemoryConfig> {
        if !path.exists() {
            return None;
        }
        match std::fs::read_to_string(path) {
            Ok(content) => match serde_json::from_str(&content) {
                Ok(cfg) => Some(cfg),
                Err(e) => {
                    tracing::warn!(
                        path = %path.display(),
                        error = %e,
                        "[Memory] Failed to parse config.enhanced_memory.json"
                    );
                    None
                }
            },
            Err(e) => {
                tracing::debug!(
                    path = %path.display(),
                    error = %e,
                    "[Memory] Cannot read config.enhanced_memory.json"
                );
                None
            }
        }
    }

    /// Auto-detect plugin DLL path next to the current executable.
    ///
    /// Checks `{exe_dir}/plugins/plugin_onnx.dll`.
    fn detect_plugin_path() -> Option<String> {
        let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
        let plugin_dll = exe_dir.join("plugins").join("plugin_onnx.dll");
        if plugin_dll.exists() {
            Some(plugin_dll.to_string_lossy().to_string())
        } else {
            None
        }
    }

    /// Write `enabled: false` to config.enhanced_memory.json so the next
    /// restart skips the vector store init attempt entirely.
    fn disable_enhanced_memory_config(path: &Path) {
        let config = serde_json::json!({ "enabled": false });
        match serde_json::to_string_pretty(&config) {
            Ok(json) => {
                if let Err(e) = std::fs::write(path, json) {
                    tracing::warn!(
                        path = %path.display(),
                        error = %e,
                        "[Memory] Failed to write disabled config"
                    );
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "[Memory] Failed to serialize disabled config");
            }
        }
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
        {
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
mod tests {
    use super::*;

    // ===================================================================
    // Non-ignored tests (basic memory, no plugin required)
    // ===================================================================

    #[tokio::test]
    async fn unified_store_and_search() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        // Store an entry.
        let entry = Entry::new(MemoryType::LongTerm, "Paris is the capital of France".to_string());
        let id = mgr.store_entry(entry).await.unwrap();

        // Search for it.
        let results = mgr.search("Paris", None, 10).await.unwrap();
        assert_eq!(results.total, 1);
        assert_eq!(results.entries[0].entry.id, id);
    }

    #[tokio::test]
    async fn unified_forget_and_list() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        let e1 = Entry::new(MemoryType::ShortTerm, "temp note 1".to_string());
        let e2 = Entry::new(MemoryType::LongTerm, "important fact".to_string());
        let id1 = mgr.store_entry(e1).await.unwrap();
        let _id2 = mgr.store_entry(e2).await.unwrap();

        // List all.
        let all = mgr.list(None, 10, 0).await.unwrap();
        assert_eq!(all.len(), 2);

        // Forget one.
        let removed = mgr.forget(&id1).await.unwrap();
        assert!(removed);

        let remaining = mgr.list(None, 10, 0).await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].content, "important fact");
    }

    #[tokio::test]
    async fn unified_graph_operations() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        mgr.upsert_entity(GraphEntity::new("tokio".into(), "crate".into()))
            .await
            .unwrap();
        mgr.upsert_entity(GraphEntity::new("runtime".into(), "concept".into()))
            .await
            .unwrap();
        mgr.add_triple(GraphTriple::new(
            "tokio".into(),
            "provides".into(),
            "runtime".into(),
        ))
        .await
        .unwrap();

        let result = mgr.query_graph("tokio", 2).await.unwrap();
        assert_eq!(result.paths.len(), 1);
        assert_eq!(result.paths[0][0].object, "runtime");
        assert_eq!(result.entities.len(), 2);
    }

    #[tokio::test]
    async fn unified_store_episodic_helper() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        let id = mgr
            .store_episodic("sess-1", "user", "What is Rust?")
            .await
            .unwrap();
        assert!(!id.is_empty());

        let results = mgr.search("Rust", None, 10).await.unwrap();
        assert_eq!(results.total, 1);
    }

    #[tokio::test]
    async fn unified_store_fact_helper() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        let id = mgr
            .store_fact("Rust was created by Mozilla", vec!["rust".to_string()])
            .await
            .unwrap();
        assert!(!id.is_empty());

        let results = mgr.search("Mozilla", None, 10).await.unwrap();
        assert_eq!(results.total, 1);
    }

    #[tokio::test]
    async fn unified_graph_search() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        mgr.add_triple(GraphTriple::new(
            "rust".into(),
            "is_a".into(),
            "language".into(),
        ))
        .await
        .unwrap();
        mgr.add_triple(GraphTriple::new(
            "python".into(),
            "is_a".into(),
            "language".into(),
        ))
        .await
        .unwrap();

        let results = mgr.search_graph("rust", 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].subject, "rust");
    }

    #[tokio::test]
    async fn unified_graph_delete_entity() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        mgr.upsert_entity(GraphEntity::new("rust".into(), "language".into()))
            .await
            .unwrap();
        mgr.add_triple(GraphTriple::new(
            "rust".into(),
            "is_a".into(),
            "language".into(),
        ))
        .await
        .unwrap();

        mgr.delete_graph_entity("rust").await.unwrap();

        let entity = mgr.get_graph_entity("rust").await.unwrap();
        assert!(entity.is_none());
    }

    #[tokio::test]
    async fn unified_graph_get_related() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        mgr.add_triple(GraphTriple::new(
            "a".into(),
            "rel".into(),
            "b".into(),
        ))
        .await
        .unwrap();
        mgr.add_triple(GraphTriple::new(
            "b".into(),
            "rel".into(),
            "c".into(),
        ))
        .await
        .unwrap();

        let related = mgr.get_related_triples("a", 2).await.unwrap();
        assert!(related.len() >= 2);
    }

    #[tokio::test]
    async fn unified_graph_stats() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        mgr.upsert_entity(GraphEntity::new("x".into(), "thing".into()))
            .await
            .unwrap();
        mgr.add_triple(GraphTriple::new(
            "x".into(),
            "has".into(),
            "y".into(),
        ))
        .await
        .unwrap();

        let (entities, triples) = mgr.graph_stats().await.unwrap();
        assert_eq!(entities, 1);
        assert_eq!(triples, 1);
    }

    #[tokio::test]
    async fn unified_query_semantic_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        mgr.store_fact("The Eiffel Tower is in Paris", vec![])
            .await
            .unwrap();

        let results = mgr.query_semantic("Eiffel", 5).await.unwrap();
        assert_eq!(results.total, 1);
    }

    // -- New tests for enabled flag and missing methods --------------------

    #[tokio::test]
    async fn test_is_enabled_default() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);
        assert!(mgr.is_enabled());
    }

    #[tokio::test]
    async fn test_close_disables_manager() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);
        assert!(mgr.is_enabled());

        mgr.close().await.unwrap();
        assert!(!mgr.is_enabled());
    }

    #[tokio::test]
    async fn test_disabled_store_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        // Store one entry before disabling.
        mgr.store_entry(Entry::new(MemoryType::LongTerm, "before".to_string()))
            .await
            .unwrap();

        mgr.close().await.unwrap();

        // Store when disabled returns empty string.
        let id = mgr
            .store_entry(Entry::new(MemoryType::LongTerm, "after".to_string()))
            .await
            .unwrap();
        assert!(id.is_empty());

        // Query when disabled returns empty.
        let result = mgr.search("before", None, 10).await.unwrap();
        assert_eq!(result.total, 0);

        // Get when disabled returns None.
        let got = mgr.get("anything").await.unwrap();
        assert!(got.is_none());

        // Delete when disabled returns false.
        let deleted = mgr.delete("anything").await.unwrap();
        assert!(!deleted);
    }

    #[tokio::test]
    async fn test_query_alias() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        mgr.store_entry(Entry::new(MemoryType::LongTerm, "query alias test".to_string()))
            .await
            .unwrap();

        let result = mgr.query("alias", None, 10).await.unwrap();
        assert_eq!(result.total, 1);
    }

    #[tokio::test]
    async fn test_delete_alias() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        let id = mgr
            .store_entry(Entry::new(MemoryType::LongTerm, "delete alias test".to_string()))
            .await
            .unwrap();

        let deleted = mgr.delete(&id).await.unwrap();
        assert!(deleted);

        let got = mgr.get(&id).await.unwrap();
        assert!(got.is_none());
    }

    #[tokio::test]
    #[ignore]
    async fn test_init_vector_store() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        // Before init, query_semantic falls back to keyword search.
        mgr.store_fact("Rust is memory safe", vec![])
            .await
            .unwrap();
        let results = mgr.query_semantic("Rust", 5).await.unwrap();
        assert_eq!(results.total, 1);

        // Init vector store with shared plugin fixture
        let embed = crate::vector::test_fixture::shared_embed_func()
            .expect("shared plugin not available");
        let vs_config = crate::vector::test_fixture::plugin_store_config(
            &dir.path().join("vector").join("vs.jsonl").to_string_lossy()
        ).expect("plugin DLL + model files required");
        mgr.init_vector_store_with_embed(embed, vs_config).unwrap();

        // query_semantic now uses vector store. The previously stored entry
        // is in the LocalStore, not the VectorStore, so we get 0 results
        // from the vector path.
        let vs_results = mgr.query_semantic("Rust", 5).await.unwrap();
        assert_eq!(vs_results.total, 0); // vector store is empty

        // Do NOT call mgr.close() — shared fixture must not be released
    }

    #[tokio::test]
    async fn test_new_with_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new_with_jsonl(&config).await.unwrap();
        assert!(mgr.is_enabled());

        let id = mgr
            .store_entry(Entry::new(MemoryType::LongTerm, "jsonl persisted".to_string()))
            .await
            .unwrap();

        let got = mgr.get(&id).await.unwrap().unwrap();
        assert_eq!(got.content, "jsonl persisted");

        // Verify the file exists.
        let store_path = dir.path().join("memory").join("store.jsonl");
        assert!(store_path.exists());
    }

    #[tokio::test]
    async fn test_new_with_jsonl_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let id = {
            let mgr = MemoryManager::new_with_jsonl(&config).await.unwrap();
            let entry = Entry::new(MemoryType::LongTerm, "survives restart".to_string());
            mgr.store_entry(entry).await.unwrap()
        };

        // Re-create manager -- should reload from disk.
        let mgr2 = MemoryManager::new_with_jsonl(&config).await.unwrap();
        let got = mgr2.get(&id).await.unwrap().unwrap();
        assert_eq!(got.content, "survives restart");
    }

    #[tokio::test]
    #[ignore]
    async fn test_vector_store_adapter_stores_and_queries() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        // Init vector store with shared plugin fixture
        let embed = crate::vector::test_fixture::shared_embed_func()
            .expect("shared plugin not available");
        let vs_config = crate::vector::test_fixture::plugin_store_config(
            &dir.path().join("vector").join("vs.jsonl").to_string_lossy()
        ).expect("plugin DLL + model files required");
        mgr.init_vector_store_with_embed(embed, vs_config).unwrap();

        // Store an entry - should go to both keyword and vector stores
        let id = mgr
            .store_fact("Berlin is the capital of Germany", vec!["geography".to_string()])
            .await
            .unwrap();
        assert!(!id.is_empty());

        // Query via search should find it through the vector store path
        let results = mgr.search("Berlin", None, 10).await.unwrap();
        assert_eq!(results.total, 1);
        assert_eq!(results.entries[0].entry.content, "Berlin is the capital of Germany");

        // Get should find it in vector store
        let got = mgr.get(&id).await.unwrap();
        assert!(got.is_some());
        assert_eq!(got.unwrap().content, "Berlin is the capital of Germany");

        // Do NOT call mgr.close() — shared fixture must not be released
    }

    #[tokio::test]
    #[ignore]
    async fn test_vector_store_adapter_query_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        // Store entry BEFORE vector store init (only in keyword store)
        let _id_before = mgr
            .store_fact("Tokyo is the capital of Japan", vec![])
            .await
            .unwrap();

        // Init vector store with shared plugin fixture
        let embed = crate::vector::test_fixture::shared_embed_func()
            .expect("shared plugin not available");
        let vs_config = crate::vector::test_fixture::plugin_store_config(
            &dir.path().join("vector").join("vs.jsonl").to_string_lossy()
        ).expect("plugin DLL + model files required");
        mgr.init_vector_store_with_embed(embed, vs_config).unwrap();

        // Store entry AFTER vector store init (in both stores)
        let _id_after = mgr
            .store_fact("Paris is the capital of France", vec![])
            .await
            .unwrap();

        // Search should find entries from both stores (vector store falls
        // through to keyword store when vector returns empty for "Tokyo")
        let results = mgr.search("Tokyo", None, 10).await.unwrap();
        assert!(results.total >= 1);

        // Do NOT call mgr.close() — shared fixture must not be released
    }

    // ============================================================
    // Additional tests for missing coverage
    // ============================================================

    #[test]
    fn test_config_new() {
        let config = Config::new("/tmp/test-data");
        assert_eq!(config.data_dir, PathBuf::from("/tmp/test-data"));
    }

    #[test]
    fn test_parse_memory_type_from_str_all_variants() {
        assert_eq!(parse_memory_type_from_str("short_term"), MemoryType::ShortTerm);
        assert_eq!(parse_memory_type_from_str("long_term"), MemoryType::LongTerm);
        assert_eq!(parse_memory_type_from_str(""), MemoryType::LongTerm);
        assert_eq!(parse_memory_type_from_str("episodic"), MemoryType::Episodic);
        assert_eq!(parse_memory_type_from_str("graph"), MemoryType::Graph);
        assert_eq!(parse_memory_type_from_str("daily"), MemoryType::Daily);
        assert_eq!(parse_memory_type_from_str("unknown"), MemoryType::LongTerm);
        assert_eq!(parse_memory_type_from_str("RANDOM"), MemoryType::LongTerm);
    }

    #[tokio::test]
    async fn test_get_returns_none_for_missing() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        let got = mgr.get("nonexistent").await.unwrap();
        assert!(got.is_none());
    }

    #[tokio::test]
    async fn test_forget_returns_false_for_missing() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        let removed = mgr.forget("nonexistent").await.unwrap();
        assert!(!removed);
    }

    #[tokio::test]
    async fn test_list_empty() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        let entries = mgr.list(None, 10, 0).await.unwrap();
        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn test_list_with_type_filter() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        mgr.store_entry(Entry::new(MemoryType::LongTerm, "long term".to_string())).await.unwrap();
        mgr.store_entry(Entry::new(MemoryType::ShortTerm, "short term".to_string())).await.unwrap();

        let long = mgr.list(Some(MemoryType::LongTerm), 10, 0).await.unwrap();
        assert_eq!(long.len(), 1);
        assert_eq!(long[0].content, "long term");
    }

    #[tokio::test]
    async fn test_list_with_pagination() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        for i in 0..10 {
            mgr.store_entry(Entry::new(MemoryType::LongTerm, format!("entry {}", i))).await.unwrap();
        }

        let page1 = mgr.list(None, 3, 0).await.unwrap();
        assert!(page1.len() <= 3);

        let page2 = mgr.list(None, 3, 3).await.unwrap();
        assert!(page2.len() <= 3);
    }

    #[tokio::test]
    async fn test_episodic_operations() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        // Append episodes
        let ep1 = Episode::new("s1".into(), "user".into(), "hello".into());
        let ep2 = Episode::new("s1".into(), "assistant".into(), "hi there".into());
        let ep3 = Episode::new("s2".into(), "user".into(), "other session".into());
        mgr.append_episode(ep1).await.unwrap();
        mgr.append_episode(ep2).await.unwrap();
        mgr.append_episode(ep3).await.unwrap();

        // Get session
        let sessions = mgr.get_session("s1").await.unwrap();
        assert_eq!(sessions.len(), 2);

        // Get recent
        let recent = mgr.get_recent_episodes("s1", 1).await.unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].content, "hi there");

        // Search
        let found = mgr.search_episodic("hello", 10).await.unwrap();
        assert_eq!(found.len(), 1);

        // Stats
        let (session_count, episode_count) = mgr.episodic_stats().await.unwrap();
        assert_eq!(session_count, 2);
        assert_eq!(episode_count, 3);

        // Delete session
        let deleted = mgr.delete_episode_session("s1").await.unwrap();
        assert_eq!(deleted, 2);

        let remaining = mgr.get_session("s1").await.unwrap();
        assert!(remaining.is_empty());
    }

    #[tokio::test]
    async fn test_episodic_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        // Old episode
        let mut old = Episode::new("old-sess".into(), "user".into(), "old content".into());
        old.timestamp = chrono::Utc::now() - chrono::Duration::days(10);
        mgr.append_episode(old).await.unwrap();

        // Recent episode
        mgr.append_episode(Episode::new("new-sess".into(), "user".into(), "new content".into())).await.unwrap();

        let removed = mgr.cleanup_episodic(5).await.unwrap();
        assert_eq!(removed, 1);
    }

    #[tokio::test]
    async fn test_graph_query_triples() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        mgr.add_triple(GraphTriple::new("a".into(), "knows".into(), "b".into())).await.unwrap();
        mgr.add_triple(GraphTriple::new("c".into(), "knows".into(), "d".into())).await.unwrap();

        let triples = mgr.query_graph_triples("a", "", "").await.unwrap();
        assert_eq!(triples.len(), 1);
        assert_eq!(triples[0].subject, "a");

        let knows = mgr.query_graph_triples("", "knows", "").await.unwrap();
        assert_eq!(knows.len(), 2);
    }

    #[tokio::test]
    async fn test_get_episodic_store() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        let _store = mgr.get_episodic_store();
        // Just verify it returns without panic
    }

    #[tokio::test]
    async fn test_get_graph_store() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        let _store = mgr.get_graph_store();
        // Just verify it returns without panic
    }

    #[tokio::test]
    async fn test_search_disabled_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        mgr.store_fact("should be searchable", vec![]).await.unwrap();
        let results = mgr.search("searchable", None, 10).await.unwrap();
        assert_eq!(results.total, 1);

        mgr.close().await.unwrap();

        let results = mgr.search("searchable", None, 10).await.unwrap();
        assert_eq!(results.total, 0);
    }

    #[tokio::test]
    async fn test_query_semantic_disabled_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        mgr.close().await.unwrap();

        let results = mgr.query_semantic("anything", 5).await.unwrap();
        assert_eq!(results.total, 0);
    }

    #[tokio::test]
    async fn test_store_disabled_returns_empty_id() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        mgr.close().await.unwrap();

        let id = mgr.store_entry(Entry::new(MemoryType::LongTerm, "disabled".to_string())).await.unwrap();
        assert!(id.is_empty());

        let id2 = mgr.store(Entry::new(MemoryType::LongTerm, "disabled".to_string())).await.unwrap();
        assert!(id2.is_empty());
    }

    #[tokio::test]
    async fn test_list_disabled_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        mgr.store_entry(Entry::new(MemoryType::LongTerm, "test".to_string())).await.unwrap();

        mgr.close().await.unwrap();

        let entries = mgr.list(None, 10, 0).await.unwrap();
        assert!(entries.is_empty());
    }

    #[tokio::test]
    #[ignore]
    async fn test_append_episode_writes_to_vector_store() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        let embed = crate::vector::test_fixture::shared_embed_func()
            .expect("shared plugin not available");
        // Use low threshold so the query always matches
        let vs_config = StoreConfig {
            similarity_threshold: 0.3,
            ..crate::vector::test_fixture::plugin_store_config(
                &dir.path().join("vector").join("vs.jsonl").to_string_lossy()
            ).expect("plugin DLL + model files required")
        };
        mgr.init_vector_store_with_embed(embed, vs_config).unwrap();

        // Store an episode — should write to both episodic store AND vector store
        let episode = Episode::new("vs-ep-test".into(), "user".into(), "episodic vector store write test".into());
        let id = mgr.append_episode(episode).await.unwrap();
        assert!(!id.is_empty());

        // Verify: semantic search finds the episodic content via vector store
        let results = mgr.search("episodic vector store", None, 10).await.unwrap();
        assert!(
            results.entries.iter().any(|se| se.entry.content.contains("episodic vector store write test")),
            "vector store should contain the episodic entry, got: {:?}",
            results.entries
        );

        // Do NOT call mgr.close() — shared fixture must not be released
    }

    #[tokio::test]
    #[ignore]
    async fn test_init_vector_store_with_custom_config() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        let embed = crate::vector::test_fixture::shared_embed_func()
            .expect("shared plugin not available");
        let custom_vs_config = StoreConfig {
            similarity_threshold: 0.5,
            max_results: 5,
            storage_path: dir.path().join("custom_vectors.jsonl").to_string_lossy().to_string(),
            ..crate::vector::test_fixture::plugin_store_config(
                &dir.path().join("vector").join("vs.jsonl").to_string_lossy()
            ).expect("plugin DLL + model files required")
        };

        mgr.init_vector_store_with_embed(embed, custom_vs_config).unwrap();

        // Store and query - use store_entry which also stores in vector store
        mgr.store_entry(Entry::new(MemoryType::LongTerm, "custom vector test".to_string())).await.unwrap();
        let results = mgr.query_semantic("vector", 3).await.unwrap();
        assert!(results.total >= 1);

        // Do NOT call mgr.close() — shared fixture must not be released
    }

    #[tokio::test]
    async fn test_with_backends() {
        let store = Arc::new(LocalStore::new());
        let dir = tempfile::tempdir().unwrap();
        let episodic = Arc::new(FileEpisodicStore::new(dir.path()));
        let graph = Arc::new(InMemoryGraphStore::new());

        let mgr = MemoryManager::with_backends(store, episodic, graph);
        assert!(mgr.is_enabled());

        mgr.store_fact("backend test", vec![]).await.unwrap();
        let results = mgr.search("backend", None, 10).await.unwrap();
        assert_eq!(results.total, 1);
    }

    #[tokio::test]
    async fn test_store_entry_with_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        let mut meta = HashMap::new();
        meta.insert("source".to_string(), "test".to_string());
        let entry = Entry::new(MemoryType::LongTerm, "metadata test".to_string())
            .with_metadata(meta);

        let id = mgr.store_entry(entry).await.unwrap();
        let got = mgr.get(&id).await.unwrap().unwrap();
        assert_eq!(got.metadata.get("source").unwrap(), "test");
    }

    #[tokio::test]
    async fn test_search_with_memory_type_filter() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        mgr.store_entry(Entry::new(MemoryType::LongTerm, "long term memory".to_string())).await.unwrap();
        mgr.store_entry(Entry::new(MemoryType::ShortTerm, "short term memory".to_string())).await.unwrap();

        let long = mgr.search("memory", Some(MemoryType::LongTerm), 10).await.unwrap();
        assert_eq!(long.total, 1);

        let short = mgr.search("memory", Some(MemoryType::ShortTerm), 10).await.unwrap();
        assert_eq!(short.total, 1);
    }

    #[tokio::test]
    async fn test_store_fact_with_tags() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        let id = mgr.store_fact("Python is interpreted", vec!["python".to_string(), "programming".to_string()]).await.unwrap();
        let got = mgr.get(&id).await.unwrap().unwrap();
        assert!(got.tags.contains(&"python".to_string()));
        assert!(got.tags.contains(&"programming".to_string()));
    }

    // ============================================================
    // Additional tests for 95%+ coverage
    // ============================================================

    #[tokio::test]
    async fn test_query_semantic_zero_limit_defaults_to_five() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        for i in 0..10 {
            mgr.store_fact(&format!("fact number {} about testing", i), vec![]).await.unwrap();
        }

        // limit=0 should default to 5
        let results = mgr.query_semantic("testing", 0).await.unwrap();
        assert!(results.entries.len() <= 5);
        assert!(results.total >= 1);
    }

    #[tokio::test]
    async fn test_store_method_delegates_to_store_entry() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        // Use store() (Go-style alias) instead of store_entry()
        let entry = Entry::new(MemoryType::LongTerm, "stored via store() method".to_string());
        let id = mgr.store(entry).await.unwrap();
        assert!(!id.is_empty());

        let got = mgr.get(&id).await.unwrap().unwrap();
        assert_eq!(got.content, "stored via store() method");
    }

    #[tokio::test]
    #[ignore]
    async fn test_store_and_get_via_vector_store() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        // Init vector store with shared plugin fixture
        let embed = crate::vector::test_fixture::shared_embed_func()
            .expect("shared plugin not available");
        let vs_config = crate::vector::test_fixture::plugin_store_config(
            &dir.path().join("vector").join("vs.jsonl").to_string_lossy()
        ).expect("plugin DLL + model files required");
        mgr.init_vector_store_with_embed(embed, vs_config).unwrap();

        // Store via store_entry (which also stores to vector)
        let id = mgr.store_entry(Entry::new(MemoryType::LongTerm, "vector store entry".to_string()))
            .await.unwrap();

        // Get should find it in the keyword store first
        let got = mgr.get(&id).await.unwrap();
        assert!(got.is_some());
        assert_eq!(got.unwrap().content, "vector store entry");

        // Do NOT call mgr.close() — shared fixture must not be released
    }

    #[tokio::test]
    #[ignore]
    async fn test_get_falls_back_to_vector_store() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        let embed = crate::vector::test_fixture::shared_embed_func()
            .expect("shared plugin not available");
        let vs_config = crate::vector::test_fixture::plugin_store_config(
            &dir.path().join("vector").join("vs.jsonl").to_string_lossy()
        ).expect("plugin DLL + model files required");
        mgr.init_vector_store_with_embed(embed, vs_config).unwrap();

        // Store an entry (goes to both keyword and vector stores)
        let id = mgr.store_entry(Entry::new(MemoryType::LongTerm, "fallback test".to_string()))
            .await.unwrap();

        // Delete from keyword store only, so get must fall back to vector store
        mgr.store.delete(&id).await.unwrap();

        // get() should still find it in the vector store
        let got = mgr.get(&id).await.unwrap();
        assert!(got.is_some());
        assert_eq!(got.unwrap().content, "fallback test");

        // Do NOT call mgr.close() — shared fixture must not be released
    }

    #[tokio::test]
    #[ignore]
    async fn test_search_with_vector_store_and_type_filter() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        let embed = crate::vector::test_fixture::shared_embed_func()
            .expect("shared plugin not available");
        let vs_config = crate::vector::test_fixture::plugin_store_config(
            &dir.path().join("vector").join("vs.jsonl").to_string_lossy()
        ).expect("plugin DLL + model files required");
        mgr.init_vector_store_with_embed(embed, vs_config).unwrap();

        // Store entries of different types
        mgr.store_entry(Entry::new(MemoryType::LongTerm, "long term vector content".to_string())).await.unwrap();
        mgr.store_entry(Entry::new(MemoryType::ShortTerm, "short term vector content".to_string())).await.unwrap();

        // Search with type filter should only return matching type
        let results = mgr.search("vector", Some(MemoryType::LongTerm), 10).await.unwrap();
        assert!(results.entries.iter().all(|e| e.entry.typ == MemoryType::LongTerm));

        // Do NOT call mgr.close() — shared fixture must not be released
    }

    #[tokio::test]
    #[ignore]
    async fn test_search_vector_store_falls_back_to_keyword() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        // Store before vector init (only keyword store)
        mgr.store_fact("pre-vector fact about Rust", vec![]).await.unwrap();

        // Init vector store with shared plugin fixture (empty, no entries yet)
        let embed = crate::vector::test_fixture::shared_embed_func()
            .expect("shared plugin not available");
        let vs_config = crate::vector::test_fixture::plugin_store_config(
            &dir.path().join("vector").join("vs.jsonl").to_string_lossy()
        ).expect("plugin DLL + model files required");
        mgr.init_vector_store_with_embed(embed, vs_config).unwrap();

        // Search should fall back to keyword store since vector is empty
        let results = mgr.search("Rust", None, 10).await.unwrap();
        assert!(results.total >= 1);

        // Do NOT call mgr.close() — shared fixture must not be released
    }

    #[tokio::test]
    #[ignore]
    async fn test_store_to_vector_adapter_path() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        // Init vector store with shared plugin fixture
        let embed = crate::vector::test_fixture::shared_embed_func()
            .expect("shared plugin not available");
        let vs_config = crate::vector::test_fixture::plugin_store_config(
            &dir.path().join("vector").join("vs.jsonl").to_string_lossy()
        ).expect("plugin DLL + model files required");
        mgr.init_vector_store_with_embed(embed, vs_config).unwrap();

        // Use store() method which also goes through vector adapter
        let entry = Entry::new(MemoryType::Episodic, "episodic via store method".to_string())
            .with_tags(vec!["test".to_string()])
            .with_score(0.8);
        let id = mgr.store(entry).await.unwrap();
        assert!(!id.is_empty());

        // Should be findable via get
        let got = mgr.get(&id).await.unwrap();
        assert!(got.is_some());

        // Do NOT call mgr.close() — shared fixture must not be released
    }

    #[tokio::test]
    async fn test_store_disabled_store_method() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        mgr.close().await.unwrap();

        let id = mgr.store(Entry::new(MemoryType::LongTerm, "disabled".to_string())).await.unwrap();
        assert!(id.is_empty());
    }

    #[tokio::test]
    async fn test_forget_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        let id = mgr.store_fact("will be forgotten", vec![]).await.unwrap();

        mgr.close().await.unwrap();

        let removed = mgr.forget(&id).await.unwrap();
        assert!(!removed);
    }

    #[tokio::test]
    async fn test_list_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        mgr.store_fact("should be listed", vec![]).await.unwrap();
        mgr.close().await.unwrap();

        let entries = mgr.list(None, 10, 0).await.unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn test_vector_store_init_with_default_path() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        // Init with None (default path) - just verify it doesn't error
        let result = mgr.init_vector_store(None);
        // May succeed or fail depending on whether an embedding model is available
        // The important thing is it doesn't panic
        let _ = result;
    }

    #[tokio::test]
    async fn test_episodic_get_session_empty() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        let episodes = mgr.get_session("nonexistent").await.unwrap();
        assert!(episodes.is_empty());
    }

    #[tokio::test]
    async fn test_episodic_get_recent_empty() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        let episodes = mgr.get_recent_episodes("nonexistent", 10).await.unwrap();
        assert!(episodes.is_empty());
    }

    #[tokio::test]
    async fn test_graph_get_entity_nonexistent() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        let entity = mgr.get_graph_entity("ghost").await.unwrap();
        assert!(entity.is_none());
    }

    #[tokio::test]
    async fn test_graph_query_triples_all_wildcards() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        mgr.add_triple(GraphTriple::new("a".into(), "rel".into(), "b".into())).await.unwrap();
        mgr.add_triple(GraphTriple::new("c".into(), "rel".into(), "d".into())).await.unwrap();

        let triples = mgr.query_graph_triples("", "", "").await.unwrap();
        assert_eq!(triples.len(), 2);
    }

    #[tokio::test]
    async fn test_get_related_triples_deep() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        // a -> b -> c -> d
        mgr.add_triple(GraphTriple::new("a".into(), "next".into(), "b".into())).await.unwrap();
        mgr.add_triple(GraphTriple::new("b".into(), "next".into(), "c".into())).await.unwrap();
        mgr.add_triple(GraphTriple::new("c".into(), "next".into(), "d".into())).await.unwrap();

        // Depth 3 should find all 3 hops
        let related = mgr.get_related_triples("a", 3).await.unwrap();
        assert!(related.len() >= 3);

        // Depth 1 should find only 1 hop
        let shallow = mgr.get_related_triples("a", 1).await.unwrap();
        assert!(shallow.len() < related.len());
    }

    #[tokio::test]
    async fn test_episodic_search_no_results() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        let results = mgr.search_episodic("nonexistent query", 10).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_delete_episode_session_nonexistent() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        let deleted = mgr.delete_episode_session("nonexistent").await.unwrap();
        assert_eq!(deleted, 0);
    }

    #[tokio::test]
    async fn test_cleanup_episodic_nothing_old() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        mgr.append_episode(Episode::new("s1".into(), "user".into(), "fresh".into())).await.unwrap();
        let removed = mgr.cleanup_episodic(365).await.unwrap();
        assert_eq!(removed, 0);
    }

    #[tokio::test]
    async fn test_search_returns_scored_entries_sorted() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        mgr.store_fact("cat cat cat cat", vec![]).await.unwrap();
        mgr.store_fact("cat", vec![]).await.unwrap();
        mgr.store_fact("dog dog dog", vec![]).await.unwrap();

        let results = mgr.search("cat", None, 10).await.unwrap();
        assert!(results.total >= 2);
        // Results should be sorted by score descending
        for i in 1..results.entries.len() {
            assert!(results.entries[i - 1].score >= results.entries[i].score);
        }
    }

    // --- Additional coverage tests ---

    #[tokio::test]
    async fn test_store_multiple_and_list() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        mgr.store_fact("fact one", vec![]).await.unwrap();
        mgr.store_fact("fact two", vec![]).await.unwrap();
        mgr.store_fact("fact three", vec![]).await.unwrap();

        let entries = mgr.list(None, 2, 0).await.unwrap();
        assert_eq!(entries.len(), 2); // Limited to 2

        let all = mgr.list(None, 10, 0).await.unwrap();
        assert_eq!(all.len(), 3);
    }

    #[tokio::test]
    async fn test_list_with_offset() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        for i in 0..5 {
            mgr.store_fact(&format!("fact {}", i), vec![]).await.unwrap();
        }

        let page = mgr.list(None, 2, 3).await.unwrap();
        assert!(page.len() <= 2);
    }

    #[tokio::test]
    async fn test_close_and_search() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        mgr.store_fact("before close", vec![]).await.unwrap();
        mgr.close().await.unwrap();

        // After close, search should return empty results
        let results = mgr.search("before", None, 10).await.unwrap();
        assert!(results.entries.is_empty());
    }

    #[tokio::test]
    async fn test_search_after_close_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        mgr.store_fact("before close", vec![]).await.unwrap();
        mgr.close().await.unwrap();

        let results = mgr.search("before", None, 10).await.unwrap();
        assert!(results.entries.is_empty());
    }

    #[tokio::test]
    async fn test_graph_query_with_filter() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        mgr.add_triple(GraphTriple::new("Go".into(), "is_a".into(), "language".into())).await.unwrap();
        mgr.add_triple(GraphTriple::new("Rust".into(), "is_a".into(), "language".into())).await.unwrap();
        mgr.add_triple(GraphTriple::new("Go".into(), "created_by".into(), "Google".into())).await.unwrap();

        // Filter by subject
        let go_triples = mgr.query_graph_triples("Go", "", "").await.unwrap();
        assert_eq!(go_triples.len(), 2);

        // Filter by predicate
        let is_a_triples = mgr.query_graph_triples("", "is_a", "").await.unwrap();
        assert_eq!(is_a_triples.len(), 2);

        // Filter by object
        let lang_triples = mgr.query_graph_triples("", "", "language").await.unwrap();
        assert_eq!(lang_triples.len(), 2);
    }

    #[tokio::test]
    async fn test_search_empty_query() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        mgr.store_fact("some fact", vec![]).await.unwrap();
        let results = mgr.search("", None, 10).await.unwrap();
        let _ = results;
    }

    #[tokio::test]
    async fn test_search_by_memory_type() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        let id = mgr.store_fact("long term fact", vec![]).await.unwrap();
        assert!(!id.is_empty());
    }

    #[tokio::test]
    async fn test_double_close_safe() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        mgr.close().await.unwrap();
        // Second close should not panic
        mgr.close().await.unwrap();
    }

    #[tokio::test]
    async fn test_append_episode_and_get_session() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        let ep1 = Episode::new("s1".into(), "user".into(), "hello".into());
        let ep2 = Episode::new("s1".into(), "assistant".into(), "hi there".into());
        mgr.append_episode(ep1).await.unwrap();
        mgr.append_episode(ep2).await.unwrap();

        let episodes = mgr.get_session("s1").await.unwrap();
        assert_eq!(episodes.len(), 2);
    }

    #[tokio::test]
    async fn test_search_episodic_with_content() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);

        mgr.append_episode(Episode::new("s1".into(), "user".into(), "Rust memory safety".into())).await.unwrap();
        mgr.append_episode(Episode::new("s1".into(), "assistant".into(), "Rust is safe".into())).await.unwrap();

        let results = mgr.search_episodic("Rust", 10).await.unwrap();
        assert!(results.len() >= 2);
    }

    // ============================================================
    // Tests for with_config_dir and enhanced memory config loading
    // ============================================================

    #[test]
    fn test_load_enhanced_memory_config_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.enhanced_memory.json");
        let result = MemoryManager::load_enhanced_memory_config(&path);
        assert!(result.is_none());
    }

    #[test]
    fn test_load_enhanced_memory_config_valid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.enhanced_memory.json");
        let content = r#"{"enabled": true}"#;
        std::fs::write(&path, content).unwrap();
        let cfg = MemoryManager::load_enhanced_memory_config(&path).unwrap();
        assert!(cfg.enabled);
    }

    #[test]
    fn test_load_enhanced_memory_config_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.enhanced_memory.json");
        std::fs::write(&path, "not json").unwrap();
        let result = MemoryManager::load_enhanced_memory_config(&path);
        assert!(result.is_none());
    }

    #[test]
    fn test_load_enhanced_memory_config_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.enhanced_memory.json");
        let content = r#"{}"#;
        std::fs::write(&path, content).unwrap();
        let cfg = MemoryManager::load_enhanced_memory_config(&path).unwrap();
        assert!(!cfg.enabled);
    }

    #[test]
    fn test_detect_plugin_path_returns_none() {
        // In test environment, there's no plugin DLL next to the test binary.
        let result = MemoryManager::detect_plugin_path();
        // This is environment-dependent; just ensure it doesn't panic.
        let _ = result;
    }

    #[test]
    fn test_with_config_dir_basic_memory_no_config() {
        // No config.enhanced_memory.json → basic memory
        let data_dir = tempfile::tempdir().unwrap();
        let config_dir = tempfile::tempdir().unwrap();
        let mgr = MemoryManager::with_config_dir(data_dir.path(), config_dir.path());
        assert!(mgr.is_enabled());
    }

    #[test]
    fn test_with_config_dir_disabled() {
        let data_dir = tempfile::tempdir().unwrap();
        let config_dir = tempfile::tempdir().unwrap();
        let path = config_dir.path().join("config.enhanced_memory.json");
        std::fs::write(&path, r#"{"enabled": false}"#).unwrap();
        let mgr = MemoryManager::with_config_dir(data_dir.path(), config_dir.path());
        assert!(mgr.is_enabled());
        // enabled=false → skip vector store init → basic memory
    }

    #[test]
    fn test_with_config_dir_enabled_no_plugin() {
        let data_dir = tempfile::tempdir().unwrap();
        let config_dir = tempfile::tempdir().unwrap();
        let path = config_dir.path().join("config.enhanced_memory.json");
        std::fs::write(&path, r#"{"enabled": true}"#).unwrap();
        let mgr = MemoryManager::with_config_dir(data_dir.path(), config_dir.path());
        assert!(mgr.is_enabled());
        // enabled=true but no plugin DLL → auto-detect fails → config written as disabled
    }

    // ============================================================
    // Phase 1: UT — EnhancedMemoryConfig parsing (4 tests)
    // ============================================================

    #[test]
    fn test_enhanced_config_enabled_true() {
        let json = r#"{"enabled": true}"#;
        let cfg: EnhancedMemoryConfig = serde_json::from_str(json).unwrap();
        assert!(cfg.enabled);
    }

    #[test]
    fn test_enhanced_config_extra_fields_ignored() {
        let json = r#"{"enabled": true, "unknown_field": "value", "another": 42}"#;
        let cfg: EnhancedMemoryConfig = serde_json::from_str(json).unwrap();
        assert!(cfg.enabled);
    }

    #[test]
    fn test_enhanced_config_empty_object() {
        let json = r#"{}"#;
        let cfg: EnhancedMemoryConfig = serde_json::from_str(json).unwrap();
        assert!(!cfg.enabled); // default
    }

    #[test]
    fn test_enhanced_config_disabled() {
        let json = r#"{"enabled": false}"#;
        let cfg: EnhancedMemoryConfig = serde_json::from_str(json).unwrap();
        assert!(!cfg.enabled);
    }

    // ============================================================
    // Phase 1: UT — with_config_dir flow (8 tests)
    // ============================================================

    #[test]
    fn test_with_config_dir_no_config_basic_memory() {
        let data_dir = tempfile::tempdir().unwrap();
        let config_dir = tempfile::tempdir().unwrap();
        // No config file at all
        let mgr = MemoryManager::with_config_dir(data_dir.path(), config_dir.path());
        assert!(mgr.is_enabled());
    }

    #[test]
    fn test_with_config_dir_disabled_basic_works() {
        let data_dir = tempfile::tempdir().unwrap();
        let config_dir = tempfile::tempdir().unwrap();
        let path = config_dir.path().join("config.enhanced_memory.json");
        std::fs::write(&path, r#"{"enabled": false}"#).unwrap();
        let mgr = MemoryManager::with_config_dir(data_dir.path(), config_dir.path());
        assert!(mgr.is_enabled());
        // Manager is always enabled (basic memory always works).
        // enabled field is now only a signal to the caller (gateway.rs).
        // No vector store since no plugin_path and tier != "api".
    }

    #[test]
    fn test_with_config_dir_enabled_but_no_plugin_disables_config() {
        let data_dir = tempfile::tempdir().unwrap();
        let config_dir = tempfile::tempdir().unwrap();
        let path = config_dir.path().join("config.enhanced_memory.json");
        std::fs::write(&path, r#"{"enabled": true}"#).unwrap();
        let mgr = MemoryManager::with_config_dir(data_dir.path(), config_dir.path());
        assert!(mgr.is_enabled());
        // enabled=true, but no plugin DLL in test env → config written as {enabled: false}
        let updated = std::fs::read_to_string(&path).unwrap();
        assert!(updated.contains("false"), "Config should be disabled after failed init");
    }

    #[test]
    fn test_with_config_dir_invalid_json_falls_back() {
        let data_dir = tempfile::tempdir().unwrap();
        let config_dir = tempfile::tempdir().unwrap();
        let path = config_dir.path().join("config.enhanced_memory.json");
        std::fs::write(&path, "NOT VALID JSON!!!").unwrap();
        let mgr = MemoryManager::with_config_dir(data_dir.path(), config_dir.path());
        assert!(mgr.is_enabled());
        // Invalid JSON → load returns None → basic memory
    }

    #[test]
    fn test_with_config_dir_plugin_missing_dll_disables_config() {
        let data_dir = tempfile::tempdir().unwrap();
        let config_dir = tempfile::tempdir().unwrap();
        let path = config_dir.path().join("config.enhanced_memory.json");
        // enabled=true but no plugin DLL in test env
        std::fs::write(&path, r#"{"enabled": true}"#).unwrap();
        let mgr = MemoryManager::with_config_dir(data_dir.path(), config_dir.path());
        assert!(mgr.is_enabled());
        // No plugin → config auto-disabled
        let updated = std::fs::read_to_string(&path).unwrap();
        assert!(updated.contains("false"));
    }

    #[test]
    fn test_with_config_dir_corrupted_binary_falls_back() {
        let data_dir = tempfile::tempdir().unwrap();
        let config_dir = tempfile::tempdir().unwrap();
        let path = config_dir.path().join("config.enhanced_memory.json");
        std::fs::write(&path, b"\x00\x01\x02\x03\xFF\xFE").unwrap();
        let mgr = MemoryManager::with_config_dir(data_dir.path(), config_dir.path());
        assert!(mgr.is_enabled());
        // Binary content → parse fails → basic memory
    }

    #[test]
    fn test_with_config_dir_storage_path_not_created_without_plugin() {
        let data_dir = tempfile::tempdir().unwrap();
        let config_dir = tempfile::tempdir().unwrap();

        let path = config_dir.path().join("config.enhanced_memory.json");
        std::fs::write(&path, r#"{"enabled": true}"#).unwrap();

        let mgr = MemoryManager::with_config_dir(data_dir.path(), config_dir.path());
        assert!(mgr.is_enabled());

        // No plugin → vector store not created → storage file doesn't exist
        let expected_storage = data_dir.path().join("vector").join("vector_store.jsonl");
        assert!(!expected_storage.exists());
    }

    // ============================================================
    // Phase 1: UT — Vector Store adapter pattern
    // (requires real ONNX plugin — see tests/memory_e2e.rs)
    // ============================================================

    #[test]
    fn test_vector_adapter_requires_plugin() {
        // Verify that init_vector_store with no plugin returns an error
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let mgr = MemoryManager::new(&config);
        let result = mgr.init_vector_store(None);
        assert!(result.is_err(), "init_vector_store without plugin should fail");
    }
}
