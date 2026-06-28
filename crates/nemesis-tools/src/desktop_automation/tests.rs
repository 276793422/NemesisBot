use super::*;

#[test]
fn test_desktop_action_from_str() {
    assert_eq!(
        DesktopAction::from_str("find_window").unwrap(),
        DesktopAction::FindWindow
    );
    assert_eq!(
        DesktopAction::from_str("list_windows").unwrap(),
        DesktopAction::ListWindows
    );
    assert_eq!(
        DesktopAction::from_str("click_at").unwrap(),
        DesktopAction::ClickAt
    );
    assert_eq!(
        DesktopAction::from_str("type_text").unwrap(),
        DesktopAction::TypeText
    );
    assert_eq!(
        DesktopAction::from_str("take_screenshot").unwrap(),
        DesktopAction::Screenshot
    );
    assert_eq!(
        DesktopAction::from_str("get_window_text").unwrap(),
        DesktopAction::GetWindowText
    );
    assert!(DesktopAction::from_str("invalid").is_err());
}

#[test]
fn test_desktop_action_display() {
    assert_eq!(DesktopAction::FindWindow.to_string(), "find_window");
    assert_eq!(DesktopAction::ListWindows.to_string(), "list_windows");
    assert_eq!(DesktopAction::ClickAt.to_string(), "click_at");
    assert_eq!(DesktopAction::TypeText.to_string(), "type_text");
    assert_eq!(DesktopAction::Screenshot.to_string(), "take_screenshot");
    assert_eq!(DesktopAction::GetWindowText.to_string(), "get_window_text");
}

#[tokio::test]
async fn test_desktop_tool_metadata() {
    let tool = DesktopTool::new(PathBuf::from("/tmp"), None);
    assert_eq!(tool.name(), "desktop");
    assert!(!tool.description().is_empty());
}

#[tokio::test]
async fn test_desktop_tool_missing_action() {
    let tool = DesktopTool::new(PathBuf::from("/tmp"), None);
    let result = tool.execute(&serde_json::json!({})).await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("'action' is required"));
}

#[tokio::test]
async fn test_desktop_tool_unknown_action() {
    let tool = DesktopTool::new(PathBuf::from("/tmp"), None);
    let result = tool
        .execute(&serde_json::json!({"action": "unknown_action"}))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("unknown desktop action"));
}

#[tokio::test]
async fn test_desktop_tool_find_window_missing_title() {
    let tool = DesktopTool::new(PathBuf::from("/tmp"), None);
    let result = tool
        .execute(&serde_json::json!({"action": "find_window"}))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("'title' is required"));
}

#[tokio::test]
async fn test_desktop_tool_click_at_missing_coords() {
    let tool = DesktopTool::new(PathBuf::from("/tmp"), None);
    let result = tool
        .execute(&serde_json::json!({"action": "click_at"}))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("'x' and 'y' are required"));
}

#[tokio::test]
async fn test_desktop_tool_type_text_missing_text() {
    let tool = DesktopTool::new(PathBuf::from("/tmp"), None);
    let result = tool
        .execute(&serde_json::json!({"action": "type_text"}))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("'text' is required"));
}

#[tokio::test]
async fn test_desktop_tool_get_window_text_missing_params() {
    let tool = DesktopTool::new(PathBuf::from("/tmp"), None);
    let result = tool
        .execute(&serde_json::json!({"action": "get_window_text"}))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("'hwnd' or 'title' is required"));
}

#[tokio::test]
async fn test_desktop_tool_parameters_schema() {
    let tool = DesktopTool::new(PathBuf::from("/tmp"), None);
    let params = tool.parameters();

    // Verify required fields
    let required = params["required"].as_array().unwrap();
    assert!(required.iter().any(|r| r.as_str() == Some("action")));

    // Verify action enum values
    let action_enum = params["properties"]["action"]["enum"].as_array().unwrap();
    assert_eq!(action_enum.len(), 6);
    assert!(action_enum.iter().any(|v| v.as_str() == Some("find_window")));
    assert!(action_enum.iter().any(|v| v.as_str() == Some("list_windows")));
    assert!(action_enum.iter().any(|v| v.as_str() == Some("click_at")));
    assert!(action_enum.iter().any(|v| v.as_str() == Some("type_text")));
    assert!(action_enum.iter().any(|v| v.as_str() == Some("take_screenshot")));
    assert!(action_enum.iter().any(|v| v.as_str() == Some("get_window_text")));
}

#[tokio::test]
async fn test_desktop_tool_no_mcp() {
    let tool = DesktopTool::new(PathBuf::from("/tmp"), None);
    assert!(!tool.has_mcp());
}

// ============================================================
// Additional desktop automation tests
// ============================================================

#[test]
fn test_desktop_action_all_variants() {
    assert!(DesktopAction::from_str("find_window").is_ok());
    assert!(DesktopAction::from_str("list_windows").is_ok());
    assert!(DesktopAction::from_str("click_at").is_ok());
    assert!(DesktopAction::from_str("type_text").is_ok());
    assert!(DesktopAction::from_str("take_screenshot").is_ok());
    assert!(DesktopAction::from_str("get_window_text").is_ok());
}

#[test]
fn test_desktop_action_display_roundtrip() {
    let actions = [
        DesktopAction::FindWindow,
        DesktopAction::ListWindows,
        DesktopAction::ClickAt,
        DesktopAction::TypeText,
        DesktopAction::Screenshot,
        DesktopAction::GetWindowText,
    ];
    for action in &actions {
        let s = action.to_string();
        let parsed = DesktopAction::from_str(&s);
        assert_eq!(parsed.unwrap(), *action);
    }
}

#[test]
fn test_window_info_serialization() {
    let info = WindowInfo {
        hwnd: "HWND(0x12345)".to_string(),
        title: "Test Window".to_string(),
        class_name: "TestClass".to_string(),
        left: 100,
        top: 200,
        width: 800,
        height: 600,
    };
    let json = serde_json::to_string(&info).unwrap();
    let restored: WindowInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.hwnd, "HWND(0x12345)");
    assert_eq!(restored.title, "Test Window");
    assert_eq!(restored.width, 800);
}

