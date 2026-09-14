//! outbox 发件箱测试（P3/D2）。
//!
//! goal 单测面：入队落盘/幂等、推送全链（fake transport）、dedup 免传、
//! 断点续传（have 跳块）、中途失败保 pending 重试、D4 worker 侧护栏 +
//! 重武装、启动清扫（pushing 重置 + cluster_logs 残留补入队）、D5 pull。

use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

// ---------------------------------------------------------------------------
// Fake transport：脚本化 begin/have/失败点，记录全部调用
// ---------------------------------------------------------------------------

struct FakeTransport {
    calls: std::sync::Mutex<Vec<(String, String, serde_json::Value)>>,
    begin_status: std::sync::Mutex<String>,
    have: std::sync::Mutex<Vec<usize>>,
    fail_chunk_seq: AtomicUsize,
    end_ok: AtomicBool,
}

impl FakeTransport {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: std::sync::Mutex::new(Vec::new()),
            begin_status: std::sync::Mutex::new("ok".into()),
            have: std::sync::Mutex::new(Vec::new()),
            fail_chunk_seq: AtomicUsize::new(usize::MAX),
            end_ok: AtomicBool::new(true),
        })
    }

    fn calls_of(&self, action: &str) -> Vec<serde_json::Value> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, a, _)| a == action)
            .map(|(_, _, p)| p.clone())
            .collect()
    }
}

#[async_trait::async_trait]
impl TransferTransport for FakeTransport {
    async fn call(
        &self,
        peer: &str,
        action: &str,
        payload: serde_json::Value,
        _timeout: Duration,
    ) -> Result<serde_json::Value, String> {
        self.calls
            .lock()
            .unwrap()
            .push((peer.to_string(), action.to_string(), payload.clone()));
        match action {
            ACTION_TRANSFER_BEGIN => {
                let status = self.begin_status.lock().unwrap().clone();
                let have = self.have.lock().unwrap().clone();
                Ok(json!({ "status": status, "have": have }))
            }
            ACTION_TRANSFER_CHUNK => {
                let seq = payload.get("seq").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                if self.fail_chunk_seq.load(Ordering::SeqCst) == seq {
                    // 一次性失败（模拟「这次断了」，下轮重试不再失败）。
                    self.fail_chunk_seq.store(usize::MAX, Ordering::SeqCst);
                    return Err("模拟传输中断".into());
                }
                Ok(json!({ "status": "ok" }))
            }
            ACTION_TRANSFER_END => {
                if self.end_ok.load(Ordering::SeqCst) {
                    Ok(json!({ "status": "ok", "file_count": 1, "total_bytes": 1 }))
                } else {
                    Err("模拟 end 失败".into())
                }
            }
            ACTION_TRANSFER_OVERLIMIT => Ok(json!({ "status": "noted" })),
            _ => Ok(serde_json::Value::Null),
        }
    }
}

// ---------------------------------------------------------------------------
// 测试脚手架
// ---------------------------------------------------------------------------

/// 唯一临时 workspace（进程内序列 + pid，与其他套件不撞）。
fn temp_ws(name: &str) -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "nemesis-cluster-outbox-{}-{name}-{n}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// 在 workspace 的 cluster_logs 里伪造一份 worker 执行记录。
fn make_task_records(ws: &Path, device: &str, task_id: &str, content: &str) {
    let dir = nemesis_path::resolve_cluster_logs_dir_in_workspace(ws)
        .join(device)
        .join(format!("2026-09-14_10-30-00-123_{task_id}"));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("00.request.md"), content).unwrap();
    std::fs::write(dir.join("01.AI.Response.raw.json"), br#"{"x":1}"#).unwrap();
}

fn outbox_dir(ws: &Path) -> PathBuf {
    nemesis_path::cluster_dir_in_workspace(ws).join("outbox")
}

type LimitFn = Box<dyn Fn() -> u64 + Send + Sync>;

fn limit_from(arc: &Arc<AtomicU64>) -> LimitFn {
    let a = arc.clone();
    Box::new(move || a.load(Ordering::SeqCst))
}

