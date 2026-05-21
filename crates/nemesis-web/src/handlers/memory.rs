//! Memory handler — status/documents/document.get/document.save/vector.status/vector.search.

use crate::handlers::{read_workspace_file, require_workspace, resolve_path, write_workspace_file};
use crate::ws_router::{ModuleHandler, RequestContext};
use std::path::PathBuf;

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
        match cmd {
            "status" => self.status(workspace),
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
            "vector.status" => self.vector_status(workspace),
            "vector.search" => {
                let data = data.ok_or("missing data")?;
                let query = crate::handlers::get_str(&data, "query")?;
                self.vector_search(&query)
            }
            _ => Err(format!("unknown command: memory.{}", cmd)),
        }
    }
}

impl MemoryHandler {
    fn status(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let memory_dir = PathBuf::from(workspace).join("memory");
        let doc_count = if memory_dir.exists() {
            count_files_recursive(&memory_dir)
        } else {
            0
        };

        // Read enhanced memory config
        let em_config_path = PathBuf::from(workspace).join("config/config.enhanced_memory.json");
        let vector_enabled = if em_config_path.exists() {
            std::fs::read_to_string(&em_config_path)
                .ok()
                .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
                .and_then(|v| v.get("enabled").and_then(|e| e.as_bool()))
                .unwrap_or(false)
        } else {
            false
        };

        Ok(Some(serde_json::json!({
            "document_memory": {
                "enabled": true,
                "document_count": doc_count,
                "directory_exists": memory_dir.exists(),
            },
            "vector_memory": {
                "enabled": vector_enabled,
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
        let em_config_path = PathBuf::from(workspace).join("config/config.enhanced_memory.json");
        if !em_config_path.exists() {
            return Ok(Some(serde_json::json!({ "enabled": false })));
        }
        let content = std::fs::read_to_string(&em_config_path)
            .map_err(|e| format!("failed to read config: {}", e))?;
        let config: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| format!("invalid config: {}", e))?;
        Ok(Some(config))
    }

    fn vector_search(&self, query: &str) -> Result<Option<serde_json::Value>, String> {
        // Stub — requires embedding plugin support
        Ok(Some(serde_json::json!({
            "query": query,
            "results": [],
            "message": "Vector search requires embedding plugin"
        })))
    }
}

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
