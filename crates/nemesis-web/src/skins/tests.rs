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
        None,
    )
    .unwrap()
}

/// 带 home 的路由（脚本闸用例；home 指向预写 config.json 的临时目录）。
fn app_home(dir: Option<&str>, active: &str, home: &str) -> axum::Router {
    super::skin_router(
        dir.map(str::to_string),
        std::sync::Arc::new(parking_lot::RwLock::new(active.to_string())),
        Some(home.to_string()),
    )
    .unwrap()
}

#[tokio::test]
async fn active_css_serves_manifest_entry() {
    let tmp = tempdir();
    write_skin(
        tmp.path(),
        "openlikebuddy",
        Some("skin/openlikebuddy.css"),
        None,
    );
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
            std::sync::Arc::new(parking_lot::RwLock::new("openlikebuddy".into())),
            None
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

/// 旧包 manifest 无 structure / script 字段 → serde 缺省 None（管理面
/// 序列化不炸，set_active 裁决回退 entry 有无——旧行为零迁移）。
#[test]
fn manifest_structure_defaults_to_none() {
    let m: ManifestInfo = serde_json::from_str(r#"{"id":"old","entry":"skin/x.css"}"#).unwrap();
    assert_eq!(m.entry.as_deref(), Some("skin/x.css"));
    assert!(m.structure.is_none());
    assert!(m.script.is_none());
}

// --- 脚本载荷（P2a 数据面；裁决序 ① 404 无脚本 → ② 403 闸关 → ③ 200）---

/// 内存构建 script 形态 .nbskin（theme 载荷 + 可选 script 载荷）。
fn write_script_skin(dir: &std::path::Path, id: &str, script: Option<&str>) {
    use std::io::Write;
    let path = dir.join(format!("{id}.nbskin"));
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let mut manifest = format!(r#"{{"id":"{id}","version":"1.0.0","entry":"skin/x.css""#);
    if let Some(s) = script {
        manifest.push_str(&format!(r#","script":"{s}""#));
    }
    manifest.push('}');
    zip.start_file("manifest.json", zip::write::SimpleFileOptions::default())
        .unwrap();
    write!(zip, "{manifest}").unwrap();
    zip.start_file("skin/x.css", zip::write::SimpleFileOptions::default())
        .unwrap();
    write!(zip, "html {{ --x: 1; }}").unwrap();
    if let Some(s) = script {
        zip.start_file(s, zip::write::SimpleFileOptions::default())
            .unwrap();
        write!(
            zip,
            "document.documentElement.dataset.nbScriptSkin = '{id}';"
        )
        .unwrap();
    }
    zip.finish().unwrap();
}

/// 预写 config.json 的 ui.skins.allow_scripts（脚本闸用例；disk-only，
/// 不触全局 store——nemesis-web 测试进程无人装 ConfigStore）。
fn seed_allow_scripts(home: &std::path::Path, allow: bool) {
    let mut cfg = nemesis_config::Config::default();
    cfg.ui = Some(nemesis_config::UiConfig {
        skin: "default".into(),
        skins: nemesis_config::SkinsPolicy {
            require_signed: false,
            allow_scripts: allow,
        },
    });
    nemesis_config::save_config(&home.join("config.json"), &mut cfg).expect("seed config");
}

/// 裁决序矩阵：① 无 script 字段 → 404（闸开也 404）；② script 在场 +
/// 闸关 → 403；③ script 在场 + 闸开 → 200 text/javascript + X-Skin-Id。
/// ③ 用未签包（测试进程无锚 → Unverified）钉死裁定 1：开关 + 同意是
/// 唯一授权，签名状态不构成加载闸。
#[tokio::test]
async fn script_endpoint_ruling_order_404_403_200() {
    let tmp = tempdir();
    let skins = tmp.path().join("skins");
    std::fs::create_dir(&skins).unwrap();
    write_script_skin(&skins, "plain", None); // 无脚本
    write_script_skin(&skins, "scripty", Some("skin/main.js")); // 带脚本
    let home = tempdir();
    let home_str = home.path().to_str().unwrap();
    let skins_str = skins.to_str().unwrap();

    // ① 无脚本 + 闸开 → 404（不是 200/403——无载荷无执行面）
    seed_allow_scripts(home.path(), true);
    let a = app_home(Some(skins_str), "plain", home_str);
    let res = a
        .oneshot(
            Request::builder()
                .uri("/skins/active/script")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), 404, "无 script 字段必须 404（闸开也一样）");

    // ② 带脚本 + 闸关 → 403（≠ 404：包确实带脚本，是授权面拒绝）
    seed_allow_scripts(home.path(), false);
    let a = app_home(Some(skins_str), "scripty", home_str);
    let res = a
        .clone()
        .oneshot(
            Request::builder()
                .uri("/skins/active/script")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), 403, "闸关必须 403");

    // ③ 带脚本 + 闸开 → 200 text/javascript + X-Skin-Id（未签包照发）
    seed_allow_scripts(home.path(), true);
    let res = a
        .oneshot(
            Request::builder()
                .uri("/skins/active/script")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "裁定 1：开关在场即放行（签名只是徽标）");
    assert_eq!(
        res.headers().get(header::CONTENT_TYPE).unwrap(),
        "text/javascript; charset=utf-8"
    );
    assert_eq!(res.headers().get("x-skin-id").unwrap(), "scripty");
    assert_eq!(
        res.headers().get(header::CACHE_CONTROL).unwrap(),
        "no-cache"
    );
    // X-Skin-Sha256 = 整包摘要（内容身份，前端同意缓存盖章材料；与
    // 管理面 SkinEntry.sha256 同源——64 hex）。
    let sha_hdr = res
        .headers()
        .get("x-skin-sha256")
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(sha_hdr.len(), 64, "sha256 header = 64 hex");
    let want_sha: [u8; 32] =
        Sha256::digest(std::fs::read(skins.join("scripty.nbskin")).unwrap()).into();
    assert_eq!(sha_hdr, hex32(&want_sha), "与整包字节摘要同源");
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(text.contains("nbScriptSkin"), "脚本字节原样分发");
}

/// 显式 id 路径（`?skin=` 预览同源）同裁决序 + 缺包 404 + 穿越 script
/// 路径 404；home 未装配（None）= fail-closed 403（闸不可判 ≠ 闸开）。
#[tokio::test]
async fn script_explicit_id_gated_and_fail_closed() {
    let tmp = tempdir();
    let skins = tmp.path().join("skins");
    std::fs::create_dir(&skins).unwrap();
    write_script_skin(&skins, "scripty", Some("skin/main.js"));
    write_script_skin(&skins, "evil", Some("../outside/main.js"));
    let home = tempdir();
    seed_allow_scripts(home.path(), true);
    let home_str = home.path().to_str().unwrap();
    let skins_str = skins.to_str().unwrap();

    let a = app_home(Some(skins_str), "scripty", home_str);
    // 显式 id + 闸开 → 200（预览路径同源放行）
    let res = a
        .clone()
        .oneshot(
            Request::builder()
                .uri("/skins/scripty/script")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    // 穿越路径（manifest.script 带 ..）→ load 拒绝 → 404
    let res = a
        .clone()
        .oneshot(
            Request::builder()
                .uri("/skins/evil/script")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
    // 缺包 → 404
    let res = a
        .oneshot(
            Request::builder()
                .uri("/skins/ghost/script")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), 404);

    // 带脚本 + 闸开 config 但 home 未装配 → fail-closed 403
    let no_home = app(Some(skins_str), "scripty");
    let res = no_home
        .oneshot(
            Request::builder()
                .uri("/skins/scripty/script")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), 403, "home 缺失 = 闸不可判 = fail-closed 403");
}

/// 管理面呈现：has_script 跟随 manifest.script（在场 true / 缺省 false），
/// 与签名状态正交。
#[test]
fn scan_skins_has_script_flag_follows_manifest() {
    let tmp = tempdir();
    write_script_skin(tmp.path(), "plain", None);
    write_script_skin(tmp.path(), "scripty", Some("skin/main.js"));
    let entries = scan_skins(tmp.path().to_str().unwrap());
    assert_eq!(entries.len(), 2);
    let plain = entries.iter().find(|e| e.id == "plain").unwrap();
    assert!(!plain.has_script);
    assert!(plain.manifest.script.is_none());
    let scripty = entries.iter().find(|e| e.id == "scripty").unwrap();
    assert!(scripty.has_script);
    assert_eq!(scripty.manifest.script.as_deref(), Some("skin/main.js"));
}

// --- helpers ---

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

// ===== P2 渠道安装 + P3 吊销第五态 + CSS url() 消毒（2026-09-27）=====

/// 锚 env 串行锁：`NEMESIS_ROOT_ANCHOR` 是进程全局，hooks 测试（本文件）
/// 与 handlers 测试（`handlers/skins/tests.rs`）经
/// `crate::skins::tests::ANCHOR_ENV_LOCK` 共用同一把——锚在场的断言窗口
/// 内禁止其它测试并发读锚（resolve_anchors 在 install/scan 内部读 env）。
pub(crate) static ANCHOR_ENV_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// RAII：设定/摘除 `NEMESIS_ROOT_ANCHOR`，drop 恢复原值（panic 展开也恢复）。
/// 测试构建下 `NEMESIS_BUILD_ROOT_ANCHOR` 缺省 → resolve_anchors 走运行时
/// env 回退，故本 guard 能确定性地控制锚在场/不在场。
pub(crate) struct AnchorEnvGuard(Option<String>);
impl AnchorEnvGuard {
    fn set(v: Option<String>) -> Self {
        let old = std::env::var("NEMESIS_ROOT_ANCHOR").ok();
        // SAFETY：调用方持 ANCHOR_ENV_LOCK；本 crate 测试进程无其它 env 写者
        //（标准库对此 API 的单线程约定按测试纪律满足）。
        unsafe {
            match &v {
                Some(hex) => std::env::set_var("NEMESIS_ROOT_ANCHOR", hex),
                None => std::env::remove_var("NEMESIS_ROOT_ANCHOR"),
            }
        }
        Self(old)
    }
}
impl Drop for AnchorEnvGuard {
    fn drop(&mut self) {
        // SAFETY：同上——持锁方（guard 的所有者）独占 env 写窗口。
        unsafe {
            match &self.0 {
                Some(v) => std::env::set_var("NEMESIS_ROOT_ANCHOR", v),
                None => std::env::remove_var("NEMESIS_ROOT_ANCHOR"),
            }
        }
    }
}

/// [u8; 32] → 64 字符 hex（锚 env 值形态，resolve_root_anchors 同款解析）。
pub(crate) fn hex32(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 内存构建 theme 形态 .nbskin 原始字节（未签；handlers 测试复用）。
pub(crate) fn theme_zip_bytes(id: &str) -> Vec<u8> {
    use std::io::Write;
    let buf = std::io::Cursor::new(Vec::new());
    let mut zip = zip::ZipWriter::new(buf);
    zip.start_file("manifest.json", zip::write::SimpleFileOptions::default())
        .unwrap();
    write!(
        zip,
        r#"{{"id":"{id}","name":"{id}","version":"1.0.0","type":"theme","entry":"skin/main.css"}}"#
    )
    .unwrap();
    zip.start_file(
        "skin/main.css",
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
    )
    .unwrap();
    write!(zip, "html {{ --accent: teal; }}").unwrap();
    let cursor = zip.finish().expect("zip finish");
    cursor.into_inner()
}

/// 测试密钥链：keygen 现签三件套（root_sk 供 CRL 根签，leaf_sk 供包签）。
struct TestChain(nemesis_verify::keygen::KeyHierarchy);
impl TestChain {
    fn new(now: u64) -> Self {
        Self(nemesis_verify::keygen::generate_at(now).expect("keygen"))
    }
    fn sign(&self, raw: &[u8], now: u64, id: &str) -> Vec<u8> {
        nemesis_verify::verify::sign_content_v4(
            raw,
            &self.0.leaf_sk,
            now,
            &self.0.chain(),
            Some(id),
        )
        .expect("sign")
    }
    fn anchor_hex(&self) -> String {
        hex32(&self.0.root_anchor_fingerprint())
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

// --- P2 install_bytes ---

/// 物理闸：非 ZIP / 超限 / manifest.id 缺失或非法 → 拒收且零落盘。
#[test]
fn install_bytes_rejects_garbage_oversize_and_bad_id() {
    let _lock = ANCHOR_ENV_LOCK.lock();
    let _no_anchor = AnchorEnvGuard::set(None);
    let dir = tempdir();
    let skins = dir.path().join("skins");

    // ① 非 ZIP 垃圾 → 物理不可服务
    let err = install_bytes(skins.to_str().unwrap(), b"not a zip at all", false).unwrap_err();
    assert!(err.contains("物理不可服务"), "{err}");

    // ② 超限（大小闸在 ZIP 解析之前，25MB+1 零向量即可）
    let big = vec![0u8; SKIN_PACKAGE_MAX_BYTES + 1];
    let err = install_bytes(skins.to_str().unwrap(), &big, false).unwrap_err();
    assert!(err.contains("上限"), "{err}");

    // ③ manifest.id 缺失 → 拒（产出物必须可路由寻址）
    use std::io::Write as _;
    let buf = std::io::Cursor::new(Vec::new());
    let mut zip = zip::ZipWriter::new(buf);
    zip.start_file("manifest.json", zip::write::SimpleFileOptions::default())
        .unwrap();
    write!(zip, r#"{{"version":"1.0.0","entry":"skin/x.css"}}"#).unwrap();
    let no_id = zip.finish().expect("zip finish").into_inner();
    let err = install_bytes(skins.to_str().unwrap(), &no_id, false).unwrap_err();
    assert!(err.contains("manifest.id"), "{err}");

    // ④ id = "default"（保留字）→ 拒
    let err =
        install_bytes(skins.to_str().unwrap(), &theme_zip_bytes("default"), false).unwrap_err();
    assert!(err.contains("default"), "{err}");

    // 全程零落盘
    let landed = std::fs::read_dir(&skins).map(|rd| rd.count()).unwrap_or(0);
    assert_eq!(landed, 0, "物理闸拒收路径不得产生任何文件");
}

/// 无锚环境：未签包 → ❔ Unverified 落盘（下载永远落盘；徽标诚实），
/// sha256 对账，落盘后 scan_skins 可见。
#[test]
fn install_bytes_unsigned_lands_unverified_without_anchor() {
    let _lock = ANCHOR_ENV_LOCK.lock();
    let _no_anchor = AnchorEnvGuard::set(None);
    let dir = tempdir();
    let skins = dir.path().join("skins");
    let bytes = theme_zip_bytes("alpha");

    let out = install_bytes(skins.to_str().unwrap(), &bytes, false).expect("unsigned lands");
    assert_eq!(out.id, "alpha");
    assert_eq!(out.file, "alpha.nbskin");
    assert_eq!(out.signature, SkinSignature::Unverified);
    assert!(!out.overwritten);
    let want_sha: [u8; 32] = Sha256::digest(&bytes).into();
    assert_eq!(out.sha256, hex32(&want_sha));

    let entries = scan_skins(skins.to_str().unwrap());
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].signature, SkinSignature::Unverified);
    assert_eq!(entries[0].status, SkinStatus::Ok);
}

/// 重名闸：默认拒绝；显式 overwrite 才覆盖旧包（内容随之更新）。
#[test]
fn install_bytes_duplicate_requires_explicit_overwrite() {
    let _lock = ANCHOR_ENV_LOCK.lock();
    let _no_anchor = AnchorEnvGuard::set(None);
    let dir = tempdir();
    let skins = dir.path().join("skins");
    install_bytes(skins.to_str().unwrap(), &theme_zip_bytes("alpha"), false)
        .expect("first install");

    let err = install_bytes(skins.to_str().unwrap(), &theme_zip_bytes("alpha"), false).unwrap_err();
    assert!(err.contains("已存在") && err.contains("overwrite"), "{err}");

    let out = install_bytes(skins.to_str().unwrap(), &theme_zip_bytes("alpha"), true)
        .expect("overwrite lands");
    assert!(out.overwritten);
    assert_eq!(scan_skins(skins.to_str().unwrap()).len(), 1, "覆盖不是新增");
}

/// 锚在场四态全落盘：signed → ✅ Verified；篡改 → 🔴 Invalid（含 outcome
/// 原文）**照常落盘**（信任闸单点在 set_active，渠道只产徽标）；未签 → ⚪。
#[test]
fn install_bytes_anchor_present_all_states_land() {
    let _lock = ANCHOR_ENV_LOCK.lock();
    let now = now_secs();
    let chain = TestChain::new(now);
    let _anchor = AnchorEnvGuard::set(Some(chain.anchor_hex()));
    let dir = tempdir();
    let skins = dir.path().join("skins");

    // ✅ Valid → Verified
    let signed = chain.sign(&theme_zip_bytes("good"), now, "good-pub");
    let out = install_bytes(skins.to_str().unwrap(), &signed, false).expect("signed lands");
    assert_eq!(out.signature, SkinSignature::Verified);
    assert!(out.sig_detail.is_none());

    // 🔴 Tampered → Invalid + 原文，且照常落盘。篡改点选 CSS 内容区
    //（"teal" 标记内翻一位）：ZIP 结构完好过物理闸，摘要失配出 Tampered
    //——魔数/目录级破坏会在物理闸被拒（那是「不是皮肤包」，非信任结论）。
    let mut tampered = chain.sign(&theme_zip_bytes("bad"), now, "bad-pub");
    let marker = tampered
        .windows(4)
        .rposition(|w| w == b"teal")
        .expect("css content marker present");
    tampered[marker] ^= 0x20;
    let out = install_bytes(skins.to_str().unwrap(), &tampered, false).expect("tampered 也落盘");
    assert_eq!(out.signature, SkinSignature::Invalid);
    assert!(out.sig_detail.as_deref().unwrap().contains("Tampered"));
    assert!(skins.join("bad.nbskin").is_file(), "信任结论不拦落盘");

    // ⚪ NoSignature → Unsigned（锚在场时的未签包结论）
    let out =
        install_bytes(skins.to_str().unwrap(), &theme_zip_bytes("plain"), false).expect("lands");
    assert_eq!(out.signature, SkinSignature::Unsigned);

    let entries = scan_skins(skins.to_str().unwrap());
    assert_eq!(entries.len(), 3, "三态全部进目录注册表");
}

// --- P3 吊销第五态 ---

/// 快照命中 → 🚫 Revoked；FileHash 维 = 单文件吊销（同钥的 donor 不受
/// 牵连），key_fp 维 = 钥级双杀；删快照 → 回四态；管理面
/// crl_snapshot_info 三相（absent/verified/entries）对账。
#[test]
fn crl_snapshot_fifth_state_hit_and_recover() {
    let _lock = ANCHOR_ENV_LOCK.lock();
    let now = now_secs();
    let chain = TestChain::new(now);
    let _anchor = AnchorEnvGuard::set(Some(chain.anchor_hex()));
    let dir = tempdir();
    let skins = dir.path().join("skins");
    std::fs::create_dir(&skins).unwrap();
    let alpha = chain.sign(&theme_zip_bytes("alpha"), now, "alpha-pub");
    let beta = chain.sign(&theme_zip_bytes("beta"), now, "beta-pub");
    // 吊销维度原料（在 beta 被条目查找遮蔽前提取）
    let beta_meta = nemesis_verify::revocation::revocation_meta(&beta);
    std::fs::write(skins.join("alpha.nbskin"), &alpha).unwrap();
    std::fs::write(skins.join("beta.nbskin"), &beta).unwrap();

    // 快照不在场：info 全默认 + 四态不变
    let entries = scan_skins(skins.to_str().unwrap());
    assert!(
        entries
            .iter()
            .all(|e| e.signature == SkinSignature::Verified),
        "基线：两包都 Verified"
    );
    let info = crl_snapshot_info(skins.to_str().unwrap(), &resolve_anchors(), &entries);
    assert!(!info.present);
    assert!(!info.verified);

    let mk_crl = |dim, value: String, version: u64| nemesis_verify::Crl {
        version,
        valid_until: u64::MAX,
        entries: vec![nemesis_verify::CrlEntry {
            dim,
            value,
            revoked_at: now,
            reason: "leak".into(),
        }],
    };
    let write_snap = |skins: &std::path::Path, crl: &nemesis_verify::Crl| {
        let snap = nemesis_verify::sign_response(crl, &chain.0.root_sk).expect("root-sign crl");
        std::fs::write(skins.join("crl.pem"), serde_json::to_string(&snap).unwrap()).unwrap();
    };

    // ① FileHash 维吊销 beta（单文件）：同钥签的 alpha 不受牵连
    let beta_content_hash = beta_meta
        .content_hash
        .expect("signed pkg exposes content_hash");
    write_snap(
        &skins,
        &mk_crl(nemesis_verify::RevDim::FileHash, beta_content_hash, 1),
    );
    let entries = scan_skins(skins.to_str().unwrap());
    let beta = entries.iter().find(|e| e.id == "beta").unwrap();
    assert_eq!(beta.signature, SkinSignature::Revoked, "命中 → 🚫");
    assert!(beta.sig_detail.as_deref().unwrap().contains("FileHash"));
    let alpha = entries.iter().find(|e| e.id == "alpha").unwrap();
    assert_eq!(alpha.signature, SkinSignature::Verified, "同钥异文件不连坐");

    let info = crl_snapshot_info(skins.to_str().unwrap(), &resolve_anchors(), &entries);
    assert!(info.present && info.verified);
    assert_eq!(info.entries, 1);
    assert_eq!(info.version, 1);

    // ② key_fp 维吊销（钥级）：同一把 leaf 签的两包全 🚫
    let beta_key_fp = beta_meta.key_fp.expect("signed pkg exposes key_fp");
    write_snap(
        &skins,
        &mk_crl(nemesis_verify::RevDim::KeyFp, beta_key_fp, 2),
    );
    let entries = scan_skins(skins.to_str().unwrap());
    assert!(
        entries
            .iter()
            .all(|e| e.signature == SkinSignature::Revoked),
        "钥级吊销 = 双杀"
    );

    // 删快照 → 回四态（离线诚实，不残留第五态）
    std::fs::remove_file(skins.join("crl.pem")).unwrap();
    let entries = scan_skins(skins.to_str().unwrap());
    assert!(
        entries
            .iter()
            .all(|e| e.signature == SkinSignature::Verified)
    );
}

/// 快照验不过（换假根签）→ 诚实注记不应用；快照过期 → expired 注记，
/// 吊销同样不生效（离线模式不猜）。
#[test]
fn crl_snapshot_unverifiable_or_expired_not_applied() {
    let _lock = ANCHOR_ENV_LOCK.lock();
    let now = now_secs();
    let chain = TestChain::new(now);
    let _anchor = AnchorEnvGuard::set(Some(chain.anchor_hex()));
    let dir = tempdir();
    let skins = dir.path().join("skins");
    std::fs::create_dir(&skins).unwrap();
    let beta = chain.sign(&theme_zip_bytes("beta"), now, "beta-pub");
    std::fs::write(skins.join("beta.nbskin"), &beta).unwrap();
    let beta_key_fp = nemesis_verify::revocation::revocation_meta(&beta)
        .key_fp
        .unwrap();

    // ① 假根签的快照 → 验签失败注记，吊销不应用
    let stranger = TestChain::new(now - 10);
    let crl = nemesis_verify::Crl {
        version: 1,
        valid_until: u64::MAX,
        entries: vec![nemesis_verify::CrlEntry {
            dim: nemesis_verify::RevDim::KeyFp,
            value: beta_key_fp.clone(),
            revoked_at: now,
            reason: "spoof".into(),
        }],
    };
    let spoofed = nemesis_verify::sign_response(&crl, &stranger.0.root_sk).unwrap();
    std::fs::write(
        skins.join("crl.pem"),
        serde_json::to_string(&spoofed).unwrap(),
    )
    .unwrap();
    let entries = scan_skins(skins.to_str().unwrap());
    assert_eq!(
        entries[0].signature,
        SkinSignature::Verified,
        "假快照不应用"
    );
    let info = crl_snapshot_info(skins.to_str().unwrap(), &resolve_anchors(), &entries);
    assert!(info.present && !info.verified);
    assert!(
        info.note.as_deref().unwrap().contains("验签失败"),
        "{info:?}"
    );

    // ② 过期快照（真根签但 valid_until 已过）→ expired 注记，不应用
    let expired_crl = nemesis_verify::Crl {
        version: 2,
        valid_until: now.saturating_sub(1),
        entries: vec![nemesis_verify::CrlEntry {
            dim: nemesis_verify::RevDim::KeyFp,
            value: beta_key_fp,
            revoked_at: now,
            reason: "late".into(),
        }],
    };
    let snap = nemesis_verify::sign_response(&expired_crl, &chain.0.root_sk).unwrap();
    std::fs::write(skins.join("crl.pem"), serde_json::to_string(&snap).unwrap()).unwrap();
    let entries = scan_skins(skins.to_str().unwrap());
    assert_eq!(entries[0].signature, SkinSignature::Verified, "过期不应用");
    let info = crl_snapshot_info(skins.to_str().unwrap(), &resolve_anchors(), &entries);
    assert!(info.present && info.verified && info.expired);
    assert!(info.note.as_deref().unwrap().contains("过期"), "{info:?}");
}

/// 没有可信签名包（donor 缺席）时快照自然无从生效——无 donor 路径不 panic、
/// 徽标维持原态（「吊销只对签名有意义」同构性）。
#[test]
fn crl_snapshot_without_verified_donor_is_inert() {
    let _lock = ANCHOR_ENV_LOCK.lock();
    let now = now_secs();
    let chain = TestChain::new(now);
    let _anchor = AnchorEnvGuard::set(Some(chain.anchor_hex()));
    let dir = tempdir();
    let skins = dir.path().join("skins");
    std::fs::create_dir(&skins).unwrap();
    // 未签包（非 Verified donor）+ 真根签快照在场
    std::fs::write(skins.join("plain.nbskin"), theme_zip_bytes("plain")).unwrap();
    let crl = nemesis_verify::Crl {
        version: 1,
        valid_until: u64::MAX,
        entries: vec![],
    };
    let snap = nemesis_verify::sign_response(&crl, &chain.0.root_sk).unwrap();
    std::fs::write(skins.join("crl.pem"), serde_json::to_string(&snap).unwrap()).unwrap();

    let entries = scan_skins(skins.to_str().unwrap());
    assert_eq!(entries[0].signature, SkinSignature::Unsigned);
    let info = crl_snapshot_info(skins.to_str().unwrap(), &resolve_anchors(), &entries);
    assert!(info.present && !info.verified);
    assert!(
        info.note.as_deref().unwrap().contains("donor")
            || info.note.as_deref().unwrap().contains("公钥"),
        "{info:?}"
    );
}

// --- CSS url() 外链消毒 ---

/// 外链（https/http/协议相对，含引号形态与大小写）→ about:blank；栅格
/// data: 放行、SVG data: 拦；相对路径放行；畸形未闭合原样照抄。
#[test]
fn sanitize_css_urls_neuters_external_and_keeps_safe() {
    // 外链三前缀（无引号）
    assert_eq!(
        sanitize_css_urls("a{background:url(https://evil.com/x.png)}"),
        "a{background:url(about:blank)}"
    );
    assert_eq!(
        sanitize_css_urls("a{background:url(http://evil.com/x.png)}"),
        "a{background:url(about:blank)}"
    );
    assert_eq!(
        sanitize_css_urls("a{background:url(//evil.com/x.png)}"),
        "a{background:url(about:blank)}"
    );
    // 引号形态（输出保留原引号结构）
    assert_eq!(
        sanitize_css_urls("a{background:url('http://evil.com/x.png')}"),
        "a{background:url('about:blank')}"
    );
    assert_eq!(
        sanitize_css_urls("a{background:url(\"//cdn.example/x.png\")}"),
        "a{background:url(\"about:blank\")}"
    );
    // 大小写不敏感（URL/协议大写也拦）
    assert_eq!(
        sanitize_css_urls("a{background:url(HTTP://EVIL.COM/x)}"),
        "a{background:url(about:blank)}"
    );
    // 栅格 data: 放行（跟踪信标豁免面 = 位图素材）
    assert!(
        sanitize_css_urls("a{background:url(data:image/png;base64,AAAA)}")
            .contains("data:image/png;base64,AAAA")
    );
    // SVG data: 拦（可携带脚本/外链）
    assert_eq!(
        sanitize_css_urls("a{background:url(data:image/svg+xml;base64,AAAA)}"),
        "a{background:url(about:blank)}"
    );
    // 相对/绝对路径放行（皮肤包内素材）
    assert!(sanitize_css_urls("a{background:url(skin/bg.png)}").contains("skin/bg.png"));
    assert!(sanitize_css_urls("a{background:url(./bg.png)}").contains("./bg.png"));
    // 畸形（token 未闭合）→ 剩余原样照抄，不 panic
    assert_eq!(
        sanitize_css_urls("a{background:url(https://evil.com/x"),
        "a{background:url(https://evil.com/x"
    );
    // 空串与无 url() 的 CSS 原样通过
    assert_eq!(sanitize_css_urls(""), "");
    assert_eq!(sanitize_css_urls("html{--x:1}"), "html{--x:1}");
}

/// 2026-09-28 加固 corpus：`@import` 字符串形态 + CSS 转义两大绕过族。
/// `@import "https://…"` 不含 `url(` 子串、`\75rl(` 解码后才是 url(——
/// 字面扫描均漏（BUG 2026-09-28），解码后扫描 + import 串处理补齐。
#[test]
fn sanitize_css_urls_import_and_escape_bypasses_neutered() {
    // @import 字符串形态（单/双引号、大小写、协议相对）
    assert_eq!(
        sanitize_css_urls(r#"@import "https://evil.com/x.css";"#),
        r#"@import "about:blank";"#
    );
    assert_eq!(
        sanitize_css_urls("@import 'http://evil.com/x.css';"),
        "@import 'about:blank';"
    );
    assert_eq!(
        sanitize_css_urls(r#"@IMPORT "https://evil.com/x.css";"#),
        r#"@IMPORT "about:blank";"#
    );
    assert_eq!(
        sanitize_css_urls(r#"@import "//evil.com/x.css";"#),
        r#"@import "about:blank";"#
    );
    // @import 合法本地目标放行；url( 形态由 url( 分支处理
    assert!(sanitize_css_urls(r#"@import "skin/base.css";"#).contains("skin/base.css"));
    assert_eq!(
        sanitize_css_urls("@import url(https://evil.com/x.css);"),
        "@import url(about:blank);"
    );
    // @importer 等普通标识符不误命中
    assert_eq!(sanitize_css_urls("a{--x:@importer}"), "a{--x:@importer}");

    // CSS 转义 function 名：\75rl( 解码后 = url( → 拦
    assert_eq!(
        sanitize_css_urls(r"a{background:\75rl(http://evil.com/x.png)}"),
        "a{background:url(about:blank)}"
    );
    // 转义带空白终结符：u\72 l( → url(
    assert_eq!(
        sanitize_css_urls(r"a{background:u\72 l(http://evil.com/x.png)}"),
        "a{background:url(about:blank)}"
    );
    // 转义藏在目标串里：\68 ttp:// → http:// → 拦
    assert_eq!(
        sanitize_css_urls(r"a{background:url(\68 ttp://evil.com/x.png)}"),
        "a{background:url(about:blank)}"
    );

    // 双重解码走私防线：\5c 75rl( 解码一轮 = \75rl(（非 url(，不拦），
    // 但输出必须把字面 \ 再转义成 \\，浏览器对输出解码后仍非 url(
    assert_eq!(
        sanitize_css_urls(r"a{background:\5c 75rl(http://evil.com/x.png)}"),
        r"a{background:\\75rl(http://evil.com/x.png)}"
    );

    // 合法转义 CSS 语义等价透传（解码形态字节变化不改变渲染）
    assert_eq!(
        sanitize_css_urls(r#"a{content:"\201C"}"#),
        "a{content:\"\u{201C}\"}"
    );
}
