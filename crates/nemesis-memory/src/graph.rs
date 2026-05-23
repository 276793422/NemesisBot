//! Knowledge graph store with entity-relation triples and BFS query.
//!
//! The graph stores triples of the form `(subject, predicate, object)` with
//! optional metadata and confidence scores. Queries use breadth-first search
//! starting from a given entity to traverse the graph.
//!
//! When configured with a persistence directory via [`InMemoryGraphStore::with_persistence`],
//! entities and triples are saved as JSONL files (`entities.jsonl` and `triples.jsonl`)
//! and reloaded on startup, mirroring the Go `graph.Store` implementation.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// A knowledge-graph triple: subject --predicate--> object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphTriple {
    /// Subject entity name.
    pub subject: String,
    /// Relationship / predicate label.
    pub predicate: String,
    /// Object entity name.
    pub object: String,
    /// Arbitrary metadata attached to the relationship.
    #[serde(default)]
    pub metadata: HashMap<String, String>,
    /// Confidence score in [0, 1].
    #[serde(default)]
    pub confidence: f64,
    /// When this triple was created.
    pub created_at: DateTime<Utc>,
}

impl GraphTriple {
    /// Create a new triple with current timestamp.
    pub fn new(subject: String, predicate: String, object: String) -> Self {
        Self {
            subject,
            predicate,
            object,
            metadata: HashMap::new(),
            confidence: 1.0,
            created_at: Utc::now(),
        }
    }

    /// Builder-style method to set confidence score.
    pub fn with_confidence(mut self, confidence: f64) -> Self {
        self.confidence = confidence;
        self
    }
}

/// An entity node in the knowledge graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEntity {
    /// Entity name (unique key within the graph).
    pub name: String,
    /// Entity type label, e.g. "person", "concept", "tool".
    #[serde(rename = "type")]
    pub typ: String,
    /// Arbitrary properties.
    #[serde(default)]
    pub properties: HashMap<String, String>,
    /// When this entity was created.
    pub created_at: DateTime<Utc>,
}

impl GraphEntity {
    /// Create a new entity with current timestamp.
    pub fn new(name: String, typ: String) -> Self {
        Self {
            name,
            typ,
            properties: HashMap::new(),
            created_at: Utc::now(),
        }
    }
}

/// A single hop in a graph traversal path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathHop {
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub confidence: f64,
}

/// Result of a BFS graph query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphQueryResult {
    /// Discovered paths (each path is a sequence of hops).
    pub paths: Vec<Vec<PathHop>>,
    /// Entities discovered during traversal.
    pub entities: Vec<GraphEntity>,
}

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Async interface for a knowledge-graph backend.
#[async_trait]
pub trait GraphStore: Send + Sync {
    /// Add a triple to the graph.
    async fn add_triple(&self, triple: GraphTriple) -> Result<(), String>;

    /// Add or update an entity.
    async fn upsert_entity(&self, entity: GraphEntity) -> Result<(), String>;

    /// Remove a specific triple.
    async fn remove_triple(&self, subject: &str, predicate: &str, object: &str) -> Result<bool, String>;

    /// Look up an entity by name.
    async fn get_entity(&self, name: &str) -> Result<Option<GraphEntity>, String>;

    /// Query the graph starting from `start_entity`, traversing up to
    /// `max_depth` hops. Returns all discovered paths.
    async fn query_bfs(
        &self,
        start_entity: &str,
        max_depth: usize,
    ) -> Result<GraphQueryResult, String>;

    /// List all triples involving a given entity (as either subject or object).
    async fn list_triples(&self, entity: &str) -> Result<Vec<GraphTriple>, String>;

    /// Search triples by text query across subject, predicate, and object fields.
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<GraphTriple>, String>;

    /// Delete an entity and all triples that reference it.
    async fn delete_entity(&self, name: &str) -> Result<(), String>;

    /// Query triples matching the given subject, predicate, and/or object.
    /// Empty strings act as wildcards.
    async fn query_triples(
        &self,
        subject: &str,
        predicate: &str,
        object: &str,
    ) -> Result<Vec<GraphTriple>, String>;

