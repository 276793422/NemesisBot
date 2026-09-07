//! LSP session management: spawn-per-(language, root), lazy idle reap,
//! graceful shutdown.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

use crate::proto;
use crate::registry::{self, Lang};

/// The operations this crate exposes. Four are read-only queries; C7 adds
/// the write-capable pair — `Rename` produces file edits that the CALLER
/// (the agent tool) must push through the security pipeline before any
/// disk write, and `CodeAction` lists quickfixes (apply stays out of
/// scope for now).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LspOp {
    Definition,
    References,
    Implementation,
    Hover,
    Rename,
    CodeAction,
}

impl LspOp {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "definition" => Some(LspOp::Definition),
            "references" => Some(LspOp::References),
            "implementation" => Some(LspOp::Implementation),
            "hover" => Some(LspOp::Hover),
            "rename" => Some(LspOp::Rename),
            "code_action" => Some(LspOp::CodeAction),
            _ => None,
        }
    }

    pub fn method(self) -> &'static str {
        match self {
            LspOp::Definition => "textDocument/definition",
            LspOp::References => "textDocument/references",
            LspOp::Implementation => "textDocument/implementation",
            LspOp::Hover => "textDocument/hover",
            LspOp::Rename => "textDocument/rename",
            LspOp::CodeAction => "textDocument/codeAction",
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
    /// C2：`textDocument/publishDiagnostics` 推送缓存（uri → 最近一次推送
    /// 的完整列表）。诊断是推送语义且每次推送都带该 uri 的**全量**列表，所以
    /// 直接整表替换。std Mutex（纳秒级 insert/remove，同 last_used 惯例）。
    diagnostics: std::sync::Mutex<HashMap<String, Vec<proto::Diagnostic>>>,
}

struct Inner {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
    /// C1：已 `didOpen` 的文档 uri → 已发送的最新 version。会话死亡/被
    /// reap 时随 Inner 一起消失——新会话对空集合，下一个 touch 自然重发
    /// didOpen（服务器状态与新会话一致）。
    open_docs: HashMap<String, i64>,
}

impl Session {
    /// C2：把一次 publishDiagnostics 推送写入缓存（整表覆盖该 uri）。
    /// 键用 [`proto::uri_key`] 规范化——服务器回显的是它自己规范化后的
    /// URI 形态（rust-analyzer 的 url crate 会把 Windows 盘符小写），与
    /// 我们 didOpen 发射的字符串可能字面不同；双端都落 WHATWG 规范键后
    /// 精确匹配才成立（2026-09-05 实机闭环验证根修）。
    fn record_diagnostics(&self, uri: &str, diags: Vec<proto::Diagnostic>) {
        let key = proto::uri_key(uri);
        self.diagnostics.lock().unwrap().insert(key, diags);
    }

