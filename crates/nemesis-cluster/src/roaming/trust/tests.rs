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
        ..Default::default()
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
        ..Default::default()
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

// ---------------------------------------------------------------------------
// L16（2026-11-02 三期）：announce 采信闸 + dial 吊销 veto
// ---------------------------------------------------------------------------

use super::{AnnounceVerdict, announce_gate, dial_vetoed};

/// 簿缺席 = regime 未激活：Adopt 且**不建文件**（announce 绝不激活 allowlist）。
#[test]
fn announce_gate_absent_book_adopts_without_creating_file() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let hex = "aa".repeat(32);
    assert_eq!(
        announce_gate(tmp.path(), &hex),
        AnnounceVerdict::Adopt,
        "簿缺席 = 未启用 = 采信"
    );
    assert!(
        !super::trust_book_path(tmp.path()).exists(),
        "announce 闸不得创建 trust.toml（regime 激活唯一路径 = 管理面显式写簿）"
    );
}

/// revoked 一票否决：整条不采信。
#[test]
fn announce_gate_revoked_vetoes() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut book = TrustBook::from_data(TrustBookData::default());
    book.revoke(&"bb".repeat(32));
    book.save(tmp.path()).expect("save");
    assert_eq!(
        announce_gate(tmp.path(), &format!("iroh:{}", "BB".repeat(32))),
        AnnounceVerdict::Veto("revoked"),
        "revoked veto（iroh: 前缀 + 大写归一命中）"
    );
}

/// trusted 命中：采信。
#[test]
fn announce_gate_trusted_adopts() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut book = TrustBook::from_data(TrustBookData::default());
    book.trust(&"cc".repeat(32));
    book.save(tmp.path()).expect("save");
    assert_eq!(
        announce_gate(tmp.path(), &"cc".repeat(32)),
        AnnounceVerdict::Adopt
    );
}

/// unknown → TOFU 首见自动钉存：采信 + 盘上回读 trusted 含 hex。
#[test]
fn announce_gate_unknown_tofu_pins_and_adopts() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut book = TrustBook::from_data(TrustBookData::default());
    book.trust(&"dd".repeat(32));
    book.save(tmp.path()).expect("save");

    let newcomer = "ee".repeat(32);
    assert_eq!(
        announce_gate(tmp.path(), &newcomer),
        AnnounceVerdict::Adopt,
        "首见 = TOFU 采信"
    );
    let reloaded = TrustBook::load(tmp.path()).expect("load").expect("簿已在");
    assert!(
        reloaded.is_trusted(&newcomer),
        "TOFU 钉存必须落盘: {:?}",
        reloaded.data()
    );
    assert!(
        reloaded.is_trusted(&"dd".repeat(32)),
        "既有 trusted 条目不受钉存影响"
    );
}

/// 读失败 fail-closed：激活态下坏簿不采信任何 announce。
#[test]
fn announce_gate_corrupt_book_fails_closed() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = super::trust_book_path(tmp.path());
    std::fs::create_dir_all(path.parent().unwrap()).expect("mkdir");
    std::fs::write(&path, "{{ not toml").expect("write");
    assert_eq!(
        announce_gate(tmp.path(), &"ff".repeat(32)),
        AnnounceVerdict::Veto("trust_book_read_failed"),
        "坏簿 fail-closed veto（与插件 accept 闸同方向）"
    );
}

/// dial veto 三态：簿缺席 false / revoked true / trusted+unknown false。
#[test]
fn dial_veto_only_blocks_revoked() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let hex = "aa".repeat(32);
    // 簿缺席 = 未启用 = 不 veto。
    assert!(!dial_vetoed(tmp.path(), &hex));

    let mut book = TrustBook::from_data(TrustBookData::default());
    book.trust(&"bb".repeat(32));
    book.revoke(&hex);
    book.save(tmp.path()).expect("save");
    assert!(
        dial_vetoed(tmp.path(), &format!("iroh:{}", hex)),
        "revoked veto"
    );
    assert!(
        !dial_vetoed(tmp.path(), &"bb".repeat(32)),
        "trusted 不 veto"
    );
    assert!(
        !dial_vetoed(tmp.path(), &"cc".repeat(32)),
        "unknown 不 veto"
    );
}

