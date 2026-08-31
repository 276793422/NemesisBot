//! LSP session management: spawn-per-(language, root), lazy idle reap,
//! graceful shutdown.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

use crate::proto;
use crate::registry::{self, Lang};

/// The four read-only operations this crate exposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LspOp {
    Definition,
    References,
    Implementation,
    Hover,
}

impl LspOp {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "definition" => Some(LspOp::Definition),
            "references" => Some(LspOp::References),
            "implementation" => Some(LspOp::Implementation),
            "hover" => Some(LspOp::Hover),
            _ => None,
        }
    }

    pub fn method(self) -> &'static str {
        match self {
            LspOp::Definition => "textDocument/definition",
            LspOp::References => "textDocument/references",
            LspOp::Implementation => "textDocument/implementation",
            LspOp::Hover => "textDocument/hover",
        }
    }
}

/// Owns one live server session. `Inner` is behind an async mutex so
/// requests to the same session serialize (LSP stdio is inherently
/// sequential from one client).
struct Session {
    /// Updated on every query; read by the idle sweep (std mutex — only
    /// ever held for a nanosecond copy).
    last_used: std::sync::Mutex<Instant>,
    inner: Mutex<Inner>,
}

struct Inner {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
}

impl Session {
    /// Send one request and wait for its response, answering server→client
    /// requests along the way (never-ignore policy — some servers block on
    /// `workspace/configuration`). Transient server-side invalidations
    /// (LSP -32800 RequestCancelled / -32801 ContentModified — e.g.
    /// rust-analyzer dropping in-flight queries when its VFS changes) are
    /// retried in place with a small backoff: the spec marks them safe to
    /// retry, and failing the query would just push the burden onto every
    /// caller.
    async fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        const TRANSIENT_RETRIES: usize = 5;
        let fut = async {
            for attempt in 0..=TRANSIENT_RETRIES {
                if attempt > 0 {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
                let mut inner = self.inner.lock().await;
                let id = inner.next_id;
                inner.next_id += 1;
                let msg = proto::request(id, method, params.clone());
                write_message(&mut inner, &msg).await?;
                loop {
                    let incoming = read_message(&mut inner).await?;
                    match proto::classify(&incoming, id) {
                        proto::Incoming::Response(resp) => {
                            if let Some(err) = resp.get("error")
                                && !err.is_null() {
                                    if proto::is_transient_error(err) {
                                        // Re-send with a fresh id; on the
                                        // last attempt the `break` falls out
                                        // of the for-loop to the descriptive
                                        // "kept cancelling" error below (a
                                        // bare -32800 would tell the model
                                        // nothing about the retry budget).
                                        break;
                                    }
                                    return Err(format!(
                                        "server error on {method}: {err}"
                                    ));
                                }
                            return Ok(resp.get("result").cloned().unwrap_or(Value::Null));
                        }
                        proto::Incoming::ServerRequest { id, method } => {
                            let reply = proto::response_ok(
                                id,
                                proto::default_server_response(&method),
                            );
                            write_message(&mut inner, &reply).await?;
                            // keep waiting for our own response
                        }
                        proto::Incoming::Notification { .. } => {
                            // progress/log noise — skip
                        }
                    }
                }
            }
            Err(format!(
                "server kept cancelling {method} (RequestCancelled/ContentModified) after {TRANSIENT_RETRIES} retries"
            ))
        };
        tokio::time::timeout(timeout, fut)
            .await
            .map_err(|_| format!("LSP request {method} timed out after {timeout:?}"))?
    }
}

async fn write_message(inner: &mut Inner, msg: &Value) -> Result<(), String> {
    inner
        .stdin
        .write_all(&proto::encode(msg))
        .await
        .map_err(|e| format!("write to server stdin failed: {e}"))?;
    inner
        .stdin
        .flush()
        .await
        .map_err(|e| format!("flush to server stdin failed: {e}"))
}

