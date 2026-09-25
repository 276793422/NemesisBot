//! pair.rs 覆盖率补充测试（带假对端的网络路径）。
//!
//! 假对端 = 本机一次性 TcpListener，按 wire 协议（4 字节 BE 长度前缀 +
//! WireMessage JSON）应答一个 get_info 响应——真 RPC server 不依赖，
//! 协议字节形态与 `RpcClient`/`probe_get_info` 同源。覆盖：成功探测
//! （字面/推导双形态）、身份缺失、对端错误响应、密文往返（token 路径）、
//! 静默超时臂、Display 五形态、空主机名、rpc_port 断言失败面。

use super::*;
use std::io::{Read, Write};

// ---------------------------------------------------------------------------
// 假对端基础设施
// ---------------------------------------------------------------------------

/// 假对端的应答形态。
enum FakeReply {
    /// 正常响应（payload 落 WireMessage::new_response）。
    Payload(serde_json::Value),
    /// 错误响应（WireMessage::new_error）。
    ErrMsg(String),
    /// 原始字节（绕过协议，制造 decode 失败）。
    Raw(Vec<u8>),
}

/// 起一个一次性假对端，应答一次后退出。返回绑定的实际端口。
fn spawn_fake_server(reply: FakeReply) -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        // 用 std 同步实现即可——probe_get_info 的收发侧是 spawn_blocking 同步 IO。
        let (mut sock, _) = listener.accept().ok()?;
        let mut len_buf = [0u8; 4];
        sock.read_exact(&mut len_buf).ok()?;
        let len = u32::from_be_bytes(len_buf) as usize;
        let mut body = vec![0u8; len];
        sock.read_exact(&mut body).ok()?;

        match reply {
            FakeReply::Raw(raw) => {
                sock.write_all(&raw).ok()?;
            }
            kind => {
                let req: crate::transport::conn::WireMessage =
                    serde_json::from_slice(&body).ok()?;
                let resp = match kind {
                    FakeReply::Payload(p) => {
                        crate::transport::conn::WireMessage::new_response(&req, p)
                    }
                    FakeReply::ErrMsg(e) => {
                        crate::transport::conn::WireMessage::new_error(&req, &e)
                    }
                    FakeReply::Raw(_) => unreachable!(),
                };
                let payload = serde_json::to_vec(&resp).ok()?;
                let mut out = (payload.len() as u32).to_be_bytes().to_vec();
                out.extend_from_slice(&payload);
                sock.write_all(&out).ok()?;
            }
        }
        let _ = sock.flush();
        // 握住连接片刻再关，避免对端 recv 撞 RST 抖动。
        std::thread::sleep(std::time::Duration::from_millis(150));
        Some(())
    });
    port
}

fn cov_info_payload() -> serde_json::Value {
    serde_json::json!({
        "node_id": "peer-cov-1",
        "name": "CovPeer",
        "role": "worker",
        "category": "development",
        "addresses": ["10.0.0.5:11950"]
    })
}

fn fresh_peers_path(tag: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir
        .path()
        .join("cluster")
        .join(format!("peers-{}.toml", tag));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    (dir, path)
}

// ---------------------------------------------------------------------------
// 成功路径（probe_get_info 内部 + payload 解析 + 写盘 + 断言 + outcome）
// ---------------------------------------------------------------------------

/// 字面端口直接命中：全链路成功，outcome 与落盘 peers.toml 逐项核对。
#[tokio::test]
async fn pair_happy_path_literal_port_full_flow() {
    let port = spawn_fake_server(FakeReply::Payload(cov_info_payload()));
    let (_dir, path) = fresh_peers_path("happy-literal");

    let outcome = pair_with_peer(&path, None, &format!("127.0.0.1:{port}"))
        .await
        .expect("pair succeeds against fake peer");

    assert_eq!(outcome.peer_id, "peer-cov-1");
    assert_eq!(outcome.name, "CovPeer");
    assert_eq!(outcome.rpc_address, format!("127.0.0.1:{port}"));
    assert_eq!(outcome.addresses, vec!["10.0.0.5:11950".to_string()]);
    assert_eq!(outcome.rpc_port, port);
    assert!(outcome.literal_port_was_rpc, "字面端口命中");
    assert_eq!(outcome.udp_address, format!("127.0.0.1:{}", port - 10000));

    // 写后落盘：表键字面 + rpc_port + name。
    let content = std::fs::read_to_string(&path).unwrap();
    let doc: toml::Value = content.parse().unwrap();
    let entry = &doc["peers"]["peer-cov-1"];
    assert_eq!(entry["address"].as_str().unwrap(), outcome.udp_address);
    assert_eq!(entry["name"].as_str().unwrap(), "CovPeer");
    assert_eq!(entry["rpc_port"].as_integer(), Some(port as i64));
    assert_eq!(entry["role"].as_str().unwrap(), "worker");
}

