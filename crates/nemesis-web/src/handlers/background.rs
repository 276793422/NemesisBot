//! 后台进程实时输出投影（2026-11-06 P1）— `background.list` /
//! `background.output` / `background.kill` / `background.steer` /
//! `background.remove` / `background.clear_finished`（用户面清除，2026-11-15）。
//!
//! B4 注册表（`nemesis_agent::BackgroundProcessRegistry`）的 WSAPI 投影面：
//! agent 工具通道（background_start/output/kill/steer）之外的用户观察面与
//! 生命周期清理。数据单一真相源 = gateway 级注册表单例，经 AppState 槽
//! 注入（与 agent 工具通道同一 Arc；headless/relay/测试装配未注入 = 诚实
//! 报错，不伪造空列表）。`list` 只投影元数据行；`output` 按单调字节坐标
//! 增量拉环形缓冲（offset 语义与 registry 同源，响应透传不加工）；`kill`
//! 幂等；`steer` 诚实拒绝面（非 steerable / stdin 已断）由 registry 层
//! 负责；`remove` 仅终态行（运行中拒绝并指引先 kill）；`clear_finished`
//! 批量清除全部终态行。

use crate::ws_router::{ModuleHandler, RequestContext};

pub struct BackgroundHandler;

/// id 宽容解析：json 数字 / 数字串都收（深链 query 回传字符串形态兜底）。
fn parse_job_id(cmd: &str, data: Option<&serde_json::Value>) -> Result<u64, String> {
    let v = data
        .and_then(|d| d.get("id"))
        .ok_or_else(|| format!("background.{cmd} requires {{id}}"))?;
    match v {
        serde_json::Value::Number(n) => n
            .as_u64()
            .ok_or_else(|| format!("background.{cmd}: id must be a u64")),
        serde_json::Value::String(s) => s
            .trim()
            .parse::<u64>()
            .map_err(|_| format!("background.{cmd}: id is not a valid job id: {s}")),
        _ => Err(format!("background.{cmd}: id must be a number")),
    }
}

/// AppState 槽取注册表：未注入（headless/relay/测试装配）= 诚实报错。
fn registry_or_err(
    ctx: &RequestContext,
) -> Result<&nemesis_agent::BackgroundProcessRegistry, String> {
    ctx.state.background_registry.as_deref().ok_or_else(|| {
        "background: registry not injected (this assembly has no background \
         process registry)"
            .to_string()
    })
}

#[async_trait::async_trait]
impl ModuleHandler for BackgroundHandler {
    fn module_name(&self) -> &str {
        "background"
    }

    fn commands(&self) -> &'static [&'static str] {
        &[
            "list",
            "output",
            "kill",
            "steer",
            "remove",
            "clear_finished",
        ]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        match cmd {
            "list" => {
                let jobs = registry_or_err(ctx)?.list().await;
                let count = jobs.len();
                Ok(Some(serde_json::json!({ "jobs": jobs, "count": count })))
            }
            "output" => {
                let id = parse_job_id(cmd, data.as_ref())?;
                let offset = data
                    .as_ref()
                    .and_then(|d| d.get("offset"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                registry_or_err(ctx)?.output(id, offset).await.map(Some)
            }
            "kill" => {
                let id = parse_job_id(cmd, data.as_ref())?;
                registry_or_err(ctx)?.kill(id).await.map(Some)
            }
            "steer" => {
                let id = parse_job_id(cmd, data.as_ref())?;
                let text = data
                    .as_ref()
                    .and_then(|d| d.get("text"))
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| "background.steer requires {text}".to_string())?;
                registry_or_err(ctx)?.steer(id, text).await.map(Some)
            }
            "remove" => {
                let id = parse_job_id(cmd, data.as_ref())?;
                registry_or_err(ctx)?.remove(id).await.map(Some)
            }
            "clear_finished" => Ok(Some(registry_or_err(ctx)?.clear_finished().await)),
            _ => Err(format!("unknown command: background.{}", cmd)),
        }
    }
}
