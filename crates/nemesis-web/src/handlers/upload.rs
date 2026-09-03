//! T8（多模态 goal 2026-09-03）：Dashboard 图片上传端点 + uploads 暂存目录
//! TTL 清扫。
//!
//! `POST /api/upload/image`（走现有 Dashboard 鉴权，`X-Auth-Token` 约定同
//! `/api/internal`）：raw body + `?name=<原始文件名>` → 白名单扩展名 + 25MB
//! 上限 + magic byte 三道校验 → 落 `{workspace}/uploads/`（路径唯一真相源
//! `nemesis_path::resolve_uploads_dir_in_workspace`）→ 返回 `{id, path,
//! size}`。写后即落盘（`std::fs::write` 关 fd 即 flush，纪律 4）。
//!
//! uploads 是 **temp 语义**：[`sweep_uploads_older_than`] 按 mtime 清 7 天
//! 旧文件（gateway 启动 + 周期，照 task_result_store 惯例）；用户点名路径
//! / 历史引用是绝对路径，文件被清扫后水合诚实降级 `[图片已失效]`，不受
//! 清扫逻辑影响。
//!
//! 协议取舍：只收 raw body（`?name=` 带扩展名），不收 multipart——前端
//! `fetch(url, {method:'POST', body: file})` 天然就是 raw body，multipart
//! 需要额外 axum feature 与解析层而无收益。

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

/// 上传体大小上限（与 T4/T5 水合验真的 `MAX_IMAGE_BYTES` 同一真相源）。
pub use nemesis_agent::image_path_detector::MAX_IMAGE_BYTES as UPLOAD_MAX_BYTES;

/// 路由层的 body 硬上限：25MB 有效载荷 + 1MB 余量（超限 axum 直接 413，
/// 不会把字节读进内存）。
pub const UPLOAD_BODY_LIMIT_BYTES: usize = 26 * 1024 * 1024;

/// uploads 暂存目录的 TTL（temp 语义，照 task_result_store 7 天惯例）。
pub const UPLOADS_TTL: Duration = Duration::from_secs(7 * 24 * 3600);

/// `POST /api/upload/image` — 上传一张图片，落盘 uploads，返回 `{id, path, size}`。
///
/// 校验序（任一失败即拒绝，不落盘）：
/// 1. `X-Auth-Token`（空 token 配置 = 鉴权可选，同 `/api/internal` 约定）；
/// 2. `?name=` 白名单扩展名（png/jpg/jpeg/webp/gif，复用 T4 检测器真相源）；
/// 3. ≤25MB（`MAX_IMAGE_BYTES`）；
/// 4. magic byte 嗅探（复用 T4 `sniff_magic`，防文本伪装 .png）；落盘扩展名
///    按**内容**定（`ext_from_magic`，F-C 2026-09-04——`?name=` 只过协议门，
///    content/type 不失配）。
pub async fn handle_upload_image(
    State(state): State<std::sync::Arc<crate::api_handlers::AppState>>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
    body: axum::body::Bytes,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    // 1) Dashboard 鉴权（与 workflow REST 端点同约定）。
    let token = headers
        .get("X-Auth-Token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !crate::api_handlers::verify_token(token, &state.auth_token) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "unauthorized"})),
        ));
    }

    // 2) 扩展名白名单（从 ?name= 抠；无 name 或非白名单 → 400）。
    //    注意这只是**协议门**——落盘扩展名最终按内容定（见步骤 4 F-C）。
    let name = params.get("name").cloned().unwrap_or_default();
    if !nemesis_agent::image_path_detector::has_image_extension(&name) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "unsupported_extension",
                "message": format!(
                    "仅支持 png/jpg/jpeg/webp/gif（收到 name={:?}）",
                    name
                ),
            })),
        ));
    }
    // 落盘扩展名不再取自 name——`has_image_extension` 上面已把过协议门。

    // 3) 大小上限。
    if body.len() as u64 > UPLOAD_MAX_BYTES {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(serde_json::json!({
                "error": "too_large",
                "message": format!(
                    "图片超过 25MB 上限（{} 字节）；不做前端压缩，请缩小后重试",
                    body.len()
                ),
                "size": body.len(),
            })),
        ));
    }

    // 4) magic byte（落盘前嗅探；与水合验真的 T4 真相源同一张签名表）。
    // F-C（2026-09-04 四轮盲审）：落盘扩展名按**内容**定（ext_from_magic），
    // 不信 `?name=` 的声明——JPEG 字节命名 .png 会以 .png 落盘，下游水合
    // 按扩展名定 media_type 时 content/type 失配。嗅探失败（非图片字节）
    // 照旧 415 拒绝、不落盘。
    let ext = match nemesis_agent::image_path_detector::ext_from_magic(&body) {
        Some(ext) => ext.to_string(),
        None => {
            return Err((
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                Json(serde_json::json!({
                    "error": "not_an_image",
                    "message": "文件内容不是图片（magic byte 校验失败）",
                })),
            ));
        }
    };

    // 落盘：uploads/web_{millis}.{ext}。写后关 fd 即落盘（纪律 4）。
    let uploads_dir = nemesis_path::resolve_uploads_dir_in_workspace(
        &nemesis_path::default_path_manager().workspace(),
    );
    if let Err(e) = std::fs::create_dir_all(&uploads_dir) {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": "upload_dir_create_failed",
                "message": format!("创建 uploads 目录失败: {}", e),
            })),
        ));
    }
    // id：web_{millis}_{seq:04x}.{ext}（2026-09-03 二次回归 SUS-2：同一毫秒
    // 并发上传只靠 millis 会碰撞互覆，补进程内原子序号；前端只持 id 不解析
    // 格式，resolve_media_ref 裸文件名校验兼容）。
    static UPLOAD_SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let seq = UPLOAD_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let id = format!(
        "web_{}_{:04x}.{}",
        chrono::Utc::now().timestamp_millis(),
        seq & 0xffff,
        ext
    );
    let dest = uploads_dir.join(&id);
    if let Err(e) = std::fs::write(&dest, &body) {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": "write_failed",
                "message": format!("写入上传文件失败: {}", e),
            })),
        ));
    }

    tracing::info!(
        file = %dest.display(),
        size = body.len(),
        "[Upload] image stored"
    );

    Ok(Json(serde_json::json!({
        "id": id,
        "path": dest.to_string_lossy(),
        "size": body.len(),
    })))
}