    /// Get all related triples within depth hops using BFS.
    async fn get_related(
        &self,
        entity_name: &str,
        depth: usize,
    ) -> Result<Vec<GraphTriple>, String>;

    /// Return the number of entities stored.
    async fn entity_count(&self) -> Result<usize, String>;

    /// Return the number of triples stored.
    async fn triple_count(&self) -> Result<usize, String>;
}

// ---------------------------------------------------------------------------
// InMemoryGraphStore
// ---------------------------------------------------------------------------

/// Thread-safe in-memory graph store backed by `DashMap`.
///
/// Optionally persists data to disk as JSONL files when a persistence
/// directory is configured via [`InMemoryGraphStore::with_persistence`].
/// On construction, existing data is loaded from `entities.jsonl` and
/// `triples.jsonl`. Every mutation (upsert, add, remove, delete) triggers
/// an atomic rewrite of the corresponding file.
pub struct InMemoryGraphStore {
    /// Entity name -> GraphEntity.
    entities: DashMap<String, GraphEntity>,
    /// All triples stored as a flat list (indexed by subject for fast lookup).
    triples_by_subject: DashMap<String, Vec<GraphTriple>>,
    /// Secondary index: object -> triples where this is the object.
    triples_by_object: DashMap<String, Vec<GraphTriple>>,
    /// Optional directory for JSONL persistence.
    persistence_dir: Option<PathBuf>,
    /// Ensures data is loaded from disk at most once.
    load_once: OnceLock<()>,
}

impl InMemoryGraphStore {
    /// Create a new in-memory graph store without disk persistence.
    pub fn new() -> Self {
        Self {
            entities: DashMap::new(),
            triples_by_subject: DashMap::new(),
            triples_by_object: DashMap::new(),
            persistence_dir: None,
            load_once: OnceLock::new(),
        }
    }

    /// Builder method to enable disk persistence.
    ///
    /// When set, the store will load existing data from `dir/entities.jsonl`
    /// and `dir/triples.jsonl` on first access, and rewrite those files
    /// after every mutation. The directory is created lazily on first write.
    pub fn with_persistence(mut self, dir: PathBuf) -> Self {
        self.persistence_dir = Some(dir);
        self
    }

    // -- Persistence helpers ------------------------------------------------

    /// Ensure data has been loaded from disk (runs at most once).
    fn ensure_loaded(&self) {
        if let Some(ref dir) = self.persistence_dir {
            self.load_once.get_or_init(|| {
                // Errors are logged and swallowed -- the store starts empty.
                if let Err(e) = self.load_from_disk(dir) {
                    tracing::warn!("[GraphStore] failed to load from disk: {e}");
                }
            });
        }
    }

    /// Read entities and triples from JSONL files in `dir`.
    fn load_from_disk(&self, dir: &Path) -> Result<(), String> {
        // Load entities
        let entities_path = dir.join("entities.jsonl");
        if entities_path.exists() {
            if let Ok(data) = std::fs::read_to_string(&entities_path) {
                for line in data.lines() {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if let Ok(entity) = serde_json::from_str::<GraphEntity>(trimmed) {
                        let key = entity.name.to_lowercase();
                        self.entities.insert(key, entity);
                    }
                }
            }
        }

        // Load triples
        let triples_path = dir.join("triples.jsonl");
        if triples_path.exists() {
            if let Ok(data) = std::fs::read_to_string(&triples_path) {
                for line in data.lines() {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if let Ok(triple) = serde_json::from_str::<GraphTriple>(trimmed) {
                        let subject = triple.subject.clone();
                        let object = triple.object.clone();

                        self.triples_by_subject
                            .entry(subject)
                            .or_default()
                            .push(triple.clone());

                        self.triples_by_object
                            .entry(object)
                            .or_default()
                            .push(triple);
                    }
                }
            }
        }

        Ok(())
    }

