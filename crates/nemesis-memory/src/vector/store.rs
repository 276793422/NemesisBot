//! Vector store - semantic search using local embeddings.
//!
//! Provides in-memory vector search with JSONL persistence. Entries are
//! embedded using the configured embedding function and stored for
//! similarity-based retrieval.

use std::collections::HashMap;
use std::path::PathBuf;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::types::VectorConfig;
use crate::vector::embedding::new_embedding_func;

/// Configuration for the vector store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreConfig {
    /// Embedding tier.
    pub embedding_tier: String,
    /// Plugin path.
    pub plugin_path: Option<String>,
    /// Config directory containing config.enhanced_memory.json.
    #[serde(skip)]
    pub config_dir: Option<String>,
    /// Maximum results per query.
    pub max_results: usize,
    /// Similarity threshold [0, 1].
    pub similarity_threshold: f64,
    /// Storage path for JSONL persistence.
    pub storage_path: String,
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            embedding_tier: "plugin".into(),
            plugin_path: None,
            config_dir: None,
            max_results: 10,
            similarity_threshold: 0.7,
            storage_path: String::new(),
        }
    }
}

/// A memory entry for the vector store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorEntry {
    /// Unique ID.
    pub id: String,
    /// Entry type.
    #[serde(rename = "type")]
    pub entry_type: String,
    /// Content text.
    pub content: String,
    /// Metadata key-value pairs.
    #[serde(default)]
    pub metadata: HashMap<String, String>,
    /// Tags.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Similarity score (set during queries).
    #[serde(default)]
    pub score: f64,
    /// Creation time.
    pub created_at: String,
    /// Update time.
    pub updated_at: String,
}

/// Result of a vector search query.
#[derive(Debug, Clone)]
pub struct QueryResult {
    /// Matching entries.
    pub entries: Vec<VectorEntry>,
    /// Total matches.
    pub total: usize,
    /// Original query.
    pub query: String,
}

/// An in-memory document with pre-computed embedding.
struct IndexedDoc {
    entry: VectorEntry,
    embedding: Vec<f32>,
}

/// The vector store provides semantic search over embedded entries.
pub struct VectorStore {
    docs: RwLock<Vec<IndexedDoc>>,
    embed: std::sync::Arc<dyn Fn(&str) -> Result<Vec<f32>, String> + Send + Sync>,
    config: StoreConfig,
    persist_path: PathBuf,
    /// 降级旗标：持久文件**整体**加载失败（不可读/非 UTF-8）时置位——
    /// 此后 rewrite_persist_file 拒绝全量覆写（空/部分索引覆写会把磁盘
    /// 历史整体覆没），delete/patch 的内存删除照常生效，append 型
    /// persist_entry 不受限（追加不读旧数据，无销毁面）。下次启动文件
    /// 可读即自然解除。
    load_failed: std::sync::atomic::AtomicBool,
    /// 加载时未能入索引的原始行留档（embed 失败 / JSON 解析失败）：
    /// rewrite 时先行写回——无关条目的改写不会把这些条目从磁盘抹掉。
    /// delete_entry 按 id 同步摘除，避免被删条目经留档行复活。
    unloaded_raw_lines: RwLock<Vec<String>>,
}

/// Compute cosine similarity between two vectors.
/// Round-5 dedup: delegates to the shared implementation in nemesis-types
/// (kept as a re-export point for existing callers).
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    nemesis_types::utils::cosine_similarity_f32(a, b)
}

impl VectorStore {
    /// Create a new vector store.
    ///
    /// Returns an error if the embedding function cannot be created
    /// (e.g., plugin DLL not found, model files missing).
    pub fn new(config: StoreConfig) -> Result<Self, String> {
        let persist_path = if config.storage_path.is_empty() {
            PathBuf::from("memory/vector/vector_store.jsonl")
        } else {
            PathBuf::from(&config.storage_path)
        };

        let vector_config = VectorConfig {
            embedding_tier: config.embedding_tier.clone(),
            plugin_path: config.plugin_path.clone(),
            config_dir: config.config_dir.clone(),
            host_services: None,
        };

        let embed: std::sync::Arc<dyn Fn(&str) -> Result<Vec<f32>, String> + Send + Sync> =
            std::sync::Arc::from(
                new_embedding_func(&vector_config)
                    .map_err(|e| format!("Failed to create embedding function: {}", e))?,
            );

        let store = Self {
            docs: RwLock::new(Vec::new()),
            embed,
            config,
            persist_path,
            load_failed: std::sync::atomic::AtomicBool::new(false),
            unloaded_raw_lines: RwLock::new(Vec::new()),
        };

        // Load persisted data
        // (In a real async context we'd await this, but since new() is sync
        // we skip auto-loading here and provide load_persisted() separately)

        Ok(store)
    }