/// dial veto 读失败 fail-open（方向性刻意：坏簿不瘫痪本节点出站漫游——
/// 对端 accept 闸守门；与 announce 闸 fail-closed 相反是设计而非笔误）。
#[test]
fn dial_veto_corrupt_book_fails_open() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = super::trust_book_path(tmp.path());
    std::fs::create_dir_all(path.parent().unwrap()).expect("mkdir");
    std::fs::write(&path, "{{ not toml").expect("write");
    assert!(!dial_vetoed(tmp.path(), &"dd".repeat(32)));
}

// ---------------------------------------------------------------------------
// L13-a（2026-11-03 三期）：mandate 公钥钉存面
// ---------------------------------------------------------------------------

use super::{
    MandateKeyVerdict, MandatePinOutcome, mandate_key_state, mandate_pin_forced, mandate_revoke,
    mandate_tofu_pin, mandate_unrevoke, normalize_mandate_pubkey,
};

/// 合法 uncompressed 130 hex 测试钥（04 头 + tag + 126 hex）。
fn pk_uncompressed(tag: u8) -> String {
    format!("04{:02x}{}", tag, "ab".repeat(63))
}

/// 合法 compressed 66 hex 测试钥（02 头 + tag + 62 hex）。
fn pk_compressed(tag: u8) -> String {
    format!("02{:02x}{}", tag, "cd".repeat(31))
}

#[test]
fn normalize_pubkey_accepts_valid_forms_and_lowercases() {
    let upper = format!("04AB{}", "AB".repeat(63));
    assert_eq!(
        normalize_mandate_pubkey(&upper),
        Some(pk_uncompressed(0xab)),
        "130 hex 大写归一小写"
    );
    assert_eq!(
        normalize_mandate_pubkey(&format!("  {}  ", pk_compressed(1))),
        Some(pk_compressed(1)),
        "66 hex + trim"
    );
    // 03 头 compressed 同法。
    let c3 = format!("03{}", "cd".repeat(32));
    assert_eq!(normalize_mandate_pubkey(&c3), Some(c3));
}

#[test]
fn normalize_pubkey_rejects_malformed() {
    assert_eq!(normalize_mandate_pubkey(""), None, "空串");
    assert_eq!(normalize_mandate_pubkey("zz"), None, "非 hex");
    assert_eq!(
        normalize_mandate_pubkey(&"04".repeat(32)),
        None,
        "64 hex 长度非法"
    );
    assert_eq!(
        normalize_mandate_pubkey(&format!("05{}", "ab".repeat(64))),
        None,
        "130 hex 但头字节非法"
    );
    assert_eq!(
        normalize_mandate_pubkey(&format!("0a{}", "cd".repeat(32))),
        None,
        "66 hex 但头字节非法"
    );
    assert_eq!(
        normalize_mandate_pubkey(&format!("04{}", "zz".repeat(32))),
        None,
        "长度对但含非 hex"
    );
}

#[test]
fn mandate_tofu_pins_first_sighting() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let out = mandate_tofu_pin(tmp.path(), "Node-A", &pk_uncompressed(1)).expect("pin");
    assert_eq!(out, MandatePinOutcome::BookAbsent, "簿缺席不建文件不钉存");
    assert!(
        !super::trust_book_path(tmp.path()).exists(),
        "announce 抵达绝不激活 trust.toml（L16 纪律镜像）"
    );
    assert_eq!(
        mandate_key_state(tmp.path(), "node-a"),
        MandateKeyVerdict::Absent,
        "簿缺席 = Absent"
    );
}

#[test]
fn mandate_tofu_pins_into_active_book() {
    let tmp = tempfile::tempdir().expect("tempdir");
    // 激活簿（L16 管理面先例：显式写簿）。
    let mut book = TrustBook::from_data(TrustBookData::default());
    book.trust(&"aa".repeat(32));
    book.save(tmp.path()).expect("save");

    let out = mandate_tofu_pin(tmp.path(), "Node-A", &pk_uncompressed(1)).expect("pin");
    assert_eq!(out, MandatePinOutcome::Pinned, "首见钉存");
    assert_eq!(
        mandate_key_state(tmp.path(), "node-a"),
        MandateKeyVerdict::Pinned(pk_uncompressed(1)),
        "归一 node_id（小写）查得 pinned"
    );

    // 同值幂等。
    let out = mandate_tofu_pin(tmp.path(), "node-a", &pk_uncompressed(1)).expect("pin");
    assert_eq!(out, MandatePinOutcome::Unchanged);

    // 异值 Conflict 保留首见。
    let out = mandate_tofu_pin(tmp.path(), "node-a", &pk_uncompressed(2)).expect("pin");
    assert_eq!(
        out,
        MandatePinOutcome::Conflict {
            existing: pk_uncompressed(1)
        },
        "strict TOFU：保留首见"
    );
    assert_eq!(
        mandate_key_state(tmp.path(), "node-a"),
        MandateKeyVerdict::Pinned(pk_uncompressed(1)),
        "冲突后 pin 不变"
    );
}

