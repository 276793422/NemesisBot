// transfer.rs 覆盖率补充测试（路径围栏剩余面 / begin staging 清理·创建
// 失败臂 / end 幂等重放 + 块长不符 + SHA 不符 + 落地替换失败 /
// validate_manifest 三态 / collect 围栏自检与符号链接跳过）。
//
// 豁免：130（`..`/`.` 落 Normal 组件不可达——components() 把它们归
// ParentDir/CurDir，走 136 的 catch-all）；121（绝对路径无盘符形态仅
// unix 可达，cfg 门测试照跑）；266-267（strip_prefix 防御，entry 恒从
// root 枚举）；268-270（非 UTF-8 文件名 Windows 不可造，unix cfg 测试
// 照跑）；278（symlink_metadata 竞态）；300（既非目录也非文件的元祖态）；
// 603-605/620-622（files_root 下建父目录失败面）；643-648（组装长度恒
// 等于 manifest 推导的块计划，长度不符会先撞 613）；668-673（rename 跨卷
// 退化，同卷恒成功）；835（dedup 序列化恒成功）。

use super::*;
use std::path::PathBuf;

fn temp_ws(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "nemesis-cluster-tfcov-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn sink_in(ws: &Path) -> TransferSink {
    TransferSink::new(ws, 0)
}

fn staging_root(ws: &Path) -> PathBuf {
    nemesis_path::cluster_dir_in_workspace(ws)
        .join("inbox")
        .join(".staging")
}

fn make_source(root: &Path, files: &[(&str, &[u8])]) -> PathBuf {
    let src = root.join("src");
    for (rel, data) in files {
        let p = src.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, data).unwrap();
    }
    src
}

fn prepare_begin(src: &Path, tid: &str, task: &str) -> TransferBegin {
    let files = collect_dir_files(src).unwrap();
    let total: u64 = files.iter().map(|f| f.size).sum();
    let chunk = 64 * 1024;
    TransferBegin {
        transfer_id: tid.into(),
        task_id: task.into(),
        kind: TRANSFER_KIND_EXECUTION_RECORDS.into(),
        source_node: "node-w".into(),
        total_bytes: total,
        chunk_size: chunk,
        chunk_count: chunk_count(&files, chunk),
        content_hash: content_hash(&files),
        files,
    }
}

/// 按 manifest 计划把全部块推进 sink（真实内容）。
fn push_all(sink: &TransferSink, begin: &TransferBegin, src: &Path) {
    let plan = plan_chunks(&begin.files, begin.chunk_size);
    use std::io::{Read, Seek, SeekFrom};
    let mut handles: Vec<(std::fs::File, u64)> = begin
        .files
        .iter()
        .map(|f| {
            let mut h = std::fs::File::open(src.join(&f.path)).unwrap();
            h.seek(SeekFrom::Start(0)).unwrap();
            (h, 0u64)
        })
        .collect();
    let _ = &mut handles;
    for (seq, (file_idx, off, len)) in plan.iter().enumerate() {
        let mut h = &handles[*file_idx].0;
        h.seek(SeekFrom::Start(*off)).unwrap();
        let mut buf = vec![0u8; *len];
        h.read_exact(&mut buf).unwrap();
        let req = TransferChunk {
            transfer_id: begin.transfer_id.clone(),
            seq,
            total: plan.len(),
            data_b64: b64_encode(&buf),
            sha256: sha256_hex(&buf),
        };
        sink.chunk(&req).unwrap();
    }
}

// ---------------------------------------------------------------------------
// 路径围栏
// ---------------------------------------------------------------------------

/// 绝对路径（无盘符形态，unix 可达；Windows 下该输入恒有盘符/反斜杠先行
/// 拦截）。
#[cfg(unix)]
#[test]
fn fence_rejects_absolute_without_drive() {
    assert!(safe_relative_path("/etc/passwd").is_err());
}

/// 8.3 短名 + 非普通组件双面（Windows 实际走 catch-all：`..` 归 ParentDir）。
#[test]
fn fence_faces_portable() {
    assert!(safe_relative_path("PROGRA~1/x.txt").is_err());
    assert!(
        safe_relative_path("a/../b").is_err(),
        "catch-all 拦 ParentDir"
    );
    assert!(safe_relative_path("").is_err());
}

/// collect_dir_files 围栏自检：8.3 名文件被静默跳过（宁可不传）。
#[test]
fn collect_skips_fence_violating_and_noise_entries() {
    let root = temp_ws("collect-fence");
    let src = root.join("src");
    std::fs::create_dir_all(src.join(".git")).unwrap();
    std::fs::create_dir_all(src.join("__pycache__")).unwrap();
    std::fs::write(src.join("PROGRA~1.txt"), b"bad name").unwrap();
    std::fs::write(src.join("junk.pyc"), b"bytecode").unwrap();
    std::fs::write(src.join(".git").join("HEAD"), b"ref").unwrap();
    std::fs::write(src.join("real.txt"), b"keep").unwrap();

    let files = collect_dir_files(&src).unwrap();
    let names: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(names, vec!["real.txt"], "{names:?}");
}