    /// Create a new vector store with a pre-built embedding function.
    ///
    /// This is a test-only constructor that allows sharing a single ONNX
    /// plugin across multiple VectorStore instances.
    #[cfg(any(test, feature = "test-fixture"))]
    pub fn new_from_embed(embed: crate::vector::EmbeddingFunc, config: StoreConfig) -> Self {
        let embed: std::sync::Arc<dyn Fn(&str) -> Result<Vec<f32>, String> + Send + Sync> =
            std::sync::Arc::from(embed);
        let persist_path = if config.storage_path.is_empty() {
            PathBuf::from("memory/vector/vector_store.jsonl")
        } else {
            PathBuf::from(&config.storage_path)
        };
        let store = Self {
            docs: RwLock::new(Vec::new()),
            embed,
            config,
            persist_path,
            load_failed: std::sync::atomic::AtomicBool::new(false),
            unloaded_raw_lines: RwLock::new(Vec::new()),
        };
        let _ = store.load_persisted_sync();
        store
    }

    /// Store an entry in the vector store.
    pub fn store_entry(&self, entry: &VectorEntry) -> Result<(), String> {
        let embedding = (self.embed)(&entry.content)?;

        let doc = IndexedDoc {
            entry: entry.clone(),
            embedding,
        };

        self.docs.write().push(doc);
        Ok(())
    }

    /// Query the vector store for similar entries.
    /// Clone the embedding handle out: lets callers run inference
    /// WITHOUT holding the store's RwLock across the (slow) embed call.
    pub fn embed_handle(
        &self,
    ) -> std::sync::Arc<dyn Fn(&str) -> Result<Vec<f32>, String> + Send + Sync> {
        self.embed.clone()
    }

    /// Store config's similarity threshold（守卫纪律消费方需要显式阈值——
    /// embed 外提后走 query_with_embedding，config 不再对调用方隐藏）。
    pub fn similarity_threshold(&self) -> f64 {
        self.config.similarity_threshold
    }

    // Round-5: removed dead `embed_raw` (zero callers). A second public embed
    // path beside embed_handle invited bypassing the lock-discipline fix.
    // The Arc-returning embed_handle is the one sanctioned way out.

    pub fn query(
        &self,
        query: &str,
        limit: usize,
        type_filter: &[String],
    ) -> Result<QueryResult, String> {
        self.query_with_threshold(query, limit, type_filter, self.config.similarity_threshold)
    }

    /// Query with an explicit minimum-score gate, overriding the store
    /// config's `similarity_threshold` (0.7 — tuned for the agent's
    /// deliberate `memory_search`). Callers with a DIFFERENT recall contract
    /// must go through here: the auto-inject prefetch (loop.rs) uses 0.35 to
    /// catch loosely-related memories, and a store-level 0.7 pre-filter
    /// silently empties those results before the caller's own threshold is
    /// ever applied (注入为空的根因——两层阈值叠加，外层失效).
    pub fn query_with_threshold(
        &self,
        query: &str,
        limit: usize,
        type_filter: &[String],
        min_score: f64,
    ) -> Result<QueryResult, String> {
        let query_embedding = (self.embed)(query)?;
        self.query_with_embedding(&query_embedding, query, limit, type_filter, min_score)
    }

    /// 以预计算 embedding 查询（BUG-20261114-032 守卫纪律配套 API）：
    /// 调用方先 clone embed 句柄 / store Arc、丢上层守卫后再推理，把结果
    /// 喂给这里——锁不跨 tens-of-ms ONNX 调用。评分/排序/limit 语义与
    /// query_with_threshold 单一真相源（本函数即其 embed 拆分形态，
    /// query_with_threshold 只是 embed+委托的便捷壳）。
    pub fn query_with_embedding(
        &self,
        query_embedding: &[f32],
        query: &str,
        limit: usize,
        type_filter: &[String],
        min_score: f64,
    ) -> Result<QueryResult, String> {
        let limit = if limit == 0 {
            self.config.max_results
        } else {
            limit
        };

        let docs = self.docs.read();

        let mut scored: Vec<(f64, &IndexedDoc)> = docs
            .iter()
            .filter(|doc| {
                if type_filter.is_empty() {
                    true
                } else {
                    type_filter.contains(&doc.entry.entry_type)
                }
            })
            .filter_map(|doc| {
                let sim = cosine_similarity(query_embedding, &doc.embedding);
                if sim >= min_score {
                    Some((sim, doc))
                } else {
                    None
                }
            })
            .collect();

        // Sort by similarity descending
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        let total = scored.len();
        let entries = scored
            .into_iter()
            .take(limit)
            .map(|(score, doc)| {
                let mut entry = doc.entry.clone();
                entry.score = score;
                entry
            })
            .collect();

        Ok(QueryResult {
            entries,
            total,
            query: query.to_string(),
        })
    }

