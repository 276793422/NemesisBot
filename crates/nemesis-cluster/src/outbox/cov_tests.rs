// outbox.rs 覆盖率补充测试（死信列举/重放失败面 / transport+kick 面板 /
// request_pull 全失败形态 / sweep_startup 非目录·坏名·补入队失败 /
// process_once 死信 age 闸 + 空载荷即删 / list_entries 坏形态 /
// find_task_dir 取最新 / push_transfer_dir begin 三态 + 部分续传 + end 未确认）。
//
// 豁免：register_transfer_handlers（925-978，需运行中的 Cluster RPC
// server）；RpcTransferTransport（127-152，需真实 RpcClient 拨号）；
// 834（collect 与 open 之间文件消失的竞态面）；867（chunk_delay env 钩子，
// edition 2024 下 set_var 是 unsafe 且与并行套件竞态）；583-585（write
// 失败需「列表可见 + 写必败」同真，磁盘上互斥）；宏内部行（397/399/619/
// 627 等——行为已由对应分支测试覆盖，行归属为宏展开伪行）。

use super::*;
use std::path::PathBuf;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicUsize, Ordering};

// ---------------------------------------------------------------------------
// 本文件专用 transport（脚本化 begin 状态 / have / end 状态 / 调用记录）
// ---------------------------------------------------------------------------

struct ScriptTransport {
    begin_status: StdMutex<String>,
    have: StdMutex<Vec<usize>>,
    end_status: StdMutex<String>,
    chunk_fail_at: AtomicUsize, // 第 N 次 chunk 调用失败（usize::MAX = 不失败）
    chunk_calls: AtomicUsize,
}

impl ScriptTransport {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            begin_status: StdMutex::new("ok".into()),
            have: StdMutex::new(Vec::new()),
            end_status: StdMutex::new("ok".into()),
            chunk_fail_at: AtomicUsize::new(usize::MAX),
            chunk_calls: AtomicUsize::new(0),
        })
    }
}

#[async_trait::async_trait]
impl TransferTransport for ScriptTransport {
    async fn call(
        &self,
        _peer: &str,
        action: &str,
        _payload: serde_json::Value,
        _timeout: Duration,
    ) -> Result<serde_json::Value, String> {
        match action {
            ACTION_TRANSFER_BEGIN => Ok(json!({
                "status": self.begin_status.lock().unwrap().clone(),
                "have": self.have.lock().unwrap().clone(),
            })),
            ACTION_TRANSFER_CHUNK => {
                let n = self.chunk_calls.fetch_add(1, Ordering::SeqCst);
                if self.chunk_fail_at.load(Ordering::SeqCst) == n {
                    return Err("chunk 爆点".into());
                }
                Ok(json!({ "status": "ok" }))
            }
            ACTION_TRANSFER_END => {
                let status = self.end_status.lock().unwrap().clone();
                if status == "ok" {
                    Ok(json!({ "status": "ok" }))
                } else {
                    Ok(json!({ "status": status, "error": "end 未确认（cov）" }))
                }
            }
            _ => Ok(json!({ "status": "noted" })),
        }
    }
}

// ---------------------------------------------------------------------------
// 脚手架
// ---------------------------------------------------------------------------

fn temp_ws(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "nemesis-cluster-obcov-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 伪造 worker 执行记录（目录名带 24 字符 ts 前缀 + `_`）。
fn make_task_records(ws: &Path, device: &str, task_id: &str, content: &str) -> PathBuf {
    let dir = nemesis_path::resolve_cluster_logs_dir_in_workspace(ws)
        .join(device)
        .join(format!("2026-09-14_10-30-00-123_{task_id}"));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("00.request.md"), content).unwrap();
    dir
}

fn make_outbox(ws: &Path, transport: Arc<ScriptTransport>) -> Arc<TransferOutbox> {
    Arc::new(TransferOutbox::new(
        ws,
        "node-w".into(),
        transport,
        Box::new(|| 0), // 0 = 不限
    ))
}

fn dead_letter_entry(task_id: &str, created_at: &str) -> OutboxEntry {
    OutboxEntry {
        task_id: task_id.into(),
        source_node: "node-m".into(),
        state: "dead".into(),
        created_at: created_at.into(),
        attempts: 3,
        total_bytes: 1,
        last_error: Some("expired".into()),
        next_retry_at: None,
    }
}