fn make_outbox(ws: &Path, transport: Arc<FakeTransport>, limit: LimitFn) -> Arc<TransferOutbox> {
    Arc::new(TransferOutbox::new(ws, "node-w".into(), transport, limit))
}

// ---------------------------------------------------------------------------
// 入队
// ---------------------------------------------------------------------------

#[tokio::test]
async fn enqueue_copies_records_and_is_idempotent() {
    let ws = temp_ws("enqueue");
    make_task_records(&ws, "node-m", "task-1", "# 内容 hello");
    let ob = make_outbox(
        &ws,
        FakeTransport::new(),
        limit_from(&Arc::new(AtomicU64::new(0))),
    );

    ob.enqueue("task-1", "node-m").unwrap();
    // 幂等：重复入队 no-op。
    ob.enqueue("task-1", "node-m").unwrap();

    let dir = outbox_dir(&ws).join("task-1");
    let entry: OutboxEntry =
        serde_json::from_str(&std::fs::read_to_string(dir.join("entry.json")).unwrap()).unwrap();
    assert_eq!(entry.state, "pending");
    assert_eq!(entry.source_node, "node-m");
    assert_eq!(entry.total_bytes, 21); // "# 内容 hello"(14) + `{"x":1}`(7)
    // payload 复制在场。
    assert!(dir.join("payload").join("00.request.md").exists());
    assert!(dir.join("payload").join("01.AI.Response.raw.json").exists());
    // 源记录未被移动/删除。
    assert!(
        nemesis_path::resolve_cluster_logs_dir_in_workspace(&ws)
            .join("node-m")
            .join("2026-09-14_10-30-00-123_task-1")
            .join("00.request.md")
            .exists()
    );
    // 未知任务诚实失败。
    assert!(ob.enqueue("no-such-task", "node-m").is_err());
}

#[tokio::test]
async fn push_delivers_and_deletes_local() {
    let ws = temp_ws("push");
    make_task_records(&ws, "node-m", "task-1", "payload-body");
    let ft = FakeTransport::new();
    let ob = make_outbox(&ws, ft.clone(), limit_from(&Arc::new(AtomicU64::new(0))));
    ob.enqueue("task-1", "node-m").unwrap();

    ob.process_once().await;

    // 本地条目已删（master ACK 后双删）。
    assert!(!outbox_dir(&ws).join("task-1").exists());
    // 调用序列：begin 1 次（无 have 跳块）+ chunk 2 次（两文件各一块）+ end 1 次。
    assert_eq!(ft.calls_of(ACTION_TRANSFER_BEGIN).len(), 1);
    assert_eq!(ft.calls_of(ACTION_TRANSFER_CHUNK).len(), 2);
    assert_eq!(ft.calls_of(ACTION_TRANSFER_END).len(), 1);
    // begin 载荷字段 sanity。
    let begin = &ft.calls_of(ACTION_TRANSFER_BEGIN)[0];
    assert_eq!(begin["task_id"], "task-1");
    assert_eq!(begin["source_node"], "node-w");
    // 目标节点 = source_node。
    let calls = ft.calls.lock().unwrap();
    assert!(calls.iter().all(|(peer, _, _)| peer == "node-m"));
}

#[tokio::test]
async fn dedup_reply_deletes_local_without_chunks() {
    let ws = temp_ws("dedup");
    make_task_records(&ws, "node-m", "task-1", "same-content");
    let ft = FakeTransport::new();
    *ft.begin_status.lock().unwrap() = "dedup".into();
    let ob = make_outbox(&ws, ft.clone(), limit_from(&Arc::new(AtomicU64::new(0))));
    ob.enqueue("task-1", "node-m").unwrap();

    ob.process_once().await;

    assert!(!outbox_dir(&ws).join("task-1").exists(), "dedup 后删本地");
    assert_eq!(ft.calls_of(ACTION_TRANSFER_BEGIN).len(), 1);
    assert!(
        ft.calls_of(ACTION_TRANSFER_CHUNK).is_empty(),
        "dedup 必须免传块"
    );
    assert!(ft.calls_of(ACTION_TRANSFER_END).is_empty());
}