#[test]
fn test_window_info_deserialization_from_json() {
    let json = r#"{"hwnd":"HWND(0xABCD)","title":"MyApp","class_name":"Chrome","left":0,"top":0,"width":1920,"height":1080}"#;
    let info: WindowInfo = serde_json::from_str(json).unwrap();
    assert_eq!(info.hwnd, "HWND(0xABCD)");
    assert_eq!(info.title, "MyApp");
    assert_eq!(info.width, 1920);
}

#[tokio::test]
async fn test_desktop_tool_description_not_empty() {
    let tool = DesktopTool::new(PathBuf::from("/tmp"), None);
    assert!(tool.description().len() > 50);
}

#[tokio::test]
async fn test_desktop_tool_parameters_valid() {
    let tool = DesktopTool::new(PathBuf::from("/tmp"), None);
    let params = tool.parameters();
    assert_eq!(params["type"], "object");
    assert!(params["properties"]["action"].is_object());
    assert!(params["required"].is_array());
}

#[tokio::test]
async fn test_desktop_tool_missing_action_returns_error() {
    let tool = DesktopTool::new(PathBuf::from("/tmp"), None);
    let result = tool.execute(&serde_json::json!({})).await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("action"));
}

#[tokio::test]
async fn test_desktop_click_at_with_mcp_and_button() {
    // Use MCP mock by reusing the test module's mock
    struct MockMCPCaller;
    impl crate::browser::MCPToolCaller for MockMCPCaller {
        fn call_tool(
            &self,
            _tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            Box::pin(async { Ok("clicked".to_string()) })
        }
        fn is_connected(&self) -> bool { true }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(MockMCPCaller)));
    let result = tool
        .execute(&serde_json::json!({"action": "click_at", "x": 50, "y": 100, "button": "right"}))
        .await;
    assert!(!result.is_error);
}

// ============================================================
// Additional tests for coverage improvement
// ============================================================

#[test]
fn test_desktop_action_equality() {
    assert_eq!(DesktopAction::FindWindow, DesktopAction::FindWindow);
    assert_ne!(DesktopAction::FindWindow, DesktopAction::ListWindows);
    assert_ne!(DesktopAction::ClickAt, DesktopAction::TypeText);
}

#[test]
fn test_desktop_action_debug_format() {
    assert!(format!("{:?}", DesktopAction::Screenshot).contains("Screenshot"));
    assert!(format!("{:?}", DesktopAction::GetWindowText).contains("GetWindowText"));
}

#[test]
fn test_window_info_default_values() {
    let json = r#"{"hwnd": "1", "title": "w"}"#;
    let info: WindowInfo = serde_json::from_str(json).unwrap();
    assert_eq!(info.class_name, "");
    assert_eq!(info.left, 0);
    assert_eq!(info.top, 0);
    assert_eq!(info.width, 0);
    assert_eq!(info.height, 0);
}

#[tokio::test]
async fn test_desktop_tool_set_timeout_value() {
    let tool = DesktopTool::new(PathBuf::from("/tmp"), None);
    let custom = Duration::from_secs(120);
    tool.set_timeout(custom).await;
    let t = *tool.timeout.lock().await;
    assert_eq!(t, Duration::from_secs(120));
}

#[tokio::test]
async fn test_desktop_tool_has_mcp_false_without_caller() {
    let tool = DesktopTool::new(PathBuf::from("/tmp"), None);
    assert!(!tool.has_mcp());
}

#[tokio::test]
async fn test_desktop_tool_find_window_empty_title() {
    let tool = DesktopTool::new(PathBuf::from("/tmp"), None);
    let result = tool.execute(&serde_json::json!({"action": "find_window", "title": ""})).await;
    assert!(result.is_error);
}

#[tokio::test]
async fn test_desktop_tool_click_at_only_x() {
    let tool = DesktopTool::new(PathBuf::from("/tmp"), None);
    let result = tool.execute(&serde_json::json!({"action": "click_at", "x": 50})).await;
    assert!(result.is_error);
}

#[tokio::test]
async fn test_desktop_tool_click_at_only_y() {
    let tool = DesktopTool::new(PathBuf::from("/tmp"), None);
    let result = tool.execute(&serde_json::json!({"action": "click_at", "y": 50})).await;
    assert!(result.is_error);
}

#[tokio::test]
async fn test_desktop_tool_type_text_empty() {
    let tool = DesktopTool::new(PathBuf::from("/tmp"), None);
    let result = tool.execute(&serde_json::json!({"action": "type_text", "text": ""})).await;
    assert!(result.is_error);
}

#[tokio::test]
async fn test_desktop_tool_get_window_text_with_hwnd_no_mcp() {
    let tool = DesktopTool::new(PathBuf::from("/tmp"), None);
    let result = tool.execute(&serde_json::json!({"action": "get_window_text", "hwnd": "12345"})).await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("window-mcp"));
}

#[tokio::test]
async fn test_desktop_tool_list_windows_no_mcp() {
    let tool = DesktopTool::new(PathBuf::from("/tmp"), None);
    // This should try PowerShell and either succeed or fail - just verify no panic
    let _ = tool.execute(&serde_json::json!({"action": "list_windows"})).await;
}

#[tokio::test]
async fn test_desktop_tool_mcp_find_window() {
    struct MockMCPCaller;
    impl crate::browser::MCPToolCaller for MockMCPCaller {
        fn call_tool(
            &self,
            tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            let name = tool_name.to_string();
            Box::pin(async move {
                match name.as_str() {
                    "find_window_by_title" => Ok(r#"{"hwnd":"HWND(0x999)","title":"Test"}"#.to_string()),
                    _ => Err("unknown tool".to_string()),
                }
            })
        }
        fn is_connected(&self) -> bool { true }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(MockMCPCaller)));
    let result = tool.execute(&serde_json::json!({"action": "find_window", "title": "Test"})).await;
    assert!(!result.is_error);
}

