//! transfer 分块通道测试（P3/D1+D3+D4+D6+D7）。
//!
//! goal 单测清单：分块切分/重组/乱序、去重幂等、超限判定、manifest 核验、
//! 路径围栏（`..`/绝对/8.3 拒绝）、先落盘后 ACK 顺序。

use super::*;
use std::sync::atomic::AtomicU32;

/// 唯一临时目录（进程内 AtomicU32 序列 + pid 命名，与其他套件不撞）。
fn temp_root(name: &str) -> PathBuf {
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "nemesis-cluster-xfer-{}-{name}-{n}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// 构造发送目录：`files/` 下写给定 (相对路径, 内容)。
fn make_source(root: &Path, files: &[(&str, &[u8])]) {
    for (rel, data) in files {
        let p = root.join(rel.replace('/', "\\"));
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, data).unwrap();
    }
}

/// sender 侧准备：收集 + begin 载荷。
fn prepare_begin(src: &Path, transfer_id: &str, task_id: &str, chunk_size: usize) -> TransferBegin {
    let files = collect_dir_files(src).unwrap();
    TransferBegin {
        transfer_id: transfer_id.into(),
        task_id: task_id.into(),
        kind: TRANSFER_KIND_EXECUTION_RECORDS.into(),
        source_node: "node-w".into(),
        total_bytes: files.iter().map(|f| f.size).sum(),
        chunk_size,
        chunk_count: chunk_count(&files, chunk_size),
        content_hash: content_hash(&files),
        files,
    }
}

/// 取文件字节（相对路径 → 实际盘上路径，Windows 分隔符翻转）。
fn file_bytes(src: &Path, rel: &str) -> Vec<u8> {
    std::fs::read(src.join(rel.replace('/', "\\"))).unwrap()
}

/// 推送一个计划块。
fn push_chunk(sink: &TransferSink, begin: &TransferBegin, src: &Path, seq: usize) {
    let plan = plan_chunks(&begin.files, begin.chunk_size);
    let (file_idx, offset, len) = plan[seq];
    let data = file_bytes(src, &begin.files[file_idx].path);
    let slice = &data[offset as usize..offset as usize + len];
    sink.chunk(&TransferChunk {
        transfer_id: begin.transfer_id.clone(),
        seq,
        total: plan.len(),
        data_b64: b64_encode(slice),
        sha256: sha256_hex(slice),
    })
    .unwrap_or_else(|e| panic!("chunk {seq} 失败: {e}"));
}

/// 全链路发送：begin → 逐块（可跳过 have）→ end。
fn push_all(
    sink: &TransferSink,
    begin: &TransferBegin,
    src: &Path,
    skip: &[usize],
) -> TransferEndReply {
    let reply = sink.begin(begin);
    assert_eq!(reply.status, "ok", "begin 失败: {:?}", reply.error);
    let plan = plan_chunks(&begin.files, begin.chunk_size);
    for seq in 0..plan.len() {
        if !skip.contains(&seq) {
            push_chunk(sink, begin, src, seq);
        }
    }
    sink.end(&begin.transfer_id).expect("end 应成功")
}

// ---------------------------------------------------------------------------
// 切分 / 重组 / 乱序 / 续传
// ---------------------------------------------------------------------------

#[test]
fn plan_chunks_splits_deterministically() {
    let files = vec![
        TransferFileEntry {
            path: "a.bin".into(),
            size: 10,
            sha256: String::new(),
        },
        TransferFileEntry {
            path: "b.bin".into(),
            size: 0,
            sha256: String::new(),
        },
        TransferFileEntry {
            path: "c.bin".into(),
            size: 25,
            sha256: String::new(),
        },
    ];
    // chunk=10：a→1 块，b（空）→0 块，c→3 块（10+10+5）。
    let plan = plan_chunks(&files, 10);
    assert_eq!(plan.len(), 4);
    assert_eq!(plan[0], (0, 0, 10));
    assert_eq!(plan[1], (2, 0, 10));
    assert_eq!(plan[2], (2, 10, 10));
    assert_eq!(plan[3], (2, 20, 5));
    // 确定性：重算一致。
    assert_eq!(plan_chunks(&files, 10), plan);
    // 单块覆盖：chunk=100 → 2 块（a、c 各一）。
    assert_eq!(plan_chunks(&files, 100).len(), 2);
}