// ---------------------------------------------------------------------------
// 死信面 + 面板
// ---------------------------------------------------------------------------

/// list_dead_letter_entries：根缺失 → 空；无 entry.json 的目录跳过；
/// 非 dead 跳过；按 created_at 升序。
#[test]
fn list_dead_letter_faces() {
    let ws = temp_ws("dead-list");
    let root = nemesis_path::cluster_dir_in_workspace(&ws).join("outbox");

    // 根不存在 → 空。
    assert!(list_dead_letter_entries(&root).is_empty());

    // 根在，但一个目录无 entry.json、一个是 pending、两个 dead。
    std::fs::create_dir_all(root.join("t-noentry")).unwrap();
    for (tid, state, created) in [
        ("t-b", "dead", "2026-09-02T10:00:00+08:00"),
        ("t-a", "dead", "2026-09-01T10:00:00+08:00"),
        ("t-live", "pending", "2026-09-03T10:00:00+08:00"),
    ] {
        let dir = root.join(tid);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("entry.json"),
            serde_json::to_string_pretty(&dead_letter_entry(tid, created)).unwrap(),
        )
        .unwrap();
        // 修正 state（helper 恒 dead；pending 那条单独改写）。
        if state != "dead" {
            let mut e = dead_letter_entry(tid, created);
            e.state = state.into();
            std::fs::write(dir.join("entry.json"), serde_json::to_string(&e).unwrap()).unwrap();
        }
    }

    let dead = list_dead_letter_entries(&root);
    assert_eq!(dead.len(), 2, "只列 dead：{dead:?}");
    assert_eq!(dead[0].task_id, "t-a", "升序");
    assert_eq!(dead[1].task_id, "t-b");
}

/// 面板：transport() 返回同一通路；kick() 可空踢（不 panic）。
#[test]
fn transport_getter_and_kick_panel() {
    let ws = temp_ws("panel");
    let ob = make_outbox(&ws, ScriptTransport::new());
    let t: &Arc<dyn TransferTransport> = ob.transport();
    assert!(Arc::ptr_eq(t, ob.transport()));
    ob.kick();
}

/// enqueue 空参拒绝（双参全空形态）。
#[test]
fn enqueue_rejects_empty_task_and_source() {
    let ws = temp_ws("enqueue-empty");
    let ob = make_outbox(&ws, ScriptTransport::new());
    let err = ob.enqueue("", "").unwrap_err();
    assert!(err.contains("为空"), "{err}");
    let err = ob.enqueue_with_changeset("t", "", None).unwrap_err();
    assert!(err.contains("为空"), "{err}");
}

// ---------------------------------------------------------------------------
// request_pull / sweep_startup 失败形态
// ---------------------------------------------------------------------------

/// request_pull：logs 根缺失 → no_archive（read_dir 失败臂）；
/// 设备位是文件 → 跳过；enqueue 因 sanitize 错位失败 → no_archive。
#[test]
fn request_pull_faces() {
    let ws = temp_ws("pull");
    let ob = make_outbox(&ws, ScriptTransport::new());

    // ① logs 根不存在 → read_dir 失败 → no_archive。
    assert_eq!(ob.request_pull("t-ghost"), "no_archive");

    // ② 设备位是文件（非目录）→ continue。
    let logs = nemesis_path::resolve_cluster_logs_dir_in_workspace(&ws);
    std::fs::create_dir_all(&logs).unwrap();
    std::fs::write(logs.join("not-a-dev"), "x").unwrap();
    assert_eq!(ob.request_pull("t-ghost"), "no_archive");

    // ③ 匹配到超长设备名目录（>80 字符，sanitize 截断），含合法 ts 前缀
    //    任务目录；enqueue 用截断后的设备名找 → 找不到 → Err →
    //    "no_archive"（436 臂）。
    let long_dev = format!("d{}", "a".repeat(85)); // 86 字符 > sanitize 80 上限
    make_task_records(&ws, &long_dev, "t-capped", "payload");
    assert_eq!(ob.request_pull("t-capped"), "no_archive");
}

