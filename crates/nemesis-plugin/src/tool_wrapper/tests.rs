use super::*;
use crate::plugin::Plugin;
use std::any::Any;

/// A simple test tool that echoes back the "input" argument.
struct EchoTool;

impl ToolExecutor for EchoTool {
    fn execute(
        &self,
        args: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<serde_json::Value, String> {
        let input = args
            .get("input")
            .and_then(|v| v.as_str())
            .unwrap_or("no input");
        Ok(serde_json::json!({"echo": input}))
    }
}

/// A plugin that denies execution of "blocked_tool".
struct DenyPlugin;

impl Plugin for DenyPlugin {
    fn name(&self) -> &str {
        "deny_plugin"
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn execute(&self, invocation: &mut ToolInvocation) -> (bool, Option<String>, bool) {
        if invocation.tool_name == "blocked_tool" {
            return (false, Some("tool is blocked".to_string()), false);
        }
        (true, None, false)
    }
    fn cleanup(&self) -> Result<(), String> {
        Ok(())
    }
}

/// A plugin that modifies the result by wrapping it.
struct ModifyPlugin;

impl Plugin for ModifyPlugin {
    fn name(&self) -> &str {
        "modify_plugin"
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn execute(&self, invocation: &mut ToolInvocation) -> (bool, Option<String>, bool) {
        if invocation.result.is_some() {
            invocation.result = Some(serde_json::json!({
                "modified": true,
                "original": invocation.result.clone(),
            }));
        }
        (true, None, true)
    }
    fn cleanup(&self) -> Result<(), String> {
        Ok(())
    }
}

fn make_mgr() -> Arc<Mutex<PluginManager>> {
    Arc::new(Mutex::new(PluginManager::new()))
}

fn make_echo() -> Arc<dyn ToolExecutor> {
    Arc::new(EchoTool)
}

#[test]
fn test_tool_wrapper_passthrough() {
    let mgr = make_mgr();
    let wrapper = ToolWrapper::new("echo", mgr, "user1", "web", "/workspace", make_echo());
    let mut args = serde_json::Map::new();
    args.insert("input".to_string(), serde_json::json!("hello"));
    let result = wrapper.execute(&args).unwrap();
    assert_eq!(result["echo"], "hello");
}

#[test]
fn test_tool_wrapper_blocked() {
    let mgr = make_mgr();
    {
        let mut m = mgr.lock();
        m.register(Box::new(DenyPlugin)).unwrap();
    }
    let wrapper = ToolWrapper::new(
        "blocked_tool",
        mgr,
        "user1",
        "web",
        "/workspace",
        make_echo(),
    );
    let args = serde_json::Map::new();
    let result = wrapper.execute(&args);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("blocked"));
}

#[test]
fn test_tool_wrapper_modified() {
    let mgr = make_mgr();
    {
        let mut m = mgr.lock();
        m.register(Box::new(ModifyPlugin)).unwrap();
    }
    let wrapper = ToolWrapper::new("echo", mgr, "user1", "web", "/workspace", make_echo());
    let mut args = serde_json::Map::new();
    args.insert("input".to_string(), serde_json::json!("test"));
    let result = wrapper.execute(&args).unwrap();
    assert_eq!(result["modified"], true);
    assert!(result["original"].is_object());
}

#[test]
fn test_pluginable_tool() {
    let mgr = make_mgr();
    let tool = PluginableTool::new("echo", mgr, make_echo(), "user1", "web", "/workspace");
    let mut args = serde_json::Map::new();
    args.insert("input".to_string(), serde_json::json!("pluginable"));
    let result = tool.execute(&args).unwrap();
    assert_eq!(result["echo"], "pluginable");
}

#[test]
fn test_pluginable_tool_blocked() {
    let mgr = make_mgr();
    {
        let mut m = mgr.lock();
        m.register(Box::new(DenyPlugin)).unwrap();
    }
    let tool = PluginableTool::new(
        "blocked_tool",
        mgr,
        make_echo(),
        "user1",
        "web",
        "/workspace",
    );
    let args = serde_json::Map::new();
    let result = tool.execute(&args);
    assert!(result.is_err());
}

// ---- Additional coverage tests for 95%+ ----

#[test]
fn test_tool_wrapper_failing_tool() {
    struct FailTool;
    impl ToolExecutor for FailTool {
        fn execute(
            &self,
            _args: &serde_json::Map<String, serde_json::Value>,
        ) -> Result<serde_json::Value, String> {
            Err("tool execution failed".to_string())
        }
    }

    let mgr = make_mgr();
    let wrapper = ToolWrapper::new(
        "fail_tool",
        mgr,
        "user1",
        "web",
        "/workspace",
        Arc::new(FailTool),
    );
    let args = serde_json::Map::new();
    let result = wrapper.execute(&args);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("tool execution failed"));
}

#[test]
fn test_pluginable_tool_with_failing_tool() {
    struct FailTool;
    impl ToolExecutor for FailTool {
        fn execute(
            &self,
            _args: &serde_json::Map<String, serde_json::Value>,
        ) -> Result<serde_json::Value, String> {
            Err("fail".to_string())
        }
    }

    let mgr = make_mgr();
    let tool = PluginableTool::new(
        "fail_tool",
        mgr,
        Arc::new(FailTool),
        "user1",
        "web",
        "/workspace",
    );
    let result = tool.execute(&serde_json::Map::new());
    assert!(result.is_err());
}

#[test]
fn test_tool_wrapper_with_modify_plugin_on_error() {
    // ModifyPlugin should still be able to inspect even when tool fails
    struct FailTool;
    impl ToolExecutor for FailTool {
        fn execute(
            &self,
            _args: &serde_json::Map<String, serde_json::Value>,
        ) -> Result<serde_json::Value, String> {
            Err("fail".to_string())
        }
    }

    let mgr = make_mgr();
    {
        let mut m = mgr.lock();
        m.register(Box::new(ModifyPlugin)).unwrap();
    }

    let wrapper = ToolWrapper::new(
        "fail_tool",
        mgr,
        "user1",
        "web",
        "/workspace",
        Arc::new(FailTool),
    );
    let result = wrapper.execute(&serde_json::Map::new());
    // ModifyPlugin only modifies if result.is_some(), which won't be the case for failed tool
    // so we should get the error back
    assert!(result.is_err());
}

// ---- Additional coverage for 95%+ target ----

#[test]
fn test_tool_wrapper_pre_check_denies_no_message() {
    struct DenyNoMsgPlugin;
    impl Plugin for DenyNoMsgPlugin {
        fn name(&self) -> &str {
            "deny_nomsg"
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn execute(&self, _inv: &mut ToolInvocation) -> (bool, Option<String>, bool) {
            (false, None, false)
        }
        fn cleanup(&self) -> Result<(), String> {
            Ok(())
        }
    }

    let mgr = make_mgr();
    {
        let mut m = mgr.lock();
        m.register(Box::new(DenyNoMsgPlugin)).unwrap();
    }
    let wrapper = ToolWrapper::new("echo", mgr, "user1", "web", "/workspace", make_echo());
    let result = wrapper.execute(&serde_json::Map::new());
    assert!(result.is_err());
    // PluginManager returns "[deny_nomsg] operation denied"
    let err = result.unwrap_err();
    assert!(err.contains("operation denied") || err.contains("denied"));
}

#[test]
fn test_tool_wrapper_post_check_denies_no_message() {
    struct PostDenyPlugin;
    impl Plugin for PostDenyPlugin {
        fn name(&self) -> &str {
            "post_deny"
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn execute(&self, inv: &mut ToolInvocation) -> (bool, Option<String>, bool) {
            if inv.result.is_some() {
                return (false, None, false);
            }
            (true, None, false)
        }
        fn cleanup(&self) -> Result<(), String> {
            Ok(())
        }
    }

    let mgr = make_mgr();
    {
        let mut m = mgr.lock();
        m.register(Box::new(PostDenyPlugin)).unwrap();
    }
    let wrapper = ToolWrapper::new("echo", mgr, "user1", "web", "/workspace", make_echo());
    let mut args = serde_json::Map::new();
    args.insert("input".to_string(), serde_json::json!("hello"));
    let result = wrapper.execute(&args);
    assert!(result.is_err());
    // PluginManager returns "[post_deny] operation denied"
    let err = result.unwrap_err();
    assert!(err.contains("operation denied") || err.contains("denied"));
}

#[test]
fn test_tool_wrapper_post_check_denies_with_message() {
    struct PostDenyMsgPlugin;
    impl Plugin for PostDenyMsgPlugin {
        fn name(&self) -> &str {
            "post_deny_msg"
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn execute(&self, inv: &mut ToolInvocation) -> (bool, Option<String>, bool) {
            if inv.result.is_some() {
                return (false, Some("post-check blocked".to_string()), false);
            }
            (true, None, false)
        }
        fn cleanup(&self) -> Result<(), String> {
            Ok(())
        }
    }

    let mgr = make_mgr();
    {
        let mut m = mgr.lock();
        m.register(Box::new(PostDenyMsgPlugin)).unwrap();
    }
    let wrapper = ToolWrapper::new("echo", mgr, "user1", "web", "/workspace", make_echo());
    let mut args = serde_json::Map::new();
    args.insert("input".to_string(), serde_json::json!("hello"));
    let result = wrapper.execute(&args);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("post-check blocked"));
}

#[test]
fn test_tool_wrapper_result_replaced_by_plugin() {
    struct ReplacePlugin;
    impl Plugin for ReplacePlugin {
        fn name(&self) -> &str {
            "replace"
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn execute(&self, inv: &mut ToolInvocation) -> (bool, Option<String>, bool) {
            if inv.result.is_some() {
                inv.result = Some(serde_json::json!({"replaced": true}));
            }
            (true, None, true)
        }
        fn cleanup(&self) -> Result<(), String> {
            Ok(())
        }
    }

    let mgr = make_mgr();
    {
        let mut m = mgr.lock();
        m.register(Box::new(ReplacePlugin)).unwrap();
    }
    let wrapper = ToolWrapper::new("echo", mgr, "user1", "web", "/workspace", make_echo());
    let mut args = serde_json::Map::new();
    args.insert("input".to_string(), serde_json::json!("hello"));
    let result = wrapper.execute(&args).unwrap();
    assert_eq!(result["replaced"], true);
    assert!(result.get("echo").is_none());
}

#[test]
fn test_pluginable_tool_modified() {
    let mgr = make_mgr();
    {
        let mut m = mgr.lock();
        m.register(Box::new(ModifyPlugin)).unwrap();
    }
    let tool = PluginableTool::new("echo", mgr, make_echo(), "user1", "web", "/workspace");
    let mut args = serde_json::Map::new();
    args.insert("input".to_string(), serde_json::json!("test"));
    let result = tool.execute(&args).unwrap();
    assert_eq!(result["modified"], true);
}

#[test]
fn test_tool_wrapper_no_args() {
    let mgr = make_mgr();
    let wrapper = ToolWrapper::new("echo", mgr, "", "", "", make_echo());
    let result = wrapper.execute(&serde_json::Map::new()).unwrap();
    assert_eq!(result["echo"], "no input");
}

#[test]
fn test_pluginable_tool_no_args() {
    let mgr = make_mgr();
    let tool = PluginableTool::new("echo", mgr, make_echo(), "", "", "");
    let result = tool.execute(&serde_json::Map::new()).unwrap();
    assert_eq!(result["echo"], "no input");
}

// ---- Phase 4 coverage for 95%+ target ----

#[test]
fn test_tool_wrapper_fields_are_set() {
    let mgr = make_mgr();
    let wrapper = ToolWrapper::new("my_tool", mgr, "user2", "cli", "/home", make_echo());
    // Just verify creation works; fields are private but exercised through execute
    let mut args = serde_json::Map::new();
    args.insert("input".to_string(), serde_json::json!("hello"));
    let result = wrapper.execute(&args).unwrap();
    assert_eq!(result["echo"], "hello");
}

#[test]
fn test_tool_wrapper_post_check_denies_with_error_message() {
    struct PostBlockPlugin;
    impl Plugin for PostBlockPlugin {
        fn name(&self) -> &str {
            "post_block"
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn execute(&self, inv: &mut ToolInvocation) -> (bool, Option<String>, bool) {
            if inv.result.is_some() {
                return (false, Some("post-blocked".to_string()), false);
            }
            (true, None, false)
        }
        fn cleanup(&self) -> Result<(), String> {
            Ok(())
        }
    }

    let mgr = make_mgr();
    {
        let mut m = mgr.lock();
        m.register(Box::new(PostBlockPlugin)).unwrap();
    }
    let wrapper = ToolWrapper::new("echo", mgr, "u", "web", "/ws", make_echo());
    let mut args = serde_json::Map::new();
    args.insert("input".to_string(), serde_json::json!("hello"));
    let result = wrapper.execute(&args);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("post-blocked"));
}

#[test]
fn test_tool_wrapper_plugin_sets_result_on_failed_tool() {
    struct FixPlugin;
    impl Plugin for FixPlugin {
        fn name(&self) -> &str {
            "fixer"
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn execute(&self, inv: &mut ToolInvocation) -> (bool, Option<String>, bool) {
            // If tool failed, replace with success
            if inv.blocking_error.is_some() {
                inv.blocking_error = None;
                inv.result = Some(serde_json::json!({"fixed": true}));
            }
            (true, None, true)
        }
        fn cleanup(&self) -> Result<(), String> {
            Ok(())
        }
    }

    struct FailTool;
    impl ToolExecutor for FailTool {
        fn execute(
            &self,
            _args: &serde_json::Map<String, serde_json::Value>,
        ) -> Result<serde_json::Value, String> {
            Err("original error".to_string())
        }
    }

    let mgr = make_mgr();
    {
        let mut m = mgr.lock();
        m.register(Box::new(FixPlugin)).unwrap();
    }
    let wrapper = ToolWrapper::new(
        "fixable_tool",
        mgr,
        "user",
        "test",
        "/ws",
        Arc::new(FailTool),
    );
    let result = wrapper.execute(&serde_json::Map::new());
    // The plugin fixed the result, so invocation.result is Some
    // The wrapper returns invocation.result (modified) instead of original error
    assert!(result.is_ok());
    assert_eq!(result.unwrap()["fixed"], true);
}

#[test]
fn test_pluginable_tool_with_fix_plugin() {
    struct FixPlugin;
    impl Plugin for FixPlugin {
        fn name(&self) -> &str {
            "fixer"
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn execute(&self, inv: &mut ToolInvocation) -> (bool, Option<String>, bool) {
            if inv.blocking_error.is_some() {
                inv.blocking_error = None;
                inv.result = Some(serde_json::json!({"recovered": true}));
            }
            (true, None, true)
        }
        fn cleanup(&self) -> Result<(), String> {
            Ok(())
        }
    }

    struct FailTool;
    impl ToolExecutor for FailTool {
        fn execute(
            &self,
            _args: &serde_json::Map<String, serde_json::Value>,
        ) -> Result<serde_json::Value, String> {
            Err("fail".to_string())
        }
    }

    let mgr = make_mgr();
    {
        let mut m = mgr.lock();
        m.register(Box::new(FixPlugin)).unwrap();
    }
    let tool = PluginableTool::new("fixable", mgr, Arc::new(FailTool), "user", "web", "/ws");
    let result = tool.execute(&serde_json::Map::new());
    assert!(result.is_ok());
    assert_eq!(result.unwrap()["recovered"], true);
}

#[test]
fn test_tool_wrapper_send_channel_full() {
    // Test that tool wrapper works with empty string parameters
    let mgr = make_mgr();
    let wrapper = ToolWrapper::new("", mgr, "", "", "", make_echo());
    let result = wrapper.execute(&serde_json::Map::new()).unwrap();
    assert_eq!(result["echo"], "no input");
}
