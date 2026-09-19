//! [`super::board_asset_rpc`]（资产 RPC 兜底通路·提供方）单测。
//!
//! 全本地：临时 workspace + 真板库（表登记白名单）+ 真密钥文件 + 真资产
//! 文件，直接调用 handler 闭包（不启 RPC server——验证链语义在闭包内，
//! server 层是通用路由，transfer/peer_chat 测试已覆盖）。

use super::*;
use nemesis_board::NewAsset;

fn temp_deps() -> (tempfile::TempDir, AssetRpcDeps, Vec<u8>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let ws = dir.path().join("workspace");
    std::fs::create_dir_all(&ws).expect("mkdir workspace");
    let secret = load_or_create_secret(&nemesis_path::resolve_asset_secret_path_in_workspace(&ws))
        .expect("secret");
    let store =
        Arc::new(BoardStore::open(&ws.join("board").join("board.db"), "NB").expect("store"));
    (
        dir,
        AssetRpcDeps {
            workspace: ws,
            board_store: Some(store),
        },
        secret,
    )
}

/// 登记 + 落盘一份资产，返回 (ref, 内容, token, expires_at)。
fn seed_asset(deps: &AssetRpcDeps, secret: &[u8], ref_name: &str, content: &[u8]) -> (String, i64) {
    let assets_dir = nemesis_path::resolve_board_assets_dir_in_workspace(&deps.workspace);
    std::fs::create_dir_all(&assets_dir).expect("mkdir assets");
    std::fs::write(assets_dir.join(ref_name), content).expect("write asset");
    let store = deps.board_store.as_ref().expect("store");
    store
        .register_asset(NewAsset {
            ref_name: ref_name.to_string(),
            origin_issue: None,
            sha256: nemesis_board::sha256_bytes(content),
            size: content.len() as i64,
        })
        .expect("register");
    let expires = chrono::Utc::now().timestamp() + 600;
    let token = nemesis_board::sign_asset_token(secret, ref_name, expires);
    (token, expires)
}

fn meta_payload(asset_ref: &str, token: &str, expires_at: i64) -> serde_json::Value {
    serde_json::json!({
        "asset_ref": asset_ref,
        "asset_token": token,
        "expires_at": expires_at,
    })
}

// ---------------------------------------------------------------------------
// asset.meta
// ---------------------------------------------------------------------------

#[test]
fn meta_returns_registered_sha_and_actual_size() {
    let (_guard, deps, secret) = temp_deps();
    let content = b"hello asset rpc";
    let (token, expires) = seed_asset(&deps, &secret, "spec.md", content);
    let meta = build_meta_handler(deps.clone());
    let out = meta(meta_payload("spec.md", &token, expires)).expect("meta ok");
    assert_eq!(out["size"], content.len() as u64, "actual file size");
    assert_eq!(out["sha256"], nemesis_board::sha256_bytes(content));
}

#[test]
fn meta_rejects_unregistered_bad_token_and_missing_file() {
    let (_guard, deps, secret) = temp_deps();
    let meta = build_meta_handler(deps.clone());
    let (token, expires) = seed_asset(&deps, &secret, "real.md", b"x");

    // 未登记 ref：一律「不存在」（防枚举，即使验签会过——token 是对
    // ref 名签的，换名验签本来就不过；这里用合法 token 打未登记名）。
    let err = meta(meta_payload("ghost.md", &token, expires)).expect_err("unregistered");
    assert!(err.contains("not found"), "got: {err}");

    // 坏密钥签的 token → Invalid。
    let wrong = nemesis_board::sign_asset_token(b"other", "real.md", expires);
    let err = meta(meta_payload("real.md", &wrong, expires)).expect_err("bad token");
    assert!(err.contains("invalid"), "got: {err}");

    // 过期 token → Expired。
    let past = chrono::Utc::now().timestamp() - 10;
    let stale = nemesis_board::sign_asset_token(&secret, "real.md", past);
    let err = meta(meta_payload("real.md", &stale, past)).expect_err("expired");
    assert!(err.contains("expired"), "got: {err}");

    // 登记过但盘上内容被删 → 诚实报缺失。
    let assets_dir = nemesis_path::resolve_board_assets_dir_in_workspace(&deps.workspace);
    std::fs::remove_file(assets_dir.join("real.md")).expect("remove");
    let err = meta(meta_payload("real.md", &token, expires)).expect_err("missing content");
    assert!(err.contains("missing on disk"), "got: {err}");

    // 路径穿越 ref：字符白名单先拦。
    let err = meta(meta_payload("../escape", &token, expires)).expect_err("traversal");
    assert!(err.contains("ref"), "got: {err}");
}