/// request_pull 正常路径（直列目录名匹配 → queued）。
#[test]
fn request_pull_happy_queues() {
    let ws = temp_ws("pull-happy");
    let ob = make_outbox(&ws, ScriptTransport::new());
    make_task_records(&ws, "dev-ok", "t-pullme", "payload");
    assert_eq!(ob.request_pull("t-pullme"), "queued");
    // 幂等：条目已在，再拉仍诚实（enqueue 幂等 Ok → queued）。
    assert_eq!(ob.request_pull("t-pullme"), "queued");
}

/// sweep_startup：设备位文件跳过；任务位文件跳过；目录名剥不出 task_id
/// 跳过；补入队失败（colon 设备）走 warn 臂；pushing 残条重置 pending。
#[test]
fn sweep_startup_faces() {
    let ws = temp_ws("sweep");
    let transport = ScriptTransport::new();
    let ob = make_outbox(&ws, transport.clone());

    let logs = nemesis_path::resolve_cluster_logs_dir_in_workspace(&ws);
    std::fs::create_dir_all(&logs).unwrap();
    // 设备位文件（479 臂）。
    std::fs::write(logs.join("dev-file"), "x").unwrap();
    // 合法设备目录：任务位文件（492 臂）+ 剥不出前缀的目录（496 臂）。
    let dev = logs.join("dev-ok");
    std::fs::create_dir_all(&dev).unwrap();
    std::fs::write(dev.join("junk-file"), "x").unwrap();
    std::fs::create_dir_all(dev.join("short-not-ts")).unwrap();
    // 超长设备名（>80 截断）：合法任务目录 → 补入队必败（503 warn 臂）。
    let long_dev = format!("d{}", "b".repeat(85));
    make_task_records(&ws, &long_dev, "t-sweep-capped", "payload");

    // pushing 残条（重启中断）→ 应被重置回 pending。
    let root = ob.outbox_root();
    let interrupted = root.join("t-interrupted");
    std::fs::create_dir_all(&interrupted).unwrap();
    let mut e = dead_letter_entry("t-interrupted", "2026-09-20T00:00:00+08:00");
    e.state = "pushing".into();
    std::fs::write(
        interrupted.join("entry.json"),
        serde_json::to_string(&e).unwrap(),
    )
    .unwrap();

    ob.sweep_startup();

    let after = std::fs::read_to_string(interrupted.join("entry.json")).unwrap();
    let entry: OutboxEntry = serde_json::from_str(&after).unwrap();
    assert_eq!(entry.state, "pending", "pushing 残条重置");
    assert!(entry.last_error.as_deref().unwrap_or("").contains("重启"));
}

// ---------------------------------------------------------------------------
// process_once：死信闸 + 空载荷即删
// ---------------------------------------------------------------------------

/// created_at 8 天前的 pending 条目 → process_once 判死信（age 闸臂）。
#[tokio::test]
async fn process_once_dead_letters_expired_entry() {
    let ws = temp_ws("dead-age");
    let ob = make_outbox(&ws, ScriptTransport::new());
    let root = ob.outbox_root();
    let dir = root.join("t-old");
    std::fs::create_dir_all(dir.join("payload")).unwrap();
    std::fs::write(dir.join("payload").join("f.txt"), "x").unwrap();
    let old = (chrono::Local::now() - chrono::Duration::days(8)).to_rfc3339();
    let mut e = dead_letter_entry("t-old", &old);
    e.state = "pending".into();
    std::fs::write(dir.join("entry.json"), serde_json::to_string(&e).unwrap()).unwrap();

    ob.process_once().await;

    let raw = std::fs::read_to_string(dir.join("entry.json")).unwrap();
    let after: OutboxEntry = serde_json::from_str(&raw).unwrap();
    assert_eq!(after.state, "dead", "8 天旧条目必须归死信");
    assert!(after.next_retry_at.is_none());
}

/// 入队后载荷被掏空 → NothingToSend → 本地条目整目录删除（双删语义）。
#[tokio::test]
async fn process_once_nothing_to_send_deletes_local() {
    let ws = temp_ws("empty-payload");
    let ob = make_outbox(&ws, ScriptTransport::new());
    make_task_records(&ws, "dev-ok", "t-empty", "payload");
    ob.enqueue("t-empty", "dev-ok").unwrap();
    let entry_dir = ob.outbox_root().join("t-empty");
    assert!(entry_dir.exists());

    // 掏空载荷 → push_transfer_dir 无档可传。
    std::fs::remove_dir_all(entry_dir.join("payload")).unwrap();
    ob.process_once().await;

    assert!(!entry_dir.exists(), "NothingToSend 也要删本地条目");
}

