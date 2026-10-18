//! `RoamingConn` 帧桥测试（从 bridge.rs 内联迁移——内联测试纪律）。

use super::*;
use serde::{Deserialize, Serialize};
use tokio::io::duplex;

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Probe {
    msg: String,
    n: u32,
}

#[tokio::test]
async fn bridge_roundtrip_plain_and_encrypted() {
    // 两对 duplex：A↔B 双向各一对（duplex 单向流）。
    let (a_w, b_r) = duplex(64 * 1024);
    let (b_w, a_r) = duplex(64 * 1024);

    let mut a = RoamingConn::new(a_w, a_r, "shared-token");
    let mut b = RoamingConn::new(b_w, b_r, "shared-token");

    // A → B（加密帧）
    a.send_json(&Probe {
        msg: "hello roaming".into(),
        n: 7,
    })
    .await
    .unwrap();
    let got: Probe = b.recv_json().await.unwrap();
    assert_eq!(got.msg, "hello roaming");
    assert_eq!(got.n, 7);

    // B → A（对称）
    b.send_frame(b"raw-bytes").await.unwrap();
    let raw = a.recv_frame().await.unwrap();
    assert_eq!(raw, b"raw-bytes");
}

#[tokio::test]
async fn bridge_wrong_token_fails_not_garbage() {
    let (a_w, b_r) = duplex(64 * 1024);
    let (_b_w, a_r) = duplex(64 * 1024);
    let mut a = RoamingConn::new(a_w, a_r, "token-a");
    let mut b = RoamingConn::new(duplex(1024).0, b_r, "token-b");
    a.send_frame(b"secret").await.unwrap();
    // 密钥不一致 → GCM tag 校验失败 → 显式错误，非垃圾数据。
    let err = b.recv_frame().await.unwrap_err();
    assert!(err.contains("decrypt failed"), "got: {err}");
}
