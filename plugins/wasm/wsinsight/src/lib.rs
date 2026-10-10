//! wsinsight 示例插件（guest 侧，world `plugin-tool`）——工作区文件洞察。
//!
//! 演示 **workspace-read 宿主面**（工作区只读 IO + 宿主路径围栏）的正规
//! 三方用法：给定工作区相对路径，返回文件元信息（字节/行/UTF-8 合法性/
//! 预览）。宿主围栏拒绝的越界路径（`../`、绝对路径、8.3 短名）以
//! `policy-denied` 返回，本插件转为 `is_error` 业务失败回灌 LLM 自纠。
//!
//! manifest config-schema 声明：`max_preview_default`（预览字符缺省值，
//! "0"-"4096"，缺省 0 = 不预览）。
//!
//! 构建：`cargo build --target wasm32-wasip2 --release`
//! 产物：`target/wasm32-wasip2/release/wsinsight_plugin.wasm`

use nemesis::plugin::host;

struct WsInsightPlugin;

/// 预览字符数：显式入参 > 实例配置 `max_preview_default` > 0（不预览）。
/// 上限 4096 字符，防单次输出撑爆会话（工具结果有会话预算）。
fn preview_limit(explicit: Option<usize>) -> usize {
    let cfg = host::config_get("max_preview_default")
        .ok()
        .flatten()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .unwrap_or(0);
    explicit.unwrap_or(cfg).min(4096)
}

impl exports::nemesis::plugin::tool::Guest for WsInsightPlugin {
    fn get_metadata() -> Result<exports::nemesis::plugin::tool::ToolMetadata, host::HostError> {
        Ok(exports::nemesis::plugin::tool::ToolMetadata {
            name: "wsinsight".into(),
            title: "WsInsight".into(),
            description: "读取工作区内文件的元信息（字节数/行数/UTF-8 合法性/可选预览）。参数：path（工作区相对路径，必填）、max_preview（预览字符数，可选）。路径越界或文件不存在会返回明确错误说明。".into(),
            parameters_json: r#"{
  "type": "object",
  "properties": {
    "path": { "type": "string", "description": "工作区相对路径，如 'IDENTITY.md'" },
    "max_preview": { "type": "integer", "description": "预览前 N 个字符（0-4096，缺省读实例配置）" }
  },
  "required": ["path"]
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
            path: String,
            max_preview: Option<usize>,
        }
        let args: Args = serde_json::from_str(&input.args_json)
            .map_err(|e| host::HostError::PolicyDenied(format!("invalid args: {e}")))?;

        // 宿主围栏在工作区只读通道内生效（相对锚定 + 越界/绝对/8.3 拒绝）：
        // Err 一律转 is_error 业务失败回灌（LLM 拿到 reason 可自纠），不 trap。
        let bytes = match host::workspace_read(&args.path) {
            Ok(b) => b,
            Err(e) => {
                let reason = match &e {
                    host::HostError::NotFound(d) => format!("文件不存在: {d}"),
                    host::HostError::PolicyDenied(d) => {
                        format!("路径被工作区围栏拒绝（须为工作区内相对路径）: {d}")
                    }
                    other => format!("读取失败: {other:?}"),
                };
                host::log(host::LogLevel::Warn, &format!("wsinsight: {reason}"));
                return Ok(exports::nemesis::plugin::tool::ToolOutput {
                    content: format!(r#"{{"path":{},"error":"{}"}}"#,
                        serde_json::to_string(&args.path).unwrap_or_else(|_| "\"?\"".into()),
                        reason.replace('\\', "\\\\").replace('"', "\\\"")),
                    is_error: true,
                });
            }
        };

        let total_bytes = bytes.len();
        let newlines = bytes.iter().filter(|&&b| b == b'\n').count();
        let lines = if total_bytes == 0 {
            0
        } else if *bytes.last().unwrap() == b'\n' {
            newlines
        } else {
            newlines + 1
        };
        let (utf8_valid, preview) = match std::str::from_utf8(&bytes) {
            Ok(s) => {
                let n = preview_limit(args.max_preview);
                let cut = s.char_indices().nth(n).map(|(i, _)| i).unwrap_or(s.len());
                (true, s[..cut].to_string())
            }
            Err(_) => (false, String::new()),
        };

        host::log(
            host::LogLevel::Info,
            &format!("wsinsight: {} -> {total_bytes} bytes / {lines} lines", args.path),
        );
        Ok(exports::nemesis::plugin::tool::ToolOutput {
            content: format!(
                r#"{{"path":{},"bytes":{total_bytes},"lines":{lines},"utf8_valid":{utf8_valid},"preview":{}}}"#,
                serde_json::to_string(&args.path).unwrap_or_else(|_| "\"?\"".into()),
                serde_json::to_string(&preview).unwrap_or_else(|_| "\"\"".into()),
            ),
            is_error: false,
        })
    }
}

nemesis_plugin_sdk::export_tool!(WsInsightPlugin);
