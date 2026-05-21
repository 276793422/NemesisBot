//! Logs handler — requests/request_detail/security.

use crate::handlers::require_workspace;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::path::PathBuf;

pub struct LogsHandler;

#[async_trait::async_trait]
impl ModuleHandler for LogsHandler {
    fn module_name(&self) -> &str {
        "logs"
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let workspace = require_workspace(ctx)?;
        match cmd {
            "requests" => {
                let limit = data
                    .as_ref()
                    .and_then(|d| d.get("limit"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(50) as usize;
                let offset = data
                    .as_ref()
                    .and_then(|d| d.get("offset"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize;
                self.requests(workspace, limit, offset)
            }
            "request_detail" => {
                let data = data.ok_or("missing data")?;
                let session = crate::handlers::get_str(&data, "session")?;
                self.request_detail(workspace, &session)
            }
            "security" => {
                let limit = data
                    .as_ref()
                    .and_then(|d| d.get("limit"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(50) as usize;
                let offset = data
                    .as_ref()
                    .and_then(|d| d.get("offset"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize;
                let risk_level = data
                    .as_ref()
                    .and_then(|d| d.get("risk_level"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                self.security(workspace, limit, offset, risk_level.as_deref())
            }
            _ => Err(format!("unknown command: logs.{}", cmd)),
        }
    }
}

fn request_log_dir(workspace: &str) -> PathBuf {
    PathBuf::from(workspace).join("logs/request_logs")
}

fn security_log_dir(workspace: &str) -> PathBuf {
    PathBuf::from(workspace).join("logs/security_logs")
}

/// Read JSONL entries from a log directory, sorted by timestamp descending.
fn read_jsonl_entries(
    dir: &std::path::Path,
    limit: usize,
    offset: usize,
    filter: Option<&str>,
    filter_field: &str,
) -> Result<(Vec<serde_json::Value>, usize), String> {
    if !dir.exists() {
        return Ok((vec![], 0));
    }

    let mut entries = Vec::new();
    let read_dir = std::fs::read_dir(dir).map_err(|e| format!("failed to read log dir: {}", e))?;

    for entry in read_dir {
        let entry = entry.map_err(|e| format!("failed to read entry: {}", e))?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            if let Ok(content) = std::fs::read_to_string(&path) {
                for line in content.lines() {
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
                        if let Some(f) = filter {
                            let field_val = val
                                .get(filter_field)
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            if !field_val.eq_ignore_ascii_case(f) {
                                continue;
                            }
                        }
                        entries.push(val);
                    }
                }
            }
        }
    }

    // Sort by timestamp descending
    entries.sort_by(|a, b| {
        let ts_a = a.get("timestamp").and_then(|v| v.as_str()).unwrap_or("");
        let ts_b = b.get("timestamp").and_then(|v| v.as_str()).unwrap_or("");
        ts_b.cmp(ts_a)
    });

    let total = entries.len();
    let page: Vec<_> = entries.into_iter().skip(offset).take(limit).collect();
    Ok((page, total))
}

impl LogsHandler {
    fn requests(
        &self,
        workspace: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Option<serde_json::Value>, String> {
        let dir = request_log_dir(workspace);
        let (entries, total) = read_jsonl_entries(&dir, limit, offset, None, "")?;
        Ok(Some(serde_json::json!({
            "entries": entries,
            "total": total,
            "limit": limit,
            "offset": offset,
        })))
    }

    fn request_detail(
        &self,
        workspace: &str,
        session: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        let dir = request_log_dir(workspace);
        if !dir.exists() {
            return Err(format!("session '{}' not found", session));
        }

        // Search for the session in log files
        let read_dir = std::fs::read_dir(&dir).map_err(|e| format!("failed to read log dir: {}", e))?;
        for entry in read_dir {
            let entry = entry.map_err(|e| format!("failed to read entry: {}", e))?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    for line in content.lines() {
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
                            let sid = val
                                .get("session_id")
                                .or_else(|| val.get("session"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            if sid == session {
                                return Ok(Some(val));
                            }
                        }
                    }
                }
            }
        }

        Err(format!("session '{}' not found", session))
    }

    fn security(
        &self,
        workspace: &str,
        limit: usize,
        offset: usize,
        risk_level: Option<&str>,
    ) -> Result<Option<serde_json::Value>, String> {
        let dir = security_log_dir(workspace);
        if !dir.exists() {
            return Ok(Some(serde_json::json!({
                "entries": [],
                "total": 0,
                "limit": limit,
                "offset": offset,
            })));
        }

        let mut entries = Vec::new();
        let read_dir = std::fs::read_dir(&dir)
            .map_err(|e| format!("failed to read security log dir: {}", e))?;

        for entry in read_dir {
            let entry = entry.map_err(|e| format!("failed to read entry: {}", e))?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    for line in content.lines() {
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
                            if let Some(filter) = risk_level {
                                let entry_level = super::security::extract_risk_level(&val);
                                if !entry_level.eq_ignore_ascii_case(filter) {
                                    continue;
                                }
                            }
                            entries.push(super::security::flatten_audit_entry(&val));
                        }
                    }
                }
            }
        }

        // Sort by timestamp descending
        entries.sort_by(|a, b| {
            let ts_a = a.get("timestamp").and_then(|v| v.as_str()).unwrap_or("");
            let ts_b = b.get("timestamp").and_then(|v| v.as_str()).unwrap_or("");
            ts_b.cmp(ts_a)
        });

        let total = entries.len();
        let page: Vec<_> = entries.into_iter().skip(offset).take(limit).collect();
        Ok(Some(serde_json::json!({
            "entries": page,
            "total": total,
            "limit": limit,
            "offset": offset,
        })))
    }
}
