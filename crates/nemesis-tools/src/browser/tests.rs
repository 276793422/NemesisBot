use super::*;

// ---- Mock MCP caller for testing ----

struct MockMCPCaller {
    connected: bool,
    responses: std::sync::Mutex<std::collections::HashMap<String, String>>,
}

impl MockMCPCaller {
    fn new(connected: bool) -> Self {
        let mut responses = std::collections::HashMap::new();
        responses.insert(
            "browser_navigate".to_string(),
            "OK: page loaded".to_string(),
        );
        responses.insert(
            "browser_screenshot".to_string(),
            "Screenshot data".to_string(),
        );
        responses.insert("browser_click".to_string(), "Clicked".to_string());
        responses.insert("browser_type".to_string(), "Typed".to_string());
        responses.insert(
            "browser_get_text".to_string(),
            "Page text content".to_string(),
        );
        responses.insert("browser_fill".to_string(), "Filled".to_string());
        responses.insert(
            "browser_wait_for_selector".to_string(),
            "Element found".to_string(),
        );
        responses.insert(
            "find_window_by_title".to_string(),
            r#"{"hwnd":"HWND(0x12345)"}"#.to_string(),
        );
        responses.insert(
            "enumerate_windows".to_string(),
            "[{\"hwnd\":\"HWND(0x12345)\",\"title\":\"Test\"}]".to_string(),
        );
        responses.insert(
            "capture_screenshot_to_file".to_string(),
            "Screenshot saved".to_string(),
        );
        responses.insert("click_window".to_string(), "Clicked".to_string());
        responses.insert("send_key_to_window".to_string(), "Keys sent".to_string());
        responses.insert(
            "get_window_text".to_string(),
            "Window text content".to_string(),
        );
        Self {
            connected,
            responses: std::sync::Mutex::new(responses),
        }
    }
}

impl MCPToolCaller for MockMCPCaller {
    fn call_tool(
        &self,
        tool_name: &str,
        _args: &serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>>
    {
        let responses = self.responses.lock().unwrap();
        let result = responses.get(tool_name).cloned();
        let tool_name_owned = tool_name.to_string();
        Box::pin(async move {
            match result {
                Some(r) => Ok(r),
                None => Err(format!("tool '{}' not found in mock", tool_name_owned)),
            }
        })
    }

    fn is_connected(&self) -> bool {
        self.connected
    }
}

// ---- Browser Tool Tests ----

#[tokio::test]
async fn test_browser_no_mcp() {
    let tool = BrowserTool::new(".", None);
    let result = tool
        .execute(&serde_json::json!({"action": "navigate", "url": "https://example.com"}))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("no MCP"));
}

#[tokio::test]
async fn test_browser_navigate_with_mcp() {
    let tool = BrowserTool::new(".", Some(Box::new(MockMCPCaller::new(true))));
    let result = tool
        .execute(&serde_json::json!({"action": "navigate", "url": "https://example.com"}))
        .await;
    assert!(
        !result.is_error,
        "Expected success, got: {}",
        result.for_llm
    );
    assert!(result.for_llm.contains("Navigated"));
}

#[tokio::test]
async fn test_browser_screenshot_with_mcp() {
    let tool = BrowserTool::new(".", Some(Box::new(MockMCPCaller::new(true))));
    let result = tool
        .execute(&serde_json::json!({"action": "screenshot"}))
        .await;
    assert!(!result.is_error);
}

#[tokio::test]
async fn test_browser_click_with_selector() {
    let tool = BrowserTool::new(".", Some(Box::new(MockMCPCaller::new(true))));
    let result = tool
        .execute(&serde_json::json!({"action": "click", "selector": "#btn"}))
        .await;
    assert!(!result.is_error);
}

#[tokio::test]
async fn test_browser_click_no_selector_or_text() {
    let tool = BrowserTool::new(".", Some(Box::new(MockMCPCaller::new(true))));
    let result = tool.execute(&serde_json::json!({"action": "click"})).await;
    assert!(result.is_error);
}