#[tokio::test]
async fn test_desktop_tool_mcp_list_windows() {
    struct MockMCPCaller;
    impl crate::browser::MCPToolCaller for MockMCPCaller {
        fn call_tool(
            &self,
            tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            let name = tool_name.to_string();
            Box::pin(async move {
                match name.as_str() {
                    "enumerate_windows" => Ok(r#"[{"hwnd":"1","title":"Win1"}]"#.to_string()),
                    _ => Err("unknown tool".to_string()),
                }
            })
        }
        fn is_connected(&self) -> bool { true }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(MockMCPCaller)));
    let result = tool.execute(&serde_json::json!({"action": "list_windows"})).await;
    assert!(!result.is_error);
}

#[tokio::test]
async fn test_desktop_tool_mcp_screenshot() {
    struct MockMCPCaller;
    impl crate::browser::MCPToolCaller for MockMCPCaller {
        fn call_tool(
            &self,
            tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            let name = tool_name.to_string();
            Box::pin(async move {
                match name.as_str() {
                    "capture_screenshot_to_file" => Ok("screenshot saved".to_string()),
                    _ => Err("unknown tool".to_string()),
                }
            })
        }
        fn is_connected(&self) -> bool { true }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(MockMCPCaller)));
    let result = tool.execute(&serde_json::json!({"action": "take_screenshot"})).await;
    assert!(!result.is_error);
}

#[tokio::test]
async fn test_desktop_tool_mcp_type_text() {
    struct MockMCPCaller;
    impl crate::browser::MCPToolCaller for MockMCPCaller {
        fn call_tool(
            &self,
            tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            let name = tool_name.to_string();
            Box::pin(async move {
                match name.as_str() {
                    "send_key_to_window" => Ok("keys sent".to_string()),
                    _ => Err("unknown tool".to_string()),
                }
            })
        }
        fn is_connected(&self) -> bool { true }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(MockMCPCaller)));
    let result = tool.execute(&serde_json::json!({"action": "type_text", "text": "hello"})).await;
    assert!(!result.is_error);
}

#[test]
fn test_window_info_serialize_deserialize_large_values() {
    let info = WindowInfo {
        hwnd: "HWND(0xFFFFFFFF)".to_string(),
        title: "A".repeat(500),
        class_name: "BigClass".to_string(),
        left: i64::MAX,
        top: i64::MIN,
        width: 7680,
        height: 4320,
    };
    let json = serde_json::to_string(&info).unwrap();
    let restored: WindowInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.hwnd, info.hwnd);
    assert_eq!(restored.title.len(), 500);
    assert_eq!(restored.left, i64::MAX);
    assert_eq!(restored.width, 7680);
}

// ============================================================
// MCP error path coverage
// ============================================================

#[tokio::test]
async fn test_desktop_tool_mcp_find_window_error() {
    struct FailingMCPCaller;
    impl crate::browser::MCPToolCaller for FailingMCPCaller {
        fn call_tool(
            &self,
            _tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            Box::pin(async { Err("connection lost".to_string()) })
        }
        fn is_connected(&self) -> bool { true }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(FailingMCPCaller)));
    let result = tool.execute(&serde_json::json!({"action": "find_window", "title": "Test"})).await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("MCP find_window failed"));
}

#[tokio::test]
async fn test_desktop_tool_mcp_list_windows_error() {
    struct FailingMCPCaller;
    impl crate::browser::MCPToolCaller for FailingMCPCaller {
        fn call_tool(
            &self,
            _tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            Box::pin(async { Err("connection lost".to_string()) })
        }
        fn is_connected(&self) -> bool { true }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(FailingMCPCaller)));
    let result = tool.execute(&serde_json::json!({"action": "list_windows"})).await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("MCP list_windows failed"));
}

#[tokio::test]
async fn test_desktop_tool_mcp_click_at_error() {
    struct FailingMCPCaller;
    impl crate::browser::MCPToolCaller for FailingMCPCaller {
        fn call_tool(
            &self,
            _tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            Box::pin(async { Err("click failed".to_string()) })
        }
        fn is_connected(&self) -> bool { true }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(FailingMCPCaller)));
    let result = tool.execute(&serde_json::json!({"action": "click_at", "x": 50, "y": 100})).await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("MCP click_at failed"));
}

#[tokio::test]
async fn test_desktop_tool_mcp_type_text_error() {
    struct FailingMCPCaller;
    impl crate::browser::MCPToolCaller for FailingMCPCaller {
        fn call_tool(
            &self,
            _tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            Box::pin(async { Err("type failed".to_string()) })
        }
        fn is_connected(&self) -> bool { true }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(FailingMCPCaller)));
    let result = tool.execute(&serde_json::json!({"action": "type_text", "text": "hello"})).await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("MCP type_text failed"));
}

#[tokio::test]
async fn test_desktop_tool_mcp_screenshot_error() {
    struct FailingMCPCaller;
    impl crate::browser::MCPToolCaller for FailingMCPCaller {
        fn call_tool(
            &self,
            _tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            Box::pin(async { Err("screenshot failed".to_string()) })
        }
        fn is_connected(&self) -> bool { true }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(FailingMCPCaller)));
    let result = tool.execute(&serde_json::json!({"action": "take_screenshot"})).await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("MCP screenshot failed"));
}

#[tokio::test]
async fn test_desktop_tool_mcp_get_window_text_with_hwnd() {
    struct MockMCPCaller;
    impl crate::browser::MCPToolCaller for MockMCPCaller {
        fn call_tool(
            &self,
            tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            let name = tool_name.to_string();
            Box::pin(async move {
                match name.as_str() {
                    "get_window_text" => Ok("Window text content".to_string()),
                    _ => Err("unknown tool".to_string()),
                }
            })
        }
        fn is_connected(&self) -> bool { true }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(MockMCPCaller)));
    let result = tool.execute(&serde_json::json!({"action": "get_window_text", "hwnd": "HWND(0x123)"})).await;
    assert!(!result.is_error);
    assert!(result.for_llm.contains("Window text content"));
}