    /// Write all entities to `entities.jsonl` using atomic write.
    ///
    /// Atomic write: data is first written to a `.tmp` file, then renamed
    /// to the final path. This prevents partial writes on crash.
    fn persist_entities(&self) -> Result<(), String> {
        let dir = self.persistence_dir.as_ref().ok_or("no persistence dir")?;
        std::fs::create_dir_all(dir).map_err(|e| format!("graph: create dir: {e}"))?;

        let final_path = dir.join("entities.jsonl");
        let tmp_path = dir.join("entities.jsonl.tmp");

        let mut file =
            std::fs::File::create(&tmp_path).map_err(|e| format!("graph: create tmp: {e}"))?;

        for entry in self.entities.iter() {
            let json = serde_json::to_string(entry.value())
                .map_err(|e| format!("graph: serialize entity: {e}"))?;
            writeln!(file, "{json}").map_err(|e| format!("graph: write entity: {e}"))?;
        }

        file.flush()
            .map_err(|e| format!("graph: flush entities: {e}"))?;
        drop(file);

        std::fs::rename(&tmp_path, &final_path)
            .map_err(|e| format!("graph: rename entities: {e}"))?;

        Ok(())
    }

    /// Write all triples to `triples.jsonl` using atomic write.
    fn persist_triples(&self) -> Result<(), String> {
        let dir = self.persistence_dir.as_ref().ok_or("no persistence dir")?;
        std::fs::create_dir_all(dir).map_err(|e| format!("graph: create dir: {e}"))?;

        let final_path = dir.join("triples.jsonl");
        let tmp_path = dir.join("triples.jsonl.tmp");

        let mut file =
            std::fs::File::create(&tmp_path).map_err(|e| format!("graph: create tmp: {e}"))?;

        // Deduplicate triples across both indices.
        let mut seen: HashSet<(String, String, String)> = HashSet::new();
        for entry in self.triples_by_subject.iter() {
            for t in entry.value().iter() {
                let key = (t.subject.clone(), t.predicate.clone(), t.object.clone());
                if seen.insert(key) {
                    let json = serde_json::to_string(t)
                        .map_err(|e| format!("graph: serialize triple: {e}"))?;
                    writeln!(file, "{json}").map_err(|e| format!("graph: write triple: {e}"))?;
                }
            }
        }

        file.flush()
            .map_err(|e| format!("graph: flush triples: {e}"))?;
        drop(file);

        std::fs::rename(&tmp_path, &final_path)
            .map_err(|e| format!("graph: rename triples: {e}"))?;

        Ok(())
    }

    /// Persist both entities and triples.
    fn persist_all(&self) -> Result<(), String> {
        self.persist_entities()?;
        self.persist_triples()
    }
}

impl Default for InMemoryGraphStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl GraphStore for InMemoryGraphStore {
    async fn add_triple(&self, triple: GraphTriple) -> Result<(), String> {
        self.ensure_loaded();

        let subject = triple.subject.clone();
        let object = triple.object.clone();

        self.triples_by_subject
            .entry(subject.clone())
            .or_default()
            .push(triple.clone());

        self.triples_by_object
            .entry(object)
            .or_default()
            .push(triple);

        if self.persistence_dir.is_some() {
            if let Err(e) = self.persist_triples() {
                tracing::warn!("[GraphStore] persist_triples failed: {e}");
            }
        }

        Ok(())
    }

    async fn upsert_entity(&self, entity: GraphEntity) -> Result<(), String> {
        self.ensure_loaded();

        let name = entity.name.clone();
        self.entities.insert(name, entity);

        if self.persistence_dir.is_some() {
            if let Err(e) = self.persist_entities() {
                tracing::warn!("[GraphStore] persist_entities failed: {e}");
            }
        }

        Ok(())
    }

    async fn remove_triple(
        &self,
        subject: &str,
        predicate: &str,
        object: &str,
    ) -> Result<bool, String> {
        self.ensure_loaded();

        let mut removed = false;

        if let Some(mut triples) = self.triples_by_subject.get_mut(subject) {
            let before = triples.len();
            triples.retain(|t| !(t.predicate == predicate && t.object == object));
            removed = triples.len() < before;
        }

        if let Some(mut triples) = self.triples_by_object.get_mut(object) {
            triples.retain(|t| !(t.subject == subject && t.predicate == predicate));
        }

        if removed && self.persistence_dir.is_some() {
            if let Err(e) = self.persist_triples() {
                tracing::warn!("[GraphStore] persist_triples failed: {e}");
            }
        }

        Ok(removed)
    }