#[test]
fn mandate_tofu_skips_revoked_node() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let book = TrustBook::from_data(TrustBookData::default());
    book.save(tmp.path()).expect("save 激活簿");
    mandate_pin_forced(tmp.path(), "Node-A", &pk_uncompressed(1)).expect("pin");
    mandate_revoke(tmp.path(), "node-a").expect("revoke");

    let out = mandate_tofu_pin(tmp.path(), "node-a", &pk_uncompressed(2)).expect("pin");
    assert_eq!(out, MandatePinOutcome::Distrusted, "吊销态 TOFU 不更新");
    assert_eq!(
        mandate_key_state(tmp.path(), "node-a"),
        MandateKeyVerdict::Revoked,
        "revoked 优先于 keys"
    );
    // 盘上 keys 保留留痕。
    let reloaded = TrustBook::load(tmp.path()).expect("load").expect("some");
    assert_eq!(
        reloaded
            .data()
            .mandate_keys
            .get("node-a")
            .map(String::as_str),
        Some(pk_uncompressed(1).as_str()),
        "吊销留痕：keys 不删"
    );
}

#[test]
fn mandate_forced_pin_overrides_and_rotates() {
    let tmp = tempfile::tempdir().expect("tempdir");
    // 簿缺席：管理面写簿激活（同 trust-add 先例）。
    let out = mandate_pin_forced(tmp.path(), "Node-A", &pk_uncompressed(1)).expect("pin");
    assert_eq!(out, MandatePinOutcome::Pinned);
    assert!(
        super::trust_book_path(tmp.path()).exists(),
        "显式 pin 激活簿"
    );

    // 轮换：forced 覆盖。
    let out = mandate_pin_forced(tmp.path(), "node-a", &pk_uncompressed(2)).expect("pin");
    assert_eq!(out, MandatePinOutcome::Pinned, "forced 覆盖返回 Pinned");
    assert_eq!(
        mandate_key_state(tmp.path(), "node-a"),
        MandateKeyVerdict::Pinned(pk_uncompressed(2))
    );

    // 非法形态 loud 拒绝。
    assert!(mandate_pin_forced(tmp.path(), "node-a", "zz").is_err());
    assert!(mandate_tofu_pin(tmp.path(), "node-a", "short").is_err());
}

#[test]
fn mandate_revoke_unrevoke_lifecycle() {
    let tmp = tempfile::tempdir().expect("tempdir");
    mandate_pin_forced(tmp.path(), "Node-A", &pk_uncompressed(1)).expect("pin");

    // 簿缺席臂：无簿吊销诚实报错。
    let tmp2 = tempfile::tempdir().expect("tempdir");
    assert!(mandate_revoke(tmp2.path(), "node-a").is_err(), "无簿不吊销");

    mandate_revoke(tmp.path(), "node-a").expect("revoke");
    assert_eq!(
        mandate_key_state(tmp.path(), "node-a"),
        MandateKeyVerdict::Revoked
    );
    // 幂等。
    mandate_revoke(tmp.path(), "node-a").expect("revoke 幂等");
    assert_eq!(
        mandate_key_state(tmp.path(), "node-a"),
        MandateKeyVerdict::Revoked
    );

    // unrevoke：pin 保留，验签态恢复（吊销只加标记语义）。
    mandate_unrevoke(tmp.path(), "node-a").expect("unrevoke");
    assert_eq!(
        mandate_key_state(tmp.path(), "node-a"),
        MandateKeyVerdict::Pinned(pk_uncompressed(1)),
        "unrevoke 后 pin 恢复生效"
    );
    // 幂等 + 无对象臂。
    mandate_unrevoke(tmp.path(), "node-a").expect("unrevoke 幂等");
    assert!(mandate_unrevoke(tmp2.path(), "node-a").is_err());
}