#[tokio::test]
async fn test_desktop_tool_mcp_get_window_text_by_title() {
    struct MockMCPCaller;
    impl crate::browser::MCPToolCaller for MockMCPCaller {
        fn call_tool(
            &self,
            tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            let name = tool_name.to_string();
            Box::pin(async move {
                match name.as_str() {
                    "find_window_by_title" => Ok(r#"{"hwnd":"HWND(0x456)"}"#.to_string()),
                    "get_window_text" => Ok("Found window text".to_string()),
                    _ => Err("unknown tool".to_string()),
                }
            })
        }
        fn is_connected(&self) -> bool { true }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(MockMCPCaller)));
    let result = tool.execute(&serde_json::json!({"action": "get_window_text", "title": "MyApp"})).await;
    assert!(!result.is_error);
    assert!(result.for_llm.contains("Found window text"));
}

#[tokio::test]
async fn test_desktop_tool_mcp_get_window_text_error() {
    struct FailingMCPCaller;
    impl crate::browser::MCPToolCaller for FailingMCPCaller {
        fn call_tool(
            &self,
            _tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            Box::pin(async { Err("get_text failed".to_string()) })
        }
        fn is_connected(&self) -> bool { true }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(FailingMCPCaller)));
    let result = tool.execute(&serde_json::json!({"action": "get_window_text", "hwnd": "12345"})).await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("MCP get_window_text failed"));
}

#[tokio::test]
async fn test_desktop_tool_mcp_get_window_text_by_title_find_fails() {
    struct MockMCPCaller;
    impl crate::browser::MCPToolCaller for MockMCPCaller {
        fn call_tool(
            &self,
            tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            let name = tool_name.to_string();
            Box::pin(async move {
                match name.as_str() {
                    "find_window_by_title" => Err("not found".to_string()),
                    _ => Err("unknown tool".to_string()),
                }
            })
        }
        fn is_connected(&self) -> bool { true }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(MockMCPCaller)));
    let result = tool.execute(&serde_json::json!({"action": "get_window_text", "title": "Nonexistent"})).await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("find_window"));
}

#[tokio::test]
async fn test_desktop_tool_mcp_get_window_text_by_title_find_returns_no_hwnd() {
    struct MockMCPCaller;
    impl crate::browser::MCPToolCaller for MockMCPCaller {
        fn call_tool(
            &self,
            tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            let name = tool_name.to_string();
            Box::pin(async move {
                match name.as_str() {
                    "find_window_by_title" => Ok(r#"{"title":"MyApp"}"#.to_string()), // No hwnd field
                    _ => Err("unknown tool".to_string()),
                }
            })
        }
        fn is_connected(&self) -> bool { true }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(MockMCPCaller)));
    let result = tool.execute(&serde_json::json!({"action": "get_window_text", "title": "MyApp"})).await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("resolve window handle"));
}

#[tokio::test]
async fn test_desktop_tool_mcp_click_at_with_hwnd() {
    struct MockMCPCaller;
    impl crate::browser::MCPToolCaller for MockMCPCaller {
        fn call_tool(
            &self,
            _tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            Box::pin(async { Ok("clicked".to_string()) })
        }
        fn is_connected(&self) -> bool { true }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(MockMCPCaller)));
    let result = tool.execute(&serde_json::json!({
        "action": "click_at", "x": 50, "y": 100, "hwnd": "HWND(0x999)"
    })).await;
    assert!(!result.is_error);
}

#[tokio::test]
async fn test_desktop_tool_mcp_type_text_with_hwnd() {
    struct MockMCPCaller;
    impl crate::browser::MCPToolCaller for MockMCPCaller {
        fn call_tool(
            &self,
            _tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            Box::pin(async { Ok("typed".to_string()) })
        }
        fn is_connected(&self) -> bool { true }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(MockMCPCaller)));
    let result = tool.execute(&serde_json::json!({
        "action": "type_text", "text": "hello", "hwnd": "HWND(0x999)"
    })).await;
    assert!(!result.is_error);
}

#[tokio::test]
async fn test_desktop_tool_mcp_screenshot_with_region() {
    struct MockMCPCaller;
    impl crate::browser::MCPToolCaller for MockMCPCaller {
        fn call_tool(
            &self,
            _tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            Box::pin(async { Ok("screenshot saved".to_string()) })
        }
        fn is_connected(&self) -> bool { true }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(MockMCPCaller)));
    let result = tool.execute(&serde_json::json!({
        "action": "take_screenshot", "x": 0, "y": 0, "width": 800, "height": 600
    })).await;
    assert!(!result.is_error);
}

#[tokio::test]
async fn test_desktop_tool_mcp_screenshot_with_hwnd() {
    struct MockMCPCaller;
    impl crate::browser::MCPToolCaller for MockMCPCaller {
        fn call_tool(
            &self,
            _tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            Box::pin(async { Ok("screenshot saved".to_string()) })
        }
        fn is_connected(&self) -> bool { true }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(MockMCPCaller)));
    let result = tool.execute(&serde_json::json!({
        "action": "take_screenshot", "hwnd": "HWND(0x123)"
    })).await;
    assert!(!result.is_error);
}

#[tokio::test]
async fn test_desktop_tool_mcp_list_windows_with_title_filter() {
    struct MockMCPCaller;
    impl crate::browser::MCPToolCaller for MockMCPCaller {
        fn call_tool(
            &self,
            tool_name: &str,
            args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            let name = tool_name.to_string();
            let has_title = args.get("title_contains").is_some();
            Box::pin(async move {
                match name.as_str() {
                    "enumerate_windows" => {
                        if has_title {
                            Ok(r#"[{"hwnd":"1","title":"Chrome"}]"#.to_string())
                        } else {
                            Ok(r#"[{"hwnd":"1","title":"Win1"}]"#.to_string())
                        }
                    }
                    _ => Err("unknown tool".to_string()),
                }
            })
        }
        fn is_connected(&self) -> bool { true }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(MockMCPCaller)));
    let result = tool.execute(&serde_json::json!({"action": "list_windows", "title": "Chrome"})).await;
    assert!(!result.is_error);
}

#[tokio::test]
async fn test_desktop_tool_mcp_disconnected() {
    struct DisconnectedMCPCaller;
    impl crate::browser::MCPToolCaller for DisconnectedMCPCaller {
        fn call_tool(
            &self,
            _tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            Box::pin(async { Ok("should not be called".to_string()) })
        }
        fn is_connected(&self) -> bool { false }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(DisconnectedMCPCaller)));
    assert!(!tool.has_mcp());
    // Should fall back to standalone PowerShell (or error on non-Windows)
    let _ = tool.execute(&serde_json::json!({"action": "list_windows"})).await;
}