    /// C2：某 uri 的当前诊断快照（无推送过 = 空）。查找键同
    /// [`Self::record_diagnostics`] 规范化。
    fn diagnostics_for_uri(&self, uri: &str) -> Vec<proto::Diagnostic> {
        let key = proto::uri_key(uri);
        self.diagnostics
            .lock()
            .unwrap()
            .get(&key)
            .cloned()
            .unwrap_or_default()
    }

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
                                && !err.is_null()
                            {
                                if proto::is_transient_error(err) {
                                    // Re-send with a fresh id; on the
                                    // last attempt the `break` falls out
                                    // of the for-loop to the descriptive
                                    // "kept cancelling" error below (a
                                    // bare -32800 would tell the model
                                    // nothing about the retry budget).
                                    break;
                                }
                                return Err(format!("server error on {method}: {err}"));
                            }
                            return Ok(resp.get("result").cloned().unwrap_or(Value::Null));
                        }
                        proto::Incoming::ServerRequest { id, method } => {
                            let reply =
                                proto::response_ok(id, proto::default_server_response(&method));
                            write_message(&mut inner, &reply).await?;
                            // keep waiting for our own response
                        }
                        proto::Incoming::Notification { method } => {
                            // C2：publishDiagnostics 推送进缓存（诊断是推送
                            // 语义，只有我们读 stdout 时才被消费）；其余
                            // progress/log 噪音照旧跳过。
                            if method == "textDocument/publishDiagnostics"
                                && let Some((uri, diags)) = proto::parse_publish_diagnostics(
                                    incoming.get("params").unwrap_or(&Value::Null),
                                )
                            {
                                self.record_diagnostics(&uri, diags);
                            }
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

    /// C2：不发送任何请求，纯读服务器主动推送。publishDiagnostics 是推送
    /// 语义——只有当我们读 stdout 时才会被消费；编辑后服务器异步推（可能
    /// 连推多轮增量收敛），这里读到一条消息就把 quiet 窗口重新计时，连续
    /// `quiet` 无消息（≈ 编辑器的 debounce 收敛）或总时长到 `max` 兜底即
    /// 返回。途中的 server→client 请求照 never-ignore 政策回应（有的服务
    /// 器会阻塞等回复）；读流出错（服务器死了）同样终止——不会再有推送。
    ///
    /// C3 实机验证修出的冷启动缺口（2026-09-05）：**首条非空
    /// publishDiagnostics 到达前** quiet 收敛不生效——（a）冷启动服务器
    /// （rust-analyzer 首析秒级）在 quiet 窗口内推不出任何东西，旧的
    /// 「单窗零消息即返回」把首推判成「已收敛」漏掉；（b）rust-analyzer
    /// 工作区加载完成时**先推一条空数组**（清屏语义），随后几秒才推真
    /// 诊断——若空推送也算「见过」，一个 progress 间隙的 150ms 静默就
    /// 会在真诊断之前提前返回空。故只有**非空**推送解锁 quiet 收敛：
    /// 错误文件（热服务器）非空推送毫秒级到达照常快收敛；干净文件/
    /// 清屏推送等满 `max` 兜底返回空——等价结果（无错可报），代价是
    /// `max` 时长的等待税（默认 2000ms，可配）。LSP 规范要求服务器
    /// 分析完推送（哪怕空数组），等待始终有界。
    async fn drain_pushes(&self, quiet: Duration, max: Duration) {
        let start = Instant::now();
        let mut saw_nonempty_push = false;
        loop {
            if start.elapsed() >= max {
                return;
            }
            let remaining = max.saturating_sub(start.elapsed());
            let window = quiet.min(remaining);
            let got = {
                let mut inner = self.inner.lock().await;
                tokio::time::timeout(window, read_message(&mut inner)).await
            };
            match got {
                Ok(Ok(msg)) => match proto::classify(&msg, -1) {
                    proto::Incoming::ServerRequest { id, method } => {
                        let mut inner = self.inner.lock().await;
                        let _ = write_message(
                            &mut inner,
                            &proto::response_ok(id, proto::default_server_response(&method)),
                        )
                        .await;
                    }
                    proto::Incoming::Notification { method } => {
                        if method == "textDocument/publishDiagnostics"
                            && let Some((uri, diags)) = proto::parse_publish_diagnostics(
                                msg.get("params").unwrap_or(&Value::Null),
                            )
                        {
                            // 空推送=清屏语义，不解锁收敛（见函数注释 b）。
                            let nonempty = !diags.is_empty();
                            self.record_diagnostics(&uri, diags);
                            if nonempty {
                                saw_nonempty_push = true;
                            }
                        }
                    }
                    // 别的（陈旧 id 的响应）——忽略。
                    proto::Incoming::Response(_) => {}
                },
                // 非空推送未到 → 继续等（冷启动/清屏后待真诊断，max 兜底；
                // timeout 已耗掉整窗，continue 无自旋）。
                Err(_) if !saw_nonempty_push => {}
                // quiet 窗口内没有消息（已收敛）/ 读流出错（服务器死了）。
                _ => return,
            }
        }
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
            && name.trim().eq_ignore_ascii_case("content-length")
        {
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
        if matches!(op, LspOp::Rename | LspOp::CodeAction) {
            // 写型/列表型走专用入口（rename 的结果要过安全闸后落盘，
            // codeAction 带诊断上下文）——不许从这条格式化捷径溜过去。
            return Err(format!(
                "op {:?} must go through the dedicated manager entry point",
                op
            ));
        }
        let mut params = json!({
            "textDocument": {"uri": proto::path_to_uri(path)},
            "position": {"line": line, "character": character},
        });
        if op == LspOp::References {
            params["context"] = json!({"includeDeclaration": true});
        }
        let result = self.request_with_recovery(op, path, params).await?;
        Ok(format_result(op, &result))
    }

    /// Preamble + request shared by `query` / `rename` / `code_actions`:
    /// resolve language → spec → file existence → server on PATH → project
    /// root, reap idle sessions, get-or-spawn, then send the request with
    /// the respawn-once recovery for mid-request transport death. Returns
    /// the raw JSON result.
    async fn request_with_recovery(
        &self,
        op: LspOp,
        path: &Path,
        params: Value,
    ) -> Result<Value, String> {
        let Some(lang) = registry::lang_for_path(path) else {
            let supported: Vec<&str> = registry::SERVERS.iter().map(|s| s.lang.label()).collect();
            return Err(format!(
                "unsupported file type: {} (supported languages: {})",
                path.display(),
                supported.join(", ")
            ));
        };
        let Some(spec) = registry::spec_for(lang) else {
            return Err(format!(
                "no language server configured for {}",
                lang.label()
            ));
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

        let session = self.get_or_spawn(lang, &root, &server_path).await?;
        *session.last_used.lock().unwrap() = Instant::now();
        match session
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
                fresh
                    .request(op.method(), params, self.request_timeout)
                    .await
            }
            Err(e) => Err(e),
        }
    }

    /// C7（devtool-upgrade 阶段 6）：`textDocument/rename` → 解析
    /// WorkspaceEdit → 逐文件读盘应用 TextEdit。**本方法不写盘**——返回
    /// 每个文件的新全文，由调用方（lsp_tool）统一过安全 8 层审批后再落盘
    /// （两阶段：先全部过闸、再全部写入，避免半改名状态落到磁盘）。
    pub async fn rename(
        &self,
        path: &Path,
        line: u32,
        character: u32,
        new_name: &str,
    ) -> Result<RenameOutcome, String> {
        if new_name.trim().is_empty() {
            return Err("new_name is empty".to_string());
        }
        let params = json!({
            "textDocument": {"uri": proto::path_to_uri(path)},
            "position": {"line": line, "character": character},
            "newName": new_name,
        });
        let result = self
            .request_with_recovery(LspOp::Rename, path, params)
            .await?;
        let file_edits = proto::parse_workspace_edit(&result);
        if file_edits.is_empty() {
            // null/空 WorkspaceEdit = 服务器判定该位置不可改名（或改名无
            // 任何效果）——诚实报错而非空成功。
            return Err(
                "server returned no edits for this rename (symbol may not be renameable at this position)"
                    .to_string(),
            );
        }
        let mut files = Vec::new();
        let mut errors = Vec::new();
        for fe in file_edits {
            // read_to_string：非 UTF-8 文件诚实进 errors（不静默跳过）。
            let content = match std::fs::read_to_string(&fe.path) {
                Ok(c) => c,
                Err(e) => {
                    errors.push(format!("{}: read failed: {e}", fe.path));
                    continue;
                }
            };
            match proto::apply_text_edits(&content, &fe.edits) {
                Ok(new_text) => files.push(AppliedFileEdit {
                    path: fe.path,
                    new_text,
                    edit_count: fe.edits.len(),
                }),
                Err(e) => errors.push(format!("{}: {e}", fe.path)),
            }
        }
        if files.is_empty() && !errors.is_empty() {
            return Err(errors.join("; "));
        }
        Ok(RenameOutcome { files, errors })
    }

    /// C7：`textDocument/codeAction` — 列出当前位置的 quickfix。上下文
    /// 带该文件当前诊断缓存（C2），让服务器给出针对性的修复动作。只列
    /// 表不执行（apply 后置，见计划 C7 规格）。
    pub async fn code_actions(
        &self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> Result<Vec<proto::CodeActionInfo>, String> {
        let diagnostics = self.diagnostics_for(path).await;
        let params = json!({
            "textDocument": {"uri": proto::path_to_uri(path)},
            "position": {"line": line, "character": character},
            "context": {
                "diagnostics": diagnostics
                    .iter()
                    .map(|d| d.to_json())
                    .collect::<Vec<_>>(),
                "only": ["quickfix"],
            },
        });
        let result = self
            .request_with_recovery(LspOp::CodeAction, path, params)
            .await?;
        Ok(proto::parse_code_actions(&result))
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
            return Err(format!(
                "no language server configured for {}",
                lang.label()
            ));
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
            open_docs: HashMap::new(),
        }),
        diagnostics: std::sync::Mutex::new(HashMap::new()),
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
    let _ = session.request_no_wait("initialized", json!({})).await;
    Ok(session)
}