#[test]
fn chunked_roundtrip_with_nested_dirs_and_binary() {
    let root = temp_root("roundtrip");
    let src = root.join("src");
    let payload: Vec<u8> = (0..=255u8).cycle().take(10_000).collect();
    let req_md = "# request\n内容 with 中文\n".as_bytes();
    let req_json = br#"{"model":"m","messages":[1,2,3]}"#;
    make_source(
        &src,
        &[
            (
                "logs/cluster_logs/node-x/20260914_000001_t-1/00.request.md",
                req_md,
            ),
            (
                "logs/cluster_logs/node-x/20260914_000001_t-1/01.AI.Request.raw.json",
                req_json,
            ),
            (
                "logs/cluster_logs/node-x/20260914_000001_t-1/blob.bin",
                &payload,
            ),
            (
                "logs/cluster_logs/node-x/20260914_000001_t-1/empty.marker",
                b"",
            ),
        ],
    );
    let sink = TransferSink::new(&root.join("ws"), 0);
    let begin = prepare_begin(&src, "xf-1", "task-1", 1024); // ~10.2KB / 1KB ≈ 10 块
    assert!(begin.chunk_count >= 9, "多块切分：{}", begin.chunk_count);
    let end = push_all(&sink, &begin, &src, &[]);
    assert_eq!(end.status, "ok");
    assert_eq!(end.file_count, Some(4));
    assert_eq!(end.total_bytes, Some(begin.total_bytes));

    // 落地内容逐字节一致（含空文件）。
    let landed = sink.inbox_root().join("task-1");
    for f in &begin.files {
        let got = std::fs::read(landed.join("files").join(f.path.replace('/', "\\"))).unwrap();
        let want = file_bytes(&src, &f.path);
        assert_eq!(got, want, "文件 {} 落地不一致", f.path);
        assert_eq!(sha256_hex(&got), f.sha256);
    }
    // manifest 镜像 + 回执在场。
    assert!(landed.join("manifest.json").exists());
    assert!(landed.join("landed.json").exists());
}

#[test]
fn out_of_order_chunks_still_assemble_correctly() {
    let root = temp_root("ooo");
    let src = root.join("src");
    let payload: Vec<u8> = (0..100u8).cycle().take(5000).collect();
    make_source(&src, &[("big.bin", &payload)]);
    let sink = TransferSink::new(&root.join("ws"), 0);
    let begin = prepare_begin(&src, "xf-2", "task-2", 1000); // 5 块
    assert_eq!(sink.begin(&begin).status, "ok");
    // 乱序推送：4,2,0,3,1。
    for seq in [4usize, 2, 0, 3, 1] {
        push_chunk(&sink, &begin, &src, seq);
    }
    sink.end(&begin.transfer_id).expect("乱序组装应成功");
    let landed = sink
        .inbox_root()
        .join("task-2")
        .join("files")
        .join("big.bin");
    assert_eq!(std::fs::read(landed).unwrap(), payload);
}

#[test]
fn resume_skips_already_received_chunks() {
    let root = temp_root("resume");
    let src = root.join("src");
    let payload = vec![7u8; 4096];
    make_source(&src, &[("data.bin", &payload)]);
    let sink = TransferSink::new(&root.join("ws"), 0);
    let begin = prepare_begin(&src, "xf-3", "task-3", 1024); // 4 块

    // 第一轮：推 0、1 块后「中断」。
    assert_eq!(sink.begin(&begin).status, "ok");
    push_chunk(&sink, &begin, &src, 0);
    push_chunk(&sink, &begin, &src, 1);

    // 第二轮（重启后）：begin 返回 have=[0,1]。
    let reply = sink.begin(&begin);
    assert_eq!(reply.status, "ok");
    assert_eq!(reply.have, vec![0, 1], "断点续传必须报告已收块");
    // 只推剩余块。
    push_chunk(&sink, &begin, &src, 2);
    push_chunk(&sink, &begin, &src, 3);
    sink.end(&begin.transfer_id).expect("续传后 end 应成功");
    assert_eq!(
        std::fs::read(
            sink.inbox_root()
                .join("task-3")
                .join("files")
                .join("data.bin")
        )
        .unwrap(),
        payload
    );
    // 成功落地后 staging 已清。
    assert!(
        !root
            .join("ws")
            .join("cluster")
            .join("inbox")
            .join(".staging")
            .join("xf-3")
            .exists()
    );
}

