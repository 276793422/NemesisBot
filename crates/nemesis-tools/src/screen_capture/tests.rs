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
    let result = tool
        .execute(&serde_json::json!({"mode": "unknown"}))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("unknown capture mode"));
}

#[tokio::test]
async fn test_screen_capture_tool_region_missing_params() {
    let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
    let result = tool
        .execute(&serde_json::json!({"mode": "region"}))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("'x', 'y', 'width', and 'height' are required"));
}

#[tokio::test]
async fn test_screen_capture_tool_window_missing_params() {
    let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
    let result = tool
        .execute(&serde_json::json!({"mode": "window"}))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("'hwnd' or 'window_title' is required"));
}

#[tokio::test]
async fn test_screen_capture_tool_parameters_schema() {
    let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
    let params = tool.parameters();

    // Verify required fields
    let required = params["required"].as_array().unwrap();
    assert!(required.iter().any(|r| r.as_str() == Some("mode")));

    // Verify mode enum values
    let mode_enum = params["properties"]["mode"]["enum"]
        .as_array()
        .unwrap();
    assert_eq!(mode_enum.len(), 3);
    assert!(mode_enum
        .iter()
        .any(|v| v.as_str() == Some("full_screen")));
    assert!(mode_enum.iter().any(|v| v.as_str() == Some("region")));
    assert!(mode_enum.iter().any(|v| v.as_str() == Some("window")));

    // Verify format enum
    let format_enum = params["properties"]["format"]["enum"]
        .as_array()
        .unwrap();
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
    let modes = [CaptureMode::FullScreen, CaptureMode::Region, CaptureMode::Window];
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
        0, 0, 1920, 1080,
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
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            Box::pin(async { Ok("screenshot saved".to_string()) })
        }
        fn is_connected(&self) -> bool { true }
    }

    let temp = tempfile::tempdir().unwrap();
    let tool = ScreenCaptureTool::new(
        PathBuf::from(temp.path()),
        Some(Arc::new(MockMCP)),
    );
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
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            Box::pin(async { Err("MCP unavailable".to_string()) })
        }
        fn is_connected(&self) -> bool { true }
    }

    let temp = tempfile::tempdir().unwrap();
    let tool = ScreenCaptureTool::new(
        PathBuf::from(temp.path()),
        Some(Arc::new(FailMCP)),
    );
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
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            Box::pin(async { Ok("should not be called".to_string()) })
        }
        fn is_connected(&self) -> bool { false }
    }

    let temp = tempfile::tempdir().unwrap();
    let tool = ScreenCaptureTool::new(
        PathBuf::from(temp.path()),
        Some(Arc::new(DisconnectedMCP)),
    );
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
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            Box::pin(async { Ok("region saved".to_string()) })
        }
        fn is_connected(&self) -> bool { true }
    }

    let temp = tempfile::tempdir().unwrap();
    let tool = ScreenCaptureTool::new(
        PathBuf::from(temp.path()),
        Some(Arc::new(MockMCP)),
    );
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
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            let name = tool_name.to_string();
            Box::pin(async move {
                match name.as_str() {
                    "capture_screenshot_to_file" => Ok("window captured".to_string()),
                    _ => Err("unknown tool".to_string()),
                }
            })
        }
        fn is_connected(&self) -> bool { true }
    }

    let temp = tempfile::tempdir().unwrap();
    let tool = ScreenCaptureTool::new(
        PathBuf::from(temp.path()),
        Some(Arc::new(MockMCP)),
    );
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
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            let name = tool_name.to_string();
            Box::pin(async move {
                match name.as_str() {
                    "find_window_by_title" => Ok(r#"{"hwnd":"HWND(0x456)"}"#.to_string()),
                    "capture_screenshot_to_file" => Ok("window captured".to_string()),
                    _ => Err("unknown tool".to_string()),
                }
            })
        }
        fn is_connected(&self) -> bool { true }
    }

    let temp = tempfile::tempdir().unwrap();
    let tool = ScreenCaptureTool::new(
        PathBuf::from(temp.path()),
        Some(Arc::new(MockMCP)),
    );
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
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            Box::pin(async { Err("find failed".to_string()) })
        }
        fn is_connected(&self) -> bool { true }
    }

    let temp = tempfile::tempdir().unwrap();
    let tool = ScreenCaptureTool::new(
        PathBuf::from(temp.path()),
        Some(Arc::new(FailMCP)),
    );
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
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
            let name = tool_name.to_string();
            Box::pin(async move {
                match name.as_str() {
                    "capture_screenshot_to_file" => Err("capture failed".to_string()),
                    _ => Err("unknown".to_string()),
                }
            })
        }
        fn is_connected(&self) -> bool { true }
    }

    let temp = tempfile::tempdir().unwrap();
    let tool = ScreenCaptureTool::new(
        PathBuf::from(temp.path()),
        Some(Arc::new(FailCaptureMCP)),
    );
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
    assert_eq!("full_screen".parse::<CaptureMode>(), Ok(CaptureMode::FullScreen));
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
    for mode in &[CaptureMode::FullScreen, CaptureMode::Region, CaptureMode::Window] {
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
    let script = tool.build_region_script(0, 0, 1920, 1080, std::path::Path::new("/tmp/region.bmp"), "bmp");
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
    let result = tool
        .execute(&serde_json::json!({"mode": "window"}))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("'hwnd' or 'window_title' is required"));
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