/// Read one framed message straight off the BufReader (line-oriented
/// headers then an exact-length body).
async fn read_message(inner: &mut Inner) -> Result<Value, String> {
    // Headers.
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        inner
            .stdout
            .read_line(&mut line)
            .await
            .map_err(|e| format!("read header line failed: {e}"))?;
        if line.is_empty() {
            return Err("server closed the stream".to_string());
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break; // end of headers
        }
        if let Some((name, value)) = trimmed.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length") {
                content_length = Some(
                    value
                        .trim()
                        .parse::<usize>()
                        .map_err(|e| format!("bad Content-Length {value:?}: {e}"))?,
                );
            }
    }
    let len = content_length.ok_or("message without Content-Length header")?;
    let mut body = vec![0u8; len];
    inner
        .stdout
        .read_exact(&mut body)
        .await
        .map_err(|e| format!("read body failed: {e}"))?;
    serde_json::from_slice(&body).map_err(|e| format!("bad JSON body: {e}"))
}

/// The public entry point. One instance per process is intended (the agent
/// tool holds one); sessions are per (language, project root).
pub struct LspManager {
    sessions: Mutex<HashMap<(Lang, PathBuf), Arc<Session>>>,
    request_timeout: Duration,
    idle_after: Duration,
}

impl LspManager {
    /// `request_timeout`: per LSP request budget (default 120s — a first
    /// query on a big repo waits behind server indexing).
    /// `idle_after`: session idle threshold for the lazy sweep (default
    /// 600s).
    pub fn new(request_timeout: Option<Duration>, idle_after: Option<Duration>) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            request_timeout: request_timeout.unwrap_or(Duration::from_secs(120)),
            idle_after: idle_after.unwrap_or(Duration::from_secs(600)),
        }
    }

    /// Run one semantic query. Returns a human/model-readable result string;
    /// `Err` carries actionable messages (unsupported file type, server not
    /// installed, transport failure).
    pub async fn query(
        &self,
        op: LspOp,
        path: &Path,
        line: u32,
        character: u32,
    ) -> Result<String, String> {
        let Some(lang) = registry::lang_for_path(path) else {
            let supported: Vec<&str> = registry::SERVERS.iter().map(|s| s.lang.label()).collect();
            return Err(format!(
                "unsupported file type: {} (supported languages: {})",
                path.display(),
                supported.join(", ")
            ));
        };
        let Some(spec) = registry::spec_for(lang) else {
            return Err(format!("no language server configured for {}", lang.label()));
        };
        if !path.is_file() {
            return Err(format!("file does not exist: {}", path.display()));
        }
        // Fresh per-call probe (not the registration-time cache): a server
        // installed since registration works without a restart.
        let server_path = registry::find_command(spec.command).ok_or_else(|| {
            format!(
                "language server for {} is not installed: `{}` not found on PATH — install it, then retry",
                lang.label(),
                spec.command
            )
        })?;

        let root = find_root(path);
        self.reap_idle().await;

        let mut params = json!({
            "textDocument": {"uri": proto::path_to_uri(path)},
            "position": {"line": line, "character": character},
        });
        if op == LspOp::References {
            params["context"] = json!({"includeDeclaration": true});
        }

        let session = self.get_or_spawn(lang, &root, &server_path).await?;
        *session.last_used.lock().unwrap() = Instant::now();
        let result = match session
            .request(op.method(), params.clone(), self.request_timeout)
            .await
        {
            Ok(v) => Ok(v),
            // Session died mid-request (server crash/exit): evict and retry
            // once on a fresh session rather than failing the query.
            Err(e) if is_transport_error(&e) => {
                tracing::warn!(
                    "[LSP] {} session for {} died mid-request ({e}); respawning once",
                    lang.label(),
                    root.display()
                );
                self.sessions.lock().await.remove(&(lang, root.clone()));
                // Tree-kill the dead session's child now instead of waiting
                // for the Arc drop: kill_on_drop only reaps the direct child
                // (the shim), and a shim-wrapped server would leak the real
                // server process as an orphan holding our pipes.
                if let Ok(mut inner) = session.inner.try_lock() {
                    kill_tree(&mut inner.child).await;
                }
                let fresh = self.get_or_spawn(lang, &root, &server_path).await?;
                *fresh.last_used.lock().unwrap() = Instant::now();
                fresh.request(op.method(), params, self.request_timeout).await
            }
            Err(e) => Err(e),
        };
        let result = result?;
        Ok(format_result(op, &result))
    }

    /// Get the cached session for (lang, root) or spawn+initialize a new
    /// one (and cache it). The lock is held THROUGH initialize on purpose:
    /// concurrent queries for the same root must not double-spawn the
    /// server (cost: cross-language queries serialize for the handshake).
    async fn get_or_spawn(
        &self,
        lang: Lang,
        root: &Path,
        server_path: &Path,
    ) -> Result<Arc<Session>, String> {
        let key = (lang, root.to_path_buf());
        let mut sessions = self.sessions.lock().await;
        if let Some(s) = sessions.get(&key) {
            return Ok(Arc::clone(s));
        }
        let Some(spec) = registry::spec_for(lang) else {
            return Err(format!("no language server configured for {}", lang.label()));
        };
        let s = Arc::new(spawn_session(lang, spec, root, server_path).await?);
        sessions.insert(key, Arc::clone(&s));
        Ok(s)
    }

    /// Lazily shut down sessions idle longer than `idle_after`. Returns the
    /// number reaped. Called before every query — no background thread, so
    /// lifecycle behavior is deterministic and testable.
    pub async fn reap_idle(&self) -> usize {
        let now = Instant::now();
        let mut stale: Vec<(Lang, PathBuf)> = Vec::new();
        {
            let sessions = self.sessions.lock().await;
            for (key, s) in sessions.iter() {
                if now.duration_since(*s.last_used.lock().unwrap()) >= self.idle_after {
                    stale.push(key.clone());
                }
            }
        }
        let mut reaped = 0usize;
        for key in &stale {
            if self.close_session(key).await {
                reaped += 1;
            }
        }
        reaped
    }

    /// Gracefully close every session (shutdown → exit → kill). Returns the
    /// number closed. Intended for process teardown and tests.
    pub async fn shutdown_all(&self) -> usize {
        let keys: Vec<(Lang, PathBuf)> = self.sessions.lock().await.keys().cloned().collect();
        let mut closed = 0usize;
        for key in &keys {
            if self.close_session(key).await {
                closed += 1;
            }
        }
        closed
    }

    /// Live session count (diagnostics/tests).
    pub async fn session_count(&self) -> usize {
        self.sessions.lock().await.len()
    }

    async fn close_session(&self, key: &(Lang, PathBuf)) -> bool {
        let session = {
            let mut sessions = self.sessions.lock().await;
            sessions.remove(key)
        };
        let Some(session) = session else {
            return false;
        };
        // Best-effort graceful shutdown; the hard kill below is the safety
        // net either way. try_lock: if a request is in flight we skip the
        // handshake — kill_on_drop reaps the child once the request's Arc
        // drops.
        let _ = session
            .request("shutdown", Value::Null, Duration::from_secs(5))
            .await;
        if let Ok(mut inner) = session.inner.try_lock() {
            let exit = proto::notification("exit", Value::Null);
            let _ = inner.stdin.write_all(&proto::encode(&exit)).await;
            let _ = inner.stdin.flush().await;
            kill_tree(&mut inner.child).await;
        }
        true
    }
}

