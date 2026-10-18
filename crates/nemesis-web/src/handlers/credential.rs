//! B4（2026-10-18 凭据 vault §7）— `credential.respond` / `credential.pending`。
//!
//! Dashboard 凭据卡的 WSAPI 入口：把用户提交的凭据值送回等待中的
//! `WebCredentialBroker::request_credential`（触发点阻塞处），broker **先落
//! vault 再送裁决**——值经本入口进入后立即写 vault，成功才回 `stored`。
//! broker 经静态槽触达（gateway 装配时 `set_broker` 注入；nemesis-web 不
//! 依赖 nemesisbot，方向约束同 plugins_wasm installer 整槽注入先例）——未
//! 装配（headless / 无 security 构建不装）时诚实报「未装配」。
//!
//! **凭据值不落日志/错误/审计**（vault 纪律）：handler 只透传 value 给
//! broker，日志只记 request_id 与回执里的 key_name/rotated。

use std::sync::Arc;

use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_types::agent::CredentialResponder;

static BROKER: std::sync::LazyLock<parking_lot::RwLock<Option<Arc<dyn CredentialResponder>>>> =
    std::sync::LazyLock::new(|| parking_lot::RwLock::new(None));

/// gateway 装配点注入（init_agent 一次性；重复安装覆盖——agent 重启重建
/// broker 实例后以最新为准）。
pub fn set_broker(broker: Arc<dyn CredentialResponder>) {
    *BROKER.write() = Some(broker);
}

/// 摘除（测试卸装用；生产流程不调——槽常驻 gateway 生命周期）。
#[doc(hidden)]
pub fn clear_broker() {
    *BROKER.write() = None;
}

pub struct CredentialHandler;

#[async_trait::async_trait]
impl ModuleHandler for CredentialHandler {
    fn module_name(&self) -> &str {
        "credential"
    }

    fn commands(&self) -> &'static [&'static str] {
        &["respond", "pending"]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        _ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        // 槽读取窗口尽量窄：clone 出 Arc 后立即放锁（同 question handler）。
        let broker = BROKER
            .read()
            .clone()
            .ok_or_else(|| "credential broker not wired".to_string())?;
        match cmd {
            "respond" => {
                let data = data.ok_or("missing data")?;
                let request_id = data
                    .get("request_id")
                    .and_then(|v| v.as_str())
                    .ok_or("missing request_id")?
                    .to_string();
                let value = data
                    .get("value")
                    .and_then(|v| v.as_str())
                    .ok_or("missing value")?
                    .to_string();
                // respond 含 vault 落盘（argon2 解锁是 CPU 重操作）——
                // spawn_blocking 让出 async worker，不阻塞 reactor。
                let receipt =
                    tokio::task::spawn_blocking(move || broker.respond(&request_id, &value))
                        .await
                        .map_err(|e| format!("credential respond 任务失败: {e}"))??;
                tracing::info!(
                    "[Web/WSAPI] credential.respond stored={} key_name={} rotated={}（值不落日志）",
                    receipt["stored"],
                    receipt["key_name"],
                    receipt["rotated"]
                );
                Ok(Some(receipt))
            }
            "pending" => Ok(Some(serde_json::json!({
                "pending": broker.pending(),
            }))),
            _ => Err(format!("unknown command: credential.{}", cmd)),
        }
    }
}

#[cfg(test)]
mod tests;
