//! Session full-text search (U20, dsh-alignment sixth batch).
//!
//! SQLite FTS5 index over the chat-log JSONL files (`session_logs/*.jsonl`),
//! queryable by the `history_search` agent tool and the
//! `nemesisbot history search` CLI. Answers "which conversation said X" with
//! a session key + snippet + timestamp, across sessions (the goal's
//! acceptance criterion).
//!
//! Design (goal §二):
//! - Index DB: `<workspace>/logs/history_index.db`, one FTS5 table
//!   `(session_key, seq, role, timestamp, content, content_bigram)` where
//!   `content_bigram` carries the CJK-2gram expansion of the content.
//!   unicode61 (the bundled FTS5 default tokenizer) treats a run of CJK
//!   chars as ONE token, so a raw Chinese query like「部署」would never match
//!   「部署文档」— the bigram column (「部署 署文 文档」) makes short Chinese
//!   queries hit. Latin text rides along unchanged (bigrams only generated
//!   for CJK runs).
//! - Update: lazy two-stage — [`reindex_session_logs`] full-scans the JSONL
//!   dir on first query (mtime-tracked, so unchanged files are skipped on
//!   later calls), and [`index_append`] inserts single rows as messages are
//!   appended. No WAL/real-time machinery.
//! - Boundary events: those live in the sidecar (`logs/boundary/`), NOT in
//!   session_logs — nothing to exclude here.
//! - Failure semantics: any DB error degrades the tool to a grep-style
//!   linear scan of the JSONL dir (same answers, slower) — search never
//!   blocks the agent flow (goal §八).

use nemesis_path::default_path_manager;
use rusqlite::Connection;
use std::io::BufRead;
use std::path::PathBuf;
use std::sync::Mutex;

/// One search hit.
#[derive(Debug, Clone, serde::Serialize)]
pub struct HistoryHit {
    pub session_key: String,
    /// 0-based line index of the matched message in its session jsonl.
    pub seq: usize,
    pub role: String,
    pub timestamp: String,
    /// Snippet around the first match (already-trimmed tool results included).
    pub snippet: String,
}

struct IndexState {
    conn: Option<Connection>,
    /// (session file stem, mtime) of indexed files — skip unchanged on reindex.
    indexed: std::collections::HashMap<String, std::time::SystemTime>,
}

static INDEX: Mutex<Option<IndexState>> = Mutex::new(None);

/// Path of the FTS index DB.
pub fn index_db_path() -> PathBuf {
    default_path_manager()
        .sessions_log_dir()
        .parent()
        .map(|p| p.join("history_index.db"))
        .unwrap_or_else(|| PathBuf::from("history_index.db"))
}

fn open_conn() -> Option<Connection> {
    let path = index_db_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let conn = Connection::open(&path).ok()?;
    // Write performance: this is a REBUILDABLE cache (reindex regenerates it
    // from the jsonl source of truth), so durability pragmas can be relaxed.
    // Default journal (delete-mode, fsync per txn) made each append-hook
    // insert cost a disk sync — visible as multi-second test runs / agent
    // append latency on busy disks.
    let _ = conn.pragma_update(None, "journal_mode", "TRUNCATE");
    let _ = conn.pragma_update(None, "synchronous", "NORMAL");
    // External-content-less plain FTS5 table. `content_bigram` doubles CJK
    // runs as bigrams (see module doc). tokenize='unicode61' is the default.
    conn.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS history_fts USING fts5(\
             session_key UNINDEXED, seq UNINDEXED, role UNINDEXED, \
             timestamp UNINDEXED, content, content_bigram, \
             tokenize='unicode61');\
         ",
    )
    .ok()?;
    Some(conn)
}