    /// Get an entry by ID.
    pub fn get_by_id(&self, id: &str) -> Option<VectorEntry> {
        self.docs
            .read()
            .iter()
            .find(|d| d.entry.id == id)
            .map(|d| d.entry.clone())
    }

    /// Delete an entry by ID. Returns true if found.
    /// Persists the deletion to disk by rewriting the JSONL file.
    /// 加载时留档的未入索条目（unloaded_raw_lines）同步按 id 摘除——
    /// 否则无关条目的 rewrite 把留档行写回，删除悄悄失效（条目复活）。
    pub fn delete_entry(&self, id: &str) -> bool {
        let mut docs = self.docs.write();
        let before = docs.len();
        docs.retain(|d| d.entry.id != id);
        let mut found = docs.len() < before;
        {
            let mut unloaded = self.unloaded_raw_lines.write();
            unloaded.retain(|raw| match serde_json::from_str::<VectorEntry>(raw) {
                Ok(e) => {
                    if e.id == id {
                        found = true;
                        false
                    } else {
                        true
                    }
                }
                // 解析失败行无 id 可比对，保留（宁多勿丢）。
                Err(_) => true,
            });
        }
        if found {
            self.rewrite_persist_file(&docs);
        }
        found
    }

    /// 字段级 patch（记忆 dreaming / 召回记账）：按 id 原位更新
    /// metadata/tags/type/updated_at，不触碰 content 与 embedding——调用方
    /// 契约是 content 不变（touch/promote/expire 都不改正文；merge 是新建
    /// 条目）。返回是否命中；命中时重写持久化文件（全量重写语义与
    /// delete_entry 一致，避免 append 日志出现同 id 双行）。
    /// 只命中已入索引条目；加载时留档的未入索条目（embed 失败）无
    /// embedding 可换算，patch 不触碰（原行留档写回语义）。
    pub fn patch_entry_fields(
        &self,
        id: &str,
        metadata: &HashMap<String, String>,
        tags: &[String],
        entry_type: Option<&str>,
        updated_at: Option<&str>,
    ) -> bool {
        let mut docs = self.docs.write();
        let mut found = false;
        for doc in docs.iter_mut() {
            if doc.entry.id != id {
                continue;
            }
            found = true;
            doc.entry.metadata = metadata.clone();
            doc.entry.tags = tags.to_vec();
            if let Some(t) = entry_type {
                doc.entry.entry_type = t.to_string();
            }
            if let Some(ts) = updated_at {
                doc.entry.updated_at = ts.to_string();
            }
        }
        if found {
            self.rewrite_persist_file(&docs);
        }
        found
    }

    /// Rewrite the entire JSONL persist file from the current docs.
    fn rewrite_persist_file(&self, docs: &[IndexedDoc]) {
        // 降级冻结：启动时整体加载失败的 store 禁止全量覆写——此刻内存
        // 索引不含磁盘上的任何历史条目，覆写 = 数据销毁。内存删除已生效
        // （下次启动重放），磁盘保持原样，append 型 persist 不受限。
        if self.load_failed.load(std::sync::atomic::Ordering::Relaxed) {
            tracing::warn!(
                path = %self.persist_path.display(),
                "[VectorStore] rewrite skipped: persist file failed to load at startup \
                 (full overwrite would destroy unloaded history)"
            );
            return;
        }

        // 唯一临时名权威 helper（旧固定 `*.jsonl.tmp` 双写者
        // 同名互踩 + 失败残留 tmp）。不可序列化条目跳过（与旧姿态一致）；
        // 写失败 warn（旧实现全静默——静默丢持久化连排查线索都没有）。
        // 留档行（加载时 embed/解析失败未入索引）先行写回：无关条目的
        // 改写不抹掉未加载条目。
        let mut body = String::new();
        for raw in self.unloaded_raw_lines.read().iter() {
            body.push_str(raw);
            body.push('\n');
        }
        for doc in docs {
            if let Ok(line) = serde_json::to_string(&doc.entry) {
                body.push_str(&line);
                body.push('\n');
            }
        }
        if let Err(e) = nemesis_utils::file::write_file_atomic(
            &self.persist_path.to_string_lossy(),
            body.as_bytes(),
            0o644,
        ) {
            tracing::warn!(
                error = %e,
                path = %self.persist_path.display(),
                "[VectorStore] persist rewrite failed"
            );
        }
    }