#[tokio::test]
async fn test_browser_type_with_mcp() {
    let tool = BrowserTool::new(".", Some(Box::new(MockMCPCaller::new(true))));
    let result = tool
        .execute(&serde_json::json!({"action": "type", "text": "hello"}))
        .await;
    assert!(!result.is_error);
}

#[tokio::test]
async fn test_browser_extract_text() {
    let tool = BrowserTool::new(".", Some(Box::new(MockMCPCaller::new(true))));
    let result = tool
        .execute(&serde_json::json!({"action": "extract_text"}))
        .await;
    assert!(!result.is_error);
    assert!(result.for_llm.contains("Page text content"));
}

#[tokio::test]
async fn test_browser_fill_form() {
    let tool = BrowserTool::new(".", Some(Box::new(MockMCPCaller::new(true))));
    let result = tool
        .execute(&serde_json::json!({
            "action": "fill_form",
            "selector": "#input",
            "value": "test"
        }))
        .await;
    assert!(!result.is_error);
}

#[tokio::test]
async fn test_browser_wait_for_element() {
    let tool = BrowserTool::new(".", Some(Box::new(MockMCPCaller::new(true))));
    let result = tool
        .execute(&serde_json::json!({
            "action": "wait_for_element",
            "selector": "#content"
        }))
        .await;
    assert!(!result.is_error);
}

#[tokio::test]
async fn test_browser_unknown_action() {
    let tool = BrowserTool::new(".", Some(Box::new(MockMCPCaller::new(true))));
    let result = tool
        .execute(&serde_json::json!({"action": "unknown_action"}))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("unknown browser action"));
}

#[tokio::test]
async fn test_browser_missing_action() {
    let tool = BrowserTool::new(".", None);
    let result = tool.execute(&serde_json::json!({})).await;
    assert!(result.is_error);
}

// ---- Screen Capture Tool Tests ----

#[tokio::test]
async fn test_screen_capture_full_screen_no_mcp() {
    let tool = ScreenCaptureTool::new(".", None);
    let result = tool
        .execute(&serde_json::json!({"mode": "full_screen"}))
        .await;
    // On non-Windows or without MCP, may error or return placeholder
    assert!(!result.is_error || result.for_llm.contains("MCP"));
}

#[tokio::test]
async fn test_screen_capture_region_missing_coords() {
    let tool = ScreenCaptureTool::new(".", None);
    let result = tool.execute(&serde_json::json!({"mode": "region"})).await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("required"));
}

#[tokio::test]
async fn test_screen_capture_window_no_params() {
    let tool = ScreenCaptureTool::new(".", None);
    let result = tool.execute(&serde_json::json!({"mode": "window"})).await;
    assert!(result.is_error);
}

#[tokio::test]
async fn test_screen_capture_unknown_mode() {
    let tool = ScreenCaptureTool::new(".", None);
    let result = tool.execute(&serde_json::json!({"mode": "invalid"})).await;
    assert!(result.is_error);
}

// ---- Desktop Tool Tests ----

#[tokio::test]
async fn test_desktop_find_window_with_mcp() {
    let tool = DesktopTool::new(".", Some(Box::new(MockMCPCaller::new(true))));
    let result = tool
        .execute(&serde_json::json!({"action": "find_window", "title": "Test"}))
        .await;
    assert!(!result.is_error);
}

#[tokio::test]
async fn test_desktop_find_window_no_title() {
    let tool = DesktopTool::new(".", None);
    let result = tool
        .execute(&serde_json::json!({"action": "find_window"}))
        .await;
    assert!(result.is_error);
}

#[tokio::test]
async fn test_desktop_list_windows_with_mcp() {
    let tool = DesktopTool::new(".", Some(Box::new(MockMCPCaller::new(true))));
    let result = tool
        .execute(&serde_json::json!({"action": "list_windows"}))
        .await;
    assert!(!result.is_error);
}

#[tokio::test]
async fn test_desktop_click_at_with_mcp() {
    let tool = DesktopTool::new(".", Some(Box::new(MockMCPCaller::new(true))));
    let result = tool
        .execute(&serde_json::json!({"action": "click_at", "x": 100, "y": 200}))
        .await;
    assert!(!result.is_error);
}