/// Expand CJK runs into overlapping bigrams for the `content_bigram` column.
/// Latin/digit/punct words pass through whole; CJK runs become bigram
/// sequences. Single-char CJK runs emit the char itself (a 1-gram can't
/// form a bigram and must stay findable).
pub fn cjk_bigrams(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2);
    let mut latin: Vec<char> = Vec::new(); // current non-CJK word run
    let mut cjk: Vec<char> = Vec::new(); // current CJK run
    let flush_cjk = |run: &mut Vec<char>, out: &mut String| {
        if run.is_empty() {
            return;
        }
        if run.len() == 1 {
            out.push(run[0]);
        } else {
            for w in run.windows(2) {
                out.push(w[0]);
                out.push(w[1]);
                out.push(' ');
            }
            // windows loop leaves a trailing space per pair; trim handled by
            // the final String builder — push trailing space for single too.
            if !out.ends_with(' ') && !out.is_empty() {
                out.push(' ');
            }
        }
        run.clear();
    };
    let flush_latin = |run: &mut Vec<char>, out: &mut String| {
        if !run.is_empty() {
            out.extend(run.iter());
            out.push(' ');
            run.clear();
        }
    };
    for ch in text.chars() {
        let is_cjk = matches!(ch as u32,
            0x3400..=0x4DBF   |  // CJK ext A
            0x4E00..=0x9FFF   |  // CJK unified
            0xF900..=0xFAFF   |  // CJK compat
            0x20000..=0x2A6DF);  // CJK ext B
        if is_cjk {
            flush_latin(&mut latin, &mut out);
            cjk.push(ch);
        } else if ch.is_alphanumeric() {
            flush_cjk(&mut cjk, &mut out);
            latin.push(ch);
        } else {
            // separator: end both runs
            flush_latin(&mut latin, &mut out);
            flush_cjk(&mut cjk, &mut out);
        }
    }
    flush_latin(&mut latin, &mut out);
    flush_cjk(&mut cjk, &mut out);
    out.trim_end().to_string()
}

fn with_conn<R>(f: impl FnOnce(&mut Connection, &mut std::collections::HashMap<String, std::time::SystemTime>) -> R) -> Option<R> {
    let mut guard = INDEX.lock().ok()?;
    let st = guard.get_or_insert_with(|| IndexState {
        conn: open_conn(),
        indexed: std::collections::HashMap::new(),
    });
    let conn = st.conn.as_mut()?;
    Some(f(conn, &mut st.indexed))
}

/// Full/lazy (re)index of `session_logs/*.jsonl`. mtime-incremental: files
/// whose mtime matches the recorded value are skipped. Returns the number of
/// rows indexed this call (0 = everything fresh).
pub fn reindex_session_logs() -> usize {
    let dir = default_path_manager().sessions_log_dir();
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return 0;
    };
    let mut rows = 0usize;
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for ent in rd.flatten() {
        let path = ent.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        // Skip our own index db / meta files by extension filter above.
        let Ok(meta) = ent.metadata() else {
            continue;
        };
        let Ok(mtime) = meta.modified() else {
            continue;
        };
        seen.insert(stem.to_string());
        let mut did_index = false;
        let mut skipped_fresh = false;
        with_conn(|conn, indexed| {
            if indexed.get(stem) == Some(&mtime) {
                skipped_fresh = true;
                return;
            }
            // Re-indexing a changed file: clear its rows first.
            let _ = conn.execute(
                "DELETE FROM history_fts WHERE session_key = ?1",
                rusqlite::params![stem],
            );
            if let Ok(file) = std::fs::File::open(&path) {
                let mut seq = 0usize;
                for line in std::io::BufReader::new(file).lines().flatten() {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                        if let Err(_) = insert_row(conn, stem, seq, &v) {
                            break;
                        }
                    }
                    seq += 1;
                }
            }
            indexed.insert(stem.to_string(), mtime);
            did_index = true;
        });
        if !skipped_fresh && did_index {
            rows += 1;
        }
    }

    // Orphan purge (2026-08-23): rows whose source file no longer exists must
    // go — pre-fix, deleted sessions' rows lingered forever and eventually
    // filled the search window with ghosts. The comparison is DB-vs-disk
    // directly: gating on the in-memory `indexed` map (`known == seen`) made
    // the purge unreachable exactly when it was needed — a fresh process's
    // full scan registers every disk file into `indexed`, so the map always
    // equals the disk set afterwards, and files deleted while no process was
    // watching (leaked test files, sessions deleted in a prior run) kept
    // their rows. One SELECT DISTINCT per reindex is cheap (stems only).
    with_conn(|conn, indexed| {
        let indexed_keys: Vec<String> = conn
            .prepare("SELECT DISTINCT session_key FROM history_fts")
            .map(|mut s| {
                s.query_map([], |r| r.get::<_, String>(0))
                    .map(|ks| ks.flatten().collect::<Vec<_>>())
                    .unwrap_or_default()
            })
            .unwrap_or_default();
        for k in indexed_keys {
            if !seen.contains(&k) {
                let _ = conn.execute(
                    "DELETE FROM history_fts WHERE session_key = ?1",
                    rusqlite::params![k],
                );
            }
        }
        indexed.retain(|k, _| seen.contains(k));
    });
    rows
}

