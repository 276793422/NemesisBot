//! VmBrowserTool 测试（纯面：schema 对账 + 无装配环境的诚实错误臂；
//! VM 真流归 cluster-uat T-VM 浏览器场景——本文件不触 supervisor）。
//!
//! `MANAGER_SLOT` 在测试进程恒 None（vm_bridge_sync_tests 同款前提——
//! 只有 gateway runtime/post_agent 装配），「管理器未装配」臂稳定可达。

use nemesis_agent::context::RequestContext;
use nemesis_agent::r#loop::Tool as _;

use super::*;

fn ctx() -> RequestContext {
    RequestContext::new("test", "chat", "tester", "sess")
}

#[test]
fn tool_name_and_actions_frozen() {
    assert_eq!(TOOL_NAME, "browser");
    assert_eq!(
        BROWSER_ACTIONS,
        [
            "navigate",
            "click",
            "type",
            "fill_form",
            "extract_text",
            "screenshot",
            "wait_for"
        ]
    );
}

#[test]
fn parameters_enum_matches_action_table() {
    // schema enum 与分派表同源（guest 侧 handle 分派表的同源镜像记忆点）。
    let params = VmBrowserTool::new(Arc::new(|| None)).parameters();
    let enums: Vec<&str> = params["properties"]["action"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(enums, BROWSER_ACTIONS.to_vec());
    // required 只有 action（其余按动作可选）。
    assert_eq!(params["required"], serde_json::json!(["action"]));
}

fn tool_with_resolve(
    r: impl Fn() -> Option<(String, std::path::PathBuf)> + Send + Sync + 'static,
) -> VmBrowserTool {
    VmBrowserTool::new(Arc::new(r))
}

#[tokio::test]
async fn execute_requires_action() {
    let t = tool_with_resolve(|| None);
    let err = t.execute("{}", &ctx()).await.unwrap_err();
    assert!(err.contains("missing 'action'"), "got: {err}");
}

#[tokio::test]
async fn execute_rejects_unknown_action() {
    // 未知 action 在身份解析之前诚实拒绝（防把恶意枚举值带进路由层）。
    let t = tool_with_resolve(|| None);
    let err = t
        .execute(r#"{"action":"rm -rf"}"#, &ctx())
        .await
        .unwrap_err();
    assert!(err.contains("unknown action"), "got: {err}");
    assert!(err.contains("navigate"), "got: {err}");
}

#[tokio::test]
async fn execute_no_route_honest_error() {
    // resolve = None（headless run / acp 形态）：诚实报错，不触 manager。
    let t = tool_with_resolve(|| None);
    let err = t
        .execute(
            r#"{"action":"navigate","url":"https://example.com"}"#,
            &ctx(),
        )
        .await
        .unwrap_err();
    assert!(err.contains("无持久 agent 路由"), "got: {err}");
}

#[tokio::test]
async fn execute_no_manager_honest_error() {
    // 路由命中但管理器槽未装配（测试进程恒 None）：诚实报错。
    let t =
        tool_with_resolve(|| Some(("agent-x".to_string(), std::path::PathBuf::from("/tmp/ws"))));
    let err = t
        .execute(r#"{"action":"screenshot"}"#, &ctx())
        .await
        .unwrap_err();
    assert!(err.contains("管理器未装配"), "got: {err}");
}
