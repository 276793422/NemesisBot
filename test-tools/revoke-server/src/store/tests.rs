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

// ===================== 2026-08-25 补测：users / issuers / signatures / meta / 文件 DB =====================

use std::sync::atomic::{AtomicU32, Ordering};

static FILE_SEQ: AtomicU32 = AtomicU32::new(0);

/// 唯一临时 DB 路径（pid + 序号防并行冲突；调用方负责删除）。
fn temp_db_path(tag: &str) -> String {
    let n = FILE_SEQ.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir()
        .join(format!("revoke_store_{tag}_{}_{}.db", std::process::id(), n))
        .to_string_lossy()
        .into_owned()
}

fn issuer_rec(name: &str, created_at: u64) -> IssuerRecord {
    IssuerRecord {
        name: name.to_string(),
        issuer_sk: "11".repeat(32),
        issuer_pub: "22".repeat(32),
        issuer_cert: "33".repeat(60),
        chain: "44".repeat(120),
        created_at,
    }
}

fn sig_rec(sig_hash: &str, registered_at: u64) -> SignatureRecord {
    SignatureRecord {
        sig_hash: sig_hash.to_string(),
        key_fp: format!("fp-{sig_hash}"),
        publisher: Some(format!("pub-{sig_hash}")),
        signed_at: registered_at - 10,
        content_hash: format!("ch-{sig_hash}"),
        user_name: Some("tester".to_string()),
        issuer_name: Some("default".to_string()),
        registered_at,
    }
}

#[test]
fn users_add_get_list_and_replace() {
    let s = store();
    s.add_user("tok-old", "alice", Some("alice-pub"), "default", 100)
        .unwrap();
    s.add_user("tok-new", "bob", None, "default", 200).unwrap();
    // 命中：全字段回读
    let u = s.get_user_by_token("tok-old").unwrap().unwrap();
    assert_eq!(u.name, "alice");
    assert_eq!(u.publisher.as_deref(), Some("alice-pub"));
    assert!(u.active);
    assert_eq!(u.created_at, 100);
    assert_eq!(u.issuer_name, "default");
    // 未命中
    assert!(s.get_user_by_token("nope").unwrap().is_none());
    // 列表按 created_at DESC
    let list = s.list_users().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].name, "bob");
    assert_eq!(list[1].name, "alice");
    // 同 token REPLACE → 覆盖字段、不增行（publisher NULL → None 回读）
    s.add_user("tok-old", "alice2", None, "acme", 300).unwrap();
    assert_eq!(s.list_users().unwrap().len(), 2);
    let u = s.get_user_by_token("tok-old").unwrap().unwrap();
    assert_eq!(u.name, "alice2");
    assert_eq!(u.publisher, None);
    assert_eq!(u.issuer_name, "acme");
    assert_eq!(u.created_at, 300);
}

#[test]
fn users_inactive_token_not_returned() {
    let s = store();
    s.add_user("tok-a", "alice", None, "default", 1).unwrap();
    // 白盒：users.active 只能经 SQL 置 0（add_user 硬编码 1），验证 WHERE active=1 过滤
    {
        let conn = s.conn.lock();
        conn.execute("UPDATE users SET active=0 WHERE token='tok-a'", [])
            .unwrap();
    }
    assert!(s.get_user_by_token("tok-a").unwrap().is_none());
}

#[test]
fn issuers_add_get_list_and_replace() {
    let s = store();
    s.add_issuer(&issuer_rec("acme", 100)).unwrap();
    s.add_issuer(&issuer_rec("beta", 200)).unwrap();
    // 命中：全字段回读（hex 串原样存取）
    let rec = s.get_issuer_by_name("acme").unwrap().unwrap();
    assert_eq!(rec.name, "acme");
    assert_eq!(rec.issuer_sk, "11".repeat(32));
    assert_eq!(rec.issuer_pub, "22".repeat(32));
    assert_eq!(rec.issuer_cert, "33".repeat(60));
    assert_eq!(rec.chain, "44".repeat(120));
    assert_eq!(rec.created_at, 100);
    // 未命中
    assert!(s.get_issuer_by_name("ghost").unwrap().is_none());
    // 列表按 created_at DESC
    let list = s.list_issuers().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].name, "beta");
    assert_eq!(list[1].name, "acme");
    // 同名 REPLACE → 覆盖不增行
    let mut r = issuer_rec("acme", 300);
    r.issuer_pub = "99".repeat(32);
    s.add_issuer(&r).unwrap();
    assert_eq!(s.list_issuers().unwrap().len(), 2);
    assert_eq!(
        s.get_issuer_by_name("acme").unwrap().unwrap().issuer_pub,
        "99".repeat(32)
    );
}