/// 解析 WSAPI `chat.send` media 项：`{id}` → uploads 内的文件（id 只允许
/// 裸文件名，防路径穿越）；`{path}` → 原样引用（用户点名路径语义，T5 附加
/// 时照常过安全管线）。返回本地路径字符串；解析失败返回 None（调用方诚实
/// 注记）。
pub fn resolve_media_ref(item: &serde_json::Value) -> Option<String> {
    if let Some(id) = item.get("id").and_then(|v| v.as_str()) {
        let id = id.trim();
        // 裸文件名白名单：字母数字 + `_` `-` `.`，拒绝分隔符与 `..`。
        if id.is_empty()
            || id.contains('/')
            || id.contains('\\')
            || id.contains("..")
            || !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
        {
            return None;
        }
        let uploads_dir = nemesis_path::resolve_uploads_dir_in_workspace(
            &nemesis_path::default_path_manager().workspace(),
        );
        let dest = uploads_dir.join(id);
        return dest.is_file().then(|| dest.to_string_lossy().into_owned());
    }
    if let Some(path) = item.get("path").and_then(|v| v.as_str()) {
        let path = path.trim();
        return (!path.is_empty()).then(|| path.to_string());
    }
    None
}

/// uploads TTL 清扫：删除 uploads 目录下 mtime 早于 `max_age` 的文件。
/// 返回删除数；目录不存在 = 0（首扫前无 uploads 属正常态）。
pub fn sweep_uploads_older_than(uploads_dir: &std::path::Path, max_age: Duration) -> usize {
    let cutoff = std::time::SystemTime::now()
        .checked_sub(max_age)
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    let mut removed = 0usize;
    let Ok(entries) = std::fs::read_dir(uploads_dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let expired = entry
            .metadata()
            .and_then(|m| m.modified())
            .map(|m| m < cutoff)
            .unwrap_or(false);
        if expired && std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    if removed > 0 {
        tracing::info!(
            dir = %uploads_dir.display(),
            removed,
            "[Upload] uploads TTL sweep"
        );
    }
    removed
}

/// Gateway 启动 + 周期清扫入口：先扫一次，再每 6 小时扫一次（7 天 TTL 下
/// 频率远超需要，只是兜底进程常驻场景）。
pub fn spawn_uploads_sweeper() {
    tokio::spawn(async move {
        let uploads_dir: PathBuf = nemesis_path::resolve_uploads_dir_in_workspace(
            &nemesis_path::default_path_manager().workspace(),
        );
        sweep_uploads_older_than(&uploads_dir, UPLOADS_TTL);
        let mut interval = tokio::time::interval(Duration::from_secs(6 * 3600));
        interval.tick().await; // 首 tick 立即返回，跳过（启动已扫过）
        loop {
            interval.tick().await;
            sweep_uploads_older_than(&uploads_dir, UPLOADS_TTL);
        }
    });
}

#[cfg(test)]
mod tests;
