//! I1 (devtool-upgrade 阶段 3) fs-watcher tests.
//!
//! Timing: watcher tests use a short debounce (150ms) and poll with a 5s
//! cap — notify delivers well within that on both Windows
//! (ReadDirectoryChangesW) and Linux (inotify); the poll cap keeps them
//! robust on loaded CI runners. Pure-logic tests (ignore table, section
//! rendering, loop buffer/window) are timing-free.

use super::*;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn cfg(extra: &[&str]) -> nemesis_config::FsWatcherConfig {
    nemesis_config::FsWatcherConfig {
        enabled: true,
        ignore: extra.iter().map(|s| s.to_string()).collect(),
    }
}

fn disabled_cfg() -> nemesis_config::FsWatcherConfig {
    nemesis_config::FsWatcherConfig {
        enabled: false,
        ignore: vec![],
    }
}

fn rel(p: &str) -> PathBuf {
    PathBuf::from(p)
}

// --- pure logic: ignore table ---

#[test]
fn test_is_ignored_builtins() {
    assert!(is_ignored(&rel(".git/HEAD"), &[]));
    assert!(is_ignored(&rel("node_modules/pkg/index.js"), &[]));
    assert!(is_ignored(&rel("target/debug/foo.exe"), &[]));
    assert!(is_ignored(&rel("logs/sessions/x.jsonl"), &[]));
    assert!(is_ignored(&rel("sub/deep/uploads/img.png"), &[]));
    // *.jsonl anywhere; *.db covers sqlite sidecars by dir too.
    assert!(is_ignored(&rel("session_log.jsonl"), &[]));
    assert!(is_ignored(&rel("board.db-wal"), &[]));
    assert!(is_ignored(&rel("board.db-shm"), &[]));
    // Ordinary source files pass.
    assert!(!is_ignored(&rel("src/main.rs"), &[]));
    assert!(!is_ignored(&rel("notes.txt"), &[]));
    assert!(!is_ignored(&rel("AGENTS.md"), &[]));
}

#[test]
fn test_is_ignored_custom_entries() {
    // Bare name → component match; *.ext → extension match.
    assert!(is_ignored(&rel("build/out.o"), &["build".to_string()]));
    assert!(is_ignored(&rel("x/bin.obj"), &["*.obj".to_string()]));
    assert!(!is_ignored(&rel("src/main.rs"), &["build".to_string()]));
}

#[test]
fn test_is_ignored_root_component_not_blamed() {
    // The caller strips the workspace root — the ROOT's own name (even
    // "target") must never ignore the whole tree.
    let extra: Vec<String> = vec![];
    assert!(!is_ignored(&rel("src/anything.rs"), &extra));
}

// --- pure logic: section rendering ---

#[test]
fn test_render_external_changes_section() {
    let s = render_external_changes_section(&["src/main.rs".to_string(), "notes.txt".to_string()]);
    assert!(s.starts_with("<external_changes>"));
    assert!(s.contains("</external_changes>"));
    assert!(s.contains("- src/main.rs"));
    assert!(s.contains("- notes.txt"));
    assert!(s.contains("外部修改了以下文件"));
}

// --- loop-side buffer + self-write window (no fs, no LLM) ---

/// Minimal provider so the tests can construct an AgentLoop (the loop's own
/// MockLlmProvider is private to its tests module).
struct IdleProvider;

#[async_trait::async_trait]
impl crate::r#loop::LlmProvider for IdleProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<crate::r#loop::LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<crate::r#loop::LlmResponse, String> {
        Err("not used".to_string())
    }
}

fn test_loop() -> std::sync::Arc<crate::r#loop::AgentLoop> {
    let config = crate::types::AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("test".to_string()),
        max_turns: 3,
        tools: vec![],
        models: std::collections::HashMap::new(),
    };
    std::sync::Arc::new(crate::r#loop::AgentLoop::new(
        Box::new(IdleProvider),
        config,
    ))
}

#[test]
fn test_external_change_buffer_push_drain_cap() {
    let arc = test_loop();
    for i in 0..(MAX_BUFFERED_CHANGES + 5) {
        arc.push_external_change(&format!("f{i}.txt"));
    }
    let drained = arc.drain_external_changes();
    assert_eq!(drained.len(), MAX_BUFFERED_CHANGES, "buffer capped");
    // One-shot: drained buffer stays empty.
    assert!(arc.drain_external_changes().is_empty());
}

#[test]
fn test_self_write_window_drops_recent_writes() {
    let arc = test_loop();
    arc.note_self_write("src/main.rs");
    // Same file (case-insensitive on the normalized form) inside the window
    // → dropped.
    arc.push_external_change("src/main.rs");
    arc.push_external_change("SRC/MAIN.RS");
    // Different file → kept.
    arc.push_external_change("src/other.rs");
    let drained = arc.drain_external_changes();
    assert_eq!(drained, vec!["src/other.rs".to_string()]);
}