#[test]
fn signatures_add_list_limit_and_replace() {
    let s = store();
    s.add_signature(&sig_rec("s1", 100)).unwrap();
    s.add_signature(&sig_rec("s2", 200)).unwrap();
    s.add_signature(&sig_rec("s3", 300)).unwrap();
    // 全量：registered_at DESC
    let all = s.list_signatures(200).unwrap();
    assert_eq!(all.len(), 3);
    assert_eq!(all[0].sig_hash, "s3");
    assert_eq!(all[1].sig_hash, "s2");
    assert_eq!(all[2].sig_hash, "s1");
    // limit 生效
    let two = s.list_signatures(2).unwrap();
    assert_eq!(two.len(), 2);
    assert_eq!(two[0].sig_hash, "s3");
    assert_eq!(two[1].sig_hash, "s2");
    // 字段回读
    let r = &all[0];
    assert_eq!(r.key_fp, "fp-s3");
    assert_eq!(r.publisher.as_deref(), Some("pub-s3"));
    assert_eq!(r.signed_at, 290);
    assert_eq!(r.content_hash, "ch-s3");
    assert_eq!(r.user_name.as_deref(), Some("tester"));
    assert_eq!(r.issuer_name.as_deref(), Some("default"));
    // 同 sig_hash REPLACE → 不增行
    s.add_signature(&sig_rec("s3", 400)).unwrap();
    assert_eq!(s.list_signatures(200).unwrap().len(), 3);
}

#[test]
fn dim_and_status_str_roundtrip_and_defaults() {
    // dim_str 四维
    assert_eq!(dim_str(RevDim::KeyFp), "key_fp");
    assert_eq!(dim_str(RevDim::SigHash), "sig_hash");
    assert_eq!(dim_str(RevDim::FileHash), "file_hash");
    assert_eq!(dim_str(RevDim::Publisher), "publisher");
    // parse_dim 往返 + 未知串回退 KeyFp
    for d in [
        RevDim::KeyFp,
        RevDim::SigHash,
        RevDim::FileHash,
        RevDim::Publisher,
    ] {
        assert_eq!(parse_dim(dim_str(d)), d);
    }
    assert_eq!(parse_dim("bogus-dim"), RevDim::KeyFp);
    // status_str / parse_status 往返 + 未知串回退 Active
    assert_eq!(status_str(KeyStatus::Active), "active");
    assert_eq!(status_str(KeyStatus::Revoked), "revoked");
    assert_eq!(parse_status("active"), KeyStatus::Active);
    assert_eq!(parse_status("revoked"), KeyStatus::Revoked);
    assert_eq!(parse_status("bogus"), KeyStatus::Active);
}

#[test]
fn crl_entries_all_dims_roundtrip_and_replace() {
    let s = store();
    let dims = [
        RevDim::KeyFp,
        RevDim::SigHash,
        RevDim::FileHash,
        RevDim::Publisher,
    ];
    for (i, d) in dims.iter().enumerate() {
        let ver = s
            .add_revoke(CrlEntry {
                dim: *d,
                value: format!("v{i}"),
                revoked_at: 100 + i as u64,
                reason: format!("reason-{i}"),
            })
            .unwrap();
        assert_eq!(ver, 2 + i as u64);
    }
    let crl = s.list_crl().unwrap();
    assert_eq!(crl.version, 5);
    assert_eq!(crl.entries.len(), 4);
    // parse_dim 四分支：各维度经 DB 往返仍正确
    for (i, d) in dims.iter().enumerate() {
        let hit = s.query_revoke(*d, &format!("v{i}")).unwrap().unwrap();
        assert_eq!(hit.revoked_at, 100 + i as u64);
        assert_eq!(hit.reason, format!("reason-{i}"));
    }
    // 同 (dim, value) REPLACE → 不增行、version 仍 +1
    let ver = s
        .add_revoke(CrlEntry {
            dim: RevDim::KeyFp,
            value: "v0".into(),
            revoked_at: 999,
            reason: "again".into(),
        })
        .unwrap();
    assert_eq!(ver, 6);
    assert_eq!(s.list_crl().unwrap().entries.len(), 4);
    let hit = s.query_revoke(RevDim::KeyFp, "v0").unwrap().unwrap();
    assert_eq!(hit.revoked_at, 999);
    assert_eq!(hit.reason, "again");
}

