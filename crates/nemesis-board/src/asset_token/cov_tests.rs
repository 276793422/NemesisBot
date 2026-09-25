// asset_token.rs 覆盖率补充测试（AssetTokenError Display 64-69 /
// AdvertisedUrl 三读 249-251 / sign_for 基址未就绪 256-258 + 就绪签发
// 260-266 / ready 268-270 / render_assets_section 空 276-278 + 未就绪注记
// 297-303 + 就绪签发 283-291）。

use super::*;
use crate::models::BoardAsset;

fn asset(ref_name: &str) -> BoardAsset {
    BoardAsset {
        id: 1,
        ref_name: ref_name.to_string(),
        origin_issue: None,
        sha256: "ab".repeat(32),
        size: 123,
        path: ref_name.to_string(),
        created_at: 1,
    }
}

fn context_with_url(url: Option<&str>) -> AssetSignContext {
    let ctx = AssetSignContext {
        secret: vec![7u8; 32],
        node_url: AdvertisedUrl::default(),
        node_id: "node-master".to_string(),
    };
    if let Some(u) = url {
        ctx.node_url.set(u.to_string());
    }
    ctx
}

/// 两类校验错误的人读文案（64-69）。
#[test]
fn token_error_display_messages() {
    assert_eq!(
        format!("{}", AssetTokenError::Expired),
        "asset token expired"
    );
    assert_eq!(
        format!("{}", AssetTokenError::Invalid),
        "asset token invalid"
    );
}

/// 基址槽读写与 ready 判定（249-251 / 268-270）；未就绪 sign_for → None
/// （256-258），set 后签发出完整 bundle（260-266）。
#[test]
fn advertised_url_and_sign_for_readiness() {
    let ctx = context_with_url(None);
    assert!(!ctx.node_url.is_set());
    assert!(!ctx.ready());
    assert!(ctx.sign_for("report.md", &"ab".repeat(32), 5, 60).is_none());

    ctx.node_url.set("http://127.0.0.1:49011".to_string());
    assert!(ctx.node_url.is_set());
    assert!(ctx.ready());
    assert_eq!(
        ctx.node_url.get().as_deref(),
        Some("http://127.0.0.1:49011")
    );

    let bundle = ctx
        .sign_for("report.md", &"ab".repeat(32), 5, 60)
        .expect("就绪后必须签发");
    assert_eq!(bundle.asset_ref, "report.md");
    assert_eq!(bundle.node_url, "http://127.0.0.1:49011");
    assert_eq!(bundle.node_id, "node-master");
}

/// render_assets_section 三形态：空资产 None（276-278）；基址未就绪逐条
/// 诚实注记（297-303）；就绪签发内嵌 bundle JSON（283-291）。
#[test]
fn render_assets_section_forms() {
    // 空 → None。
    let ctx = context_with_url(Some("http://127.0.0.1:49011"));
    assert!(render_assets_section(&ctx, &[], 60).is_none());

    // 未就绪 → 注记行。
    let raw = context_with_url(None);
    let none_ready = render_assets_section(&raw, &[asset("a.txt")], 60).unwrap();
    assert!(none_ready.contains("下载地址未就绪"), "{none_ready}");
    assert!(!none_ready.contains("```json"), "{none_ready}");

    // 就绪 → 每条带 bundle JSON 代码块。
    let ready = render_assets_section(&ctx, &[asset("a.txt"), asset("b.bin")], 60).unwrap();
    assert!(ready.contains("## 任务资产"));
    assert_eq!(ready.matches("```json").count(), 2, "{ready}");
    assert!(
        ready.contains("a.txt") && ready.contains("b.bin"),
        "{ready}"
    );
}

// ===========================================================================
// wave6 追加：sha256 字节/文件双形态 + secret 路径父目录缺失形态。
// ===========================================================================

/// sha256_bytes 已知摘要锁定（172-177）。
#[test]
fn w6_sha256_bytes_known_digest() {
    assert_eq!(
        sha256_bytes(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        sha256_bytes(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

/// sha256_file：文件存在与 bytes 版一致；缺失路径诚实报错（180-183）。
#[test]
fn w6_sha256_file_ok_and_missing() {
    let dir = std::env::temp_dir().join(format!("nmb-asset-w6-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("blob.bin");
    std::fs::write(&path, b"abc").unwrap();
    assert_eq!(sha256_file(&path).unwrap(), sha256_bytes(b"abc"));
    let err = sha256_file(&dir.join("gone.bin")).unwrap_err();
    assert!(err.contains("read"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// load_or_create_secret：多级父目录缺失时逐级创建（213-214）。
#[test]
fn w6_load_or_create_secret_creates_nested_parents() {
    let dir = std::env::temp_dir().join(format!("nmb-asset-w6n-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("config").join("deeper").join("asset_secret.key");
    let secret = load_or_create_secret(&path).unwrap();
    assert_eq!(secret.len(), 32);
    assert!(path.is_file(), "secret 文件落盘");
    let _ = std::fs::remove_dir_all(&dir);
}
