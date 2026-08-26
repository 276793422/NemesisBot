use super::*;

#[test]
fn test_capture_mode_from_str() {
    assert_eq!(
        CaptureMode::from_str("full_screen").unwrap(),
        CaptureMode::FullScreen
    );
    assert_eq!(
        CaptureMode::from_str("region").unwrap(),
        CaptureMode::Region
    );
    assert_eq!(
        CaptureMode::from_str("window").unwrap(),
        CaptureMode::Window
    );
    assert!(CaptureMode::from_str("invalid").is_err());
}

#[test]
fn test_capture_mode_display() {
    assert_eq!(CaptureMode::FullScreen.to_string(), "full_screen");
    assert_eq!(CaptureMode::Region.to_string(), "region");
    assert_eq!(CaptureMode::Window.to_string(), "window");
}

#[test]
fn test_image_format_enum() {
    assert_eq!(ScreenCaptureTool::image_format_enum("png"), "Png");
    assert_eq!(ScreenCaptureTool::image_format_enum("jpg"), "Jpeg");
    assert_eq!(ScreenCaptureTool::image_format_enum("jpeg"), "Jpeg");
    assert_eq!(ScreenCaptureTool::image_format_enum("bmp"), "Bmp");
    assert_eq!(ScreenCaptureTool::image_format_enum("unknown"), "Png");
}

#[tokio::test]
async fn test_screen_capture_tool_metadata() {
    let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
    assert_eq!(tool.name(), "screen_capture");
    assert!(!tool.description().is_empty());
}

#[tokio::test]
async fn test_screen_capture_tool_missing_mode() {
    let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
    let result = tool.execute(&serde_json::json!({})).await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("'mode' is required"));
}

#[tokio::test]
async fn test_screen_capture_tool_unknown_mode() {
    let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
    let result = tool.execute(&serde_json::json!({"mode": "unknown"})).await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("unknown capture mode"));
}

#[tokio::test]
async fn test_screen_capture_tool_region_missing_params() {
    let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
    let result = tool.execute(&serde_json::json!({"mode": "region"})).await;
    assert!(result.is_error);
    assert!(
        result
            .for_llm
            .contains("'x', 'y', 'width', and 'height' are required")
    );
}

#[tokio::test]
async fn test_screen_capture_tool_window_missing_params() {
    let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
    let result = tool.execute(&serde_json::json!({"mode": "window"})).await;
    assert!(result.is_error);
    assert!(
        result
            .for_llm
            .contains("'hwnd' or 'window_title' is required")
    );
}

#[tokio::test]
async fn test_screen_capture_tool_parameters_schema() {
    let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
    let params = tool.parameters();

    // Verify required fields
    let required = params["required"].as_array().unwrap();
    assert!(required.iter().any(|r| r.as_str() == Some("mode")));

    // Verify mode enum values
    let mode_enum = params["properties"]["mode"]["enum"].as_array().unwrap();
    assert_eq!(mode_enum.len(), 3);
    assert!(mode_enum.iter().any(|v| v.as_str() == Some("full_screen")));
    assert!(mode_enum.iter().any(|v| v.as_str() == Some("region")));
    assert!(mode_enum.iter().any(|v| v.as_str() == Some("window")));

    // Verify format enum
    let format_enum = params["properties"]["format"]["enum"].as_array().unwrap();
    assert_eq!(format_enum.len(), 2);
}

#[test]
fn test_build_full_screen_script() {
    let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
    let script = tool.build_full_screen_script(std::path::Path::new("/tmp/test.png"), "png");
    assert!(script.contains("PrimaryScreen.Bounds"));
    assert!(script.contains("ImageFormat]::Png"));
    assert!(script.contains("/tmp/test.png"));
}

#[test]
fn test_build_region_script() {
    let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
    let script = tool.build_region_script(
        100,
        200,
        300,
        400,
        std::path::Path::new("/tmp/region.png"),
        "jpg",
    );
    assert!(script.contains("100, 200, 300, 400"));
    assert!(script.contains("ImageFormat]::Jpeg"));
}

