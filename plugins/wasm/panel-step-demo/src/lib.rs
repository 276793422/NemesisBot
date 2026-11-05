//! panel-step-demo 示例插件（guest 侧，组合 world `plugin-panel-flow-step`）
//! ——「面板管配置/凭据，步骤做取数/回写」形态的三方起点（2026-11-03 组合
//! world 批）。
//!
//! 演示组合插件的完整要素：
//!
//! 1. **`export_panel_step!` 一键导出**：同一组件同时实现
//!    `exports::nemesis::plugin::ui::Guest`（面板命令通道）与
//!    `exports::nemesis::plugin::flow_step::Guest`（flow 步骤执行体）。
//! 2. **帧纪律互不渗透（活体证明）**：ui 命令走 Panel 帧——secret-set /
//!    config-set 写面在场（自持写通道）；flow-step 的
//!    `probe_write_denial` op 同样调这两个写函数，但走 FlowStep 帧
//!    （构造期即无写面）→ 每一路都应报 `denied_unavailable`。
//!    面板能写、步骤写不动，同一组件同一份 guest 代码——闸在帧上。
//!
//! ui 命令词表（单入口 handle-ui-command）：
//! - `ping`：连通性。
//! - `set_secret {value}`：secret-set 写凭据（`probe_token`，manifest
//!   x-secret 声明）。
//! - `set_config {value}`：config-set 写实例配置（`probe_mode`，
//!   config-schema 声明）。
//! - `get_config`：config-get 读回 `probe_mode`。
//!
//! flow-step 词表（kinds 自报）：
//! - `probe_write_denial`：尝试 secret-set / config-set 并把每一路结果
//!   （allowed / denied_unavailable / denied_other:*）放进 artifacts。
//!
//! 构建：`cargo build --target wasm32-wasip2 --release`
//! 产物：`target/wasm32-wasip2/release/panel_step_demo_plugin.wasm`

use nemesis::plugin::host;
use serde_json::json;

struct PanelStepDemo;

/// 写调用结果分类（flow-step 侧探针用）：Unavailable = 帧纪律拦下
///（期望值），其余 host-error = 其他闸层（也如实上报，不吞）。
fn classify_write(r: Result<(), host::HostError>) -> serde_json::Value {
    match r {
        Ok(()) => json!("allowed"),
        Err(host::HostError::Unavailable(_)) => json!("denied_unavailable"),
        Err(other) => json!(format!("denied_other:{other:?}")),
    }
}

impl exports::nemesis::plugin::ui::Guest for PanelStepDemo {
    fn handle_ui_command(input: String) -> Result<String, host::HostError> {
        let v: serde_json::Value = serde_json::from_str(&input)
            .map_err(|e| host::HostError::PolicyDenied(format!("input not json: {e}")))?;
        let cmd = v.get("command").and_then(|x| x.as_str()).unwrap_or("");
        let args = v.get("args").cloned().unwrap_or(json!({}));
        match cmd {
            "ping" => Ok(json!({ "pong": true, "world": "panel-flow-step" }).to_string()),

            // Panel 帧写通道正向控制：声明名才放行（cb 闸序：帧 → writer
            // 接线 → 声明 namespace）。业务失败不 trap，回 {"error":...}。
            "set_secret" => {
                let value = args.get("value").and_then(|x| x.as_str()).unwrap_or("");
                match host::secret_set("probe_token", value) {
                    Ok(()) => Ok(json!({ "ok": true }).to_string()),
                    Err(e) => Ok(json!({ "error": format!("{e:?}") }).to_string()),
                }
            }
            "set_config" => {
                let value = args.get("value").and_then(|x| x.as_str()).unwrap_or("");
                match host::config_set("probe_mode", value) {
                    Ok(()) => Ok(json!({ "ok": true }).to_string()),
                    Err(e) => Ok(json!({ "error": format!("{e:?}") }).to_string()),
                }
            }
            "get_config" => match host::config_get("probe_mode") {
                Ok(Some(val)) => Ok(json!({ "value": val }).to_string()),
                Ok(None) => Ok(json!({ "value": serde_json::Value::Null }).to_string()),
                Err(e) => Ok(json!({ "error": format!("{e:?}") }).to_string()),
            },

            other => Ok(json!({ "error": format!("unknown command '{other}'") }).to_string()),
        }
    }
}

impl exports::nemesis::plugin::flow_step::Guest for PanelStepDemo {
    fn get_step_metadata(
    ) -> Result<exports::nemesis::plugin::flow_step::StepMetadata, host::HostError> {
        Ok(exports::nemesis::plugin::flow_step::StepMetadata {
            kinds: vec!["probe_write_denial".into()],
        })
    }

    fn run_step(input: String) -> Result<String, host::HostError> {
        let v: serde_json::Value = serde_json::from_str(&input)
            .map_err(|e| host::HostError::PolicyDenied(format!("input not json: {e}")))?;
        let op = v.get("op").and_then(|x| x.as_str()).unwrap_or("");
        if op != "probe_write_denial" {
            return Err(host::HostError::NotFound(format!(
                "op '{op}' not in kinds wordlist"
            )));
        }
        // 同一份写调用在 FlowStep 帧里必须被拦（构造期无写面）——
        // 结果逐路放 artifacts，e2e 断言 denied_unavailable。
        let secret = classify_write(host::secret_set("probe_token", "from-flow-step"));
        let config = classify_write(host::config_set("probe_mode", "from-flow-step"));
        Ok(json!({
            "status": "done",
            "secret_set": secret,
            "config_set": config,
        })
        .to_string())
    }
}

nemesis_plugin_sdk::export_panel_step!(PanelStepDemo);