// ============================================================
// Additional coverage tests for 95%+ target
// ============================================================

#[test]
fn test_desktop_tool_new_no_mcp() {
    let tool = DesktopTool::new(PathBuf::from("/tmp"), None);
    assert_eq!(tool.name(), "desktop");
    assert!(!tool.has_mcp());
}

#[test]
fn test_desktop_tool_new_with_mcp() {
    struct MockMCP;
    impl crate::browser::MCPToolCaller for MockMCP {
        fn call_tool(
            &self,
            _tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            Box::pin(async { Ok("ok".to_string()) })
        }
        fn is_connected(&self) -> bool { true }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(MockMCP)));
    assert!(tool.has_mcp());
}

#[test]
fn test_window_info_deserialize() {
    let json = r#"{"hwnd":"HWND(0x123)","title":"Test Window","class_name":"TestClass","left":10,"top":20,"width":800,"height":600}"#;
    let info: WindowInfo = serde_json::from_str(json).unwrap();
    assert_eq!(info.hwnd, "HWND(0x123)");
    assert_eq!(info.title, "Test Window");
    assert_eq!(info.class_name, "TestClass");
    assert_eq!(info.left, 10);
    assert_eq!(info.top, 20);
    assert_eq!(info.width, 800);
    assert_eq!(info.height, 600);
}

#[test]
fn test_window_info_deserialize_defaults() {
    let json = r#"{"hwnd":"HWND(0x1)","title":"Min"}"#;
    let info: WindowInfo = serde_json::from_str(json).unwrap();
    assert_eq!(info.hwnd, "HWND(0x1)");
    assert_eq!(info.title, "Min");
    assert_eq!(info.class_name, "");
    assert_eq!(info.left, 0);
    assert_eq!(info.top, 0);
    assert_eq!(info.width, 0);
    assert_eq!(info.height, 0);
}

#[test]
fn test_window_info_serialize_roundtrip() {
    let info = WindowInfo {
        hwnd: "HWND(0x999)".to_string(),
        title: "Roundtrip".to_string(),
        class_name: "RTClass".to_string(),
        left: 100,
        top: 200,
        width: 1024,
        height: 768,
    };
    let json = serde_json::to_string(&info).unwrap();
    let back: WindowInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(back.hwnd, info.hwnd);
    assert_eq!(back.title, info.title);
    assert_eq!(back.left, info.left);
}

#[test]
fn test_desktop_tool_parameters_structure() {
    let tool = DesktopTool::new(PathBuf::from("."), None);
    let params = tool.parameters();
    assert_eq!(params["type"], "object");
    assert!(params["properties"]["action"].is_object());
    assert!(params["required"].as_array().unwrap().contains(&serde_json::json!("action")));
}

#[tokio::test]
async fn test_desktop_tool_missing_action_v2() {
    let tool = DesktopTool::new(PathBuf::from("."), None);
    let result = tool.execute(&serde_json::json!({"action": "nonexistent"})).await;
    assert!(result.is_error);
}

#[tokio::test]
async fn test_desktop_tool_find_window_mcp_success() {
    struct MockMCPCaller;
    impl crate::browser::MCPToolCaller for MockMCPCaller {
        fn call_tool(
            &self,
            _tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            Box::pin(async { Ok(r#"{"hwnd":"HWND(0x100)","title":"MyApp"}"#.to_string()) })
        }
        fn is_connected(&self) -> bool { true }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(MockMCPCaller)));
    let result = tool.execute(&serde_json::json!({"action": "find_window", "title": "MyApp"})).await;
    assert!(!result.is_error);
    assert!(result.for_llm.contains("HWND(0x100)"));
}

#[tokio::test]
async fn test_desktop_tool_find_window_mcp_error() {
    struct FailingMCPCaller;
    impl crate::browser::MCPToolCaller for FailingMCPCaller {
        fn call_tool(
            &self,
            _tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            Box::pin(async { Err("find failed".to_string()) })
        }
        fn is_connected(&self) -> bool { true }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(FailingMCPCaller)));
    let result = tool.execute(&serde_json::json!({"action": "find_window", "title": "Test"})).await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("find_window failed"));
}

#[tokio::test]
async fn test_desktop_tool_find_window_missing_title_v2() {
    struct MockMCPCaller;
    impl crate::browser::MCPToolCaller for MockMCPCaller {
        fn call_tool(
            &self,
            _tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            Box::pin(async { Ok("ok".to_string()) })
        }
        fn is_connected(&self) -> bool { true }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(MockMCPCaller)));
    let result = tool.execute(&serde_json::json!({"action": "find_window"})).await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("title"));
}

#[tokio::test]
async fn test_desktop_tool_list_windows_mcp_error_v2() {
    struct FailingMCPCaller;
    impl crate::browser::MCPToolCaller for FailingMCPCaller {
        fn call_tool(
            &self,
            _tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            Box::pin(async { Err("list failed".to_string()) })
        }
        fn is_connected(&self) -> bool { true }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(FailingMCPCaller)));
    let result = tool.execute(&serde_json::json!({"action": "list_windows"})).await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("list_windows failed"));
}

#[tokio::test]
async fn test_desktop_tool_click_at_missing_y() {
    struct MockMCPCaller;
    impl crate::browser::MCPToolCaller for MockMCPCaller {
        fn call_tool(
            &self,
            _tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            Box::pin(async { Ok("ok".to_string()) })
        }
        fn is_connected(&self) -> bool { true }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(MockMCPCaller)));
    let result = tool.execute(&serde_json::json!({"action": "click_at", "x": 10})).await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("x") || result.for_llm.contains("y"));
}