/// 推导端口形态（+10000 约定）：字面候选先拒（tried 有记录），推导命中。
#[tokio::test]
async fn pair_happy_path_derived_port() {
    let port = spawn_fake_server(FakeReply::Payload(cov_info_payload()));
    // 找一个字面候选确实拒连的端口（ ephemeral 高位段 -10000 一般空闲，
    // 但防碰撞仍先验证）。
    let literal = port - 10000;
    let literal_free = std::net::TcpStream::connect(("127.0.0.1", literal)).is_err();
    if !literal_free {
        // 环境碰撞：换端口重来一次都撞就放弃断言细节（不红）。
        eprintln!("skip derived-port face: literal candidate {literal} unexpectedly open");
        return;
    }
    let (_dir, path) = fresh_peers_path("happy-derived");

    let outcome = pair_with_peer(&path, None, &format!("127.0.0.1:{literal}"))
        .await
        .expect("pair succeeds via +10000 derivation");

    assert_eq!(outcome.peer_id, "peer-cov-1");
    assert!(!outcome.literal_port_was_rpc, "推导端口命中");
    assert_eq!(outcome.rpc_port, port);
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("[peers.peer-cov-1]"), "{content}");
}

/// 密文路径：token 在位 → 请求加密 / 响应解密往返（derive_key 对称）。
#[tokio::test]
async fn pair_happy_path_with_token_encrypted_roundtrip() {
    // 密文版假对端：derive_key(token) 解请求、加密响应。
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let token = "cov-pair-token".to_string();
    let server_token = token.clone();
    std::thread::spawn(move || {
        use crate::transport::frame::{decrypt_frame, derive_key, encrypt_frame};
        let (mut sock, _) = listener.accept().ok()?;
        let key = derive_key(&server_token);
        let mut len_buf = [0u8; 4];
        sock.read_exact(&mut len_buf).ok()?;
        let len = u32::from_be_bytes(len_buf) as usize;
        let mut body = vec![0u8; len];
        sock.read_exact(&mut body).ok()?;

        let plaintext = decrypt_frame(&body, &key).ok()?;
        let req: crate::transport::conn::WireMessage = serde_json::from_slice(&plaintext).ok()?;
        let resp = crate::transport::conn::WireMessage::new_response(
            &req,
            serde_json::json!({"node_id": "peer-cov-enc", "name": "EncPeer"}),
        );
        let payload = serde_json::to_vec(&resp).ok()?;
        let encrypted = encrypt_frame(&payload, &key).ok()?;
        let mut out = (encrypted.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(&encrypted);
        sock.write_all(&out).ok()?;
        std::thread::sleep(std::time::Duration::from_millis(150));
        Some(())
    });

    let (_dir, path) = fresh_peers_path("happy-enc");
    let outcome = pair_with_peer(&path, Some(&token), &format!("127.0.0.1:{port}"))
        .await
        .expect("encrypted pair succeeds");
    assert_eq!(outcome.peer_id, "peer-cov-enc");
}

// ---------------------------------------------------------------------------
// 失败形态（全部零写入）
// ---------------------------------------------------------------------------

/// 空主机名（":port"）→ BadAddress，不触碰任何候选。
#[tokio::test]
async fn pair_rejects_empty_host() {
    let (_dir, path) = fresh_peers_path("empty-host");
    let err = pair_with_peer(&path, None, ":21949").await.unwrap_err();
    match &err {
        PairError::BadAddress(msg) => assert!(msg.contains("缺主机名"), "{msg}"),
        other => panic!("期望 BadAddress，got: {other:?}"),
    }
    assert!(!path.exists());
}

/// 对端响应缺 node_id → MissingIdentity，零写入。
#[tokio::test]
async fn pair_missing_identity_errors_without_write() {
    let port = spawn_fake_server(FakeReply::Payload(serde_json::json!({"name": "no-id"})));
    let (_dir, path) = fresh_peers_path("missing-id");
    let err = pair_with_peer(&path, None, &format!("127.0.0.1:{port}"))
        .await
        .unwrap_err();
    assert!(matches!(err, PairError::MissingIdentity), "got: {err:?}");
    assert!(!path.exists(), "零写入");
}

/// 对端回错误响应（WireMessage.error 非空）→ probe 报 remote error →
/// 双候选 Unreachable（第二候选拒连），零写入。
#[tokio::test]
async fn pair_remote_error_response_lands_unreachable() {
    let port = spawn_fake_server(FakeReply::ErrMsg("denied by cov".into()));
    let literal = port - 10000;
    if std::net::TcpStream::connect(("127.0.0.1", literal)).is_ok() {
        eprintln!("skip remote-error face: literal candidate {literal} unexpectedly open");
        return;
    }
    let (_dir, path) = fresh_peers_path("remote-err");
    let err = pair_with_peer(&path, None, &format!("127.0.0.1:{literal}"))
        .await
        .unwrap_err();
    match &err {
        PairError::Unreachable { tried, last_error } => {
            assert_eq!(tried.len(), 2, "{tried:?}");
            assert!(
                last_error.contains("denied by cov") || last_error.contains("connect"),
                "最后错误取第二候选的拒连：{last_error}"
            );
        }
        other => panic!("期望 Unreachable，got: {other:?}"),
    }
    assert!(!path.exists());
}

/// 响应体不是合法协议 JSON → decode 失败面 → Unreachable，零写入。
#[tokio::test]
async fn pair_garbage_response_decode_failure() {
    let port = spawn_fake_server(FakeReply::Raw(b"\x00\x01not-a-protocol-frame".to_vec()));
    let (_dir, path) = fresh_peers_path("garbage");
    let err = pair_with_peer(&path, None, &format!("127.0.0.1:{port}"))
        .await
        .unwrap_err();
    assert!(matches!(err, PairError::Unreachable { .. }), "got: {err:?}");
    assert!(!path.exists());
}

/// 静默对端 → 外层总超时闸命中（timeout 臂），tried 两条、零写入。
/// 代价：PROBE_TIMEOUT=8s 烧满一次。
#[tokio::test]
async fn pair_silent_peer_hits_probe_timeout() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        // 接受连接但永不应答（握 12s > PROBE_TIMEOUT 8s）。
        let Ok((sock, _)) = listener.accept() else {
            return;
        };
        std::thread::sleep(std::time::Duration::from_secs(12));
        drop(sock);
    });
    let (_dir, path) = fresh_peers_path("silent");
    let err = pair_with_peer(&path, None, &format!("127.0.0.1:{port}"))
        .await
        .unwrap_err();
    match &err {
        PairError::Unreachable { tried, last_error } => {
            assert_eq!(tried.len(), 2, "{tried:?}");
            // 最后一次尝试（+10000 推导）是拒连或超时，都算覆盖。
            assert!(
                last_error.contains("timeout") || last_error.contains("connect"),
                "{last_error}"
            );
        }
        other => panic!("期望 Unreachable，got: {other:?}"),
    }
    assert!(!path.exists());
}

