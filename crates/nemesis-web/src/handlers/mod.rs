//! WebSocket API handlers for all dashboard modules.
//!
//! Each module handler implements [`ModuleHandler`](crate::ws_router::ModuleHandler)
//! and is registered via [`register_all`]. Handlers are pure business logic and
//! transport-agnostic — they read/write configuration files and workspace data.

pub mod agent;
pub mod channels;
pub mod cluster;
pub mod config;
pub mod forge;
pub mod identity;
pub mod logs;
pub mod mcp;
pub mod memory;
pub mod models;
pub mod scanner;
pub mod security;
pub mod skills;
pub mod system;
pub mod tasks;
pub mod tools;
pub mod voice;
pub mod persona;
pub mod workflow;

use std::path::PathBuf;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Register all module handlers with the given router.
pub fn register_all(router: &mut crate::ws_router::WsRouter) {
    router.register(Arc::new(system::SystemHandler));
    router.register(Arc::new(config::ConfigHandler::new()));
    router.register(Arc::new(models::ModelsHandler::new()));
    router.register(Arc::new(channels::ChannelsHandler::new()));
    router.register(Arc::new(identity::IdentityHandler));
    router.register(Arc::new(tools::ToolsHandler));
    router.register(Arc::new(scanner::ScannerHandler::new()));
    router.register(Arc::new(memory::MemoryHandler));
    router.register(Arc::new(skills::SkillsHandler::new()));
    router.register(Arc::new(mcp::McpHandler::new()));
    router.register(Arc::new(security::SecurityHandler::new()));
    router.register(Arc::new(forge::ForgeHandler::new()));
    router.register(Arc::new(tasks::TasksHandler));
    router.register(Arc::new(cluster::ClusterHandler::new()));
    router.register(Arc::new(logs::LogsHandler));
    router.register(Arc::new(agent::AgentHandler));
    router.register(Arc::new(voice::VoiceHandler::new()));
    router.register(Arc::new(persona::PersonaHandler::new()));
    router.register(Arc::new(workflow::WorkflowHandler));
}

// ---------------------------------------------------------------------------
// Shared utilities
// ---------------------------------------------------------------------------

/// Mask a sensitive string, showing only the first 4 and last 4 characters.
pub fn mask_sensitive(value: &str) -> String {
    if value.len() <= 8 {
        return "****".to_string();
    }
    format!("{}****{}", &value[..4], &value[value.len() - 4..])
}

/// Check whether a field name is considered sensitive and should be masked.
pub fn is_sensitive_field(field_name: &str) -> bool {
    matches!(
        field_name.to_lowercase().as_str(),
        "api_key"
            | "token"
            | "secret"
            | "password"
            | "auth_token"
            | "app_secret"
            | "encrypt_key"
            | "access_token"
            | "bot_token"
            | "app_token"
            | "client_secret"
    )
}

/// Resolve a path relative to the workspace, preventing path traversal.
pub fn resolve_path(workspace: &str, relative: &str) -> Result<PathBuf, String> {
    // Reject paths that look absolute (drive letter or leading slash)
    let rel_path = PathBuf::from(relative);
    if rel_path.is_absolute() || relative.starts_with('/') || relative.starts_with('\\') {
        return Err("absolute paths not allowed".to_string());
    }

    let base = PathBuf::from(workspace);
    let resolved = base.join(relative);

    // Quick string check: resolved should start with base (catches drive-root escapes on Windows)
    let base_str = base.to_string_lossy();
    let resolved_str = resolved.to_string_lossy();
    if !resolved_str.starts_with(base_str.as_ref()) {
        return Err("path traversal denied".to_string());
    }

    // Canonicalize both paths for accurate comparison (handles .., symlinks, etc.)
    let canonical_base = base.canonicalize().unwrap_or_else(|_| base.clone());
    let canonical_resolved = match resolved.canonicalize() {
        Ok(p) => p,
        Err(_) => {
            // File doesn't exist yet — the string check above already caught most traversal
            if relative.contains("..") {
                return Err("path traversal denied".to_string());
            }
            return Ok(resolved);
        }
    };

    if !canonical_resolved.starts_with(&canonical_base) {
        return Err("path traversal denied".to_string());
    }
    Ok(resolved)
}

/// Extract a required string field from a JSON value.
pub fn get_str(data: &serde_json::Value, field: &str) -> Result<String, String> {
    data.get(field)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("missing field: {}", field))
}

/// Extract an optional string field from a JSON value.
pub fn get_opt_str(data: &serde_json::Value, field: &str) -> Option<String> {
    data.get(field).and_then(|v| v.as_str()).map(|s| s.to_string())
}

/// Read a text file from the workspace.
pub fn read_workspace_file(workspace: &str, relative: &str) -> Result<String, String> {
    let path = resolve_path(workspace, relative)?;
    std::fs::read_to_string(&path).map_err(|e| format!("failed to read {}: {}", relative, e))
}

/// Write a text file to the workspace (atomic write via tmp + rename).
pub fn write_workspace_file(workspace: &str, relative: &str, content: &str) -> Result<(), String> {
    let path = resolve_path(workspace, relative)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("failed to create dir: {}", e))?;
    }
    let tmp_path = path.with_extension("tmp");
    std::fs::write(&tmp_path, content).map_err(|e| format!("failed to write tmp: {}", e))?;
    if let Err(e) = std::fs::rename(&tmp_path, &path) {
        let _ = std::fs::remove_file(&tmp_path);
        std::fs::write(&path, content).map_err(|e| format!("failed to write file: {}", e))?;
        tracing::warn!(error = %e, "[WebServer] Atomic rename failed, fell back to direct write");
    }
    Ok(())
}

/// List files in a workspace directory, returning relative paths.
pub fn list_workspace_dir(workspace: &str, relative: &str) -> Result<Vec<String>, String> {
    let dir = resolve_path(workspace, relative)?;
    if !dir.exists() {
        return Ok(vec![]);
    }
    let mut entries = Vec::new();
    let read_dir = std::fs::read_dir(&dir).map_err(|e| format!("failed to read dir: {}", e))?;
    for entry in read_dir {
        let entry = entry.map_err(|e| format!("failed to read entry: {}", e))?;
        if let Some(name) = entry.file_name().to_str() {
            entries.push(name.to_string());
        }
    }
    entries.sort();
    Ok(entries)
}

/// Get workspace path from context or return error.
pub fn require_workspace(ctx: &crate::ws_router::RequestContext) -> Result<&str, String> {
    ctx.workspace
        .as_deref()
        .ok_or_else(|| "workspace not configured".to_string())
}

/// Get home directory from context or return error.
pub fn require_home(ctx: &crate::ws_router::RequestContext) -> Result<&str, String> {
    ctx.home
        .as_deref()
        .ok_or_else(|| "home not configured".to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
