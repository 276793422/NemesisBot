//! Logs handler — 9 commands for the Logs Dashboard.
//!
//! Reads from four on-disk data sources:
//! - `request_logs/{ts}_{rand}/` (markdown + raw.json files per LLM call)
//! - `cluster_logs/{device}/{ts_ms}_{task_id}/` (same format, per cluster task)
//! - `security_logs/security_audit_YYYY-MM-DD.log` (pipe-delimited audit events)
//! - `security_logs/audit_chain.jsonl` (+ rotations) (integrity chain, optional)
//! - EpisodicStore (JSONL conversation episodes, via MemoryManager)
//!
//! All commands are pure read-only IO. No mutation of state.

use crate::handlers::require_workspace;
use crate::ws_router::{ModuleHandler, RequestContext};
use chrono::NaiveDateTime;
use nemesis_security::integrity::AuditEvent;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

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
                let limit = opt_u64(&data, "limit", 50);
                let offset = opt_u64(&data, "offset", 0);
                self.requests(workspace, limit, offset)
            }
            "request_detail" => {
                let data = data.ok_or("missing data")?;
                let id = crate::handlers::get_str(&data, "id")
                    .or_else(|_| crate::handlers::get_str(&data, "session"))?;
                self.request_detail(workspace, &id)
            }
            "cluster_task_list" => {
                let limit = opt_u64(&data, "limit", 50);
                let offset = opt_u64(&data, "offset", 0);
                let device_filter = data
                    .as_ref()
                    .and_then(|d| d.get("device_id"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                self.cluster_task_list(ctx, workspace, limit, offset, device_filter)
            }
            "cluster_task_detail" => {
                let data = data.ok_or("missing data")?;
                let task_id = crate::handlers::get_str(&data, "task_id")?;
                let perspective = data
                    .get("perspective")
                    .and_then(|v| v.as_str())
                    .unwrap_or("self");
                self.cluster_task_detail(ctx, workspace, &task_id, perspective)
            }
            "security" => {
                let limit = opt_u64(&data, "limit", 50);
                let offset = opt_u64(&data, "offset", 0);
                let risk_level = data
                    .as_ref()
                    .and_then(|d| d.get("risk_level"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                self.security(workspace, limit, offset, risk_level.as_deref())
            }
            "chain_list" => {
                let limit = opt_u64(&data, "limit", 100);
                let offset = opt_u64(&data, "offset", 0);
                self.chain_list(workspace, limit, offset)
            }
            "chain_verify" => self.chain_verify(workspace),
            "session_list" => {
                let limit = opt_u64(&data, "limit", 50);
                let offset = opt_u64(&data, "offset", 0);
                self.session_list(ctx, workspace, limit, offset).await
            }
            "session_detail" => {
                let data = data.ok_or("missing data")?;
                let session = crate::handlers::get_str(&data, "session")?;
                self.session_detail(ctx, workspace, &session).await
            }
            _ => Err(format!("unknown command: logs.{}", cmd)),
        }
    }
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

fn opt_u64(data: &Option<serde_json::Value>, field: &str, default: usize) -> usize {
    data.as_ref()
        .and_then(|d| d.get(field))
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .unwrap_or(default)
}

fn request_log_dir(workspace: &str) -> PathBuf {
    PathBuf::from(workspace).join("logs/request_logs")
}

fn cluster_log_dir(workspace: &str) -> PathBuf {
    PathBuf::from(workspace).join("logs/cluster_logs")
}

fn security_log_dir(workspace: &str) -> PathBuf {
    PathBuf::from(workspace).join("logs/security_logs")
}

fn audit_chain_path(workspace: &str) -> PathBuf {
    security_log_dir(workspace).join("audit_chain.jsonl")
}

fn local_node_id(ctx: &RequestContext) -> Option<String> {
    ctx.state.cluster.as_ref().map(|c| c.node_id().to_string())
}

// ---------------------------------------------------------------------------
// Markdown helpers (for *.request.md / *.response.md / *.Local.md)
// ---------------------------------------------------------------------------

/// Extract a `**Key**: value` header line from markdown.
///
/// Matches lines starting with optional `- `/`* ` list marker + `**{key}**:`.
pub(crate) fn extract_md_header(content: &str, key: &str) -> Option<String> {
    let prefix = format!("**{}**", key);
    for line in content.lines() {
        let trimmed = line.trim_start();
        let after_marker = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .unwrap_or(trimmed);
        if after_marker.len() >= prefix.len() {
            let (head, rest) = after_marker.split_at(prefix.len());
            if head.eq_ignore_ascii_case(&prefix) {
                let rest = rest.trim_start_matches(':').trim_start_matches(' ').trim();
                if !rest.is_empty() {
                    return Some(rest.to_string());
                }
            }
        }
    }
    None
}

/// Read the content between `## {header}` and the next `#`-prefixed line.
pub(crate) fn read_md_section(content: &str, header: &str) -> Option<String> {
    let prefix = format!("## {}", header);
    let mut in_section = false;
    let mut out = String::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if in_section {
            if trimmed.starts_with('#') {
                break;
            }
            out.push_str(line);
            out.push('\n');
        } else if trimmed.starts_with(&prefix) {
            in_section = true;
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Pull the first user message under `## Message` (truncated to 200 chars).
pub(crate) fn extract_md_first_message(content: &str) -> String {
    let body = read_md_section(content, "Message").unwrap_or_default();
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let limit = 200.min(trimmed.chars().count());
    let end = trimmed
        .char_indices()
        .nth(limit)
        .map(|(i, _)| i)
        .unwrap_or(trimmed.len());
    trimmed[..end].to_string()
}

// ---------------------------------------------------------------------------
// Filename helpers
// ---------------------------------------------------------------------------

/// Parse a request_logs dir name `{YYYY-MM-DD_HH-MM-SS}_{rand}` into (timestamp, suffix).
pub(crate) fn parse_request_dir_name(name: &str) -> Option<(String, String)> {
    for split_pos in name.rmatch_indices('_').map(|(i, _)| i) {
        let candidate = &name[..split_pos];
        if NaiveDateTime::parse_from_str(candidate, "%Y-%m-%d_%H-%M-%S").is_ok() {
            return Some((candidate.to_string(), name[split_pos + 1..].to_string()));
        }
    }
    None
}

/// Parse a cluster_logs dir name `{YYYY-MM-DD_HH-MM-SS[-MS]}_{task_id}`.
pub(crate) fn parse_cluster_dir_name(name: &str) -> Option<(String, String)> {
    for split_pos in name.rmatch_indices('_').map(|(i, _)| i) {
        let candidate = &name[..split_pos];
        let ok_ms = NaiveDateTime::parse_from_str(candidate, "%Y-%m-%d_%H-%M-%S-%3f").is_ok();
        let ok_plain = NaiveDateTime::parse_from_str(candidate, "%Y-%m-%d_%H-%M-%S").is_ok();
        if ok_ms || ok_plain {
            return Some((candidate.to_string(), name[split_pos + 1..].to_string()));
        }
    }
    None
}

/// List subdirs of `dir`, sorted by name descending.
pub(crate) fn list_subdirs_sorted_desc(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut subs: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.path())
        .collect();
    subs.sort_by(|a, b| {
        let an = a.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let bn = b.file_name().and_then(|n| n.to_str()).unwrap_or("");
        bn.cmp(an)
    });
    subs
}

/// Sort files in a dir by their filename prefix numeric value (00, 01, ...).
fn sorted_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<(usize, String, PathBuf)> = entries
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let prefix_str = name.split('.').next().unwrap_or("0");
            let prefix_num: usize = prefix_str.parse().unwrap_or(usize::MAX);
            Some((prefix_num, name, e.path()))
        })
        .collect();
    files.sort_by_key(|(n, _, _)| *n);
    files.into_iter().map(|(_, _, p)| p).collect()
}

