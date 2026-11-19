use super::*;
use crate::r#loop::Tool;
use nemesis_mcp::adapter::{ToolDefinition, ToolResult};

struct MockAdapterTool {
    definition: ToolDefinition,
    result: ToolResult,
}

#[async_trait::async_trait]
impl nemesis_mcp::adapter::Tool for MockAdapterTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }
    async fn execute(&self, _args: serde_json::Value) -> ToolResult {
        self.result.clone()
    }
}

fn make_context() -> crate::context::RequestContext {
    crate::context::RequestContext::new("test", "chat1", "session1", "corr1")
}

#[tokio::test]
async fn test_bridge_success() {
    let mock = MockAdapterTool {
        definition: ToolDefinition {
            name: "test_tool".to_string(),
            description: "A test tool".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            ..Default::default()
        },
        result: ToolResult::ok("hello world"),
    };
    let bridge = McpToolBridge::new(Box::new(mock));
    assert_eq!(bridge.description(), "A test tool");
    let result = bridge.execute("{}", &make_context()).await;
    assert_eq!(result.unwrap(), "hello world");
}

#[tokio::test]
async fn test_bridge_error() {
    let mock = MockAdapterTool {
        definition: ToolDefinition {
            name: "fail_tool".to_string(),
            description: "A failing tool".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            ..Default::default()
        },
        result: ToolResult::err("something went wrong"),
    };
    let bridge = McpToolBridge::new(Box::new(mock));
    let result = bridge.execute("{}", &make_context()).await;
    assert_eq!(result.unwrap_err(), "something went wrong");
}

#[tokio::test]
async fn test_bridge_invalid_json_args() {
    let mock = MockAdapterTool {
        definition: ToolDefinition {
            name: "t".to_string(),
            description: "desc".to_string(),
            parameters: serde_json::json!({}),
            ..Default::default()
        },
        result: ToolResult::ok("ok"),
    };
    let bridge = McpToolBridge::new(Box::new(mock));
    // Invalid JSON should be replaced with {} by the bridge
    let result = bridge.execute("not json{{{", &make_context()).await;
    assert_eq!(result.unwrap(), "ok");
}

#[tokio::test]
async fn test_bridge_parameters_forwarded() {
    let params = serde_json::json!({"type": "object", "properties": {"q": {"type": "string"}}});
    let mock = MockAdapterTool {
        definition: ToolDefinition {
            name: "search".to_string(),
            description: "Search".to_string(),
            parameters: params.clone(),
            ..Default::default()
        },
        result: ToolResult::ok("found"),
    };
    let bridge = McpToolBridge::new(Box::new(mock));
    assert_eq!(bridge.parameters(), params);
}

/// annotations 双 hint 经桥透传（definition.annotations → Tool trait）。
#[test]
fn test_bridge_hints_passthrough_from_annotations() {
    let ann = nemesis_mcp::types::McpToolAnnotations {
        read_only_hint: Some(true),
        destructive_hint: None,
    };
    let mock = MockAdapterTool {
        definition: ToolDefinition {
            name: "ro_tool".to_string(),
            description: "read only".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            annotations: Some(ann),
        },
        result: ToolResult::ok("ok"),
    };
    let bridge = McpToolBridge::new(Box::new(mock));
    assert_eq!(Tool::read_only_hint(&bridge), Some(true));
    assert_eq!(Tool::destructive_hint(&bridge), None);

    // 无 annotations = 双 None（内置工具同形，基线不受影响）。
    let mock = MockAdapterTool {
        definition: ToolDefinition {
            name: "bare_tool".to_string(),
            description: "no annotations".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            ..Default::default()
        },
        result: ToolResult::ok("ok"),
    };
    let bridge = McpToolBridge::new(Box::new(mock));
    assert_eq!(Tool::read_only_hint(&bridge), None);
    assert_eq!(Tool::destructive_hint(&bridge), None);
}

// ---------------------------------------------------------------------------
// Phase 5 F1：`[Image saved: <path>]` 标记 → tool_images 槽
// ---------------------------------------------------------------------------

const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

fn write_png(dir: &std::path::Path, name: &str) -> String {
    std::fs::create_dir_all(dir).unwrap();
    let p = dir.join(name);
    std::fs::write(&p, PNG_MAGIC).unwrap();
    p.to_string_lossy().to_string()
}

/// 标记解析：真实 PNG 路径提取成功；缺失文件 / 非图片字节 / 无 `]` 闭合 /
/// 无标记 四类防御臂全部跳过。
#[test]
fn test_extract_saved_image_paths_matrix() {
    let tmp = std::env::temp_dir().join(format!("mcpb_{}", std::process::id()));
    let good = write_png(&tmp, "good.png");
    // 缺失文件路径 + 非图片内容路径。
    let fake = tmp.join("fake.png");
    std::fs::write(&fake, b"not an image").unwrap();

    let fake_str = fake.to_string_lossy().to_string();
    let content = format!(
        "text before\n[Image saved: {good}]\n[Image saved: {}]\n[Image saved: {fake_str}]\n[Image saved: unterminated\nafter",
        tmp.join("missing.png").display()
    );
    let got = extract_saved_image_paths(&content);
    assert_eq!(got, vec![good]);

    // 无标记 = 空。
    assert!(extract_saved_image_paths("plain text").is_empty());
    let _ = std::fs::remove_dir_all(&tmp);
}

/// e2e：MockAdapterTool 回带标记文本 → bridge execute → tool_images 槽
/// 收到路径；错误路径不推槽。
#[tokio::test]
async fn test_bridge_pushes_marker_paths_into_tool_images() {
    let tmp = std::env::temp_dir().join(format!("mcpb2_{}", std::process::id()));
    let good = write_png(&tmp, "hit.png");
    let mock = MockAdapterTool {
        definition: ToolDefinition {
            name: "img_tool".to_string(),
            description: "returns image marker".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            ..Default::default()
        },
        result: ToolResult::ok(format!("done\n[Image saved: {good}]")),
    };
    let bridge = McpToolBridge::new(Box::new(mock));
    let ctx = make_context();
    let out = bridge.execute("{}", &ctx).await.unwrap();
    assert!(out.contains("[Image saved:"));
    assert_eq!(ctx.take_tool_images(), vec![good]);
    let _ = std::fs::remove_dir_all(&tmp);
}
