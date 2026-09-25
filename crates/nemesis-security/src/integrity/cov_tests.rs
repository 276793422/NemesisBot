// integrity.rs 覆盖率补充测试（append 序列化落盘 + 段轮转 + export_chain
// + load_chain + load_segments 恢复 + get_event 全臂 + verify_range 触发
// read_events_range 的内外层 break + close 的 info 行）。
//
// 无豁免——本文件 12 条 miss 行全部可由一条完整生命周期测试驱动
// （179/302/304/378/388/415/417/449/488/500/509/511 均为轮转/导出/加载/
// 关闭路径的收尾 brace 与循环体，真实 IO 即达）。

use super::*;

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("nmb-int-cov-{}-{tag}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn chain_with(max_events: u64, dir: &std::path::Path) -> AuditChain {
    AuditChain::new(AuditChainConfig {
        enabled: true,
        storage_path: dir.join("audit.jsonl"),
        max_file_size: 50 * 1024 * 1024,
        verify_on_load: false,
        max_events_per_segment: max_events,
        signing_key: None,
    })
}

/// 完整生命周期：append 5 条（段上限 2 → 轮转）→ 导出 → 导入 →
/// load_segments 恢复计数 → get_event 全臂 → verify_range 双 break →
/// close 后 append 拒绝。
#[test]
fn chain_full_lifecycle_rotation_export_load_verify_and_close() {
    let dir = temp_dir("life");
    let chain = chain_with(2, &dir);

    for i in 0..5 {
        let ev = chain
            .append(
                "file_write",
                "write_file",
                "cov-user",
                "cli",
                &format!("f{i}.txt"),
                "allow",
                "cov",
            )
            .unwrap();
        assert!(!ev.hash.is_empty());
    }
    // 段上限 2 → 5 条触发两次轮转（segment_count 初始 1 → 3），
    // 落在 3 个段（seg1=本体 + seg0002/seg0003）。
    assert_eq!(chain.segment_count(), 3);
    assert_eq!(chain.total_event_count(), 5);
    assert_ne!(chain.current_hash(), "");

    // export_chain → JSON 全事件（302/304 读取循环）。
    let out = dir.join("export.json");
    chain.export_chain(&out).unwrap();
    let exported: Vec<AuditEvent> =
        serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(exported.len(), 5);

    // load_segments 用全新实例（从段文件恢复 total/last_hash，415/417
    // total>0 臂）——放在 load_chain 之前，避免导入回写把段文件翻倍。
    let chain2 = chain_with(2, &dir);
    let seg_total = chain2.load_segments().unwrap();
    assert_eq!(seg_total, 5);
    assert_eq!(chain2.total_event_count(), 5);
    assert_ne!(chain2.current_hash(), "");

    // get_event：命中首条 / 跳过首条命中次条（count += 1 臂）/ 越界 None。
    assert_eq!(chain2.get_event(0).unwrap().target, "f0.txt");
    assert_eq!(chain2.get_event(1).unwrap().target, "f1.txt");
    assert!(chain2.get_event(999).is_none());

    // verify_range(0,1) → to+1=2：首段满 2 条触发内层 break（500），
    // 进入第二个文件时外层 break（488）。
    assert!(chain2.verify_range(0, 1).unwrap());
    // 大范围全量校验（509/511 收尾）。
    assert!(chain2.verify_range(0, 4).unwrap());

    // load_chain（从导出 JSON 读回：verify_chain + fetch_add 累计 + 回写
    // 存储段，378/388 统计循环）。导入是累加语义：total 5 → 10。
    let loaded = chain.load_chain(&out).unwrap();
    assert_eq!(loaded, 5);
    assert_eq!(chain.total_event_count(), 10);

    // close → info 行（449）+ is_closed + append 拒绝。
    chain.close().unwrap();
    assert!(chain.is_closed());
    // 重复 close 幂等早退。
    chain.close().unwrap();
    assert!(
        chain
            .append("x", "t", "u", "s", "tg", "allow", "r")
            .is_err()
    );

    // verify_chain 负例：篡改前向哈希 → false。
    let mut tampered = exported.clone();
    tampered[1].prev_hash = "deadbeef".into();
    assert!(!AuditChain::verify_chain(&tampered));
    assert!(AuditChain::verify_chain(&exported));

    let _ = std::fs::remove_dir_all(&dir);
}

/// append_with_sign：带签名字段的事件照常入链且校验通过。
#[test]
fn append_with_sign_stores_signature_field() {
    let dir = temp_dir("sign");
    let chain = chain_with(100, &dir);
    let ev = chain
        .append_with_sign(
            "exec",
            "shell",
            "u",
            "s",
            "t",
            "allow",
            "r",
            Some("cov-sig".into()),
        )
        .unwrap();
    assert_eq!(ev.sign.as_deref(), Some("cov-sig"));

    let seg = dir.join("audit.jsonl");
    let raw = std::fs::read_to_string(seg).unwrap();
    assert!(raw.contains("cov-sig"), "签名必须落盘: {raw}");

    let _ = std::fs::remove_dir_all(&dir);
}