/// 符号链接跳过（unix；Windows 建链要特权，不跑）。
#[cfg(unix)]
#[test]
fn collect_skips_symlinks() {
    let root = temp_ws("collect-symlink");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("real.txt"), b"keep").unwrap();
    std::os::unix::fs::symlink(src.join("real.txt"), src.join("link.txt")).unwrap();
    let files = collect_dir_files(&src).unwrap();
    assert_eq!(files.len(), 1, "{files:?}");
    assert_eq!(files[0].path, "real.txt");
}

/// 非 UTF-8 文件名 → 诚实报错（unix 可造；Windows UTF-16 无此形态）。
#[cfg(unix)]
#[test]
fn collect_errors_on_non_utf8_name() {
    use std::os::unix::ffi::OsStrExt;
    let root = temp_ws("collect-nonutf8");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    let bad = src.join(std::ffi::OsStr::from_bytes(b"bad\xffname.txt"));
    std::fs::write(&bad, b"x").unwrap();
    let err = collect_dir_files(&src).unwrap_err();
    assert!(err.contains("非 UTF-8"), "{err}");
}

// ---------------------------------------------------------------------------
// validate_manifest 三态
// ---------------------------------------------------------------------------

#[test]
fn validate_manifest_error_faces() {
    let ws = temp_ws("manifest-faces");
    let sink = sink_in(&ws);
    let src = make_source(&ws, &[("a.txt", b"hello world hello")]);
    let mut begin = prepare_begin(&src, "tid-vm", "task-vm");

    begin.transfer_id = String::new();
    let reply = sink.begin(&begin);
    assert_eq!(reply.status, "error");
    assert!(reply.error.unwrap().contains("不能为空"));

    begin.transfer_id = "tid-vm".into();
    begin.chunk_size = 0;
    let reply = sink.begin(&begin);
    assert!(reply.error.unwrap().contains("chunk_size"));

    begin.chunk_size = 64 * 1024;
    begin.content_hash = String::new();
    let reply = sink.begin(&begin);
    assert!(reply.error.unwrap().contains("content_hash"));
}

// ---------------------------------------------------------------------------
// begin staging 失败臂 + end 失败臂
// ---------------------------------------------------------------------------

/// 旧 staging 清不掉（内部文件被无 DELETE 共享位的句柄握住）→ begin 报
/// 「清理旧 staging 失败」。Unix 用只读目录权限拦 rmdir。
#[test]
fn begin_cleanup_failure_is_honest_error() {
    let ws = temp_ws("begin-clean-fail");
    let sink = sink_in(&ws);
    let src = make_source(&ws, &[("a.txt", b"content-a")]);
    let begin = prepare_begin(&src, "tid-dirty", "task-dirty");

    // 旧 staging 残留 + 内容指纹不同（resume=false）。
    let st = staging_root(&ws).join(sanitize_transfer_id(&begin.transfer_id));
    std::fs::create_dir_all(&st).unwrap();
    std::fs::write(st.join("manifest.json"), r#"{"content_hash":"stale"}"#).unwrap();

    #[cfg(windows)]
    let held = {
        use std::os::windows::fs::OpenOptionsExt;
        let f = st.join("lock.bin");
        std::fs::write(&f, b"x").unwrap();
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0x0001 | 0x0002) // 无 DELETE → remove_dir_all 失败
            .open(&f)
            .unwrap()
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut p = std::fs::metadata(&st).unwrap().permissions();
        p.set_mode(0o555);
        std::fs::set_permissions(&st, p).unwrap();
    }

    let reply = sink.begin(&begin);
    assert_eq!(reply.status, "error", "清不掉旧 staging 必须诚实报错");
    assert!(reply.error.unwrap().contains("清理旧 staging 失败"));

    drop(held);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut p = std::fs::metadata(&st).unwrap().permissions();
        p.set_mode(0o755);
        std::fs::set_permissions(&st, p).unwrap();
    }
}

/// staging 根变文件 → create_dir_all 失败 → begin 报「创建 staging 失败」。
#[test]
fn begin_create_failure_when_staging_root_is_file() {
    let ws = temp_ws("begin-create-fail");
    let sink = sink_in(&ws);
    let st_root = staging_root(&ws);
    std::fs::remove_dir_all(&st_root).unwrap();
    std::fs::write(&st_root, "not a dir").unwrap();

    let src = make_source(&ws, &[("a.txt", b"content-b")]);
    let begin = prepare_begin(&src, "tid-create-fail", "task-create-fail");
    let reply = sink.begin(&begin);
    assert_eq!(reply.status, "error");
    assert!(reply.error.unwrap().contains("创建 staging 失败"));
}