// ---------------------------------------------------------------------------
// list_entries / find_task_dir 磁盘形态
// ---------------------------------------------------------------------------

/// list_entries：根变文件 → 空；非目录子项跳过；坏 entry.json 跳过。
#[tokio::test]
async fn list_entries_faces() {
    let ws = temp_ws("list-faces");
    let transport = ScriptTransport::new();
    let ob = make_outbox(&ws, transport.clone());
    let root = ob.outbox_root().to_path_buf();

    // 根变文件 → read_dir 失败 → 空（构造后替换形态）。
    std::fs::remove_dir_all(&root).unwrap();
    std::fs::write(&root, "not a dir").unwrap();
    assert!(ob.list_entries().is_empty());

    // 恢复目录：塞文件位 + 坏 entry.json 目录 + 好条目。
    std::fs::remove_file(&root).unwrap();
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("loose-file"), "x").unwrap();
    let bad = root.join("t-bad");
    std::fs::create_dir_all(&bad).unwrap();
    std::fs::write(bad.join("entry.json"), "{broken").unwrap();
    let good = root.join("t-good");
    std::fs::create_dir_all(&good).unwrap();
    std::fs::write(
        good.join("entry.json"),
        serde_json::to_string(&dead_letter_entry("t-good", "2026-09-20T00:00:00+08:00")).unwrap(),
    )
    .unwrap();

    let entries = ob.list_entries();
    assert_eq!(entries.len(), 1, "坏形态全跳过：{entries:?}");
    assert_eq!(entries[0].0.task_id, "t-good");
}

/// find_task_dir：多轮执行取字典序最新；非目录子项跳过；无匹配 → None。
#[tokio::test]
async fn find_task_dir_takes_latest_and_skips_files() {
    let ws = temp_ws("find-latest");
    let transport = ScriptTransport::new();
    let ob = make_outbox(&ws, transport.clone());
    let logs = nemesis_path::resolve_cluster_logs_dir_in_workspace(&ws);
    let dev = logs.join("dev-ok");
    std::fs::create_dir_all(&dev).unwrap();

    // 两轮执行：ts 前缀不同（23 定长 + `_`），取字典序大者。
    let old_dir = dev.join("2026-09-14_10-30-00-123_t-multi");
    let new_dir = dev.join("2026-09-14_11-45-00-999_t-multi");
    std::fs::create_dir_all(&old_dir).unwrap();
    std::fs::create_dir_all(&new_dir).unwrap();
    std::fs::write(
        dev.join("2026-09-14_11-45-00-999_t-multi.txt"),
        "file-not-dir",
    )
    .unwrap();

    let found = {
        // 走 enqueue 全链验证所选目录（enqueue 后 payload 含新目录标记文件）。
        std::fs::write(new_dir.join("marker-new.txt"), "newest").unwrap();
        std::fs::write(old_dir.join("marker-old.txt"), "oldest").unwrap();
        ob.enqueue("t-multi", "dev-ok").unwrap();
        let payload = ob.outbox_root().join("t-multi").join("payload");
        std::fs::read_to_string(payload.join("marker-new.txt")).ok()
    };
    assert_eq!(found.as_deref(), Some("newest"), "必须取最新一轮");

    // 无匹配 → enqueue 报执行记录缺失。
    let err = ob.enqueue("t-never", "dev-ok").unwrap_err();
    assert!(err.contains("无该任务执行记录"), "{err}");
}

// ---------------------------------------------------------------------------
// push_transfer_dir 三态 + 部分续传 + end 未确认
// ---------------------------------------------------------------------------

async fn seed_payload(ws: &Path) -> PathBuf {
    let dir = ws.join("payload-src");
    std::fs::create_dir_all(&dir).unwrap();
    // 两个文件、各多块（chunk_bytes=1MB；a=2 块、b=2 块）。
    std::fs::write(dir.join("a.txt"), vec![b'a'; 1_200_000]).unwrap();
    std::fs::write(dir.join("b.txt"), vec![b'b'; 1_100_000]).unwrap();
    dir
}

