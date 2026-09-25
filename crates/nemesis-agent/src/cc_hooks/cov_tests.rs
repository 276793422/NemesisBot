// cc_hooks.rs 覆盖率补充测试（legacy hooks.json 迁移 / 方言解析决策表 /
// run_hook_script 失败面 / ScriptOutcome 方言语义 / load_from_dir）。

use super::*;

// ---------------------------------------------------------------------------
// migrate_legacy_home_hooks_config：copy-once 迁移
// ---------------------------------------------------------------------------

#[test]
fn legacy_hooks_config_migrates_once() {
    let home = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let legacy = home.path().join(HOOKS_FILE);
    let target = ws.path().join(HOOKS_FILE);

    // 无 legacy → 早退（不创建目标）。
    migrate_legacy_home_hooks_config(home.path(), ws.path());
    assert!(!target.exists());

    // 有 legacy → 复制。
    std::fs::write(&legacy, r#"{"hooks":{}}"#).unwrap();
    migrate_legacy_home_hooks_config(home.path(), ws.path());
    assert!(target.exists());
    assert_eq!(std::fs::read_to_string(&target).unwrap(), r#"{"hooks":{}}"#);

    // copy-once：改 legacy 再迁移 → 目标不动（保留旧目标 = 用户现状优先）。
    std::fs::write(&legacy, r#"{"changed":true}"#).unwrap();
    migrate_legacy_home_hooks_config(home.path(), ws.path());
    assert_eq!(std::fs::read_to_string(&target).unwrap(), r#"{"hooks":{}}"#);
}

// ---------------------------------------------------------------------------
// parse_cc_hooks / CcEvents 计数
// ---------------------------------------------------------------------------

#[test]
fn parse_cc_hooks_accepts_both_shapes_and_counts() {
    let raw = r#"{
        "hooks": {
            "PreToolUse": [
                {"matcher": "Edit|Write", "hooks": [
                    {"type": "command", "command": "cmd /C exit 0", "timeout": 5},
                    {"type": "command", "command": "cmd /C exit 2"}
                ]}
            ],
            "Stop": [
                {"hooks": [{"type": "command", "command": "cmd /C echo done"}]}
            ]
        }
    }"#;
    let events = parse_cc_hooks(raw).expect("standard shape parses");
    let counts = events.script_counts();
    assert_eq!(counts[0], ("PreToolUse", 2));
    assert_eq!(counts[5], ("Stop", 1));
    assert_eq!(
        events.script_counts().iter().map(|(_, n)| n).sum::<usize>(),
        3
    );

    // 裸顶层形态（手写省略外层）也收。
    let bare = r#"{"SessionStart":[{"hooks":[{"type":"command","command":"x"}]}]}"#;
    let events = parse_cc_hooks(bare).expect("bare shape parses");
    assert_eq!(events.script_counts()[3], ("SessionStart", 1));

    // 非法 JSON → Err。
    assert!(parse_cc_hooks("{not json").is_err());

    // 空配置 → is_empty。
    let empty = parse_cc_hooks("{}").unwrap();
    assert!(empty.is_empty());
}

// ---------------------------------------------------------------------------
// run_hook_script：成功 / 非零退出 / spawn 失败
// ---------------------------------------------------------------------------

#[tokio::test]
async fn run_hook_script_captures_exit_and_output() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_hook_script("echo cov-hook-ok", 10, "{}", dir.path()).await;
    assert_eq!(out.code, Some(0));
    assert!(out.stdout.contains("cov-hook-ok"), "got: {}", out.stdout);
    assert!(!out.timed_out);
}

#[tokio::test]
async fn run_hook_script_nonzero_exit_captures_stderr() {
    let dir = tempfile::tempdir().unwrap();
    let cmd = if cfg!(windows) {
        "echo cov-hook-err 1>&2 & exit 2"
    } else {
        "echo cov-hook-err 1>&2; exit 2"
    };
    let out = run_hook_script(cmd, 10, "{}", dir.path()).await;
    assert_eq!(out.code, Some(2));
    assert!(out.stderr.contains("cov-hook-err"), "got: {}", out.stderr);
}

/// cwd 指向不存在目录 → spawn 失败臂（诚实 stderr，不 panic）。
#[tokio::test]
async fn run_hook_script_spawn_failure_reports_honestly() {
    let ghost = std::env::temp_dir().join(format!("cov-no-hook-dir-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ghost);
    let out = run_hook_script("echo x", 5, "{}", &ghost).await;
    assert_eq!(out.code, None);
    assert!(
        out.stderr.contains("failed to spawn hook"),
        "got: {}",
        out.stderr
    );
}

// ---------------------------------------------------------------------------
// ScriptOutcome 方言语义
// ---------------------------------------------------------------------------

#[test]
fn script_outcome_dialect_semantics() {
    let ok = ScriptOutcome {
        code: Some(0),
        stdout: r#"{"decision":"block","reason":"no writes on friday"}"#.to_string(),
        stderr: String::new(),
        timed_out: false,
    };
    assert!(!ok.is_blocking_exit());
    assert_eq!(
        ok.json_block_reason().as_deref(),
        Some("no writes on friday")
    );
    // block_text 是原文（stderr 优先，trim），不做 JSON 提取。
    assert_eq!(
        ok.block_text(),
        r#"{"decision":"block","reason":"no writes on friday"}"#
    );

    // exit 2 = 阻断退出码；block_text 用 stderr。
    let blocking = ScriptOutcome {
        code: Some(2),
        stdout: String::new(),
        stderr: "  protected file  \n".to_string(),
        timed_out: false,
    };
    assert!(blocking.is_blocking_exit());
    assert!(
        blocking.json_block_reason().is_none(),
        "non-zero → no JSON verdict"
    );
    assert_eq!(blocking.block_text(), "protected file");

    // 无 reason 的 block → 占位文案。
    let terse = ScriptOutcome {
        code: Some(0),
        stdout: r#"{"decision":"block"}"#.to_string(),
        stderr: String::new(),
        timed_out: false,
    };
    assert_eq!(
        terse.json_block_reason().as_deref(),
        Some("blocked by hook (no reason given)")
    );

    // 非 block 决议 / 解析失败 → None。
    let other = ScriptOutcome {
        code: Some(0),
        stdout: r#"{"decision":"allow"}"#.to_string(),
        stderr: String::new(),
        timed_out: false,
    };
    assert!(other.json_block_reason().is_none());
    let garbage = ScriptOutcome {
        code: Some(0),
        stdout: "plain text".to_string(),
        stderr: String::new(),
        timed_out: false,
    };
    assert!(garbage.json_block_reason().is_none());

    // 超时形态：非阻断错误。
    let timed_out = ScriptOutcome {
        code: None,
        stdout: String::new(),
        stderr: "hook timed out after 3s".to_string(),
        timed_out: true,
    };
    assert!(!timed_out.is_blocking_exit());

    // 双流全空 → 占位文案。
    let silent = ScriptOutcome {
        code: Some(1),
        stdout: "   ".to_string(),
        stderr: String::new(),
        timed_out: false,
    };
    assert_eq!(silent.block_text(), "blocked by hook (no output)");
}

// ---------------------------------------------------------------------------
// load_from_dir：缺文件 = None；空配置 = None；有效 = Some
// ---------------------------------------------------------------------------

#[test]
fn load_from_dir_fail_open_semantics() {
    let dir = tempfile::tempdir().unwrap();
    // 没有 hooks.json = 没配（静默 None）。
    assert!(CcHookBridge::load_from_dir(dir.path(), dir.path().to_path_buf()).is_none());

    // 存在但非法 JSON → warn + None（fail-open）。
    std::fs::write(dir.path().join(HOOKS_FILE), "{broken").unwrap();
    assert!(CcHookBridge::load_from_dir(dir.path(), dir.path().to_path_buf()).is_none());

    // 合法但 0 脚本 → None。
    std::fs::write(dir.path().join(HOOKS_FILE), r#"{"hooks":{}}"#).unwrap();
    assert!(CcHookBridge::load_from_dir(dir.path(), dir.path().to_path_buf()).is_none());

    // 有脚本 → Some。
    std::fs::write(
        dir.path().join(HOOKS_FILE),
        r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo hi"}]}]}}"#,
    )
    .unwrap();
    let bridge = CcHookBridge::load_from_dir(dir.path(), dir.path().to_path_buf());
    assert!(bridge.is_some());
    assert_eq!(bridge.unwrap().current_events().total_scripts(), 1);
}

// ---------------------------------------------------------------------------
// wave5c：current_events 热更三分支（换装/解析失败保旧/文件消失注销）+
// spawn_notification 无 runtime 降级
// ---------------------------------------------------------------------------

/// 构造绑定盘上 hooks.json 的桥（seen_mtime 置 None 强制首检走重载）。
fn hot_bridge(json: &str, hooks_json: &std::path::Path) -> CcHookBridge {
    let mut b = CcHookBridge::from_json(json, std::env::temp_dir()).expect("parse");
    b.hooks_path = Some(hooks_json.to_path_buf());
    *b.seen_mtime.lock().unwrap() = None;
    b
}

#[test]
fn current_events_hot_reload_three_branches() {
    let dir = tempfile::tempdir().unwrap();
    let hooks_json = dir.path().join("hooks.json");

    // ① mtime 变化 + 解析成功 → 原子换装（脚本计数可见）。
    std::fs::write(
        &hooks_json,
        r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"cmd /C echo hi"}]}]}}"#,
    )
    .unwrap();
    let bridge = hot_bridge(r#"{"hooks":{}}"#, &hooks_json);
    let ev = bridge.current_events();
    assert_eq!(ev.total_scripts(), 1, "reloaded config takes effect");

    // ② mtime 再变 + 解析失败 → 保留旧配置（fail-open 保旧）。
    *bridge.seen_mtime.lock().unwrap() = Some(std::time::SystemTime::UNIX_EPOCH);
    std::fs::write(&hooks_json, "{broken").unwrap();
    let ev = bridge.current_events();
    assert_eq!(ev.total_scripts(), 1, "parse failure keeps old config");

    // ③ 文件消失 → 换装为空 = 事实注销。
    *bridge.seen_mtime.lock().unwrap() = Some(std::time::SystemTime::UNIX_EPOCH);
    std::fs::remove_file(&hooks_json).unwrap();
    let ev = bridge.current_events();
    assert_eq!(ev.total_scripts(), 0, "file gone = deactivated");
}

/// 无 tokio runtime 的同步上下文调用 → debug 降级跳过（不 panic）。
#[test]
fn spawn_notification_without_runtime_is_noop() {
    let bridge = std::sync::Arc::new(
        CcHookBridge::from_json(r#"{"hooks":{}}"#, std::env::temp_dir()).expect("parse"),
    );
    spawn_notification(
        &bridge,
        "sk".to_string(),
        "ApprovalRequest",
        "test",
        serde_json::json!({}),
    );
}
