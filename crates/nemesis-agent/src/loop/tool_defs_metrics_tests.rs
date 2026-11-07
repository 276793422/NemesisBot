// 量测：tool defs 字节量测埋点测试。
//
// 覆盖面：
// ① 单一真相源一致性——`tool_defs_metrics` 的 tools 计数与 bytes 都从
//    `build_tool_defs()` 派生（空注册面 = 空数组字节 2；注册工具后计数
//    增长且 bytes 严格等于 defs 的 serde_json 序列化字节数）；
// ② 闸联动——`agents.hidden_tools` 隐藏工具后 metrics 同步收缩（量测走
//    全闸链，不是独立旁路——隐藏后 bytes 严格缩小）；
// ③ 口径钉子——bytes 恒等于 openai ToolDefinition 形态 JSON 字节
//    （建设惰性 schema 前后对比基线的口径稳定性回归锁）。
//
// 自带迷你 Mock（兄弟模块私有类型不共享，同 b4_b2_gate_tests 形状）。

use super::*;

/// 恒定回复的迷你 provider。
struct MetricsMockProvider;

#[async_trait]
impl LlmProvider for MetricsMockProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Ok(LlmResponse {
            finish_reason: None,
            escalated_to: None,
            content: "ok".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

/// 迷你测试工具。
struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok("echo_ok".to_string())
    }
}

fn metrics_loop() -> AgentLoop {
    AgentLoop::new(
        Box::new(MetricsMockProvider),
        AgentConfig {
            model: "test-model".to_string(),
            system_prompt: None,
            max_turns: 5,
            tools: vec![],
            models: std::collections::HashMap::new(),
            tool_fence_profession: None,
        },
    )
}

/// ① 空注册面 = 0 工具、字节为空 JSON 数组 2 字节；注册后计数与字节同步
/// 增长，且 bytes 严格等于 build_tool_defs 序列化字节。
#[test]
fn tool_defs_metrics_tracks_registry_and_serialization() {
    let mut agent_loop = metrics_loop();
    let m = agent_loop.tool_defs_metrics();
    assert_eq!(m["tools"], 0, "空注册面必须 0 工具");
    assert_eq!(m["bytes"], 2, "空 defs 序列化为 \"[]\" 恰 2 字节");

    agent_loop.register_tool("echo_a".to_string(), Box::new(EchoTool));
    agent_loop.register_tool("echo_b".to_string(), Box::new(EchoTool));
    let m = agent_loop.tool_defs_metrics();
    assert_eq!(m["tools"], 2, "注册两工具后计数必须为 2");
    let expected = serde_json::to_vec(&agent_loop.build_tool_defs())
        .unwrap()
        .len();
    assert_eq!(
        m["bytes"], expected,
        "bytes 必须严格等于 defs 的 JSON 序列化字节（单一真相源）"
    );
    assert!(expected > 2, "非空 defs 字节必须大于空数组");
}

/// ② 闸联动：hidden_tools 隐藏后 metrics 同步收缩（量测走全闸链），
/// 收缩后 bytes 仍严格等于当期 defs 序列化字节。
#[test]
fn tool_defs_metrics_reflects_hidden_tools_gate() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    std::fs::write(&path, r#"{"agents": {"hidden_tools": ["echo_b"]}}"#).unwrap();

    let mut agent_loop = metrics_loop();
    agent_loop.register_tool("echo_a".to_string(), Box::new(EchoTool));
    agent_loop.register_tool("echo_b".to_string(), Box::new(EchoTool));

    // 无 config_path：全量供给。
    let full = agent_loop.tool_defs_metrics();
    assert_eq!(full["tools"], 2, "无隐藏配置时两工具全量");

    // 挂上 config 后 echo_b 被隐藏，量测同步收缩。
    agent_loop.set_config_path(path);
    let m = agent_loop.tool_defs_metrics();
    assert_eq!(m["tools"], 1, "hidden_tools 必须联动量测（全闸链）");

    let defs = agent_loop.build_tool_defs();
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].function.name, "echo_a");
    assert_eq!(
        m["bytes"],
        serde_json::to_vec(&defs).unwrap().len(),
        "收缩后 bytes 仍严格等于 defs 序列化字节"
    );
    assert!(
        full["bytes"].as_u64().unwrap() > m["bytes"].as_u64().unwrap(),
        "隐藏后字节必须严格缩小"
    );
}

