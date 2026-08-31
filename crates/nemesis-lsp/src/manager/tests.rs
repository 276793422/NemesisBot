//! Manager unit tests (pure helpers) + the live rust-analyzer acceptance
//! test (L1/U19 验收 ① and ③). Live tests skip with a stderr note when
//! rust-analyzer is not installed — the skip itself mirrors the
//! registration semantics (no server ⇒ no capability).

// 刻意设计：本文件测试用进程级串行锁（GLOBAL_STATE_LOCK 等 env/资源互斥锁）
// 保护环境操作，guard 必须跨 async 测试体的 await 持有；#[tokio::test] 每个
// 测试独立 current_thread runtime，持锁方在自己线程上恢复运行，不会死锁。
// 测试域统一豁免（逐处 allow ~200 个不现实）。
#![allow(clippy::await_holding_lock)]

use super::*;
use crate::registry::Lang;

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

#[test]
fn lsp_op_parse_and_method() {
    assert_eq!(LspOp::parse("definition"), Some(LspOp::Definition));
    assert_eq!(LspOp::parse("references"), Some(LspOp::References));
    assert_eq!(LspOp::parse("implementation"), Some(LspOp::Implementation));
    assert_eq!(LspOp::parse("hover"), Some(LspOp::Hover));
    assert_eq!(LspOp::parse("rename"), None);
    assert_eq!(LspOp::Definition.method(), "textDocument/definition");
    assert_eq!(LspOp::References.method(), "textDocument/references");
}

#[test]
fn transport_error_detection() {
    assert!(is_transport_error("server closed the stream"));
    assert!(is_transport_error("write to server stdin failed: broken pipe"));
    assert!(is_transport_error("read header line failed: eof"));
    assert!(!is_transport_error("server error on textDocument/definition: {code:-32601}"));
    assert!(!is_transport_error("LSP request textDocument/hover timed out after 120s"));
}

