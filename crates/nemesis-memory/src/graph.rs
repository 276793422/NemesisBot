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
                    tracing::warn!("graph store: failed to load from disk: {e}");
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
                tracing::warn!("graph store: persist_triples failed: {e}");
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
                tracing::warn!("graph store: persist_entities failed: {e}");
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
                tracing::warn!("graph store: persist_triples failed: {e}");
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
                tracing::warn!("graph store: persist_all failed: {e}");
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
mod tests {
    use super::*;

    #[tokio::test]
    async fn add_and_query_triples() {
        let store = InMemoryGraphStore::new();

        store
            .add_triple(GraphTriple::new(
                "rust".into(),
                "is_a".into(),
                "language".into(),
            ))
            .await
            .unwrap();
        store
            .add_triple(GraphTriple::new(
                "language".into(),
                "has_feature".into(),
                "memory_safety".into(),
            ))
            .await
            .unwrap();

        let result = store.query_bfs("rust", 3).await.unwrap();
        // Two paths: rust->language, and rust->language->memory_safety
        assert_eq!(result.paths.len(), 2);

        let direct: Vec<_> = result
            .paths
            .iter()
            .filter(|p| p.len() == 1)
            .collect();
        assert_eq!(direct.len(), 1);
        assert_eq!(direct[0][0].object, "language");
    }

    #[tokio::test]
    async fn entity_upsert_and_get() {
        let store = InMemoryGraphStore::new();

        store
            .upsert_entity(GraphEntity::new("rust".into(), "language".into()))
            .await
            .unwrap();

        let entity = store.get_entity("rust").await.unwrap().unwrap();
        assert_eq!(entity.name, "rust");
        assert_eq!(entity.typ, "language");

        // Upsert overwrites.
        store
            .upsert_entity(GraphEntity::new("rust".into(), "tool".into()))
            .await
            .unwrap();
        let updated = store.get_entity("rust").await.unwrap().unwrap();
        assert_eq!(updated.typ, "tool");
    }

    #[tokio::test]
    async fn remove_triple() {
        let store = InMemoryGraphStore::new();
        store
            .add_triple(GraphTriple::new("a".into(), "b".into(), "c".into()))
            .await
            .unwrap();

        let removed = store.remove_triple("a", "b", "c").await.unwrap();
        assert!(removed);

        let result = store.query_bfs("a", 1).await.unwrap();
        assert!(result.paths.is_empty());

        let removed_again = store.remove_triple("a", "b", "c").await.unwrap();
        assert!(!removed_again);
    }

    #[tokio::test]
    async fn list_triples_for_entity() {
        let store = InMemoryGraphStore::new();
        store
            .add_triple(GraphTriple::new("x".into(), "rel1".into(), "y".into()))
            .await
            .unwrap();
        store
            .add_triple(GraphTriple::new("z".into(), "rel2".into(), "x".into()))
            .await
            .unwrap();

        let triples = store.list_triples("x").await.unwrap();
        assert_eq!(triples.len(), 2);
    }

    // -- Persistence tests --------------------------------------------------

