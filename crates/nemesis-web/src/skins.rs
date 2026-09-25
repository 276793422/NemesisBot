//! 皮肤包（`.nbskin`）加载与分发。
//!
//! `.nbskin` = 单文件 ZIP（nbskin v1，未签名，预留 append-only 签名接缝）：
//! `manifest.json`（元数据，`entry` 指向 CSS 载荷）+ 载荷。两种皮肤形态：
//!
//! - **theme**（纯 CSS 换肤）：载荷 `skin/*.css`，经 `/skins/active.css`
//!   与 `/skins/{id}` 分发，前端注入 `<style>`；
//! - **app**（皮肤包自带完整前端应用）：载荷 `app/**`（自包含静态站，
//!   如 openlikebuddy 的 WorkBuddy 形态 UI），经 `/skins/{id}/app[/…]`
//!   分发。bot 前端（web/）零改动——不装皮肤这套 UI 就不存在。
//!
//! 皮肤包部署在 **exe 同级 `skins/` 目录**（与 `static/` 同策略，不落
//! home），格式定案见 `docs/PLAN/2026-09-14_openanybuddy-ide-skin-goal.md`
//! §3.3.3/Q16。
//!
//! 服务端只做**分发**。解析/注入/主题属性全在前端。皮肤路由与静态资源
//! 同级公开（皮肤包是主题资产，无敏感面；app 页面的数据面走鉴权 WS）。
//!
//! 激活 id 来自 `config.json` 的 `ui.skin`（`"default"`/空 = 无皮肤，
//! active.css 404，前端回落内置皮肤并清缓存）。

use axum::Router;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;
use std::io::Read;
use std::path::PathBuf;

/// 皮肤宿主：skins 目录 + 激活 id。
#[derive(Clone)]
pub struct SkinHost {
    /// exe 同级 `skins/` 目录（None = 无法定位 exe 目录，皮肤面整体关闭）。
    dir: Option<PathBuf>,
    /// 激活皮肤 id（config `ui.skin`）。
    active_id: String,
}

/// id 合法性：非空、非 "default"、无路径穿越成分。
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id != "default"
        && !id.contains("..")
        && !id.contains('/')
        && !id.contains('\\')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

/// app 载荷内相对路径合法性：非空、无 `..` 成分、无绝对路径/反斜杠。
fn valid_app_rel(rel: &str) -> bool {
    !rel.is_empty()
        && !rel.split('/').any(|seg| seg == ".." || seg.is_empty())
        && !rel.contains('\\')
}

/// 按扩展名映射 Content-Type（皮肤包内静态资产全集）。
fn app_mime(rel: &str) -> &'static str {
    match rel.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "txt" => "text/plain; charset=utf-8",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

impl SkinHost {
    /// 从 `.nbskin` 包读取皮肤 CSS。任何失败（文件缺失/ZIP 损坏/manifest
    /// 缺 entry）一律 `None` → 上层 404，前端回落内置皮肤。
    fn load_css(&self, id: &str) -> Option<String> {
        if !valid_id(id) {
            return None;
        }
        let path = self.dir.as_ref()?.join(format!("{id}.nbskin"));
        let file = std::fs::File::open(path).ok()?;
        let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file)).ok()?;

        // manifest.json → entry（CSS 载荷路径）
        let mut manifest = String::new();
        zip.by_name("manifest.json")
            .ok()?
            .read_to_string(&mut manifest)
            .ok()?;
        let entry = serde_json::from_str::<serde_json::Value>(&manifest)
            .ok()?
            .get("entry")?
            .as_str()?
            .to_string();
        if entry.contains("..") {
            return None;
        }

        let mut css = String::new();
        zip.by_name(&entry).ok()?.read_to_string(&mut css).ok()?;
        Some(css)
    }

    /// 从 `.nbskin` 包读取 app 载荷文件（`app/{rel}`）。空 rel = index.html。
    /// 返回 (字节, Content-Type)。任何失败 → `None` → 404。
    fn load_app_file(&self, id: &str, rel: &str) -> Option<(Vec<u8>, &'static str)> {
        if !valid_id(id) {
            return None;
        }
        let rel = if rel.is_empty() { "index.html" } else { rel };
        if !valid_app_rel(rel) {
            return None;
        }
        let path = self.dir.as_ref()?.join(format!("{id}.nbskin"));
        let file = std::fs::File::open(path).ok()?;
        let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file)).ok()?;
        let mut entry = zip.by_name(&format!("app/{rel}")).ok()?;
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).ok()?;
        Some((bytes, app_mime(rel)))
    }
}

