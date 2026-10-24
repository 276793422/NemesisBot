//! TrustBook 测试（QUIC 插件化 M3 自 endpoint/tests.rs 迁入——裁决语义
//! 已随 accept 闸搬 plugin_roaming，数据形态/裁决纯函数仍归宿主）。

use super::{TrustBook, TrustBookData};

#[test]
fn trust_book_decision_priority() {
    let book = TrustBook::from_data(TrustBookData {
        trusted: vec![
            format!("iroh:{}", "aa".repeat(32)),
            "BB".repeat(32).to_lowercase(),
        ],
        revoked: vec!["cc".repeat(32)],
    });
    // trusted 放行（带 iroh: 前缀 + 大小写归一）。
    assert!(book.is_trusted(&format!("iroh:{}", "AA".repeat(32))));
    assert!(book.is_trusted(&"bb".repeat(32)));
    // revoked 一票否决（即使再出现在 trusted 也拒——此处单测 revoked 形态）。
    assert!(!book.is_trusted(&"cc".repeat(32)));
    // 默认拒（allowlist 语义）。
    assert!(!book.is_trusted(&"dd".repeat(32)));
}

#[test]
fn trust_book_revoke_removes_from_trusted() {
    let mut book = TrustBook::from_data(TrustBookData {
        trusted: vec!["ee".repeat(32)],
        revoked: vec![],
    });
    assert!(book.is_trusted(&"ee".repeat(32)));
    book.revoke(&format!("iroh:{}", "ee".repeat(32)));
    assert!(!book.is_trusted(&"ee".repeat(32)), "吊销应立即生效");
    // revoked 留痕。
    assert_eq!(book.data().revoked.len(), 1);
    assert!(book.data().trusted.is_empty());
    // unrevoke 后仍不自动恢复信任（重新信任是显式动作）。
    book.unrevoke(&"ee".repeat(32));
    assert!(!book.is_trusted(&"ee".repeat(32)));
}

#[test]
fn trust_book_save_load_roundtrip() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    let mut book = TrustBook::from_data(TrustBookData::default());
    book.trust(&"aa".repeat(32));
    book.revoke(&"bb".repeat(32));
    book.save(ws).expect("save");

    // 文件存在 → load 返回 Some。
    let reloaded = TrustBook::load(ws).expect("load").expect("应已启用");
    assert!(reloaded.is_trusted(&"aa".repeat(32)));
    assert!(!reloaded.is_trusted(&"bb".repeat(32)));
}

#[test]
fn trust_book_absent_means_not_enabled() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let loaded = TrustBook::load(tmp.path()).expect("load 不应报错");
    assert!(loaded.is_none(), "文件不存在 = 未启用（None）");
}