// ---------------------------------------------------------------------------
// 去重幂等（D3）
// ---------------------------------------------------------------------------

#[test]
fn duplicate_push_is_deduped_and_original_landing_untouched() {
    let root = temp_root("dedup");
    let src = root.join("src");
    make_source(&src, &[("r/00.request.md", b"hello")]);
    let sink = TransferSink::new(&root.join("ws"), 0);
    let begin = prepare_begin(&src, "xf-4", "task-4", 64);

    push_all(&sink, &begin, &src, &[]);
    // 重复投递（同载荷重新走全流程）→ dedup，不再收块。
    let reply = sink.begin(&begin);
    assert_eq!(reply.status, "dedup", "同 (task_id, content_hash) 必须去重");
    // 原落地未被破坏（end 未被调用第二次）。
    assert!(
        sink.inbox_root()
            .join("task-4")
            .join("files")
            .join("r")
            .join("00.request.md")
            .exists()
    );
    assert_eq!(
        sink.dedup_entry("task-4").unwrap().content_hash,
        begin.content_hash
    );

    // 去重索引持久化：新 sink（重启）同样去重。
    let sink2 = TransferSink::new(&root.join("ws"), 0);
    assert_eq!(sink2.begin(&begin).status, "dedup", "去重索引必须跨重启");
}

#[test]
fn dedup_does_not_block_repush_after_landing_moved_away() {
    // D5 关键语义：ingest 把 inbox/<task_id> 搬走后，同载荷重推必须放行
    // 重传（dedup 只在档案实体仍在收件箱时生效——否则兜底拉取被索引挡死，
    // worker 推→dedup→删发件箱→sweep 永远补不回档案）。
    let root = temp_root("dedup-moved");
    let src = root.join("src");
    make_source(&src, &[("r/x.md", b"archive")]);
    let sink = TransferSink::new(&root.join("ws"), 0);
    let begin = prepare_begin(&src, "xf-13", "task-13", 64);
    push_all(&sink, &begin, &src, &[]);
    let landed = sink.inbox_root().join("task-13");
    assert!(landed.join("landed.json").exists());

    // ingest 语义：整个任务目录被搬走。
    std::fs::remove_dir_all(&landed).unwrap();

    // 同 (task_id, content_hash) 重推 → 放行（非 dedup），可完整重落。
    assert_eq!(
        sink.begin(&begin).status,
        "ok",
        "档案实体已搬走，dedup 必须放行重传"
    );
    push_all(&sink, &begin, &src, &[]);
    assert!(landed.join("landed.json").exists(), "重传后档案重新落地");

    // 档案在场时重推仍然 dedup（不重复收块）。
    assert_eq!(sink.begin(&begin).status, "dedup");
}

#[test]
fn same_task_new_content_relands_replacing_old() {
    let root = temp_root("reland");
    let src1 = root.join("src1");
    make_source(&src1, &[("a.md", b"v1")]);
    let sink = TransferSink::new(&root.join("ws"), 0);
    push_all(
        &sink,
        &prepare_begin(&src1, "xf-5a", "task-5", 64),
        &src1,
        &[],
    );

    let src2 = root.join("src2");
    make_source(&src2, &[("a.md", b"v2-new-content")]);
    let begin2 = prepare_begin(&src2, "xf-5b", "task-5", 64);
    let reply = sink.begin(&begin2);
    assert_eq!(reply.status, "ok", "不同 content_hash 必须重落（替换旧档）");
    push_all(&sink, &begin2, &src2, &[]);
    let got = std::fs::read(sink.inbox_root().join("task-5").join("files").join("a.md")).unwrap();
    assert_eq!(got, b"v2-new-content");
}

// ---------------------------------------------------------------------------
// 体积护栏（D4）
// ---------------------------------------------------------------------------

