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

// ============ B5 管理对称族（治理补全 P4，2026-10-19） ============

/// 最小合法清单模板（rest 型，无凭据槽位）。
const B5_MIN_MANIFEST: &str = r#"
[connector]
id = "b5-target"
name = "B5 对称命令测试件"
version = "0.1.0"
kind = "rest"

[connector.http]
base = "http://127.0.0.1:9"

[[op]]
id = "ping"
intent = ["连通性探测"]
risk = "low"
class = "network"
http = { method = "GET", path = "/ping" }
"#;

/// add 的 --file/--toml 单选解析：文件读取、内联文本、双提供/缺省拒绝。
#[test]
fn test_resolve_add_manifest_exclusive_sources() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let f = tmp.path().join("m.toml");
    std::fs::write(&f, B5_MIN_MANIFEST).expect("write manifest file");

    // --file 路径读入
    let text = resolve_add_manifest(&Some(f.clone()), &None).expect("file arm ok");
    assert_eq!(text, B5_MIN_MANIFEST);
    // --toml 内联
    let text = resolve_add_manifest(&None, &Some("x = 1".to_string())).expect("toml arm ok");
    assert_eq!(text, "x = 1");
    // 双提供拒绝
    assert!(resolve_add_manifest(&Some(f.clone()), &Some("y".into())).is_err());
    // 全缺拒绝
    assert!(resolve_add_manifest(&None, &None).is_err());
    // 文件不存在诚实报错
    let missing = tmp.path().join("nope.toml");
    let err = format!(
        "{}",
        resolve_add_manifest(&Some(missing), &None).expect_err("missing file rejects")
    );
    assert!(err.contains("nope.toml"), "错误点名文件: {err}");
}

/// enable/disable 执行核：add → disable → list 反映 → enable 回翻；sidecar
/// 文件落位；未知名诚实拒绝。id-or-dir 两形态都可用。
#[test]
fn test_run_connector_toggle_sidecar_roundtrip() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();

    // 先落一个 connector（走 add 同一 registry 函数）
    let c = nemesis_connectors::registry::create_connector(ws, "b5-target", B5_MIN_MANIFEST)
        .expect("create ok");
    assert!(c.enabled, "清单缺省 enabled = true");

    // 按目录名停用
    run_connector_toggle(ws, "b5-target", false).expect("disable ok");
    let sidecar = ws
        .join("config")
        .join("connector_state")
        .join("b5-target.json");
    assert!(sidecar.exists(), "sidecar 启停位文件落位");
    let disabled = nemesis_connectors::registry::scan_connectors(ws);
    let row = disabled
        .iter()
        .find(|l| {
            l.connector
                .as_ref()
                .map(|c| c.id == "b5-target")
                .unwrap_or(false)
        })
        .expect("entry present");
    assert!(
        !row.connector.as_ref().unwrap().enabled,
        "disable 后 scan 反映停用"
    );

    // 按 id 启用（id-or-dir 解析：id 命中换算目录名）
    run_connector_toggle(ws, "b5-target", true).expect("enable ok");
    let enabled = nemesis_connectors::registry::scan_connectors(ws);
    let row = enabled
        .iter()
        .find(|l| {
            l.connector
                .as_ref()
                .map(|c| c.id == "b5-target")
                .unwrap_or(false)
        })
        .expect("entry present");
    assert!(
        row.connector.as_ref().unwrap().enabled,
        "enable 后 scan 反映启用"
    );

    // 未知名诚实拒绝
    let err = format!(
        "{}",
        run_connector_toggle(ws, "ghost", true).expect_err("ghost rejects")
    );
    assert!(err.contains("ghost"), "错误点名: {err}");
}

/// dir ≠ id 的条目：按目录名翻位同样生效，sidecar 落 **id 键控**路径
/// （state::state_path 键是 manifest id，不是目录名）。
#[test]
fn test_run_connector_toggle_dir_name_differs_from_id() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    let manifest = B5_MIN_MANIFEST.replace("b5-target", "b5-id");
    nemesis_connectors::registry::create_connector(ws, "b5-dir", &manifest)
        .expect("create ok (dir=b5-dir, id=b5-id)");

    // 按目录名停用
    run_connector_toggle(ws, "b5-dir", false).expect("disable by dir ok");
    // sidecar 落 id 键控路径
    assert!(
        ws.join("config")
            .join("connector_state")
            .join("b5-id.json")
            .exists(),
        "sidecar 按 manifest id 键控落位"
    );
    assert!(
        !ws.join("config")
            .join("connector_state")
            .join("b5-dir.json")
            .exists(),
        "目录名键控的路径不该存在"
    );
    let disabled = nemesis_connectors::registry::scan_connectors(ws);
    let row = disabled
        .iter()
        .find(|l| {
            l.connector
                .as_ref()
                .map(|c| c.id == "b5-id")
                .unwrap_or(false)
        })
        .expect("entry present");
    assert!(!row.connector.as_ref().unwrap().enabled, "scan 反映停用");

    // 按目录名回翻
    run_connector_toggle(ws, "b5-dir", true).expect("enable by dir ok");
    let enabled = nemesis_connectors::registry::scan_connectors(ws);
    let row = enabled
        .iter()
        .find(|l| {
            l.connector
                .as_ref()
                .map(|c| c.id == "b5-id")
                .unwrap_or(false)
        })
        .expect("entry present");
    assert!(row.connector.as_ref().unwrap().enabled, "scan 反映启用");
}

/// remove：删目录、凭条保留（追加式流水语义）、在册外名字诚实拒绝。
#[test]
fn test_remove_connector_preserves_receipts() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    nemesis_connectors::registry::create_connector(ws, "b5-target", B5_MIN_MANIFEST)
        .expect("create ok");
    // 手写一条凭条（append-only 面，不走执行核）
    let r = nemesis_connectors::receipt::Receipt {
        receipt_id: "RCPT-1000-0001".into(),
        ts: "2026-10-19T08:00:00+00:00".into(),
        connector: "b5-target".into(),
        op: "ping".into(),
        risk: "low".into(),
        class: "network".into(),
        outcome: "executed".into(),
        args_digest: "0123456789abcdef".into(),
        summary: "B5 测试凭条".into(),
        receipt_key: None,
        receipt_value: None,
        budget_consumed: None,
        approval_ref: None,
        session_key: None,
        undoable: false,
    };
    nemesis_connectors::receipt::append_receipt(ws, &r).expect("receipt ok");

    // 在册外名字先拒
    assert!(nemesis_connectors::registry::remove_connector(ws, "ghost").is_err());

    // 删除在册条目
    nemesis_connectors::registry::remove_connector(ws, "b5-target").expect("remove ok");
    assert!(
        !ws.join("connectors").join("b5-target").exists(),
        "目录已删"
    );
    let receipts =
        nemesis_connectors::receipt::list_receipts(ws, 50, Some("b5-target")).expect("receipts");
    assert_eq!(receipts.len(), 1, "凭条历史保留（不抹账）");
}