#[test]
fn meta_without_registry_honestly_refuses() {
    // board_store None（registry 未装配的裁剪形态）→ 诚实拒绝，不退化成
    // 无白名单服务。
    let dir = tempfile::tempdir().expect("tempdir");
    let ws = dir.path().join("workspace");
    std::fs::create_dir_all(&ws).expect("mkdir");
    let bare = AssetRpcDeps {
        workspace: ws,
        board_store: None,
    };
    let meta = build_meta_handler(bare);
    let err = meta(meta_payload("spec.md", "any", 1)).expect_err("no registry");
    assert!(err.contains("registry not available"), "got: {err}");
}

// ---------------------------------------------------------------------------
// asset.chunk
// ---------------------------------------------------------------------------

#[test]
fn chunk_reads_first_middle_last_block_with_eof() {
    let (_guard, deps, secret) = temp_deps();
    // 5 字节内容，按 2 字节块拉三刀：0/2/4。
    let content: &[u8] = b"01234";
    let (token, expires) = seed_asset(&deps, &secret, "blob.bin", content);
    let chunk = build_chunk_handler(deps.clone());

    let pull = |offset: u64, len: u64| -> (Vec<u8>, bool) {
        let out = chunk(serde_json::json!({
            "asset_ref": "blob.bin", "asset_token": token,
            "expires_at": expires, "offset": offset, "len": len,
        }))
        .expect("chunk ok");
        let data = nemesis_cluster::transfer::b64_decode(out["data"].as_str().expect("b64"))
            .expect("decode");
        (data, out["eof"].as_bool().expect("eof flag"))
    };

    let (d, eof) = pull(0, 2);
    assert_eq!(d, b"01");
    assert!(!eof);
    let (d, eof) = pull(2, 2);
    assert_eq!(d, b"23");
    assert!(!eof);
    let (d, eof) = pull(4, 2);
    assert_eq!(d, b"4", "tail block truncated to file end");
    assert!(eof);
}

#[test]
fn chunk_len_clamped_and_bad_args_refused() {
    let (_guard, deps, secret) = temp_deps();
    let (token, expires) = seed_asset(&deps, &secret, "big.bin", &vec![7u8; 4096]);
    let chunk = build_chunk_handler(deps.clone());

    // 请求超 MAX_CHUNK_LEN → 服务端钳制（返回块不超上限）。
    let out = chunk(serde_json::json!({
        "asset_ref": "big.bin", "asset_token": token, "expires_at": expires,
        "offset": 0, "len": MAX_CHUNK_LEN * 4,
    }))
    .expect("clamped read ok");
    let data =
        nemesis_cluster::transfer::b64_decode(out["data"].as_str().expect("b64")).expect("decode");
    assert_eq!(
        data.len() as u64,
        MAX_CHUNK_LEN.min(4096),
        "clamped to file end"
    );

    // offset 越界 → 诚实报错。
    let err = chunk(serde_json::json!({
        "asset_ref": "big.bin", "asset_token": token, "expires_at": expires,
        "offset": 9999, "len": 16,
    }))
    .expect_err("offset beyond size");
    assert!(err.contains("beyond asset size"), "got: {err}");

    // len=0 / 缺字段 → 拒绝。
    let err = chunk(serde_json::json!({
        "asset_ref": "big.bin", "asset_token": token, "expires_at": expires,
        "offset": 0, "len": 0,
    }))
    .expect_err("zero len");
    assert!(err.contains("len must be > 0"), "got: {err}");
    assert!(
        chunk(serde_json::json!({
            "asset_ref": "big.bin", "asset_token": token, "expires_at": expires,
        }))
        .is_err(),
        "missing offset/len refused"
    );

    // 验签链同样生效（坏 token 拉块被拒）。
    let err = chunk(serde_json::json!({
        "asset_ref": "big.bin", "asset_token": "deadbeef", "expires_at": expires,
        "offset": 0, "len": 16,
    }))
    .expect_err("bad token");
    assert!(err.contains("invalid"), "got: {err}");
}

#[test]
fn chunk_oversize_asset_beyond_one_mib_reads_clamped_block() {
    // >1MiB 资产：0 偏移拉 MAX_CHUNK_LEN*2 → 恰好回 1MiB（钳制生效）。
    let (_guard, deps, secret) = temp_deps();
    let big = vec![9u8; (MAX_CHUNK_LEN as usize) + 123];
    let (token, expires) = seed_asset(&deps, &secret, "huge.bin", &big);
    let chunk = build_chunk_handler(deps);
    let out = chunk(serde_json::json!({
        "asset_ref": "huge.bin", "asset_token": token, "expires_at": expires,
        "offset": 0, "len": MAX_CHUNK_LEN * 2,
    }))
    .expect("chunk ok");
    let data =
        nemesis_cluster::transfer::b64_decode(out["data"].as_str().expect("b64")).expect("decode");
    assert_eq!(data.len(), MAX_CHUNK_LEN as usize, "server-side clamp");
    assert!(!out["eof"].as_bool().expect("eof flag"));
}
