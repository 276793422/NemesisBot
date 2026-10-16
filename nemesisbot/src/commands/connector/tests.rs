//! connector CLI 单测（独立测试文件——内联测试禁令）。
//!
//! 覆盖：30 秒四承诺演示证据面（幕 1 凭据铁律零泄露 + error 回执 /
//! 幕 2 审批引用凭条 + order_id / 幕 3 撤单诚实报错 / 幕 4 账本收尾）
//! + paper-broker 模板可解析契约（include_str! 单一真相源防漂移）。

use super::*;

#[test]
fn test_demo_four_acts_evidence() {
    // demo_acts 就地设 NEMESISBOT_VAULT_PASSPHRASE（argon2 读侧解锁）——env 是
    // 进程单例，与 vault_runtime / commands::vault 的 env 触及测试群经
    // crate::GLOBAL_STATE_LOCK 互斥（nextest 独立进程下无竞争直通）。std 锁
    // guard 不得活过 await（clippy await_holding_lock）——锁在同步外壳持有，
    // async 体经 block_on 执行（commands/vault run_arms 同款形态）。
    let _env_guard = crate::GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(test_demo_four_acts_evidence_inner());
}

async fn test_demo_four_acts_evidence_inner() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ev = demo_acts(tmp.path()).await.expect("demo acts ok");

    // 幕 1：凭据注入后 127.0.0.1:9 connect refused 诚实报错 + 零泄露
    // （无 security 构建幕 1 诚实跳过：strict_error=跳过文案、扫描零文件、
    // 零命中平凡成立——覆盖文件数断言随 security 同闸，与下方
    // strict_receipt_outcome 断言同款先例）。
    assert!(!ev.strict_error.is_empty(), "幕1 应有诚实报错");
    #[cfg(feature = "security")]
    assert!(ev.leak_files_scanned > 0, "泄露扫描应覆盖文件");
    assert_eq!(ev.leak_hits, 0, "凭据铁律：密钥字节零命中");
    #[cfg(feature = "security")]
    assert_eq!(
        ev.strict_receipt_outcome.as_deref(),
        Some("error"),
        "错误也是账：demo-strict 应有 error 回执"
    );

    // 幕 2：executed 凭条带审批引用凭条 + 业务主键
    assert_eq!(
        ev.approval_ref.as_deref(),
        Some("appr-demo-42"),
        "审批引用凭条应原样进回执"
    );
    let oid = ev.order_id.as_deref().expect("order_id 应回填");
    assert!(oid.starts_with("NB-ORD-"), "order_id 形态: {oid}");

    // 幕 3：撤单诚实报错（v1 即时成交无挂单）
    assert!(!ev.cancel_error.is_empty(), "撤单应诚实报错而非装成功");

    // 幕 4：账本收尾（现金 1_000_000 - 1000；订单 1 笔）
    assert_eq!(ev.ledger_orders, 1);
    assert!((ev.ledger_cash - 999_000.0).abs() < 1e-9);

    // 回执面补查：place_order 凭条 risk/class/主键 + 幕 3 error 回执在册
    let ws = tmp.path();
    let receipts = nemesis_connectors::receipt::list_receipts(ws, 50, None).expect("receipts");
    let placed = receipts
        .iter()
        .find(|r| r.op == "place_order" && r.outcome == "executed")
        .expect("place_order executed 凭条");
    assert_eq!(placed.risk, "critical");
    assert_eq!(placed.class, "write");
    assert_eq!(placed.receipt_key.as_deref(), Some("order_id"));
    assert!(
        receipts
            .iter()
            .any(|r| r.op == "cancel_order" && r.outcome == "error"),
        "撤单失败也应落 error 回执"
    );
}

/// 模板契约：paper-broker 模板能被 include_str! 原样解析（demo 与 workspace
/// 同源——模板坏了这里先红）。
#[test]
fn test_paper_broker_template_parses() {
    let c = nemesis_connectors::manifest::parse_connector_manifest(PAPER_BROKER_TEMPLATE)
        .expect("模板应可解析");
    assert_eq!(c.id, "paper-broker");
    assert!(
        c.ops.iter().any(|o| o.id.as_str() == "place_order"),
        "place_order op 应在册"
    );
    assert!(
        c.ops.iter().any(|o| o.id.as_str() == "cancel_order"),
        "cancel_order op 应在册"
    );
}

// ---- D6.1 签名信任族 CLI 辅助（validate_hex64 / resolve_trust_entry） ----

#[test]
fn test_validate_hex64_discriminates() {
    let good = "a".repeat(64);
    assert!(validate_hex64(&good, "公钥").is_ok());
    assert!(validate_hex64("zz", "公钥").is_err(), "非 hex 拒");
    assert!(validate_hex64(&"a".repeat(63), "公钥").is_err(), "63 位拒");
    assert!(
        validate_hex64(&"g".repeat(64), "公钥").is_err(),
        "g 不是 hex"
    );
}

#[test]
fn test_resolve_trust_entry_key_name_variants() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let store = nemesis_connectors::signing::workspace_trust_store(tmp.path());
    let pair = nemesis_connectors::signing::generate_key_pair().expect("keygen");
    store.add_key(&pair.public_key, "signer-a", {
        use nemesis_connectors::signing::TrustLevel;
        TrustLevel::Verified
    });

    // 按 key / 按 name / 两者一致 → Ok
    let by_key =
        resolve_trust_entry(&store, &Some(pair.public_key.clone()), &None).expect("按 key 解析");
    assert_eq!(by_key.name, "signer-a");
    let by_name =
        resolve_trust_entry(&store, &None, &Some("signer-a".into())).expect("按 name 解析");
    assert_eq!(by_name.public_key, pair.public_key);
    let both = resolve_trust_entry(
        &store,
        &Some(pair.public_key.clone()),
        &Some("signer-a".into()),
    )
    .expect("两者一致");
    assert_eq!(both.public_key, pair.public_key);

    // 两者不一致 / 不在库 / 缺一 → Err
    assert!(
        resolve_trust_entry(
            &store,
            &Some(pair.public_key.clone()),
            &Some("别的名".into())
        )
        .is_err()
    );
    assert!(resolve_trust_entry(&store, &Some("a".repeat(64)), &None).is_err());
    assert!(resolve_trust_entry(&store, &None, &None).is_err());
}

#[test]
fn test_trust_revoke_readback_flow() {
    // CLI TrustRevoke 臂的核（resolve → revoke → 盘上真相回读 Revoked）。
    let tmp = tempfile::tempdir().expect("tempdir");
    let store = nemesis_connectors::signing::workspace_trust_store(tmp.path());
    let pair = nemesis_connectors::signing::generate_key_pair().expect("keygen");
    store.add_key(&pair.public_key, "signer-b", {
        use nemesis_connectors::signing::TrustLevel;
        TrustLevel::Community
    });
    let entry = resolve_trust_entry(&store, &None, &Some("signer-b".into())).expect("resolve");
    store
        .revoke_key_by_public_key(&entry.public_key)
        .expect("revoke");
    // 全新 store 实例读盘（持久化真相，非内存态）。
    let persisted = nemesis_connectors::signing::workspace_trust_store(tmp.path())
        .get_key(&entry.public_key)
        .map(|k| k.level);
    assert!(matches!(
        persisted,
        Some(nemesis_connectors::signing::TrustLevel::Revoked)
    ));
}