#[tokio::test]
async fn test_desktop_tool_click_at_mcp_error() {
    struct FailingMCPCaller;
    impl crate::browser::MCPToolCaller for FailingMCPCaller {
        fn call_tool(
            &self,
            _tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            Box::pin(async { Err("click failed".to_string()) })
        }
        fn is_connected(&self) -> bool { true }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(FailingMCPCaller)));
    let result = tool.execute(&serde_json::json!({"action": "click_at", "x": 10, "y": 20})).await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("click_at failed"));
}

#[tokio::test]
async fn test_desktop_tool_type_text_mcp_error() {
    struct FailingMCPCaller;
    impl crate::browser::MCPToolCaller for FailingMCPCaller {
        fn call_tool(
            &self,
            _tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            Box::pin(async { Err("type failed".to_string()) })
        }
        fn is_connected(&self) -> bool { true }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(FailingMCPCaller)));
    let result = tool.execute(&serde_json::json!({"action": "type_text", "text": "hello"})).await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("type_text failed"));
}

#[tokio::test]
async fn test_desktop_tool_type_text_empty_text() {
    struct MockMCPCaller;
    impl crate::browser::MCPToolCaller for MockMCPCaller {
        fn call_tool(
            &self,
            _tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            Box::pin(async { Ok("ok".to_string()) })
        }
        fn is_connected(&self) -> bool { true }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(MockMCPCaller)));
    let result = tool.execute(&serde_json::json!({"action": "type_text", "text": ""})).await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("text"));
}

#[tokio::test]
async fn test_desktop_tool_get_window_text_no_params() {
    struct MockMCPCaller;
    impl crate::browser::MCPToolCaller for MockMCPCaller {
        fn call_tool(
            &self,
            _tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            Box::pin(async { Ok("ok".to_string()) })
        }
        fn is_connected(&self) -> bool { true }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(MockMCPCaller)));
    let result = tool.execute(&serde_json::json!({"action": "get_window_text"})).await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("hwnd") || result.for_llm.contains("title"));
}

#[tokio::test]
async fn test_desktop_tool_get_window_text_no_mcp() {
    let tool = DesktopTool::new(PathBuf::from("."), None);
    let result = tool.execute(&serde_json::json!({"action": "get_window_text", "hwnd": "123"})).await;
    // No MCP backend - should error about needing MCP
    // On non-Windows, platform check fires first
    if cfg!(target_os = "windows") {
        assert!(result.is_error);
        assert!(result.for_llm.contains("window-mcp") || result.for_llm.contains("requires"));
    }
}

#[tokio::test]
async fn test_desktop_tool_set_timeout() {
    let tool = DesktopTool::new(PathBuf::from("."), None);
    tool.set_timeout(Duration::from_secs(60)).await;
    let timeout = tool.timeout.lock().await;
    assert_eq!(*timeout, Duration::from_secs(60));
}

#[tokio::test]
async fn test_desktop_tool_screenshot_mcp_success() {
    struct ScreenshotMCPCaller;
    impl crate::browser::MCPToolCaller for ScreenshotMCPCaller {
        fn call_tool(
            &self,
            _tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            Box::pin(async { Ok("screenshot_saved.png".to_string()) })
        }
        fn is_connected(&self) -> bool { true }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(ScreenshotMCPCaller)));
    let result = tool.execute(&serde_json::json!({"action": "take_screenshot"})).await;
    assert!(!result.is_error);
    assert!(result.for_llm.contains("Screenshot"));
}

#[tokio::test]
async fn test_desktop_tool_screenshot_mcp_with_region() {
    struct ScreenshotMCPCaller;
    impl crate::browser::MCPToolCaller for ScreenshotMCPCaller {
        fn call_tool(
            &self,
            tool_name: &str,
            args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            let tool_name = tool_name.to_string();
            let args = args.clone();
            Box::pin(async move {
                assert_eq!(tool_name, "capture_screenshot_to_file");
                assert_eq!(args["x"], 10);
                assert_eq!(args["y"], 20);
                assert_eq!(args["width"], 100);
                assert_eq!(args["height"], 200);
                Ok("region_screenshot.png".to_string())
            })
        }
        fn is_connected(&self) -> bool { true }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(ScreenshotMCPCaller)));
    let result = tool.execute(&serde_json::json!({
        "action": "take_screenshot", "x": 10, "y": 20, "width": 100, "height": 200
    })).await;
    assert!(!result.is_error);
}

#[tokio::test]
async fn test_desktop_tool_screenshot_mcp_error() {
    struct FailingMCPCaller;
    impl crate::browser::MCPToolCaller for FailingMCPCaller {
        fn call_tool(
            &self,
            _tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            Box::pin(async { Err("screenshot failed".to_string()) })
        }
        fn is_connected(&self) -> bool { true }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(FailingMCPCaller)));
    let result = tool.execute(&serde_json::json!({"action": "take_screenshot"})).await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("screenshot failed"));
}

#[tokio::test]
async fn test_desktop_tool_list_windows_mcp_success() {
    struct ListMCPCaller;
    impl crate::browser::MCPToolCaller for ListMCPCaller {
        fn call_tool(
            &self,
            _tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            Box::pin(async { Ok("[{\"hwnd\":\"123\",\"title\":\"Test\"}]".to_string()) })
        }
        fn is_connected(&self) -> bool { true }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(ListMCPCaller)));
    let result = tool.execute(&serde_json::json!({"action": "list_windows"})).await;
    assert!(!result.is_error);
    assert!(result.for_llm.contains("Test"));
}

#[tokio::test]
async fn test_desktop_tool_list_windows_mcp_error() {
    struct FailingMCPCaller;
    impl crate::browser::MCPToolCaller for FailingMCPCaller {
        fn call_tool(
            &self,
            _tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            Box::pin(async { Err("list failed".to_string()) })
        }
        fn is_connected(&self) -> bool { true }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(FailingMCPCaller)));
    let result = tool.execute(&serde_json::json!({"action": "list_windows"})).await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("list_windows failed"));
}

