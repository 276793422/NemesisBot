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

/// BUG-20261123-009A：续传 resume 判定必须同 chunk_size。content_hash 是
/// 文件清单指纹不含块大小——同 transfer_id + 同清单 + 异 chunk_size 的
/// begin 若被当续传，旧计划的块序号到新计划错位（长度恰好一致的块混进
/// have），end 组装 sha256 兜住报 Err，sender 重试 begin 又拿同一批错位
/// have = 死循环，且重试刷新 staging mtime 令按龄清扫永不清（卡死）。
/// 修复：chunk_size 不同 = 计划不同 = 清掉旧 staging 全新开始（have 空）。
#[test]
fn bug20261123_009a_chunk_size_mismatch_rejects_resume() {
    let root = temp_root("bug009a");
    let src = root.join("src");
    let payload = vec![3u8; 4096];
    make_source(&src, &[("data.bin", &payload)]);
    let sink = TransferSink::new(&root.join("ws"), 0);
    // 同 transfer_id / 同 task_id / 同内容；块大小 1024 → 512 变更。
    let begin = prepare_begin(&src, "xf-b009a", "task-b009a", 1024); // 4 块

    // 第一轮：旧块大小推 0、1 块后「中断」。
    assert_eq!(sink.begin(&begin).status, "ok");
    push_chunk(&sink, &begin, &src, 0);
    push_chunk(&sink, &begin, &src, 1);

    // 第二轮：同 id 同 hash 但块大小变更——不得当续传（have 必须为空，
    // 全量重发自愈；旧块在新计划下错位不可信）。
    let begin2 = prepare_begin(&src, "xf-b009a", "task-b009a", 512); // 8 块
    assert_eq!(
        begin2.content_hash, begin.content_hash,
        "前置：同清单同指纹"
    );
    let reply = sink.begin(&begin2);
    assert_eq!(reply.status, "ok");
    assert!(
        reply.have.is_empty(),
        "chunk_size 变更不得复用旧块：have = {:?}",
        reply.have
    );
    // 全量重推（新计划 8 块）→ end 成功，落地内容逐字节一致。
    let end = push_all(&sink, &begin2, &src, &[]);
    assert_eq!(end.status, "ok");
    assert_eq!(
        std::fs::read(
            sink.inbox_root()
                .join("task-b009a")
                .join("files")
                .join("data.bin")
        )
        .unwrap(),
        payload
    );
}

/// BUG-20261123-009B：begin 的 content_hash 必须与 files 清单一致。去重
/// 闸与确定性 transfer_id 全建立在此指纹上，只查非空 = 远端可任意声明
/// ——同 task_id 换内容重传（worker 改进交付）声明旧 hash 可误命中
/// dedup，新交付被假命中挡死。receiver 零成本重算比对 = 防线闭环。
#[test]
fn bug20261123_009b_declared_content_hash_must_match_manifest() {
    let root = temp_root("bug009b");
    let src = root.join("src");
    make_source(&src, &[("data.bin", &[1u8; 256])]);
    let sink = TransferSink::new(&root.join("ws"), 0);
    let mut begin = prepare_begin(&src, "xf-b009b", "task-b009b", 1024);

    // 声明一个非空但与清单不符的指纹（64 hex 形态，最迷惑的伪造）。
    begin.content_hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".into();
    let reply = sink.begin(&begin);
    assert_eq!(reply.status, "error", "假指纹必须诚实拒绝");
    assert!(
        reply
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("content_hash 与清单不一致"),
        "错误须指明指纹不一致：{:?}",
        reply.error
    );
    // 同批载荷指纹改回真值即通过（排除「载荷本身非法」假阳性）。
    let files = collect_dir_files(&src).unwrap();
    begin.content_hash = content_hash(&files);
    assert_eq!(sink.begin(&begin).status, "ok");
}