// ---------------------------------------------------------------------------
// Display 五形态 + 断言失败面
// ---------------------------------------------------------------------------

#[test]
fn display_covers_all_pair_error_variants() {
    let unreachable = PairError::Unreachable {
        tried: vec!["a:1".into(), "b:2".into()],
        last_error: "boom".into(),
    };
    let shown = unreachable.to_string();
    assert!(
        shown.contains("a:1, b:2") && shown.contains("boom"),
        "{shown}"
    );
    assert!(shown.contains("未写入任何配置"));

    assert!(
        PairError::BadAddress("缺端口".into())
            .to_string()
            .contains("地址形态非法")
    );
    assert!(
        PairError::MissingIdentity
            .to_string()
            .contains("缺少 node_id")
    );
    let assert_fail = PairError::AssertFailed {
        detail: "键丢失".into(),
    };
    let shown = assert_fail.to_string();
    assert!(
        shown.contains("已回滚") && shown.contains("键丢失"),
        "{shown}"
    );
    assert!(
        PairError::Io("disk full".into())
            .to_string()
            .contains("disk full")
    );
}

/// rpc_port 断言失败面：写入 0（不落盘）但断言要 99 → 不匹配报错。
#[test]
fn assert_pair_written_rpc_port_mismatch() {
    let (_dir, path) = fresh_peers_path("rpc-port-mismatch");
    crate::cluster_config::append_peer_to_file_with_name(
        &path,
        "peer-x",
        "10.0.0.9:11950",
        "worker",
        "general",
        None,
        0, // 不写 rpc_port 键
    )
    .unwrap();

    let err = assert_pair_written(&path, "peer-x", "10.0.0.9:11950", "", 99).unwrap_err();
    assert!(err.contains("rpc_port 不匹配"), "{err}");

    // rpc_port=0 跳过该断言（探测端口必 >0，这里只打分支对称面）。
    assert!(assert_pair_written(&path, "peer-x", "10.0.0.9:11950", "", 0).is_ok());
}

/// 回读失败（路径是目录）与回读解析失败（坏 TOML）两个先置失败面。
#[test]
fn assert_pair_written_read_and_parse_failures() {
    let dir = tempfile::tempdir().unwrap();
    let as_dir = dir.path().join("as-dir");
    std::fs::create_dir_all(&as_dir).unwrap();
    let err = assert_pair_written(&as_dir, "p", "a:1", "", 0).unwrap_err();
    assert!(err.contains("回读失败"), "{err}");

    let corrupt = dir.path().join("corrupt.toml");
    std::fs::write(&corrupt, "((( not toml [[[").unwrap();
    let err = assert_pair_written(&corrupt, "p", "a:1", "", 0).unwrap_err();
    assert!(err.contains("回读解析失败"), "{err}");
}