#[test]
fn over_limit_begin_is_honestly_rejected() {
    let root = temp_root("limit");
    let src = root.join("src");
    make_source(&src, &[("big.bin", &vec![0u8; 2048])]);
    let sink = TransferSink::new(&root.join("ws"), 1024); // 护栏 1KiB < 载荷 2KiB
    let begin = prepare_begin(&src, "xf-6", "task-6", 512);
    let reply = sink.begin(&begin);
    assert_eq!(reply.status, "over_limit");
    assert!(reply.error.unwrap().contains("超护栏"));
    // 超限后调大护栏 → 同载荷可过（热刷新语义）。
    sink.set_max_bytes(0); // 0 = 不限
    assert_eq!(sink.begin(&begin).status, "ok");
}

#[test]
fn zero_limit_means_unlimited() {
    let root = temp_root("nolimit");
    let src = root.join("src");
    make_source(&src, &[("x.bin", &vec![1u8; 4096])]);
    let sink = TransferSink::new(&root.join("ws"), 0);
    assert_eq!(
        sink.begin(&prepare_begin(&src, "xf-7", "task-7", 1024))
            .status,
        "ok"
    );
}

// ---------------------------------------------------------------------------
// manifest 核验（D6）
// ---------------------------------------------------------------------------

#[test]
fn end_fails_honestly_on_corrupted_chunk_and_missing_chunk() {
    let root = temp_root("verify");
    let src = root.join("src");
    make_source(&src, &[("f.bin", &vec![9u8; 2000])]);
    let sink = TransferSink::new(&root.join("ws"), 0);
    let begin = prepare_begin(&src, "xf-8", "task-8", 1000); // 2 块

    // 缺块 end → 诚实失败。
    assert_eq!(sink.begin(&begin).status, "ok");
    let err = sink.end(&begin.transfer_id).unwrap_err();
    assert!(err.contains("缺失"), "缺块必须诚实报错: {err}");

    // 块内容与 sha 不匹配 → chunk 拒绝。
    let resp = sink.chunk(&TransferChunk {
        transfer_id: begin.transfer_id.clone(),
        seq: 0,
        total: 2,
        data_b64: b64_encode(b"tampered-data"),
        sha256: sha256_hex(&vec![9u8; 1000]),
    });
    assert!(resp.is_err(), "sha 不匹配的块必须拒绝");

    // 声明总量与 manifest 不符 → begin 校验拒绝。
    let mut bad = begin.clone();
    bad.total_bytes += 1;
    let reply = sink.begin(&bad);
    assert_eq!(reply.status, "error");
    assert!(reply.error.unwrap().contains("总量"));

    // 块数声明不符 → begin 校验拒绝。
    let mut bad2 = begin.clone();
    bad2.chunk_count += 1;
    assert_eq!(sink.begin(&bad2).status, "error");

    // 补齐正确块后 end 成功（staging 未被失败路径污染）。
    push_chunk(&sink, &begin, &src, 0);
    push_chunk(&sink, &begin, &src, 1);
    let end = sink.end(&begin.transfer_id).expect("补齐后应成功");
    assert_eq!(end.status, "ok");
}

#[test]
fn begin_rejects_invalid_paths_in_manifest() {
    let root = temp_root("badpath");
    let sink = TransferSink::new(&root.join("ws"), 0);
    let mk = |path: &str| TransferBegin {
        transfer_id: "xf-9".into(),
        task_id: "task-9".into(),
        kind: TRANSFER_KIND_EXECUTION_RECORDS.into(),
        source_node: "w".into(),
        total_bytes: 1,
        chunk_size: 64,
        chunk_count: 1,
        content_hash: "h".into(),
        files: vec![TransferFileEntry {
            path: path.into(),
            size: 1,
            sha256: "x".into(),
        }],
    };
    for bad in [
        "../escape.md",
        "a/../../b",
        "/abs/path.md",
        "C:\\x.md",
        "a\\b.md",
        "X:/drive.md",
        "dir~1/name.md",
        "",
    ] {
        let reply = sink.begin(&mk(bad));
        assert_eq!(reply.status, "error", "路径 {bad:?} 必须被围栏拒绝");
    }
    // 合法相对路径放行。
    assert_eq!(sink.begin(&mk("logs/cluster_logs/t/00.md")).status, "ok");
}