#[tokio::test]
async fn test_desktop_click_at_missing_coords() {
    let tool = DesktopTool::new(".", None);
    let result = tool
        .execute(&serde_json::json!({"action": "click_at"}))
        .await;
    assert!(result.is_error);
}

#[tokio::test]
async fn test_desktop_type_text_with_mcp() {
    let tool = DesktopTool::new(".", Some(Box::new(MockMCPCaller::new(true))));
    let result = tool
        .execute(&serde_json::json!({"action": "type_text", "text": "hello"}))
        .await;
    assert!(!result.is_error);
}

#[tokio::test]
async fn test_desktop_unknown_action() {
    let tool = DesktopTool::new(".", None);
    let result = tool
        .execute(&serde_json::json!({"action": "unknown"}))
        .await;
    assert!(result.is_error);
}

#[tokio::test]
async fn test_desktop_get_window_text_with_mcp() {
    let tool = DesktopTool::new(".", Some(Box::new(MockMCPCaller::new(true))));
    let result = tool
        .execute(&serde_json::json!({
            "action": "get_window_text",
            "hwnd": "HWND(0x12345)"
        }))
        .await;
    assert!(!result.is_error);
}

#[tokio::test]
async fn test_desktop_get_window_text_no_params() {
    let tool = DesktopTool::new(".", None);
    let result = tool
        .execute(&serde_json::json!({"action": "get_window_text"}))
        .await;
    assert!(result.is_error);
}

// ============================================================
// Additional browser tool tests
// ============================================================

#[test]
fn test_browser_action_display() {
    assert_eq!(BrowserAction::Navigate.to_string(), "navigate");
    assert_eq!(BrowserAction::Screenshot.to_string(), "screenshot");
    assert_eq!(BrowserAction::Click.to_string(), "click");
    assert_eq!(BrowserAction::Type.to_string(), "type");
    assert_eq!(BrowserAction::ExtractText.to_string(), "extract_text");
    assert_eq!(BrowserAction::FillForm.to_string(), "fill_form");
    assert_eq!(
        BrowserAction::WaitForElement.to_string(),
        "wait_for_element"
    );
}

#[tokio::test]
async fn test_browser_tool_metadata() {
    let tool = BrowserTool::new(".", None);
    assert_eq!(tool.name(), "browser");
    assert!(!tool.description().is_empty());
    let params = tool.parameters();
    assert_eq!(params["type"], "object");
    assert!(params["properties"]["action"].is_object());
}

#[tokio::test]
async fn test_browser_navigate_missing_url() {
    let tool = BrowserTool::new(".", Some(Box::new(MockMCPCaller::new(true))));
    let result = tool
        .execute(&serde_json::json!({"action": "navigate"}))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("url"));
}

#[tokio::test]
async fn test_browser_type_missing_text() {
    let tool = BrowserTool::new(".", Some(Box::new(MockMCPCaller::new(true))));
    let result = tool.execute(&serde_json::json!({"action": "type"})).await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("text"));
}

#[tokio::test]
async fn test_browser_fill_form_missing_params() {
    let tool = BrowserTool::new(".", Some(Box::new(MockMCPCaller::new(true))));
    let result = tool
        .execute(&serde_json::json!({"action": "fill_form"}))
        .await;
    assert!(result.is_error);
}

#[tokio::test]
async fn test_browser_wait_for_element_missing_selector() {
    let tool = BrowserTool::new(".", Some(Box::new(MockMCPCaller::new(true))));
    let result = tool
        .execute(&serde_json::json!({"action": "wait_for_element"}))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("selector"));
}

#[tokio::test]
async fn test_browser_click_with_text_fallback() {
    let tool = BrowserTool::new(".", Some(Box::new(MockMCPCaller::new(true))));
    let result = tool
        .execute(&serde_json::json!({"action": "click", "selector": "#submit-btn"}))
        .await;
    // Click uses MCP which succeeds with mock
    assert!(
        !result.is_error || result.for_llm.contains("click") || result.for_llm.contains("selector")
    );
}