/// BUG-20261123-009B 配套：chunk_size 续传闸不影响合法续传——同 id 同
/// hash 同 chunk_size 的 resume 行为与修复前完全一致（回归防线）。
#[test]
fn bug20261123_009_same_chunk_size_resume_unchanged() {
    let root = temp_root("bug009ctl");
    let src = root.join("src");
    let payload = vec![5u8; 3072];
    make_source(&src, &[("data.bin", &payload)]);
    let sink = TransferSink::new(&root.join("ws"), 0);
    let begin = prepare_begin(&src, "xf-b009c", "task-b009c", 1024); // 3 块

    assert_eq!(sink.begin(&begin).status, "ok");
    push_chunk(&sink, &begin, &src, 0);
    let reply = sink.begin(&begin);
    assert_eq!(reply.status, "ok");
    assert_eq!(reply.have, vec![0], "同 chunk_size 合法续传不受影响");
    // 只推剩余块，续传完成。
    push_chunk(&sink, &begin, &src, 1);
    push_chunk(&sink, &begin, &src, 2);
    sink.end(&begin.transfer_id)
        .expect("同块大小续传 end 应成功");
    assert_eq!(
        std::fs::read(
            sink.inbox_root()
                .join("task-b009c")
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

/// BUG-20261123-042 回归锁：list_received_chunks 不得按**远端声明的**
/// chunk_count 逐 seq stat（声明值只过一致性对账、绝对量不受体积护栏
/// 钳制——total=2GiB + chunk_size=1 → 声明 2^31 块 = 21 亿次不存在的
/// stat = begin 处理线程挂死数小时/次，重发 begin 可重复触发）。契约：
/// have 只从**实际落盘**的 chunk 文件枚举，声明量无关；升序；非块文件
/// 与畸形名忽略；长度不匹配 planned_chunk_len 的块不算已收。
#[test]
fn bug20261123_042_list_received_chunks_derives_from_actual_files_only() {
    let mk = |path: &str, size: u64| TransferFileEntry {
        path: path.into(),
        size,
        sha256: String::new(),
    };
    // 3 块计划（cs=1000, size=2500）：末块 500。staging 只落了 seq 0/2
    // （乱序写盘），另有 manifest.json 与畸形名干扰项；seq 1 缺失、
    // seq 2 长度对得上。
    let dir = temp_root("bug042-received-chunks");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("chunk_000002.bin"), vec![0u8; 500]).unwrap();
    std::fs::write(dir.join("chunk_000000.bin"), vec![0u8; 1000]).unwrap();
    std::fs::write(dir.join("manifest.json"), b"{}").unwrap();
    std::fs::write(dir.join("chunk_garbage.bin"), b"x").unwrap();
    // 声明量与实际无关地巨大（旧实现此处 = 按声明量 stat 循环，测试挂死）。
    let begin = TransferBegin {
        transfer_id: "t".into(),
        task_id: "task".into(),
        kind: String::new(),
        source_node: String::new(),
        total_bytes: 2500,
        chunk_size: 1000,
        chunk_count: 2_000_000_000,
        files: vec![mk("a", 2500)],
        content_hash: String::new(),
    };
    let have = list_received_chunks(&dir, &begin);
    assert_eq!(have, vec![0, 2], "只枚举实际落盘且长度匹配的块，升序");
    // 空目录/不存在的 staging = 空 have（旧语义保持）。
    let empty = temp_root("bug042-received-chunks-empty");
    let have_empty = list_received_chunks(&empty, &begin);
    assert!(have_empty.is_empty());
}

/// BUG-20261123-042 组装面回归锁：end 必须在 plan_chunks 物化**之前**
/// 按算术块数拒绝无界声明（声明值与 sizes/chunk_size 自洽即可过
/// validate——绝对量不受体积护栏钳制：max_bytes=0 不限 + chunk_size=4096
/// + total=20GB → 4.88M 块计划 = 无界物化）。
///
/// 诚实对端不受影响：默认 2GiB 护栏 ÷ 4KiB 最小块 = 524,288 块恒在 4M 闸内。
#[test]
fn bug20261123_042_end_refuses_unbounded_chunk_plan() {
    let root = temp_root("bug042-end-cap");
    let sink = TransferSink::new(&root.join("ws"), 0); // 0=不限体积护栏
    let mk = |path: &str, size: u64| TransferFileEntry {
        path: path.into(),
        size,
        sha256: "x".into(),
    };
    let files = vec![mk("big.bin", 20_000_000_000)];
    let begin = TransferBegin {
        transfer_id: "xf-042cap".into(),
        task_id: "task-042cap".into(),
        kind: TRANSFER_KIND_EXECUTION_RECORDS.into(),
        source_node: "node-w".into(),
        total_bytes: 20_000_000_000,
        chunk_size: 4096,
        chunk_count: chunk_count(&files, 4096), // 4,882,812 > 4M，一致性成立
        content_hash: content_hash(&files),
        files,
    };
    // begin 过（一致性对账只查自洽）；end 必须在物化前拒绝。
    let reply = sink.begin(&begin);
    assert_eq!(
        reply.status, "ok",
        "begin 一致性校验应通过: {:?}",
        reply.error
    );
    let err = sink.end(&begin.transfer_id).unwrap_err();
    assert!(err.contains("超上限"), "end 应按块数上限拒绝: {err}");
    assert!(
        err.contains("chunk_size"),
        "错误应给增大 chunk_size 的指引: {err}"
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
    // content_hash 必须与清单一致（BUG-20261123-009B 校验）——夹具按实
    // 构造真指纹，被测维度（路径围栏）外的载荷保持合法。
    let mk = |path: &str| {
        let files = vec![TransferFileEntry {
            path: path.into(),
            size: 1,
            sha256: "x".into(),
        }];
        let content_hash = content_hash(&files);
        TransferBegin {
            transfer_id: "xf-9".into(),
            task_id: "task-9".into(),
            kind: TRANSFER_KIND_EXECUTION_RECORDS.into(),
            source_node: "w".into(),
            total_bytes: 1,
            chunk_size: 64,
            chunk_count: 1,
            content_hash,
            files,
        }
    };
    for bad in [
        "../escape.md",
        "a/../../b",
        "/abs/path.md",
        "C:\\x.md",
        "a\\b.md",
        "X:/drive.md",
        "dir~1/name.md",
        // Windows 保留设备名 wire 级闸（BUG-20261124-030）：size=0 的 NUL
        // 形态曾是静默完整性破坏（组装写进设备→空核验恒过→ACK ok 但落地
        // 缺文件）。
        "NUL",
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
        // Windows 保留设备名（BUG-20261124-030）：裸名/带扩展/大小写/嵌套
        // 全拒；Linux worker 交付树可合法含这些名（意外触发面）。
        "CON",
        "con.txt",
        "NUL",
        "nul",
        "COM1",
        "lpt9",
        "AUX.log",
        "sub/CON/data.md",
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
        // 仅前缀重合的普通名不误伤（全词匹配）。
        "CONST.md",
        "console.log",
        "sub/NULLIFY.md",
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

// ---------------------------------------------------------------------------
// sweep_stale_landed（横扫四轮，2026-11-06）：已落地未消费任务目录按龄清扫
// ---------------------------------------------------------------------------

// max_age=0：过期落地目录整删 + dedup 条目同步摘除并持久化；隐藏抽屉
// （.done）、去重索引文件、非目录条目不碰。
#[test]
fn test_sweep_stale_landed_zero_age_clears_dirs_and_dedup() {
    let root = temp_root("landed-sweep");
    let sink = TransferSink::new(&root.join("ws"), 0);
    // 真实落地两个任务（begin→chunk→end 全流程，dedup 条目同源生成）。
    let src_a = root.join("src-a");
    make_source(&src_a, &[("r.md", b"archive a")]);
    let begin_a = prepare_begin(&src_a, "tr-la", "task-la", 1000);
    push_all(&sink, &begin_a, &src_a, &[]);
    let src_b = root.join("src-b");
    make_source(&src_b, &[("r.md", b"archive b")]);
    let begin_b = prepare_begin(&src_b, "tr-lb", "task-lb", 1000);
    push_all(&sink, &begin_b, &src_b, &[]);
    let inbox = sink.inbox_root();
    assert!(inbox.join("task-la").is_dir());
    assert!(
        sink.dedup_entry("task-la").is_some(),
        "前置：dedup 条目在场"
    );
    assert!(inbox.join(".done").is_dir(), "前置：end 已建 .done 抽屉");
    assert!(inbox.join(".dedup.json").is_file(), "前置：索引已持久化");
    // 手工残留形态：非目录条目。
    std::fs::write(inbox.join("stray-file.tmp"), b"not a dir").unwrap();

    let removed = sink.sweep_stale_landed(std::time::Duration::ZERO);
    assert_eq!(removed, 2, "两个未消费落地目录都被清除");
    assert!(!inbox.join("task-la").exists());
    assert!(!inbox.join("task-lb").exists());
    assert!(sink.dedup_entry("task-la").is_none(), "dedup 条目同步摘除");
    assert!(sink.dedup_entry("task-lb").is_none());
    assert!(inbox.join("stray-file.tmp").exists(), "非目录条目不碰");
    assert!(inbox.join(".done").is_dir(), "隐藏抽屉不碰");
    assert!(inbox.join(".dedup.json").is_file(), "去重索引文件不碰");
    // 摘除已持久化：新 sink 重载索引后条目仍缺席。
    let sink2 = TransferSink::new(&root.join("ws"), 0);
    assert!(
        sink2.dedup_entry("task-la").is_none(),
        "摘除写入 .dedup.json"
    );

    let _ = std::fs::remove_dir_all(&root);
}

// max_age 超大：新鲜落地目录一律保留（正常驻留不受影响）。
#[test]
fn test_sweep_stale_landed_large_age_keeps_everything() {
    let root = temp_root("landed-keep");
    let sink = TransferSink::new(&root.join("ws"), 0);
    let src = root.join("src");
    make_source(&src, &[("r.md", b"archive")]);
    let begin = prepare_begin(&src, "tr-lk", "task-lk", 1000);
    push_all(&sink, &begin, &src, &[]);

    let removed = sink.sweep_stale_landed(std::time::Duration::from_secs(100 * 24 * 3600));
    assert_eq!(removed, 0, "新鲜落地目录不被清扫");
    assert!(sink.inbox_root().join("task-lk").is_dir());
    assert!(sink.dedup_entry("task-lk").is_some(), "条目原样保留");

    let _ = std::fs::remove_dir_all(&root);
}

// sweep_stale_done_receipts（横扫六轮，2026-11-06）：幂等重放回执按龄清扫
// ---------------------------------------------------------------------------

// max_age=0：过期回执清除；抽屉内非 .json 条目与抽屉外条目不碰。
#[test]
fn test_sweep_stale_done_receipts_zero_age_clears_receipts() {
    let root = temp_root("done-sweep");
    let sink = TransferSink::new(&root.join("ws"), 0);
    // 真实 end 两次 → .done 抽屉两份真实回执。
    for (tr, task) in [("tr-da", "task-da"), ("tr-db", "task-db")] {
        let src = root.join(format!("src-{tr}"));
        make_source(&src, &[("r.md", b"archive")]);
        let begin = prepare_begin(&src, tr, task, 1000);
        push_all(&sink, &begin, &src, &[]);
    }
    let done = sink.inbox_root().join(".done");
    assert!(done.join("tr-da.json").is_file(), "前置：回执 A 在场");
    assert!(done.join("tr-db.json").is_file(), "前置：回执 B 在场");
    // 手工残留形态：抽屉内非 .json 条目 + 抽屉外条目。
    std::fs::write(done.join("stray.txt"), b"not a receipt").unwrap();
    std::fs::write(sink.inbox_root().join("outside.json"), b"not in drawer").unwrap();

    let removed = sink.sweep_stale_done_receipts(std::time::Duration::ZERO);
    assert_eq!(removed, 2, "两份回执都被清除");
    assert!(!done.join("tr-da.json").exists());
    assert!(!done.join("tr-db.json").exists());
    assert!(done.join("stray.txt").exists(), "抽屉内非 .json 条目不碰");
    assert!(
        sink.inbox_root().join("outside.json").exists(),
        "抽屉外条目不碰"
    );

    let _ = std::fs::remove_dir_all(&root);
}

// max_age 超大：新鲜回执一律保留（重放窗口内语义不受影响）。
#[test]
fn test_sweep_stale_done_receipts_large_age_keeps_everything() {
    let root = temp_root("done-keep");
    let sink = TransferSink::new(&root.join("ws"), 0);
    let src = root.join("src-dk");
    make_source(&src, &[("r.md", b"archive")]);
    let begin = prepare_begin(&src, "tr-dk", "task-dk", 1000);
    push_all(&sink, &begin, &src, &[]);
    let done = sink.inbox_root().join(".done");

    let removed = sink.sweep_stale_done_receipts(std::time::Duration::from_secs(100 * 24 * 3600));
    assert_eq!(removed, 0, "新鲜回执不被清扫");
    assert!(done.join("tr-dk.json").is_file(), "回执原样保留");

    let _ = std::fs::remove_dir_all(&root);
}

// 抽屉不存在（零传输）：诚实 0。
#[test]
fn test_sweep_stale_done_receipts_missing_drawer_is_zero() {
    let root = temp_root("done-missing");
    let sink = TransferSink::new(&root.join("ws"), 0);
    assert_eq!(sink.sweep_stale_done_receipts(std::time::Duration::ZERO), 0);
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
    let mk = |n: usize| {
        let files: Vec<_> = (0..n)
            .map(|i| TransferFileEntry {
                path: format!("f/{i}.txt"),
                size: 0,
                sha256: String::new(),
            })
            .collect();
        // content_hash 与清单一致（BUG-20261123-009B 校验）——夹具按实
        // 构造真指纹，被测维度（条数闸）外的载荷保持合法。
        let content_hash = content_hash(&files);
        TransferBegin {
            transfer_id: "tr-gate".into(),
            task_id: "t-gate".into(),
            kind: TRANSFER_KIND_EXECUTION_RECORDS.into(),
            source_node: "node-a".into(),
            total_bytes: 0,
            chunk_size: 64 * 1024,
            chunk_count: 0,
            content_hash,
            files,
        }
    };
    let over = mk(MAX_MANIFEST_FILES + 1);
    let err = validate_manifest(&over).unwrap_err();
    assert!(err.contains("条目"), "错误指明条数超闸: {err}");
    let ok = mk(MAX_MANIFEST_FILES);
    assert!(validate_manifest(&ok).is_ok(), "闸值内通过结构校验");
}

// 回归锁 BUG-20261113-129（与 nemesis-migrate workspace/tests.rs 同族形态）：
// 目录符号链接环必须诚实跳过（不递归）。旧实现 from.is_dir() 跟随链接，
// 链接环 = 病态嵌套复制直到 OS 路径上限（Windows ~260 / Linux PATH_MAX）
// 才 Err，dst 留深层垃圾树。链接创建免提权降级链：symlink_dir（开发者
// 模式/管理员）→ junction（mklink /J，任何用户）。
#[cfg(windows)]
fn make_dir_link(target: &std::path::Path, link: &std::path::Path) {
    use std::os::windows::fs::symlink_dir;
    if symlink_dir(target, link).is_ok() {
        return;
    }
    let out = std::process::Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .expect("spawn cmd mklink");
    assert!(
        out.status.success(),
        "创建目录链接失败: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[cfg(unix)]
fn make_dir_link(target: &std::path::Path, link: &std::path::Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[test]
fn test_copy_dir_symlink_cycle_skipped_not_recursed() {
    let root = temp_root("symlink-cycle");
    let src = root.join("src");
    let dst = root.join("dst");
    let mem = src.join("memory");
    std::fs::create_dir_all(&mem).unwrap();
    std::fs::write(mem.join("notes.txt"), "real content").unwrap();

    // 链接环：memory/loop → memory 自身（旧实现跟随 = 无限嵌套复制）。
    make_dir_link(&mem, &mem.join("loop"));

    copy_dir_recursive(&src, &dst).unwrap();
    // 链接条目被跳过：dst 无 loop 目录、无深层垃圾嵌套树。
    assert!(!dst.join("memory").join("loop").exists());
    assert!(!dst.join("memory").join("loop").join("loop").exists());
    // 常规文件照常拷贝。
    assert_eq!(
        std::fs::read_to_string(dst.join("memory").join("notes.txt")).unwrap(),
        "real content"
    );
    let _ = std::fs::remove_dir_all(&root);
}