/// 统一响应：200 + text/css，或 404。
fn css_response(css: Option<String>) -> impl IntoResponse {
    match css {
        Some(css) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "text/css; charset=utf-8"),
                (header::CACHE_CONTROL, "no-cache"),
            ],
            css,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// app 文件统一响应：200 + 按扩展名的 Content-Type + no-cache，或 404。
fn app_file_response(file: Option<(Vec<u8>, &'static str)>) -> impl IntoResponse {
    match file {
        Some((bytes, mime)) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, mime),
                (header::CACHE_CONTROL, "no-cache"),
            ],
            bytes,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// `GET /skins/active.css` — config `ui.skin` 指向的皮肤。响应头
/// `X-Skin-Id` 回传激活 id（前端打 `data-skin` 属性的真相源——localStorage
/// 缓存可能过期）。
async fn handle_active_css(State(host): State<SkinHost>) -> impl IntoResponse {
    match host.load_css(&host.active_id) {
        Some(css) => {
            let mut headers = header::HeaderMap::new();
            headers.insert(
                header::CONTENT_TYPE,
                header::HeaderValue::from_static("text/css; charset=utf-8"),
            );
            headers.insert(
                header::CACHE_CONTROL,
                header::HeaderValue::from_static("no-cache"),
            );
            if let Ok(v) = header::HeaderValue::from_str(&host.active_id) {
                headers.insert(header::HeaderName::from_static("x-skin-id"), v);
            }
            (StatusCode::OK, headers, css).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// `GET /skins/{id}` — 显式 id（前端 `?skin=` 预览路径）。axum 0.8 段内
/// 不允许「参数+后缀」，故路由不带 `.css`；这里接受带/不带后缀两种写法
///（`/skins/openlikebuddy.css` 的段值 = `openlikebuddy.css`）。
async fn handle_skin_css(
    State(host): State<SkinHost>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let id = id.strip_suffix(".css").unwrap_or(&id);
    css_response(host.load_css(id))
}

/// `GET /skins/{id}/app` — 302 到带尾斜杠的 `/skins/{id}/app/`。
///
/// 必须重定向而非直出：皮肤 app 的 HTML 自带 `<base href="./">`（防
/// BridgeSubpathService 注入 `<base href="/">` 把相对资产解析回根路径，
/// 见 ui-src/index.html），而 `./` 以「文档 URL 去掉最后一段」为基——
/// 无尾斜杠时最后一段 `app` 会被丢掉，资产解析落 `/skins/{id}/` 下 404。
async fn handle_app_index(Path(id): Path<String>) -> impl IntoResponse {
    if valid_id(&id) {
        axum::response::Redirect::to(&format!("/skins/{id}/app/")).into_response()
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

/// `GET /skins/{id}/app/` — app 形态皮肤入口页（尾斜杠形态）。
async fn handle_app_index_slash(
    State(host): State<SkinHost>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    app_file_response(host.load_app_file(&id, "index.html"))
}

/// `GET /skins/{id}/app/{*rest}` — app 形态皮肤静态资产。
async fn handle_app_file(
    State(host): State<SkinHost>,
    Path((id, rest)): Path<(String, String)>,
) -> impl IntoResponse {
    app_file_response(host.load_app_file(&id, &rest))
}

/// 构建皮肤路由（挂在主 router 之外、鉴权层之外）。`dir = None`（exe
/// 路径不可定位）→ 不挂任何路由。
pub fn skin_router(dir: Option<String>, active_id: String) -> Option<Router> {
    let dir = dir?;
    let host = SkinHost {
        dir: Some(PathBuf::from(dir)),
        active_id,
    };
    Some(
        Router::new()
            .route("/skins/active.css", get(handle_active_css))
            .route("/skins/{id}", get(handle_skin_css))
            .route("/skins/{id}/app", get(handle_app_index))
            .route("/skins/{id}/app/", get(handle_app_index_slash))
            .route("/skins/{id}/app/{*rest}", get(handle_app_file))
            .with_state(host),
    )
}

#[cfg(test)]
mod tests;