impl Session {
    /// Fire a notification (no response expected).
    async fn request_no_wait(&self, method: &str, params: Value) -> Result<(), String> {
        let mut inner = self.inner.lock().await;
        write_message(&mut inner, &proto::notification(method, params)).await
    }
}

impl LspManager {
    /// C1（devtool-upgrade 阶段 2）：把磁盘上的文件全文同步给语言服务器
    /// ——未 open 过则 `textDocument/didOpen`，已 open 则 `textDocument/didChange`
    /// （full 同步）。C3 编辑后诊断回灌的入口之一。
    pub async fn touch_file(&self, path: &Path) -> Result<(), String> {
        self.sync_doc(path, None).await
    }

    /// C1：工具刚写完的内容直接同步（文本来自参数，免重读磁盘）。
    pub async fn notify_change(&self, path: &Path, new_text: &str) -> Result<(), String> {
        self.sync_doc(path, Some(new_text)).await
    }

    /// didOpen/didChange 共同实现。尽力而为语义：文件类型不支持、该语言
    /// 没配服务器、服务器不在 PATH —— 三者都静默 `Ok(())`（诊断闭环是
    /// 优化不是依赖，语言服务器缺席绝不能拖垮编辑路径）；会话 spawn 失败
    /// / 写管道失败仍返回 Err（调用方记日志决定后续）。
    async fn sync_doc(&self, path: &Path, text_override: Option<&str>) -> Result<(), String> {
        let Some(lang) = registry::lang_for_path(path) else {
            return Ok(());
        };
        let Some(spec) = registry::spec_for(lang) else {
            return Ok(());
        };
        let Some(server_path) = registry::find_command(spec.command) else {
            return Ok(());
        };
        let text = match text_override {
            Some(t) => t.to_string(),
            None => std::fs::read_to_string(path)
                .map_err(|e| format!("read for LSP sync failed: {e}"))?,
        };
        let root = find_root(path);
        self.reap_idle().await;
        let session = self.get_or_spawn(lang, &root, &server_path).await?;
        *session.last_used.lock().unwrap() = Instant::now();

        let uri = proto::path_to_uri(path);
        let mut inner = session.inner.lock().await;
        match inner.open_docs.get_mut(&uri) {
            None => {
                // didOpen：version 从 1 起（LSP 惯例）。
                inner.open_docs.insert(uri.clone(), 1);
                let note = proto::notification(
                    "textDocument/didOpen",
                    json!({
                        "textDocument": {
                            "uri": uri,
                            "languageId": lang.lsp_language_id(),
                            "version": 1,
                            "text": text,
                        },
                    }),
                );
                write_message(&mut inner, &note).await
            }
            Some(version) => {
                // didChange：full 文本同步 + version 单调递增（服务器据此
                // 丢弃乱序通知）。
                *version += 1;
                let note = proto::notification(
                    "textDocument/didChange",
                    json!({
                        "textDocument": {"uri": uri, "version": *version},
                        "contentChanges": [{"text": text}],
                    }),
                );
                write_message(&mut inner, &note).await
            }
        }
    }