    async fn get_entity(&self, name: &str) -> Result<Option<GraphEntity>, String> {
        self.ensure_loaded();
        Ok(self.entities.get(name).map(|r| r.value().clone()))
    }

    async fn query_bfs(
        &self,
        start_entity: &str,
        max_depth: usize,
    ) -> Result<GraphQueryResult, String> {
        self.ensure_loaded();

        let mut paths: Vec<Vec<PathHop>> = Vec::new();
        let mut visited: HashSet<String> = HashSet::new();
        let mut discovered_entities: Vec<GraphEntity> = Vec::new();

        // BFS queue: (current_entity, path_so_far)
        let mut queue: VecDeque<(String, Vec<PathHop>)> = VecDeque::new();
        queue.push_back((start_entity.to_string(), Vec::new()));
        visited.insert(start_entity.to_string());

        // Include the start entity if it exists.
        if let Some(e) = self.entities.get(start_entity) {
            discovered_entities.push(e.value().clone());
        }

        while let Some((current, path)) = queue.pop_front() {
            if path.len() >= max_depth {
                continue;
            }

            // Find all outgoing triples from `current`.
            if let Some(triples) = self.triples_by_subject.get(&current) {
                for triple in triples.iter() {
                    let mut new_path = path.clone();
                    new_path.push(PathHop {
                        subject: triple.subject.clone(),
                        predicate: triple.predicate.clone(),
                        object: triple.object.clone(),
                        confidence: triple.confidence,
                    });
                    paths.push(new_path.clone());

                    if !visited.contains(&triple.object) {
                        visited.insert(triple.object.clone());
                        if let Some(e) = self.entities.get(&triple.object) {
                            discovered_entities.push(e.value().clone());
                        }
                        queue.push_back((triple.object.clone(), new_path));
                    }
                }
            }
        }

        Ok(GraphQueryResult {
            paths,
            entities: discovered_entities,
        })
    }

    async fn list_triples(&self, entity: &str) -> Result<Vec<GraphTriple>, String> {
        self.ensure_loaded();

        let mut result = Vec::new();
        let mut seen: HashSet<(String, String, String)> = HashSet::new();

        // Triples where entity is the subject.
        if let Some(triples) = self.triples_by_subject.get(entity) {
            for t in triples.iter() {
                let key = (t.subject.clone(), t.predicate.clone(), t.object.clone());
                if seen.insert(key) {
                    result.push(t.clone());
                }
            }
        }

        // Triples where entity is the object.
        if let Some(triples) = self.triples_by_object.get(entity) {
            for t in triples.iter() {
                let key = (t.subject.clone(), t.predicate.clone(), t.object.clone());
                if seen.insert(key) {
                    result.push(t.clone());
                }
            }
        }

        Ok(result)
    }

    async fn search(&self, query: &str, limit: usize) -> Result<Vec<GraphTriple>, String> {
        self.ensure_loaded();

        let limit = if limit == 0 { 20 } else { limit };
        let query_lower = query.to_lowercase();
        let mut results: Vec<GraphTriple> = Vec::new();

        // Collect all triples from both indices
        let mut seen: HashSet<(String, String, String)> = HashSet::new();

        for entry in self.triples_by_subject.iter() {
            for t in entry.value().iter() {
                if seen.insert((t.subject.clone(), t.predicate.clone(), t.object.clone())) {
                    if t.subject.to_lowercase().contains(&query_lower)
                        || t.predicate.to_lowercase().contains(&query_lower)
                        || t.object.to_lowercase().contains(&query_lower)
                    {
                        results.push(t.clone());
                        if results.len() >= limit {
                            break;
                        }
                    }
                }
            }
            if results.len() >= limit {
                break;
            }
        }

        Ok(results)
    }