#[tokio::test]
async fn resume_skips_have_chunks_and_completes() {
    let ws = temp_ws("resume");
    make_task_records(&ws, "node-m", "task-1", "body");
    let ft = FakeTransport::new();
    *ft.have.lock().unwrap() = vec![0, 1]; // master 已收 2 块
    let ob = make_outbox(&ws, ft.clone(), limit_from(&Arc::new(AtomicU64::new(0))));
    ob.enqueue("task-1", "node-m").unwrap();

    ob.process_once().await;

    assert!(!outbox_dir(&ws).join("task-1").exists());
    let chunks = ft.calls_of(ACTION_TRANSFER_CHUNK);
    assert_eq!(chunks.len(), 0, "全部块都已被 master 收到 → 免传");
    assert_eq!(ft.calls_of(ACTION_TRANSFER_END).len(), 1);
}

#[tokio::test]
async fn mid_push_failure_keeps_pending_and_retries() {
    let ws = temp_ws("fail");
    make_task_records(&ws, "node-m", "task-1", "x".repeat(100).as_str());
    let ft = FakeTransport::new();
    ft.fail_chunk_seq.store(1, Ordering::SeqCst); // 第 1 块失败（一次）
    let ob = make_outbox(&ws, ft.clone(), limit_from(&Arc::new(AtomicU64::new(0))));
    ob.enqueue("task-1", "node-m").unwrap();

    ob.process_once().await;

    // 条目还在，状态回 pending，attempts+1。
    let dir = outbox_dir(&ws).join("task-1");
    assert!(dir.exists(), "失败必须保留条目重试");
    let entry: OutboxEntry =
        serde_json::from_str(&std::fs::read_to_string(dir.join("entry.json")).unwrap()).unwrap();
    assert_eq!(entry.state, "pending");
    assert_eq!(entry.attempts, 1);
    assert!(entry.last_error.as_deref().unwrap().contains("中断"));

    // 重试（不再失败）→ 完成。
    ob.process_once().await;
    assert!(!outbox_dir(&ws).join("task-1").exists());
    // 确定性 transfer_id：两轮 begin 的 transfer_id 一致（同内容同块大小）。
    let begins = ft.calls_of(ACTION_TRANSFER_BEGIN);
    assert_eq!(begins.len(), 2);
    assert_eq!(begins[0]["transfer_id"], begins[1]["transfer_id"]);
}

// ---------------------------------------------------------------------------
// D4 worker 侧护栏
// ---------------------------------------------------------------------------

#[tokio::test]
async fn overlimit_marks_entry_and_notifies_master_then_rearms() {
    let ws = temp_ws("overlimit");
    make_task_records(&ws, "node-m", "task-1", "0".repeat(1000).as_str());
    let ft = FakeTransport::new();
    let limit = Arc::new(AtomicU64::new(100)); // 护栏 100 < 载荷 1000
    let ob = make_outbox(&ws, ft.clone(), limit_from(&limit));
    ob.enqueue("task-1", "node-m").unwrap();

    ob.process_once().await;

    // 本地 over_limit 标记（不删不传）。
    let dir = outbox_dir(&ws).join("task-1");
    let entry: OutboxEntry =
        serde_json::from_str(&std::fs::read_to_string(dir.join("entry.json")).unwrap()).unwrap();
    assert_eq!(entry.state, "over_limit");
    assert!(
        ft.calls_of(ACTION_TRANSFER_BEGIN).is_empty(),
        "超限不得发 begin"
    );
    // 通知 master 出卡。
    let ols = ft.calls_of(ACTION_TRANSFER_OVERLIMIT);
    assert_eq!(ols.len(), 1);
    assert_eq!(ols[0]["task_id"], "task-1");
    assert_eq!(ols[0]["limit"], 100);
    assert_eq!(ols[0]["total_bytes"], 1007); // "0"*1000 + `{"x":1}`(7)

    // 护栏调大（热生效）→ 启动清扫重新武装 → 推送成功。
    limit.store(0, Ordering::SeqCst); // 0 = 不限
    ob.sweep_startup();
    let entry: OutboxEntry =
        serde_json::from_str(&std::fs::read_to_string(dir.join("entry.json")).unwrap()).unwrap();
    assert_eq!(entry.state, "pending", "护栏调大后必须重新武装");
    ob.process_once().await;
    assert!(!outbox_dir(&ws).join("task-1").exists());
}