#[test]
fn safe_relative_path_unit_matrix() {
    for bad in [
        "..",
        "../x",
        "a/../b",
        "/abs",
        "C:/x",
        "C:\\x",
        "back\\slash.md",
        "colon:name.md",
        "short~1.md",
        "dir/name~9.txt",
        "",
        "   ",
    ] {
        assert!(safe_relative_path(bad).is_err(), "{bad:?} 应拒绝");
    }
    for ok in [
        "a.md",
        "logs/cluster_logs/t-1/00.request.md",
        "中文 目录/文件.md",
        "a..b.md",
        "x~.md",
    ] {
        assert!(safe_relative_path(ok).is_ok(), "{ok:?} 应放行");
    }
}

// ---------------------------------------------------------------------------
// 先落盘后 ACK 顺序（D3）
// ---------------------------------------------------------------------------

#[test]
fn chunk_ack_implies_bytes_on_disk() {
    let root = temp_root("ack");
    let src = root.join("src");
    make_source(&src, &[("f.bin", &vec![3u8; 1500])]);
    let sink = TransferSink::new(&root.join("ws"), 0);
    let begin = prepare_begin(&src, "xf-10", "task-10", 1000);
    assert_eq!(sink.begin(&begin).status, "ok");
    push_chunk(&sink, &begin, &src, 0);
    // ACK 之后、end 之前：字节必须已在 staging 磁盘上（master 崩溃也不丢
    // 已 ACK 的数据——重启后 begin resume 能看到）。
    let st = root
        .join("ws")
        .join("cluster")
        .join("inbox")
        .join(".staging")
        .join("xf-10");
    let on_disk = std::fs::read(st.join("chunk_000000.bin")).unwrap();
    assert_eq!(on_disk, vec![3u8; 1000]);
    // 同块重推幂等（覆盖写）。
    push_chunk(&sink, &begin, &src, 0);
    // chunk 无 begin 直接推 → 拒绝（协议顺序强制）。
    let orphan = sink.chunk(&TransferChunk {
        transfer_id: "xf-never-begun".into(),
        seq: 0,
        total: 1,
        data_b64: b64_encode(b"x"),
        sha256: sha256_hex(b"x"),
    });
    assert!(orphan.is_err());
}

// ---------------------------------------------------------------------------
// 落地 / 超限回调
// ---------------------------------------------------------------------------

#[test]
fn landed_callback_fires_with_task_and_dir() {
    let root = temp_root("cb");
    let src = root.join("src");
    make_source(&src, &[("r/x.md", b"data")]);
    let sink = Arc::new(TransferSink::new(&root.join("ws"), 0));
    let hits: Arc<std::sync::Mutex<Vec<(String, PathBuf)>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let hits2 = hits.clone();
    sink.set_on_landed(Arc::new(move |task_id, dir| {
        hits2
            .lock()
            .unwrap()
            .push((task_id.to_string(), dir.to_path_buf()));
    }));
    let begin = prepare_begin(&src, "xf-11", "task-11", 64);
    push_all(&sink, &begin, &src, &[]);
    let got = hits.lock().unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].0, "task-11");
    assert!(got[0].1.ends_with("task-11"));
}

#[test]
fn overlimit_callback_fires() {
    let root = temp_root("ol");
    let sink = TransferSink::new(&root.join("ws"), 0);
    let hits: Arc<std::sync::Mutex<Vec<TransferOverlimit>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let hits2 = hits.clone();
    sink.set_on_overlimit(Arc::new(move |req| hits2.lock().unwrap().push(req.clone())));
    sink.note_overlimit(&TransferOverlimit {
        task_id: "task-12".into(),
        source_node: "w".into(),
        total_bytes: 100,
        limit: 50,
        kind: TRANSFER_KIND_EXECUTION_RECORDS.into(),
    });
    assert_eq!(hits.lock().unwrap().len(), 1);
}

// ---------------------------------------------------------------------------
// 收集器 / 块大小解析
// ---------------------------------------------------------------------------