/// end 幂等重放：成功落地后再 end → .done 回执重放（防 sender 死锁）；
/// 无 staging 且无回执 → 诚实 Err。
#[test]
fn end_replay_and_missing_staging_faces() {
    let ws = temp_ws("end-replay");
    let sink = sink_in(&ws);
    let src = make_source(&ws, &[("a.txt", b"payload for replay")]);
    let begin = prepare_begin(&src, "tid-replay", "task-replay");

    let reply1 = sink.begin(&begin);
    assert_eq!(reply1.status, "ok", "{:?}", reply1.error);
    push_all(&sink, &begin, &src);
    let end1 = sink.end(&begin.transfer_id).unwrap();
    assert_eq!(end1.status, "ok");

    // staging 已清 → 重放 .done 回执，结果一致。
    let end2 = sink.end(&begin.transfer_id).unwrap();
    assert_eq!(end2.status, "ok");
    assert_eq!(end2.file_count, end1.file_count);
    assert_eq!(end2.total_bytes, end1.total_bytes);

    // 完全未知的 transfer → Err。
    let err = sink.end("tid-never-seen").unwrap_err();
    assert!(err.contains("staging 不存在"), "{err}");
}

/// 块文件长度与计划不符 → end 诚实报「长度 ≠ 计划」。
#[test]
fn end_chunk_length_mismatch_is_honest_error() {
    let ws = temp_ws("end-len");
    let sink = sink_in(&ws);
    let src = make_source(&ws, &[("a.txt", vec![b'x'; 200_000].as_slice())]);
    let begin = prepare_begin(&src, "tid-len", "task-len");
    sink.begin(&begin);
    push_all(&sink, &begin, &src);

    // 全部块就位后，把第 1 块覆写为短内容（长度错）。
    let st = staging_root(&ws).join(sanitize_transfer_id(&begin.transfer_id));
    std::fs::write(st.join("chunk_000001.bin"), b"short").unwrap();
    let err = sink.end(&begin.transfer_id).unwrap_err();
    assert!(err.contains("≠ 计划"), "{err}");
}

/// 块内容被换（同长度不同字节）→ 组装成功但文件 SHA 不符 → 诚实 Err。
#[test]
fn end_sha256_mismatch_is_honest_error() {
    let ws = temp_ws("end-sha");
    let sink = sink_in(&ws);
    let src = make_source(&ws, &[("a.txt", vec![b'a'; 200_000].as_slice())]);
    let begin = prepare_begin(&src, "tid-sha", "task-sha");
    sink.begin(&begin);

    // 同长度、异内容覆写全部块。
    let st = staging_root(&ws).join(sanitize_transfer_id(&begin.transfer_id));
    let plan = plan_chunks(&begin.files, begin.chunk_size);
    for (seq, (_, _, len)) in plan.iter().enumerate() {
        std::fs::write(st.join(format!("chunk_{seq:06}.bin")), vec![b'z'; *len]).unwrap();
    }
    let err = sink.end(&begin.transfer_id).unwrap_err();
    assert!(err.contains("SHA-256 核验失败"), "{err}");
}

/// 同 task 二次落地时旧收件目录删不掉（句柄占用）→ 诚实 Err。
#[test]
fn end_landed_replace_failure_is_honest_error() {
    let ws = temp_ws("end-landed");
    let sink = sink_in(&ws);

    let land_first = |tid: &str, task: &str, byte: u8| {
        let src = make_source(&ws, &[("a.txt", vec![byte; 150_000].as_slice())]);
        let begin = prepare_begin(&src, tid, task);
        sink.begin(&begin);
        push_all(&sink, &begin, &src);
        begin
    };

    let b1 = land_first("tid-l1", "task-landed", b'1');
    sink.end(&b1.transfer_id).unwrap();
    let landed = nemesis_path::cluster_dir_in_workspace(&ws)
        .join("inbox")
        .join("task-landed");
    assert!(landed.exists());

    // 第二次传输（不同内容同 task）→ 落地前要整替旧目录 → 删失败。
    #[cfg(windows)]
    let held = {
        use std::os::windows::fs::OpenOptionsExt;
        let f = landed.join("files").join("a.txt");
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0x0001 | 0x0002)
            .open(&f)
            .unwrap()
    };

    let b2 = land_first("tid-l2", "task-landed", b'2');
    #[cfg(unix)]
    {
        // unix：把旧 landed 目录变只读 → 删子项失败。
        use std::os::unix::fs::PermissionsExt;
        let mut p = std::fs::metadata(&landed).unwrap().permissions();
        p.set_mode(0o555);
        std::fs::set_permissions(&landed, p).unwrap();
    }

    let err = sink.end(&b2.transfer_id).unwrap_err();
    assert!(err.contains("清理旧收件目录失败"), "{err}");

    drop(held);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut p = std::fs::metadata(&landed).unwrap().permissions();
        p.set_mode(0o755);
        std::fs::set_permissions(&landed, p).unwrap();
    }
}