#[test]
fn mandate_legacy_book_without_sections_loads_empty() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = super::trust_book_path(tmp.path());
    std::fs::create_dir_all(path.parent().unwrap()).expect("mkdir");
    // L16-only 存量簿（无 mandate 段）。
    std::fs::write(&path, "trusted = [\"aabb\"]\nrevoked = []\n").expect("write");
    let book = TrustBook::load(tmp.path()).expect("load").expect("some");
    assert!(book.data().mandate_keys.is_empty(), "缺段 = 空 map 零迁移");
    assert!(book.data().mandate_revoked.is_empty());
    assert_eq!(
        mandate_key_state(tmp.path(), "any-node"),
        MandateKeyVerdict::Absent
    );
}

/// BUG-20261104-358：簿在场但读失败 = BookUnreadable（与 Absent 分立——
/// 旧实现把 Err 归入 Absent，verify 降级臂上线后坏簿 = 已激活 regime 被
/// 无声降级为放行态）。
#[test]
fn mandate_corrupt_book_verdict_is_book_unreadable() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = super::trust_book_path(tmp.path());
    std::fs::create_dir_all(path.parent().unwrap()).expect("mkdir");
    std::fs::write(&path, "{{ not toml").expect("write");
    assert_eq!(
        mandate_key_state(tmp.path(), "node-a"),
        MandateKeyVerdict::BookUnreadable,
        "坏簿 ≠ 簿缺席：fail-closed 专用态（announce 闸同方向）"
    );
    // 簿缺席仍是 Absent（降级臂的合法输入）。
    let tmp2 = tempfile::tempdir().expect("tempdir");
    assert_eq!(
        mandate_key_state(tmp2.path(), "node-a"),
        MandateKeyVerdict::Absent
    );
}

/// BUG-20261031-260 横向补齐：trust.toml 带 UTF-8 BOM（记事本形态）不再
/// 构成坏簿——剥 BOM 后正常解析（合法内容照常钉存生效）。
#[test]
fn mandate_bom_book_loads_normally() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = super::trust_book_path(tmp.path());
    std::fs::create_dir_all(path.parent().unwrap()).expect("mkdir");
    let body = format!(
        "trusted = []\nrevoked = []\n\n[mandate_keys]\n\"node-a\" = \"{}\"\n",
        pk_uncompressed(1)
    );
    let mut bom = String::from("\u{feff}");
    bom.push_str(&body);
    std::fs::write(&path, bom).expect("write");
    assert_eq!(
        mandate_key_state(tmp.path(), "node-a"),
        MandateKeyVerdict::Pinned(pk_uncompressed(1)),
        "BOM 簿剥前缀后正常解析"
    );
}

#[test]
fn mandate_pins_and_iroh_sections_coexist() {
    let tmp = tempfile::tempdir().expect("tempdir");
    mandate_pin_forced(tmp.path(), "node-a", &pk_uncompressed(1)).expect("pin a");
    mandate_pin_forced(tmp.path(), "node-b", &pk_compressed(1)).expect("pin b");
    // 回读两 pin 并存（整簿替换不丢段）。
    assert_eq!(
        mandate_key_state(tmp.path(), "node-a"),
        MandateKeyVerdict::Pinned(pk_uncompressed(1))
    );
    assert_eq!(
        mandate_key_state(tmp.path(), "node-b"),
        MandateKeyVerdict::Pinned(pk_compressed(1))
    );
    // iroh 段与 mandate 段同簿并存：iroh 段写入不冲掉 mandate 段。
    let mut reloaded = TrustBook::load(tmp.path()).expect("load").expect("some");
    reloaded.trust(&"aa".repeat(32));
    reloaded.save(tmp.path()).expect("save");
    assert!(
        TrustBook::load(tmp.path())
            .expect("load")
            .expect("some")
            .is_trusted(&"aa".repeat(32))
    );
    assert_eq!(
        mandate_key_state(tmp.path(), "node-a"),
        MandateKeyVerdict::Pinned(pk_uncompressed(1)),
        "iroh 段写入不冲掉 mandate 段"
    );
}
