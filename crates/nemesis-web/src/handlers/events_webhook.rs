//! webhook 事件源 HTTP 入口 — `POST /api/events/webhook/{token}`（2026-11-16
//! 事件源扩展 P1）。
//!
//! **token 即凭据**（`/api/share/{token}` 同款先例）：路径段 token 与
//! events.json 里 webhook 规则的 `watch.token` 精确匹配，命中 → 载荷进泵
//! pending（三闸/estop 冻结/投递管线全复用），返回命中的规则 id 列表；
//! 未命中任何规则 = 404（诚实：调用方拼错 token 不给假成功）；泵未装配
//! （feature 关 / headless）= 503。
//!
//! 装配形态：**进程级 OnceLock 静态槽**（`set_events_webhook`，gateway
//! 泵 start 前注入 submitter）——不走 AppState 字段（AppState 测试字面量
//! 装配面 40+ 处，加字段即全量机械改动；静态槽与 plugins_wasm
//! tool_sync_hook 同款先例）。None = 路由存在但诚实 503。
//!
//! estop 语义：token 匹配/应答不受冻结影响（fs 事件同语义——入队后冻结期
//! 由泵 run_cycle 诚实丢弃计数），HTTP 面 200 只代表「token 认领、载荷入队」。

use std::sync::OnceLock;

/// 泵 webhook 提交句柄静态槽（gateway 泵启动前注入；进程内单值）。
static EVENTS_WEBHOOK: OnceLock<nemesis_events::WebhookSubmitter> = OnceLock::new();

/// 注入提交句柄（gateway post_agent events 块调用；重复注入以首次为准——
/// 生产 gateway 单台单泵，重复注入 = 装配 bug，warn 留痕）。
pub fn set_events_webhook(submitter: nemesis_events::WebhookSubmitter) {
    if EVENTS_WEBHOOK.set(submitter).is_err() {
        tracing::warn!("[Events/webhook] submitter already set (重复注入以首次为准)");
    }
}

/// 载荷字节上限（HTTP 面独立闸；泵内另有 per-event 1024B 截断——
/// 合并消息体封顶）。超限 = 413，不静默截外部调用方的载荷。
const WEBHOOK_BODY_MAX: usize = 8 * 1024;

/// POST /api/events/webhook/{token} —— 公开端点，**故意不校验
/// verify_token**（token 即凭据；auth_exempt_path 放行）。
pub async fn handle_events_webhook(
    axum::extract::Path(token): axum::extract::Path<String>,
    body: axum::body::Bytes,
) -> axum::response::Response {
    respond(EVENTS_WEBHOOK.get(), &token, body).await
}

/// 处理核心（submitter 显式传入——测试可直调，不经进程级静态槽，
/// 避免测试间 OnceLock 状态串扰）。
pub(crate) async fn respond(
    submitter: Option<&nemesis_events::WebhookSubmitter>,
    token: &str,
    body: axum::body::Bytes,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let json = |code: axum::http::StatusCode, v: serde_json::Value| -> axum::response::Response {
        (code, axum::Json(v)).into_response()
    };
    let Some(submitter) = submitter else {
        return json(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            serde_json::json!({"error": "events pump not wired (feature off or not started)"}),
        );
    };
    if body.len() > WEBHOOK_BODY_MAX {
        return json(
            axum::http::StatusCode::PAYLOAD_TOO_LARGE,
            serde_json::json!({"error": format!("payload exceeds {}B", WEBHOOK_BODY_MAX)}),
        );
    }
    let payload = String::from_utf8_lossy(&body);
    match submitter.submit(token, payload.trim()).await {
        Ok(matched) if !matched.is_empty() => {
            tracing::info!(
                "[Events/webhook] token accepted, matched rule(s): {:?}",
                matched
            );
            json(
                axum::http::StatusCode::OK,
                serde_json::json!({"accepted": true, "rules": matched}),
            )
        }
        // token 无规则认领 = 404（不泄露「token 存在但被禁用」之类的细粒度
        // 状态——对未授权调用方统一未知形态）。
        Ok(_) => json(
            axum::http::StatusCode::NOT_FOUND,
            serde_json::json!({"accepted": false, "error": "no rule claims this token"}),
        ),
        Err(e) => json(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            serde_json::json!({"accepted": false, "error": e}),
        ),
    }
}

#[cfg(all(test, feature = "events"))]
mod tests;