#[test]
fn find_root_walks_to_marker_then_falls_back() {
    let tmp = tempfile::tempdir().unwrap();
    // Marker dir: nested file resolves to the marker dir.
    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(proj.join("src/inner")).unwrap();
    std::fs::write(proj.join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
    let lib = proj.join("src/inner/mod.rs");
    std::fs::write(&lib, "// x\n").unwrap();
    assert_eq!(find_root(&lib), proj);

    // No marker anywhere: falls back to the file's own directory — assert
    // strictly only when no marker lurks somewhere above the temp tree
    // (machine-dependent: a stray .git/Cargo.toml in an ancestor would
    // legitimately win).
    let bare = tmp.path().join("loose.py");
    std::fs::write(&bare, "x = 1\n").unwrap();
    let marker_above = bare.parent().unwrap().ancestors().skip(1).any(|d| {
        [".git", "Cargo.toml", "package.json", "pyproject.toml", "go.mod"]
            .iter()
            .any(|m| d.join(m).exists())
    });
    if !marker_above {
        assert_eq!(find_root(&bare), bare.parent().unwrap());
    }
}

#[test]
fn format_result_renders_locations_and_hover() {
    let def = serde_json::json!([{
        "uri": "file:///a/b.rs",
        "range": {"start": {"line": 4, "character": 7}, "end": {"line": 4, "character": 9}}
    }]);
    let out = format_result(LspOp::Definition, &def);
    assert!(out.contains("1 definitions"), "{out}");
    assert!(out.contains("/a/b.rs:4:7"), "{out}");

    let empty = format_result(LspOp::References, &serde_json::Value::Null);
    assert!(empty.contains("no references found"), "{empty}");

    let hover = format_result(
        LspOp::Hover,
        &serde_json::json!({"contents": {"kind": "markdown", "value": "docs here"}}),
    );
    assert_eq!(hover, "docs here");
    assert_eq!(
        format_result(LspOp::Hover, &serde_json::Value::Null),
        "(no hover information at this position)"
    );
}

// ---------------------------------------------------------------------------
// Live acceptance (rust-analyzer required; skips otherwise)
// ---------------------------------------------------------------------------

/// Fixture crate for the live test. Layout + exact line/col anchors:
///
/// ```text
/// 0: pub fn fixture_answer() -> u32 { 42 }
/// 1:
/// 2: pub trait Greeter { fn greet(&self) -> String; }
/// 3:
/// 4: pub struct English;
/// 5: impl Greeter for English {
/// 6:     fn greet(&self) -> String { "hello".to_string() }
/// 7: }
/// 8:
/// 9: pub fn caller() -> u32 {
/// 10:     let v = fixture_answer();
/// 11:     let g = English;
/// 12:     let _s = g.greet();
/// 13:     v
/// 14: }
/// ```
const FIXTURE_LIB_RS: &str = "\
pub fn fixture_answer() -> u32 { 42 }

pub trait Greeter { fn greet(&self) -> String; }

pub struct English;
impl Greeter for English {
    fn greet(&self) -> String { \"hello\".to_string() }
}

pub fn caller() -> u32 {
    let v = fixture_answer();
    let g = English;
    let _s = g.greet();
    v
}
";

fn fixture_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"lsp_fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), FIXTURE_LIB_RS).unwrap();
    dir
}

/// rust-analyzer answers definition queries with null until indexing
/// finishes — retry with backoff instead of treating the cold-start null
/// as "no definition".
async fn query_until(
    mgr: &LspManager,
    op: LspOp,
    path: &Path,
    line: u32,
    character: u32,
    want_nonempty: bool,
) -> String {
    let mut last = String::new();
    for _ in 0..30 {
        match mgr.query(op, path, line, character).await {
            Ok(res) => {
                let empty = res.starts_with("no ") || res.starts_with("(no ");
                if !want_nonempty || !empty {
                    return res;
                }
                last = res;
            }
            Err(e) => last = format!("ERR: {e}"),
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    last
}

/// Serialize the two live rust-analyzer tests: each spawns a real server
/// + cargo metadata; in parallel (default test threads) they contend for
///   CPU and stretch cold-start past any sane retry window. (tokio's Mutex
///   isn't const-constructible, hence the OnceLock.)
static LIVE_LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();

async fn live_lock() -> tokio::sync::MutexGuard<'static, ()> {
    LIVE_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await
}

/// L1/U19 验收 ①: four read-only operations produce correct results on a
/// real (tiny) cargo repo with real symbols, through a real rust-analyzer
/// process. 验收 ③ (lifecycle) is asserted at the tail.
#[tokio::test]
async fn rust_analyzer_four_ops_on_real_repo() {
    if !registry::server_available(Lang::Rust) {
        eprintln!("SKIP: rust-analyzer not installed — live LSP acceptance not run");
        return;
    }
    let _live = live_lock().await;
    let dir = fixture_repo();
    let lib = dir.path().join("src/lib.rs");

    let mgr = LspManager::new(Some(Duration::from_secs(60)), Some(Duration::from_secs(600)));

    // definition: use site (line 10, col of `fixture_answer`) → def line 0.
    let def = query_until(&mgr, LspOp::Definition, &lib, 10, 13, true).await;
    assert!(
        def.contains("lib.rs:0:"),
        "definition should land on the fn at line 0, got: {def}"
    );

    // references: fn def (line 0, col 8) → at least the def + the call.
    let refs = query_until(&mgr, LspOp::References, &lib, 0, 8, true).await;
    assert!(refs.contains("lib.rs:10:"), "references should include the call site, got: {refs}");
    assert!(refs.contains("lib.rs:0:"), "references should include the declaration, got: {refs}");

    // implementation: trait method decl (line 2, col 23 `greet`) → impl line 6.
    let impls = query_until(&mgr, LspOp::Implementation, &lib, 2, 23, true).await;
    assert!(
        impls.contains("lib.rs:6:"),
        "implementation should land on the impl at line 6, got: {impls}"
    );

    // hover: fn def → signature text mentions the fn name.
    let hover = query_until(&mgr, LspOp::Hover, &lib, 0, 8, true).await;
    assert!(hover.contains("fixture_answer"), "hover should name the function, got: {hover}");

    // Lifecycle (验收 ③): one cached session; shutdown_all closes it.
    assert_eq!(mgr.session_count().await, 1);
    assert_eq!(mgr.shutdown_all().await, 1);
    assert_eq!(mgr.session_count().await, 0);
}

/// L1/U19 验收 ③: idle sessions are reaped lazily, and a reaped session
/// transparently respawns on the next query.
///
/// Determinism notes: the retry cadence (500ms) sits well under the idle
/// threshold (10s) so the warm-up session survives indexing retries even
/// on a loaded machine; expiry is forced by back-dating `last_used`
/// (child-module access) instead of real sleeping — the reap mechanics
/// are what's under test, not the clock.
#[tokio::test]
async fn idle_sessions_are_reaped_and_respawn() {
    if !registry::server_available(Lang::Rust) {
        eprintln!("SKIP: rust-analyzer not installed — live LSP reaper test not run");
        return;
    }
    let _live = live_lock().await;
    let dir = fixture_repo();
    let lib = dir.path().join("src/lib.rs");
    let mgr = LspManager::new(Some(Duration::from_secs(60)), Some(Duration::from_secs(10)));

    let hover = query_until_fast(&mgr, LspOp::Hover, &lib, 0, 8).await;
    assert!(hover.contains("fixture_answer"), "warm-up query should work: {hover}");
    assert_eq!(mgr.session_count().await, 1);

    // Back-date last_used past the threshold → the lazy sweep must reap it.
    {
        let sessions = mgr.sessions.lock().await;
        for s in sessions.values() {
            *s.last_used.lock().unwrap() =
                Instant::now() - Duration::from_secs(60);
        }
    }
    assert_eq!(mgr.reap_idle().await, 1, "idle session should be reaped");
    assert_eq!(mgr.session_count().await, 0);

    // Re-query: fresh session spawns transparently and still answers.
    let again = query_until_fast(&mgr, LspOp::Hover, &lib, 0, 8).await;
    assert!(again.contains("fixture_answer"), "respawned session should answer: {again}");
    assert_eq!(mgr.session_count().await, 1);
    assert_eq!(mgr.shutdown_all().await, 1);
}

/// query_until variant with a fast cadence for short-idle managers: 500ms
/// sleeps keep the session under its idle threshold while waiting for a
/// cold server to finish loading.
async fn query_until_fast(
    mgr: &LspManager,
    op: LspOp,
    path: &Path,
    line: u32,
    character: u32,
) -> String {
    let mut last = String::new();
    for _ in 0..60 {
        match mgr.query(op, path, line, character).await {
            Ok(res) => {
                let empty = res.starts_with("no ") || res.starts_with("(no ");
                if !empty {
                    return res;
                }
                last = res;
            }
            Err(e) => last = format!("ERR: {e}"),
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    last
}

/// Per-language availability errors are clear, not panics — the
/// registered-tool-but-this-language's-server-missing path.
#[tokio::test]
async fn unsupported_file_and_missing_file_errors() {
    let mgr = LspManager::new(None, None);
    let err = mgr
        .query(LspOp::Definition, Path::new("/definitely/nope.md"), 0, 0)
        .await
        .unwrap_err();
    assert!(err.contains("unsupported file type"), "{err}");

    let err = mgr
        .query(LspOp::Definition, Path::new("/definitely/missing.rs"), 0, 0)
        .await
        .unwrap_err();
    assert!(err.contains("file does not exist"), "{err}");
}

// ---------------------------------------------------------------------------
// Deterministic fake-server tests (M7 补测): a python LSP server answering
// framed JSON-RPC over stdio, planted on PATH as a `gopls` shim. This pins
// the `Session::request` retry loop (transient -32800), the per-request
// timeout, and the mid-request-death respawn — behaviors the live
// rust-analyzer tests can only exercise nondeterministically.
// ---------------------------------------------------------------------------

/// Process-global env writers must share one lock (env-test-race-lock-pattern).
static FAKE_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Holds the env lock and restores PATH on drop (even on assertion failure).
/// The lock must live for the whole test: the PATH mutation outlives the
/// helper that made it, so the helper's own scope is not enough.
struct PathRestore {
    _lock: std::sync::MutexGuard<'static, ()>,
    orig: String,
}
impl Drop for PathRestore {
    fn drop(&mut self) {
        unsafe {
            std::env::set_var("PATH", &self.orig);
        }
    }
}

const FAKE_SERVER_PY: &str = r#"
import sys, os, json, time

MODE = "default"
for a in sys.argv[1:]:
    if a.startswith("--mode="):
        MODE = a.split("=", 1)[1]

def here(name):
    return os.path.join(os.path.dirname(os.path.abspath(__file__)), name)

def read_msg():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.strip()
        if not line:
            break
        if line.lower().startswith(b"content-length:"):
            length = int(line.split(b":", 1)[1].strip())
    if length is None:
        return None
    return json.loads(sys.stdin.buffer.read(length))

def send(obj):
    body = json.dumps(obj).encode()
    if MODE == "extra-header":
        # S1: one extra header with a colon (ignored) and one bare line with
        # no colon (skipped entirely) — the client must tolerate both.
        sys.stdout.buffer.write(b"Content-Type: application/vscode-jsonrpc; charset=utf-8\r\n")
        sys.stdout.buffer.write(b"X-S1-Bare-Line-No-Colon\r\n")
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body))
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()

cancels_seen = 0

def answer_hover(mid):
    global cancels_seen
    if MODE == "hang":
        # Long enough to outlive the 1s request timeout + 5s shutdown
        # budget, short enough that the orphaned grandchild (kill() only
        # reaps the cmd shim, not python) exits on its own and lets the
        # test process terminate.
        time.sleep(20)
    if MODE == "null-error":
        # S1: the error field is present but JSON-null — must NOT be treated
        # as an error; the client falls through to the result.
        send({"jsonrpc": "2.0", "id": mid, "error": None, "result": {"contents": {"kind": "markdown", "value": "null_error_hover"}}})
        return
    if MODE == "cancel-always":
        send({"jsonrpc": "2.0", "id": mid, "error": {"code": -32800, "message": "RequestCancelled"}})
        return
    if MODE == "cancel-then-ok":
        cancels_seen += 1
        if cancels_seen <= 2:
            send({"jsonrpc": "2.0", "id": mid, "error": {"code": -32800, "message": "RequestCancelled"}})
            return
    send({"jsonrpc": "2.0", "id": mid, "result": {"contents": {"kind": "markdown", "value": "fake_hover_value"}}})

def handle(msg):
    method = msg.get("method")
    mid = msg.get("id")
    if method == "initialize":
        # Server->client request the client must answer while waiting for
        # its own initialize response (never-ignore policy).
        send({"jsonrpc": "2.0", "id": 9001, "method": "workspace/configuration", "params": {"items": []}})
        send({"jsonrpc": "2.0", "id": mid, "result": {"capabilities": {}}})
        return
    if method == "initialized" or method == "$/setTrace":
        return
    if method == "shutdown":
        open(here("shutdown_seen.marker"), "w").write("seen")
        send({"jsonrpc": "2.0", "id": mid, "result": None})
        return
    if method == "exit":
        sys.exit(0)
    if method == "textDocument/hover":
        answer_hover(mid)
        return
    if method in ("textDocument/definition", "textDocument/references", "textDocument/implementation"):
        send({"jsonrpc": "2.0", "id": mid, "result": [{
            "uri": "file:///fake/a.go",
            "range": {"start": {"line": 3, "character": 4}, "end": {"line": 3, "character": 8}},
        }]})
        return
    if mid is not None:
        send({"jsonrpc": "2.0", "id": mid, "error": {"code": -32601, "message": "method not found"}})

if MODE == "die-once":
    # Only the FIRST process dies right after initialize; every later
    # process (the respawned session) behaves normally.
    died = os.path.exists(here("died_once.marker"))
    while True:
        m = read_msg()
        if m is None:
            break
        if died:
            handle(m)
            continue
        if m.get("method") == "initialize" and "id" in m:
            send({"jsonrpc": "2.0", "id": m["id"], "result": {"capabilities": {}}})
            open(here("died_once.marker"), "w").write("1")
            sys.exit(0)
else:
    while True:
        m = read_msg()
        if m is None:
            break
        handle(m)
"#;

/// Plant `fake_lsp_server.py` + a `gopls` shim (mode baked in) in a fresh
/// temp dir, and prepend that dir to PATH for the test's duration.
/// `gopls` is chosen because the machine running tests has no real gopls
/// on PATH (verified 2026-08-25); even if one existed, the prepended dir
/// wins resolution order.
fn plant_fake_gopls(mode: &str) -> (tempfile::TempDir, PathRestore) {
    let lock = FAKE_ENV_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("fake_lsp_server.py"), FAKE_SERVER_PY).unwrap();

    let py = dir.path().join("fake_lsp_server.py").to_string_lossy().to_string();
    #[cfg(windows)]
    std::fs::write(
        dir.path().join("gopls.cmd"),
        format!("@python \"{py}\" --mode={mode}\r\n"),
    )
    .unwrap();
    #[cfg(not(windows))]
    {
        std::fs::write(
            dir.path().join("gopls"),
            format!("#!/bin/sh\nexec python3 \"{py}\" --mode={mode}\n"),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            dir.path().join("gopls"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }

    // go.mod marker makes find_root resolve to the temp dir deterministically.
    std::fs::write(dir.path().join("go.mod"), "module faketest\n\ngo 1.25\n").unwrap();
    std::fs::write(dir.path().join("main.go"), "package main\n\nfunc main() {}\n").unwrap();

    let orig = std::env::var("PATH").unwrap_or_default();
    let new_path = std::env::join_paths(std::iter::once(dir.path().to_path_buf()).chain(
        std::env::split_paths(&orig),
    ))
    .unwrap()
    .to_string_lossy()
    .to_string();
    unsafe {
        std::env::set_var("PATH", &new_path);
    }
    (dir, PathRestore { _lock: lock, orig })
}

/// Happy path through the full `query` chain: initialize handshake (with a
/// server→client request answered mid-handshake), hover, a location op,
/// and the graceful shutdown handshake (marker proves the server saw it).
#[tokio::test]
async fn fake_server_full_query_chain_and_shutdown_handshake() {
    let (dir, _path) = plant_fake_gopls("default");
    let mgr = LspManager::new(Some(Duration::from_secs(30)), None);
    let go = dir.path().join("main.go");

    let hover = mgr.query(LspOp::Hover, &go, 0, 0).await.unwrap();
    assert!(hover.contains("fake_hover_value"), "{hover}");

    let def = mgr.query(LspOp::Definition, &go, 0, 9).await.unwrap();
    assert!(def.contains("1 definitions"), "{def}");
    assert!(def.contains("/fake/a.go:3:4"), "{def}");

    assert_eq!(mgr.session_count().await, 1, "same root must reuse one session");
    assert_eq!(mgr.shutdown_all().await, 1);
    assert!(
        dir.path().join("shutdown_seen.marker").exists(),
        "server should have received the shutdown request"
    );
}

/// -32800 on every attempt: exactly TRANSIENT_RETRIES retries, then the
/// explicit "kept cancelling" error (not a transport error, not a hang).
#[tokio::test]
async fn fake_server_transient_cancel_exhausts_retries() {
    let (dir, _path) = plant_fake_gopls("cancel-always");
    let mgr = LspManager::new(Some(Duration::from_secs(30)), None);
    let err = mgr
        .query(LspOp::Hover, &dir.path().join("main.go"), 0, 0)
        .await
        .unwrap_err();
    assert!(err.contains("kept cancelling"), "{err}");
    assert!(
        err.contains("5 retries"),
        "error should state the retry budget: {err}"
    );
    let _ = mgr.shutdown_all().await;
}

/// Transient cancels followed by success: the in-place retry recovers the
/// query — the caller never sees the cancellation.
#[tokio::test]
async fn fake_server_transient_cancel_then_retry_succeeds() {
    let (dir, _path) = plant_fake_gopls("cancel-then-ok");
    let mgr = LspManager::new(Some(Duration::from_secs(30)), None);
    let hover = mgr
        .query(LspOp::Hover, &dir.path().join("main.go"), 0, 0)
        .await
        .unwrap();
    assert!(hover.contains("fake_hover_value"), "{hover}");
    let _ = mgr.shutdown_all().await;
}

/// A server that never replies trips the per-request timeout (not a hang
/// past the budget). The server sleeps 20s — long enough to outlive both
/// the 1s request timeout and the 5s shutdown budget; `kill_tree` must
/// take the shim-wrapped python down with its cmd shim, or this test
/// (and any host process doing the same) stalls until the sleep expires.
#[tokio::test]
async fn fake_server_unresponsive_times_out() {
    let (dir, _path) = plant_fake_gopls("hang");
    let mgr = LspManager::new(Some(Duration::from_secs(1)), None);
    let err = mgr
        .query(LspOp::Hover, &dir.path().join("main.go"), 0, 0)
        .await
        .unwrap_err();
    assert!(err.contains("timed out"), "{err}");
    let _ = mgr.shutdown_all().await;
}

/// Server dies right after initialize: the first query hits a broken
/// transport, is detected as such, evicted, and retried once on a fresh
/// (healthy) session — the query still succeeds.
#[tokio::test]
async fn fake_server_death_mid_request_respawns_once() {
    let (dir, _path) = plant_fake_gopls("die-once");
    let mgr = LspManager::new(Some(Duration::from_secs(30)), None);
    let hover = mgr
        .query(LspOp::Hover, &dir.path().join("main.go"), 0, 0)
        .await
        .unwrap();
    assert!(hover.contains("fake_hover_value"), "{hover}");
    assert!(
        dir.path().join("died_once.marker").exists(),
        "first session must actually have died (proves the respawn path ran)"
    );
    assert_eq!(mgr.session_count().await, 1);
    let _ = mgr.shutdown_all().await;
}

// ---------------------------------------------------------------------------
// S1 补测（2026-08-26）：null-error 字段回退 / 额外头行容错 / PATH 无服务器
// ---------------------------------------------------------------------------

/// `"error": null` in a response: the field is present but JSON-null — the
/// client must fall through to the result instead of treating it as an error.
#[tokio::test]
async fn s1_fake_server_null_error_field_falls_through_to_result() {
    let (dir, _path) = plant_fake_gopls("null-error");
    let mgr = LspManager::new(Some(Duration::from_secs(30)), None);
    let hover = mgr
        .query(LspOp::Hover, &dir.path().join("main.go"), 0, 0)
        .await
        .unwrap();
    assert!(hover.contains("null_error_hover"), "{hover}");
    let _ = mgr.shutdown_all().await;
}

/// A server that prefixes every frame with an extra `Content-Type:` header
/// (has a colon, not Content-Length) and a bare header line (no colon at
/// all): the reader must skip both without derailing the framing.
#[tokio::test]
async fn s1_fake_server_extra_headers_are_skipped() {
    let (dir, _path) = plant_fake_gopls("extra-header");
    let mgr = LspManager::new(Some(Duration::from_secs(30)), None);
    let hover = mgr
        .query(LspOp::Hover, &dir.path().join("main.go"), 0, 0)
        .await
        .unwrap();
    assert!(hover.contains("fake_hover_value"), "{hover}");
    let _ = mgr.shutdown_all().await;
}

/// No gopls anywhere on (a filtered) PATH: query must fail fast with the
/// actionable "not installed" error instead of attempting a session spawn.
#[tokio::test]
async fn s1_missing_server_on_path_yields_not_installed_error() {
    let _lock = FAKE_ENV_LOCK.lock().unwrap();
    let orig = std::env::var("PATH").unwrap_or_default();
    // Keep every PATH entry that does not itself provide a gopls executable
    // (rust-analyzer and everything else stay resolvable for parallel tests).
    let filtered: Vec<_> = std::env::split_paths(&orig)
        .filter(|p| {
            ["gopls", "gopls.exe", "gopls.cmd", "gopls.bat", "gopls.com"]
                .iter()
                .all(|n| !p.join(n).exists())
        })
        .collect();
    unsafe {
        std::env::set_var(
            "PATH",
            std::env::join_paths(filtered)
                .unwrap()
                .to_string_lossy()
                .to_string(),
        );
    }

    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("go.mod"), "module s1miss\n\ngo 1.25\n").unwrap();
    let go = tmp.path().join("main.go");
    std::fs::write(&go, "package main\n\nfunc main() {}\n").unwrap();

    let mgr = LspManager::new(Some(Duration::from_secs(5)), None);
    let err = mgr.query(LspOp::Definition, &go, 0, 0).await.unwrap_err();
    assert!(err.contains("is not installed"), "{err}");
    assert!(err.contains("gopls"), "{err}");

    unsafe {
        std::env::set_var("PATH", &orig);
    }
}