// ---------------------------------------------------------------------------
// 启动清扫（崩溃兜底）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sweep_startup_rearms_pushing_and_backfills_cluster_logs() {
    let ws = temp_ws("sweep");
    // 残留 1：cluster_logs 有记录但无发件箱条目（终态钩子没来得及跑）。
    make_task_records(&ws, "node-m", "task-a", "aaa");
    // 残留 2：已有条目但卡在 pushing（推送中途崩溃）。
    make_task_records(&ws, "node-m", "task-b", "bbb");
    let ft = FakeTransport::new();
    let ob = make_outbox(&ws, ft.clone(), limit_from(&Arc::new(AtomicU64::new(0))));
    ob.enqueue("task-b", "node-m").unwrap();
    let dir_b = outbox_dir(&ws).join("task-b");
    let mut e: OutboxEntry =
        serde_json::from_str(&std::fs::read_to_string(dir_b.join("entry.json")).unwrap()).unwrap();
    e.state = "pushing".into();
    ob.sweep_startup(); // 内部会写回 pending——先手动破坏，再 sweep
    // sweep 已跑过一次（可能已推送 task-b）；重置后验证状态闭环。
    // 重新构造 pushing 态验证重置逻辑：
    let dir_b = outbox_dir(&ws).join("task-b");
    if dir_b.exists() {
        let mut e2: OutboxEntry =
            serde_json::from_str(&std::fs::read_to_string(dir_b.join("entry.json")).unwrap())
                .unwrap();
        e2.state = "pushing".into();
        std::fs::write(
            dir_b.join("entry.json"),
            serde_json::to_string(&e2).unwrap(),
        )
        .unwrap();
        let ob2 = make_outbox(&ws, ft.clone(), limit_from(&Arc::new(AtomicU64::new(0))));
        ob2.sweep_startup();
        let e3: OutboxEntry =
            serde_json::from_str(&std::fs::read_to_string(dir_b.join("entry.json")).unwrap())
                .unwrap();
        assert_eq!(e3.state, "pending", "pushing 必须被启动清扫重置");
    }

    // task-a 残留被补入队（可推送）。
    let dir_a = outbox_dir(&ws).join("task-a");
    assert!(
        dir_a.join("entry.json").exists(),
        "cluster_logs 残留必须被补入队"
    );
    let ea: OutboxEntry =
        serde_json::from_str(&std::fs::read_to_string(dir_a.join("entry.json")).unwrap()).unwrap();
    assert_eq!(ea.source_node, "node-m");
    assert_eq!(ea.state, "pending");

    // 清扫后一轮推送：全部送达清空。
    ob.process_once().await;
    assert!(!dir_a.exists());
}

#[tokio::test]
async fn sweep_skips_unknown_device_dir() {
    let ws = temp_ws("unknown");
    make_task_records(&ws, "_unknown", "task-x", "orphan");
    let ft = FakeTransport::new();
    let ob = make_outbox(&ws, ft.clone(), limit_from(&Arc::new(AtomicU64::new(0))));
    ob.sweep_startup();
    assert!(
        !outbox_dir(&ws).join("task-x").exists(),
        "无主记录（source 未知）不可寻址，不得入队（否则永久重试失败）"
    );
}

// ---------------------------------------------------------------------------
// D5 兜底拉取
// ---------------------------------------------------------------------------

