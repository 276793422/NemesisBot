//! bulk 分块账与打包单测（宿主可跑的纯宿主面；带真实 VM 的全链路归
//! guest 集成测试 full_chain）。H1 回归锁住在这里：分块账必须按 tar
//! 实际长度计，头部开销漏算会让大变更集跨 4MiB 边界时截尾。

#[test]
fn read_chunk_plan_boundaries() {
    assert_eq!(super::read_chunk_plan(0), 1, "空 tar 仍要给 1 块（收尾块）");
    assert_eq!(super::read_chunk_plan(1), 1);
    assert_eq!(
        super::read_chunk_plan(super::BULK_READ_CHUNK_BYTES),
        1,
        "整除边界不进位"
    );
    assert_eq!(super::read_chunk_plan(super::BULK_READ_CHUNK_BYTES + 1), 2);
    assert_eq!(super::read_chunk_plan(super::BULK_READ_CHUNK_BYTES * 3), 3);
    assert_eq!(
        super::read_chunk_plan(super::BULK_READ_CHUNK_BYTES * 3 + 1),
        4
    );
}

#[test]
fn pack_files_tar_len_exceeds_payload_sum() {
    // tar 头（512B/条目）+ 1024B 收尾块是真实开销——旧分块账（按 payload
    // 字节和）漏了它，正是 H1 截尾的根因。小包这里只验证不等式成立；
    // 跨界数学由 read_chunk_plan_boundaries 钉死。
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    let n = 64u32;
    for i in 0..n {
        std::fs::write(ws.join(format!("f{i:03}.txt")), b"x").unwrap();
    }
    let rels: Vec<String> = (0..n).map(|i| format!("f{i:03}.txt")).collect();
    let out = ws.join("out.tar");
    let packed = super::pack_files(ws, &rels, &out).unwrap();
    assert_eq!(packed.bytes, n as u64, "payload 字节和 = 文件尺寸和");
    assert_eq!(packed.present.len(), n as usize, "全部条目应进包");
    let tar_len = std::fs::metadata(&out).unwrap().len();
    assert!(
        tar_len > packed.bytes,
        "tar 实际长度必须含头部开销: tar_len={tar_len} payload={}",
        packed.bytes
    );
}

#[test]
fn pack_files_missing_files_are_absent_not_fatal() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    std::fs::write(ws.join("here.txt"), b"ok").unwrap();
    let rels = vec![
        "here.txt".to_string(),
        "missing.txt".to_string(), // 缺席 → present 不含、不报错
    ];
    let out = ws.join("out.tar");
    let packed = super::pack_files(ws, &rels, &out).unwrap();
    assert_eq!(packed.present, vec!["here.txt".to_string()]);
    assert_eq!(packed.bytes, 2);
}
