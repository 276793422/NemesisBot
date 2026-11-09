//! exec deny 词表接线测试（2026-11-08 激活 `tools.exec.*` 死键）。
//!
//! 语义钉死：
//! - `ExecTool::new` / `AsyncExecTool::new` = 内置默认词表（接线前行为）；
//! - `new_with_config` 自定义词表 = **替换**语义（非叠加）；
//! - 自定义词表全部编译失败 / 传入空表 = 回落内置默认（永不全放行）；
//! - deny 命中 → execute 返回 Err 且消息带 pattern 原文；
//! - 词表单一真相源 = `nemesis_tools::shell::DEFAULT_EXEC_DENY_PATTERNS`。

use super::*;
use crate::context::RequestContext;

fn ctx() -> RequestContext {
    RequestContext::new("web", "chat1", "user1", "sess1")
}

/// 默认词表非空且关键模式在场（单一真相源卫兵——词表被误清空时先红这里）。
#[test]
fn default_deny_table_nonempty_with_key_patterns() {
    let table = nemesis_tools::shell::DEFAULT_EXEC_DENY_PATTERNS;
    assert!(table.len() >= 20, "默认词表至少 20 条，got {}", table.len());
    let joined = table.join("\n");
    for key in [
        "\\brm\\s",
        "\\bsudo\\b",
        "shutdown",
        "git\\s+push",
        "\\beval\\b",
    ] {
        assert!(joined.contains(key), "默认词表必须含 {key} 相关模式");
    }
}

/// `ExecTool::new` = 默认词表：危险命令被拒、普通命令放行（deny_hit 层）。
#[test]
fn exec_tool_default_deny_blocks_dangerous() {
    let t = ExecTool::new("/tmp/ws", true);
    assert!(
        t.deny_hit("sudo rm -rf /").is_some(),
        "sudo 必须命中默认词表"
    );
    assert!(
        t.deny_hit("git push --force origin main").is_some(),
        "git push 必须命中"
    );
    assert!(t.deny_hit("shutdown /s").is_some(), "shutdown 必须命中");
    assert!(t.deny_hit("ls -la").is_none(), "普通只读命令不得误伤");
    assert!(t.deny_hit("cargo build --release").is_none());
}

/// 大小写不敏感（deny_hit 统一小写后比对）。
#[test]
fn exec_tool_deny_case_insensitive() {
    let t = ExecTool::new("/tmp/ws", true);
    assert!(t.deny_hit("SUDO rm -rf /").is_some());
    assert!(t.deny_hit("Git PUSH origin").is_some());
}

/// `new_with_config` 自定义词表 = **替换**语义：自定义命中、
/// 默认词表条目放行。
#[test]
fn exec_tool_custom_deny_replaces_defaults() {
    let custom = ["forbidden_tool\\.exe"];
    let t = ExecTool::new_with_config("/tmp/ws", true, Some(&custom));
    assert!(
        t.deny_hit("run forbidden_tool.exe --flag").is_some(),
        "自定义词表条目必须命中"
    );
    assert!(
        t.deny_hit("sudo rm -rf /").is_none(),
        "替换语义：默认词表条目在自定义模式下放行（Go 版 NewExecToolWithConfig 同语义）"
    );
}

/// 自定义词表全部编译失败 = 回落默认词表（不静默全放行）。
#[test]
fn exec_tool_invalid_custom_falls_back_to_defaults() {
    let custom = ["([unclosed", "also(bad"];
    let t = ExecTool::new_with_config("/tmp/ws", true, Some(&custom));
    assert!(
        t.deny_hit("sudo ls").is_some(),
        "全无效自定义词表必须回落内置默认词表"
    );
}

/// 空表 / None = 默认词表（装配侧「开关开但词表空」的回落形态）。
#[test]
fn exec_tool_empty_custom_uses_defaults() {
    let empty: [&str; 0] = [];
    let t = ExecTool::new_with_config("/tmp/ws", true, Some(&empty));
    assert!(t.deny_hit("sudo ls").is_some(), "空自定义词表 = 默认词表");

    let t2 = ExecTool::new_with_config("/tmp/ws", true, None);
    assert!(t2.deny_hit("sudo ls").is_some(), "None = 默认词表");
}

/// execute 面：deny 命中返回 Err 且消息带 pattern（spawn 之前拦截，
/// 不产生子进程）。普通命令照常走 spawn 路径（此处只断言拒绝面）。
#[tokio::test]
async fn exec_tool_execute_denies_before_spawn() {
    let t = ExecTool::new("/tmp/ws", true);
    let args = serde_json::json!({ "command": "sudo echo hi" }).to_string();
    let err = t.execute(&args, &ctx()).await.unwrap_err();
    assert!(
        err.contains("blocked by deny pattern"),
        "拒绝消息形态: {err}"
    );
    assert!(err.contains("sudo"), "拒绝消息须带 pattern 原文: {err}");
}

/// execute 面：词表放行的普通命令在 deny 层不报错（走到围栏/spawn 层——
/// 本测试只断言错误不是 deny 拒绝）。
#[tokio::test]
async fn exec_tool_execute_non_denied_passes_deny_layer() {
    let t = ExecTool::new("/tmp/ws", true);
    let args = serde_json::json!({ "command": "echo deny-pass-through" }).to_string();
    match t.execute(&args, &ctx()).await {
        Ok(_) => {}
        Err(e) => assert!(
            !e.contains("blocked by deny pattern"),
            "普通命令不得被 deny 层拦截: {e}"
        ),
    }
}

/// `AsyncExecTool` 同源同语义（默认词表 + 自定义替换 + execute 拒绝面）。
#[test]
fn async_exec_tool_deny_parity() {
    let t = AsyncExecTool::new("/tmp/ws", true);
    assert!(t.deny_hit("shutdown /s").is_some());
    assert!(t.deny_hit("notepad.exe").is_none());

    let custom = ["evil\\.exe"];
    let t2 = AsyncExecTool::new_with_config("/tmp/ws", true, Some(&custom));
    assert!(t2.deny_hit("start evil.exe").is_some());
    assert!(
        t2.deny_hit("sudo rm -rf /").is_none(),
        "异步工具同为替换语义"
    );
}

#[tokio::test]
async fn async_exec_tool_execute_denies_before_spawn() {
    let t = AsyncExecTool::new("/tmp/ws", true);
    let args = serde_json::json!({ "command": "sudo notepad" }).to_string();
    let err = t.execute(&args, &ctx()).await.unwrap_err();
    assert!(
        err.contains("blocked by deny pattern"),
        "拒绝消息形态: {err}"
    );
}