/// Kill the child AND its descendants. LSP servers on Windows are routinely
/// shim-wrapped (`.cmd`/npm shims): TerminateProcess on the shim orphans
/// the grandchild, and a surviving grandchild holds the host's pipes
/// hostage — the host process then refuses to exit until the orphan dies
/// on its own (observed with the fake-server tests: a hung shim-wrapped
/// python kept the whole test process alive past its own sleep). Same
/// lesson as the CLI-delegation layer's tree-kill.
async fn kill_tree(child: &mut Child) {
    #[cfg(windows)]
    if let Some(pid) = child.id() {
        let mut c = tokio::process::Command::new("taskkill");
        c.args(["/T", "/F", "/PID", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            // Never open a console window (project background-process rule).
            .creation_flags(0x0800_0000);
        let _ = c.status().await;
    }
    // Non-Windows: process groups make the direct kill sufficient (and
    // kill_on_drop remains the backstop on every platform).
    let _ = child.kill().await;
}

async fn spawn_session(
    _lang: Lang,
    spec: &registry::ServerSpec,
    root: &Path,
    server_path: &Path,
) -> Result<Session, String> {
    let mut cmd = Command::new(server_path);
    cmd.args(spec.args)
        .current_dir(root)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        // stderr dropped: progress/status spam (rust-analyzer is chatty);
        // piping it without reading risks a full-pipe deadlock, and read-only
        // queries don't need it.
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        // Never open a console window (project background-process rule).
        // tokio's Command has an inherent `creation_flags` on Windows.
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to spawn {}: {e}", spec.command))?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| "no stdin pipe".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "no stdout pipe".to_string())?;

    let session = Session {
        last_used: std::sync::Mutex::new(Instant::now()),
        inner: Mutex::new(Inner {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
        }),
    };

    // Initialize handshake. `capabilities: {}` = client supports nothing
    // fancy; servers degrade to plain text responses.
    let root_uri = proto::path_to_uri(root);
    let init_params = json!({
        "processId": Value::Null,
        "rootUri": root_uri,
        "capabilities": {},
        "workspaceFolders": [{"uri": root_uri, "name": root.display().to_string()}],
    });
    let _caps = session
        .request("initialize", init_params, Duration::from_secs(60))
        .await?;
    let _ = session
        .request_no_wait("initialized", json!({}))
        .await;
    Ok(session)
}

impl Session {
    /// Fire a notification (no response expected).
    async fn request_no_wait(&self, method: &str, params: Value) -> Result<(), String> {
        let mut inner = self.inner.lock().await;
        write_message(&mut inner, &proto::notification(method, params)).await
    }
}

/// Whether an error smells like the transport itself broke (dead child)
/// versus a well-formed LSP error.
fn is_transport_error(e: &str) -> bool {
    e.contains("closed the stream")
        || e.contains("stdin failed")
        || e.contains("read header")
        || e.contains("read body")
}

/// Find the project root for a file: walk up until a directory containing
/// a project marker (`.git`, `Cargo.toml`, `package.json`, `pyproject.toml`,
/// `go.mod`), falling back to the file's own directory. Language servers
/// discover their workspace from this root (they walk up further on their
/// own when needed, e.g. cargo workspace parents).
fn find_root(path: &Path) -> PathBuf {
    const MARKERS: [&str; 5] = [".git", "Cargo.toml", "package.json", "pyproject.toml", "go.mod"];
    let start = path
        .parent()
        .map(|p| p.to_path_buf())
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_default();
    let mut dir = start.clone();
    loop {
        if MARKERS.iter().any(|m| dir.join(m).exists()) {
            return dir;
        }
        match dir.parent() {
            Some(parent) if parent != dir => dir = parent.to_path_buf(),
            _ => break,
        }
    }
    // No marker anywhere up the tree — the file's directory is the best
    // root we can offer.
    start
}

/// Render a result for the model: hover as text, location ops as a
/// `path:line:character` list (0-based, matching the input convention).
fn format_result(op: LspOp, result: &Value) -> String {
    if op == LspOp::Hover {
        let text = proto::parse_hover(result);
        if text.trim().is_empty() {
            return "(no hover information at this position)".to_string();
        }
        return text;
    }
    let locs = proto::parse_locations(result);
    if locs.is_empty() {
        return format!("no {} found at this position", method_noun(op));
    }
    let mut out = format!(
        "{} {} (path:line:character, 0-based):\n",
        locs.len(),
        method_noun(op)
    );
    for (i, loc) in locs.iter().enumerate() {
        out.push_str(&format!("{}. {}:{}:{}\n", i + 1, loc.path, loc.line, loc.character));
    }
    out
}

fn method_noun(op: LspOp) -> &'static str {
    match op {
        LspOp::Definition => "definitions",
        LspOp::References => "references",
        LspOp::Implementation => "implementations",
        LspOp::Hover => "hover",
    }
}

#[cfg(test)]
mod tests;