#[tokio::test]
async fn test_desktop_tool_find_window_with_mcp_success() {
    struct FindMCPCaller;
    impl crate::browser::MCPToolCaller for FindMCPCaller {
        fn call_tool(
            &self,
            _tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            Box::pin(async { Ok("{\"hwnd\":\"0x123\",\"title\":\"Chrome\"}".to_string()) })
        }
        fn is_connected(&self) -> bool { true }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(FindMCPCaller)));
    let result = tool.execute(&serde_json::json!({"action": "find_window", "title": "Chrome"})).await;
    assert!(!result.is_error);
    assert!(result.for_llm.contains("Chrome"));
}

#[tokio::test]
async fn test_desktop_tool_find_window_with_mcp_error() {
    struct FailingMCPCaller;
    impl crate::browser::MCPToolCaller for FailingMCPCaller {
        fn call_tool(
            &self,
            _tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            Box::pin(async { Err("find failed".to_string()) })
        }
        fn is_connected(&self) -> bool { true }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(FailingMCPCaller)));
    let result = tool.execute(&serde_json::json!({"action": "find_window", "title": "Chrome"})).await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("find_window failed"));
}

#[tokio::test]
async fn test_desktop_tool_type_text_mcp_success() {
    struct TypeMCPCaller;
    impl crate::browser::MCPToolCaller for TypeMCPCaller {
        fn call_tool(
            &self,
            _tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            Box::pin(async { Ok("typed".to_string()) })
        }
        fn is_connected(&self) -> bool { true }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(TypeMCPCaller)));
    let result = tool.execute(&serde_json::json!({"action": "type_text", "text": "hello"})).await;
    assert!(!result.is_error);
    assert!(result.for_llm.contains("Typed"));
}

#[tokio::test]
async fn test_desktop_tool_click_at_mcp_success() {
    struct ClickMCPCaller;
    impl crate::browser::MCPToolCaller for ClickMCPCaller {
        fn call_tool(
            &self,
            _tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            Box::pin(async { Ok("clicked".to_string()) })
        }
        fn is_connected(&self) -> bool { true }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(ClickMCPCaller)));
    let result = tool.execute(&serde_json::json!({"action": "click_at", "x": 100, "y": 200})).await;
    assert!(!result.is_error);
    assert!(result.for_llm.contains("Clicked"));
}

#[tokio::test]
async fn test_desktop_tool_get_window_text_with_hwnd_mcp() {
    struct TextMCPCaller;
    impl crate::browser::MCPToolCaller for TextMCPCaller {
        fn call_tool(
            &self,
            tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            let name = tool_name.to_string();
            Box::pin(async move {
                if name == "get_window_text" {
                    Ok("Window Title Text".to_string())
                } else {
                    Ok("{}".to_string())
                }
            })
        }
        fn is_connected(&self) -> bool { true }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(TextMCPCaller)));
    let result = tool.execute(&serde_json::json!({"action": "get_window_text", "hwnd": "0x123"})).await;
    assert!(!result.is_error);
    assert!(result.for_llm.contains("Window Title Text"));
}

#[tokio::test]
async fn test_desktop_tool_get_window_text_with_title_mcp_find_error() {
    struct FailingMCPCaller;
    impl crate::browser::MCPToolCaller for FailingMCPCaller {
        fn call_tool(
            &self,
            _tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            Box::pin(async { Err("find failed".to_string()) })
        }
        fn is_connected(&self) -> bool { true }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(FailingMCPCaller)));
    let result = tool.execute(&serde_json::json!({"action": "get_window_text", "title": "Chrome"})).await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("find_window"));
}

#[tokio::test]
async fn test_desktop_tool_get_window_text_with_title_mcp_find_returns_empty_hwnd() {
    struct EmptyHwndMCPCaller;
    impl crate::browser::MCPToolCaller for EmptyHwndMCPCaller {
        fn call_tool(
            &self,
            tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            let name = tool_name.to_string();
            Box::pin(async move {
                if name == "find_window_by_title" {
                    Ok("{\"hwnd\":\"\"}".to_string())
                } else {
                    Ok("{}".to_string())
                }
            })
        }
        fn is_connected(&self) -> bool { true }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(EmptyHwndMCPCaller)));
    let result = tool.execute(&serde_json::json!({"action": "get_window_text", "title": "Chrome"})).await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("could not resolve"));
}

#[tokio::test]
async fn test_desktop_tool_get_window_text_mcp_error() {
    struct FailingMCPCaller;
    impl crate::browser::MCPToolCaller for FailingMCPCaller {
        fn call_tool(
            &self,
            _tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            Box::pin(async { Err("get_text failed".to_string()) })
        }
        fn is_connected(&self) -> bool { true }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(FailingMCPCaller)));
    let result = tool.execute(&serde_json::json!({"action": "get_window_text", "hwnd": "0x123"})).await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("get_window_text failed"));
}

#[test]
fn test_desktop_action_from_str_edge_cases() {
    assert!(DesktopAction::from_str("").is_err());
    assert!(DesktopAction::from_str("FIND_WINDOW").is_err());
    assert!(DesktopAction::from_str("find_window ").is_err());
}

#[test]
fn test_window_info_default_fields() {
    let info = WindowInfo {
        hwnd: String::new(),
        title: String::new(),
        class_name: String::new(),
        left: 0,
        top: 0,
        width: 0,
        height: 0,
    };
    assert!(info.hwnd.is_empty());
    assert_eq!(info.width, 0);
}

// ============================================================
// Additional branch coverage for get_window_text and screenshot
// ============================================================

