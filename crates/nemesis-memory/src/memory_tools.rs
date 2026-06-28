//! Memory tools - agent tool definitions and Execute logic for the memory subsystem.
//!
//! Defines four agent tools: memory_search, memory_store, memory_forget, memory_list.
//! Each tool has a full implementation that delegates to the MemoryManager.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::graph::GraphTriple;
use crate::manager::MemoryManager;

// ---------------------------------------------------------------------------
// Tool definition types
// ---------------------------------------------------------------------------

/// Tool definition for memory operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryTool {
    /// Tool name.
    pub name: String,
    /// Tool description.
    pub description: String,
    /// Tool parameter schema (JSON Schema).
    pub parameters: serde_json::Value,
}

/// Result of executing a memory tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryToolResult {
    /// Whether the execution succeeded.
    pub success: bool,
    /// Result content.
    pub content: String,
}

impl MemoryToolResult {
    /// Create a successful result.
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            success: true,
            content: content.into(),
        }
    }

    /// Create an error result.
    pub fn err(message: impl Into<String>) -> Self {
        Self {
            success: false,
            content: message.into(),
        }
    }
}

/// Returns all memory tool definitions.
pub fn memory_tool_definitions() -> Vec<MemoryTool> {
    vec![
        MemoryTool {
            name: "memory_search".into(),
            description: "Search stored memories. Can search episodic memories (past conversations) or knowledge graph (entities and relationships).".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Search query text"},
                    "memory_type": {"type": "string", "description": "Type of memory to search: 'episodic', 'graph', or 'all'", "default": "all"},
                    "limit": {"type": "number", "default": 10, "description": "Maximum number of results (1-50)"},
                },
                "required": ["query"],
            }),
        },
        MemoryTool {
            name: "memory_store".into(),
            description: "Store information in memory. Can save episodic memories (conversation experiences) or knowledge graph entities and relationships.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "memory_type": {"type": "string", "description": "Type of memory to store: 'episodic' or 'graph'", "enum": ["episodic", "graph"], "default": "episodic"},
                    "content": {"type": "string", "description": "Content for episodic memory (ignored for graph type)"},
                    "role": {"type": "string", "description": "Role for episodic memory: 'user', 'assistant', 'system'", "default": "assistant"},
                    "tags": {"type": "array", "items": {"type": "string"}, "description": "Tags for the memory entry"},
                    "session_key": {"type": "string", "description": "Session key for episodic memory (auto-generated if empty)"},
                    "entity_name": {"type": "string", "description": "Entity name for graph memory"},
                    "entity_type": {"type": "string", "description": "Entity type for graph: 'person', 'place', 'thing', 'concept'"},
                    "entity_properties": {"type": "object", "description": "Additional properties for the entity (string key-value pairs)"},
                    "triple_subject": {"type": "string", "description": "Subject of a graph triple"},
                    "triple_predicate": {"type": "string", "description": "Predicate (relationship type) of a graph triple"},
                    "triple_object": {"type": "string", "description": "Object of a graph triple"},
                    "confidence": {"type": "number", "description": "Confidence score for the triple (0.0-1.0)"},
                },
                "required": [],
            }),
        },
        MemoryTool {
            name: "memory_forget".into(),
            description: "Remove memories. Can delete by ID, delete episodic sessions, cleanup old memories, or remove knowledge graph entities.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {"type": "string", "description": "Action to perform: 'delete_session', 'cleanup', 'delete_entity', 'delete_by_id'", "enum": ["delete_session", "cleanup", "delete_entity", "delete_by_id"]},
                    "session_key": {"type": "string", "description": "Session key to delete (for delete_session action)"},
                    "older_than_days": {"type": "number", "description": "Remove memories older than N days (for cleanup action)", "minimum": 1},
                    "entity_name": {"type": "string", "description": "Entity name to delete (for delete_entity action)"},
                    "id": {"type": "string", "description": "Memory entry ID to delete (for delete_by_id action). Use the ID from memory_search results."},
                },
                "required": ["action"],
            }),
        },
        MemoryTool {
            name: "memory_list".into(),
            description: "List stored memories. Shows episodic memory sessions, knowledge graph entities, or related graph entries.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "list_type": {"type": "string", "description": "What to list: 'episodes', 'graph_query', 'graph_related', or 'status'", "default": "status"},
                    "session_key": {"type": "string", "description": "Session key for listing episodes"},
                    "limit": {"type": "number", "default": 10, "description": "Maximum number of results (1-50)"},
                    "entity_name": {"type": "string", "description": "Entity name for graph_related"},
                    "depth": {"type": "number", "description": "Depth for graph_related search (1-3)", "default": 1},
                    "subject": {"type": "string", "description": "Filter by subject for graph_query"},
                    "predicate": {"type": "string", "description": "Filter by predicate for graph_query"},
                    "object": {"type": "string", "description": "Filter by object for graph_query"},
                },
            }),
        },
    ]
}