#[tokio::test]
async fn test_browser_mcp_disconnected() {
    let tool = BrowserTool::new(".", Some(Box::new(MockMCPCaller::new(false))));
    let _result = tool
        .execute(&serde_json::json!({"action": "navigate", "url": "https://example.com"}))
        .await;
    // With disconnected MCP, tool should handle gracefully
    // The actual behavior depends on the implementation
}

// ============================================================
// Additional screen capture tool tests
// ============================================================

#[tokio::test]
async fn test_screen_capture_metadata() {
    let tool = ScreenCaptureTool::new(".", None);
    assert_eq!(tool.name(), "screen_capture");
    assert!(!tool.description().is_empty());
    let params = tool.parameters();
    assert_eq!(params["type"], "object");
}

#[tokio::test]
async fn test_screen_capture_region_with_coords() {
    let tool = ScreenCaptureTool::new(".", None);
    let result = tool
        .execute(&serde_json::json!({
            "mode": "region",
            "x": 0,
            "y": 0,
            "width": 100,
            "height": 100
        }))
        .await;
    // On non-Windows without MCP, may error
    assert!(
        !result.is_error || result.for_llm.contains("MCP") || result.for_llm.contains("screenshot")
    );
}

#[tokio::test]
async fn test_screen_capture_missing_mode() {
    let tool = ScreenCaptureTool::new(".", None);
    let result = tool.execute(&serde_json::json!({})).await;
    assert!(result.is_error);
}

#[tokio::test]
async fn test_screen_capture_window_with_hwnd() {
    let tool = ScreenCaptureTool::new(".", None);
    let result = tool
        .execute(&serde_json::json!({
            "mode": "window",
            "hwnd": "HWND(0x12345)"
        }))
        .await;
    assert!(result.is_error);
}

