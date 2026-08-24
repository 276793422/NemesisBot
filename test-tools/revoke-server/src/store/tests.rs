use super::*;

fn store() -> SqliteStore {
    SqliteStore::open_memory().unwrap()
}

#[test]
fn crl_add_and_query() {
    let s = store();
    let entry = CrlEntry {
        dim: RevDim::KeyFp,
        value: "abc".into(),
        revoked_at: 100,
        reason: "leak".into(),
    };
    let ver = s.add_revoke(entry.clone()).unwrap();
    assert_eq!(ver, 2); // 初始 version=1，add 后 2
    // 查
    let hit = s.query_revoke(RevDim::KeyFp, "abc").unwrap().unwrap();
    assert_eq!(hit.value, "abc");
    assert_eq!(hit.reason, "leak");
    // 不命中
    assert!(s.query_revoke(RevDim::KeyFp, "none").unwrap().is_none());
    // 列表
    let crl = s.list_crl().unwrap();
    assert_eq!(crl.version, 2);
    assert_eq!(crl.entries.len(), 1);
}

#[test]
fn trusted_keys_upsert_list() {
    let s = store();
    let k = TrustedKey {
        key_fp: "ff".into(),
        status: KeyStatus::Active,
        not_after: None,
    };
    let v1 = s.upsert_trusted_key(k.clone()).unwrap();
    assert_eq!(v1, 2);
    // 更新同 key_fp → version 再 +1
    let k2 = TrustedKey {
        key_fp: "ff".into(),
        status: KeyStatus::Revoked,
        not_after: None,
    };
    let v2 = s.upsert_trusted_key(k2).unwrap();
    assert_eq!(v2, 3);
    let tkl = s.list_trusted_keys().unwrap();
    assert_eq!(tkl.keys.len(), 1);
    assert_eq!(tkl.keys[0].status, KeyStatus::Revoked);
}

#[test]
fn audit_append_and_list() {
    let s = store();
    s.add_audit(AuditRecord {
        id: 0,
        timestamp: 1,
        action: "revoke".into(),
        operator: "tester".into(),
        dim: Some("key_id".into()),
        value: Some("abc".into()),
        reason: Some("leak".into()),
        detail: None,
    })
    .unwrap();
    s.add_audit(AuditRecord {
        id: 0,
        timestamp: 2,
        action: "trust_upsert".into(),
        operator: "tester".into(),
        dim: None,
        value: None,
        reason: None,
        detail: None,
    })
    .unwrap();
    let list = s.list_audit(10).unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].action, "trust_upsert"); // DESC → 最新在前
    assert_eq!(list[1].action, "revoke");
}