// ---------------------------------------------------------------------------
// Tool Executor
// ---------------------------------------------------------------------------

/// Approval gate for agent-initiated memory writes/deletes. Implemented by the
/// agent/gateway layer (which owns the approval middleware) and injected into
/// `MemoryToolExecutor`. YOLO/auto modes must NOT bypass this — memory is a
/// high-trust surface, so every agent store/forget needs a fresh human approval.
/// When no gate is attached, store/forget run ungated (backward compatible).
#[async_trait::async_trait]
pub trait MemoryApprovalGate: Send + Sync {
    /// Approve a `memory_store` call. `preview` is a short human-readable summary
    /// (type + key fields + content snippet) shown in the approval prompt.
    async fn approve_store(&self, preview: &str) -> bool;
    /// Approve a `memory_forget` call. `preview` describes what will be removed.
    async fn approve_forget(&self, preview: &str) -> bool;
}

/// Executes a memory tool by name, delegating to the MemoryManager.
pub struct MemoryToolExecutor {
    manager: Arc<MemoryManager>,
    approval_gate: parking_lot::Mutex<Option<Arc<dyn MemoryApprovalGate>>>,
}

impl MemoryToolExecutor {
    /// Create a new executor backed by the given MemoryManager (no approval gate).
    pub fn new(manager: Arc<MemoryManager>) -> Self {
        Self {
            manager,
            approval_gate: parking_lot::Mutex::new(None),
        }
    }

    /// Attach an approval gate (called by the gateway after wiring the approval
    /// middleware). After this, agent `memory_store`/`memory_forget` calls require
    /// human approval via the gate.
    pub fn set_approval_gate(&self, gate: Arc<dyn MemoryApprovalGate>) {
        *self.approval_gate.lock() = Some(gate);
    }

    /// Returns true if the store is approved (or no gate is attached). The lock is
    /// released before awaiting so the gate can itself await user input safely.
    async fn check_store_approved(&self, preview: &str) -> bool {
        let gate = self.approval_gate.lock().as_ref().map(Arc::clone);
        match gate {
            Some(g) => g.approve_store(preview).await,
            None => true,
        }
    }

    async fn check_forget_approved(&self, preview: &str) -> bool {
        let gate = self.approval_gate.lock().as_ref().map(Arc::clone);
        match gate {
            Some(g) => g.approve_forget(preview).await,
            None => true,
        }
    }

    /// Execute a tool by name with the provided arguments.
    pub async fn execute(
        &self,
        tool_name: &str,
        args: &serde_json::Value,
    ) -> MemoryToolResult {
        match tool_name {
            "memory_search" => self.execute_search(args).await,
            "memory_store" => self.execute_store(args).await,
            "memory_forget" => self.execute_forget(args).await,
            "memory_list" => self.execute_list(args).await,
            _ => MemoryToolResult::err(format!("Unknown memory tool: {}", tool_name)),
        }
    }

    // -- memory_search -------------------------------------------------------

