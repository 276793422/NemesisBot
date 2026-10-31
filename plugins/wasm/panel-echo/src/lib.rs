//! panel-echo 示例插件（guest 侧，world `plugin-panel`）。
//!
//! 三方开发者照这里写面板插件：manifest `[[ui]]` 段声明页面骨架（宿主
//! 预置组件渲染），guest 实现 [`Guest::handle_ui_command`] 承载数据投影
//! 与动作翻译（血肉）。安装：`nemesisbot plugin install <staging目录>`，
//! `plugin enable panel-echo` 后 Dashboard【定制】分组出现页面。
//!
//! 本插件同时是宿主 e2e 测试的行为夹具，`command` 字段切换行为：
//! - `echo`：回显 `args.text`（基线往返）。
//! - `rows`：回固定表格行（table 视图数据投影形态）。
//! - `fail`：业务失败形态 `{"error":...}`（渲染层诚实展示，非 trap）。
//! - `trap`：故意 panic（宿主错误面归约路径）。
//! - `probe`：宿主能力面自检摘要（log / now / tool-invoke 预期 unavailable）。
//!
//! 注意：args 解析是夹具级的子串匹配（不做通用 JSON 解析），三方插件
//! 请用 serde_json 之类正经实现。

wit_bindgen::generate!({
    path: "wit",
    world: "plugin-panel",
});

use exports::nemesis::plugin::ui::Guest;
use nemesis::plugin::host::{self, HostError, LogLevel};

struct PanelEchoPlugin;

/// 夹具级 JSON 字符串字段提取（`"key":"value"` 子串匹配；无转义处理）。
fn extract_str(input: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\":\"");
    let start = input.find(&needle)? + needle.len();
    let rest = &input[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

impl Guest for PanelEchoPlugin {
    fn handle_ui_command(input: String) -> Result<String, HostError> {
        let command = extract_str(&input, "command").unwrap_or_default();
        match command.as_str() {
            "echo" => {
                let text = extract_str(&input, "text").unwrap_or_default();
                Ok(format!(r#"{{"ok":true,"echo":"{text}"}}"#))
            }
            "rows" => Ok(r#"{"rows":[{"id":"NB-1","title":"样例行","status":"todo"},
                {"id":"NB-2","title":"第二行","status":"done"}]}"#.replace('\n', "")),
            "fail" => Ok(r#"{"error":"演示性业务失败（guest 约定形态，非 trap）"}"#.to_string()),
            "trap" => {
                panic!("deliberate panel trap fixture");
            }
            "probe" => {
                host::log(LogLevel::Info, "panel probe begin");
                let now = host::now_millis();
                // 面板帧 invoker 在场（guest 动作要走 tool-invoke 全闸），
                // 但独立宿主运行时（测试形态）无接线 → unavailable。
                let invoke = host::tool_invoke("exec", r#"{"cmd":"echo hi"}"#);
                let invoke_note = match invoke {
                    Ok(_) => "tool-invoke=unexpected-ok",
                    Err(HostError::Unavailable(_)) => "tool-invoke=unavailable",
                    Err(_) => "tool-invoke=other",
                };
                host::log(LogLevel::Info, "panel probe end");
                Ok(format!(r#"{{"ok":true,"now":{now},"invoke":"{invoke_note}"}}"#))
            }
            other => Ok(format!(r#"{{"error":"unknown command: {other}"}}"#)),
        }
    }
}

export!(PanelEchoPlugin);
