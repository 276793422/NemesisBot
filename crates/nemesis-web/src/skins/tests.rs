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
    super::skin_router(dir.map(str::to_string), active.to_string()).unwrap()
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
    assert!(super::skin_router(None, "openlikebuddy".into()).is_none());
}

/// 建一个 app 形态皮肤包（manifest + app/index.html + app/assets/app.js）。
fn write_app_skin(dir: &std::path::Path, id: &str) {
    use std::io::Write;
    let path = dir.join(format!("{id}.nbskin"));
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file("manifest.json", zip::write::SimpleFileOptions::default())
        .unwrap();
    write!(
        zip,
        r#"{{"id":"{id}","version":"1.0.0","type":"app","entry":"app/index.html"}}"#
    )
    .unwrap();
    zip.start_file("app/index.html", zip::write::SimpleFileOptions::default())
        .unwrap();
    write!(zip, "<!doctype html><html><body>app-{id}</body></html>").unwrap();
    zip.start_file(
        "app/assets/app.js",
        zip::write::SimpleFileOptions::default(),
    )
    .unwrap();
    write!(zip, "console.log(1)").unwrap();
    zip.finish().unwrap();
}

#[tokio::test]
async fn app_index_and_asset_served_with_mime() {
    let tmp = tempdir();
    write_app_skin(tmp.path(), "buddy");
    let a = app(Some(tmp.path().to_str().unwrap()), "buddy");

    // 入口页：无尾斜杠 = 重定向到带尾斜杠形态（`<base href="./">` 以文档
    // 目录为基，无尾斜杠会丢最后一段 → 必须重定向归一；axum Redirect::to = 303）
    let res = a
        .clone()
        .oneshot(
            Request::builder()
                .uri("/skins/buddy/app")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), 303);
    assert_eq!(
        res.headers().get(header::LOCATION).unwrap(),
        "/skins/buddy/app/"
    );

    // 入口页（带尾斜杠）
    let res = a
        .clone()
        .oneshot(
            Request::builder()
                .uri("/skins/buddy/app/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(
        res.headers().get(header::CONTENT_TYPE).unwrap(),
        "text/html; charset=utf-8"
    );

    // 静态资产按扩展名给 Content-Type
    let res = a
        .oneshot(
            Request::builder()
                .uri("/skins/buddy/app/assets/app.js")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(
        res.headers().get(header::CONTENT_TYPE).unwrap(),
        "text/javascript; charset=utf-8"
    );
}

#[tokio::test]
async fn app_traversal_and_missing_rejected() {
    let tmp = tempdir();
    write_app_skin(tmp.path(), "buddy");
    let a = app(Some(tmp.path().to_str().unwrap()), "buddy");
    for bad in [
        "/skins/buddy/app/..%2F..%2Fmanifest.json",
        "/skins/buddy/app/nope.js",
        "/skins/ghost/app/", // 幽灵 id（带尾斜杠直打入口页形态）
    ] {
        let res = a
            .clone()
            .oneshot(Request::builder().uri(bad).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), 404, "must 404: {bad}");
    }
}

// --- helpers ---

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}
