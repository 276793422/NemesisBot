// plugin.rs CLI 单元测试（信任吊销面 resolve_trust_entry 族）。

use super::*;

// BUG-20261122-012 回归锁（mirror connector 同名助手同款用例）：同名异钥
// 按 name 解析必须拒歧义——旧实现走 HashMap 迭代序任意取（CLI 每进程新
// RandomState，同一信任库同一 --name 跨 run 可落到不同公钥条目，吊销/
// 移除可错改）。单名/零名行为不变；重名列出 fingerprint 前缀要求
// --key 精确指定。
#[test]
fn test_resolve_trust_entry_duplicate_name_refuses() {
    let store = nemesis_security::signature::TrustStore::in_memory();
    let pair1 = nemesis_security::signature::generate_key_pair().expect("keygen");
    let pair2 = nemesis_security::signature::generate_key_pair().expect("keygen");
    store.add_key(&pair1.public_key, "signer-dup", {
        use nemesis_security::signature::TrustLevel;
        TrustLevel::Verified
    });
    store.add_key(&pair2.public_key, "signer-dup", {
        use nemesis_security::signature::TrustLevel;
        TrustLevel::Community
    });

    // 重名 → Err 且带拒歧义指引（同名异钥 / --key）。
    let err = resolve_trust_entry(&store, &None, &Some("signer-dup".into()))
        .expect_err("同名异钥必须拒绝而非任意取");
    let msg = format!("{err:#}");
    assert!(msg.contains("同名异钥"), "报错须点名同名异钥: {msg}");
    assert!(msg.contains("--key"), "报错须指引 --key 精确指定: {msg}");
    // 两条 fingerprint 前缀都要在列（用户能据此挑条目）。
    for fp in [
        store.get_key(&pair1.public_key).unwrap().fingerprint,
        store.get_key(&pair2.public_key).unwrap().fingerprint,
    ] {
        assert!(
            msg.contains(&fp.chars().take(12).collect::<String>()),
            "报错须列出 fingerprint 前缀: {msg}"
        );
    }

    // 单名仍 Ok、零名仍 not_found（既有行为不回归）。
    let solo = nemesis_security::signature::generate_key_pair().unwrap();
    store.add_key(&solo.public_key, "signer-solo", {
        use nemesis_security::signature::TrustLevel;
        TrustLevel::Verified
    });
    assert!(
        resolve_trust_entry(&store, &None, &Some("signer-solo".into())).is_ok(),
        "唯一名解析不受拒歧义臂影响"
    );
    assert!(
        resolve_trust_entry(&store, &None, &Some("没有这个名字".into())).is_err(),
        "无名条目仍 not_found"
    );
}

// 基线用例：按 key / 按 name / 两者一致（既有行为钉死，防拒歧义臂
// 误伤单条目路径）。
#[test]
fn test_resolve_trust_entry_single_entry_variants() {
    let store = nemesis_security::signature::TrustStore::in_memory();
    let pair = nemesis_security::signature::generate_key_pair().expect("keygen");
    store.add_key(&pair.public_key, "signer-a", {
        use nemesis_security::signature::TrustLevel;
        TrustLevel::Verified
    });

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

    // 不一致 / 不在库 / 缺一 → Err（既有行为不变）。
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