#[test]
fn request_pull_queues_when_records_exist() {
    let ws = temp_ws("pull");
    make_task_records(&ws, "node-m", "task-9", "rescue-me");
    let ob = make_outbox(
        &ws,
        FakeTransport::new(),
        limit_from(&Arc::new(AtomicU64::new(0))),
    );
    assert_eq!(ob.request_pull("task-9"), "queued");
    assert!(
        outbox_dir(&ws).join("task-9").join("entry.json").exists(),
        "pull 命中必须入队"
    );
    // 重复 pull 幂等（已有条目 → 仍 queued）。
    assert_eq!(ob.request_pull("task-9"), "queued");
    // 未命中诚实 no_archive。
    assert_eq!(ob.request_pull("no-such"), "no_archive");
    assert_eq!(ob.request_pull(""), "no_archive");
}

// ---------------------------------------------------------------------------
// P4/E2：通用推送 push_transfer_dir（基线推送与 outbox 共用真相源）
// ---------------------------------------------------------------------------

/// 在目录里落两个文件（push_transfer_dir 输入）。
fn make_payload_dir(name: &str) -> PathBuf {
    let dir = temp_ws(name);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("00.md"), "record-body").unwrap();
    std::fs::write(dir.join("sub").join("blob.bin"), vec![1u8; 300]).unwrap();
    dir
}

#[tokio::test]
async fn push_transfer_dir_delivers_full_sequence() {
    let dir = make_payload_dir("ptd");
    let ft = FakeTransport::new();
    let out = push_transfer_dir(
        ft.as_ref(),
        "node-b",
        "task-p",
        crate::transfer::TRANSFER_KIND_PROJECT_BASELINE,
        "node-a",
        &dir,
        0,
    )
    .await
    .unwrap();
    assert_eq!(out, PushDirOutcome::Delivered);
    assert_eq!(ft.calls_of(ACTION_TRANSFER_BEGIN).len(), 1);
    assert_eq!(ft.calls_of(ACTION_TRANSFER_CHUNK).len(), 2);
    assert_eq!(ft.calls_of(ACTION_TRANSFER_END).len(), 1);
    let begin = &ft.calls_of(ACTION_TRANSFER_BEGIN)[0];
    assert_eq!(begin["task_id"], "task-p");
    assert_eq!(
        begin["kind"],
        crate::transfer::TRANSFER_KIND_PROJECT_BASELINE
    );
    assert_eq!(begin["source_node"], "node-a");
    let calls = ft.calls.lock().unwrap();
    assert!(calls.iter().all(|(peer, _, _)| peer == "node-b"));
}

#[tokio::test]
async fn push_transfer_dir_overlimit_local_guard_without_begin() {
    let dir = make_payload_dir("ptd-ol");
    let ft = FakeTransport::new();
    let out = push_transfer_dir(ft.as_ref(), "node-b", "task-p", "kind", "node-a", &dir, 10)
        .await
        .unwrap();
    match out {
        PushDirOutcome::Overlimit { total_bytes, limit } => {
            assert!(total_bytes > 10);
            assert_eq!(limit, 10);
        }
        other => panic!("必须本地护栏拦截，实得 {other:?}"),
    }
    assert!(
        ft.calls_of(ACTION_TRANSFER_BEGIN).is_empty(),
        "超限不得发 begin"
    );
}

#[tokio::test]
async fn push_transfer_dir_nothing_to_send_and_dedup() {
    let empty = temp_ws("ptd-empty");
    let ft = FakeTransport::new();
    let out = push_transfer_dir(ft.as_ref(), "b", "t", "kind", "a", &empty, 0)
        .await
        .unwrap();
    assert_eq!(out, PushDirOutcome::NothingToSend);

    let dir = make_payload_dir("ptd-dedup");
    *ft.begin_status.lock().unwrap() = "dedup".into();
    let out = push_transfer_dir(ft.as_ref(), "b", "t", "kind", "a", &dir, 0)
        .await
        .unwrap();
    assert_eq!(out, PushDirOutcome::Delivered, "dedup = 已有同档，视为送达");
    assert!(
        ft.calls_of(ACTION_TRANSFER_CHUNK).is_empty(),
        "dedup 免传块"
    );
    assert!(ft.calls_of(ACTION_TRANSFER_END).is_empty());
}

// ---------------------------------------------------------------------------
// P4/E3：变更集搭车入队
// ---------------------------------------------------------------------------

