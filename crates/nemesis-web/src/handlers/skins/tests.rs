//! skins WSAPI handler 测试。
//!
//! 覆盖：list/detail 扫描呈现（含 broken 灰卡与目录缺失态）、set_active
//! 全决策路径（happy / default 关皮肤 / 纯 app 拒绝 / 不存在拒绝 /
//! require_signed 闸 / config 写入与锁热翻）、签名管线端到端（keygen 现签
//! → verify_bytes Valid → classify 徽标）。
//!
//! 装配槽 SKINS_SLOT 是进程级单例：全部用例持 SLOT_LOCK 串行（与
//! background_registry TEST_LOCK 同款纪律）。

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::skins::{SkinSignature, SkinStatus, classify_signature, scan_skins};
use crate::ws_router::{ModuleHandler, RequestContext};
use std::io::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

static SLOT_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// L1 注册表契约：module 名 + commands() 四命令单一真相源。
#[test]
fn commands_registry_shape() {
    use crate::ws_router::ModuleHandler as _;
    let h = SkinsHandler::new();
    assert_eq!(h.module_name(), "skins");
    assert_eq!(h.commands(), &["list", "detail", "reload", "set_active"]);
}

/// 路由级 dispatch 契约：ws_router 把 msg.cmd 原样递给 handle_cmd（不拼
/// module 前缀），前端 `request('skins', 'list')` 必须命中裸名臂。此前
/// commands()/match 臂写 `skins.list` 前缀形态，直调方法的单测发现不了
/// 这个不匹配——用本测试钉死。
#[tokio::test]
async fn handle_cmd_dispatches_bare_names_as_router_sends_them() {
    use crate::ws_router::ModuleHandler as _;
    let _guard = SLOT_LOCK.lock();
    let tmp = tempfile::tempdir().unwrap();
    // 空目录即可：list 走通 = 裸名臂命中（前缀形态会落 unknown command 臂）
    set_handle(
        Some(tmp.path().join("skins").to_string_lossy().to_string()),
        Arc::new(parking_lot::RwLock::new("default".into())),
    );
    let h = SkinsHandler::new();
    let ctx = make_ctx(tmp.path());
    let got = h.handle_cmd("list", None, &ctx).await;
    assert!(got.is_ok(), "bare 'list' must dispatch: {:?}", got.err());
    // 未知命令的报错形态仍是裸名回显
    let err = h.handle_cmd("nonexistent", None, &ctx).await.unwrap_err();
    assert_eq!(err, "unknown command: skins.nonexistent");
}