    /// List entries with optional type filter and pagination.
    pub fn list_entries(&self, type_filter: &[String], offset: usize, limit: usize) -> QueryResult {
        let docs = self.docs.read();
        let filtered: Vec<&IndexedDoc> = docs
            .iter()
            .filter(|doc| {
                if type_filter.is_empty() {
                    true
                } else {
                    type_filter.contains(&doc.entry.entry_type)
                }
            })
            .collect();

        let total = filtered.len();
        let entries = filtered
            .into_iter()
            .skip(offset)
            .take(if limit > 0 { limit } else { usize::MAX })
            .map(|doc| doc.entry.clone())
            .collect();

        QueryResult {
            entries,
            total,
            query: String::new(),
        }
    }

    /// Return the number of stored entries.
    pub fn len(&self) -> usize {
        self.docs.read().len()
    }

    /// Return whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.docs.read().is_empty()
    }

    /// Load persisted entries from JSONL (async version).
    pub async fn load_persisted(&self) -> Result<(), String> {
        self.load_persisted_sync()
    }

    /// Load persisted entries from JSONL (sync version).
    ///
    /// 加载语义（数据保全优先）：
    /// - 文件整体不可读（IO 错误 / 非 UTF-8）→ 置位 `load_failed` 降级
    ///   旗标并返回 Err——后续 rewrite_persist_file 冻结全量覆写，防
    ///   「空索引 + 一次无关改写」把磁盘历史整体覆没（数据销毁面）；
    /// - 单行失败（embed 失败 / JSON 解析失败）→ 原始行留档进
    ///   `unloaded_raw_lines`（不静默消失），rewrite 时写回磁盘。
    pub fn load_persisted_sync(&self) -> Result<(), String> {
        if !self.persist_path.exists() {
            return Ok(());
        }

        let content = std::fs::read_to_string(&self.persist_path).map_err(|e| {
            // 整体加载失败：挂降级旗标（rewrite 冻结），保磁盘原样。
            self.load_failed
                .store(true, std::sync::atomic::Ordering::Relaxed);
            e.to_string()
        })?;

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match serde_json::from_str::<VectorEntry>(line) {
                Ok(entry) => {
                    if let Err(e) = self.store_entry(&entry) {
                        tracing::warn!(
                            error = %e,
                            id = %entry.id,
                            path = %self.persist_path.display(),
                            "[VectorStore] entry skipped at load (kept on disk): embed failed"
                        );
                        self.unloaded_raw_lines.write().push(line.to_string());
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        path = %self.persist_path.display(),
                        "[VectorStore] unparsable line kept on disk at load"
                    );
                    self.unloaded_raw_lines.write().push(line.to_string());
                }
            }
        }

        Ok(())
    }

    /// Persist an entry to JSONL (async version).
    pub async fn persist_entry(&self, entry: &VectorEntry) -> Result<(), String> {
        Self::persist_entry_sync_inner(&self.persist_path, entry)
    }

    /// Persist an entry to JSONL (sync version for use from sync contexts).
    pub fn persist_entry_sync(&self, entry: &VectorEntry) -> Result<(), String> {
        Self::persist_entry_sync_inner(&self.persist_path, entry)
    }

    /// Shared implementation for persist.
    fn persist_entry_sync_inner(
        persist_path: &std::path::Path,
        entry: &VectorEntry,
    ) -> Result<(), String> {
        if let Some(parent) = persist_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(persist_path)
            .map_err(|e| e.to_string())?;

        let mut line = serde_json::to_string(entry).map_err(|e| e.to_string())?;
        line.push('\n');
        use std::io::Write;
        file.write_all(line.as_bytes()).map_err(|e| e.to_string())?;

        Ok(())
    }
}

#[cfg(test)]
mod extra_tests;

#[cfg(test)]
mod w2c_tests;

#[cfg(test)]
mod tests;