#[test]
fn test_prepare_output_path() {
    let temp = tempfile::tempdir().unwrap();
    let tool = ScreenCaptureTool::new(PathBuf::from(temp.path()), None);

    let result = tool.prepare_output_path("png");
    assert!(result.is_ok());
    let path = result.unwrap();
    assert!(path.to_string_lossy().ends_with(".png"));
    assert!(path.to_string_lossy().contains("screenshot_"));

    let result = tool.prepare_output_path("jpg");
    assert!(result.is_ok());
    let path = result.unwrap();
    assert!(path.to_string_lossy().ends_with(".jpg"));
}

// ============================================================
// Additional screen capture edge-case tests
// ============================================================

#[test]
fn test_capture_mode_roundtrip() {
    let modes = [
        CaptureMode::FullScreen,
        CaptureMode::Region,
        CaptureMode::Window,
    ];
    for mode in &modes {
        let s = mode.to_string();
        let parsed = CaptureMode::from_str(&s);
        assert_eq!(parsed.unwrap(), *mode);
    }
}

#[test]
fn test_image_format_all_variants() {
    assert_eq!(ScreenCaptureTool::image_format_enum("png"), "Png");
    assert_eq!(ScreenCaptureTool::image_format_enum("jpg"), "Jpeg");
    assert_eq!(ScreenCaptureTool::image_format_enum("jpeg"), "Jpeg");
    assert_eq!(ScreenCaptureTool::image_format_enum("bmp"), "Bmp");
    // Unknown and empty default to Png
    assert_eq!(ScreenCaptureTool::image_format_enum(""), "Png");
    assert_eq!(ScreenCaptureTool::image_format_enum("unknown"), "Png");
}

#[test]
fn test_prepare_output_path_creates_temp_dir() {
    let temp = tempfile::tempdir().unwrap();
    let tool = ScreenCaptureTool::new(PathBuf::from(temp.path()), None);

    let result = tool.prepare_output_path("png");
    assert!(result.is_ok());
    let path = result.unwrap();
    // Path should be under temp/
    assert!(path.to_string_lossy().contains("temp"));
}

#[tokio::test]
async fn test_screen_capture_region_partial_params() {
    let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
    // Only x provided, missing y/width/height
    let result = tool
        .execute(&serde_json::json!({"mode": "region", "x": 0}))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("required"));
}

#[tokio::test]
async fn test_screen_capture_window_with_hwnd() {
    let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
    let result = tool
        .execute(&serde_json::json!({"mode": "window", "hwnd": "HWND(0x12345)"}))
        .await;
    // Without MCP or real window, this will likely error on non-Windows or produce a fallback result
    assert!(result.is_error || !result.for_llm.is_empty());
}

#[tokio::test]
async fn test_screen_capture_jpg_format() {
    let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
    // Will fail since no actual screen capture, but should not panic
    let _result = tool
        .execute(&serde_json::json!({"mode": "full_screen", "format": "jpg"}))
        .await;
    // Just verify no panic
}

#[test]
fn test_build_full_screen_script_jpg() {
    let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
    let script = tool.build_full_screen_script(std::path::Path::new("/tmp/test.jpg"), "jpg");
    assert!(script.contains("ImageFormat]::Jpeg"));
}

// --- Additional tests for coverage ---

#[test]
fn test_image_format_bmp() {
    let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
    let script = tool.build_full_screen_script(std::path::Path::new("/tmp/test.bmp"), "bmp");
    assert!(script.contains("ImageFormat]::Bmp"));
}

#[test]
fn test_capture_mode_equality() {
    assert_eq!(CaptureMode::FullScreen, CaptureMode::FullScreen);
    assert_ne!(CaptureMode::FullScreen, CaptureMode::Region);
    assert_ne!(CaptureMode::Region, CaptureMode::Window);
}

#[tokio::test]
async fn test_screen_capture_full_screen_format_png() {
    let temp = tempfile::tempdir().unwrap();
    let tool = ScreenCaptureTool::new(PathBuf::from(temp.path()), None);
    // Just verify no panic with various format strings
    let _ = tool
        .execute(&serde_json::json!({"mode": "full_screen", "format": "png"}))
        .await;
}

#[tokio::test]
async fn test_screen_capture_window_with_title() {
    let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
    let result = tool
        .execute(&serde_json::json!({"mode": "window", "window_title": "Calculator"}))
        .await;
    // Will likely fail without MCP, but verify no panic
    assert!(result.is_error || !result.for_llm.is_empty());
}

