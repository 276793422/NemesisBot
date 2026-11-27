//! (devtool-upgrade) fs-watcher tests.
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
    // 构建产物/虚拟环境目录扩充进共享真相表。
    assert!(is_ignored(&rel("build/out.o"), &[]));
    assert!(is_ignored(&rel("out/objs.o"), &[]));
    assert!(is_ignored(&rel("bin/app.exe"), &[]));
    assert!(is_ignored(&rel(".next/server/x.js"), &[]));
    assert!(is_ignored(&rel("venv/lib/x.py"), &[]));
    assert!(is_ignored(&rel(".venv/lib/x.py"), &[]));
    assert!(is_ignored(&rel("pkg/__pycache__/m.pyc"), &[]));
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
        system_prompt_split: None,
        max_turns: 3,
        tools: vec![],
        models: std::collections::HashMap::new(),

        tool_fence_profession: None,
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
    // Age the entry past the window directly (no sleeping). checked_sub 而非
    // 裸减：Instant 自系统启动起算，新启 VM 纪元余量不足会下溢 panic；回拨
    // 不了时环境边界诚实跳过。
    let Some(aged) = Instant::now()
        .checked_sub(SELF_WRITE_WINDOW)
        .and_then(|t| t.checked_sub(Duration::from_secs(1)))
    else {
        eprintln!("SKIP 自写窗口过期断言：系统刚启动，Instant 纪元余量不足 21s");
        return;
    };
    let mut map = arc.recent_self_writes.lock();
    map.insert("a.txt".to_string(), aged);
    drop(map);
    arc.push_external_change("a.txt");
    assert_eq!(arc.drain_external_changes(), vec!["a.txt".to_string()]);
}

#[test]
fn test_self_write_absolute_path_window_match() {
    // BUG-20261106-032：工具层 resolve_workspace_path 原样接受绝对路径，
    // agent 用绝对路径（反斜杠 + 大小写漂移的常见拼法）写文件是常态——
    // 自写登记的 key 必须折算成 workspace-relative 才能拦下 watcher 的
    // rel 事件，否则刚写的文件被谎报为「外部修改（非本会话操作）」。
    let dir = tempfile::tempdir().unwrap();
    let arc = test_loop();
    arc.set_workspace_root(dir.path().to_path_buf());
    let abs = format!("{}\\SRC\\A.rs", dir.path().display());
    arc.note_self_write(&abs);
    // watcher 事件形态：strip workspace 根后的 rel 路径。
    arc.push_external_change("src/a.rs");
    assert!(
        arc.drain_external_changes().is_empty(),
        "绝对路径形态的自写登记必须拦下 watcher rel 事件"
    );
}

#[test]
fn test_self_write_dot_relative_window_match() {
    // BUG-20261106-032 对照臂：`./` 前缀相对形态同样失配（归一不清洗
    // `.` 组件）——登记 key 必须与 watcher 事件的 rel 形态一致。
    let arc = test_loop();
    arc.note_self_write("./src/b.rs");
    arc.push_external_change("src/b.rs");
    assert!(
        arc.drain_external_changes().is_empty(),
        "`./` 前缀形态必须匹配"
    );
    // 根前缀不误剥（`<root>2` 与 `<root>` 是不同目录——组件级 strip 语义，
    // 字符级 strip_prefix 会把 `2/z.rs` 误剥成兄弟路径匹配）。
    let dir = tempfile::tempdir().unwrap();
    let arc2 = test_loop();
    arc2.set_workspace_root(dir.path().to_path_buf());
    let sibling = format!("{}2\\z.rs", dir.path().display());
    arc2.note_self_write(&sibling);
    arc2.push_external_change("2/z.rs");
    let drained = arc2.drain_external_changes();
    assert_eq!(
        drained,
        vec!["2/z.rs".to_string()],
        "根前缀组件级误剥必须回落不匹配"
    );
}

// --- BUG-20261118-091：multiedit 经 touch_instruction_chain 的自写登记 ---

fn test_instance() -> crate::instance::AgentInstance {
    crate::instance::AgentInstance::new(crate::types::AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("test".to_string()),
        system_prompt_split: None,
        max_turns: 3,
        tools: vec![],
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
    })
}

fn multiedit_tc(arguments: &str) -> crate::types::ToolCallInfo {
    crate::types::ToolCallInfo {
        id: "mc1".to_string(),
        name: "multiedit".to_string(),
        arguments: arguments.to_string(),
    }
}

#[test]
fn test_multiedit_touch_registers_self_write_for_each_edit_path() {
    // multiedit 是真实注册的写族工具（registry default + boundary 双形态，
    // big tier 白名单在列），单次调用批量编辑 N 个文件。touch_instruction_
    // chain 此前只登记 write_file/edit_file——agent 刚批量编辑的文件被
    // watcher 事件照常投递，下一轮上下文谎报「外部修改（非本会话操作）」。
    // 回归锁：multiedit 成功后每个 edits[].path（含大小写漂移形态）都必须
    // 进自写窗拦下 watcher 事件。
    let arc = test_loop();
    let instance = test_instance();
    let tc = multiedit_tc(
        r#"{"edits":[{"path":"src/a.rs","old":"x","new":"y"},{"path":"SRC/B.RS","old":"1","new":"2"}]}"#,
    );
    arc.touch_instruction_chain(&instance, &tc, true);
    arc.push_external_change("src/a.rs");
    arc.push_external_change("src/b.rs");
    // 对照臂：未登记文件照常呈现。
    arc.push_external_change("src/other.rs");
    assert_eq!(
        arc.drain_external_changes(),
        vec!["src/other.rs".to_string()],
        "multiedit 的每个 edits[].path 必须进自写窗拦下 watcher 事件"
    );
}