fn insert_row(
    conn: &Connection,
    session_key: &str,
    seq: usize,
    v: &serde_json::Value,
) -> rusqlite::Result<usize> {
    let role = v.get("role").and_then(|r| r.as_str()).unwrap_or("");
    let content = v.get("content").and_then(|c| c.as_str()).unwrap_or("");
    let timestamp = v.get("timestamp").and_then(|t| t.as_str()).unwrap_or("");
    conn.execute(
        "INSERT INTO history_fts (session_key, seq, role, timestamp, content, content_bigram)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            session_key,
            seq as i64,
            role,
            timestamp,
            content,
            cjk_bigrams(content)
        ],
    )
}

/// Incremental hook for `append_chat_log_full` — index one freshly appended
/// message. Best-effort: DB failures are swallowed (warn) — the next full
/// reindex picks the row up. Track seq by file line count only when the
/// index already knows the file; otherwise skip (full reindex handles it).
pub fn index_append(session_key: &str, role: &str, content: &str, timestamp: &str) {
    // Namespace: the DB rows and the `indexed` map are keyed by FILE STEM
    // (colons replaced) — same as reindex_session_logs, since chat_log
    // writes `<stem>.jsonl`. 2026-08-23 fix: this used to look up the RAW
    // session key, so for the normal production key form (`chan:chat:user`,
    // always contains ':') the "file already indexed" check never matched
    // and the incremental hook silently never fired — masked until the
    // orphan-row purge stopped stale rows from satisfying the tests.
    let stem = session_key.replace(':', "_");
    let _ = with_conn(|conn, indexed| {
        if !indexed.contains_key(&stem) {
            return; // never full-indexed — first query reindexes everything
        }
        let seq: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(seq), -1) + 1 FROM history_fts WHERE session_key = ?1",
                rusqlite::params![stem],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let v = serde_json::json!({"role": role, "content": content, "timestamp": timestamp});
        if let Err(e) = insert_row(conn, &stem, seq as usize, &v) {
            tracing::warn!("[history_search] index_append failed: {e}");
        }
    });
}

/// Build the FTS5 match expression: the raw query as a phrase against
/// `content`, OR every bigram token of the query against `content_bigram`
/// (any-hit semantics — a phrase match on the bigram column would require
/// the source text to contain the query as a contiguous bigram subsequence,
/// which is stricter than "the words appear"). Tokens are quoted to avoid
/// FTS5 syntax injection.
fn match_expr(query: &str) -> String {
    let q = query.replace('"', "\"\"");
    let mut parts = vec![format!("content : \"{q}\"")];
    let bigram_str = cjk_bigrams(query);
    let bigrams: Vec<&str> = bigram_str
        .split_whitespace()
        // latin words already covered by the phrase arm; keep only tokens
        // containing CJK chars (the bigrams proper).
        .filter(|t| t.chars().any(|c| (c as u32) >= 0x3400))
        .collect();
    if !bigrams.is_empty() {
        let ors: Vec<String> = bigrams.iter().map(|t| format!("content_bigram : \"{}\"", t)).collect();
        parts.push(ors.join(" OR "));
    }
    parts.join(" OR ")
}