#[tokio::test]
async fn test_get_window_text_prefers_hwnd_over_title() {
    // When both hwnd and title are supplied, the hwnd branch is used and
    // find_window_by_title should NOT be invoked.
    struct MockMCPCaller {
        called: Arc<std::sync::Mutex<Vec<String>>>,
    }
    impl crate::browser::MCPToolCaller for MockMCPCaller {
        fn call_tool(
            &self,
            tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            let name = tool_name.to_string();
            self.called.lock().unwrap().push(name.clone());
            Box::pin(async move {
                if name == "get_window_text" {
                    Ok("text via hwnd".to_string())
                } else {
                    // find_window_by_title must not be called when hwnd is present
                    Err("find should not be called".to_string())
                }
            })
        }
        fn is_connected(&self) -> bool {
            true
        }
    }
    let called = Arc::new(std::sync::Mutex::new(Vec::new()));
    let tool = DesktopTool::new(
        PathBuf::from("."),
        Some(Arc::new(MockMCPCaller { called: called.clone() })),
    );
    let result = tool
        .execute(&serde_json::json!({
            "action": "get_window_text",
            "hwnd": "HWND(0x123)",
            "title": "Something"
        }))
        .await;
    assert!(!result.is_error);
    assert!(result.for_llm.contains("text via hwnd"));
    let calls = called.lock().unwrap();
    assert!(
        !calls.iter().any(|c| c == "find_window_by_title"),
        "find_window_by_title must not be called when hwnd is supplied"
    );
}

#[tokio::test]
async fn test_get_window_text_find_returns_json_array_no_hwnd() {
    // find_window_by_title returns valid JSON that is an array (not an object),
    // so parsed["hwnd"].as_str() is None -> resolved_hwnd empty -> error.
    struct MockMCPCaller;
    impl crate::browser::MCPToolCaller for MockMCPCaller {
        fn call_tool(
            &self,
            tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            let name = tool_name.to_string();
            Box::pin(async move {
                if name == "find_window_by_title" {
                    Ok(r#"[{"title":"a"},{"title":"b"}]"#.to_string())
                } else {
                    Err("unknown".to_string())
                }
            })
        }
        fn is_connected(&self) -> bool {
            true
        }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(MockMCPCaller)));
    let result = tool
        .execute(&serde_json::json!({"action": "get_window_text", "title": "X"}))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("could not resolve"));
}

#[tokio::test]
async fn test_get_window_text_find_returns_invalid_json() {
    // find_window_by_title returns non-JSON text -> from_str fails -> empty hwnd.
    struct MockMCPCaller;
    impl crate::browser::MCPToolCaller for MockMCPCaller {
        fn call_tool(
            &self,
            tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            let name = tool_name.to_string();
            Box::pin(async move {
                if name == "find_window_by_title" {
                    Ok("not valid json at all".to_string())
                } else {
                    Err("unknown".to_string())
                }
            })
        }
        fn is_connected(&self) -> bool {
            true
        }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(MockMCPCaller)));
    let result = tool
        .execute(&serde_json::json!({"action": "get_window_text", "title": "X"}))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("could not resolve"));
}

#[tokio::test]
async fn test_screenshot_mcp_with_partial_region_fields() {
    // Only some of x/y/width/height present -> those that are present get
    // forwarded to the MCP args.
    struct ScreenshotMCPCaller;
    impl crate::browser::MCPToolCaller for ScreenshotMCPCaller {
        fn call_tool(
            &self,
            tool_name: &str,
            args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            let tool_name = tool_name.to_string();
            let args = args.clone();
            Box::pin(async move {
                assert_eq!(tool_name, "capture_screenshot_to_file");
                // x and width provided, y and height absent
                assert_eq!(args["x"], 5);
                assert_eq!(args["width"], 50);
                assert!(args.get("y").is_none());
                assert!(args.get("height").is_none());
                Ok("ok".to_string())
            })
        }
        fn is_connected(&self) -> bool {
            true
        }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(ScreenshotMCPCaller)));
    let result = tool
        .execute(&serde_json::json!({"action": "take_screenshot", "x": 5, "width": 50}))
        .await;
    assert!(!result.is_error);
}

#[tokio::test]
async fn test_click_at_default_button_is_left() {
    // No button specified -> defaults to "left".
    struct ClickMCPCaller;
    impl crate::browser::MCPToolCaller for ClickMCPCaller {
        fn call_tool(
            &self,
            tool_name: &str,
            args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            let tool_name = tool_name.to_string();
            let args = args.clone();
            Box::pin(async move {
                assert_eq!(tool_name, "click_window");
                assert_eq!(args["button"], "left");
                assert_eq!(args["x"], 10);
                assert_eq!(args["y"], 20);
                Ok("ok".to_string())
            })
        }
        fn is_connected(&self) -> bool {
            true
        }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(ClickMCPCaller)));
    let result = tool
        .execute(&serde_json::json!({"action": "click_at", "x": 10, "y": 20}))
        .await;
    assert!(!result.is_error);
    assert!(result.for_llm.contains("left"));
}

#[tokio::test]
async fn test_click_at_middle_button_forwarded() {
    struct ClickMCPCaller;
    impl crate::browser::MCPToolCaller for ClickMCPCaller {
        fn call_tool(
            &self,
            _tool_name: &str,
            args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            let args = args.clone();
            Box::pin(async move {
                assert_eq!(args["button"], "middle");
                Ok("ok".to_string())
            })
        }
        fn is_connected(&self) -> bool {
            true
        }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(ClickMCPCaller)));
    let result = tool
        .execute(&serde_json::json!({"action": "click_at", "x": 1, "y": 2, "button": "middle"}))
        .await;
    assert!(!result.is_error);
    assert!(result.for_llm.contains("middle"));
}

#[tokio::test]
async fn test_list_windows_no_title_does_not_set_title_contains() {
    // list_windows without title -> mcp_args has filter_visible but no title_contains.
    struct ListMCPCaller;
    impl crate::browser::MCPToolCaller for ListMCPCaller {
        fn call_tool(
            &self,
            _tool_name: &str,
            args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            let args = args.clone();
            Box::pin(async move {
                assert_eq!(args["filter_visible"], true);
                assert!(args.get("title_contains").is_none());
                Ok(r#"[{"hwnd":"1","title":"w"}]"#.to_string())
            })
        }
        fn is_connected(&self) -> bool {
            true
        }
    }
    let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(ListMCPCaller)));
    let result = tool.execute(&serde_json::json!({"action": "list_windows"})).await;
    assert!(!result.is_error);
}