    /// C2：某文件的当前诊断快照（被动读缓存；无会话/不支持类型 = 空）。
    /// 诊断按 uri 匹配——双端都过 [`proto::uri_key`] WHATWG 规范化：
    /// 服务器（rust-analyzer 等 url-crate 系）回显的是它规范化后的形态
    /// （Windows 盘符小写），与本端发射字符串字面不同也能对上
    /// （2026-09-05 实机闭环验证根修）。
    pub async fn diagnostics_for(&self, path: &Path) -> Vec<proto::Diagnostic> {
        let Some(lang) = registry::lang_for_path(path) else {
            return vec![];
        };
        let root = find_root(path);
        let session = {
            let sessions = self.sessions.lock().await;
            sessions.get(&(lang, root)).cloned()
        };
        match session {
            Some(s) => s.diagnostics_for_uri(&proto::path_to_uri(path)),
            None => vec![],
        }
    }

    /// C2：等待并收集某文件的诊断。无会话（文档从未 touch/notify 同步过）
    /// 诚实返回空——不 spawn，等待语义只对服务器已见过的文档成立；C3 的
    /// 调用序是编辑 → notify_change → wait，会话必然已存在。
    pub async fn wait_for_diagnostics(
        &self,
        path: &Path,
        quiet_ms: u64,
        max_ms: u64,
    ) -> Vec<proto::Diagnostic> {
        let Some(lang) = registry::lang_for_path(path) else {
            return vec![];
        };
        let root = find_root(path);
        let session = {
            let sessions = self.sessions.lock().await;
            sessions.get(&(lang, root)).cloned()
        };
        let Some(session) = session else {
            return vec![];
        };
        *session.last_used.lock().unwrap() = Instant::now();
        session
            .drain_pushes(
                Duration::from_millis(quiet_ms),
                Duration::from_millis(max_ms),
            )
            .await;
        session.diagnostics_for_uri(&proto::path_to_uri(path))
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
    const MARKERS: [&str; 5] = [
        ".git",
        "Cargo.toml",
        "package.json",
        "pyproject.toml",
        "go.mod",
    ];
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
        out.push_str(&format!(
            "{}. {}:{}:{}\n",
            i + 1,
            loc.path,
            loc.line,
            loc.character
        ));
    }
    out
}

fn method_noun(op: LspOp) -> &'static str {
    match op {
        LspOp::Definition => "definitions",
        LspOp::References => "references",
        LspOp::Implementation => "implementations",
        LspOp::Hover => "hover",
        // query()/format_result 不接这两个 op（走 rename()/code_actions()
        // 专用入口，query 里有显式守卫）；分支只为穷尽性存在。
        LspOp::Rename => "renames",
        LspOp::CodeAction => "code actions",
    }
}

/// C7：单文件改名应用结果——新全文已按 TextEdit 算好，**尚未写盘**。
#[derive(Debug, Clone)]
pub struct AppliedFileEdit {
    pub path: String,
    pub new_text: String,
    pub edit_count: usize,
}

/// C7：rename 全量结果。`files` 是成功应用的文件（待调用方过闸+落盘）；
/// `errors` 是读盘/应用失败的文件（不整体失败——一个文件坏不该挡住其余
/// 文件的审批，但两个列表都必须如实上报，调用方决定取舍）。
#[derive(Debug, Clone)]
pub struct RenameOutcome {
    pub files: Vec<AppliedFileEdit>,
    pub errors: Vec<String>,
}

#[cfg(test)]
mod tests;
