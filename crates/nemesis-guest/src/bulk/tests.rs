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

// ---- symlink 闸回归（2026-10-20 全面复查批）----
// symlink 创建是 Unix 语义（Windows 需特权/开发者模式），用例整体
// #[cfg(unix)]——真实执行面（VM 内 guest）本就是 Linux。

/// 手工组 tar（pack_files 从磁盘打包，造不出「链接在前、条目穿链接」
/// 的恶意形态——攻击面测试必须手工组包）。
#[cfg(unix)]
fn build_tar(ws: &std::path::Path, name: &str, entries: &[(&str, &[u8])]) -> std::path::PathBuf {
    // 调用方传的是 tempdir 下的新鲜子目录（如 tmp/t）——先建父目录，
    // 否则 File::create 直接 ENOENT（2026-10-20 CI Linux coverage job 实证）。
    std::fs::create_dir_all(ws).unwrap();
    let out = ws.join(name);
    let f = std::fs::File::create(&out).unwrap();
    let mut b = tar::Builder::new(f);
    for (rel, data) in entries {
        let mut h = tar::Header::new_gnu();
        h.set_size(data.len() as u64);
        h.set_mode(0o644);
        h.set_cksum();
        b.append_data(&mut h, rel, *data).unwrap();
    }
    b.finish().unwrap();
    out
}

#[test]
fn ensure_gate_passes_absent_and_plain_paths() {
    // 常规路径（组件全缺）必须放行——闸不能误伤正常解包。
    let tmp = tempfile::tempdir().unwrap();
    super::ensure_no_symlink_components(tmp.path(), std::path::Path::new("a/b/c.txt")).unwrap();
    // 已存在为普通目录/文件：放行（文件祖先的冲突交给 create_dir_all 诚实报错）。
    std::fs::create_dir_all(tmp.path().join("d")).unwrap();
    super::ensure_no_symlink_components(tmp.path(), std::path::Path::new("d/e.txt")).unwrap();
}

#[cfg(unix)]
#[test]
fn extract_refuses_symlink_ancestor() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    let outside = tmp.path().join("outside");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    // agent 造的目录链接：ws/link -> outside（工作区内名字，工作区外目标）。
    std::os::unix::fs::symlink(&outside, ws.join("link")).unwrap();
    let tar = build_tar(&tmp.path().join("t"), "atk.tar", &[("link/x.txt", b"pwn")]);
    let err = super::extract_tar_safe(&tar, &ws).unwrap_err();
    assert!(
        err.contains("symlink"),
        "必须 fail-closed 拒绝，实际错误: {err}"
    );
    assert!(
        !outside.join("x.txt").exists(),
        "解包不得顺着链接写出工作区"
    );
}

#[cfg(unix)]
#[test]
fn extract_refuses_final_component_symlink() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    let outside = tmp.path().join("outside");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("secret.txt"), b"keep").unwrap();
    // 末端组件是链接：ws/victim -> outside/secret.txt。
    std::os::unix::fs::symlink(outside.join("secret.txt"), ws.join("victim")).unwrap();
    let tar = build_tar(
        &tmp.path().join("t"),
        "atk2.tar",
        &[("victim", b"overwritten")],
    );
    let err = super::extract_tar_safe(&tar, &ws).unwrap_err();
    assert!(
        err.contains("symlink"),
        "必须 fail-closed 拒绝，实际错误: {err}"
    );
    assert_eq!(
        std::fs::read(outside.join("secret.txt")).unwrap(),
        b"keep",
        "链接目标不得被覆盖"
    );
}

#[cfg(unix)]
#[test]
fn extract_plain_paths_still_succeed_after_gate() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let tar = build_tar(
        &tmp.path().join("t"),
        "ok.tar",
        &[("a/b.txt", b"hello"), ("top.txt", b"world")],
    );
    let (written, skipped) = super::extract_tar_safe(&tar, &ws).unwrap();
    assert_eq!((written, skipped), (2, 0));
    assert_eq!(std::fs::read(ws.join("a/b.txt")).unwrap(), b"hello");
}

#[cfg(unix)]
#[test]
fn clean_removes_symlink_dir_without_following() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    let outside = tmp.path().join("outside");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::create_dir_all(outside.join("real")).unwrap();
    std::fs::write(outside.join("real").join("f.txt"), b"valuable").unwrap();
    // 工作区内的真实目录 + 指向工作区外目录的链接（node_modules/.bin 常态形态）。
    std::fs::create_dir_all(ws.join("d")).unwrap();
    std::os::unix::fs::symlink(outside.join("real"), ws.join("s")).unwrap();
    super::clean_workspace(&ws).unwrap();
    assert!(
        outside.join("real").join("f.txt").exists(),
        "clean 不得顺着链接清到工作区外"
    );
    assert!(!ws.join("s").exists(), "链接本身应被移除");
    assert!(!ws.join("d").exists(), "真实目录应被移除");
}