/// 组装一份合法变更集目录（write_changeset 正路）。
fn make_changeset_dir(name: &str, base_commit: &str) -> PathBuf {
    let dir = temp_ws(name);
    let content = b"worker edited common header\n".to_vec();
    let manifest = changeset::ChangesetManifest {
        version: changeset::CHANGESET_VERSION,
        base_commit: base_commit.into(),
        upserts: vec![changeset::ChangesetUpsert {
            path: "src/common.h".into(),
            sha256: crate::transfer::sha256_hex(&content),
            size: content.len() as u64,
            executable: false,
        }],
        deletions: vec![],
    };
    let contents = vec![changeset::ChangesetContent {
        path: "src/common.h".into(),
        content,
        executable: false,
    }];
    changeset::write_changeset(&dir, &manifest, &contents).unwrap();
    dir
}

#[tokio::test]
async fn enqueue_with_changeset_rides_payload_and_pushes() {
    let ws = temp_ws("cs-enqueue");
    make_task_records(&ws, "node-m", "task-1", "# 记录");
    let cs = make_changeset_dir("cs-src", "deadbeef");
    let ft = FakeTransport::new();
    let ob = make_outbox(&ws, ft.clone(), limit_from(&Arc::new(AtomicU64::new(0))));

    ob.enqueue_with_changeset("task-1", "node-m", Some(&cs))
        .unwrap();

    // 载荷内变更集在位，且 read_changeset 能从 payload 根读回（同形布局）。
    let payload = outbox_dir(&ws).join("task-1").join("payload");
    let (manifest, contents) = changeset::read_changeset(&payload).unwrap().unwrap();
    assert_eq!(manifest.base_commit, "deadbeef");
    assert_eq!(contents[0].path, "src/common.h");

    ob.process_once().await;

    // 同一次传输：begin manifest 含变更集文件（原子落地，无第二趟传输）。
    assert_eq!(ft.calls_of(ACTION_TRANSFER_BEGIN).len(), 1);
    let begin = &ft.calls_of(ACTION_TRANSFER_BEGIN)[0];
    let files = begin["files"].as_array().unwrap();
    assert!(
        files
            .iter()
            .any(|f| f["path"] == "changeset/changeset.json"),
        "变更集清单必须随载荷上传: {files:?}"
    );
    assert!(
        files
            .iter()
            .any(|f| f["path"] == "changeset/files/src/common.h"),
        "变更集文件必须随载荷上传: {files:?}"
    );
    assert!(!outbox_dir(&ws).join("task-1").exists(), "送达后双删");
}

#[tokio::test]
async fn enqueue_with_changeset_missing_manifest_is_honest_error() {
    let ws = temp_ws("cs-missing");
    make_task_records(&ws, "node-m", "task-1", "# 记录");
    let cs = temp_ws("cs-empty"); // 目录在、清单不在
    let ft = FakeTransport::new();
    let ob = make_outbox(&ws, ft.clone(), limit_from(&Arc::new(AtomicU64::new(0))));

    let err = ob
        .enqueue_with_changeset("task-1", "node-m", Some(&cs))
        .unwrap_err();
    assert!(err.contains("变更集清单缺失"), "{err}");
    // 半套变更集不得造成条目落盘（防推送半套数据）。
    assert!(!outbox_dir(&ws).join("task-1").join("entry.json").exists());
    assert!(ft.calls_of(ACTION_TRANSFER_BEGIN).is_empty());
}

#[test]
fn task_dir_name_derivation_matches_logger_format() {
    // cluster_request_logger_observer 实际格式：%Y-%m-%d_%H-%M-%S-%3f_{task}
    let name = "2026-09-14_10-30-00-123_ab12cd34";
    assert_eq!(derive_task_id_from_dir(name).as_deref(), Some("ab12cd34"));
    assert!(task_dir_matches(name, "ab12cd34"));
    assert!(!task_dir_matches(name, "other"));
    // 非 ts 前缀形态 → None（宁缺毋错键）。
    assert!(derive_task_id_from_dir("short").is_none());
    assert!(derive_task_id_from_dir("2026-09-14_10-30-00-123").is_none());
}