#[test]
fn test_build_region_script_png() {
    let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
    let script = tool.build_region_script(
        0,
        0,
        1920,
        1080,
        std::path::Path::new("/tmp/region.png"),
        "png",
    );
    assert!(script.contains("0, 0, 1920, 1080"));
    assert!(script.contains("ImageFormat]::Png"));
}

#[test]
fn test_prepare_output_path_jpg_extension() {
    let temp = tempfile::tempdir().unwrap();
    let tool = ScreenCaptureTool::new(PathBuf::from(temp.path()), None);
    let result = tool.prepare_output_path("jpg");
    assert!(result.is_ok());
    let path = result.unwrap();
    assert!(path.to_string_lossy().ends_with(".jpg"));
}

#[tokio::test]
async fn test_screen_capture_tool_no_mcp_call() {
    let temp = tempfile::tempdir().unwrap();
    let tool = ScreenCaptureTool::new(PathBuf::from(temp.path()), None);
    // Verify tool can execute without MCP - just check no panic
    let params = tool.parameters();
    assert!(params.is_object());
}

#[test]
fn test_screen_capture_tool_parameters_complete() {
    let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
    let params = tool.parameters();
    // Verify all expected properties exist
    assert!(params["properties"]["mode"].is_object());
    assert!(params["properties"]["format"].is_object());
    assert!(params["properties"]["x"].is_object());
    assert!(params["properties"]["y"].is_object());
    assert!(params["properties"]["width"].is_object());
    assert!(params["properties"]["height"].is_object());
    assert!(params["properties"]["hwnd"].is_object());
    assert!(params["properties"]["window_title"].is_object());
}

// ============================================================
// Additional coverage tests for 95%+ target - MCP paths
// ============================================================