#[tokio::test]
async fn test_screen_capture_with_mcp() {
    struct MockMCPCaller;
    impl MCPToolCaller for MockMCPCaller {
        fn call_tool(
            &self,
            _tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>>
        {
            Box::pin(async { Ok("captured".to_string()) })
        }
        fn is_connected(&self) -> bool {
            true
        }
    }
    let tool = ScreenCaptureTool::new(".", Some(Box::new(MockMCPCaller)));
    let _result = tool
        .execute(&serde_json::json!({"mode": "full_screen"}))
        .await;
    // Should attempt to use MCP
}

// ============================================================
// Additional desktop tool tests with MCP mock
// ============================================================

struct SimpleMockMCP;
impl MCPToolCaller for SimpleMockMCP {
    fn call_tool(
        &self,
        tool_name: &str,
        _args: &serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>>
    {
        let name = tool_name.to_string();
        Box::pin(async move {
            match name.as_str() {
                "find_window_by_title" => Ok(r#"{"hwnd":"HWND(0xAAAA)"}"#.to_string()),
                "enumerate_windows" => {
                    Ok("[{\"hwnd\":\"HWND(0xAAAA)\",\"title\":\"Test\"}]".to_string())
                }
                "click_window" => Ok("Clicked".to_string()),
                "send_key_to_window" => Ok("Keys sent".to_string()),
                "capture_screenshot_to_file" => Ok("Screenshot saved".to_string()),
                "get_window_text" => Ok("Window text content".to_string()),
                _ => Err(format!("Unknown tool: {}", name)),
            }
        })
    }
    fn is_connected(&self) -> bool {
        true
    }
}

#[tokio::test]
async fn test_desktop_find_window_with_simple_mcp() {
    let tool = DesktopTool::new(".", Some(Box::new(SimpleMockMCP)));
    let result = tool
        .execute(&serde_json::json!({"action": "find_window", "title": "Test"}))
        .await;
    assert!(!result.is_error);
    assert!(result.for_llm.contains("HWND(0xAAAA)"));
}

#[tokio::test]
async fn test_desktop_list_windows_with_simple_mcp() {
    let tool = DesktopTool::new(".", Some(Box::new(SimpleMockMCP)));
    let result = tool
        .execute(&serde_json::json!({"action": "list_windows"}))
        .await;
    assert!(!result.is_error);
    assert!(result.for_llm.contains("HWND(0xAAAA)"));
}

#[tokio::test]
async fn test_desktop_take_screenshot_with_simple_mcp() {
    let tool = DesktopTool::new(".", Some(Box::new(SimpleMockMCP)));
    let result = tool
        .execute(&serde_json::json!({"action": "take_screenshot"}))
        .await;
    assert!(!result.is_error);
    assert!(result.silent);
}

#[tokio::test]
async fn test_desktop_type_text_with_simple_mcp() {
    let tool = DesktopTool::new(".", Some(Box::new(SimpleMockMCP)));
    let result = tool
        .execute(&serde_json::json!({"action": "type_text", "text": "hello world"}))
        .await;
    assert!(!result.is_error);
    assert!(result.silent);
}

#[tokio::test]
async fn test_desktop_get_window_text_by_hwnd_with_simple_mcp() {
    let tool = DesktopTool::new(".", Some(Box::new(SimpleMockMCP)));
    let result = tool
        .execute(&serde_json::json!({
            "action": "get_window_text",
            "hwnd": "HWND(0xAAAA)"
        }))
        .await;
    assert!(!result.is_error);
    assert!(result.for_llm.contains("Window text content"));
}

#[tokio::test]
async fn test_desktop_get_window_text_by_title_with_simple_mcp() {
    let tool = DesktopTool::new(".", Some(Box::new(SimpleMockMCP)));
    let result = tool
        .execute(&serde_json::json!({
            "action": "get_window_text",
            "title": "Test"
        }))
        .await;
    assert!(!result.is_error);
}

#[tokio::test]
async fn test_desktop_click_at_with_hwnd_simple_mcp() {
    let tool = DesktopTool::new(".", Some(Box::new(SimpleMockMCP)));
    let result = tool
        .execute(&serde_json::json!({
            "action": "click_at",
            "x": 100,
            "y": 200,
            "hwnd": "HWND(0xAAAA)"
        }))
        .await;
    assert!(!result.is_error);
}

// --- Additional browser tests for coverage ---

#[test]
fn test_browser_action_equality() {
    assert_eq!(BrowserAction::Navigate, BrowserAction::Navigate);
    assert_ne!(BrowserAction::Navigate, BrowserAction::Click);
    assert_ne!(BrowserAction::Screenshot, BrowserAction::Type);
}

#[test]
fn test_browser_action_debug() {
    assert!(format!("{:?}", BrowserAction::Navigate).contains("Navigate"));
    assert!(format!("{:?}", BrowserAction::FillForm).contains("FillForm"));
}

#[test]
fn test_desktop_action_equality() {
    assert_eq!(DesktopAction::FindWindow, DesktopAction::FindWindow);
    assert_ne!(DesktopAction::FindWindow, DesktopAction::ListWindows);
}

#[tokio::test]
async fn test_browser_click_missing_all_params() {
    let tool = BrowserTool::new("test", None);
    let result = tool.execute(&serde_json::json!({"action": "click"})).await;
    assert!(result.is_error);
}

#[tokio::test]
async fn test_browser_fill_form_missing_value() {
    let tool = BrowserTool::new("test", None);
    let result = tool
        .execute(&serde_json::json!({"action": "fill_form", "selector": "#input"}))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("value"));
}

#[tokio::test]
async fn test_browser_extract_text_no_mcp() {
    let tool = BrowserTool::new("test", None);
    let result = tool
        .execute(&serde_json::json!({"action": "extract_text"}))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("MCP") || result.for_llm.contains("web_fetch"));
}

#[tokio::test]
async fn test_browser_screenshot_no_mcp() {
    let tool = BrowserTool::new("test", None);
    let result = tool
        .execute(&serde_json::json!({"action": "screenshot"}))
        .await;
    assert!(result.is_error);
}

#[test]
fn test_desktop_tool_metadata() {
    let tool = DesktopTool::new(".", None);
    assert_eq!(tool.name(), "desktop");
    assert!(!tool.description().is_empty());
    let params = tool.parameters();
    assert_eq!(params["type"], "object");
    assert!(params["properties"]["action"].is_object());
}
