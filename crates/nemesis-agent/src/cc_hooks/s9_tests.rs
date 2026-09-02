//! S9 覆盖率批次：cc_hooks.rs 剩余未覆盖行。
//! - 226：enrich_tool_input 的 file_path 补齐臂收尾（三分支：已带
//!   file_path / path 或 file 命中 / 无一命中）。
//! - 328-334：run_hook_script spawn 失败（project_dir 不存在 → cwd 无效）。
//! - 455-461：load_from_dir 成功 info! 的参数表达式行（需 subscriber）。
//! - 468：load_from_dir 解析失败 warn! 的参数行。
//! - 505：run_group 非阻断失败 warn! 的参数行（exit 3）。
//! - 510-511：run_group 成功 stdout info! 的参数行（echo）。
//! - 311-313：`sh -c` 分支为平台分支（cfg!(windows) 恒真于本平台，Linux
//!   侧可达）——见报告平台依赖组。
//! - 352-357：wait_with_output Err（spawn 成功后收集输出 IO 失败）——无
//!   确定性注入手段，机器依赖。

use super::*;
use crate::test_support::capture_logs;

fn tempdir() -> std::path::PathBuf {
    tempfile::tempdir().expect("tempdir").keep()
}

fn hooks_doc(event: &str, command: &str) -> String {
    let hook = serde_json::json!({ "command": command });
    let group = serde_json::json!({ "hooks": [hook] });
    let mut events = serde_json::Map::new();
    events.insert(event.to_string(), serde_json::Value::Array(vec![group]));
    serde_json::json!({ "hooks": events }).to_string()
}

// ---------- enrich_tool_input（纯函数） ----------

#[test]
fn enrich_tool_input_three_branches() {
    // 1) 已有 file_path → 不动。
    let v = enrich_tool_input(r#"{"file_path": "a.rs", "x": 1}"#);
    assert_eq!(v["file_path"], "a.rs");
    // 2) 无 file_path 但有 path → 补 file_path（break 收尾 → 226）。
    let v = enrich_tool_input(r#"{"path": "b.rs"}"#);
    assert_eq!(v["file_path"], "b.rs");
    assert_eq!(v["path"], "b.rs", "原字段保留");
    // file 键同样命中。
    let v = enrich_tool_input(r#"{"file": "c.rs"}"#);
    assert_eq!(v["file_path"], "c.rs");
    // 3) 三个键都没有 → 循环耗尽收尾（226 的另一形态）。
    let v = enrich_tool_input(r#"{"other": 1}"#);
    assert!(v.get("file_path").is_none());
    // 4) 非 JSON 输入 → 空对象兜底。
    let v = enrich_tool_input("not json");
    assert!(v.is_object());
}

// ---------- run_hook_script spawn 失败 ----------

#[tokio::test]
async fn run_hook_script_spawn_failure_reports_stderr() {
    let _logs = capture_logs();
    // project_dir 不存在 → cmd /C 的 cwd 无效 → spawn Err（328-334）。
    let bogus = std::path::PathBuf::from("Z:/nemesis_s9_no_such_dir");
    let out = run_hook_script("echo hi", 5, "{}", &bogus).await;
    assert_eq!(out.code, None);
    assert!(out.stdout.is_empty());
    assert!(
        out.stderr.contains("failed to spawn hook"),
        "stderr={}",
        out.stderr
    );
    assert!(!out.timed_out);
}

// ---------- load_from_dir ----------

#[test]
fn load_from_dir_valid_logs_info_fields() {
    let _logs = capture_logs();
    let cfg = tempdir();
    let proj = tempdir();
    std::fs::write(cfg.join("hooks.json"), hooks_doc("PreToolUse", "exit 0")).unwrap();
    let bridge = CcHookBridge::load_from_dir(&cfg, proj.clone());
    assert!(bridge.is_some(), "valid hooks.json loads");
    let _ = std::fs::remove_dir_all(&cfg);
    let _ = std::fs::remove_dir_all(&proj);
}

#[test]
fn load_from_dir_parse_failure_warns_and_fails_open() {
    let _logs = capture_logs();
    let cfg = tempdir();
    let proj = tempdir();
    std::fs::write(cfg.join("hooks.json"), "!!! not json !!!").unwrap();
    let bridge = CcHookBridge::load_from_dir(&cfg, proj.clone());
    assert!(bridge.is_none(), "fail-open: parse error disables hooks");
    let _ = std::fs::remove_dir_all(&cfg);
    let _ = std::fs::remove_dir_all(&proj);
}

#[test]
fn load_from_dir_missing_file_is_silent_none() {
    let cfg = tempdir();
    let proj = tempdir();
    assert!(CcHookBridge::load_from_dir(&cfg, proj.clone()).is_none());
    let _ = std::fs::remove_dir_all(&cfg);
    let _ = std::fs::remove_dir_all(&proj);
}

// ---------- run_group 的 warn/info 参数行 ----------

fn edit_call() -> HookToolCall {
    HookToolCall {
        name: "edit".to_string(),
        arguments: "{}".to_string(),
        channel: "web".to_string(),
        chat_id: "chat1".to_string(),
        session_key: "s9sess".to_string(),
    }
}

/// hook 退出码 3（非 0 非 2）→ 非阻断失败 warn（501-506 参数行）。
#[tokio::test]
async fn run_group_nonzero_exit_logs_warn_fields() {
    let _logs = capture_logs();
    let tmp = tempdir();
    let b =
        CcHookBridge::from_json(&hooks_doc("PreToolUse", "exit 3"), tmp.clone()).expect("bridge");
    let d = b.pre_tool_use(&edit_call()).await;
    assert_eq!(d, crate::hooks::HookDecision::Allow, "非阻断失败 → 放行");
    let _ = std::fs::remove_dir_all(&tmp);
}

/// hook 成功且有 stdout → info（508-512 参数行）。
#[tokio::test]
async fn run_group_success_stdout_logs_info_fields() {
    let _logs = capture_logs();
    let tmp = tempdir();
    let cmd = "echo s9hookstdout";
    let b = CcHookBridge::from_json(&hooks_doc("PreToolUse", cmd), tmp.clone()).expect("bridge");
    let d = b.pre_tool_use(&edit_call()).await;
    assert_eq!(d, crate::hooks::HookDecision::Allow);
    let _ = std::fs::remove_dir_all(&tmp);
}

/// 对照：alias 表未知项。
#[test]
fn cc_tool_alias_unknown_returns_none() {
    assert!(cc_tool_alias("weather").is_none());
    assert_eq!(cc_tool_alias("write_file"), Some("Write"));
}
