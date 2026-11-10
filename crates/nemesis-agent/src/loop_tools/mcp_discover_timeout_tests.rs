//! BUG-20261110-056：mcp_discover 超时的 LLM 输入面硬上界。
//!
//! 旧行为：`args["timeout"]` 纯透传 `one_shot_budget`（`if >0` 原样
//! `Duration::from_secs`）——工具执行无外层墙钟，prompt 注入可传
//! `timeout=10^12` 让一次 mcp_discover 挂死整个 agent loop（工具内部
//! 超时是唯一防线）。CLI `mcp discover --timeout` 用户显式意志不经过
//! 工具层，不受影响。同族纪律先例：`tools.exec.max_timeout_secs`
//! （缺省生效 600s）、oauth `bounded_poll_wait`（远端可控间隔不推出
//! deadline）。

use super::mcp::{MCP_DISCOVER_MAX_TIMEOUT_SECS, resolve_discover_timeout};

#[test]
fn bug056_missing_and_zero_fall_back_to_default_15() {
    // 缺省（键缺席 → None）保持旧行为 15s。
    assert_eq!(resolve_discover_timeout(None), 15);
    // 显式 0 在旧行为里经 one_shot_budget 的 `if >0` 也落 15s——语义保留。
    assert_eq!(resolve_discover_timeout(Some(0)), 15);
}

#[test]
fn bug056_normal_values_pass_through() {
    assert_eq!(resolve_discover_timeout(Some(1)), 1);
    assert_eq!(resolve_discover_timeout(Some(15)), 15);
    assert_eq!(resolve_discover_timeout(Some(300)), 300);
    // 上界本身放行（CLI 之外的工具面仍可到 600，与 exec 生效口径一致）。
    assert_eq!(resolve_discover_timeout(Some(600)), 600);
}

#[test]
fn bug056_huge_values_are_capped_at_600() {
    // 注入面实锤形态：10^12 秒（≈31709 年）→ 600s。
    assert_eq!(resolve_discover_timeout(Some(1_000_000_000_000)), 600);
    assert_eq!(resolve_discover_timeout(Some(u64::MAX)), 600);
    assert_eq!(
        MCP_DISCOVER_MAX_TIMEOUT_SECS, 600,
        "上界与 tools.exec.max_timeout_secs 缺省生效口径一致"
    );
}

/// execute 接线级验证：巨值 timeout + 必然 spawn 失败的 command——
/// 两条臂都应快速返回（不挂死、不 panic），证明 cap 后的值正常流转进
/// `discover_server_metadata`（真实 600s 等待归 nemesis-mcp 既有
/// one_shot_budget 超时阶梯测试覆盖，此处不重复计时）。
#[tokio::test]
async fn bug056_execute_with_huge_timeout_fails_fast_on_bad_command() {
    use super::mcp::McpDiscoverTool;
    use super::{RequestContext, Tool};

    let tool = McpDiscoverTool::new();
    let ctx = RequestContext::new("web", "bug056", "tester", "bug056-session");
    // timeout 巨值（注入面形态）+ 不存在的命令 → spawn 立即失败。
    let args = r#"{"command": "nb-bug056-nonexistent-cmd", "timeout": 999999999999999}"#;
    let out = tokio::time::timeout(std::time::Duration::from_secs(30), tool.execute(args, &ctx))
        .await
        .expect("execute must return promptly (cap must not hang the test)");
    assert!(
        out.is_err(),
        "nonexistent command must yield Err, got: {out:?}"
    );
}
