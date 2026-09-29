//! textstat 示例插件（guest 侧，world `plugin-tool`）——**推荐的三方起点**。
//!
//! 与 translate（原始 wit-bindgen 写法）不同，本示例走
//! [`nemesis_plugin_sdk::export_tool!`] 一键导出 + SDK 宿主能力面封装，
//! 演示一个「正经」三方插件的完整要素：
//!
//! 1. **args 解析**：serde_json 正经解析（不像夹具用子串匹配）。
//! 2. **config-schema 消费**：`include_spaces` 配置键（宿主 Dashboard
//!    插件页可设置，热生效）。
//! 3. **数据目录持久化**：调用计数落 `/data`（仅本插件可见的 WASI 挂载点）。
//! 4. **operation_type 声明**：`read`（纯计算 + 只读宿主通道）。
//!
//! 构建：`cargo build --target wasm32-wasip2 --release`
//! 产物：`target/wasm32-wasip2/release/textstat_plugin.wasm`

use nemesis::plugin::host;

struct TextstatPlugin;

/// manifest config-schema 声明：
/// `include_spaces`（"true"/"false"，字符计数是否含空格，缺省含空格）。
fn include_spaces() -> bool {
    !matches!(
        host::config_get("include_spaces").ok().flatten().as_deref(),
        Some("false")
    )
}

/// 调用计数落数据目录（`/data/call_count`；失败静默——计数非关键路径）。
fn bump_call_count() -> u64 {
    let Ok(dir) = host::data_dir_path() else {
        return 0;
    };
    let path = format!("{dir}/call_count");
    let prev = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    let next = prev + 1;
    let _ = std::fs::write(&path, next.to_string());
    next
}

impl exports::nemesis::plugin::tool::Guest for TextstatPlugin {
    fn get_metadata() -> Result<exports::nemesis::plugin::tool::ToolMetadata, host::HostError> {
        Ok(exports::nemesis::plugin::tool::ToolMetadata {
            name: "textstat".into(),
            title: "TextStat".into(),
            description: "统计文本的字符/词/行数（演示 SDK 三方插件的完整要素）".into(),
            parameters_json: r#"{
  "type": "object",
  "properties": {
    "text": { "type": "string", "description": "待统计文本" }
  },
  "required": ["text"]
}"#
            .into(),
            operation_type: "read".into(),
        })
    }

    fn execute(
        input: exports::nemesis::plugin::tool::ToolInput,
    ) -> Result<exports::nemesis::plugin::tool::ToolOutput, host::HostError> {
        #[derive(serde::Deserialize)]
        struct Args {
            text: String,
        }
        let args: Args = serde_json::from_str(&input.args_json).map_err(|e| {
            host::HostError::PolicyDenied(format!("invalid args: {e}"))
        })?;

        let text = &args.text;
        let spaces_on = include_spaces();
        let chars: usize = text
            .chars()
            .filter(|c| spaces_on || !c.is_whitespace())
            .count();
        let words = text.split_whitespace().count();
        let lines = text.lines().count().max(1);
        let calls = bump_call_count();

        host::log(
            host::LogLevel::Info,
            &format!("textstat: {chars} chars / {words} words / {lines} lines"),
        );
        Ok(exports::nemesis::plugin::tool::ToolOutput {
            content: format!(
                r#"{{"chars":{chars},"words":{words},"lines":{lines},"include_spaces":{spaces_on},"calls":{calls}}}"#
            ),
            is_error: false,
        })
    }
}

nemesis_plugin_sdk::export_tool!(TextstatPlugin);