/// begin = dedup → Delivered（零块零 end）；begin = over_limit → Overlimit
/// 且 limit 置 0；begin = 未知状态 → Err。
#[tokio::test]
async fn push_transfer_dir_begin_statuses() {
    let ws = temp_ws("begin-faces");
    let dir = seed_payload(&ws).await;

    let t = ScriptTransport::new();
    *t.begin_status.lock().unwrap() = "dedup".into();
    let out = push_transfer_dir(t.as_ref(), "node-m", "t1", "kind", "node-w", &dir, 0)
        .await
        .unwrap();
    assert_eq!(out, PushDirOutcome::Delivered);
    assert_eq!(t.chunk_calls.load(Ordering::SeqCst), 0, "dedup 零块");

    *t.begin_status.lock().unwrap() = "over_limit".into();
    let out = push_transfer_dir(t.as_ref(), "node-m", "t1", "kind", "node-w", &dir, 0)
        .await
        .unwrap();
    assert_eq!(
        out,
        PushDirOutcome::Overlimit {
            total_bytes: 2_300_000,
            limit: 0
        }
    );

    *t.begin_status.lock().unwrap() = "martian".into();
    let err = push_transfer_dir(t.as_ref(), "node-m", "t1", "kind", "node-w", &dir, 0)
        .await
        .unwrap_err();
    assert!(err.contains("begin 异常状态 martian"), "{err}");
}

/// 部分续传：have 覆盖某文件部分块 → 文件内逐块循环里跳过已有块。
#[tokio::test]
async fn push_transfer_dir_partial_resume_skips_have_chunks() {
    let ws = temp_ws("partial-resume");
    let dir = seed_payload(&ws).await;
    let chunk = chunk_bytes();

    let t = ScriptTransport::new();
    // 假定 a.txt 是 file_idx 0（2 块）：have 覆盖其第 1 块 → 文件不整跳、
    // 块内循环跳过该块。
    assert!(chunk < 1_200_000, "测试假设 a.txt 多块：chunk={chunk}");
    *t.have.lock().unwrap() = vec![0];
    let out = push_transfer_dir(t.as_ref(), "node-m", "t1", "kind", "node-w", &dir, 0)
        .await
        .unwrap();
    assert_eq!(out, PushDirOutcome::Delivered);
    let sent = t.chunk_calls.load(Ordering::SeqCst);
    let total_chunks = chunk_count(&crate::transfer::collect_dir_files(&dir).unwrap(), chunk);
    assert!(
        sent < total_chunks,
        "have 覆盖必须少发：sent={sent} total={total_chunks}"
    );
}

/// end 回复非 ok → 「end 未确认」Err（收完所有块后诚实失败）。
#[tokio::test]
async fn push_transfer_dir_end_unconfirmed() {
    let ws = temp_ws("end-fail");
    let dir = seed_payload(&ws).await;
    let t = ScriptTransport::new();
    *t.end_status.lock().unwrap() = "assembling".into();
    let err = push_transfer_dir(t.as_ref(), "node-m", "t1", "kind", "node-w", &dir, 0)
        .await
        .unwrap_err();
    assert!(err.contains("end 未确认"), "{err}");
}

/// chunk 中途失败 → Err（上抛给 outbox 记账重试）。
#[tokio::test]
async fn push_transfer_dir_chunk_failure_propagates() {
    let ws = temp_ws("chunk-fail");
    let dir = seed_payload(&ws).await;
    let t = ScriptTransport::new();
    t.chunk_fail_at.store(1, Ordering::SeqCst); // 第 2 次 chunk 调用爆
    let err = push_transfer_dir(t.as_ref(), "node-m", "t1", "kind", "node-w", &dir, 0)
        .await
        .unwrap_err();
    assert!(err.contains("chunk 爆点"), "{err}");
}

/// 本地护栏：超 max_bytes → Overlimit（对端零接触）。
#[tokio::test]
async fn push_transfer_dir_local_overlimit_no_rpc() {
    let ws = temp_ws("local-limit");
    let dir = seed_payload(&ws).await;
    let t = ScriptTransport::new();
    let out = push_transfer_dir(t.as_ref(), "node-m", "t1", "kind", "node-w", &dir, 100)
        .await
        .unwrap();
    assert_eq!(
        out,
        PushDirOutcome::Overlimit {
            total_bytes: 2_300_000,
            limit: 100
        }
    );
    assert_eq!(t.chunk_calls.load(Ordering::SeqCst), 0, "护栏拦下零 RPC");
}