#[test]
fn test_self_write_window_expires() {
    let arc = test_loop();
    arc.note_self_write("a.txt");
    // Age the entry past the window directly (no sleeping).
    let mut map = arc.recent_self_writes.lock();
    map.insert(
        "a.txt".to_string(),
        Instant::now() - SELF_WRITE_WINDOW - Duration::from_secs(1),
    );
    drop(map);
    arc.push_external_change("a.txt");
    assert_eq!(arc.drain_external_changes(), vec!["a.txt".to_string()]);
}

// --- end-to-end: watcher fires through the loop ---

#[test]
fn test_watcher_external_change_surfaces_through_loop() {
    let dir = tempfile::tempdir().unwrap();
    let arc = test_loop();
    arc.set_workspace_root(dir.path().to_path_buf());
    crate::r#loop::AgentLoop::start_fs_watcher(&arc, &cfg(&[]))
        .expect("watcher start must succeed");

    // Wait for the watcher to arm, then write an ordinary file.
    std::thread::sleep(Duration::from_millis(300));
    std::fs::write(dir.path().join("notes.txt"), "hello").unwrap();

    let changes = wait_for_change(&arc, "notes.txt");
    assert!(changes.iter().any(|p| p.contains("notes.txt")));
}

/// Poll the loop's buffer (drain keeps re-pushing is destructive — use a
/// helper that peeks via drain and accumulates).
fn wait_for_change(arc: &std::sync::Arc<crate::r#loop::AgentLoop>, needle: &str) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut acc: Vec<String> = Vec::new();
    while Instant::now() < deadline {
        let drained = arc.drain_external_changes();
        if drained.iter().any(|p| p.contains(needle)) {
            acc.extend(drained);
            return acc;
        }
        acc.extend(drained);
        std::thread::sleep(Duration::from_millis(50));
    }
    acc
}

#[test]
fn test_watcher_instruction_file_fires_not_external() {
    let dir = tempfile::tempdir().unwrap();
    let fired = Arc::new(Mutex::new(false));
    let (ext_tx, ext_rx) = mpsc::channel::<String>();

    let on_instr: Callback = {
        let fired = fired.clone();
        Arc::new(move || *fired.lock().unwrap() = true)
    };
    let on_ext: PathCallback = Arc::new(move |p| {
        let _ = ext_tx.send(p.to_string());
    });

    let handle = start_with_debounce(
        dir.path(),
        &cfg(&[]),
        Duration::from_millis(150),
        on_instr,
        on_ext,
    )
    .expect("start ok")
    .expect("enabled");
    std::thread::sleep(Duration::from_millis(300));
    std::fs::write(dir.path().join("AGENTS.md"), "# rules").unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    while !*fired.lock().unwrap() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(*fired.lock().unwrap(), "instruction callback must fire");
    // The instruction file must NOT surface as an external change.
    std::thread::sleep(Duration::from_millis(400));
    assert!(
        ext_rx.try_recv().is_err(),
        "instruction file must not surface as external change"
    );
    drop(handle);
}

#[test]
fn test_watcher_ignores_ignored_dir() {
    let dir = tempfile::tempdir().unwrap();
    let (tx, rx) = mpsc::channel::<String>();
    let on_instr: Callback = Arc::new(|| {});
    let on_ext: PathCallback = Arc::new(move |p| {
        let _ = tx.send(p.to_string());
    });
    let _handle = start_with_debounce(
        dir.path(),
        &cfg(&[]),
        Duration::from_millis(150),
        on_instr,
        on_ext,
    )
    .expect("start ok")
    .expect("enabled");
    std::thread::sleep(Duration::from_millis(300));
    std::fs::create_dir_all(dir.path().join("target")).unwrap();
    std::fs::write(dir.path().join("target/out.txt"), "x").unwrap();
    // Give the watcher ample time to (not) deliver.
    std::thread::sleep(Duration::from_millis(1500));
    assert!(
        rx.try_recv().is_err(),
        "ignored-dir writes must not surface"
    );
}

#[test]
fn test_watcher_disabled_config_returns_none() {
    let dir = tempfile::tempdir().unwrap();
    let on_instr: Callback = Arc::new(|| {});
    let on_ext: PathCallback = Arc::new(|_| {});
    let res = start_with_debounce(
        dir.path(),
        &disabled_cfg(),
        Duration::from_millis(150),
        on_instr,
        on_ext,
    )
    .expect("disabled is not an error");
    assert!(res.is_none(), "disabled config → no handle");
}