    async fn execute_search(&self, args: &serde_json::Value) -> MemoryToolResult {
        let query = args["query"].as_str().unwrap_or("").to_string();
        if query.is_empty() {
            return MemoryToolResult::err("query is required");
        }

        let memory_type = args["memory_type"].as_str().unwrap_or("all").to_string();
        let limit = args["limit"].as_u64().unwrap_or(10).min(50) as usize;
        let limit = if limit == 0 { 10 } else { limit };

        let mut output = format!("Memory search results for: {}\n\n", query);

        // Semantic / keyword search over general store (includes vector store)
        if memory_type == "all" {
            match self.manager.search(&query, None, limit).await {
                Ok(results) => {
                    if results.entries.is_empty() {
                        output.push_str("No semantic memories found.\n");
                    } else {
                        output.push_str(&format!(
                            "### Semantic Memories ({} results)\n\n",
                            results.entries.len()
                        ));
                        for (i, se) in results.entries.iter().enumerate() {
                            let score_str = if se.score > 0.0 {
                                format!(" [{:.0}%]", se.score * 100.0)
                            } else {
                                String::new()
                            };
                            output.push_str(&format!(
                                "{}. [ID: {}]{} {}\n",
                                i + 1,
                                se.entry.id,
                                score_str,
                                truncate_text(&se.entry.content, 200)
                            ));
                            if !se.entry.tags.is_empty() {
                                output.push_str(&format!(
                                    "   Tags: {}\n",
                                    se.entry.tags.join(", ")
                                ));
                            }
                        }
                        output.push('\n');
                    }
                }
                Err(e) => {
                    output.push_str(&format!("Semantic search error: {}\n", e));
                }
            }
        }

        // Search episodic memories
        if memory_type == "all" || memory_type == "episodic" {
            match self.manager.search_episodic(&query, limit).await {
                Ok(episodes) => {
                    if episodes.is_empty() {
                        output.push_str("No episodic memories found.\n");
                    } else {
                        output.push_str(&format!(
                            "### Episodic Memories ({} results)\n\n",
                            episodes.len()
                        ));
                        for (i, ep) in episodes.iter().enumerate() {
                            output.push_str(&format!(
                                "{}. [ID: {}] [Session: {}] [{}] {}: {}\n",
                                i + 1,
                                ep.id,
                                ep.session_key,
                                ep.timestamp.format("%Y-%m-%d %H:%M"),
                                ep.role,
                                truncate_text(&ep.content, 200)
                            ));
                            if !ep.tags.is_empty() {
                                output.push_str(&format!(
                                    "   Tags: {}\n",
                                    ep.tags.join(", ")
                                ));
                            }
                        }
                        output.push('\n');
                    }
                }
                Err(e) => {
                    output.push_str(&format!("Episodic search error: {}\n", e));
                }
            }
        }

        // Search knowledge graph
        if memory_type == "all" || memory_type == "graph" {
            match self.manager.search_graph(&query, limit).await {
                Ok(triples) => {
                    if triples.is_empty() {
                        output.push_str("No knowledge graph entries found.\n");
                    } else {
                        output.push_str(&format!(
                            "### Knowledge Graph ({} results)\n\n",
                            triples.len()
                        ));
                        for (i, t) in triples.iter().enumerate() {
                            let confidence = if t.confidence > 0.0 && t.confidence < 1.0 {
                                format!(" (confidence: {:.0}%)", t.confidence * 100.0)
                            } else {
                                String::new()
                            };
                            output.push_str(&format!(
                                "{}. {} --[{}]--> {}{}\n",
                                i + 1,
                                t.subject,
                                t.predicate,
                                t.object,
                                confidence
                            ));
                        }
                    }
                }
                Err(e) => {
                    output.push_str(&format!("Graph search error: {}\n", e));
                }
            }
        }

        MemoryToolResult::ok(output)
    }

    // -- memory_store --------------------------------------------------------

    async fn execute_store(&self, args: &serde_json::Value) -> MemoryToolResult {
        let memory_type = args["memory_type"].as_str().unwrap_or("episodic").to_string();

        // Approval gate — never bypassed by YOLO/auto (enforced by the gate impl).
        let store_preview = format!(
            "store {} memory: {}",
            memory_type,
            args["content"]
                .as_str()
                .or_else(|| args["entity_name"].as_str())
                .unwrap_or("(graph triple)")
        );
        if !self.check_store_approved(&store_preview).await {
            return MemoryToolResult::err(
                "memory_store denied: pending human approval (not approved)",
            );
        }

        match memory_type.as_str() {
            "episodic" => self.store_episodic(args).await,
            "graph" => self.store_graph(args).await,
            _ => MemoryToolResult::err(format!(
                "unknown memory_type: {} (use 'episodic' or 'graph')",
                memory_type
            )),
        }
    }

