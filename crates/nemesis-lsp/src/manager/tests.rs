//! Manager unit tests (pure helpers) + the live rust-analyzer acceptance
//! test (L1/U19 验收 ① and ③). Live tests skip with a stderr note when
//! rust-analyzer is not installed — the skip itself mirrors the
//! registration semantics (no server ⇒ no capability).

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
/// CPU and stretch cold-start past any sane retry window. (tokio's Mutex
/// isn't const-constructible, hence the OnceLock.)
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
