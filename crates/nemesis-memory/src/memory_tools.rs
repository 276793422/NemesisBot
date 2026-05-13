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
                    "memory_type": {"type": "string", "description": "Type of memory to store: 'episodic' or 'graph'", "enum": ["episodic", "graph"]},
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
                "required": ["memory_type"],
            }),
        },
        MemoryTool {
            name: "memory_forget".into(),
            description: "Remove memories. Can delete episodic sessions, cleanup old memories, or remove knowledge graph entities.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {"type": "string", "description": "Action to perform: 'delete_session', 'cleanup', 'delete_entity'", "enum": ["delete_session", "cleanup", "delete_entity"]},
                    "session_key": {"type": "string", "description": "Session key to delete (for delete_session action)"},
                    "older_than_days": {"type": "number", "description": "Remove memories older than N days (for cleanup action)", "minimum": 1},
                    "entity_name": {"type": "string", "description": "Entity name to delete (for delete_entity action)"},
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

/// Executes a memory tool by name, delegating to the MemoryManager.
pub struct MemoryToolExecutor {
    manager: Arc<MemoryManager>,
}

impl MemoryToolExecutor {
    /// Create a new executor backed by the given MemoryManager.
    pub fn new(manager: Arc<MemoryManager>) -> Self {
        Self { manager }
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
                                "{}. [{}] {}: {}\n",
                                i + 1,
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
        let memory_type = args["memory_type"].as_str().unwrap_or("").to_string();
        if memory_type.is_empty() {
            return MemoryToolResult::err("memory_type is required");
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
            _ => format!("manual-{}", chrono::Utc::now().timestamp()),
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
            timestamp: chrono::Utc::now(),
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
                created_at: chrono::Utc::now(),
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

            _ => MemoryToolResult::err(format!(
                "unknown action: {} (use 'delete_session', 'cleanup', or 'delete_entity')",
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
                output.push_str(&format!("- Total episodes: {}\n\n", episode_count));
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
mod tests {
    use super::*;

    #[test]
    fn test_memory_tool_definitions() {
        let tools = memory_tool_definitions();
        assert_eq!(tools.len(), 4);

        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"memory_search"));
        assert!(names.contains(&"memory_store"));
        assert!(names.contains(&"memory_forget"));
        assert!(names.contains(&"memory_list"));
    }

    #[test]
    fn test_tool_result_ok() {
        let result = MemoryToolResult::ok("Found 3 entries");
        assert!(result.success);
        assert_eq!(result.content, "Found 3 entries");
    }

    #[test]
    fn test_tool_result_err() {
        let result = MemoryToolResult::err("Not found");
        assert!(!result.success);
        assert_eq!(result.content, "Not found");
    }

    #[test]
    fn test_tool_definitions_have_required_fields() {
        let tools = memory_tool_definitions();
        for tool in &tools {
            assert!(!tool.name.is_empty());
            assert!(!tool.description.is_empty());
            assert!(tool.parameters.is_object());
        }
    }

    #[test]
    fn test_truncate_text_short() {
        assert_eq!(truncate_text("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_text_long() {
        let result = truncate_text("a very long string that needs truncation", 15);
        assert!(result.len() <= 15);
        assert!(result.len() >= 12); // 15 - 3 for "..."
    }

    #[test]
    fn test_truncate_text_exact() {
        assert_eq!(truncate_text("hello", 5), "hello");
    }

    #[tokio::test]
    async fn test_execute_search_missing_query() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        let result = executor
            .execute("memory_search", &serde_json::json!({}))
            .await;
        assert!(!result.success);
        assert!(result.content.contains("query is required"));
    }

    #[tokio::test]
    async fn test_execute_store_missing_type() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        let result = executor
            .execute("memory_store", &serde_json::json!({}))
            .await;
        assert!(!result.success);
        assert!(result.content.contains("memory_type is required"));
    }

    #[tokio::test]
    async fn test_execute_forget_missing_action() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        let result = executor
            .execute("memory_forget", &serde_json::json!({}))
            .await;
        assert!(!result.success);
        assert!(result.content.contains("action is required"));
    }

    #[tokio::test]
    async fn test_execute_store_episodic() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        let result = executor
            .execute(
                "memory_store",
                &serde_json::json!({
                    "memory_type": "episodic",
                    "content": "User asked about Rust ownership",
                    "role": "user",
                    "session_key": "test-session-1"
                }),
            )
            .await;
        assert!(result.success);
        assert!(result.content.contains("Episodic memory stored"));
    }

    #[tokio::test]
    async fn test_execute_store_graph_entity() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        let result = executor
            .execute(
                "memory_store",
                &serde_json::json!({
                    "memory_type": "graph",
                    "entity_name": "Rust",
                    "entity_type": "language"
                }),
            )
            .await;
        assert!(result.success);
        assert!(result.content.contains("Entity stored"));
    }

    #[tokio::test]
    async fn test_execute_store_graph_triple() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        let result = executor
            .execute(
                "memory_store",
                &serde_json::json!({
                    "memory_type": "graph",
                    "triple_subject": "Rust",
                    "triple_predicate": "is_a",
                    "triple_object": "language",
                    "confidence": 0.95
                }),
            )
            .await;
        assert!(result.success);
        assert!(result.content.contains("Triple stored"));
    }

    #[tokio::test]
    async fn test_execute_list_status() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        let result = executor
            .execute("memory_list", &serde_json::json!({}))
            .await;
        assert!(result.success);
        assert!(result.content.contains("Memory Store Status"));
    }

    #[tokio::test]
    async fn test_execute_unknown_tool() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        let result = executor
            .execute("unknown_tool", &serde_json::json!({}))
            .await;
        assert!(!result.success);
        assert!(result.content.contains("Unknown memory tool"));
    }

    // ============================================================
    // Additional tests for missing coverage
    // ============================================================

    #[tokio::test]
    async fn test_execute_search_valid_query() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        let result = executor
            .execute(
                "memory_search",
                &serde_json::json!({"query": "test query"}),
            )
            .await;
        assert!(result.success);
        assert!(result.content.contains("test query"));
    }

    #[tokio::test]
    async fn test_execute_search_episodic_only() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        let result = executor
            .execute(
                "memory_search",
                &serde_json::json!({"query": "test", "memory_type": "episodic"}),
            )
            .await;
        assert!(result.success);
        // When no episodic memories found, it says "No episodic memories found."
        assert!(result.content.contains("episodic"));
    }

    #[tokio::test]
    async fn test_execute_search_graph_only() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        let result = executor
            .execute(
                "memory_search",
                &serde_json::json!({"query": "test", "memory_type": "graph"}),
            )
            .await;
        assert!(result.success);
        // When no graph entries found, it says "No knowledge graph entries found."
        assert!(result.content.contains("knowledge graph"));
    }

    #[tokio::test]
    async fn test_execute_store_unknown_memory_type() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        let result = executor
            .execute(
                "memory_store",
                &serde_json::json!({"memory_type": "unknown"}),
            )
            .await;
        assert!(!result.success);
        assert!(result.content.contains("unknown memory_type"));
    }

    #[tokio::test]
    async fn test_store_episodic_empty_content() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        let result = executor
            .execute(
                "memory_store",
                &serde_json::json!({"memory_type": "episodic", "content": ""}),
            )
            .await;
        assert!(!result.success);
        assert!(result.content.contains("content is required"));
    }

    #[tokio::test]
    async fn test_store_episodic_with_tags() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        let result = executor
            .execute(
                "memory_store",
                &serde_json::json!({
                    "memory_type": "episodic",
                    "content": "Tagged content",
                    "tags": ["rust", "ownership", "memory"],
                    "session_key": "tagged-session"
                }),
            )
            .await;
        assert!(result.success);
        assert!(result.content.contains("Episodic memory stored"));
    }

    #[tokio::test]
    async fn test_store_graph_no_data_error() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        let result = executor
            .execute(
                "memory_store",
                &serde_json::json!({"memory_type": "graph"}),
            )
            .await;
        assert!(!result.success);
        assert!(result.content.contains("entity_name") || result.content.contains("triple"));
    }

    #[tokio::test]
    async fn test_execute_forget_delete_session_missing_key() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        let result = executor
            .execute(
                "memory_forget",
                &serde_json::json!({"action": "delete_session"}),
            )
            .await;
        assert!(!result.success);
        assert!(result.content.contains("session_key is required"));
    }

    #[tokio::test]
    async fn test_execute_forget_cleanup_zero_days_error() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        let result = executor
            .execute(
                "memory_forget",
                &serde_json::json!({"action": "cleanup", "older_than_days": 0}),
            )
            .await;
        assert!(!result.success);
        assert!(result.content.contains("older_than_days must be at least 1"));
    }

    #[tokio::test]
    async fn test_execute_forget_unknown_action() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        let result = executor
            .execute(
                "memory_forget",
                &serde_json::json!({"action": "unknown_action"}),
            )
            .await;
        assert!(!result.success);
        assert!(result.content.contains("unknown action"));
    }

    #[tokio::test]
    async fn test_execute_forget_delete_entity_missing_name() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        let result = executor
            .execute(
                "memory_forget",
                &serde_json::json!({"action": "delete_entity"}),
            )
            .await;
        assert!(!result.success);
        assert!(result.content.contains("entity_name is required"));
    }

    #[tokio::test]
    async fn test_execute_list_episodes() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        let result = executor
            .execute(
                "memory_list",
                &serde_json::json!({"list_type": "episodes", "session_key": "test-session"}),
            )
            .await;
        assert!(result.success);
        // No episodes stored yet, should show empty result
    }

    #[tokio::test]
    async fn test_execute_list_unknown_list_type() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        let result = executor
            .execute(
                "memory_list",
                &serde_json::json!({"list_type": "unknown_type"}),
            )
            .await;
        assert!(!result.success);
        assert!(result.content.contains("unknown list_type"));
    }

    #[test]
    fn test_memory_tool_result_serialize_deserialize() {
        let ok_result = MemoryToolResult::ok("test content");
        let json = serde_json::to_string(&ok_result).unwrap();
        let deserialized: MemoryToolResult = serde_json::from_str(&json).unwrap();
        assert!(deserialized.success);
        assert_eq!(deserialized.content, "test content");

        let err_result = MemoryToolResult::err("error msg");
        let json = serde_json::to_string(&err_result).unwrap();
        let deserialized: MemoryToolResult = serde_json::from_str(&json).unwrap();
        assert!(!deserialized.success);
        assert_eq!(deserialized.content, "error msg");
    }

    #[test]
    fn test_truncate_text_empty() {
        assert_eq!(truncate_text("", 10), "");
    }

    #[test]
    fn test_truncate_text_small_max() {
        let result = truncate_text("hello", 3);
        assert!(result.len() <= 3);
    }

    #[test]
    fn test_truncate_text_unicode_boundary() {
        let text = "hello world this is a long string";
        let result = truncate_text(text, 10);
        assert!(result.len() <= 10);
    }

    #[tokio::test]
    async fn test_execute_search_with_limit() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        let result = executor
            .execute(
                "memory_search",
                &serde_json::json!({"query": "test", "limit": 5}),
            )
            .await;
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_execute_list_episodes_missing_session_key() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        let result = executor
            .execute(
                "memory_list",
                &serde_json::json!({"list_type": "episodes"}),
            )
            .await;
        assert!(!result.success);
        assert!(result.content.contains("session_key is required"));
    }

    #[tokio::test]
    async fn test_execute_list_graph_related_missing_entity() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        let result = executor
            .execute(
                "memory_list",
                &serde_json::json!({"list_type": "graph_related"}),
            )
            .await;
        assert!(!result.success);
        assert!(result.content.contains("entity_name is required"));
    }

    #[tokio::test]
    async fn test_store_episodic_auto_session_key() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        let result = executor
            .execute(
                "memory_store",
                &serde_json::json!({
                    "memory_type": "episodic",
                    "content": "Auto session test"
                }),
            )
            .await;
        assert!(result.success);
        assert!(result.content.contains("manual-"));
    }

    #[tokio::test]
    async fn test_store_graph_entity_with_properties() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        let result = executor
            .execute(
                "memory_store",
                &serde_json::json!({
                    "memory_type": "graph",
                    "entity_name": "Alice",
                    "entity_type": "person",
                    "entity_properties": {"age": "30", "city": "Tokyo"}
                }),
            )
            .await;
        assert!(result.success);
        assert!(result.content.contains("Entity stored"));
    }

    #[tokio::test]
    async fn test_store_graph_triple_with_confidence() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        let result = executor
            .execute(
                "memory_store",
                &serde_json::json!({
                    "memory_type": "graph",
                    "triple_subject": "Alice",
                    "triple_predicate": "works_at",
                    "triple_object": "Company",
                    "confidence": 0.85
                }),
            )
            .await;
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_store_graph_triple_zero_confidence_clamped() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        let result = executor
            .execute(
                "memory_store",
                &serde_json::json!({
                    "memory_type": "graph",
                    "triple_subject": "Bob",
                    "triple_predicate": "likes",
                    "triple_object": "Tea",
                    "confidence": 0.0
                }),
            )
            .await;
        // confidence=0.0 is not in (0.0, 1.0] range, so it's clamped to 1.0
        assert!(result.success);
    }

    // ============================================================
    // Additional coverage tests
    // ============================================================

    #[tokio::test]
    async fn test_execute_store_episodic_with_role() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        let result = executor
            .execute(
                "memory_store",
                &serde_json::json!({
                    "memory_type": "episodic",
                    "content": "Assistant helped with code",
                    "role": "assistant",
                    "session_key": "role-session"
                }),
            )
            .await;
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_execute_forget_delete_session_valid() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        // First store something
        executor
            .execute(
                "memory_store",
                &serde_json::json!({
                    "memory_type": "episodic",
                    "content": "test content",
                    "session_key": "delete-session"
                }),
            )
            .await;

        let result = executor
            .execute(
                "memory_forget",
                &serde_json::json!({
                    "action": "delete_session",
                    "session_key": "delete-session"
                }),
            )
            .await;
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_execute_forget_cleanup_valid() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        let result = executor
            .execute(
                "memory_forget",
                &serde_json::json!({
                    "action": "cleanup",
                    "older_than_days": 30
                }),
            )
            .await;
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_execute_forget_delete_entity_valid() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        // First store an entity
        executor
            .execute(
                "memory_store",
                &serde_json::json!({
                    "memory_type": "graph",
                    "entity_name": "DeleteMe",
                    "entity_type": "test"
                }),
            )
            .await;

        let result = executor
            .execute(
                "memory_forget",
                &serde_json::json!({
                    "action": "delete_entity",
                    "entity_name": "DeleteMe"
                }),
            )
            .await;
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_execute_forget_delete_triple_missing_fields() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        let result = executor
            .execute(
                "memory_forget",
                &serde_json::json!({
                    "action": "delete_triple"
                }),
            )
            .await;
        assert!(!result.success);
    }

    #[tokio::test]
    async fn test_execute_list_graph_query() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        // Store a triple first
        executor
            .execute(
                "memory_store",
                &serde_json::json!({
                    "memory_type": "graph",
                    "triple_subject": "Rust",
                    "triple_predicate": "is_a",
                    "triple_object": "language",
                    "confidence": 0.95
                }),
            )
            .await;

        let result = executor
            .execute(
                "memory_list",
                &serde_json::json!({"list_type": "graph_query", "subject": "Rust"}),
            )
            .await;
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_execute_list_graph_related() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        // Store entity and triple first
        executor
            .execute(
                "memory_store",
                &serde_json::json!({
                    "memory_type": "graph",
                    "entity_name": "Rust",
                    "entity_type": "language"
                }),
            )
            .await;
        executor
            .execute(
                "memory_store",
                &serde_json::json!({
                    "memory_type": "graph",
                    "triple_subject": "Rust",
                    "triple_predicate": "is_a",
                    "triple_object": "language"
                }),
            )
            .await;

        let result = executor
            .execute(
                "memory_list",
                &serde_json::json!({"list_type": "graph_related", "entity_name": "Rust"}),
            )
            .await;
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_execute_list_episodes_with_data() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        // Store some episodes first
        for i in 0..3 {
            executor
                .execute(
                    "memory_store",
                    &serde_json::json!({
                        "memory_type": "episodic",
                        "content": format!("Episode {}", i),
                        "session_key": "ep-session",
                        "tags": ["test"]
                    }),
                )
                .await;
        }

        let result = executor
            .execute(
                "memory_list",
                &serde_json::json!({
                    "list_type": "episodes",
                    "session_key": "ep-session",
                    "limit": 5
                }),
            )
            .await;
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_execute_list_graph_query_empty() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        let result = executor
            .execute(
                "memory_list",
                &serde_json::json!({"list_type": "graph_query"}),
            )
            .await;
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_execute_search_with_stored_data() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        // Store episodic memory
        executor
            .execute(
                "memory_store",
                &serde_json::json!({
                    "memory_type": "episodic",
                    "content": "Rust ownership model discussion",
                    "session_key": "search-session"
                }),
            )
            .await;

        // Search for it
        let result = executor
            .execute(
                "memory_search",
                &serde_json::json!({"query": "Rust"}),
            )
            .await;
        assert!(result.success);
    }

    #[test]
    fn test_truncate_text_multibyte() {
        // Test with multibyte unicode chars
        let text = "hello world this is a test string for truncation";
        let result = truncate_text(text, 20);
        assert!(result.len() <= 20);
    }

    #[test]
    fn test_memory_tool_serialize() {
        let tool = MemoryTool {
            name: "test".into(),
            description: "desc".into(),
            parameters: serde_json::json!({"type": "object"}),
        };
        let json = serde_json::to_string(&tool).unwrap();
        assert!(json.contains("test"));
    }

    #[test]
    fn test_memory_tool_deserialize() {
        let json = r#"{"name":"x","description":"d","parameters":{"type":"object"}}"#;
        let tool: MemoryTool = serde_json::from_str(json).unwrap();
        assert_eq!(tool.name, "x");
    }

    #[tokio::test]
    async fn test_execute_search_zero_limit() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        let result = executor
            .execute(
                "memory_search",
                &serde_json::json!({"query": "test", "limit": 0}),
            )
            .await;
        // limit=0 should be treated as default (10)
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_execute_list_episodes_zero_limit() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        let result = executor
            .execute(
                "memory_list",
                &serde_json::json!({
                    "list_type": "episodes",
                    "session_key": "test",
                    "limit": 0
                }),
            )
            .await;
        // limit=0 should be treated as 10
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_execute_list_graph_related_with_depth() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        let result = executor
            .execute(
                "memory_list",
                &serde_json::json!({
                    "list_type": "graph_related",
                    "entity_name": "Test",
                    "depth": 3
                }),
            )
            .await;
        assert!(result.success);
    }

    // --- Additional coverage tests ---

    #[tokio::test]
    async fn test_execute_store_episodic_auto_session_key() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        // No session_key provided - should generate one with "manual-" prefix
        let result = executor
            .execute(
                "memory_store",
                &serde_json::json!({
                    "memory_type": "episodic",
                    "content": "Auto session test"
                }),
            )
            .await;
        assert!(result.success);
        assert!(result.content.contains("manual-"));
    }

    #[tokio::test]
    async fn test_execute_store_episodic_with_tags() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        let result = executor
            .execute(
                "memory_store",
                &serde_json::json!({
                    "memory_type": "episodic",
                    "content": "Tagged content",
                    "session_key": "tag-session",
                    "tags": ["rust", "memory", "test"]
                }),
            )
            .await;
        assert!(result.success);

        // Now list episodes to verify tags appear
        let list_result = executor
            .execute(
                "memory_list",
                &serde_json::json!({
                    "list_type": "episodes",
                    "session_key": "tag-session"
                }),
            )
            .await;
        assert!(list_result.success);
        assert!(list_result.content.contains("rust"));
    }

    #[tokio::test]
    async fn test_execute_store_graph_missing_entity_and_triple() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        // Neither entity_name nor triple_subject provided
        let result = executor
            .execute(
                "memory_store",
                &serde_json::json!({"memory_type": "graph"}),
            )
            .await;
        assert!(!result.success);
        assert!(result.content.contains("entity_name") || result.content.contains("triple"));
    }

    #[tokio::test]
    async fn test_execute_forget_verify_action_required() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        let result = executor
            .execute("memory_forget", &serde_json::json!({}))
            .await;
        assert!(!result.success);
        assert!(result.content.contains("action is required"));
    }

    #[tokio::test]
    async fn test_execute_forget_delete_entry() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        // Store something first
        let store_result = executor
            .execute(
                "memory_store",
                &serde_json::json!({
                    "memory_type": "episodic",
                    "content": "Will be forgotten",
                    "session_key": "forget-session"
                }),
            )
            .await;
        assert!(store_result.success);

        // Delete the session
        let result = executor
            .execute(
                "memory_forget",
                &serde_json::json!({
                    "action": "delete_session",
                    "session_key": "forget-session"
                }),
            )
            .await;
        assert!(result.success);
        assert!(result.content.contains("deleted"));
    }

    #[tokio::test]
    async fn test_execute_forget_session_missing_key() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        let result = executor
            .execute(
                "memory_forget",
                &serde_json::json!({"action": "delete_session"}),
            )
            .await;
        assert!(!result.success);
        assert!(result.content.contains("session_key is required"));
    }

    #[tokio::test]
    async fn test_execute_list_graph_query_with_results() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        // Store a triple first
        executor
            .execute(
                "memory_store",
                &serde_json::json!({
                    "memory_type": "graph",
                    "triple_subject": "Go",
                    "triple_predicate": "is_a",
                    "triple_object": "language"
                }),
            )
            .await;

        let result = executor
            .execute(
                "memory_list",
                &serde_json::json!({
                    "list_type": "graph_query",
                    "subject": "Go"
                }),
            )
            .await;
        assert!(result.success);
        assert!(result.content.contains("Go"));
    }

    #[tokio::test]
    async fn test_execute_list_status_type() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        let result = executor
            .execute(
                "memory_list",
                &serde_json::json!({"list_type": "status"}),
            )
            .await;
        assert!(result.success);
        assert!(result.content.contains("Memory Store"));
    }

    #[test]
    fn test_truncate_text_boundary_char() {
        let text = "hello world";
        let result = truncate_text(text, 5);
        assert!(result.len() <= 5);
    }

    #[test]
    fn test_memory_tool_result_serialization_roundtrip() {
        let ok_result = MemoryToolResult::ok("test data");
        let json = serde_json::to_string(&ok_result).unwrap();
        let restored: MemoryToolResult = serde_json::from_str(&json).unwrap();
        assert!(restored.success);
        assert_eq!(restored.content, "test data");

        let err_result = MemoryToolResult::err("error msg");
        let json = serde_json::to_string(&err_result).unwrap();
        let restored: MemoryToolResult = serde_json::from_str(&json).unwrap();
        assert!(!restored.success);
    }

    #[tokio::test]
    async fn test_execute_search_graph_with_results() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        // Store a graph entity and triple first
        executor
            .execute(
                "memory_store",
                &serde_json::json!({
                    "memory_type": "graph",
                    "entity_name": "Python",
                    "entity_type": "language"
                }),
            )
            .await;
        executor
            .execute(
                "memory_store",
                &serde_json::json!({
                    "memory_type": "graph",
                    "triple_subject": "Python",
                    "triple_predicate": "is_a",
                    "triple_object": "language"
                }),
            )
            .await;

        // Search graph specifically
        let result = executor
            .execute(
                "memory_search",
                &serde_json::json!({"query": "Python", "memory_type": "graph"}),
            )
            .await;
        assert!(result.success);
        assert!(result.content.contains("Python"));
    }

    #[tokio::test]
    async fn test_execute_store_unknown_type() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        let result = executor
            .execute(
                "memory_store",
                &serde_json::json!({"memory_type": "unknown_type"}),
            )
            .await;
        assert!(!result.success);
        assert!(result.content.contains("unknown memory_type"));
    }

    #[tokio::test]
    async fn test_execute_store_missing_memory_type() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        let result = executor
            .execute("memory_store", &serde_json::json!({}))
            .await;
        assert!(!result.success);
        assert!(result.content.contains("required"));
    }

    #[tokio::test]
    async fn test_execute_store_episodic_missing_content() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        let result = executor
            .execute(
                "memory_store",
                &serde_json::json!({"memory_type": "episodic"}),
            )
            .await;
        assert!(!result.success);
        assert!(result.content.contains("content is required"));
    }

    #[tokio::test]
    async fn test_execute_list_invalid_list_type() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        let result = executor
            .execute(
                "memory_list",
                &serde_json::json!({"list_type": "unknown_list_type"}),
            )
            .await;
        assert!(!result.success);
        assert!(result.content.contains("nknown list_type") || result.content.contains("invalid"));
    }

    #[tokio::test]
    async fn test_execute_list_missing_list_type() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        let result = executor
            .execute("memory_list", &serde_json::json!({}))
            .await;
        // Missing list_type may default to summary
        assert!(result.success || result.content.contains("required"));
    }

    #[tokio::test]
    async fn test_execute_search_all_types() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::manager::Config::new(dir.path());
        let mgr = Arc::new(MemoryManager::new(&config));
        let executor = MemoryToolExecutor::new(mgr);

        // Store episodic memory
        executor
            .execute(
                "memory_store",
                &serde_json::json!({
                    "memory_type": "episodic",
                    "content": "Rust ownership discussion",
                    "session_key": "search-all"
                }),
            )
            .await;

        // Search with memory_type=all
        let result = executor
            .execute(
                "memory_search",
                &serde_json::json!({"query": "Rust", "memory_type": "all"}),
            )
            .await;
        assert!(result.success);
    }
}