/// 构造最小 RequestContext（home 指向临时目录；state 字段全 None 桩）。
fn make_ctx(home: &std::path::Path) -> RequestContext {
    let ws = home.to_string_lossy().to_string();
    let state = Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: Some(ws.clone()),
        home: Some(ws.clone()),
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new("m".to_string())),
        model_base: Arc::new(parking_lot::Mutex::new(String::new())),
        model_has_key: Arc::new(AtomicBool::new(false)),
        event_hub: Arc::new(EventHub::new()),
        running: Arc::new(AtomicBool::new(true)),
        session_manager: Arc::new(SessionManager::with_default_timeout()),
        inbound_tx: None,
        streaming_provider: None,
        ws_router: None,
        agent_service: None,
        data_store: None,
        memory_manager: None,
        forge: None,
        agent_loop: Arc::new(parking_lot::RwLock::new(None)),
        cluster: None,
        cluster_service: None,
        cluster_log_dir: None,
        workflow_engine: None,
        #[cfg(feature = "workflow")]
        chat_secret_store: std::sync::Arc::new(
            nemesis_workflow::chat_secrets::ChatSecretStore::in_memory(),
        ),
        #[cfg(not(feature = "workflow"))]
        chat_secret_store: std::sync::Arc::new(()),
        #[cfg(feature = "workflow")]
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        #[cfg(not(feature = "workflow"))]
        webhook_rate_limiter: Arc::new(()),
        internal_cmd_tx: None,
        estop: None,
        signature_verify: None,
        cron: None,
        board: None,
    });
    RequestContext {
        session_id: "s".to_string(),
        chat_id: "c".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

/// 写一个 theme 形态 .nbskin（manifest + CSS 载荷），可指定 manifest.id
///（id_mismatch 注记用）。
fn write_theme_skin(dir: &std::path::Path, file_stem: &str, manifest_id: &str) {
    let path = dir.join(format!("{file_stem}.nbskin"));
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file("manifest.json", zip::write::SimpleFileOptions::default())
        .unwrap();
    write!(
        zip,
        r#"{{"id":"{manifest_id}","name":"{manifest_id}","version":"1.0.0","type":"theme","entry":"skin/main.css"}}"#
    )
    .unwrap();
    zip.start_file("skin/main.css", zip::write::SimpleFileOptions::default())
        .unwrap();
    // CSS 内容带 stem 标记：hot_flip 用例以「响应体含当前激活 id」断言热切。
    write!(zip, "/* skin:{file_stem} */ html {{ --accent: teal; }}").unwrap();
    zip.finish().unwrap();
}

/// 写一个无主题载荷的 .nbskin（manifest 有 type=app 字样但无 entry——
/// 皮肤语义裁定后统一按「无观感载荷」拒绝，不再区分形态类型学）。
fn write_app_skin(dir: &std::path::Path, file_stem: &str) {
    let path = dir.join(format!("{file_stem}.nbskin"));
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file("manifest.json", zip::write::SimpleFileOptions::default())
        .unwrap();
    write!(
        zip,
        r#"{{"id":"{file_stem}","version":"1.0.0","type":"app","app":"app"}}"#
    )
    .unwrap();
    zip.start_file("app/index.html", zip::write::SimpleFileOptions::default())
        .unwrap();
    write!(zip, "<!doctype html><html></html>").unwrap();
    zip.finish().unwrap();
}

/// 预写 config.json（可指定 require_signed），供 set_active 装载。
fn seed_config(home: &std::path::Path, require_signed: bool) {
    let mut cfg = nemesis_config::Config::default();
    cfg.ui = Some(nemesis_config::UiConfig {
        skin: "default".into(),
        skins: nemesis_config::SkinsPolicy { require_signed },
    });
    nemesis_config::save_config(&home.join("config.json"), &mut cfg).expect("seed config");
}

fn read_config_skin(home: &std::path::Path) -> String {
    let cfg = nemesis_config::load_config(&home.join("config.json")).expect("reload config");
    cfg.ui.map(|u| u.skin).unwrap_or_default()
}

#[test]
fn list_reports_entries_broken_and_dir_state() {
    let _guard = SLOT_LOCK.lock();
    let tmp = tempfile::tempdir().unwrap();
    let skins = tmp.path().join("skins");
    std::fs::create_dir(&skins).unwrap();
    write_theme_skin(&skins, "alpha", "alpha");
    // broken：写垃圾字节（非 ZIP）
    std::fs::write(skins.join("broken.nbskin"), b"not a zip at all").unwrap();
    // 非 .nbskin 文件不进注册表
    std::fs::write(skins.join("readme.txt"), b"hi").unwrap();

    let active = Arc::new(parking_lot::RwLock::new("default".to_string()));
    set_handle(Some(skins.to_string_lossy().to_string()), active);
    let res = SkinsHandler::new()
        .list()
        .expect("list ok")
        .expect("payload");
    assert_eq!(res["dir_exists"], json!(true));
    let arr = res["skins"].as_array().expect("skins array");
    assert_eq!(arr.len(), 2, "broken 也展示（灰卡），txt 不进");
    let alpha = arr.iter().find(|e| e["id"] == "alpha").unwrap();
    assert_eq!(alpha["status"], json!("ok"));
    assert_eq!(alpha["manifest"]["version"], json!("1.0.0"));
    assert!(alpha["sha256"].as_str().unwrap().len() == 64);
    let broken = arr.iter().find(|e| e["id"] == "broken").unwrap();
    assert_eq!(broken["status"], json!("broken"));
    assert!(broken["status_detail"].as_str().unwrap().contains("ZIP"));
}

#[test]
fn list_without_dir_reports_honest_empty() {
    let _guard = SLOT_LOCK.lock();
    set_handle(None, Arc::new(parking_lot::RwLock::new("default".into())));
    let res = SkinsHandler::new().list().expect("ok").expect("payload");
    assert_eq!(res["dir"], serde_json::Value::Null);
    assert_eq!(res["dir_exists"], json!(false));
    assert_eq!(res["skins"].as_array().unwrap().len(), 0);
}

#[test]
fn list_missing_dir_is_dir_exists_false() {
    let _guard = SLOT_LOCK.lock();
    let tmp = tempfile::tempdir().unwrap();
    set_handle(
        Some(tmp.path().join("nope").to_string_lossy().to_string()),
        Arc::new(parking_lot::RwLock::new("default".into())),
    );
    let res = SkinsHandler::new().list().expect("ok").expect("payload");
    assert_eq!(res["dir_exists"], json!(false));
}

#[test]
fn detail_hits_and_misses() {
    let _guard = SLOT_LOCK.lock();
    let tmp = tempfile::tempdir().unwrap();
    let skins = tmp.path().join("skins");
    std::fs::create_dir(&skins).unwrap();
    write_theme_skin(&skins, "alpha", "alpha");
    set_handle(
        Some(skins.to_string_lossy().to_string()),
        Arc::new(parking_lot::RwLock::new("default".into())),
    );
    let h = SkinsHandler::new();
    let hit = h
        .detail(Some(json!({ "id": "alpha" })))
        .expect("hit")
        .expect("payload");
    assert_eq!(hit["skin"]["id"], json!("alpha"));
    let miss = h.detail(Some(json!({ "id": "ghost" }))).unwrap_err();
    assert!(miss.contains("皮肤不存在"), "{miss}");
    let no_id = h.detail(None).unwrap_err();
    assert!(no_id.contains("缺少 id"), "{no_id}");
}

#[test]
fn set_active_happy_path_writes_config_and_flips_lock() {
    let _guard = SLOT_LOCK.lock();
    let tmp = tempfile::tempdir().unwrap();
    let skins = tmp.path().join("skins");
    std::fs::create_dir(&skins).unwrap();
    write_theme_skin(&skins, "alpha", "alpha");
    seed_config(tmp.path(), false);
    let active = Arc::new(parking_lot::RwLock::new("default".to_string()));
    set_handle(
        Some(skins.to_string_lossy().to_string()),
        Arc::clone(&active),
    );
    let ctx = make_ctx(tmp.path());

    let res = SkinsHandler::new()
        .set_active(Some(json!({ "id": "alpha" })), &ctx)
        .expect("set ok")
        .expect("payload");
    assert_eq!(res["active"], json!("alpha"));
    assert_eq!(*active.read(), "alpha");
    assert_eq!(read_config_skin(tmp.path()), "alpha");
}

#[test]
fn set_active_default_turns_off_without_scan() {
    let _guard = SLOT_LOCK.lock();
    let tmp = tempfile::tempdir().unwrap();
    // 目录里放一个坏包：default 路径不做扫描，坏包在也不影响关皮肤。
    let skins = tmp.path().join("skins");
    std::fs::create_dir(&skins).unwrap();
    std::fs::write(skins.join("junk.nbskin"), b"garbage").unwrap();
    seed_config(tmp.path(), false);
    let active = Arc::new(parking_lot::RwLock::new("alpha".to_string()));
    set_handle(
        Some(skins.to_string_lossy().to_string()),
        Arc::clone(&active),
    );
    let ctx = make_ctx(tmp.path());

    SkinsHandler::new()
        .set_active(Some(json!({ "id": "default" })), &ctx)
        .expect("default ok");
    assert_eq!(*active.read(), "default");
    assert_eq!(read_config_skin(tmp.path()), "default");
}

#[test]
fn set_active_rejects_no_payload_broken_and_missing() {
    let _guard = SLOT_LOCK.lock();
    let tmp = tempfile::tempdir().unwrap();
    let skins = tmp.path().join("skins");
    std::fs::create_dir(&skins).unwrap();
    write_app_skin(&skins, "apponly");
    std::fs::write(skins.join("broken.nbskin"), b"garbage").unwrap();
    seed_config(tmp.path(), false);
    let active = Arc::new(parking_lot::RwLock::new("default".to_string()));
    set_handle(
        Some(skins.to_string_lossy().to_string()),
        Arc::clone(&active),
    );
    let ctx = make_ctx(tmp.path());
    let h = SkinsHandler::new();

    let app_err = h
        .set_active(Some(json!({ "id": "apponly" })), &ctx)
        .unwrap_err();
    assert!(app_err.contains("无主题载荷"), "{app_err}");
    assert_eq!(*active.read(), "default", "拒绝路径不得翻锁");

    let broken_err = h
        .set_active(Some(json!({ "id": "broken" })), &ctx)
        .unwrap_err();
    assert!(broken_err.contains("损坏"), "{broken_err}");

    let miss_err = h
        .set_active(Some(json!({ "id": "ghost" })), &ctx)
        .unwrap_err();
    assert!(miss_err.contains("皮肤不存在"), "{miss_err}");
}

#[test]
fn set_active_require_signed_gate() {
    let _guard = SLOT_LOCK.lock();
    let tmp = tempfile::tempdir().unwrap();
    let skins = tmp.path().join("skins");
    std::fs::create_dir(&skins).unwrap();
    write_theme_skin(&skins, "alpha", "alpha");
    seed_config(tmp.path(), true); // require_signed = true
    let active = Arc::new(parking_lot::RwLock::new("default".to_string()));
    set_handle(
        Some(skins.to_string_lossy().to_string()),
        Arc::clone(&active),
    );
    let ctx = make_ctx(tmp.path());
    let h = SkinsHandler::new();

    let err = h
        .set_active(Some(json!({ "id": "alpha" })), &ctx)
        .unwrap_err();
    assert!(err.contains("require_signed"), "{err}");
    assert_eq!(read_config_skin(tmp.path()), "default", "拒绝不得写 config");

    // 关闸后同包放行（测试进程无锚 → Unverified；require_signed=false 不拦）。
    seed_config(tmp.path(), false);
    h.set_active(Some(json!({ "id": "alpha" })), &ctx)
        .expect("ok");
    assert_eq!(read_config_skin(tmp.path()), "alpha");
}

#[test]
fn scan_skins_id_mismatch_and_signature_shape() {
    let tmp = tempfile::tempdir().unwrap();
    write_theme_skin(tmp.path(), "renamed", "original-id");
    let entries = scan_skins(tmp.path().to_str().unwrap());
    assert_eq!(entries.len(), 1);
    let e = &entries[0];
    assert_eq!(e.id, "renamed");
    assert!(e.id_mismatch, "manifest.id ≠ 文件名 stem 必须注记");
    assert_eq!(e.status, SkinStatus::Ok);
    // 徽标值合法即可（测试进程锚在场与否随环境漂移，不硬断言具体态）。
    assert!(matches!(
        e.signature,
        SkinSignature::Verified
            | SkinSignature::Unsigned
            | SkinSignature::Invalid
            | SkinSignature::Unverified
    ));
}

#[test]
fn format_version_mismatch_is_broken() {
    let tmp = tempfile::tempdir().unwrap();
    let file = std::fs::File::create(tmp.path().join("future.nbskin")).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file("manifest.json", zip::write::SimpleFileOptions::default())
        .unwrap();
    write!(
        zip,
        r#"{{"id":"future","format_version":2,"entry":"skin/x.css"}}"#
    )
    .unwrap();
    zip.finish().unwrap();
    let entries = scan_skins(tmp.path().to_str().unwrap());
    assert_eq!(entries[0].status, SkinStatus::Broken);
    assert!(
        entries[0]
            .status_detail
            .as_deref()
            .unwrap()
            .contains("format_version")
    );
}

/// 签名管线端到端：keygen 现签 → verify_bytes Valid → classify = Verified；
/// 篡改一字节 → Tampered → Invalid。锚直传（不经 env，免进程全局污染）。
#[test]
fn signature_pipeline_end_to_end() {
    use nemesis_verify::keygen;
    use nemesis_verify::verify::verify_bytes;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let keys = keygen::generate_at(now).expect("keygen");
    let anchor = keys.root_anchor_fingerprint();

    // 真实 ZIP 打包 → raw 签发（footer 附尾）
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("signed.nbskin");
    {
        let file = std::fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file("manifest.json", zip::write::SimpleFileOptions::default())
            .unwrap();
        write!(zip, r#"{{"id":"signed","entry":"skin/main.css"}}"#).unwrap();
        zip.start_file("skin/main.css", zip::write::SimpleFileOptions::default())
            .unwrap();
        write!(zip, "html {{ --x: 1; }}").unwrap();
        zip.finish().unwrap();
    }
    let raw = std::fs::read(&path).unwrap();
    let signed = nemesis_verify::verify::sign_content_v4(
        &raw,
        &keys.leaf_sk,
        now,
        &keys.chain(),
        Some("nemesisbot-test"),
    )
    .expect("sign");
    std::fs::write(&path, &signed).unwrap();

    // 锚在场：Valid → Verified；scan 层面 ZIP（含 footer）仍可解析 = ok。
    let outcome = verify_bytes(&signed, &[anchor], now);
    assert!(
        matches!(outcome, nemesis_verify::verify::VerifyOutcome::Valid { .. }),
        "signed package must verify Valid, got {outcome:?}"
    );
    let (sig, detail) = classify_signature(nemesis_verify::verify::VerifyOutcome::Valid {
        signed_at: now,
        key_fp: [0u8; 32],
        pubkey: [0u8; 65],
    });
    assert_eq!(sig, SkinSignature::Verified);
    assert!(detail.is_none());

    // 篡改 payload 首字节（摘要覆盖 [0,L)，L = footer 前的 ZIP 区——改
    // footer 区自身只会 Malformed 而非 Tampered）→ Tampered → Invalid。
    let mut tampered = signed.clone();
    tampered[0] ^= 0xFF;
    let outcome = verify_bytes(&tampered, &[anchor], now);
    assert!(matches!(
        outcome,
        nemesis_verify::verify::VerifyOutcome::Tampered(_)
    ));
    let (sig, detail) = classify_signature(outcome);
    assert_eq!(sig, SkinSignature::Invalid);
    assert!(detail.unwrap().contains("Tampered"));

    // 剥 footer（原文重写）→ NoSignature → Unsigned
    let outcome = verify_bytes(&raw, &[anchor], now);
    assert!(matches!(
        outcome,
        nemesis_verify::verify::VerifyOutcome::NoSignature
    ));
    let (sig, _) = classify_signature(outcome);
    assert_eq!(sig, SkinSignature::Unsigned);

    // 无锚 → Unverified（scan 的 anchors.is_empty 短路语义）
    let outcome = verify_bytes(&signed, &[], now);
    assert!(matches!(
        outcome,
        nemesis_verify::verify::VerifyOutcome::Untrusted
    ));
    let entries = scan_skins(tmp.path().to_str().unwrap());
    assert_eq!(
        entries[0].status,
        SkinStatus::Ok,
        "signed ZIP（footer 附尾）必须可解析"
    );
}

#[test]
fn hot_flip_lock_reflected_in_router() {
    // 锁热翻 → active.css 下一请求即切（免重启语义的数据面锚定）。
    let tmp = tempfile::tempdir().unwrap();
    write_theme_skin(tmp.path(), "alpha", "alpha");
    write_theme_skin(tmp.path(), "beta", "beta");
    let active = Arc::new(parking_lot::RwLock::new("alpha".to_string()));
    let app = crate::skins::skin_router(
        Some(tmp.path().to_string_lossy().to_string()),
        Arc::clone(&active),
    )
    .unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();

    let body = rt.block_on(async {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/skins/active.css")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.headers().get("x-skin-id").unwrap(), "alpha");
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    });
    assert!(body.contains("alpha"), "热翻前必须服务 alpha");

    // 热翻：WSAPI set_active 第③步的同款动作。
    *active.write() = "beta".to_string();
    let app2 = crate::skins::skin_router(
        Some(tmp.path().to_string_lossy().to_string()),
        Arc::clone(&active),
    )
    .unwrap();
    rt.block_on(async {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;
        let res = app2
            .oneshot(
                Request::builder()
                    .uri("/skins/active.css")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.headers().get("x-skin-id").unwrap(), "beta");
    });
}
