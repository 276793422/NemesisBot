//! stepdemo 示例插件（guest 侧，world `plugin-flow-step`）——
//! flow 步骤插件的三方起点（2026-11-03 guest step 批）。
//!
//! 演示 flow-step 插件的完整要素：
//!
//! 1. **`export_step!` 一键导出**（与 textstat 的 export_tool! 同纪律：
//!    工程根须有 `wit/` 合同副本）。
//! 2. **kinds 词表自报**：`get_step_metadata` 返回本插件可跑的 op 名单
//!    （安装期对账 + 运行期路由事实源）。
//! 3. **四态生命周期**：done/running/failed/skipped 全形态示范。
//! 4. **幂等由 guest 负责**：`tally` op 把受理计数落数据目录
//!    （`/data/tally_<instance>`，仅本插件可见），前 2 次投递回
//!    `running`、第 3 次回 `done`——宿主每 tick 重投，guest 凭自身
//!    记账去重，`attempts` 入参用于对账。
//!
//! 三个 op（词表即文档）：
//! - `echo`：同步 done，artifacts 回显 args（演示出参自由载荷）。
//! - `tally`：受理 3 次才 done（演示 running 轮询 + guest 侧幂等记账）。
//! - `chaos`：`args.mode` 驱动 done/running/failed/skipped/malformed/err
//!   六分支（测试确定性问题注入器；malformed/err 演示宿主侧失败处置）。
//!
//! 构建：`cargo build --target wasm32-wasip2 --release`
//! 产物：`target/wasm32-wasip2/release/stepdemo_plugin.wasm`

use nemesis::plugin::host;
use serde_json::json;

struct StepdemoPlugin;

/// 数据目录受理计数（`/data/tally_<instance>`；失败按 0 起算——计数
/// 非关键路径，诚实降级为多跑一轮）。
fn tally_count(instance: &str) -> u64 {
    let Ok(dir) = host::data_dir_path() else {
        return 0;
    };
    std::fs::read_to_string(format!("{dir}/tally_{instance}"))
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

fn tally_bump(instance: &str) {
    if let Ok(dir) = host::data_dir_path() {
        let _ = std::fs::write(
            format!("{dir}/tally_{instance}"),
            (tally_count(instance) + 1).to_string(),
        );
    }
}

impl exports::nemesis::plugin::flow_step::Guest for StepdemoPlugin {
    fn get_step_metadata(
    ) -> Result<exports::nemesis::plugin::flow_step::StepMetadata, host::HostError> {
        Ok(exports::nemesis::plugin::flow_step::StepMetadata {
            // 词表即文档：模板里 `kind = "plugin:stepdemo:<op>"` 只认这三者。
            kinds: vec!["echo".into(), "tally".into(), "chaos".into()],
        })
    }

    fn run_step(input: String) -> Result<String, host::HostError> {
        let v: serde_json::Value = serde_json::from_str(&input)
            .map_err(|e| host::HostError::PolicyDenied(format!("input not json: {e}")))?;
        let op = v.get("op").and_then(|x| x.as_str()).unwrap_or("");
        let instance = v
            .get("instance")
            .and_then(|x| x.get("instance"))
            .and_then(|x| x.as_str())
            .unwrap_or("unknown");

        match op {
            // 同步 done：artifacts 回显 args（token 已由宿主渲染）。
            "echo" => Ok(json!({
                "status": "done",
                "echoed": v.get("args").cloned().unwrap_or(serde_json::Value::Null),
            })
            .to_string()),

            // running ×2 → done：guest 记账幂等（数据目录计数），
            // attempts 入参只作对账注记，不作为状态依据。
            "tally" => {
                let n = tally_count(instance) + 1;
                tally_bump(instance);
                if n < 3 {
                    Ok(json!({ "status": "running", "polls": n }).to_string())
                } else {
                    Ok(json!({
                        "status": "done",
                        "polls": n,
                        "attempts_seen": v.get("attempts").cloned().unwrap_or(json!(0)),
                    })
                    .to_string())
                }
            }

            // 确定性四态驱动器（测试用）：mode 六分支。
            "chaos" => {
                let mode = v
                    .get("args")
                    .and_then(|a| a.get("mode"))
                    .and_then(|m| m.as_str())
                    .unwrap_or("done");
                match mode {
                    "done" => Ok(json!({ "status": "done", "chaos": "resolved" }).to_string()),
                    "running" => Ok(json!({ "status": "running" }).to_string()),
                    "failed" => Ok(json!({
                        "status": "failed",
                        "error": "chaos: 注定失败（mode=failed）",
                    })
                    .to_string()),
                    "skipped" => Ok(json!({ "status": "skipped", "why": "chaos" }).to_string()),
                    // 畸形出参：宿主按步骤失败处置（走 on_fail 链）。
                    "malformed" => Ok("not-json-at-all".into()),
                    // host-error 臂：guest 侧真正报错（非业务失败形态）。
                    "err" => Err(host::HostError::Unavailable(
                        "chaos: guest 侧异常（mode=err）".into(),
                    )),
                    other => Ok(json!({ "status": other }).to_string()),
                }
            }

            other => Err(host::HostError::NotFound(format!(
                "op '{other}' not in kinds wordlist"
            ))),
        }
    }
}

nemesis_plugin_sdk::export_step!(StepdemoPlugin);
