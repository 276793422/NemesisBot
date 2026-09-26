//! skins 模块测试（内联纪律：生产文件只留 `mod tests;` 声明）。

use super::*;
use axum::body::Body;
use axum::http::Request;
use tower::ServiceExt;

/// 建一个临时 skins 目录，写入 `id + ".nbskin"` 包（manifest 指向给定
/// entry 文件）。
fn write_skin(dir: &std::path::Path, id: &str, entry: &str) {
    use std::io::Write;
    let path = dir.join(format!("{id}.nbskin"));
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    zip.add_directory("skin", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.start_file("manifest.json", zip::write::SimpleFileOptions::default())
        .unwrap();
    write!(
        zip,
        r#"{{"id":"{id}","version":"0.1.0","entry":"{entry}"}}"#
    )
    .unwrap();
    zip.start_file(entry, zip::write::SimpleFileOptions::default())
        .unwrap();
    write!(zip, "html[data-skin=\"{id}\"] {{ --accent: #00C29A; }}").unwrap();
    zip.finish().unwrap();
}

fn app(dir: Option<&str>, active: &str) -> axum::Router {
    super::skin_router(
        dir.map(str::to_string),
        std::sync::Arc::new(parking_lot::RwLock::new(active.to_string())),
    )
    .unwrap()
}

#[tokio::test]
async fn active_css_serves_manifest_entry() {
    let tmp = tempdir();
    write_skin(tmp.path(), "openlikebuddy", "skin/openlikebuddy.css");
    let res = app(Some(tmp.path().to_str().unwrap()), "openlikebuddy")
        .oneshot(
            Request::builder()
                .uri("/skins/active.css")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(
        res.headers().get(header::CONTENT_TYPE).unwrap(),
        "text/css; charset=utf-8"
    );
    assert_eq!(res.headers().get("x-skin-id").unwrap(), "openlikebuddy");
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(text.contains("--accent: #00C29A"));
}

#[tokio::test]
async fn explicit_id_serves_and_missing_is_404() {
    let tmp = tempdir();
    write_skin(tmp.path(), "alpha", "skin/x.css");
    let a = app(Some(tmp.path().to_str().unwrap()), "alpha");

    // 带 .css 后缀（段值整体 = "alpha.css"，handler 剥后缀）
    let ok = a
        .clone()
        .oneshot(
            Request::builder()
                .uri("/skins/alpha.css")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ok.status(), 200);

    // 裸 id 同样可用
    let bare = a
        .clone()
        .oneshot(
            Request::builder()
                .uri("/skins/alpha")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bare.status(), 200);

    // active id 指向缺失包（部署时包没拷）→ 404
    let miss = app(Some(tmp.path().to_str().unwrap()), "ghost")
        .oneshot(
            Request::builder()
                .uri("/skins/active.css")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(miss.status(), 404);
}

#[tokio::test]
async fn default_active_id_is_404() {
    let tmp = tempdir();
    write_skin(tmp.path(), "openlikebuddy", "skin/x.css");
    let res = app(Some(tmp.path().to_str().unwrap()), "default")
        .oneshot(
            Request::builder()
                .uri("/skins/active.css")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
}

#[tokio::test]
async fn path_traversal_and_bad_ids_rejected() {
    let tmp = tempdir();
    let a = app(Some(tmp.path().to_str().unwrap()), "openlikebuddy");
    for bad in [
        "/skins/..%2F..%2Fsecret.css",
        "/skins/has%20space.css",
        "/skins/no_such.css",
    ] {
        let res = a
            .clone()
            .oneshot(Request::builder().uri(bad).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), 404, "bad id must 404: {bad}");
    }
}

#[tokio::test]
async fn no_dir_no_routes() {
    assert!(
        super::skin_router(
            None,
            std::sync::Arc::new(parking_lot::RwLock::new("openlikebuddy".into()))
        )
        .is_none()
    );
}

/// app 形态已整体移除（皮肤语义裁定：皮肤只给当前应用换观感，不存在
/// 「打开一个独立应用」的形态）——`/skins/{id}/app[/…]` 必须全部 404。
#[tokio::test]
async fn app_form_routes_are_gone() {
    let tmp = tempdir();
    write_skin(tmp.path(), "buddy", "skin/x.css");
    let a = app(Some(tmp.path().to_str().unwrap()), "buddy");
    for gone in [
        "/skins/buddy/app",
        "/skins/buddy/app/",
        "/skins/buddy/app/assets/app.js",
    ] {
        let res = a
            .clone()
            .oneshot(Request::builder().uri(gone).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), 404, "app route must be gone: {gone}");
    }
}

// --- helpers ---

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}