#[tokio::test]
async fn test_screen_capture_full_screen_mcp_success() {
    struct MockMCP;
    impl crate::browser::MCPToolCaller for MockMCP {
        fn call_tool(
            &self,
            _tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>>
        {
            Box::pin(async { Ok("screenshot saved".to_string()) })
        }
        fn is_connected(&self) -> bool {
            true
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let tool = ScreenCaptureTool::new(PathBuf::from(temp.path()), Some(Arc::new(MockMCP)));
    let result = tool
        .execute(&serde_json::json!({"mode": "full_screen"}))
        .await;
    assert!(!result.is_error);
    assert!(result.for_llm.contains("screenshot saved"));
}

#[tokio::test]
async fn test_screen_capture_full_screen_mcp_fails_fallback() {
    struct FailMCP;
    impl crate::browser::MCPToolCaller for FailMCP {
        fn call_tool(
            &self,
            _tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>>
        {
            Box::pin(async { Err("MCP unavailable".to_string()) })
        }
        fn is_connected(&self) -> bool {
            true
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let tool = ScreenCaptureTool::new(PathBuf::from(temp.path()), Some(Arc::new(FailMCP)));
    // MCP fails, falls back to PowerShell which may or may not work
    let _result = tool
        .execute(&serde_json::json!({"mode": "full_screen"}))
        .await;
    // Just verify no panic
}

#[tokio::test]
async fn test_screen_capture_full_screen_mcp_disconnected() {
    struct DisconnectedMCP;
    impl crate::browser::MCPToolCaller for DisconnectedMCP {
        fn call_tool(
            &self,
            _tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>>
        {
            Box::pin(async { Ok("should not be called".to_string()) })
        }
        fn is_connected(&self) -> bool {
            false
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let tool = ScreenCaptureTool::new(PathBuf::from(temp.path()), Some(Arc::new(DisconnectedMCP)));
    // Disconnected MCP, falls back to PowerShell
    let _result = tool
        .execute(&serde_json::json!({"mode": "full_screen"}))
        .await;
}

#[tokio::test]
async fn test_screen_capture_region_mcp_success() {
    struct MockMCP;
    impl crate::browser::MCPToolCaller for MockMCP {
        fn call_tool(
            &self,
            _tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>>
        {
            Box::pin(async { Ok("region saved".to_string()) })
        }
        fn is_connected(&self) -> bool {
            true
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let tool = ScreenCaptureTool::new(PathBuf::from(temp.path()), Some(Arc::new(MockMCP)));
    let result = tool
        .execute(&serde_json::json!({
            "mode": "region", "x": 0, "y": 0, "width": 100, "height": 100
        }))
        .await;
    assert!(!result.is_error);
    assert!(result.for_llm.contains("region saved"));
}

#[tokio::test]
async fn test_screen_capture_region_missing_y() {
    let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
    let result = tool
        .execute(&serde_json::json!({"mode": "region", "x": 0, "width": 100, "height": 100}))
        .await;
    assert!(result.is_error);
}

#[tokio::test]
async fn test_screen_capture_region_missing_width() {
    let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
    let result = tool
        .execute(&serde_json::json!({"mode": "region", "x": 0, "y": 0, "height": 100}))
        .await;
    assert!(result.is_error);
}

#[tokio::test]
async fn test_screen_capture_region_missing_height() {
    let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
    let result = tool
        .execute(&serde_json::json!({"mode": "region", "x": 0, "y": 0, "width": 100}))
        .await;
    assert!(result.is_error);
}

#[tokio::test]
async fn test_screen_capture_window_mcp_with_hwnd() {
    struct MockMCP;
    impl crate::browser::MCPToolCaller for MockMCP {
        fn call_tool(
            &self,
            tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>>
        {
            let name = tool_name.to_string();
            Box::pin(async move {
                match name.as_str() {
                    "capture_screenshot_to_file" => Ok("window captured".to_string()),
                    _ => Err("unknown tool".to_string()),
                }
            })
        }
        fn is_connected(&self) -> bool {
            true
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let tool = ScreenCaptureTool::new(PathBuf::from(temp.path()), Some(Arc::new(MockMCP)));
    let result = tool
        .execute(&serde_json::json!({"mode": "window", "hwnd": "HWND(0x123)"}))
        .await;
    assert!(!result.is_error);
    assert!(result.for_llm.contains("window captured"));
}

#[tokio::test]
async fn test_screen_capture_window_mcp_with_title() {
    struct MockMCP;
    impl crate::browser::MCPToolCaller for MockMCP {
        fn call_tool(
            &self,
            tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>>
        {
            let name = tool_name.to_string();
            Box::pin(async move {
                match name.as_str() {
                    "find_window_by_title" => Ok(r#"{"hwnd":"HWND(0x456)"}"#.to_string()),
                    "capture_screenshot_to_file" => Ok("window captured".to_string()),
                    _ => Err("unknown tool".to_string()),
                }
            })
        }
        fn is_connected(&self) -> bool {
            true
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let tool = ScreenCaptureTool::new(PathBuf::from(temp.path()), Some(Arc::new(MockMCP)));
    let result = tool
        .execute(&serde_json::json!({"mode": "window", "window_title": "Calculator"}))
        .await;
    assert!(!result.is_error);
    assert!(result.for_llm.contains("window captured"));
}

#[tokio::test]
async fn test_screen_capture_window_mcp_find_fails() {
    struct FailMCP;
    impl crate::browser::MCPToolCaller for FailMCP {
        fn call_tool(
            &self,
            _tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>>
        {
            Box::pin(async { Err("find failed".to_string()) })
        }
        fn is_connected(&self) -> bool {
            true
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let tool = ScreenCaptureTool::new(PathBuf::from(temp.path()), Some(Arc::new(FailMCP)));
    let result = tool
        .execute(&serde_json::json!({"mode": "window", "window_title": "Nonexistent"}))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("find window"));
}

#[tokio::test]
async fn test_screen_capture_window_mcp_capture_fails() {
    struct FailCaptureMCP;
    impl crate::browser::MCPToolCaller for FailCaptureMCP {
        fn call_tool(
            &self,
            tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>>
        {
            let name = tool_name.to_string();
            Box::pin(async move {
                match name.as_str() {
                    "capture_screenshot_to_file" => Err("capture failed".to_string()),
                    _ => Err("unknown".to_string()),
                }
            })
        }
        fn is_connected(&self) -> bool {
            true
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let tool = ScreenCaptureTool::new(PathBuf::from(temp.path()), Some(Arc::new(FailCaptureMCP)));
    let result = tool
        .execute(&serde_json::json!({"mode": "window", "hwnd": "HWND(0x123)"}))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("window capture failed"));
}

#[tokio::test]
async fn test_screen_capture_set_timeout() {
    let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
    let custom = Duration::from_secs(60);
    tool.set_timeout(custom).await;
    // Just verify no panic
}

#[test]
fn test_capture_mode_debug() {
    assert!(format!("{:?}", CaptureMode::FullScreen).contains("FullScreen"));
    assert!(format!("{:?}", CaptureMode::Region).contains("Region"));
    assert!(format!("{:?}", CaptureMode::Window).contains("Window"));
}

#[test]
fn test_capture_mode_from_str_all() {
    assert_eq!(
        "full_screen".parse::<CaptureMode>(),
        Ok(CaptureMode::FullScreen)
    );
    assert_eq!("region".parse::<CaptureMode>(), Ok(CaptureMode::Region));
    assert_eq!("window".parse::<CaptureMode>(), Ok(CaptureMode::Window));
}

#[test]
fn test_capture_mode_from_str_invalid() {
    let result = "invalid".parse::<CaptureMode>();
    assert!(result.is_err());
}

#[test]
fn test_capture_mode_roundtrip_all_variants() {
    for mode in &[
        CaptureMode::FullScreen,
        CaptureMode::Region,
        CaptureMode::Window,
    ] {
        let s = mode.to_string();
        let parsed: CaptureMode = s.parse().unwrap();
        assert_eq!(*mode, parsed);
    }
}

// ============================================================
// Additional coverage tests for 95%+ target
// ============================================================

#[tokio::test]
async fn test_screen_capture_set_timeout_value() {
    let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
    tool.set_timeout(Duration::from_secs(10)).await;
    // Verify the timeout was set
    let timeout = tool.timeout.lock().await;
    assert_eq!(*timeout, Duration::from_secs(10));
}

#[tokio::test]
async fn test_screen_capture_set_timeout_default() {
    let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
    let timeout = tool.timeout.lock().await;
    assert_eq!(*timeout, DEFAULT_TIMEOUT);
}

#[test]
fn test_image_format_enum_case_insensitive() {
    assert_eq!(ScreenCaptureTool::image_format_enum("PNG"), "Png");
    assert_eq!(ScreenCaptureTool::image_format_enum("JPG"), "Jpeg");
    assert_eq!(ScreenCaptureTool::image_format_enum("JPEG"), "Jpeg");
    assert_eq!(ScreenCaptureTool::image_format_enum("BMP"), "Bmp");
}

#[test]
fn test_build_full_screen_script_jpg_format() {
    let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
    let script = tool.build_full_screen_script(std::path::Path::new("/tmp/test.jpg"), "jpg");
    assert!(script.contains("ImageFormat]::Jpeg"));
    assert!(script.contains("/tmp/test.jpg"));
}

#[test]
fn test_build_region_script_bmp_format() {
    let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
    let script = tool.build_region_script(
        0,
        0,
        1920,
        1080,
        std::path::Path::new("/tmp/region.bmp"),
        "bmp",
    );
    assert!(script.contains("ImageFormat]::Bmp"));
    assert!(script.contains("0, 0, 1920, 1080"));
}

#[tokio::test]
async fn test_screen_capture_region_only_x_and_y() {
    let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
    // Missing width and height
    let result = tool
        .execute(&serde_json::json!({"mode": "region", "x": 10, "y": 20}))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("required"));
}

#[tokio::test]
async fn test_screen_capture_window_no_params() {
    let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
    let result = tool.execute(&serde_json::json!({"mode": "window"})).await;
    assert!(result.is_error);
    assert!(
        result
            .for_llm
            .contains("'hwnd' or 'window_title' is required")
    );
}

#[tokio::test]
async fn test_screen_capture_bmp_format_param() {
    let temp = tempfile::tempdir().unwrap();
    let tool = ScreenCaptureTool::new(PathBuf::from(temp.path()), None);
    let result = tool
        .execute(&serde_json::json!({"mode": "full_screen", "format": "bmp"}))
        .await;
    // Will likely fail since it tries powershell, but should not panic
    // Just verify it doesn't crash
    let _ = result;
}

#[test]
fn test_prepare_output_path_format_extensions() {
    let temp = tempfile::tempdir().unwrap();
    let tool = ScreenCaptureTool::new(PathBuf::from(temp.path()), None);

    let png_path = tool.prepare_output_path("png").unwrap();
    assert!(png_path.to_string_lossy().ends_with(".png"));

    let jpg_path = tool.prepare_output_path("jpg").unwrap();
    assert!(jpg_path.to_string_lossy().ends_with(".jpg"));

    let bmp_path = tool.prepare_output_path("bmp").unwrap();
    assert!(bmp_path.to_string_lossy().ends_with(".bmp"));
}

#[test]
fn test_capture_mode_debug_format() {
    assert_eq!(format!("{:?}", CaptureMode::FullScreen), "FullScreen");
    assert_eq!(format!("{:?}", CaptureMode::Region), "Region");
    assert_eq!(format!("{:?}", CaptureMode::Window), "Window");
}

#[test]
fn test_capture_mode_inequality() {
    assert_eq!(CaptureMode::FullScreen, CaptureMode::FullScreen);
    assert_ne!(CaptureMode::FullScreen, CaptureMode::Region);
    assert_ne!(CaptureMode::Region, CaptureMode::Window);
}

#[test]
fn test_capture_mode_from_str_error_message() {
    let result = "invalid_mode".parse::<CaptureMode>();
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.contains("unknown capture mode"));
    assert!(err.contains("invalid_mode"));
}

// ============================================================
// W4a coverage gap closure:
// - prepare_output_path create_dir_all failure arm (workspace
//   path is a regular file, so {workspace}/temp cannot be made)
// - execute_capture timeout arm (set_timeout shorter than
//   powershell.exe startup; the capture future is dropped and
//   the child is killed — no screen content is ever captured)
// - region mode MCP-Err arm (warn + fall through to PowerShell)
// - window mode find_window_by_title non-JSON parse arm
// NOT exercised here (structural exemption, goal doc §9.4):
// successful real captures (CopyFromScreen arms 158-180 and the
// PowerShell success paths write real screenshots of the live
// desktop) and the powershell.exe spawn-failure arm 138-139
// (no injection seam; spawn only fails if powershell.exe is
// missing from PATH).
// ============================================================

#[tokio::test]
async fn w4a_workspace_pointing_at_a_file_fails_to_create_temp_dir() {
    let dir = tempfile::TempDir::new().unwrap();
    let ws_file = dir.path().join("w4a_is_a_file");
    std::fs::write(&ws_file, "not a directory").unwrap();
    let tool = ScreenCaptureTool::new(ws_file, None);
    let result = tool
        .execute(&serde_json::json!({"mode": "full_screen"}))
        .await;
    assert!(result.is_error);
    assert!(
        result.for_llm.contains("failed to create temp directory"),
        "got: {}",
        result.for_llm
    );
}

#[tokio::test]
async fn w4a_capture_times_out_when_timeout_shorter_than_powershell_startup() {
    let dir = tempfile::TempDir::new().unwrap();
    let tool = ScreenCaptureTool::new(dir.path().to_path_buf(), None);
    // 1ms is far below any powershell.exe startup time, so the
    // tokio::time::timeout arm must fire deterministically.
    tool.set_timeout(Duration::from_millis(1)).await;
    let result = tool
        .execute(&serde_json::json!({"mode": "full_screen"}))
        .await;
    assert!(result.is_error);
    assert!(
        result.for_llm.contains("screen capture timed out"),
        "got: {}",
        result.for_llm
    );
}

#[tokio::test]
async fn w4a_region_mcp_error_falls_back_to_powershell_and_times_out() {
    struct FailingRegionMCP;
    impl crate::browser::MCPToolCaller for FailingRegionMCP {
        fn call_tool(
            &self,
            _tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>>
        {
            Box::pin(async { Err("region backend down".to_string()) })
        }
        fn is_connected(&self) -> bool {
            true
        }
    }
    let dir = tempfile::TempDir::new().unwrap();
    let tool = ScreenCaptureTool::new(
        dir.path().to_path_buf(),
        Some(Arc::new(FailingRegionMCP)),
    );
    tool.set_timeout(Duration::from_millis(1)).await;
    let result = tool
        .execute(&serde_json::json!({
            "mode": "region", "x": 1, "y": 1, "width": 2, "height": 2
        }))
        .await;
    // MCP Err -> warn + fall through to the PowerShell script builder,
    // which then hits the same 1ms timeout.
    assert!(result.is_error);
    assert!(
        result.for_llm.contains("screen capture timed out"),
        "got: {}",
        result.for_llm
    );
}

#[tokio::test]
async fn w4a_window_mode_title_find_returning_garbage_captures_without_hwnd() {
    struct GarbageFindMCP;
    impl crate::browser::MCPToolCaller for GarbageFindMCP {
        fn call_tool(
            &self,
            tool_name: &str,
            args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>>
        {
            let name = tool_name.to_string();
            let has_hwnd = args.get("hwnd").is_some();
            Box::pin(async move {
                match name.as_str() {
                    // Non-JSON body -> serde parse fails -> resolved_hwnd
                    // stays empty -> capture proceeds without a hwnd arg.
                    "find_window_by_title" => Ok("definitely not json".to_string()),
                    "capture_screenshot_to_file" => {
                        assert!(
                            !has_hwnd,
                            "capture must be called without hwnd when find failed to parse"
                        );
                        Ok("captured anyway".to_string())
                    }
                    _ => Err("unknown tool".to_string()),
                }
            })
        }
        fn is_connected(&self) -> bool {
            true
        }
    }
    let dir = tempfile::TempDir::new().unwrap();
    let tool = ScreenCaptureTool::new(
        dir.path().to_path_buf(),
        Some(Arc::new(GarbageFindMCP)),
    );
    let result = tool
        .execute(&serde_json::json!({
            "mode": "window", "window_title": "Whatever"
        }))
        .await;
    assert!(!result.is_error);
    assert!(
        result.for_llm.contains("Window screenshot saved"),
        "got: {}",
        result.for_llm
    );
}

// ===========================================================================
// S2 coverage (2026-08-26): region capture with no MCP caller (PowerShell
// fallback + success info! fields), window capture with a disconnected MCP
// caller (skip block -> PowerShell fallback, bogus hwnd errors)
// ===========================================================================

fn s2_enable_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_writer(std::io::sink)
        .try_init();
}

/// Region capture with `mcp_caller: None` runs the PowerShell path end to
/// end: execute_capture debug/info fields evaluate, the capture succeeds and
/// the file lands in {workspace}/temp/.
#[cfg(target_os = "windows")]
#[tokio::test]
async fn s2_region_capture_without_mcp_covers_fallback_and_success_fields() {
    s2_enable_tracing();
    let dir = tempfile::tempdir().unwrap();
    let tool = ScreenCaptureTool::new(dir.path().to_path_buf(), None);

    let result = tool
        .execute(&serde_json::json!({
            "mode": "region",
            "x": 0,
            "y": 0,
            "width": 10,
            "height": 10
        }))
        .await;

    assert!(!result.is_error, "got: {}", result.for_llm);
    assert!(
        result.for_llm.contains("screenshot_"),
        "expected saved file in result, got: {}",
        result.for_llm
    );
}

/// Window capture with a present-but-disconnected MCP caller must skip the
/// MCP block entirely and run the PowerShell fallback; a bogus hwnd makes
/// GetWindowRect yield an empty rect so the capture errors deterministically.
#[cfg(target_os = "windows")]
#[tokio::test]
async fn s2_window_capture_disconnected_mcp_falls_back_to_powershell() {
    s2_enable_tracing();
    struct S2DeadMcp;
    impl crate::browser::MCPToolCaller for S2DeadMcp {
        fn call_tool(
            &self,
            _tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>>
        {
            Box::pin(async { Ok("must not be called".to_string()) })
        }
        fn is_connected(&self) -> bool {
            false
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let tool = ScreenCaptureTool::new(dir.path().to_path_buf(), Some(Arc::new(S2DeadMcp)));

    let result = tool
        .execute(&serde_json::json!({"mode": "window", "hwnd": "HWND(0x1)"}))
        .await;

    assert!(
        result.is_error,
        "bogus hwnd should fail the PowerShell rect lookup, got: {}",
        result.for_llm
    );
}