#[test]
fn parse_dim_unknown_row_falls_back_to_key_fp() {
    let s = store();
    // 白盒：直插非法 dim 串（正常路径不会出现），覆盖 list_crl 里 parse_dim 的默认分支
    {
        let conn = s.conn.lock();
        conn.execute(
            "INSERT INTO crl_entries(dim, value, revoked_at, reason) VALUES('bogus','x',1,'r')",
            [],
        )
        .unwrap();
    }
    let crl = s.list_crl().unwrap();
    let hit = crl.entries.iter().find(|e| e.value == "x").unwrap();
    assert_eq!(hit.dim, RevDim::KeyFp);
}

#[test]
fn parse_status_unknown_row_falls_back_to_active() {
    let s = store();
    // 白盒：直插非法 status 串，覆盖 list_trusted_keys 里 parse_status 的默认分支
    {
        let conn = s.conn.lock();
        conn.execute(
            "INSERT INTO trusted_keys(key_fp, status, not_after) VALUES('weird-key','bogus-status',NULL)",
            [],
        )
        .unwrap();
    }
    let tkl = s.list_trusted_keys().unwrap();
    let hit = tkl.keys.iter().find(|k| k.key_fp == "weird-key").unwrap();
    assert_eq!(hit.status, KeyStatus::Active);
    assert_eq!(hit.not_after, None);
}

#[test]
fn trusted_key_not_after_roundtrip() {
    let s = store();
    s.upsert_trusted_key(TrustedKey {
        key_fp: "aa".into(),
        status: KeyStatus::Active,
        not_after: Some(4102444800), // 2100-01-01（i64 安全范围）
    })
    .unwrap();
    s.upsert_trusted_key(TrustedKey {
        key_fp: "bb".into(),
        status: KeyStatus::Active,
        not_after: None,
    })
    .unwrap();
    let tkl = s.list_trusted_keys().unwrap();
    assert_eq!(
        tkl.keys.iter().find(|k| k.key_fp == "aa").unwrap().not_after,
        Some(4102444800)
    );
    assert_eq!(
        tkl.keys.iter().find(|k| k.key_fp == "bb").unwrap().not_after,
        None
    );
}

#[test]
fn audit_limit_honored() {
    let s = store();
    for i in 0..3u64 {
        s.add_audit(AuditRecord {
            id: 0,
            timestamp: i,
            action: format!("act-{i}"),
            operator: "t".into(),
            dim: None,
            value: None,
            reason: None,
            detail: None,
        })
        .unwrap();
    }
    let two = s.list_audit(2).unwrap();
    assert_eq!(two.len(), 2);
    assert_eq!(two[0].action, "act-2"); // DESC → 最新在前
    assert_eq!(two[1].action, "act-1");
}

#[test]
fn sqlite_file_persistence_across_reopen() {
    let path = temp_db_path("persist");
    {
        let s = SqliteStore::open(&path).unwrap();
        s.add_revoke(CrlEntry {
            dim: RevDim::Publisher,
            value: "p".into(),
            revoked_at: 5,
            reason: "r".into(),
        })
        .unwrap();
        s.add_audit(AuditRecord {
            id: 0,
            timestamp: 1,
            action: "a".into(),
            operator: "o".into(),
            dim: None,
            value: None,
            reason: None,
            detail: None,
        })
        .unwrap();
    } // drop：Windows 上先释放文件锁再重开
    let s2 = SqliteStore::open(&path).unwrap();
    let crl = s2.list_crl().unwrap();
    assert_eq!(
        crl.version, 2,
        "meta 版本跨重开保留（INSERT OR IGNORE 不重置）"
    );
    assert_eq!(crl.entries.len(), 1);
    assert_eq!(crl.entries[0].dim, RevDim::Publisher);
    assert_eq!(s2.list_audit(10).unwrap().len(), 1);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn sqlite_open_bad_directory_errors() {
    let bad = std::env::temp_dir()
        .join("revoke_store_definitely_missing_dir_9527")
        .join("x.db");
    assert!(SqliteStore::open(bad.to_str().unwrap()).is_err());
}

#[test]
fn meta_set_and_get_internal() {
    let s = store();
    s.meta_set("custom-key", "v1").unwrap();
    {
        let conn = s.conn.lock();
        assert_eq!(meta_get(&conn, "custom-key").unwrap(), "v1");
        assert!(meta_get(&conn, "no-such-meta-key").is_err());
    }
    // INSERT OR REPLACE 覆盖写
    s.meta_set("custom-key", "v2").unwrap();
    {
        let conn = s.conn.lock();
        assert_eq!(meta_get(&conn, "custom-key").unwrap(), "v2");
    }
    // 初始 meta 种子存在（init_schema INSERT OR IGNORE）
    {
        let conn = s.conn.lock();
        assert_eq!(meta_get(&conn, "crl_version").unwrap(), "1");
        assert_eq!(meta_get(&conn, "trusted_keys_version").unwrap(), "1");
    }
}
