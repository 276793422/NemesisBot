//! Swarm M3（§5.4/D6）：看板资产下载端点——引用随包走、内容按需 HTTP 拉。
//!
//! `GET /api/board/asset/{ref}?asset_token=&expires_at=` —— 公开端点，
//! **故意不走 dashboard verify_token**（web token 不跨节点，worker 没有
//! master 的 dashboard 凭据；下载凭据 = asset_token 随引用走，同
//! `/api/share/{token}` 的 token-as-credential 模式）。
//!
//! 校验序列（每步都先于下一步，任一失败即诚实拒绝）：
//! 1. board 服务/资产配置就绪（503）
//! 2. 引用名字符白名单（[`nemesis_board::asset_token::sanitize_asset_ref`]，
//!    400——路径分隔符/`..`/隐藏名在字符集层就数学排除）
//! 3. token + expires_at query 齐（400）
//! 4. asset 表登记白名单（404——只认登记过的 ref，未登记一律不存在）
//! 5. HMAC 签名 + 过期（403；Invalid/Expired message 区分，状态码统一：
//!    对攻击者不泄露区分度，对合法使用者可读懂「该重拿引用了」）
//! 6. 实体文件存在（404——表有盘无诚实报错）
//! 7. 全局并发闸节流（失控拉取循环排队自然限速）→ 流式回文件
//!
//! 只读：无任何写路径；Range 断点续传依赖 axum/前端后续需要时再补
//! （集群内网文件量级下整流已够，诚实边界）。

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use tokio::sync::Semaphore;

use crate::api_handlers::AppState;
use nemesis_board::asset_token::{sanitize_asset_ref, verify_asset_token, AssetTokenError};

/// 全局并发闸：集群互拉就几个节点，4 并发足够；失控拉取循环在这里
/// 排队节流（进程级单例——IP 维度限速需要 ConnectInfo，当前 serve
/// 未启用，集群内网场景全局闸更诚实）。
static ASSET_FETCH_PERMITS: OnceLock<Semaphore> = OnceLock::new();

fn fetch_permits() -> &'static Semaphore {
    ASSET_FETCH_PERMITS.get_or_init(|| Semaphore::new(4))
}

fn err_response(status: StatusCode, msg: &str) -> Response {
    (status, axum::Json(serde_json::json!({ "error": msg }))).into_response()
}

/// GET /api/board/asset/{ref} —— 公开资产下载（token 即凭据）。
pub async fn handle_board_asset_download(
    Path(asset_ref): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let Some(board) = state.board.as_ref() else {
        return err_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "board service not available",
        );
    };
    if !board.asset_serving_ready() {
        return err_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "asset serving not configured on this node",
        );
    }
    let secret = match board.asset_secret() {
        Some(s) => s.to_vec(),
        None => {
            return err_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "asset serving not configured on this node",
            )
        }
    };

    // 字符白名单先行：非法名（含路径分隔符/`..`）根本不值得查表。
    if let Err(e) = sanitize_asset_ref(&asset_ref) {
        return err_response(StatusCode::BAD_REQUEST, &e);
    }
    let Some(token) = params.get("asset_token").map(String::as_str) else {
        return err_response(StatusCode::BAD_REQUEST, "missing asset_token query param");
    };
    let Some(expires_raw) = params.get("expires_at") else {
        return err_response(StatusCode::BAD_REQUEST, "missing expires_at query param");
    };
    let Ok(expires_at) = expires_raw.parse::<i64>() else {
        return err_response(StatusCode::BAD_REQUEST, "expires_at must be unix seconds");
    };

    // 表登记白名单：未登记的 ref 一律「不存在」（防枚举 + 防穿越兜底）。
    let asset = match board.store().lookup_asset(&asset_ref) {
        Ok(Some(a)) => a,
        Ok(None) => {
            return err_response(StatusCode::NOT_FOUND, "asset not found");
        }
        Err(e) => return err_response(StatusCode::INTERNAL_SERVER_ERROR, &e),
    };

    // HMAC + 过期。
    match verify_asset_token(&secret, &asset_ref, expires_at, token) {
        Ok(()) => {}
        Err(AssetTokenError::Expired) => {
            return err_response(
                StatusCode::FORBIDDEN,
                "asset token expired — request a fresh reference",
            )
        }
        Err(AssetTokenError::Invalid) => {
            return err_response(StatusCode::FORBIDDEN, "asset token invalid")
        }
    }

    // 实体文件（sanitize 已保证单段文件名；join 后不可能是目录外路径）。
    let assets_dir = board
        .assets_dir()
        .expect("checked by asset_serving_ready above")
        .to_path_buf();
    let file_path = assets_dir.join(&asset_ref);
    let Ok(meta) = tokio::fs::metadata(&file_path).await else {
        return err_response(
            StatusCode::NOT_FOUND,
            "asset registered but content missing on disk",
        );
    };
    let Ok(file) = tokio::fs::File::open(&file_path).await else {
        return err_response(
            StatusCode::NOT_FOUND,
            "asset registered but content unreadable",
        );
    };

    let _permit = fetch_permits().acquire().await;
    let size = meta.len();
    let sha_header = asset.sha256.clone();
    let content_type = crate::server::content_type_for(&asset_ref);
    let stream = tokio_util::io::ReaderStream::new(file);
    let body = axum::body::Body::from_stream(stream);
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type),
            (header::CONTENT_LENGTH, size.to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{asset_ref}\""),
            ),
            (
                header::HeaderName::from_static("x-asset-sha256"),
                sha_header,
            ),
        ],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests;
