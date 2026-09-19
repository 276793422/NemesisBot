//! [`super::BoardAssetTool`] 单测。全部本地：publish 走临时 workspace 真库
//! 真文件；fetch 用 tokio 裸 TCP 手写 HTTP 响应（免新增 dev-dep，G9 端到端
//! 语义在批次 G cluster-uat 再覆盖真 gateway）。

use super::*;
use nemesis_agent::r#loop::Tool as _;
use std::sync::Mutex;

fn ctx() -> RequestContext {
    RequestContext::new("web", "chat-1", "user", "session-1")
}

fn temp_workspace() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let ws = dir.path().join("workspace");
    std::fs::create_dir_all(&ws).expect("mkdir workspace");
    (dir, ws)
}

/// 裸 TCP HTTP 服务器：无限循环 accept，读请求头后回 200 + 固定 body，
/// 收到的请求行记进共享 Vec（测试断言 URL 拼装用）。
async fn spawn_asset_server(body: &'static [u8]) -> (String, std::sync::Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let requests = std::sync::Arc::new(Mutex::new(Vec::<String>::new()));
    let req_sink = requests.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                break;
            };
            let sink = req_sink.clone();
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = Vec::new();
                let mut chunk = [0u8; 1024];
                // 读到头部结束（\r\n\r\n）为止——GET 无 body。
                loop {
                    let Ok(n) = sock.read(&mut chunk).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let text = String::from_utf8_lossy(&buf);
                if let Some(line) = text.lines().next() {
                    sink.lock().expect("sink lock").push(line.to_string());
                }
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.write_all(body).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    (format!("http://{addr}"), requests)
}

// ---------------------------------------------------------------------------
// parse_asset_args
// ---------------------------------------------------------------------------

