//! pair 模块测试（发现②/B6）。
//!
//! 网络探测路径需要真实对端——真机回归在 cluster-uat（T 系列双节点）。
//! 这里钉死零网络依赖的纯逻辑面：入参解析、双形态候选推导、写后回读
//! 断言（含篡改注入）、失败零写入/回滚语义、字面键 round-trip。

use super::*;
use std::path::PathBuf;

fn temp_peers_path(tag: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir
        .path()
        .join("cluster")
        .join(format!("peers-{}.toml", tag));
    (dir, path)
}

/// 预置一个合法 peers.toml（含 [node] 与一个既有 peer）。
fn seed_peers_file(path: &PathBuf) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        path,
        "[node]\nid = \"local-node-001\"\n\n[peers.existing]\naddress = \"10.0.0.1:11950\"\n",
    )
    .unwrap();
}

// -- 入参解析 ------------------------------------------------------------

#[tokio::test]
async fn test_pair_rejects_missing_port() {
    let (_dir, path) = temp_peers_path("no-port");
    let err = pair_with_peer(&path, None, "192.168.1.5")
        .await
        .unwrap_err();
    assert!(matches!(err, PairError::BadAddress(_)), "got: {err:?}");
    assert!(!path.exists(), "零写入：失败不得建文件");
}

#[tokio::test]
async fn test_pair_rejects_zero_port() {
    let (_dir, path) = temp_peers_path("zero-port");
    let err = pair_with_peer(&path, None, "192.168.1.5:0")
        .await
        .unwrap_err();
    assert!(matches!(err, PairError::BadAddress(_)), "got: {err:?}");
    assert!(!path.exists(), "零写入：失败不得建文件");
}

#[tokio::test]
async fn test_pair_rejects_bad_port() {
    let (_dir, path) = temp_peers_path("bad-port");
    let err = pair_with_peer(&path, None, "host.example:abc")
        .await
        .unwrap_err();
    assert!(matches!(err, PairError::BadAddress(_)), "got: {err:?}");
}

#[test]
fn test_parse_host_port_strict_ipv6_brackets() {
    let (host, port) = parse_host_port_strict("[::1]:21949").unwrap();
    assert_eq!(host, "::1");
    assert_eq!(port, 21949);
}

#[test]
fn test_parse_host_port_strict_plain() {
    let (host, port) = parse_host_port_strict("192.168.137.1:19411").unwrap();
    assert_eq!(host, "192.168.137.1");
    assert_eq!(port, 19411);
}

// -- 不可达：双形态全败零写入 ---------------------------------------------

#[tokio::test]
async fn test_pair_unreachable_reports_both_candidates_and_zero_write() {
    let (_dir, path) = seed_empty_path("unreachable");
    // 127.0.0.1 上保留端口段，双形态（字面/+10000）都不可达。
    let err = pair_with_peer(&path, None, "127.0.0.1:19990")
        .await
        .unwrap_err();
    match &err {
        PairError::Unreachable { tried, .. } => {
            assert_eq!(tried.len(), 2, "必须双形态都试过：{tried:?}");
            assert!(tried.contains(&"127.0.0.1:19990".to_string()), "{tried:?}");
            assert!(tried.contains(&"127.0.0.1:29990".to_string()), "{tried:?}");
        }
        other => panic!("期望 Unreachable，got: {other:?}"),
    }
    assert!(!path.exists(), "零写入：探测全败不得建文件");
}

fn seed_empty_path(tag: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir
        .path()
        .join("cluster")
        .join(format!("peers-{}.toml", tag));
    (dir, path)
}

// -- 写后回读断言（篡改注入：模拟序列化/并发改写出错）----------------------

/// 断言路径单测：直接构造写盘后篡改文件内容 → assert_pair_written 必须红。
#[test]
fn test_assert_pair_written_detects_missing_key() {
    let (_dir, path) = temp_peers_path("assert-missing");
    seed_peers_file(&path);
    // 文件里没有 peer-real 键
    let err = assert_pair_written(&path, "peer-real", "10.0.0.9:11950", "N", 0);
    assert!(err.is_err(), "键缺失必须红");
}

#[test]
fn test_assert_pair_written_detects_address_mismatch() {
    let (_dir, path) = temp_peers_path("assert-addr");
    seed_peers_file(&path);
    let content = std::fs::read_to_string(&path).unwrap();
    let tampered = content.replace(
        "[peers.existing]",
        "[peers.peer-real]\naddress = \"10.9.9.9:11950\"\n\n[peers.existing]",
    );
    std::fs::write(&path, tampered).unwrap();

    let err = assert_pair_written(&path, "peer-real", "10.0.0.9:11950", "N", 0);
    assert!(err.is_err(), "地址不一致必须红");
    let msg = err.unwrap_err();
    assert!(msg.contains("地址不匹配"), "报错要指向不一致点：{msg}");
}

