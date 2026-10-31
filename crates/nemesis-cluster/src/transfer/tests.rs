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

/// 逻辑相对路径（`/` 分隔）→ 盘上路径（逐段 join，分隔符随平台）。
///
/// 不得用 `replace('/', "\\")`——`\` 只在 Windows 是分隔符，Linux 上会
/// 把嵌套路径写成「名字带 `\` 的单个怪文件」，collect 的排除判定按组件
/// 匹配看不到 `.git` 等目录 → 排除测试全灭（2026-09-18 CI Linux 实录）。
fn disk_path(root: &Path, rel: &str) -> PathBuf {
    let mut p = root.to_path_buf();
    for seg in rel.split('/') {
        p.push(seg);
    }
    p
}

/// 构造发送目录：`files/` 下写给定 (相对路径, 内容)。
fn make_source(root: &Path, files: &[(&str, &[u8])]) {
    for (rel, data) in files {
        let p = disk_path(root, rel);
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

/// 取文件字节（逻辑相对路径 → 盘上路径，逐段 join 平台无关）。
fn file_bytes(src: &Path, rel: &str) -> Vec<u8> {
    std::fs::read(disk_path(src, rel)).unwrap()
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
        let got = std::fs::read(disk_path(&landed.join("files"), &f.path)).unwrap();
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

/// BUG-20261030-198：接收端 chunk 写入中途崩溃会留下撕裂半块
/// （裸 fs::write 非原子）。begin 续传的 have 不得把「长度与计划不符的
/// 半块」当已收——否则 sender 永久跳过它，end 组装长度失配诚实 Err，
/// resume 永远信任半块 → 传输死循环直至 staging 被按龄清扫（7 天）。
/// 长度不符 = 不进 have，sender 忠实重发覆盖自愈。
#[test]
fn bug198_torn_chunk_not_reported_in_resume_have() {
    let root = temp_root("bug198");
    let src = root.join("src");
    let payload = vec![9u8; 4096];
    make_source(&src, &[("data.bin", &payload)]);
    let sink = TransferSink::new(&root.join("ws"), 0);
    let begin = prepare_begin(&src, "xf-b198", "task-b198", 1024); // 4 块

    // 第一轮：推 0、1 块。
    assert_eq!(sink.begin(&begin).status, "ok");
    push_chunk(&sink, &begin, &src, 0);
    push_chunk(&sink, &begin, &src, 1);

    // 模拟崩溃撕裂：chunk 1 被截成半块（crash mid-fs::write 残留形态）。
    let st = root
        .join("ws")
        .join("cluster")
        .join("inbox")
        .join(".staging")
        .join("xf-b198");
    let torn = st.join("chunk_000001.bin");
    let full = std::fs::read(&torn).unwrap();
    std::fs::write(&torn, &full[..full.len() / 2]).unwrap();

    // 第二轮（重启后续传）：撕裂的 1 不得进 have。
    let reply = sink.begin(&begin);
    assert_eq!(reply.status, "ok");
    assert_eq!(reply.have, vec![0], "长度不符的半块不得进 have");
    // sender 按 have 忠实重推（半块被重发覆盖）→ end 成功。
    for seq in 0..begin.chunk_count {
        if !reply.have.contains(&seq) {
            push_chunk(&sink, &begin, &src, seq);
        }
    }
    sink.end(&begin.transfer_id)
        .expect("半块被重发覆盖后 end 应成功");
    assert_eq!(
        std::fs::read(
            sink.inbox_root()
                .join("task-b198")
                .join("files")
                .join("data.bin")
        )
        .unwrap(),
        payload
    );
}

/// BUG-20261030-198 配套：seq → 计划块长算术（不物化计划）的边界形态
/// ——末块短块、空文件不占序号、跨文件序号连续、越界 None。
#[test]
fn bug198_planned_chunk_len_arithmetic() {
    let mk = |path: &str, size: u64| TransferFileEntry {
        path: path.into(),
        size,
        sha256: String::new(),
    };
    let files = vec![mk("empty", 0), mk("a", 2500), mk("b", 1024)];
    // cs=1000：empty→0 块；a→3 块 (1000,1000,500)；b→2 块 (1000,24)。
    assert_eq!(planned_chunk_len(&files, 1000, 0), Some(1000));
    assert_eq!(planned_chunk_len(&files, 1000, 1), Some(1000));
    assert_eq!(planned_chunk_len(&files, 1000, 2), Some(500));
    assert_eq!(planned_chunk_len(&files, 1000, 3), Some(1000));
    assert_eq!(planned_chunk_len(&files, 1000, 4), Some(24));
    assert_eq!(planned_chunk_len(&files, 1000, 5), None);
    assert_eq!(planned_chunk_len(&files, 1000, usize::MAX), None);
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

/// T8（2026-10-17 三轮审计）：远端可控 task_id 落地目录消毒——`../` 形态
/// 不得逃出 inbox；去重探针与落地同源（重传命中 dedup）。
#[test]
fn hostile_task_id_lands_inside_inbox() {
    let root = temp_root("t8");
    let src = root.join("src");
    make_source(&src, &[("r/x.md", b"data")]);
    let sink = TransferSink::new(&root.join("ws"), 0);
    let begin = prepare_begin(&src, "xf-t8", "../escape", 64);
    push_all(&sink, &begin, &src, &[]);

    let inbox = root.join("ws").join("cluster").join("inbox");
    // 旧实现裸拼会落到 inbox/../escape = cluster/escape。
    assert!(
        !root.join("ws").join("cluster").join("escape").exists(),
        "task_id 路径逃逸必须被消毒拦截"
    );
    // 落地目录 = 白名单消毒段（`../escape` → `___escape`），恰一个。
    let mut landed: Vec<String> = std::fs::read_dir(&inbox)
        .unwrap()
        .flatten()
        .filter_map(|e| e.file_name().to_str().map(String::from))
        .filter(|n| !n.starts_with('.'))
        .collect();
    landed.sort();
    assert_eq!(landed, vec!["___escape".to_string()], "实际: {landed:?}");

    // 落地目录内容完整（files + 回执）。
    let landed_dir = inbox.join("___escape");
    assert!(landed_dir.join("landed.json").exists());
    assert!(landed_dir.join("files").join("r").join("x.md").exists());

    // 去重探针同源：同 (task_id, content_hash) 重传命中 dedup。
    let reply = sink.begin(&prepare_begin(&src, "xf-t8-2", "../escape", 64));
    assert_eq!(reply.status, "dedup", "消毒名探针必须命中已落地档案");
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

// ---------------------------------------------------------------------------
// sweep_stale_staging（BUG-2，2026-10-17）：孤儿分块目录按龄清扫
// ---------------------------------------------------------------------------

// max_age=0：所有分块目录视为过期，全部清除；非目录条目不碰；
// staging 根自身保留（下轮传输还要用）。
#[test]
fn test_sweep_stale_staging_zero_age_clears_all_dirs() {
    let root = temp_root("sweep-all");
    let sink = TransferSink::new(&root.join("ws"), 0);
    let staging = sink.inbox_root().join(".staging");
    for id in ["tr-a", "tr-b"] {
        std::fs::create_dir_all(staging.join(id)).unwrap();
        std::fs::write(staging.join(id).join("chunk_0001"), b"payload").unwrap();
    }
    std::fs::write(staging.join("stray-file.tmp"), b"not a dir").unwrap();

    let removed = sink.sweep_stale_staging(std::time::Duration::ZERO);
    assert_eq!(removed, 2, "两个孤儿目录都被清除");
    assert!(!staging.join("tr-a").exists());
    assert!(!staging.join("tr-b").exists());
    assert!(staging.join("stray-file.tmp").exists(), "非目录条目不碰");
    assert!(staging.exists(), "staging 根保留");

    let _ = std::fs::remove_dir_all(&root);
}

// max_age 超大：进行中/新鲜目录一律保留（返回 0）。
#[test]
fn test_sweep_stale_staging_large_age_keeps_everything() {
    let root = temp_root("sweep-keep");
    let sink = TransferSink::new(&root.join("ws"), 0);
    let staging = sink.inbox_root().join(".staging");
    std::fs::create_dir_all(staging.join("tr-live")).unwrap();
    std::fs::write(staging.join("tr-live").join("chunk_0000"), b"x").unwrap();

    let removed = sink.sweep_stale_staging(std::time::Duration::from_secs(100 * 24 * 3600));
    assert_eq!(removed, 0, "新鲜目录不被清扫");
    assert!(staging.join("tr-live").exists());

    let _ = std::fs::remove_dir_all(&root);
}

// staging 根不存在（传输栈未用过）→ 静默 0，不 panic。
#[test]
fn test_sweep_stale_staging_missing_root_is_zero() {
    let root = temp_root("sweep-missing");
    let sink = TransferSink::new(&root.join("ws"), 0);
    // TransferSink::new 会建 staging 根——删掉模拟未初始化盘面。
    let staging = sink.inbox_root().join(".staging");
    std::fs::remove_dir_all(&staging).unwrap();
    assert_eq!(sink.sweep_stale_staging(std::time::Duration::ZERO), 0);
    let _ = std::fs::remove_dir_all(&root);
}

// LOW-7（2026-10-17 四轮审计）：chunk 按 staging 计划双闸——序号越界与
// 单块超尺寸在写盘前诚实拒绝（此前远端可控 seq/len 可写任意多/任意大的
// 块文件进 staging = 磁盘填充面）。
#[test]
fn chunk_rejects_seq_and_size_beyond_staging_plan() {
    let root = temp_root("plan");
    let src = root.join("src");
    make_source(&src, &[("f.bin", &vec![7u8; 1500])]);
    let sink = TransferSink::new(&root.join("ws"), 0);
    let begin = prepare_begin(&src, "xf-plan", "task-plan", 1000); // 2 块
    let data = vec![7u8; 1000];
    let mk = |seq: usize, payload: &[u8]| TransferChunk {
        transfer_id: begin.transfer_id.clone(),
        seq,
        total: begin.chunk_count,
        data_b64: b64_encode(payload),
        sha256: sha256_hex(payload),
    };

    // begin 前（staging 目录都不在）：诚实拒绝。
    let err = sink.chunk(&mk(0, &data)).unwrap_err();
    assert!(err.contains("staging 不存在"), "{err}");

    // staging 在场但 manifest 未落（begin 窄窗口）：诚实拒绝，sender 重试。
    std::fs::create_dir_all(sink.staging_dir(&begin.transfer_id)).unwrap();
    let err = sink.chunk(&mk(0, &data)).unwrap_err();
    assert!(err.contains("begin 未完成"), "{err}");

    // begin 后：越界 seq 拒绝（chunk_count=2，seq=2 越界）。
    assert_eq!(sink.begin(&begin).status, "ok");
    let err = sink.chunk(&mk(2, &data)).unwrap_err();
    assert!(err.contains("块序号 2 超出计划"), "{err}");

    // 单块超尺寸拒绝（计划 1000B，推 1001B）。
    let big = vec![7u8; 1001];
    let err = sink.chunk(&mk(0, &big)).unwrap_err();
    assert!(err.contains("块大小 1001 超出计划"), "{err}");

    // 计划内合法块照常收（闸不误伤）。
    push_chunk(&sink, &begin, &src, 0);
    push_chunk(&sink, &begin, &src, 1);
    let end = sink.end(&begin.transfer_id).expect("计划内块必须正常收全");
    assert_eq!(end.status, "ok");
}

// LOW-8 回归锁（hash-of-hash 事故，2026-10-17）：流式哈希必须与整读
// sha256_hex 同值——首版实现把 finalize 摘要又哈希了一遍，收发两侧同错
// 自洽互证通过，跨函数比较（diff_walk 声明 vs 变更集写侧重验）当场现形。
#[test]
fn streaming_hash_matches_whole_read_hash() {
    let root = temp_root("sh");
    std::fs::create_dir_all(&root).unwrap();
    let p = root.join("blob.bin");
    // 30 万字节 > 256KiB 单缓冲，强制多块路径。
    let payload: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(&p, &payload).unwrap();
    let (size, hash) = sha256_file_streaming(&p).unwrap();
    assert_eq!(size, payload.len() as u64);
    assert_eq!(hash, sha256_hex(&payload), "流式与整读必须同值");
}

// ---------------------------------------------------------------------------
// BUG-20261030-202 / BUG-20261030-203：回调锁卫生 + manifest 条数闸
// ---------------------------------------------------------------------------

/// BUG-20261030-202：回调 panic 不毒化锁（恢复式，后续接线照常生效）；
/// 回调内重入 set_on_overlimit（同一把锁）不死锁——消费点锁内 clone、
/// 锁外调用（旧形态持锁调回调在此永久阻塞）。
#[test]
fn callback_panics_do_not_poison_and_reentrant_set_is_safe() {
    let root = temp_root("cb-hygiene");
    let sink = Arc::new(TransferSink::new(&root, 0));
    let req = TransferOverlimit {
        task_id: "t-cb".into(),
        source_node: "node-a".into(),
        total_bytes: 1,
        limit: 0,
        kind: TRANSFER_KIND_EXECUTION_RECORDS.into(),
    };
    // 毒化恢复：panic 回调照常传播（不吞），锁不中毒——新回调可接线并执行。
    sink.set_on_overlimit(Arc::new(|_| panic!("boom")));
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        sink.note_overlimit(&req);
    }));
    assert!(r.is_err(), "回调 panic 照常传播");
    let hit = Arc::new(AtomicU32::new(0));
    let h2 = hit.clone();
    sink.set_on_overlimit(Arc::new(move |_| {
        h2.fetch_add(1, Ordering::SeqCst);
    }));
    sink.note_overlimit(&req);
    assert_eq!(hit.load(Ordering::SeqCst), 1, "毒化后新回调照常执行");
    // 重入安全：回调内再 set_on_overlimit（同一把锁）。
    let weak = Arc::downgrade(&sink);
    sink.set_on_overlimit(Arc::new(move |_| {
        if let Some(s) = weak.upgrade() {
            s.set_on_overlimit(Arc::new(|_| {}));
        }
    }));
    sink.note_overlimit(&req);
}

/// BUG-20261030-203：files 条数超闸 → 诚实拒绝（可诊断错误 + 打包建议），
/// 不再放行到传输层撞 16MiB 帧；闸值内通过结构校验。
#[test]
fn manifest_file_count_gate() {
    let mk = |n: usize| TransferBegin {
        transfer_id: "tr-gate".into(),
        task_id: "t-gate".into(),
        kind: TRANSFER_KIND_EXECUTION_RECORDS.into(),
        source_node: "node-a".into(),
        total_bytes: 0,
        chunk_size: 64 * 1024,
        chunk_count: 0,
        content_hash: "h".into(),
        files: (0..n)
            .map(|i| TransferFileEntry {
                path: format!("f/{i}.txt"),
                size: 0,
                sha256: String::new(),
            })
            .collect(),
    };
    let over = mk(MAX_MANIFEST_FILES + 1);
    let err = validate_manifest(&over).unwrap_err();
    assert!(err.contains("条目"), "错误指明条数超闸: {err}");
    let ok = mk(MAX_MANIFEST_FILES);
    assert!(validate_manifest(&ok).is_ok(), "闸值内通过结构校验");
}