    async fn delete_entity(&self, name: &str) -> Result<(), String> {
        self.ensure_loaded();

        let name_lower = name.to_lowercase();

        // Remove entity
        self.entities.remove(&name_lower);

        // Remove all triples where this entity is subject or object.
        // Collect the triples to remove first.
        let triples_to_remove: Vec<(String, String, String)> = {
            let mut to_remove = Vec::new();
            if let Some(triples) = self.triples_by_subject.get(&name_lower) {
                for t in triples.iter() {
                    to_remove.push((t.subject.clone(), t.predicate.clone(), t.object.clone()));
                }
            }
            if let Some(triples) = self.triples_by_object.get(&name_lower) {
                for t in triples.iter() {
                    to_remove.push((t.subject.clone(), t.predicate.clone(), t.object.clone()));
                }
            }
            to_remove
        };

        for (subject, predicate, object) in triples_to_remove {
            // Remove from subject index
            if let Some(mut triples) = self.triples_by_subject.get_mut(&subject) {
                triples.retain(|t| !(t.predicate == predicate && t.object == object));
            }
            // Remove from object index
            if let Some(mut triples) = self.triples_by_object.get_mut(&object) {
                triples.retain(|t| !(t.subject == subject && t.predicate == predicate));
            }
        }

        if self.persistence_dir.is_some() {
            if let Err(e) = self.persist_all() {
                tracing::warn!("[GraphStore] persist_all failed: {e}");
            }
        }

        Ok(())
    }

    async fn query_triples(
        &self,
        subject: &str,
        predicate: &str,
        object: &str,
    ) -> Result<Vec<GraphTriple>, String> {
        self.ensure_loaded();

        let mut results = Vec::new();
        let mut seen: HashSet<(String, String, String)> = HashSet::new();

        // Iterate all triples from the subject index
        for entry in self.triples_by_subject.iter() {
            for t in entry.value().iter() {
                if seen.insert((t.subject.clone(), t.predicate.clone(), t.object.clone())) {
                    if !subject.is_empty()
                        && t.subject.to_lowercase() != subject.to_lowercase()
                    {
                        continue;
                    }
                    if !predicate.is_empty()
                        && t.predicate.to_lowercase() != predicate.to_lowercase()
                    {
                        continue;
                    }
                    if !object.is_empty()
                        && t.object.to_lowercase() != object.to_lowercase()
                    {
                        continue;
                    }
                    results.push(t.clone());
                }
            }
        }

        Ok(results)
    }

    async fn get_related(
        &self,
        entity_name: &str,
        depth: usize,
    ) -> Result<Vec<GraphTriple>, String> {
        self.ensure_loaded();

        let depth = if depth == 0 { 1 } else { depth };
        let entity_lower = entity_name.to_lowercase();

        let mut visited: HashSet<String> = HashSet::new();
        let mut results: Vec<GraphTriple> = Vec::new();
        let mut queue: VecDeque<(String, usize)> = VecDeque::new();

        visited.insert(entity_lower.clone());
        queue.push_back((entity_lower, 0));

        while let Some((current, current_depth)) = queue.pop_front() {
            if current_depth >= depth {
                continue;
            }

            // Find triples where current is the subject
            if let Some(triples) = self.triples_by_subject.get(&current) {
                for t in triples.iter() {
                    let neighbor = t.object.to_lowercase();
                    results.push(t.clone());
                    if !visited.contains(&neighbor) {
                        visited.insert(neighbor.clone());
                        queue.push_back((neighbor, current_depth + 1));
                    }
                }
            }

            // Find triples where current is the object
            if let Some(triples) = self.triples_by_object.get(&current) {
                for t in triples.iter() {
                    let neighbor = t.subject.to_lowercase();
                    results.push(t.clone());
                    if !visited.contains(&neighbor) {
                        visited.insert(neighbor.clone());
                        queue.push_back((neighbor, current_depth + 1));
                    }
                }
            }
        }

        // Deduplicate results
        let mut seen: HashSet<(String, String, String)> = HashSet::new();
        results.retain(|t| seen.insert((t.subject.clone(), t.predicate.clone(), t.object.clone())));

        Ok(results)
    }

    async fn entity_count(&self) -> Result<usize, String> {
        self.ensure_loaded();
        Ok(self.entities.len())
    }

    async fn triple_count(&self) -> Result<usize, String> {
        self.ensure_loaded();

        let mut count = 0;
        let mut seen: HashSet<(String, String, String)> = HashSet::new();
        for entry in self.triples_by_subject.iter() {
            for t in entry.value().iter() {
                if seen.insert((t.subject.clone(), t.predicate.clone(), t.object.clone())) {
                    count += 1;
                }
            }
        }
        Ok(count)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