#[test]
fn collect_dir_files_is_sorted_deterministic_and_fenced() {
    let root = temp_root("collect");
    let src = root.join("src");
    make_source(
        &src,
        &[("b/02.md", b"2"), ("b/01.md", b"1"), ("a.md", b"0")],
    );
    let files = collect_dir_files(&src).unwrap();
    let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, vec!["a.md", "b/01.md", "b/02.md"], "字典序确定性");
    // content_hash 稳定：重收集一致。
    assert_eq!(
        content_hash(&files),
        content_hash(&collect_dir_files(&src).unwrap())
    );
    // 空目录 = 空 manifest。
    assert!(collect_dir_files(&root.join("nope")).unwrap().is_empty());
    // 统一分隔符：Windows 下盘上 rel 用 '\\'，收集产物必须归一 '/'。
    assert!(files.iter().all(|f| !f.path.contains('\\')));
}

#[test]
fn collect_dir_files_excludes_vcs_internal_dirs() {
    // VCS 内部目录（任意层级的 .git）不进收集产物——混入变更集会让 master
    // 三方合并 upsert 保留路径失败（2026-09-16 showcase 实证）。
    let root = temp_root("collect_vcs");
    let src = root.join("src");
    make_source(
        &src,
        &[
            ("strutil.py", b"x"),
            (".git/COMMIT_EDITMSG", b"feat: x"),
            (".git/objects/ab/cdef", b"obj"),
            ("pkg/.git/HEAD", b"ref: refs/heads/master"),
            ("pkg/keep.txt", b"keep"),
        ],
    );
    let files = collect_dir_files(&src).unwrap();
    let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        paths,
        vec!["pkg/keep.txt", "strutil.py"],
        ".git 整棵排除，工作文件保留"
    );
}

#[test]
fn collect_dir_files_excludes_build_noise() {
    // S-O3：无争议构建垃圾（字节码/缓存/依赖目录/系统垃圾文件）不进收集
    // 产物——2026-09-16 showcase NB-5 变更集只剩 __pycache__/*.pyc 实证。
    // 通用名（target/dist/build）不排——防静默丢真实交付。
    let root = temp_root("collect_noise");
    let src = root.join("src");
    make_source(
        &src,
        &[
            ("app.py", b"x"),
            ("__pycache__/app.cpython-313.pyc", b"bytecode"),
            ("pkg/__pycache__/util.cpython-313.pyc", b"bc2"),
            ("run.pyc", b"loose pyc"),
            ("node_modules/left-pad/index.js", b"dep"),
            (".pytest_cache/v/cache/lastfailed", b"cache"),
            (".venv/lib/py.py", b"venv"),
            ("nested/.DS_Store", b"macos"),
            ("nested/Thumbs.db", b"win"),
            ("nested/keep.py", b"keep"),
            ("target/output.bin", b"kept: generic name"),
        ],
    );
    let files = collect_dir_files(&src).unwrap();
    let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        paths,
        vec!["app.py", "nested/keep.py", "target/output.bin"],
        "构建垃圾整棵排除；通用名目录保留"
    );
}

#[test]
fn sanitize_transfer_id_and_chunk_size_resolution() {
    assert_eq!(sanitize_transfer_id("abc-123_X.y"), "abc-123_X.y");
    assert_eq!(sanitize_transfer_id("a/b\\c:d"), "a_b_c_d");
    assert_eq!(sanitize_transfer_id(""), "transfer");
    // SAN-03：`..` 曾原样放行（staging 逃一级目录）——点守卫折叠。
    assert_eq!(sanitize_transfer_id(".."), "__");
    assert_eq!(sanitize_transfer_id("../../etc"), "______etc");
    // 块大小解析（纯函数）：合法值透传，越界/缺失回落默认。
    assert_eq!(resolve_chunk_bytes(Some(8192)), 8192);
    assert_eq!(resolve_chunk_bytes(Some(4096)), 4096);
    assert_eq!(resolve_chunk_bytes(Some(8 * 1024 * 1024)), 8 * 1024 * 1024);
    assert_eq!(resolve_chunk_bytes(Some(999)), DEFAULT_CHUNK_BYTES);
    assert_eq!(
        resolve_chunk_bytes(Some(99 * 1024 * 1024)),
        DEFAULT_CHUNK_BYTES
    );
    assert_eq!(resolve_chunk_bytes(None), DEFAULT_CHUNK_BYTES);
}