/// ④ -建设：惰性 schema 供给联动量测——同注册面同档位（normal，带
/// config 前后翻 `agents.lazy_tool_schema`），lazy on 字节严格小于 lazy
/// off（参数折叠生效）；lazy off 时量测与既有基线口径零变化（回归锁）。
/// 量测走全闸链（tier 白名单 + 折叠都在 build_tool_defs 内），数字随
/// 注册面真实 schema 增长——本测试用两个带 required 的具名 schema 桩。
struct SchemafulTool;

#[async_trait]
impl Tool for SchemafulTool {
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok("ok".to_string())
    }
    fn description(&self) -> String {
        "A tool with a realistic parameter schema for byte measurement.".to_string()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Absolute path to the target file."},
                "content": {"type": "string", "description": "Full content to write."},
                "mode": {"type": "string", "enum": ["overwrite", "append"], "description": "Write mode."}
            },
            "required": ["path", "content"]
        })
    }
}

#[test]
fn tool_defs_metrics_reflects_lazy_schema_supply() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    std::fs::write(&path, r#"{"agents": {"lazy_tool_schema": true}}"#).unwrap();

    let mut agent_loop = metrics_loop();
    *agent_loop.tier.write() = nemesis_types::capability::ModelTier::Normal;
    // normal 白名单内三名（read_file/multiedit/grep）——供给面过滤后的真实
    // 口径。三名全量 schema 体积足以让折叠节省显著大于取回工具自身 def
    // 体积（2 名时两者恰好抵消，字节等式巧合，故取 3 名拉开余量）。
    // 取回工具生产装配恒注册；裸 loop 测试自插（l10/w7 同款）。
    agent_loop.register_tool("read_file".to_string(), Box::new(SchemafulTool));
    agent_loop.register_tool("multiedit".to_string(), Box::new(SchemafulTool));
    agent_loop.register_tool("grep".to_string(), Box::new(SchemafulTool));
    agent_loop.register_tool(
        "tools_schema_get".to_string(),
        Box::new(crate::loop_tools::ToolsSchemaGetTool),
    );

    // lazy off（显式写 off 基线）：取回工具被供给闸隐藏（lazy off 无消费
    // 场景），其余三名全量 schema 上行。
    let dir2 = tempfile::tempdir().unwrap();
    let path2 = dir2.path().join("config.json");
    std::fs::write(&path2, r#"{"agents": {"lazy_tool_schema": false}}"#).unwrap();
    std::mem::forget(dir2);
    agent_loop.set_config_path(path2);
    let full = agent_loop.tool_defs_metrics();
    assert_eq!(full["tools"], 3, "lazy off：三工具全量供给（取回工具隐藏）");

    // 翻 lazy on（热生效语义：set_config_path 换新 config，下一轮即生效）。
    std::mem::forget(dir);
    agent_loop.set_config_path(path);
    let lazy = agent_loop.tool_defs_metrics();
    assert_eq!(
        lazy["tools"],
        full["tools"].as_u64().unwrap() + 1,
        "lazy on：工具数 +1（新增 tools_schema_get 取回工具）"
    );
    let lazy_bytes = lazy["bytes"].as_u64().unwrap();
    let full_bytes = full["bytes"].as_u64().unwrap();
    println!(
        "L10-建设量测（3 schemaful 工具 + normal 档）: lazy off = {full_bytes} bytes / lazy on = {lazy_bytes} bytes"
    );
    assert!(
        lazy_bytes < full_bytes,
        "惰性供给后字节必须严格缩小（占位替换完整 schema 的净收益为正）"
    );
}