#[test]
fn test_multiedit_failure_does_not_register_self_write() {
    // 失败的 multiedit（tool_succeeded=false）不得登记——否则会吞掉
    // 真实的外部修改通知（自写窗语义只覆盖真实写成功的路径）。
    let arc = test_loop();
    let instance = test_instance();
    let tc = multiedit_tc(r#"{"edits":[{"path":"src/a.rs","old":"x","new":"y"}]}"#);
    arc.touch_instruction_chain(&instance, &tc, false);
    arc.push_external_change("src/a.rs");
    assert_eq!(arc.drain_external_changes(), vec!["src/a.rs".to_string()]);
}

#[test]
fn test_multiedit_malformed_args_no_panic() {
    // edits 缺失 / 非数组 / path 非字符串——防御性解析：零路径、不 panic、
    // 不登记（行为与漏登记一致但不炸 dispatch）。
    let arc = test_loop();
    let instance = test_instance();
    for args in [
        r#"{}"#,
        r#"{"edits":"not-an-array"}"#,
        r#"{"edits":[{"path":42}]}"#,
    ] {
        let tc = multiedit_tc(args);
        arc.touch_instruction_chain(&instance, &tc, true);
    }
    // 三个畸形 args 都没登记任何路径。
    arc.push_external_change("src/a.rs");
    assert_eq!(arc.drain_external_changes(), vec!["src/a.rs".to_string()]);
}

#[test]
fn test_write_file_touch_still_registers_self_write() {
    // 重构对照臂：单 path 工具（write_file）经重排后的路径提取仍照常登记。
    let arc = test_loop();
    let instance = test_instance();
    let tc = crate::types::ToolCallInfo {
        id: "wf1".to_string(),
        name: "write_file".to_string(),
        arguments: r#"{"path":"w/f.txt","content":"hi"}"#.to_string(),
    };
    arc.touch_instruction_chain(&instance, &tc, true);
    arc.push_external_change("w/f.txt");
    assert!(
        arc.drain_external_changes().is_empty(),
        "write_file 单 path 登记不得因 multiedit 重排回归"
    );
}

// --- BUG-20261127-046：防抖批内同路径先删后建（git checkout / 原子保存
// --- 编辑器形态）必须呈现「外部修改」---

/// 同一防抖批内 Remove→Create：去重表只记首个分类（Remove=true），后到的
/// Create 被丢弃，flush 侧 `!removed` 把真实修改静默吞掉——模型拿旧内容
/// 答题且无任何提示。修复语义 = 最新分类制（同路径后到事件覆盖分类：
/// 先删后建 → 以 Create 收尾 = 文件在场且内容已变 → 必须呈现）。
#[test]
fn test_watcher_remove_create_same_batch_surfaces_change() {
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

    // 第一批：首建（建立基线，收掉创建通知）。
    std::fs::write(dir.path().join("notes.txt"), "v1").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && rx.try_recv().is_err() {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        Instant::now() < deadline,
        "前置：首建通知必须先到达（批次隔离）"
    );
    // 排空残留事件，防止串批。
    while rx.try_recv().is_ok() {}

    // 第二批：快速 先删→再写（同路径、同一防抖窗口——两操作间隔远小于
    // 150ms，事件间 reset 语义保证同批）。
    let p = dir.path().join("notes.txt");
    std::fs::remove_file(&p).unwrap();
    std::fs::write(&p, "v2").unwrap();

    // 真实修改必须呈现（红证基线：修复前 flush 侧 removed=true 抑制 =
    // 5s 内无通知）。
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut got = false;
    while Instant::now() < deadline {
        if let Ok(p) = rx.try_recv()
            && p.contains("notes.txt")
        {
            got = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        got,
        "先删后建（git checkout / 原子保存形态）必须呈现外部修改通知"
    );
}

/// 对照臂（设计语义钉死）：同批 Create→Remove（改完即删）→ 以 Remove
/// 收尾 = 文件已不在 → 不呈现（删除通知本就不呈现；呈现会让模型去重读
/// 一个已消失的文件）。
#[test]
fn test_watcher_create_remove_same_batch_not_surfaced() {
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

    // 第一批：首建基线 + 排空。
    std::fs::write(dir.path().join("temp.txt"), "v1").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && rx.try_recv().is_err() {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(Instant::now() < deadline, "前置：首建通知必须先到达");
    while rx.try_recv().is_ok() {}

    // 第二批：写完即删（同窗口内 Modify→Remove，收尾分类 = Remove）。
    let p = dir.path().join("temp.txt");
    std::fs::write(&p, "v2").unwrap();
    std::fs::remove_file(&p).unwrap();

    // 5s 内不得呈现（红证基线：修复前首分类 Create=false 反而呈现）。
    std::thread::sleep(Duration::from_millis(1500));
    let leaked = rx.try_iter().any(|p| p.contains("temp.txt"));
    assert!(!leaked, "改完即删（收尾分类 Remove）不得呈现外部修改通知");
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