#[test]
fn test_assert_pair_written_detects_name_mismatch() {
    let (_dir, path) = temp_peers_path("assert-name");
    seed_peers_file(&path);
    let content = std::fs::read_to_string(&path).unwrap();
    let tampered = content.replace(
        "[peers.existing]",
        "[peers.peer-real]\naddress = \"10.0.0.9:11950\"\nname = \"Wrong\"\n\n[peers.existing]",
    );
    std::fs::write(&path, tampered).unwrap();

    let err = assert_pair_written(&path, "peer-real", "10.0.0.9:11950", "Right", 0);
    assert!(err.is_err(), "name 不一致必须红");
}

#[test]
fn test_assert_pair_written_happy() {
    let (_dir, path) = temp_peers_path("assert-happy");
    seed_peers_file(&path);
    let content = std::fs::read_to_string(&path).unwrap();
    let tampered = content.replace(
        "[peers.existing]",
        "[peers.peer-real]\naddress = \"10.0.0.9:11950\"\nname = \"Right\"\n\n[peers.existing]",
    );
    std::fs::write(&path, tampered).unwrap();

    assert!(assert_pair_written(&path, "peer-real", "10.0.0.9:11950", "Right", 0).is_ok());
    // 无 name（写入时未带 name）也必须绿
    assert!(assert_pair_written(&path, "peer-real", "10.0.0.9:11950", "", 0).is_ok());
}

// -- 回滚语义（写盘后断言失败 → 文件恢复原样）------------------------------

#[test]
fn test_rollback_restores_pre_write_content() {
    let (_dir, path) = temp_peers_path("rollback");
    seed_peers_file(&path);
    let pre = std::fs::read_to_string(&path).unwrap();

    // 模拟：写入成功但内容被外部篡改（断言会红）→ 走回滚分支恢复 pre。
    let pre_content: Option<String> = Some(pre.clone());
    let write_and_fail_assert = || -> Result<(), PairError> {
        crate::cluster_config::append_peer_to_file_with_name(
            &path,
            "peer-x",
            "10.0.0.7:11950",
            "worker",
            "general",
            Some("X"),
            0,
        )
        .map_err(|e| PairError::Io(e.to_string()))?;
        // 篡改让断言必红（模拟「写盘链路出错」的最终形态）
        let content = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, content.replace("10.0.0.7", "10.8.8.8")).unwrap();
        if let Err(detail) = assert_pair_written(&path, "peer-x", "10.0.0.7:11950", "X", 0) {
            let _ = if let Some(c) = &pre_content {
                std::fs::write(&path, c).is_err()
            } else {
                std::fs::remove_file(&path).is_err()
            };
            return Err(PairError::AssertFailed { detail });
        }
        Ok(())
    };
    let err = write_and_fail_assert().unwrap_err();
    assert!(
        matches!(err, PairError::AssertFailed { .. }),
        "got: {err:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        pre,
        "回滚后文件必须恢复写前原样"
    );
}

// -- 字面键 round-trip（含 `.`/`:` id）——B3 联动 ---------------------------

#[test]
fn test_pair_written_key_is_literal_for_dotted_and_colon_ids() {
    // 直接走写盘 + 断言链（不依赖网络）：模拟 pair 对自定义 id 含 `.`/`:`
    // 的对端写入——表键必须字面保真（TOML 引号键），断言必须绿。
    let (_dir, path) = temp_peers_path("literal-keys");
    seed_peers_file(&path);

    for peer_id in ["node.a", "node:b", "node-c"] {
        crate::cluster_config::append_peer_to_file_with_name(
            &path,
            peer_id,
            "10.0.0.9:11950",
            "worker",
            "general",
            None,
            0,
        )
        .unwrap();
        assert!(
            assert_pair_written(&path, peer_id, "10.0.0.9:11950", "", 0).is_ok(),
            "字面键 {peer_id} 必须可回读命中"
        );
    }

    let content = std::fs::read_to_string(&path).unwrap();
    assert!(
        content.contains("\"node.a\""),
        "含 . 的键必须引号字面：{content}"
    );
    assert!(
        content.contains("\"node:b\""),
        "含 : 的键必须引号字面：{content}"
    );
    assert!(
        content.contains("[peers.node-c]"),
        "bare 键保持 bare：{content}"
    );
    assert!(
        !content.contains("node_a"),
        "不得再落 sanitize 有损键：{content}"
    );
}