/// Strip the `NN.` numeric prefix from a filename. Returns the rest (e.g. "AI.Request.raw.json").
fn file_type_name(filename: &str) -> &str {
    match filename.split_once('.') {
        Some((_, rest)) => rest,
        None => filename,
    }
}

// ---------------------------------------------------------------------------
// Audit chain segment collection + hashing
// ---------------------------------------------------------------------------

fn collect_audit_segments(main_path: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if main_path.exists() {
        files.push(main_path.to_path_buf());
    }
    let Some(parent) = main_path.parent() else {
        return files;
    };
    let stem = main_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("audit_chain");
    let Ok(entries) = std::fs::read_dir(parent) else {
        return files;
    };
    let mut segs: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.starts_with(stem) && name.contains("_seg")
        })
        .map(|e| e.path())
        .collect();
    segs.sort();
    files.extend(segs);
    files
}

fn read_all_audit_events(main_path: &Path) -> Vec<AuditEvent> {
    let mut events = Vec::new();
    for path in collect_audit_segments(main_path) {
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(ev) = serde_json::from_str::<AuditEvent>(line) {
                events.push(ev);
            }
        }
    }
    events
}

/// SHA-256 hash using the same field order as `AuditChain::append_with_sign`.
fn compute_audit_hash(ev: &AuditEvent) -> String {
    let mut hasher = Sha256::new();
    hasher.update(ev.prev_hash.as_bytes());
    hasher.update(ev.timestamp.as_bytes());
    hasher.update(ev.operation.as_bytes());
    hasher.update(ev.tool_name.as_bytes());
    hasher.update(ev.user.as_bytes());
    hasher.update(ev.target.as_bytes());
    hasher.update(ev.decision.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn sanitize_session_key(key: &str) -> String {
    key.replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_")
}

// ---------------------------------------------------------------------------
// Security audit log parsing (pipe-delimited text format)
// ---------------------------------------------------------------------------

/// Parse a `security_audit_YYYY-MM-DD.log` file into AuditEntry JSON objects.
///
/// File format (first 3 lines are header `#` comments, ignored):
/// ```text
/// TIMESTAMP | EVENT_ID | DECISION | OPERATION | USER | SOURCE | TARGET | DANGER | REASON | POLICY
/// ```
fn parse_security_audit_file(path: &Path) -> Vec<serde_json::Value> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = trimmed.split(" | ").collect();
        if parts.len() < 10 {
            continue;
        }
        let timestamp = parts[0].trim().to_string();
        let event_id = parts[1].trim().to_string();
        let decision_raw = parts[2].trim().to_string();
        let operation = parts[3].trim().to_string();
        let user = parts[4].trim().to_string();
        let _source = parts[5].trim();
        let target = parts[6].trim().to_string();
        let danger = parts[7].trim().to_string();
        let reason = parts[8].trim().to_string();
        let policy = parts[9].trim().to_string();

        let result = if decision_raw.to_lowercase().starts_with("allow")
            || decision_raw.eq_ignore_ascii_case("approved")
        {
            "allow"
        } else {
            "deny"
        };

        out.push(serde_json::json!({
            "id": event_id,
            "timestamp": timestamp,
            "operation": operation,
            "risk_level": danger,
            "target": target,
            "result": result,
            "decision": decision_raw,
            "user": user,
            "reason": reason,
            "policy": policy,
            "raw": {
                "timestamp": timestamp,
                "event_id": event_id,
                "decision": decision_raw,
                "operation": operation,
                "user": user,
                "source": _source,
                "target": target,
                "danger": danger,
                "reason": reason,
                "policy": policy,
            },
        }));
    }
    out
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

impl LogsHandler {
    fn requests(
        &self,
        workspace: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Option<serde_json::Value>, String> {
        let dir = request_log_dir(workspace);
        if !dir.exists() {
            return Ok(Some(serde_json::json!({
                "entries": [],
                "total": 0,
                "limit": limit,
                "offset": offset,
            })));
        }

        let subs = list_subdirs_sorted_desc(&dir);
        let mut entries: Vec<serde_json::Value> = Vec::new();
        for sub in &subs {
            let name = match sub.file_name().and_then(|n| n.to_str()) {
                Some(n) => n,
                None => continue,
            };
            let Some((ts, suffix)) = parse_request_dir_name(name) else {
                continue;
            };
            let entry = build_request_entry(sub, name, &ts, &suffix);
            entries.push(entry);
        }

        let total = entries.len();
        let page: Vec<_> = entries.into_iter().skip(offset).take(limit).collect();
        Ok(Some(serde_json::json!({
            "entries": page,
            "total": total,
            "limit": limit,
            "offset": offset,
        })))
    }

    fn request_detail(
        &self,
        workspace: &str,
        id: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        let dir = request_log_dir(workspace).join(id);
        if !dir.exists() {
            return Err(format!("request '{}' not found", id));
        }

        let name = id.to_string();
        let (ts, suffix) = parse_request_dir_name(&name).unwrap_or((String::new(), String::new()));
        let mut entry = build_request_entry(&dir, &name, &ts, &suffix);

        let iterations = parse_request_iterations(&dir);
        entry.as_object_mut().map(|m| m.insert("iterations".into(), iterations));
        Ok(Some(entry))
    }

    fn cluster_task_list(
        &self,
        ctx: &RequestContext,
        workspace: &str,
        limit: usize,
        offset: usize,
        device_filter: Option<String>,
    ) -> Result<Option<serde_json::Value>, String> {
        let root = cluster_log_dir(workspace);
        if !root.exists() {
            return Ok(Some(serde_json::json!({
                "entries": [],
                "total": 0,
                "limit": limit,
                "offset": offset,
            })));
        }

        let local = local_node_id(ctx);
        let device_dirs: Vec<PathBuf> = if let Some(ref dev) = device_filter {
            vec![root.join(dev)]
                .into_iter()
                .filter(|p| p.exists())
                .collect()
        } else {
            std::fs::read_dir(&root)
                .map_err(|e| format!("failed to read cluster_logs: {}", e))?
                .flatten()
                .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                .map(|e| e.path())
                .collect()
        };

        let mut rows: Vec<(String, PathBuf, String, String, String)> = Vec::new();
        for dev_dir in &device_dirs {
            let device = dev_dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            for task_dir in list_subdirs_sorted_desc(dev_dir) {
                let name = match task_dir.file_name().and_then(|n| n.to_str()) {
                    Some(n) => n.to_string(),
                    None => continue,
                };
                let Some((ts_str, task_id)) = parse_cluster_dir_name(&name) else {
                    continue;
                };
                rows.push((ts_str.clone(), task_dir, device.clone(), task_id, ts_str));
            }
        }

        rows.sort_by(|a, b| b.0.cmp(&a.0));
        let total = rows.len();

        let page = rows.into_iter().skip(offset).take(limit);
        let mut entries = Vec::new();
        for (_, dir, device, task_id, ts_str) in page {
            entries.push(build_cluster_task_entry(&dir, &device, &task_id, &ts_str, &local));
        }

        Ok(Some(serde_json::json!({
            "entries": entries,
            "total": total,
            "limit": limit,
            "offset": offset,
        })))
    }

    fn cluster_task_detail(
        &self,
        ctx: &RequestContext,
        workspace: &str,
        task_id: &str,
        perspective: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        let root = cluster_log_dir(workspace);
        let local = local_node_id(ctx);
        let sanitized_target = sanitize_session_key(task_id);

        let mut candidates: Vec<(PathBuf, String)> = Vec::new();
        if root.exists() {
            for dev_entry in std::fs::read_dir(&root)
                .map_err(|e| format!("read cluster_logs: {}", e))?
                .flatten()
            {
                if !dev_entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    continue;
                }
                let dev_path = dev_entry.path();
                let Some(dev_sub) = std::fs::read_dir(&dev_path).ok() else {
                    continue;
                };
                for sub in dev_sub.flatten() {
                    let name = sub.file_name().to_string_lossy().to_string();
                    if let Some((_ts, id)) = parse_cluster_dir_name(&name) {
                        if id == sanitized_target || id == task_id {
                            let dev_name = dev_path
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("")
                                .to_string();
                            candidates.push((sub.path(), dev_name));
                        }
                    }
                }
            }
        }

        if candidates.is_empty() {
            return Err(format!("cluster task '{}' not found", task_id));
        }

        let chosen = match perspective {
            "peer" => candidates
                .iter()
                .find(|(_, dev)| Some(dev) != local.as_ref())
                .or_else(|| candidates.first())
                .cloned()
                .unwrap(),
            _ => candidates
                .iter()
                .find(|(_, dev)| Some(dev) == local.as_ref())
                .or_else(|| candidates.first())
                .cloned()
                .unwrap(),
        };

        let (dir, device) = chosen;
        let name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        let (ts_str, task_part) =
            parse_cluster_dir_name(&name).unwrap_or((String::new(), String::new()));
        let mut entry = build_cluster_task_entry(&dir, &device, &task_part, &ts_str, &local);
        let iterations = parse_request_iterations(&dir);
        entry
            .as_object_mut()
            .map(|m| m.insert("iterations".into(), iterations));
        Ok(Some(entry))
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
            let fname = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            // Only read security_audit_*.log files (pipe-delimited text).
            // Skip audit_chain* (integrity chain, exposed via logs.chain_list).
            if !fname.starts_with("security_audit_") || !fname.ends_with(".log") {
                continue;
            }
            for mut ev in parse_security_audit_file(&path) {
                if let Some(filter) = risk_level {
                    let level = ev
                        .get("risk_level")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if !level.eq_ignore_ascii_case(filter) {
                        continue;
                    }
                }
                // Attach the source file for the detail panel.
                if let Some(obj) = ev.as_object_mut() {
                    obj.insert("_source_file".into(), serde_json::Value::String(fname.clone()));
                }
                entries.push(ev);
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

    fn chain_list(
        &self,
        workspace: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Option<serde_json::Value>, String> {
        let main_path = audit_chain_path(workspace);
        let events = read_all_audit_events(&main_path);

        let mut segments: Vec<serde_json::Value> = Vec::with_capacity(events.len());
        let mut prev_hash: Option<String> = None;
        for (i, ev) in events.iter().enumerate() {
            let computed = compute_audit_hash(ev);
            let hash_match = computed == ev.hash;
            let prev_match = match prev_hash.as_ref() {
                Some(p) => p == &ev.prev_hash,
                None => true,
            };
            let prev_ok = if i == 0 {
                ev.prev_hash.chars().all(|c| c == '0') || ev.prev_hash.is_empty()
            } else {
                prev_match
            };
            let valid = hash_match && prev_ok;

            let break_reason = if !hash_match {
                Some("hash mismatch".to_string())
            } else if !prev_ok {
                Some("prev_hash mismatch".to_string())
            } else {
                None
            };

            let summary = format!("{} | {} | {}", ev.operation, ev.target, ev.decision);

            segments.push(serde_json::json!({
                "index": i,
                "timestamp": ev.timestamp,
                "hash": ev.hash,
                "prevHash": ev.prev_hash,
                "valid": valid,
                "breakReason": break_reason,
                "payloadSummary": summary,
            }));
            prev_hash = Some(ev.hash.clone());
        }

        let total = segments.len();
        let page: Vec<_> = segments.into_iter().skip(offset).take(limit).collect();
        Ok(Some(serde_json::json!({
            "segments": page,
            "total": total,
            "limit": limit,
            "offset": offset,
        })))
    }

    fn chain_verify(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let main_path = audit_chain_path(workspace);
        let events = read_all_audit_events(&main_path);
        let total = events.len() as u64;

        let valid = nemesis_security::integrity::AuditChain::verify_chain(&events);
        if valid {
            return Ok(Some(serde_json::json!({
                "valid": true,
                "total_segments": total,
                "first_broken_index": null,
                "broken_count": 0,
            })));
        }

        let mut first_broken: Option<u64> = None;
        let mut broken_count = 0u64;
        for (i, ev) in events.iter().enumerate() {
            let computed = compute_audit_hash(ev);
            let hash_ok = computed == ev.hash;
            let prev_ok = if i == 0 {
                ev.prev_hash.chars().all(|c| c == '0') || ev.prev_hash.is_empty()
            } else {
                events[i - 1].hash == ev.prev_hash
            };
            if !hash_ok || !prev_ok {
                broken_count += 1;
                if first_broken.is_none() {
                    first_broken = Some(i as u64);
                }
            }
        }

        Ok(Some(serde_json::json!({
            "valid": false,
            "total_segments": total,
            "first_broken_index": first_broken,
            "broken_count": broken_count,
        })))
    }

    async fn session_list(
        &self,
        ctx: &RequestContext,
        _workspace: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Option<serde_json::Value>, String> {
        let Some(ref mgr) = ctx.state.memory_manager else {
            return Ok(Some(serde_json::json!({
                "sessions": [],
                "total": 0,
                "limit": limit,
                "offset": offset,
            })));
        };

        let store = mgr.get_episodic_store();
        let keys = store
            .list_sessions()
            .await
            .map_err(|e| format!("list_sessions failed: {}", e))?;

        let mut sessions: Vec<serde_json::Value> = Vec::new();
        for key in &keys {
            let episodes = match store.get_session(key).await {
                Ok(eps) => eps,
                Err(_) => continue,
            };
            if episodes.is_empty() {
                continue;
            }
            let first = episodes.first().unwrap();
            let last = episodes.last().unwrap();
            let channel = key.split([':', '_']).next().unwrap_or(key).to_string();

            let first_message = first.content.chars().take(100).collect::<String>();

            let model = first
                .metadata
                .get("model")
                .cloned()
                .unwrap_or_default();

            let trigger_cluster = episodes
                .iter()
                .any(|e| e.tags.iter().any(|t| t == "cluster"));

            sessions.push(serde_json::json!({
                "id": key,
                "channel": channel,
                "startTime": first.timestamp.to_rfc3339(),
                "lastTime": last.timestamp.to_rfc3339(),
                "messageCount": episodes.len(),
                "model": model,
                "firstMessage": first_message,
                "triggerCluster": trigger_cluster,
                "messages": [],
            }));
        }

        sessions.sort_by(|a, b| {
            let la = a.get("lastTime").and_then(|v| v.as_str()).unwrap_or("");
            let lb = b.get("lastTime").and_then(|v| v.as_str()).unwrap_or("");
            lb.cmp(la)
        });

        let total = sessions.len();
        let page: Vec<_> = sessions.into_iter().skip(offset).take(limit).collect();
        Ok(Some(serde_json::json!({
            "sessions": page,
            "total": total,
            "limit": limit,
            "offset": offset,
        })))
    }

    async fn session_detail(
        &self,
        ctx: &RequestContext,
        _workspace: &str,
        session: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        let Some(ref mgr) = ctx.state.memory_manager else {
            return Ok(Some(serde_json::json!({
                "session": session,
                "messages": [],
            })));
        };
        let store = mgr.get_episodic_store();
        let episodes = store
            .get_session(session)
            .await
            .map_err(|e| format!("get_session failed: {}", e))
            .unwrap_or_default();

        let messages: Vec<serde_json::Value> = episodes
            .iter()
            .map(|ep| {
                let mut obj = serde_json::json!({
                    "role": ep.role,
                    "content": ep.content,
                    "timestamp": ep.timestamp.to_rfc3339(),
                });
                let trigger_cluster = ep.tags.iter().any(|t| t == "cluster");
                if trigger_cluster {
                    obj["triggerCluster"] = serde_json::Value::Bool(true);
                }
                if let Some(n) = ep.metadata.get("tool_calls").and_then(|s| s.parse::<u32>().ok()) {
                    obj["toolCalls"] = serde_json::Value::Number(n.into());
                }
                obj
            })
            .collect();

        Ok(Some(serde_json::json!({
            "session": session,
            "messages": messages,
        })))
    }
}

// ---------------------------------------------------------------------------
// Request / cluster log entry builders (parse raw.json + .md files)
// ---------------------------------------------------------------------------

/// Build a top-level LlmRequestEntry summary from a request_logs/{ts}_{rand}/ dir.
fn build_request_entry(dir: &Path, id: &str, ts: &str, _suffix: &str) -> serde_json::Value {
    let files = sorted_files(dir);

    let mut model = String::new();
    let mut total_duration_ms: u64 = 0;
    let mut total_tool_calls: usize = 0;
    let mut max_round: usize = 0;
    let mut first_message = String::new();
    let mut final_duration_ms: u64 = 0;
    let mut final_rounds: usize = 0;

    for path in &files {
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        let ftype = file_type_name(&name);

        if ftype == "request.md" {
            if let Ok(content) = std::fs::read_to_string(path) {
                first_message = extract_md_first_message(&content);
            }
        } else if ftype == "AI.Request.raw.json" {
            if let Ok(content) = std::fs::read_to_string(path) {
                if let Ok(envelope) = serde_json::from_str::<serde_json::Value>(&content) {
                    if model.is_empty() {
                        model = envelope
                            .get("body")
                            .and_then(|b| b.get("model"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                    }
                    if let Some(r) = envelope.get("round").and_then(|v| v.as_u64()) {
                        if (r as usize) > max_round {
                            max_round = r as usize;
                        }
                    }
                }
            }
        } else if ftype == "AI.Response.raw.json" {
            if let Ok(content) = std::fs::read_to_string(path) {
                if let Ok(envelope) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(d) = envelope.get("duration_ms").and_then(|v| v.as_u64()) {
                        total_duration_ms = total_duration_ms.saturating_add(d);
                    }
                    let tool_count = envelope
                        .get("body")
                        .and_then(|b| b.get("choices"))
                        .and_then(|c| c.get(0))
                        .and_then(|c| c.get("message"))
                        .and_then(|m| m.get("tool_calls"))
                        .and_then(|t| t.as_array())
                        .map(|a| a.len())
                        .unwrap_or(0);
                    total_tool_calls += tool_count;
                }
            }
        } else if ftype == "response.md" {
            if let Ok(content) = std::fs::read_to_string(path) {
                if let Some(d) = extract_md_header(&content, "Total Duration") {
                    if let Some(ms) = parse_duration_seconds(&d) {
                        final_duration_ms = ms;
                    }
                }
                if let Some(r) = extract_md_header(&content, "LLM Rounds") {
                    if let Ok(n) = r.parse::<usize>() {
                        final_rounds = n;
                    }
                }
            }
        }
    }

    // Prefer the final-response.md Total Duration (covers all rounds including
    // local tool execution time between LLM calls); fall back to sum of raw response durations.
    let duration_ms = if final_duration_ms > 0 {
        final_duration_ms
    } else {
        total_duration_ms
    };
    let message_count = if final_rounds > 0 {
        final_rounds
    } else {
        max_round
    };

    serde_json::json!({
        "id": id,
        "timestamp": ts,
        "model": model,
        "duration_ms": duration_ms,
        "toolCallCount": total_tool_calls,
        "messageCount": message_count,
        "firstMessage": first_message,
        "iterations": [],
    })
}

/// Build a top-level ClusterTaskEntry summary from a cluster task dir.
fn build_cluster_task_entry(
    dir: &Path,
    device: &str,
    task_id: &str,
    ts_str: &str,
    local_node: &Option<String>,
) -> serde_json::Value {
    let files = sorted_files(dir);

    let mut first_message = String::new();
    let mut total_tool_calls: usize = 0;
    let mut total_duration_ms: u64 = 0;
    let mut has_error = false;
    let mut last_was_response = false;
    let mut final_duration_ms: u64 = 0;

    for path in &files {
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        let ftype = file_type_name(&name);

        if ftype == "request.md" {
            if let Ok(content) = std::fs::read_to_string(path) {
                first_message = extract_md_first_message(&content);
            }
        } else if ftype == "AI.Response.raw.json" {
            if let Ok(content) = std::fs::read_to_string(path) {
                if let Ok(envelope) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(d) = envelope.get("duration_ms").and_then(|v| v.as_u64()) {
                        total_duration_ms = total_duration_ms.saturating_add(d);
                    }
                    let tool_count = envelope
                        .get("body")
                        .and_then(|b| b.get("choices"))
                        .and_then(|c| c.get(0))
                        .and_then(|c| c.get("message"))
                        .and_then(|m| m.get("tool_calls"))
                        .and_then(|t| t.as_array())
                        .map(|a| a.len())
                        .unwrap_or(0);
                    total_tool_calls += tool_count;
                }
            }
        } else if ftype == "Local.md" {
            if let Ok(content) = std::fs::read_to_string(path) {
                if content.contains("### Error") {
                    has_error = true;
                }
            }
            last_was_response = false;
        } else if ftype == "response.md" {
            if let Ok(content) = std::fs::read_to_string(path) {
                if let Some(d) = extract_md_header(&content, "Total Duration") {
                    if let Some(ms) = parse_duration_seconds(&d) {
                        final_duration_ms = ms;
                    }
                }
            }
            last_was_response = true;
        }
    }

    let direction = match local_node {
        Some(local) if local == device => "outbound",
        Some(_) => "inbound",
        None => "unknown",
    };
    let peer_node = if direction == "inbound" {
        device.to_string()
    } else {
        String::new()
    };
    let status = if has_error {
        "failed"
    } else if last_was_response {
        "completed"
    } else {
        "unknown"
    };
    let duration_ms = if final_duration_ms > 0 {
        final_duration_ms
    } else {
        total_duration_ms
    };

    serde_json::json!({
        "id": task_id,
        "timestamp": ts_str,
        "duration_ms": duration_ms,
        "direction": direction,
        "peerNode": peer_node,
        "action": "",
        "firstMessage": first_message,
        "toolCallCount": total_tool_calls,
        "status": status,
        "iterations": [],
    })
}

/// Parse "**Duration**: 4.2s" → 4200 (ms).
fn parse_duration_seconds(value: &str) -> Option<u64> {
    let s = value.trim().trim_end_matches('s');
    let f: f64 = s.parse().ok()?;
    if f.is_finite() && f >= 0.0 {
        Some((f * 1000.0) as u64)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Iteration parsing (group by `round` field in raw.json envelopes)
// ---------------------------------------------------------------------------

#[derive(Default)]
struct IterationFiles {
    request: Option<PathBuf>,
    response: Option<PathBuf>,
    locals: Vec<PathBuf>,
    round: usize,
}

/// Walk the files of a dir, grouping them into iterations by the `round` field
/// in `*.AI.Request.raw.json` / `*.AI.Response.raw.json` envelopes.
///
/// Rules:
/// - Each AI.Request.raw.json opens a new iteration (round taken from envelope).
/// - The matching AI.Response.raw.json is the one with the same `round`.
/// - All Local.md files between the iteration's AI.Request and the next iteration's
///   AI.Request attach to this iteration.
fn parse_request_iterations(dir: &Path) -> serde_json::Value {
    let files = sorted_files(dir);

    // First pass: collect raw.json envelopes by file path so we know their round.
    let mut iterations: Vec<IterationFiles> = Vec::new();
    let mut pending_locals: Vec<PathBuf> = Vec::new();

    for path in &files {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let ftype = file_type_name(name);

        match ftype {
            "AI.Request.raw.json" => {
                let round = read_json_round(path).unwrap_or(iterations.len() + 1);
                // Attach any pending locals to the previous iteration (if any).
                if let Some(prev) = iterations.last_mut() {
                    prev.locals.extend(pending_locals.drain(..));
                } else {
                    // Locals appeared before any AI.Request — drop them.
                    pending_locals.clear();
                }
                iterations.push(IterationFiles {
                    request: Some(path.clone()),
                    response: None,
                    locals: Vec::new(),
                    round,
                });
            }
            "AI.Response.raw.json" => {
                let round = read_json_round(path).unwrap_or(0);
                // Match against the iteration with the same round, fallback to last.
                let len = iterations.len();
                let target_idx = iterations
                    .iter()
                    .rev()
                    .position(|it| it.round == round)
                    .map(|rev_idx| len.saturating_sub(1).saturating_sub(rev_idx));
                let target = match target_idx {
                    Some(idx) => iterations.get_mut(idx),
                    None => iterations.last_mut(),
                };
                if let Some(it) = target {
                    it.response = Some(path.clone());
                } else {
                    // Response without request — create stub.
                    iterations.push(IterationFiles {
                        request: None,
                        response: Some(path.clone()),
                        locals: Vec::new(),
                        round,
                    });
                }
            }
            "Local.md" => {
                if let Some(it) = iterations.last_mut() {
                    it.locals.push(path.clone());
                } else {
                    pending_locals.push(path.clone());
                }
            }
            _ => {}
        }
    }

    let result: Vec<serde_json::Value> = iterations
        .iter()
        .enumerate()
        .map(|(idx, group)| build_iteration_json(group, idx))
        .collect();
    serde_json::Value::Array(result)
}

/// Read the `round` field from a raw.json envelope. Returns None on any IO/parse error.
fn read_json_round(path: &Path) -> Option<usize> {
    let content = std::fs::read_to_string(path).ok()?;
    let envelope: serde_json::Value = serde_json::from_str(&content).ok()?;
    envelope.get("round").and_then(|v| v.as_u64()).map(|v| v as usize)
}

/// Build a single LlmIteration JSON from a grouped set of files.
fn build_iteration_json(group: &IterationFiles, index: usize) -> serde_json::Value {
    // Request: model + messages
    let (model, messages) = group
        .request
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
        .map(|env| {
            let body = env.get("body");
            let model = body
                .and_then(|b| b.get("model"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let messages = body
                .and_then(|b| b.get("messages"))
                .and_then(|m| m.as_array())
                .map(|arr| {
                    arr.iter()
                        .map(|m| {
                            let role = m
                                .get("role")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            // content may be a string or an array of content parts.
                            let content = stringify_message_content(m.get("content"));
                            serde_json::json!({ "role": role, "content": content })
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            (model, messages)
        })
        .unwrap_or_default();

    // Response: content + tool_calls + duration_ms + finish_reason
    let (response_content, response_tool_calls, duration_ms, finish_reason, usage) = group
        .response
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
        .map(|env| {
            let dur = env
                .get("duration_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let body = env.get("body");
            let choice = body
                .and_then(|b| b.get("choices"))
                .and_then(|c| c.get(0));
            let message = choice.and_then(|c| c.get("message"));
            let content = stringify_message_content(message.and_then(|m| m.get("content")));
            let finish = choice
                .and_then(|c| c.get("finish_reason"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let tool_calls = message
                .and_then(|m| m.get("tool_calls"))
                .and_then(|t| t.as_array())
                .map(|arr| {
                    arr.iter()
                        .map(|tc| {
                            let id = tc
                                .get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let func = tc.get("function");
                            let name = func
                                .and_then(|f| f.get("name"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let args_str = func
                                .and_then(|f| f.get("arguments"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let args_val: serde_json::Value =
                                serde_json::from_str(&args_str).unwrap_or_else(|_| {
                                    serde_json::Value::String(args_str.clone())
                                });
                            serde_json::json!({
                                "id": id,
                                "name": name,
                                "args": args_val,
                            })
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let usage = body
                .and_then(|b| b.get("usage"))
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            (content, tool_calls, dur, finish, usage)
        })
        .unwrap_or_default();

    // Tool results: parse Local.md files. Multiple Local.md may exist for one round.
    let tool_results: Vec<serde_json::Value> = group
        .locals
        .iter()
        .flat_map(|p| match std::fs::read_to_string(p) {
            Ok(content) => parse_local_tool_results(&content),
            Err(_) => Vec::new(),
        })
        .collect();

    let mut response = serde_json::json!({
        "content": response_content,
        "toolCalls": response_tool_calls,
        "duration_ms": duration_ms,
    });
    if !finish_reason.is_empty() {
        response
            .as_object_mut()
            .map(|m| m.insert("finish_reason".into(), serde_json::Value::String(finish_reason)));
    }
    if !usage.is_null() {
        response
            .as_object_mut()
            .map(|m| m.insert("usage".into(), usage));
    }

    serde_json::json!({
        "index": index,
        "round": group.round,
        "request": {
            "model": model,
            "messages": messages,
        },
        "response": response,
        "toolResults": tool_results,
    })
}

/// Convert an OpenAI-style `content` field (string OR array of parts) into a plain string.
fn stringify_message_content(content: Option<&serde_json::Value>) -> String {
    match content {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(arr)) => arr
            .iter()
            .filter_map(|part| {
                part.get("text")
                    .and_then(|v| v.as_str())
                    .or_else(|| part.get("content").and_then(|v| v.as_str()))
                    .map(|s| s.to_string())
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

/// Parse `## Operation N: Tool Execution` blocks from a Local.md.
fn parse_local_tool_results(content: &str) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    let mut current_name = String::new();
    let mut current_status = String::new();
    let mut current_args = String::new();
    let mut current_result = String::new();
    let mut current_error = String::new();
    let mut current_duration_ms: u64 = 0;
    let mut in_op = false;
    let mut in_args = false;
    let mut in_result = false;
    let mut in_error = false;

    let flush = |out: &mut Vec<serde_json::Value>,
                 name: &mut String,
                 status: &mut String,
                 args: &mut String,
                 result: &mut String,
                 error: &mut String,
                 duration_ms: &mut u64| {
        if name.is_empty() {
            return;
        }
        let result_val: serde_json::Value = if !error.is_empty() {
            serde_json::json!({ "status": status.clone(), "error": error.clone() })
        } else {
            serde_json::json!({
                "status": status.clone(),
                "output": result.clone(),
            })
        };
        let mut obj = serde_json::json!({
            "callId": name.clone(),
            "name": name.clone(),
            "result": result_val,
        });
        if let Some(o) = obj.as_object_mut() {
            if !args.is_empty() {
                let parsed: serde_json::Value =
                    serde_json::from_str(args.trim()).unwrap_or_else(|_| {
                        serde_json::Value::String(args.trim().to_string())
                    });
                o.insert("args".into(), parsed);
            }
            if *duration_ms > 0 {
                o.insert("duration_ms".into(), serde_json::Value::Number((*duration_ms).into()));
            }
        }
        out.push(obj);
        name.clear();
        status.clear();
        args.clear();
        result.clear();
        error.clear();
        *duration_ms = 0;
    };

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("## Operation ") {
            // flush previous
            if in_op {
                flush(
                    &mut out,
                    &mut current_name,
                    &mut current_status,
                    &mut current_args,
                    &mut current_result,
                    &mut current_error,
                    &mut current_duration_ms,
                );
            }
            in_op = true;
            in_args = false;
            in_result = false;
            in_error = false;
            continue;
        }
        if !in_op {
            continue;
        }
        // Section markers within an operation
        if trimmed.starts_with("### Arguments") {
            in_args = true;
            in_result = false;
            in_error = false;
            continue;
        }
        if trimmed.starts_with("### Result") {
            in_args = false;
            in_result = true;
            in_error = false;
            continue;
        }
        if trimmed.starts_with("### Error") {
            in_args = false;
            in_result = false;
            in_error = true;
            continue;
        }
        if trimmed.starts_with("### Duration") {
            in_args = false;
            in_result = false;
            in_error = false;
            // parse "0.022s" on this line
            if let Some(rest) = trimmed.strip_prefix("### Duration") {
                let s = rest.trim().trim_end_matches('s');
                if let Ok(f) = s.parse::<f64>() {
                    current_duration_ms = (f * 1000.0) as u64;
                }
            }
            continue;
        }
        if trimmed.starts_with("---") {
            // Section separator within Local.md; flush current op
            flush(
                &mut out,
                &mut current_name,
                &mut current_status,
                &mut current_args,
                &mut current_result,
                &mut current_error,
                &mut current_duration_ms,
            );
            in_op = false;
            in_args = false;
            in_result = false;
            in_error = false;
            continue;
        }
        // Strip code fences
        if trimmed.starts_with("```") {
            continue;
        }
        // Field headers
        if let Some(rest) = trimmed.strip_prefix("**Name**: ") {
            current_name = rest.to_string();
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("**Status**: ") {
            current_status = rest.to_string();
            continue;
        }
        // Body accumulation
        if in_args {
            current_args.push_str(line);
            current_args.push('\n');
        } else if in_result {
            current_result.push_str(line);
            current_result.push('\n');
        } else if in_error {
            current_error.push_str(line);
            current_error.push('\n');
        }
    }
    if in_op {
        flush(
            &mut out,
            &mut current_name,
            &mut current_status,
            &mut current_args,
            &mut current_result,
            &mut current_error,
            &mut current_duration_ms,
        );
    }
    out
}