    async fn store_episodic(&self, args: &serde_json::Value) -> MemoryToolResult {
        let content = args["content"].as_str().unwrap_or("").to_string();
        if content.is_empty() {
            return MemoryToolResult::err("content is required for episodic memory");
        }

        let role = args["role"]
            .as_str()
            .unwrap_or("assistant")
            .to_string();

        let session_key = match args["session_key"].as_str() {
            Some(sk) if !sk.is_empty() => sk.to_string(),
            _ => format!("manual-{}", chrono::Local::now().timestamp()),
        };

        let tags: Vec<String> = args["tags"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let episode = crate::episodic::Episode {
            id: uuid::Uuid::new_v4().to_string(),
            session_key: session_key.clone(),
            role,
            content,
            timestamp: chrono::Local::now(),
            metadata: std::collections::HashMap::new(),
            tags,
        };

        match self.manager.append_episode(episode.clone()).await {
            Ok(id) => MemoryToolResult::ok(format!(
                "Episodic memory stored successfully (ID: {}, session: {})",
                id, session_key
            )),
            Err(e) => MemoryToolResult::err(format!(
                "failed to store episodic memory: {}",
                e
            )),
        }
    }

    async fn store_graph(&self, args: &serde_json::Value) -> MemoryToolResult {
        let mut results = Vec::new();

        // Store entity if provided
        let entity_name = args["entity_name"].as_str().unwrap_or("").to_string();
        if !entity_name.is_empty() {
            let entity_type = args["entity_type"]
                .as_str()
                .unwrap_or("concept")
                .to_string();

            let properties: std::collections::HashMap<String, String> = args
                ["entity_properties"]
                .as_object()
                .map(|obj| {
                    obj.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
                .unwrap_or_default();

            let entity = crate::graph::GraphEntity {
                name: entity_name.clone(),
                typ: entity_type.clone(),
                properties,
                created_at: chrono::Local::now(),
            };

            match self.manager.upsert_entity(entity).await {
                Ok(()) => {
                    results.push(format!("Entity stored: {} ({})", entity_name, entity_type));
                }
                Err(e) => {
                    return MemoryToolResult::err(format!("failed to store entity: {}", e));
                }
            }
        }

        // Store triple if provided
        let subject = args["triple_subject"].as_str().unwrap_or("").to_string();
        let predicate = args["triple_predicate"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let object = args["triple_object"].as_str().unwrap_or("").to_string();

        if !subject.is_empty() && !predicate.is_empty() && !object.is_empty() {
            let confidence = args["confidence"].as_f64().unwrap_or(1.0);
            let confidence = if confidence > 0.0 && confidence <= 1.0 {
                confidence
            } else {
                1.0
            };

            let triple = GraphTriple::new(subject.clone(), predicate.clone(), object.clone())
                .with_confidence(confidence);

            match self.manager.add_triple(triple).await {
                Ok(()) => {
                    results.push(format!(
                        "Triple stored: {} --[{}]--> {}",
                        subject, predicate, object
                    ));
                }
                Err(e) => {
                    return MemoryToolResult::err(format!("failed to store triple: {}", e));
                }
            }
        }

        if results.is_empty() {
            return MemoryToolResult::err(
                "for graph memory, provide entity_name and/or triple_subject+triple_predicate+triple_object",
            );
        }

        MemoryToolResult::ok(format!(
            "Graph memory stored:\n{}",
            results.join("\n")
        ))
    }

    // -- memory_forget -------------------------------------------------------

    async fn execute_forget(&self, args: &serde_json::Value) -> MemoryToolResult {
        let action = args["action"].as_str().unwrap_or("").to_string();
        if action.is_empty() {
            return MemoryToolResult::err("action is required");
        }

        // Approval gate — never bypassed by YOLO/auto (enforced by the gate impl).
        let mut forget_preview = format!("forget memory (action={}", action);
        if let Some(s) = args["session_key"].as_str() {
            forget_preview.push_str(&format!(", session={}", s));
        }
        if let Some(i) = args["id"].as_str() {
            forget_preview.push_str(&format!(", id={}", i));
        }
        forget_preview.push(')');
        if !self.check_forget_approved(&forget_preview).await {
            return MemoryToolResult::err(
                "memory_forget denied: pending human approval (not approved)",
            );
        }

        match action.as_str() {
            "delete_session" => {
                let session_key = args["session_key"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                if session_key.is_empty() {
                    return MemoryToolResult::err(
                        "session_key is required for delete_session action",
                    );
                }

                match self.manager.delete_episode_session(&session_key).await {
                    Ok(count) => MemoryToolResult::ok(format!(
                        "Session '{}' deleted successfully ({} episodes removed)",
                        session_key, count
                    )),
                    Err(e) => {
                        MemoryToolResult::err(format!("failed to delete session: {}", e))
                    }
                }
            }

            "cleanup" => {
                let older_than_days = args["older_than_days"].as_u64().unwrap_or(90) as usize;
                if older_than_days == 0 {
                    return MemoryToolResult::err("older_than_days must be at least 1");
                }

                match self.manager.cleanup_episodic(older_than_days).await {
                    Ok(removed) => MemoryToolResult::ok(format!(
                        "Cleanup completed: removed {} episodes older than {} days",
                        removed, older_than_days
                    )),
                    Err(e) => MemoryToolResult::err(format!("cleanup failed: {}", e)),
                }
            }

            "delete_entity" => {
                let entity_name = args["entity_name"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                if entity_name.is_empty() {
                    return MemoryToolResult::err(
                        "entity_name is required for delete_entity action",
                    );
                }

                match self.manager.delete_graph_entity(&entity_name).await {
                    Ok(()) => MemoryToolResult::ok(format!(
                        "Entity '{}' and all related triples deleted",
                        entity_name
                    )),
                    Err(e) => {
                        MemoryToolResult::err(format!("failed to delete entity: {}", e))
                    }
                }
            }

            "delete_by_id" => {
                let id = args["id"].as_str().unwrap_or("").to_string();
                if id.is_empty() {
                    return MemoryToolResult::err(
                        "id is required for delete_by_id action. Use memory_search to find the ID first.",
                    );
                }

                match self.manager.delete_by_id(&id).await {
                    Ok(true) => MemoryToolResult::ok(format!(
                        "Memory entry '{}' deleted successfully",
                        id
                    )),
                    Ok(false) => MemoryToolResult::ok(format!(
                        "No memory entry found with ID '{}'. It may have already been deleted.",
                        id
                    )),
                    Err(e) => {
                        MemoryToolResult::err(format!("failed to delete by ID: {}", e))
                    }
                }
            }

            _ => MemoryToolResult::err(format!(
                "unknown action: {} (use 'delete_session', 'cleanup', 'delete_entity', or 'delete_by_id')",
                action
            )),
        }
    }

    // -- memory_list ---------------------------------------------------------

    async fn execute_list(&self, args: &serde_json::Value) -> MemoryToolResult {
        let list_type = args["list_type"]
            .as_str()
            .unwrap_or("status")
            .to_string();

        match list_type.as_str() {
            "status" => self.list_status().await,
            "episodes" => self.list_episodes(args).await,
            "graph_query" => self.list_graph_query(args).await,
            "graph_related" => self.list_graph_related(args).await,
            _ => MemoryToolResult::err(format!("unknown list_type: {}", list_type)),
        }
    }

    async fn list_status(&self) -> MemoryToolResult {
        let mut output = String::from("## Memory Store Status\n\n");

        // Episodic status
        match self.manager.episodic_stats().await {
            Ok((session_count, episode_count)) => {
                output.push_str("### Episodic Memory\n");
                output.push_str(&format!("- Sessions: {}\n", session_count));
                output.push_str(&format!("- Total episodes: {}\n", episode_count));
                // List session keys
                if let Ok(sessions) = self.manager.list_episodic_sessions().await {
                    if !sessions.is_empty() {
                        output.push_str("- Session keys:\n");
                        for sk in &sessions {
                            output.push_str(&format!("  - {}\n", sk));
                        }
                    }
                }
                output.push('\n');
            }
            Err(_) => {
                output.push_str("### Episodic Memory\n- Not available\n\n");
            }
        }

        // Graph status
        match self.manager.graph_stats().await {
            Ok((entity_count, triple_count)) => {
                output.push_str("### Knowledge Graph\n");
                output.push_str(&format!("- Entities: {}\n", entity_count));
                output.push_str(&format!("- Triples: {}\n", triple_count));
            }
            Err(_) => {
                output.push_str("### Knowledge Graph\n- Not available\n");
            }
        }

        MemoryToolResult::ok(output)
    }

    async fn list_episodes(&self, args: &serde_json::Value) -> MemoryToolResult {
        let session_key = args["session_key"]
            .as_str()
            .unwrap_or("")
            .to_string();
        if session_key.is_empty() {
            return MemoryToolResult::err(
                "session_key is required for episodes listing",
            );
        }

        let limit = args["limit"].as_u64().unwrap_or(10).min(50) as usize;
        let limit = if limit == 0 { 10 } else { limit };

        match self
            .manager
            .get_recent_episodes(&session_key, limit)
            .await
        {
            Ok(episodes) => {
                if episodes.is_empty() {
                    return MemoryToolResult::ok(format!(
                        "No episodes found for session: {}",
                        session_key
                    ));
                }

                let mut output = format!(
                    "### Recent Episodes for {} ({} results)\n\n",
                    session_key,
                    episodes.len()
                );
                for (i, ep) in episodes.iter().enumerate() {
                    output.push_str(&format!(
                        "{}. [{}] {}: {}\n",
                        i + 1,
                        ep.timestamp.format("%Y-%m-%d %H:%M"),
                        ep.role,
                        truncate_text(&ep.content, 200)
                    ));
                    if !ep.tags.is_empty() {
                        output.push_str(&format!("   Tags: {}\n", ep.tags.join(", ")));
                    }
                }

                MemoryToolResult::ok(output)
            }
            Err(e) => MemoryToolResult::err(format!("failed to list episodes: {}", e)),
        }
    }

    async fn list_graph_query(&self, args: &serde_json::Value) -> MemoryToolResult {
        let subject = args["subject"].as_str().unwrap_or("").to_string();
        let predicate = args["predicate"].as_str().unwrap_or("").to_string();
        let object = args["object"].as_str().unwrap_or("").to_string();

        match self
            .manager
            .query_graph_triples(&subject, &predicate, &object)
            .await
        {
            Ok(triples) => {
                if triples.is_empty() {
                    return MemoryToolResult::ok("No matching triples found");
                }

                let mut output =
                    format!("### Graph Query Results ({} triples)\n\n", triples.len());
                for (i, t) in triples.iter().enumerate() {
                    let confidence = if t.confidence > 0.0 && t.confidence < 1.0 {
                        format!(" ({:.0}%)", t.confidence * 100.0)
                    } else {
                        String::new()
                    };
                    output.push_str(&format!(
                        "{}. {} --[{}]--> {}{}\n",
                        i + 1,
                        t.subject,
                        t.predicate,
                        t.object,
                        confidence
                    ));
                    if !t.metadata.is_empty() {
                        let meta_str: Vec<String> = t
                            .metadata
                            .iter()
                            .map(|(k, v)| format!("{}={}", k, v))
                            .collect();
                        output.push_str(&format!("   Metadata: {}\n", meta_str.join(", ")));
                    }
                }

                MemoryToolResult::ok(output)
            }
            Err(e) => MemoryToolResult::err(format!("query failed: {}", e)),
        }
    }

    async fn list_graph_related(&self, args: &serde_json::Value) -> MemoryToolResult {
        let entity_name = args["entity_name"]
            .as_str()
            .unwrap_or("")
            .to_string();
        if entity_name.is_empty() {
            return MemoryToolResult::err("entity_name is required for graph_related listing");
        }

        let depth = args["depth"].as_u64().unwrap_or(1).min(3) as usize;
        let depth = if depth == 0 { 1 } else { depth };

        // Show entity info first
        let mut output = String::new();
        match self.manager.get_graph_entity(&entity_name).await {
            Ok(Some(entity)) => {
                output.push_str(&format!("### Entity: {}\n", entity.name));
                output.push_str(&format!("- Type: {}\n", entity.typ));
                if !entity.properties.is_empty() {
                    output.push_str("- Properties:\n");
                    for (k, v) in &entity.properties {
                        output.push_str(&format!("  - {}: {}\n", k, v));
                    }
                }
                output.push('\n');
            }
            Ok(None) => {}
            Err(_) => {}
        }

        match self
            .manager
            .get_related_triples(&entity_name, depth)
            .await
        {
            Ok(triples) => {
                if triples.is_empty() {
                    return MemoryToolResult::ok(format!(
                        "No relationships found for: {}",
                        entity_name
                    ));
                }

                output.push_str(&format!(
                    "### Related to {} (depth={}, {} triples)\n\n",
                    entity_name,
                    depth,
                    triples.len()
                ));
                for (i, t) in triples.iter().enumerate() {
                    output.push_str(&format!(
                        "{}. {} --[{}]--> {}\n",
                        i + 1,
                        t.subject,
                        t.predicate,
                        t.object
                    ));
                }

                MemoryToolResult::ok(output)
            }
            Err(e) => {
                MemoryToolResult::err(format!("failed to get related entities: {}", e))
            }
        }
    }
}

/// Truncate text to max_len characters, adding "..." if truncated.
fn truncate_text(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        return s;
    }
    let boundary = if max_len > 3 { max_len - 3 } else { max_len };
    // Find a valid char boundary
    let mut end = boundary;
    while !s.is_char_boundary(end) && end > 0 {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests;