/// Snippet around the first occurrence of any query word in the content.
fn snippet_around(content: &str, query: &str, max_chars: usize) -> String {
    let max_chars = max_chars.max(32);
    let lower = content.to_lowercase();
    let ql = query.trim().to_lowercase();
    let pos = ql
        .split_whitespace()
        .find_map(|w| lower.find(w).filter(|_| !w.is_empty()))
        .unwrap_or(0);
    // Char-boundary-safe window (multibyte panic discipline).
    let start = pos.saturating_sub(max_chars / 3);
    let start = floor_char_boundary(content, start);
    let end = (start + max_chars).min(content.len());
    let end = ceil_char_boundary(content, end);
    let mut s: String = content[start..end].to_string();
    if start > 0 {
        s.insert_str(0, "…");
    }
    if end < content.len() {
        s.push('…');
    }
    s
}

fn floor_char_boundary(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_char_boundary(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// Query across sessions. FTS path first; on any DB failure degrade to a
/// linear grep-style scan of the JSONL dir (goal §八 fallback).
pub fn search(query: &str, limit: usize) -> Vec<HistoryHit> {
    let limit = limit.clamp(1, 100);
    if query.trim().is_empty() {
        return Vec::new();
    }
    if let Some(hits) = search_fts(query, limit) {
        return hits;
    }
    search_linear(query, limit)
}

fn search_fts(query: &str, limit: usize) -> Option<Vec<HistoryHit>> {
    with_conn(|conn, _| {
        let expr = match_expr(query);
        let mut stmt = conn
            .prepare(
                "SELECT session_key, seq, role, timestamp, content \
                 FROM history_fts WHERE history_fts MATCH ?1 \
                 ORDER BY rank LIMIT ?2",
            )
            .ok()?;
        let rows = stmt
            .query_map(rusqlite::params![expr, limit as i64], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                ))
            })
            .ok()?;
        let mut hits = Vec::new();
        for row in rows.flatten() {
            hits.push(HistoryHit {
                session_key: row.0,
                seq: row.1 as usize,
                role: row.2,
                timestamp: row.3,
                snippet: snippet_around(&row.4, query, 160),
            });
        }
        Some(hits)
    })
    .flatten()
}

/// Degraded path: scan `session_logs/*.jsonl` directly (no index). Same
/// answers, slower — the DB failing must not break the tool.
fn search_linear(query: &str, limit: usize) -> Vec<HistoryHit> {
    let dir = default_path_manager().sessions_log_dir();
    let mut hits = Vec::new();
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return hits;
    };
    let ql = query.trim().to_lowercase();
    if ql.is_empty() {
        return hits;
    }
    for ent in rd.flatten() {
        let path = ent.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let Ok(file) = std::fs::File::open(&path) else {
            continue;
        };
        for (seq, line) in std::io::BufReader::new(file).lines().flatten().enumerate() {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            let content = v.get("content").and_then(|c| c.as_str()).unwrap_or("");
            if content.to_lowercase().contains(&ql) {
                hits.push(HistoryHit {
                    session_key: stem.clone(),
                    seq,
                    role: v.get("role").and_then(|r| r.as_str()).unwrap_or("").to_string(),
                    timestamp: v.get("timestamp").and_then(|t| t.as_str()).unwrap_or("").to_string(),
                    snippet: snippet_around(content, query, 160),
                });
                if hits.len() >= limit {
                    return hits;
                }
            }
        }
    }
    hits
}

/// Render hits for the LLM (the `history_search` tool result body).
pub fn render_hits(hits: &[HistoryHit]) -> String {
    if hits.is_empty() {
        return "没有找到匹配的历史消息。".to_string();
    }
    let mut out = format!("找到 {} 条匹配：\n", hits.len());
    for h in hits {
        out.push_str(&format!(
            "- [{}] session={} seq={} time={}\n  {}\n",
            h.role, h.session_key, h.seq, h.timestamp, h.snippet
        ));
    }
    out.push_str("\n可用 read_chat_log 式会话定位（session_key）进一步查看上下文。");
    out
}

#[cfg(test)]
mod tests;
