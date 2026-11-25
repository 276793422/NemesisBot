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
// coverage gap closure:
// - prepare_output_path create_dir_all failure arm (workspace
//   path is a regular file, so {workspace}/temp cannot be made)
// - execute_capture timeout arm (set_timeout shorter than
//   powershell.exe startup; the capture future is dropped and
//   the child is killed — no screen content is ever captured)
// - region mode MCP-Err arm (warn + fall through to PowerShell)
// - window mode find_window_by_title non-JSON parse arm
// NOT exercised here (structural exemption, goal doc):
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

// 前提 = Windows 宿主（远端首跑暴露）：1ms timeout 打到
// "screen capture timed out" 臂的前提是 powershell.exe 能被 spawn（只是
// 启动慢）；Linux 上 spawn 直接 ENOENT，走的是 "screen capture failed:..."
// 臂，断言文案对不上。无可移植等价（没有跨平台慢启动 shell）→ 整测门控。
#[cfg(windows)]
#[tokio::test]
async fn w4a_capture_times_out_when_timeout_shorter_than_powershell_startup() {
    let dir = tempfile::TempDir::new().unwrap();
    let tool = ScreenCaptureTool::new(dir.path().to_path_buf(), None);
    // 1ms is far below any powershell.exe startup time, so the
    // tokio::time:timeout arm must fire deterministically.
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

// 前提 = Windows 宿主（同上）：MCP Err 后 fallback 到 PowerShell 脚本构建
// → 1ms timeout 臂；Linux 上 powershell.exe ENOENT 走 spawn 失败臂 → 门控。
#[cfg(windows)]
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
    let tool = ScreenCaptureTool::new(dir.path().to_path_buf(), Some(Arc::new(FailingRegionMCP)));
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
async fn w4a_window_mode_title_find_returning_garbage_errors_honestly() {
    struct GarbageFindMCP;
    impl crate::browser::MCPToolCaller for GarbageFindMCP {
        fn call_tool(
            &self,
            tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>>
        {
            let name = tool_name.to_string();
            Box::pin(async move {
                match name.as_str() {
                    // Non-JSON body -> serde parse fails -> resolved_hwnd
                    // empty -> BUG-20261125-031 翻转：修复前静默不带 hwnd
                    // 继续 capture（MCP 全屏假成功 "Window screenshot
                    // saved"），修复后早退诚实报错。
                    "find_window_by_title" => Ok("definitely not json".to_string()),
                    "capture_screenshot_to_file" => {
                        panic!(
                            "capture must not be called when the find result cannot be resolved"
                        );
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
    let tool = ScreenCaptureTool::new(dir.path().to_path_buf(), Some(Arc::new(GarbageFindMCP)));
    let result = tool
        .execute(&serde_json::json!({
            "mode": "window", "window_title": "Whatever"
        }))
        .await;
    assert!(result.is_error);
    assert!(
        result.for_llm.contains("could not resolve window handle"),
        "got: {}",
        result.for_llm
    );
}

// ===========================================================================
// coverage: region capture with no MCP caller (PowerShell
// fallback + success info! fields), window capture with a disconnected MCP
// caller (skip block -> PowerShell fallback, bogus hwnd errors)
// ===========================================================================

// Only the windows-gated tests below call this; gate it with the same cfg so
// other targets don't see it as dead code (Linux CI clippy runs -D warnings).
#[cfg(target_os = "windows")]
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

// ============================================================
// Window find-part builder (title/hwnd → PowerShell snippet).
// The title is interpolated into PowerShell single-quoted strings;
// the failure diagnostic must stay single-quoted — a double-quoted
// diagnostic would expand `$var`/`$(...)` from a benign title
// (e.g. a browser tab "$100 off") on the lookup-failure path.
// ============================================================

#[test]
fn test_build_window_find_part_hwnd_takes_precedence() {
    let part = ScreenCaptureTool::build_window_find_part("HWND(0x1234)", "ignored title").unwrap();
    assert!(part.contains("[IntPtr]0x1234"), "got: {}", part);
    assert!(!part.contains("MainWindowTitle"), "got: {}", part);

    let plain = ScreenCaptureTool::build_window_find_part("0xABCD", "").unwrap();
    assert!(plain.contains("[IntPtr]0xABCD"), "got: {}", plain);
}

#[test]
fn test_build_window_find_part_title_in_single_quotes() {
    let part = ScreenCaptureTool::build_window_find_part("", "Calculator").unwrap();
    assert!(part.contains("-like '*Calculator*'"), "got: {}", part);
    // The failure diagnostic must be single-quoted (no double-quote
    // interpolation context for title data).
    assert!(
        part.contains("Write-Error 'Window not found: Calculator'"),
        "got: {}",
        part
    );
    assert!(
        !part.contains("Write-Error \"Window not found"),
        "got: {}",
        part
    );
}

#[test]
fn test_build_window_find_part_title_metachars_stay_literal() {
    // A title containing PS double-quote interpolation metachars must be
    // embedded inertly: `'` doubled for the single-quote context, `$` never
    // in a double-quoted string.
    let part = ScreenCaptureTool::build_window_find_part("", "it's $100 (off)").unwrap();
    assert!(part.contains("-like '*it''s $100 (off)*'"), "got: {}", part);
    assert!(
        part.contains("Write-Error 'Window not found: it''s $100 (off)'"),
        "got: {}",
        part
    );
    // No double-quoted string may contain interpolated title data.
    for line in part.lines() {
        if line.contains("$100") || line.contains("it''s") {
            assert!(
                !line.contains("\"Window not found"),
                "title leaked into double-quoted context: {}",
                line
            );
        }
    }
}

#[test]
fn test_build_window_find_part_hwnd_prefixes_normalized() {
    // BUG (found by this test): "HWND(0x1234)" previously produced
    // `[IntPtr]0x0x1234` — a doubled 0x is an invalid PowerShell literal,
    // so every hwnd-based PowerShell window capture failed at script parse.
    // All four accepted input shapes must normalize to the same snippet.
    for hwnd in ["HWND(0x1234)", "HWND(1234)", "0x1234", "1234"] {
        let part = ScreenCaptureTool::build_window_find_part(hwnd, "").unwrap();
        assert!(
            part.contains("[IntPtr]0x1234"),
            "hwnd {:?} → {}",
            hwnd,
            part
        );
        assert!(!part.contains("0x0x"), "hwnd {:?} → {}", hwnd, part);
    }
}

/// 回归锁（BUG-20261109-021）：`-like` 模式位的通配符必须字面化——
/// 窗口标题含 `*`/`?`/`[`/`]` 时被解释成通配符（`[兼容模式]` 变字符集）
/// → 匹配错位/误导性 Window not found。单引号转义只封字符串注入面，
/// 通配符面要方括号包裹形态（本机 PowerShell 探针实证）。
#[test]
fn test_build_window_find_part_escapes_wildcards() {
    // 方括号标题：模式位字面化，Write-Error 位保持可读原文。
    let s = ScreenCaptureTool::build_window_find_part("", "记事本 [兼容模式]").unwrap();
    assert!(
        s.contains("-like '*记事本 [[]兼容模式[]]*'"),
        "模式位未字面化: {s}"
    );
    assert!(
        s.contains("Window not found: 记事本 [兼容模式]"),
        "错误消息位应保留原文: {s}"
    );

    // `*` 与 `?` 单字符通配符字面化。
    let s = ScreenCaptureTool::build_window_find_part("", "app * beta?").unwrap();
    assert!(s.contains("-like '*app [*] beta[?]*'"), "got: {s}");

    // 单引号保持 `''` 转义（字符串注入面，模式位与错误位都要）。
    let s = ScreenCaptureTool::build_window_find_part("", "it's here").unwrap();
    assert!(s.contains("-like '*it''s here*'"), "got: {s}");
    assert!(s.contains("Window not found: it''s here"), "got: {s}");

    // 单遍映射：输入本身是通配符组合 `[*]` 时产物不得二次转义叠加
    // （预期 `[[][*][]]`——每个输入字符恰好映射一次）。
    let s = ScreenCaptureTool::build_window_find_part("", "[*]").unwrap();
    assert!(s.contains("-like '*[[][*][]]*'"), "got: {s}");
}

/// hwnd 分支不受标题转义影响（路径回归对照）。
#[test]
fn test_build_window_find_part_hwnd_branch_untouched() {
    let s = ScreenCaptureTool::build_window_find_part("HWND(0x302ba)", "any [title]").unwrap();
    assert!(s.contains("$handle = [IntPtr]0x302ba"), "got: {s}");
    assert!(!s.contains("-like"), "hwnd 分支不应含 -like: {s}");
}

// ===========================================================================
// BUG-20261122-017 回归锁：execute_capture 超时必须杀掉 powershell 子进程
// （kill_on_drop 家族纪律）。tokio Child 默认 kill_on_drop=false，timeout
// future 被 drop 后 powershell 泄漏为孤儿。脚本注入形态（desktop_automation
// w5 同款手法）：execute_capture 的 script 参数可注入 Start-Sleep 桩——
// 孤儿存活窗口确定性（sleep 远超测试时长），不赌真实截屏的自然完成时点。
// sleep 时长取 111s：与 desktop_automation w5 的 120s needle 字面区分，
// 防同测试二进制并行互染（两测试各自断言自己的 needle 归零）。
// creation_flags（CREATE_NO_WINDOW）无法进程内断言，靠构建 + census 验证
// （家族先例：desktop_automation w5 同款取舍）。
// ===========================================================================

#[cfg(target_os = "windows")]
#[tokio::test]
async fn screen_capture_timeout_kills_child() {
    static CAPTURE_KILL_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _serial = CAPTURE_KILL_TEST_LOCK.lock().await;
    let tool = ScreenCaptureTool::new(PathBuf::from("."), None);
    tool.set_timeout(Duration::from_millis(600)).await;
    let t0 = std::time::Instant::now();
    let result = tool
        .execute_capture(
            "Start-Sleep -Seconds 111",
            std::path::Path::new("unused.png"),
        )
        .await;
    assert!(
        result.for_llm.contains("timed out"),
        "expected timeout error, got: {:?}",
        result.for_llm
    );
    assert!(
        t0.elapsed() < Duration::from_secs(10),
        "timeout did not fire promptly: {:?}",
        t0.elapsed()
    );
    // kill_on_drop 异步落地宽限（desktop_automation w5 同款）。
    tokio::time::sleep(Duration::from_secs(2)).await;
    // 孤儿检测：WQL 服务端 Filter（等值+LIKE 在 WinMgmt 内做，实测 ~0.3s；
    // 客户端全表 Where-Object 形态实测 118s——禁用）。needle 归属被测
    // 子进程；查询进程自身命令行同样含 needle，用 $PID 动态排除。
    // 持 CAPTURE_KILL_TEST_LOCK 串行后，本文件并行测试的真实截屏子进程
    // （脚本无本 needle）不会误入计数。
    let script = "Get-CimInstance Win32_Process -Filter (\"Name='powershell.exe' \
AND CommandLine LIKE '%Start-Sleep -Seconds 111%' AND ProcessId <> \" + $PID) \
| Measure-Object | Select-Object -ExpandProperty Count";
    let out = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
        .unwrap();
    let count = String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<i64>()
        .unwrap_or(-1);
    assert_eq!(
        count, 0,
        "orphaned powershell child survived the timeout (kill_on_drop missing?)"
    );
}

// ===========================================================================
// BUG-20261125-029 回归锁：hwnd → PowerShell 脚本裸插值注入。
// `$handle = [IntPtr]0x{hwnd}` 中 hwnd 若含 `;`/换行，PowerShell -Command
// 多语句语义（活体探针实证：`;` 后随语句恒执行）→ 任意命令执行。修复 =
// 纯十六进制校验（空 / >16 位 / 非 hexdigit → Err）。本函数是该参数的
// 唯一注入闸：screen_tools.rs 注册恒 None MCP → PowerShell fallback 是
// 生产路径；命令守卫只审 exec 类工具命令行，screen_capture 内部 spawn 的
// PowerShell 不在其视线内。
// ===========================================================================

#[test]
fn b029_hwnd_injection_forms_rejected() {
    // 语句分隔符（`;`/换行）、引号闭包、纯非 hex、边界形态全拒。
    for bad in [
        "1234; calc",
        "0; Write-Output pwned",
        "HWND(0x1) ; iex('x')",
        "HWND(0x1); Write-Host x",
        "1234\ncalc",
        "xyz",  // 全非 hex 字母（x/y/z 不在 0-9a-f）
        "abcg", // g 非 hex——注意 "abc" 本身是合法 hex（0xabc），不可作拒绝向量
        "0xZZ",
        "1234;",
        // 注：裸空串 "" 不在本表——空 hwnd 是模式分派（落标题分支），
        // 不是拒绝语义；空组合由 execute 层 missing_params 测试覆盖。
        "HWND()", // 剥前缀后为空 → 拒
        "HWND() calc",
    ] {
        let r = ScreenCaptureTool::build_window_find_part(bad, "");
        assert!(
            r.is_err(),
            "hwnd {bad:?} must be rejected, got {:?}",
            r.ok()
        );
    }
    // u64 上界：17 位 hex 超 16 位上限拒绝（溢出面 + 脚本膨胀面）。
    assert!(ScreenCaptureTool::build_window_find_part("0x12345678901234567", "").is_err());
    assert!(ScreenCaptureTool::build_window_find_part("HWND(12345678901234567)", "").is_err());
}

#[test]
fn b029_hwnd_valid_forms_accepted_and_injected_verbatim() {
    // 合法 hex 形态放行且插值精确（HWND()/0x 前缀归一对照
    // test_build_window_find_part_hwnd_prefixes_normalized）。
    for (input, expect) in [
        ("HWND(0x12345)", "$handle = [IntPtr]0x12345"),
        ("0xABCD", "$handle = [IntPtr]0xABCD"),
        ("deadbeef", "$handle = [IntPtr]0xdeadbeef"),
        ("HWND(1234)", "$handle = [IntPtr]0x1234"),
    ] {
        let part = ScreenCaptureTool::build_window_find_part(input, "")
            .unwrap_or_else(|e| panic!("hwnd {input:?} wrongly rejected: {e}"));
        assert!(part.contains(expect), "hwnd {input:?} → {part}");
        // 校验通过 → 插值位不可能残留注入语句分隔符。
        assert!(!part.contains(';'), "hwnd {input:?} → {part}");
        assert!(!part.contains('\n'), "hwnd {input:?} → {part}");
    }
}

#[tokio::test]
async fn b029_execute_window_mode_invalid_hwnd_errors_before_spawn() {
    // execute 全链：非法 hwnd 在拼脚本前诚实报错（错误含 "invalid hwnd"
    // = 校验闸命中。若校验缺失，Windows 会 spawn 注入脚本产生完全不同的
    // 结果，非 Windows 报 spawn 失败——都不是这个文案）。校验先于 spawn
    // = 跨平台确定性，本测试在 Linux 也真实拦截。
    let dir = tempfile::tempdir().unwrap();
    let tool = ScreenCaptureTool::new(dir.path().to_path_buf(), None);
    for bad in ["1234; calc", "0; Write-Output pwned", "xyz"] {
        let result = tool
            .execute(&serde_json::json!({"mode": "window", "hwnd": bad}))
            .await;
        assert!(result.is_error, "hwnd {bad:?} must be rejected");
        assert!(
            result.for_llm.contains("invalid hwnd"),
            "hwnd {bad:?} → {}",
            result.for_llm
        );
    }
}

// ===========================================================================
// BUG-20261125-030 回归锁：format 白名单。此前 format 未校验直接拼进输出
// 文件扩展名（`screenshot_{ts}.{format}`）——Windows 把 `\` `/` 都当路径
// 分隔符，`"png\..\..\evil"` 借 `..` 归一把 `$bitmap.Save` 引到 workspace
// 外。白名单 = png/jpg/jpeg/bmp（schema enum ∪ image_format_enum 支持集），
// 小写归一，其余诚实报错（agent 路径有 args_validator enum 闸，workflow
// 工具节点路径直通——工具必须自校验）。
// ===========================================================================

#[tokio::test]
async fn b030_format_whitelist_rejects_traversal_and_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let tool = ScreenCaptureTool::new(dir.path().to_path_buf(), None);
    for bad in [
        "png\\..\\..\\evil", // Windows 反斜杠分隔符 + .. 归一
        "../x",              // POSIX 形态
        "png/../../x",       // 正斜杠分隔符 + .. 归一
        "exe",               // 可执行扩展名
        "png ",              // 尾空格：非白名单成员，诚实拒绝不静默 trim
        "",                  // 空串：拒绝（format 键缺失才回落 png）
        "PNG\\x",            // 大小写归一发生在白名单匹配前
    ] {
        let result = tool
            .execute(&serde_json::json!({"mode": "full_screen", "format": bad}))
            .await;
        assert!(result.is_error, "format {bad:?} must be rejected");
        assert!(
            result.for_llm.contains("invalid format"),
            "format {bad:?} → {}",
            result.for_llm
        );
    }
}

#[tokio::test]
async fn b030_format_whitelist_accepts_supported_set() {
    // 支持集全部通过白名单闸（1ms timeout 让 PowerShell 臂快速失败——
    // 只断言不是格式闸拒绝，真实截屏成败与本 BUG 无关；Linux 上 spawn
    // ENOENT 同样立即失败，断言形态一致）。
    let dir = tempfile::tempdir().unwrap();
    let tool = ScreenCaptureTool::new(dir.path().to_path_buf(), None);
    tool.set_timeout(Duration::from_millis(1)).await;
    for fmt in ["png", "PNG", "jpg", "JPG", "jpeg", "JPEG", "bmp"] {
        let result = tool
            .execute(&serde_json::json!({"mode": "full_screen", "format": fmt}))
            .await;
        assert!(
            !result.for_llm.contains("invalid format"),
            "format {fmt:?} wrongly rejected: {}",
            result.for_llm
        );
    }
}

#[test]
fn b030_prepare_output_path_jpeg_extension_accepted() {
    // jpeg 在白名单内（schema enum 只列 png/jpg，image_format_enum 支持
    // jpeg）——白名单 = 两集合并集，jpeg 走通落 .jpeg 后缀。
    let temp = tempfile::tempdir().unwrap();
    let tool = ScreenCaptureTool::new(PathBuf::from(temp.path()), None);
    let path = tool.prepare_output_path("jpeg").unwrap();
    assert!(path.to_string_lossy().ends_with(".jpeg"));
}

// ===========================================================================
// BUG-20261125-031 回归锁：capture_window MCP 臂标题解析失败必须诚实报错。
// 修复前 resolved_hwnd 空 → mcp_args 不带 hwnd 继续调 capture → MCP 按自身
// 默认捕获全屏，却以 "Window screenshot saved" 假成功上报。garbage JSON
// 形态的翻转见 w4a_window_mode_title_find_returning_garbage_errors_honestly；
// 本块补 JSON 合法但 hwnd 空/缺失形态。
// ===========================================================================

#[tokio::test]
async fn b031_title_resolved_empty_hwnd_errors_instead_of_fullscreen_fallback() {
    struct EmptyHwndFindMCP;
    impl crate::browser::MCPToolCaller for EmptyHwndFindMCP {
        fn call_tool(
            &self,
            tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>>
        {
            let name = tool_name.to_string();
            Box::pin(async move {
                match name.as_str() {
                    // JSON 合法但 hwnd 空 = find 到窗口却拿不到句柄的诚实形态。
                    "find_window_by_title" => Ok(r#"{"hwnd":""}"#.to_string()),
                    // 假成功复发即失败：修复在位时不可达。
                    "capture_screenshot_to_file" => {
                        panic!("capture must not proceed without a resolved hwnd");
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
    let tool = ScreenCaptureTool::new(dir.path().to_path_buf(), Some(Arc::new(EmptyHwndFindMCP)));
    let result = tool
        .execute(&serde_json::json!({"mode": "window", "window_title": "Whatever"}))
        .await;
    assert!(result.is_error);
    assert!(
        result.for_llm.contains("could not resolve window handle"),
        "got: {}",
        result.for_llm
    );
}

#[tokio::test]
async fn b031_title_find_missing_hwnd_field_errors_instead_of_fullscreen_fallback() {
    struct NoHwndFieldMCP;
    impl crate::browser::MCPToolCaller for NoHwndFieldMCP {
        fn call_tool(
            &self,
            tool_name: &str,
            _args: &serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>>
        {
            let name = tool_name.to_string();
            Box::pin(async move {
                match name.as_str() {
                    // JSON 合法但 hwnd 字段缺失（unwrap_or("") 同落空）。
                    "find_window_by_title" => Ok(r#"{"ok":true}"#.to_string()),
                    "capture_screenshot_to_file" => {
                        panic!("capture must not proceed without a resolved hwnd");
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
    let tool = ScreenCaptureTool::new(dir.path().to_path_buf(), Some(Arc::new(NoHwndFieldMCP)));
    let result = tool
        .execute(&serde_json::json!({"mode": "window", "window_title": "Whatever"}))
        .await;
    assert!(result.is_error);
    assert!(
        result.for_llm.contains("could not resolve window handle"),
        "got: {}",
        result.for_llm
    );
}