#[test]
fn parse_fetch_and_publish_happy_paths() {
    let fetch = parse_asset_args(
        r#"{"action":"fetch","node_url":"http://10.0.0.2:49100/","asset_ref":"a.md",
            "asset_token":"ab12","expires_at":123,"sha256":"ABC"}"#,
    )
    .expect("parse fetch");
    match fetch {
        AssetArgs::Fetch {
            node_url,
            asset_ref,
            asset_token,
            expires_at,
            sha256,
            node_id,
        } => {
            assert_eq!(node_url, "http://10.0.0.2:49100/");
            assert_eq!(asset_ref, "a.md");
            assert_eq!(asset_token, "ab12");
            assert_eq!(expires_at, 123);
            assert_eq!(sha256, "ABC");
            // node_id 可选：缺省 None（旧 bundle 仅 HTTP 语义）。
            assert_eq!(node_id, None);
        }
        other => panic!("expected Fetch, got {other:?}"),
    }

    // 带 node_id（新 bundle）→ RPC 兜底可用；空白串视同缺省。
    let with_id = parse_asset_args(
        r#"{"action":"fetch","node_url":"http://x","asset_ref":"a","asset_token":"t",
            "expires_at":1,"sha256":"s","node_id":" node-worker-1 "}"#,
    )
    .expect("parse fetch with node_id");
    match with_id {
        AssetArgs::Fetch { node_id, .. } => assert_eq!(node_id, Some("node-worker-1".to_string())),
        other => panic!("expected Fetch, got {other:?}"),
    }
    let blank_id = parse_asset_args(
        r#"{"action":"fetch","node_url":"http://x","asset_ref":"a","asset_token":"t",
            "expires_at":1,"sha256":"s","node_id":"  "}"#,
    )
    .expect("parse fetch blank node_id");
    match blank_id {
        AssetArgs::Fetch { node_id, .. } => assert_eq!(node_id, None),
        other => panic!("expected Fetch, got {other:?}"),
    }

    let publish =
        parse_asset_args(r#"{"action":"publish","path":"out/r.md"}"#).expect("parse publish");
    assert_eq!(
        publish,
        AssetArgs::Publish {
            path: "out/r.md".to_string(),
            ref_name: None
        }
    );

    let named =
        parse_asset_args(r#"{"action":"publish","path":"r.md","ref_name":" report-final "}"#)
            .expect("parse publish named");
    assert_eq!(
        named,
        AssetArgs::Publish {
            path: "r.md".to_string(),
            ref_name: Some("report-final".to_string())
        }
    );
}

#[test]
fn parse_rejects_bad_action_missing_fields_and_bad_json() {
    assert!(parse_asset_args("not json").is_err());
    assert!(parse_asset_args(r#"{"action":"download"}"#).is_err());
    assert!(parse_asset_args(r#"{}"#).is_err());
    // fetch 缺字段（sha256 / expires_at / asset_token 各缺一）。
    assert!(parse_asset_args(r#"{"action":"fetch","node_url":"http://x"}"#).is_err());
    assert!(
        parse_asset_args(r#"{"action":"fetch","node_url":"http://x","asset_ref":"a","asset_token":"t","expires_at":1}"#)
            .is_err()
    );
    assert!(
        parse_asset_args(r#"{"action":"fetch","node_url":"http://x","asset_ref":"a","asset_token":"t","sha256":"x"}"#)
            .is_err()
    );
    // publish 空 path / 空 ref_name（过滤成 None）。
    assert!(parse_asset_args(r#"{"action":"publish","path":"  "}"#).is_err());
}

// ---------------------------------------------------------------------------
// validate_fetch_params / validate_publish_size
// ---------------------------------------------------------------------------

#[test]
fn validate_fetch_builds_url_and_normalizes_sha() {
    let (url, sha) = validate_fetch_params(
        "http://192.168.1.10:49100/",
        "report.md",
        "deadbeef",
        1700000000,
        "AABBCCDDEEFF0011223344556677889900112233445566778899AABBCCDDEEFF",
    )
    .expect("valid");
    // 结尾斜杠被吃掉，query 完整；sha 归一为小写。
    assert_eq!(
        url,
        "http://192.168.1.10:49100/api/board/asset/report.md?asset_token=deadbeef&expires_at=1700000000"
    );
    assert_eq!(
        sha,
        "aabbccddeeff0011223344556677889900112233445566778899aabbccddeeff"
    );
}

#[test]
fn validate_fetch_rejects_traversal_and_bad_sha() {
    let sha_ok = "a".repeat(64);
    // 路径穿越 / 隐藏名 / 非法字符都被 ref 白名单拦（sanitize 单测在
    // nemesis-board 侧，这里只钉工具链路真的过了这道闸）。
    assert!(validate_fetch_params("http://x", "../etc", "t", 1, &sha_ok).is_err());
    assert!(validate_fetch_params("http://x", ".hidden", "t", 1, &sha_ok).is_err());
    assert!(validate_fetch_params("http://x", "a/b", "t", 1, &sha_ok).is_err());
    // sha 格式：长度错 / 非 hex。
    assert!(validate_fetch_params("http://x", "a.md", "t", 1, "abc").is_err());
    assert!(validate_fetch_params("http://x", "a.md", "t", 1, &"z".repeat(64)).is_err());
}

#[test]
fn publish_size_boundary() {
    assert!(validate_publish_size(0).is_ok());
    assert!(validate_publish_size(MAX_PUBLISH_BYTES).is_ok());
    assert!(validate_publish_size(MAX_PUBLISH_BYTES + 1).is_err());
}

// ---------------------------------------------------------------------------
// publish 端到端（真库真文件，无网络）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn publish_outside_workspace_is_refused() {
    let (_guard, ws) = temp_workspace();
    let outside = tempfile::tempdir().expect("outside dir");
    let secret_file = outside.path().join("leak.txt");
    std::fs::write(&secret_file, "sensitive").expect("write");

    let tool = BoardAssetTool::new(ws);
    let args = serde_json::json!({
        "action": "publish",
        "path": secret_file.display().to_string(),
    })
    .to_string();
    let err = tool.execute(&args, &ctx()).await.expect_err("must refuse");
    assert!(err.contains("outside the workspace"), "got: {err}");
}

#[tokio::test]
async fn publish_full_flow_registers_and_signs_bundle() {
    let (_guard, ws) = temp_workspace();
    let source = ws.join("report.md");
    let content = b"# delivery\nverified body";
    std::fs::write(&source, content).expect("write source");

    let tool = BoardAssetTool::new(ws.clone());
    let args = serde_json::json!({ "action": "publish", "path": "report.md" }).to_string();

    // 第一跑：gateway 还没落盘 node url → 诚实失败（拷贝/登记/密钥副作用
    // 已发生且幂等，重试不需要清理）。
    let err = tool
        .execute(&args, &ctx())
        .await
        .expect_err("no url file yet");
    assert!(err.contains("gateway must be running"), "got: {err}");

    // 补上 url 文件（gateway bind 后写的同一路径），重跑成功。
    let url_path = nemesis_path::resolve_asset_node_url_path_in_workspace(&ws);
    std::fs::create_dir_all(url_path.parent().unwrap()).expect("mkdir config");
    std::fs::write(&url_path, "http://192.168.1.10:49100/\n").expect("write url");

    let out = tool.execute(&args, &ctx()).await.expect("publish ok");
    let bundle_line = out
        .lines()
        .find(|l| l.trim_start().starts_with('{'))
        .expect("bundle json line in output");
    let bundle: serde_json::Value = serde_json::from_str(bundle_line.trim()).expect("bundle json");
    assert_eq!(bundle["asset_ref"], "report.md");
    assert_eq!(bundle["node_url"], "http://192.168.1.10:49100");
    assert_eq!(
        bundle["sha256"],
        nemesis_board::sha256_bytes(content),
        "integrity baseline must be fetchable from the bundle"
    );
    assert_eq!(bundle["size"], content.len() as i64);
    let token = bundle["asset_token"].as_str().expect("token str");
    assert_eq!(token.len(), 64, "hmac-sha256 hex");
    assert!(bundle["expires_at"].as_i64().expect("expiry") > chrono::Utc::now().timestamp());

    // 索引已登记 + sha 与盘上一致 + 密钥文件已生成。
    let store = nemesis_board::BoardStore::open(&ws.join("board").join("board.db"), "NB")
        .expect("open store");
    let asset = store
        .lookup_asset("report.md")
        .expect("lookup")
        .expect("registered");
    assert_eq!(asset.sha256, nemesis_board::sha256_bytes(content));
    assert_eq!(asset.size, content.len() as i64);
    let copied = ws.join("board").join("assets").join("report.md");
    assert_eq!(std::fs::read(&copied).expect("read copy"), content);
    let secret_path = nemesis_path::resolve_asset_secret_path_in_workspace(&ws);
    let secret_text = std::fs::read_to_string(&secret_path).expect("secret file");
    assert_eq!(secret_text.trim().len(), 64, "32-byte secret as 64 hex");
}

#[tokio::test]
async fn publish_custom_ref_and_relative_path() {
    let (_guard, ws) = temp_workspace();
    std::fs::create_dir_all(ws.join("out")).expect("mkdir out");
    std::fs::write(ws.join("out").join("blob.bin"), b"\x00\x01\x02").expect("write");
    let url_path = nemesis_path::resolve_asset_node_url_path_in_workspace(&ws);
    std::fs::create_dir_all(url_path.parent().unwrap()).expect("mkdir config");
    std::fs::write(&url_path, "http://10.1.1.1:49000").expect("write url");

    let tool = BoardAssetTool::new(ws.clone());
    let args = serde_json::json!({
        "action": "publish",
        "path": "out/blob.bin",
        "ref_name": "blob-v2.bin"
    })
    .to_string();
    let out = tool.execute(&args, &ctx()).await.expect("publish ok");
    assert!(out.contains("blob-v2.bin"), "got: {out}");
    assert!(ws.join("board").join("assets").join("blob-v2.bin").exists());
}

// ---------------------------------------------------------------------------
// fetch 端到端（裸 TCP 假提供方）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fetch_downloads_verifies_and_lands_file() {
    let body: &'static [u8] = b"asset payload 0123456789";
    let (base, requests) = spawn_asset_server(body).await;
    let (_guard, ws) = temp_workspace();
    let tool = BoardAssetTool::new(ws.clone());

    let args = serde_json::json!({
        "action": "fetch",
        "node_url": base,
        "asset_ref": "spec.md",
        "asset_token": "cafebabe",
        "expires_at": chrono::Utc::now().timestamp() + 600,
        "sha256": nemesis_board::sha256_bytes(body),
    })
    .to_string();
    let out = tool.execute(&args, &ctx()).await.expect("fetch ok");
    assert!(out.contains("sha256-verified"), "got: {out}");

    // URL 拼装语义：路径 + 两个凭据参数都在请求行上。
    let lines = requests.lock().expect("requests lock");
    assert_eq!(lines.len(), 1, "exactly one download request");
    assert!(
        lines[0].contains("GET /api/board/asset/spec.md?"),
        "got: {}",
        lines[0]
    );
    assert!(
        lines[0].contains("asset_token=cafebabe"),
        "got: {}",
        lines[0]
    );
    assert!(lines[0].contains("expires_at="), "got: {}", lines[0]);

    // 落定文件内容一致，无 .part 残留。
    let final_path = ws.join("board").join("assets").join("spec.md");
    assert_eq!(std::fs::read(&final_path).expect("read final"), body);
    assert!(
        !ws.join("board")
            .join("assets")
            .join("spec.md.part")
            .exists()
    );
}

#[tokio::test]
async fn fetch_sha_mismatch_discards_file() {
    let body: &'static [u8] = b"tampered-or-stale content";
    let (base, _requests) = spawn_asset_server(body).await;
    let (_guard, ws) = temp_workspace();
    let tool = BoardAssetTool::new(ws.clone());

    let wrong_sha = format!("{:0>64}", "0");
    let args = serde_json::json!({
        "action": "fetch",
        "node_url": base,
        "asset_ref": "spec.md",
        "asset_token": "cafebabe",
        "expires_at": chrono::Utc::now().timestamp() + 600,
        "sha256": wrong_sha,
    })
    .to_string();
    let err = tool
        .execute(&args, &ctx())
        .await
        .expect_err("must mismatch");
    assert!(err.contains("sha256 mismatch"), "got: {err}");
    let assets = ws.join("board").join("assets");
    assert!(!assets.join("spec.md").exists(), "final file must not land");
    assert!(!assets.join("spec.md.part").exists(), "part file cleaned");
}

// ---------------------------------------------------------------------------
// fetch RPC 兜底（2026-09-20：HTTP 连接级失败 → 集群 RPC 分块拉取）
// ---------------------------------------------------------------------------

use std::sync::Arc as StdArc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// 回 HTTP 403 的假提供方（业务错场景——验证不触发 RPC 兜底）。
async fn spawn_403_server() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = vec![0u8; 1024];
                let _ = sock.read(&mut buf).await;
                let _ = sock
                    .write_all(
                        b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .await;
                let _ = sock.shutdown().await;
            });
        }
    });
    format!("http://{addr}")
}

/// 构造带 RPC mock 的集群：meta/chunk 从给定资产字节出发，行为与真
/// board_asset_rpc handler 一致（在测试里复刻语义——handler 本体的
/// 验证链单测在 board_asset_rpc/tests.rs，此处只钉工具侧的兜底编排）。
fn mock_cluster(
    body: Vec<u8>,
    provider_sha: String,
    meta_size: u64,
) -> (StdArc<Cluster>, StdArc<AtomicUsize>) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config = nemesis_cluster::types::ClusterConfig {
        node_id: String::new(),
        bind_address: "0.0.0.0:0".to_string(),
        peers: vec![],
        node_name: "MockPeer".to_string(),
    };
    let cluster = StdArc::new(Cluster::with_workspace(config, tmp.path().to_path_buf()));
    let calls = StdArc::new(AtomicUsize::new(0));
    let body = StdArc::new(body);
    let sha_call = calls.clone();
    cluster.set_call_with_context_fn(Box::new(move |_peer, action, payload| {
        sha_call.fetch_add(1, Ordering::SeqCst);
        match action {
            "asset.meta" => Ok(serde_json::json!({
                "size": meta_size,
                "sha256": provider_sha,
            })
            .to_string()
            .into_bytes()),
            "asset.chunk" => {
                let offset = payload["offset"].as_u64().expect("offset") as usize;
                let len = payload["len"].as_u64().expect("len") as usize;
                let end = (offset + len).min(body.len());
                Ok(serde_json::json!({
                    "data": nemesis_cluster::transfer::b64_encode(&body[offset..end]),
                    "eof": end >= body.len(),
                })
                .to_string()
                .into_bytes())
            }
            other => Err(format!("unexpected action {other}")),
        }
    }));
    (cluster, calls)
}

fn fetch_args_with_node_id(base: &str, sha: &str, node_id: &str) -> String {
    serde_json::json!({
        "action": "fetch",
        "node_url": base,
        "asset_ref": "spec.md",
        "asset_token": "cafebabe",
        "expires_at": chrono::Utc::now().timestamp() + 600,
        "sha256": sha,
        "node_id": node_id,
    })
    .to_string()
}

fn dead_http_base() -> String {
    // 端口 1（tcpmux）几乎必然连接拒绝——快且无副作用。
    "http://127.0.0.1:1".to_string()
}

#[tokio::test]
async fn connection_error_without_cluster_honestly_reports() {
    let (_guard, ws) = temp_workspace();
    let tool = BoardAssetTool::new(ws.clone());
    let args = fetch_args_with_node_id(&dead_http_base(), &"a".repeat(64), "node-provider");
    let err = tool.execute(&args, &ctx()).await.expect_err("must fail");
    assert!(
        err.contains("download request"),
        "连接级失败形态，got: {err}"
    );
    assert!(
        err.contains("no cluster connection"),
        "诚实报无集群句柄，got: {err}"
    );
}

#[tokio::test]
async fn connection_error_without_node_id_reports_legacy_bundle() {
    let body = b"x".to_vec();
    let (cluster, _calls) = mock_cluster(body, "a".repeat(64), 1);
    let (_guard, ws) = temp_workspace();
    let tool = BoardAssetTool::new(ws.clone()).with_cluster(cluster);
    // bundle 未带 node_id（旧格式）→ 无法兜底。
    let args = serde_json::json!({
        "action": "fetch",
        "node_url": dead_http_base(),
        "asset_ref": "spec.md",
        "asset_token": "cafebabe",
        "expires_at": chrono::Utc::now().timestamp() + 600,
        "sha256": "a".repeat(64),
    })
    .to_string();
    let err = tool.execute(&args, &ctx()).await.expect_err("must fail");
    assert!(err.contains("carries no node_id"), "got: {err}");
}

#[tokio::test]
async fn connection_error_falls_back_to_rpc_chunks() {
    let body = b"fallback payload delivered over cluster rpc".to_vec();
    let sha = nemesis_board::sha256_bytes(&body);
    let (cluster, calls) = mock_cluster(body.clone(), sha.clone(), body.len() as u64);
    let (_guard, ws) = temp_workspace();
    let tool = BoardAssetTool::new(ws.clone()).with_cluster(cluster);
    let args = fetch_args_with_node_id(&dead_http_base(), &sha, "node-provider");

    let out = tool.execute(&args, &ctx()).await.expect("rpc fallback ok");
    assert!(out.contains("cluster RPC"), "got: {out}");
    assert!(out.contains("sha256-verified"), "got: {out}");

    let final_path = ws.join("board").join("assets").join("spec.md");
    assert_eq!(std::fs::read(&final_path).expect("read final"), body);
    assert!(!final_path.with_extension("part").exists(), "part cleaned");
    assert!(calls.load(Ordering::SeqCst) >= 2, "meta + 至少一次 chunk");
}

#[tokio::test]
async fn rpc_fallback_reports_provider_drift_and_overlimit() {
    let body = b"provider content".to_vec();
    let (_guard, ws) = temp_workspace();

    // 提供方文件漂移：meta 回的 sha ≠ bundle sha → 拒绝，提示要新引用。
    let (cluster, _calls) = mock_cluster(body.clone(), "b".repeat(64), body.len() as u64);
    let tool = BoardAssetTool::new(ws.clone()).with_cluster(cluster);
    let args = fetch_args_with_node_id(&dead_http_base(), &"a".repeat(64), "node-provider");
    let err = tool.execute(&args, &ctx()).await.expect_err("drift");
    assert!(
        err.contains("does not match the reference bundle"),
        "got: {err}"
    );

    // 超 64MiB 上限：诚实拒绝，提示换同网段 HTTP。
    let (cluster, _calls) = mock_cluster(body, "a".repeat(64), MAX_RPC_FETCH_BYTES + 1);
    let tool = BoardAssetTool::new(ws.clone()).with_cluster(cluster);
    let err = tool.execute(&args, &ctx()).await.expect_err("overlimit");
    assert!(err.contains("too large for RPC fallback"), "got: {err}");
}

#[tokio::test]
async fn http_business_error_does_not_trigger_fallback() {
    let base = spawn_403_server().await;
    let body = b"x".to_vec();
    let (cluster, calls) = mock_cluster(body, "a".repeat(64), 1);
    let (_guard, ws) = temp_workspace();
    let tool = BoardAssetTool::new(ws.clone()).with_cluster(cluster);
    let args = fetch_args_with_node_id(&base, &"a".repeat(64), "node-provider");
    let err = tool.execute(&args, &ctx()).await.expect_err("403");
    assert!(err.contains("HTTP 403"), "业务错原样上抛，got: {err}");
    assert!(!err.contains("cluster RPC"), "业务错不触发兜底，got: {err}");
    assert_eq!(calls.load(Ordering::SeqCst), 0, "mock RPC 未被调用");
}

#[test]
fn connection_level_error_classification() {
    // 连接级（send 阶段）→ 兜底。
    assert!(is_connection_level_error(
        "download request: error sending request for url (http://x): connection refused"
    ));
    // HTTP 状态错 / 本地 IO 错 → 不兜底。
    assert!(!is_connection_level_error("HTTP 403 Forbidden"));
    assert!(!is_connection_level_error("read body: channel closed"));
    assert!(!is_connection_level_error("write: Access is denied"));
}