    #[tokio::test]
    async fn persist_entities_to_disk() {
        let dir = tempfile::tempdir().unwrap();
        let store = InMemoryGraphStore::new()
            .with_persistence(dir.path().to_path_buf());

        store
            .upsert_entity(GraphEntity::new("alice".into(), "person".into()))
            .await
            .unwrap();
        store
            .upsert_entity(GraphEntity::new("bob".into(), "person".into()))
            .await
            .unwrap();

        let entities_path = dir.path().join("entities.jsonl");
        assert!(entities_path.exists());

        let data = std::fs::read_to_string(&entities_path).unwrap();
        let count = data.lines().filter(|l| !l.trim().is_empty()).count();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn persist_triples_to_disk() {
        let dir = tempfile::tempdir().unwrap();
        let store = InMemoryGraphStore::new()
            .with_persistence(dir.path().to_path_buf());

        store
            .add_triple(GraphTriple::new("a".into(), "knows".into(), "b".into()))
            .await
            .unwrap();
        store
            .add_triple(GraphTriple::new("b".into(), "works_with".into(), "c".into()))
            .await
            .unwrap();

        let triples_path = dir.path().join("triples.jsonl");
        assert!(triples_path.exists());

        let data = std::fs::read_to_string(&triples_path).unwrap();
        let count = data.lines().filter(|l| !l.trim().is_empty()).count();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn reload_entities_from_disk() {
        let dir = tempfile::tempdir().unwrap();

        // Write data with first store instance.
        {
            let store = InMemoryGraphStore::new()
                .with_persistence(dir.path().to_path_buf());
            store
                .upsert_entity(GraphEntity::new("rust".into(), "language".into()))
                .await
                .unwrap();
            store
                .upsert_entity(GraphEntity::new("go".into(), "language".into()))
                .await
                .unwrap();
        }

        // Create a new store -- should reload from disk.
        let store2 = InMemoryGraphStore::new()
            .with_persistence(dir.path().to_path_buf());
        let entity = store2.get_entity("rust").await.unwrap().unwrap();
        assert_eq!(entity.name, "rust");
        assert_eq!(entity.typ, "language");

        let go = store2.get_entity("go").await.unwrap().unwrap();
        assert_eq!(go.name, "go");

        let count = store2.entity_count().await.unwrap();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn reload_triples_from_disk() {
        let dir = tempfile::tempdir().unwrap();

        // Write data with first store instance.
        {
            let store = InMemoryGraphStore::new()
                .with_persistence(dir.path().to_path_buf());
            store
                .add_triple(GraphTriple::new("x".into(), "rel".into(), "y".into()))
                .await
                .unwrap();
            store
                .add_triple(GraphTriple::new("y".into(), "rel".into(), "z".into()))
                .await
                .unwrap();
        }

        // Create a new store -- should reload from disk.
        let store2 = InMemoryGraphStore::new()
            .with_persistence(dir.path().to_path_buf());
        let triples = store2.list_triples("x").await.unwrap();
        assert_eq!(triples.len(), 1);
        assert_eq!(triples[0].object, "y");

        let count = store2.triple_count().await.unwrap();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn persist_after_delete_entity() {
        let dir = tempfile::tempdir().unwrap();
        let store = InMemoryGraphStore::new()
            .with_persistence(dir.path().to_path_buf());

        store
            .upsert_entity(GraphEntity::new("target".into(), "thing".into()))
            .await
            .unwrap();
        store
            .add_triple(GraphTriple::new("a".into(), "refers".into(), "target".into()))
            .await
            .unwrap();

        store.delete_entity("target").await.unwrap();

        // Verify files on disk reflect the deletion.
        let entities_data =
            std::fs::read_to_string(dir.path().join("entities.jsonl")).unwrap();
        assert!(
            !entities_data.contains("target"),
            "entity should be gone from persisted file"
        );

        let triples_data =
            std::fs::read_to_string(dir.path().join("triples.jsonl")).unwrap();
        assert!(
            triples_data.trim().is_empty(),
            "triples referencing deleted entity should be gone"
        );
    }

    #[tokio::test]
    async fn persist_after_remove_triple() {
        let dir = tempfile::tempdir().unwrap();
        let store = InMemoryGraphStore::new()
            .with_persistence(dir.path().to_path_buf());

        store
            .add_triple(GraphTriple::new("a".into(), "b".into(), "c".into()))
            .await
            .unwrap();

        let triples_data_before =
            std::fs::read_to_string(dir.path().join("triples.jsonl")).unwrap();
        assert!(triples_data_before.contains("b"));

        store.remove_triple("a", "b", "c").await.unwrap();

        let triples_data_after =
            std::fs::read_to_string(dir.path().join("triples.jsonl")).unwrap();
        assert!(
            triples_data_after.trim().is_empty(),
            "removed triple should be gone from persisted file"
        );
    }

    #[tokio::test]
    async fn no_persistence_without_dir() {
        let store = InMemoryGraphStore::new();
        store
            .upsert_entity(GraphEntity::new("x".into(), "thing".into()))
            .await
            .unwrap();
        store
            .add_triple(GraphTriple::new("a".into(), "b".into(), "c".into()))
            .await
            .unwrap();

        // No files were written -- nothing to assert, just ensure no panic.
        assert_eq!(store.entity_count().await.unwrap(), 1);
        assert_eq!(store.triple_count().await.unwrap(), 1);
    }

    // ============================================================
    // Additional tests for missing coverage
    // ============================================================

    #[tokio::test]
    async fn graph_entity_properties() {
        let store = InMemoryGraphStore::new();
        let mut entity = GraphEntity::new("rust".into(), "language".into());
        entity.properties.insert("paradigm".into(), "multi-paradigm".into());
        entity.properties.insert("year".into(), "2010".into());
        store.upsert_entity(entity).await.unwrap();

        let retrieved = store.get_entity("rust").await.unwrap().unwrap();
        assert_eq!(retrieved.properties.get("paradigm").unwrap(), "multi-paradigm");
        assert_eq!(retrieved.properties.get("year").unwrap(), "2010");
    }

    #[tokio::test]
    async fn graph_triple_confidence() {
        let store = InMemoryGraphStore::new();
        let triple = GraphTriple::new("a".into(), "rel".into(), "b".into())
            .with_confidence(0.75);
        store.add_triple(triple).await.unwrap();

        let triples = store.list_triples("a").await.unwrap();
        assert_eq!(triples.len(), 1);
        assert!((triples[0].confidence - 0.75).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn graph_default_confidence_is_one() {
        let triple = GraphTriple::new("x".into(), "y".into(), "z".into());
        assert!((triple.confidence - 1.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn graph_triple_metadata() {
        let store = InMemoryGraphStore::new();
        let mut triple = GraphTriple::new("alice".into(), "knows".into(), "bob".into());
        triple.metadata.insert("since".into(), "2020".into());
        store.add_triple(triple).await.unwrap();

        let triples = store.list_triples("alice").await.unwrap();
        assert_eq!(triples[0].metadata.get("since").unwrap(), "2020");
    }

    #[tokio::test]
    async fn graph_bfs_empty_graph() {
        let store = InMemoryGraphStore::new();
        let result = store.query_bfs("nonexistent", 3).await.unwrap();
        assert!(result.paths.is_empty());
        assert!(result.entities.is_empty());
    }

    #[tokio::test]
    async fn graph_bfs_depth_zero() {
        let store = InMemoryGraphStore::new();
        store.add_triple(GraphTriple::new("a".into(), "rel".into(), "b".into())).await.unwrap();

        let result = store.query_bfs("a", 0).await.unwrap();
        // Depth 0 means path.len() >= max_depth immediately, so no traversal
        assert!(result.paths.is_empty());
    }

    #[tokio::test]
    async fn graph_bfs_multi_hop() {
        let store = InMemoryGraphStore::new();
        store.add_triple(GraphTriple::new("a".into(), "rel".into(), "b".into())).await.unwrap();
        store.add_triple(GraphTriple::new("b".into(), "rel".into(), "c".into())).await.unwrap();
        store.add_triple(GraphTriple::new("c".into(), "rel".into(), "d".into())).await.unwrap();

        let result = store.query_bfs("a", 3).await.unwrap();
        // Should find paths: a->b, a->b->c, a->b->c->d
        assert!(result.paths.len() >= 3);
    }

    #[tokio::test]
    async fn graph_bfs_cyclic_graph() {
        let store = InMemoryGraphStore::new();
        store.add_triple(GraphTriple::new("a".into(), "rel".into(), "b".into())).await.unwrap();
        store.add_triple(GraphTriple::new("b".into(), "rel".into(), "a".into())).await.unwrap();

        let result = store.query_bfs("a", 5).await.unwrap();
        // Should not infinite loop; visited set prevents revisiting
        assert!(!result.paths.is_empty());
    }

    #[tokio::test]
    async fn graph_search_case_insensitive() {
        let store = InMemoryGraphStore::new();
        store.add_triple(GraphTriple::new("Rust".into(), "is_a".into(), "Language".into())).await.unwrap();

        let results = store.search("rust", 10).await.unwrap();
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn graph_search_by_predicate() {
        let store = InMemoryGraphStore::new();
        store.add_triple(GraphTriple::new("a".into(), "knows".into(), "b".into())).await.unwrap();
        store.add_triple(GraphTriple::new("c".into(), "hates".into(), "d".into())).await.unwrap();

        let results = store.search("knows", 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].subject, "a");
    }

    #[tokio::test]
    async fn graph_search_by_object() {
        let store = InMemoryGraphStore::new();
        store.add_triple(GraphTriple::new("a".into(), "rel".into(), "paris".into())).await.unwrap();
        store.add_triple(GraphTriple::new("b".into(), "rel".into(), "london".into())).await.unwrap();

        let results = store.search("paris", 10).await.unwrap();
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn graph_search_empty_query() {
        let store = InMemoryGraphStore::new();
        store.add_triple(GraphTriple::new("a".into(), "b".into(), "c".into())).await.unwrap();

        let results = store.search("", 10).await.unwrap();
        // Empty query matches everything (contains(""))
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn graph_search_limit() {
        let store = InMemoryGraphStore::new();
        for i in 0..10 {
            store.add_triple(GraphTriple::new(
                format!("a{}", i), "rel".into(), format!("b{}", i),
            )).await.unwrap();
        }

        let results = store.search("a", 3).await.unwrap();
        assert!(results.len() <= 3);
    }

    #[tokio::test]
    async fn graph_query_triples_by_subject() {
        let store = InMemoryGraphStore::new();
        store.add_triple(GraphTriple::new("alice".into(), "knows".into(), "bob".into())).await.unwrap();
        store.add_triple(GraphTriple::new("carol".into(), "knows".into(), "dave".into())).await.unwrap();

        let results = store.query_triples("alice", "", "").await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].subject, "alice");
    }

    #[tokio::test]
    async fn graph_query_triples_by_predicate() {
        let store = InMemoryGraphStore::new();
        store.add_triple(GraphTriple::new("a".into(), "knows".into(), "b".into())).await.unwrap();
        store.add_triple(GraphTriple::new("c".into(), "hates".into(), "d".into())).await.unwrap();

        let results = store.query_triples("", "knows", "").await.unwrap();
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn graph_query_triples_by_object() {
        let store = InMemoryGraphStore::new();
        store.add_triple(GraphTriple::new("a".into(), "rel".into(), "target".into())).await.unwrap();
        store.add_triple(GraphTriple::new("b".into(), "rel".into(), "other".into())).await.unwrap();

        let results = store.query_triples("", "", "target").await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].object, "target");
    }

    #[tokio::test]
    async fn graph_query_triples_wildcard() {
        let store = InMemoryGraphStore::new();
        store.add_triple(GraphTriple::new("a".into(), "rel1".into(), "b".into())).await.unwrap();
        store.add_triple(GraphTriple::new("c".into(), "rel2".into(), "d".into())).await.unwrap();

        let results = store.query_triples("", "", "").await.unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn graph_get_related_depth_one() {
        let store = InMemoryGraphStore::new();
        store.add_triple(GraphTriple::new("a".into(), "rel".into(), "b".into())).await.unwrap();
        store.add_triple(GraphTriple::new("b".into(), "rel".into(), "c".into())).await.unwrap();
        store.add_triple(GraphTriple::new("d".into(), "rel".into(), "e".into())).await.unwrap();

        let related = store.get_related("a", 1).await.unwrap();
        assert_eq!(related.len(), 1); // Only a->b
    }

    #[tokio::test]
    async fn graph_get_related_depth_zero_defaults_to_one() {
        let store = InMemoryGraphStore::new();
        store.add_triple(GraphTriple::new("a".into(), "rel".into(), "b".into())).await.unwrap();

        let related = store.get_related("a", 0).await.unwrap();
        assert_eq!(related.len(), 1); // depth=0 defaults to 1
    }

    #[tokio::test]
    async fn graph_get_related_bidirectional() {
        let store = InMemoryGraphStore::new();
        store.add_triple(GraphTriple::new("a".into(), "rel".into(), "b".into())).await.unwrap();
        store.add_triple(GraphTriple::new("c".into(), "back".into(), "a".into())).await.unwrap();

        let related = store.get_related("a", 1).await.unwrap();
        // Should find both a->b and c->a
        assert_eq!(related.len(), 2);
    }

    #[tokio::test]
    async fn graph_delete_entity_cascades_triples() {
        let store = InMemoryGraphStore::new();
        store.upsert_entity(GraphEntity::new("target".into(), "thing".into())).await.unwrap();
        store.add_triple(GraphTriple::new("a".into(), "refers".into(), "target".into())).await.unwrap();
        store.add_triple(GraphTriple::new("target".into(), "knows".into(), "b".into())).await.unwrap();
        store.add_triple(GraphTriple::new("c".into(), "unrelated".into(), "d".into())).await.unwrap();

        store.delete_entity("target").await.unwrap();

        // Entity gone
        assert!(store.get_entity("target").await.unwrap().is_none());

        // All triples involving "target" should be gone
        let remaining = store.list_triples("target").await.unwrap();
        assert!(remaining.is_empty());

        // Unrelated triples should remain
        let unrelated = store.list_triples("c").await.unwrap();
        assert_eq!(unrelated.len(), 1);
    }

    #[tokio::test]
    async fn graph_delete_nonexistent_entity() {
        let store = InMemoryGraphStore::new();
        // Should not panic
        store.delete_entity("ghost").await.unwrap();
        assert_eq!(store.entity_count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn graph_remove_triple_nonexistent() {
        let store = InMemoryGraphStore::new();
        let removed = store.remove_triple("x", "y", "z").await.unwrap();
        assert!(!removed);
    }

    #[tokio::test]
    async fn graph_entity_count_multiple() {
        let store = InMemoryGraphStore::new();
        for i in 0..5 {
            store.upsert_entity(GraphEntity::new(format!("e{}", i), "thing".into())).await.unwrap();
        }
        assert_eq!(store.entity_count().await.unwrap(), 5);
    }

    #[tokio::test]
    async fn graph_triple_count_multiple() {
        let store = InMemoryGraphStore::new();
        for i in 0..5 {
            store.add_triple(GraphTriple::new(
                format!("s{}", i), "rel".into(), format!("o{}", i),
            )).await.unwrap();
        }
        assert_eq!(store.triple_count().await.unwrap(), 5);
    }

    #[tokio::test]
    async fn graph_upsert_entity_overwrites() {
        let store = InMemoryGraphStore::new();
        store.upsert_entity(GraphEntity::new("x".into(), "original".into())).await.unwrap();
        store.upsert_entity(GraphEntity::new("x".into(), "updated".into())).await.unwrap();

        let entity = store.get_entity("x").await.unwrap().unwrap();
        assert_eq!(entity.typ, "updated");
        assert_eq!(store.entity_count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn graph_list_triples_no_match() {
        let store = InMemoryGraphStore::new();
        store.add_triple(GraphTriple::new("a".into(), "b".into(), "c".into())).await.unwrap();

        let triples = store.list_triples("nonexistent").await.unwrap();
        assert!(triples.is_empty());
    }

    #[tokio::test]
    async fn graph_persist_and_reload_full_cycle() {
        let dir = tempfile::tempdir().unwrap();

        {
            let store = InMemoryGraphStore::new()
                .with_persistence(dir.path().to_path_buf());
            store.upsert_entity(GraphEntity::new("alice".into(), "person".into())).await.unwrap();
            store.upsert_entity(GraphEntity::new("bob".into(), "person".into())).await.unwrap();
            store.add_triple(GraphTriple::new("alice".into(), "knows".into(), "bob".into())).await.unwrap();
            store.add_triple(GraphTriple::new("bob".into(), "works_with".into(), "alice".into())).await.unwrap();

            // Remove one triple
            store.remove_triple("bob", "works_with", "alice").await.unwrap();
        }

        // Reload
        let store2 = InMemoryGraphStore::new()
            .with_persistence(dir.path().to_path_buf());
        assert_eq!(store2.entity_count().await.unwrap(), 2);
        assert_eq!(store2.triple_count().await.unwrap(), 1);

        let triple = store2.list_triples("alice").await.unwrap();
        assert_eq!(triple.len(), 1);
        assert_eq!(triple[0].predicate, "knows");
    }

    #[tokio::test]
    async fn graph_multiple_triples_same_subject() {
        let store = InMemoryGraphStore::new();
        store.add_triple(GraphTriple::new("a".into(), "rel1".into(), "b".into())).await.unwrap();
        store.add_triple(GraphTriple::new("a".into(), "rel2".into(), "c".into())).await.unwrap();
        store.add_triple(GraphTriple::new("a".into(), "rel3".into(), "d".into())).await.unwrap();

        let triples = store.list_triples("a").await.unwrap();
        assert_eq!(triples.len(), 3);
    }

    #[tokio::test]
    async fn graph_path_hop_fields() {
        let store = InMemoryGraphStore::new();
        let triple = GraphTriple::new("x".into(), "connects".into(), "y".into())
            .with_confidence(0.9);
        store.add_triple(triple).await.unwrap();

        let result = store.query_bfs("x", 1).await.unwrap();
        assert_eq!(result.paths.len(), 1);
        assert_eq!(result.paths[0].len(), 1);
        let hop = &result.paths[0][0];
        assert_eq!(hop.subject, "x");
        assert_eq!(hop.predicate, "connects");
        assert_eq!(hop.object, "y");
        assert!((hop.confidence - 0.9).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn graph_default_store() {
        let store = InMemoryGraphStore::default();
        assert_eq!(store.entity_count().await.unwrap(), 0);
        assert_eq!(store.triple_count().await.unwrap(), 0);
    }
}
