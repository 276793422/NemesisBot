//! skins 模块测试（内联纪律：生产文件只留 `mod tests;` 声明）。

use super::*;
use axum::body::Body;
use axum::http::Request;
use tower::ServiceExt;

/// 建一个临时 skins 目录，写入 `id + ".nbskin"` 包（manifest 按给定载荷
/// 组合拼装：entry = CSS 载荷，structure = 结构载荷，任一可缺）。
fn write_skin(dir: &std::path::Path, id: &str, entry: Option<&str>, structure: Option<&str>) {
    use std::io::Write;
    let path = dir.join(format!("{id}.nbskin"));
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    zip.add_directory("skin", zip::write::SimpleFileOptions::default())
        .unwrap();
    let mut manifest = format!(r#"{{"id":"{id}","version":"0.1.0""#);
    if let Some(e) = entry {
        manifest.push_str(&format!(r#","entry":"{e}""#));
    }
    if let Some(s) = structure {
        manifest.push_str(&format!(r#","structure":"{s}""#));
    }
    manifest.push('}');
    zip.start_file("manifest.json", zip::write::SimpleFileOptions::default())
        .unwrap();
    write!(zip, "{manifest}").unwrap();
    if let Some(e) = entry {
        zip.start_file(e, zip::write::SimpleFileOptions::default())
            .unwrap();
        write!(zip, "html[data-skin=\"{id}\"] {{ --accent: #00C29A; }}").unwrap();
    }
    if let Some(s) = structure {
        zip.start_file(s, zip::write::SimpleFileOptions::default())
            .unwrap();
        write!(zip, "<template data-nb-slot=\"titlebar\" data-nb-engine=\"1\"><b data-nb-bind=\"brand\"></b></template>").unwrap();
    }
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
    write_skin(tmp.path(), "openlikebuddy", Some("skin/openlikebuddy.css"), None);
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
    write_skin(tmp.path(), "alpha", Some("skin/x.css"), None);
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
    write_skin(tmp.path(), "openlikebuddy", Some("skin/x.css"), None);
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
    write_skin(tmp.path(), "buddy", Some("skin/x.css"), None);
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

// --- structure 载荷（v2 结构引擎数据面）---

#[tokio::test]
async fn active_structure_serves_manifest_structure() {
    let tmp = tempdir();
    write_skin(
        tmp.path(),
        "buddy",
        Some("skin/x.css"),
        Some("skin/structure.html"),
    );
    let res = app(Some(tmp.path().to_str().unwrap()), "buddy")
        .oneshot(
            Request::builder()
                .uri("/skins/active/structure")
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
    assert_eq!(res.headers().get("x-skin-id").unwrap(), "buddy");
    assert_eq!(
        res.headers().get(header::CACHE_CONTROL).unwrap(),
        "no-cache"
    );
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(text.contains("data-nb-slot=\"titlebar\""));
}

#[tokio::test]
async fn structure_endpoints_missing_cases_are_404() {
    let tmp = tempdir();
    // 纯 CSS 包（无 structure 字段）→ structure 端点 404（CSS 端点照常）
    write_skin(tmp.path(), "csstheme", Some("skin/x.css"), None);
    // structure-only 包（无 entry）→ CSS 端点 404（structure 照常）
    write_skin(tmp.path(), "structonly", None, Some("skin/structure.html"));
    let a = app(Some(tmp.path().to_str().unwrap()), "structonly");

    let css_of_css = a
        .clone()
        .oneshot(
            Request::builder()
                .uri("/skins/csstheme")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(css_of_css.status(), 200);
    let struct_of_css = a
        .clone()
        .oneshot(
            Request::builder()
                .uri("/skins/csstheme/structure")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(struct_of_css.status(), 404);

    let struct_of_only = a
        .clone()
        .oneshot(
            Request::builder()
                .uri("/skins/structonly/structure")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(struct_of_only.status(), 200);
    let css_of_only = a
        .clone()
        .oneshot(
            Request::builder()
                .uri("/skins/structonly.css")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(css_of_only.status(), 404);

    // active id 指向缺失包 → structure 端点同 CSS 端点诚实 404
    let miss = a
        .oneshot(
            Request::builder()
                .uri("/skins/ghost/structure")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(miss.status(), 404);
}

#[tokio::test]
async fn structure_path_traversal_and_bad_manifest_rejected() {
    let tmp = tempdir();
    // manifest structure 带 `..` 穿越 → load 拒绝 → 404
    write_skin(
        tmp.path(),
        "evil",
        Some("skin/x.css"),
        Some("../outside/structure.html"),
    );
    let res = app(Some(tmp.path().to_str().unwrap()), "evil")
        .oneshot(
            Request::builder()
                .uri("/skins/evil/structure")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
}

/// 旧包 manifest 无 structure 字段 → serde 缺省 None（管理面序列化不炸，
/// set_active 裁决回退 entry 有无——旧行为零迁移）。
#[test]
fn manifest_structure_defaults_to_none() {
    let m: ManifestInfo =
        serde_json::from_str(r#"{"id":"old","entry":"skin/x.css"}"#).unwrap();
    assert_eq!(m.entry.as_deref(), Some("skin/x.css"));
    assert!(m.structure.is_none());
}

// --- helpers ---

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}
