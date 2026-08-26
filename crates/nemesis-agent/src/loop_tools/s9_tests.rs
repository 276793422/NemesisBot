//! S9 覆盖率批次（loop_tools 单点）：
//! - claude_code_tool.rs:87 / codex_tool.rs:91 — execute 首行 from_str 的
//!   map_err 闭包（非法 JSON 参数）；既有 tests.rs 只喂合法 JSON。
//! - cli_delegation.rs:158 — `where` 命中但无 Windows 可执行扩展名候选 →
//!   回退第一行（npm shim 布局的真实形态）。
//! - cli_delegation.rs:162 — 带可执行扩展名的候选被 pick_windows_exec_candidate
//!   选中。
//! 环境注意：PATH 操纵在本文件静态锁内完成并立即还原；查询名全局唯一，
//!   与其他并行测试（spawn `where`）不冲突。

use super::cli_delegation::find_cli_on_path;
use super::codex_tool::CodexTool;
use super::claude_code_tool::ClaudeCodeTool;
use super::Tool;
use crate::context::RequestContext;
use std::sync::Mutex;

/// 串行化 PATH 操纵（set_var 进程全局；本 crate 其他测试不读 PATH，
/// 唯一并发面是其他 spawn `where` 的测试——查询名唯一故无交叉）。
static PATH_LOCK: Mutex<()> = Mutex::new(());

fn delegation_ctx() -> RequestContext {
    RequestContext {
        channel: "web".to_string(),
        chat_id: "chat".to_string(),
        user: "u".to_string(),
        session_key: "agent:test/s9".to_string(),
        correlation_id: None,
        async_callback: None,
    }
}

/// 非法 JSON → "Invalid arguments" 闭包求值（87）。
#[tokio::test]
async fn claude_code_tool_invalid_json_args_err() {
    let t = ClaudeCodeTool::new("C:/fake/claude.exe".into(), None, None);
    let err = t
        .execute("this is not json {{{", &delegation_ctx())
        .await
        .unwrap_err();
    assert!(
        err.contains("Invalid arguments"),
        "got: {}",
        err
    );
}

/// 同上（91）。
#[tokio::test]
async fn codex_tool_invalid_json_args_err() {
    let t = CodexTool::new("C:/fake/codex.exe".into(), None, None);
    let err = t
        .execute("]]] not json", &delegation_ctx())
        .await
        .unwrap_err();
    assert!(err.contains("Invalid arguments"), "got: {}", err);
}

/// PATH 前置一个只含**无扩展名** shim 的目录 → where 命中该文件 → 无可
/// 执行扩展名候选 → 回退第一行（158）。
#[test]
#[cfg(windows)]
fn find_cli_on_path_extensionless_shim_falls_back_to_first_line() {
    let _guard = PATH_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let name = format!("nemesis_s9_shim_{}", std::process::id());
    let shim = dir.path().join(&name);
    std::fs::write(&shim, b"#!/bin/sh\nexit 0\n").unwrap();

    let old_path = std::env::var("PATH").unwrap_or_default();
    // SAFETY: PATH 操纵在 PATH_LOCK 互斥下串行；查询名全局唯一，其他
    // 并行测试不依赖被改写的 PATH 值（spawn 的子进程拿到的是快照）。
    unsafe {
        std::env::set_var("PATH", format!("{};{}", dir.path().display(), old_path));
    }
    let found = find_cli_on_path(&name);
    // SAFETY: 同上（锁内还原）。
    unsafe { std::env::set_var("PATH", &old_path); }

    let found = found.expect("where must find the extensionless shim");
    assert!(
        found.replace('/', "\\").contains(&name),
        "fallback must return the shim path: {}",
        found
    );
}

/// 查询带 .cmd 扩展名的名字 → 命中且带可执行扩展名 → pick 分支（162）。
#[test]
#[cfg(windows)]
fn find_cli_on_path_exec_extension_candidate_is_picked() {
    let _guard = PATH_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let stem = format!("nemesis_s9_cmd_{}", std::process::id());
    let fname = format!("{}.cmd", stem);
    std::fs::write(dir.path().join(&fname), b"@echo off\r\n").unwrap();

    let old_path = std::env::var("PATH").unwrap_or_default();
    // SAFETY: PATH 操纵在 PATH_LOCK 互斥下串行；查询名全局唯一，其他
    // 并行测试不依赖被改写的 PATH 值（spawn 的子进程拿到的是快照）。
    unsafe {
        std::env::set_var("PATH", format!("{};{}", dir.path().display(), old_path));
    }
    let found = find_cli_on_path(&fname);
    // SAFETY: 同上（锁内还原）。
    unsafe { std::env::set_var("PATH", &old_path); }

    let found = found.expect("where must find the .cmd file");
    assert!(
        found.replace('/', "\\").contains(&fname),
        "picked candidate must be the .cmd: {}",
        found
    );
}
