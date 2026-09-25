// loop_continuation.rs 覆盖率补充测试（图片重水合三 helpers / no-vision
// 投影 / stale_task_ids / 磁盘快照剥字节 / 内存 map 同步入口 /
// wait_for_continuation 就绪双检）。
//
// 声明挂 loop_continuation.rs 的 cfg(test) 块（同 tests.rs / s9_tests.rs）。

use super::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::time::Duration;

/// 最小 PNG magic（hydrate 嗅探只认头部字节）。
fn png_bytes() -> Vec<u8> {
    let mut v = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    v.extend_from_slice(&[0, 0, 0, 13, b'I', b'H', b'D', b'R']);
    v
}

fn user_msg(content: &str) -> LlmMessage {
    LlmMessage {
        role: "user".to_string(),
        content: content.to_string(),
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
        images: Vec::new(),
    }
}

fn tool_msg(id: &str, content: &str) -> LlmMessage {
    LlmMessage {
        role: "tool".to_string(),
        content: content.to_string(),
        tool_calls: None,
        tool_call_id: Some(id.to_string()),
        reasoning_content: None,
        images: Vec::new(),
    }
}

fn cont_data(ready: bool) -> Arc<ContinuationData> {
    Arc::new(ContinuationData {
        messages: Vec::new(),
        tool_call_id: "tc".to_string(),
        channel: "web".to_string(),
        chat_id: "c".to_string(),
        session_key: String::new(),
        peer_id: String::new(),
        image_refs: Vec::new(),
        image_refs_by_user_turn: Vec::new(),
        ready: Arc::new(tokio::sync::Notify::new()),
        ready_flag: Arc::new(AtomicBool::new(ready)),
    })
}

// ---------------------------------------------------------------------------
// rehydrate_last_user_images
// ---------------------------------------------------------------------------

/// 无 user 消息 → 早退（不 panic、不改消息）。
#[test]
fn rehydrate_last_user_images_without_user_message_is_noop() {
    let mut msgs = vec![LlmMessage {
        role: "assistant".to_string(),
        content: "hi".to_string(),
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
        images: Vec::new(),
    }];
    rehydrate_last_user_images(&mut msgs, &["/x.png".to_string()]);
    assert!(msgs[0].images.is_empty());
    assert_eq!(msgs[0].content, "hi");
}

/// 有图可读 → last user 轮 images 水合；重复调用占位不堆叠。
#[test]
fn rehydrate_last_user_images_hydrates_and_dedups_placeholder() {
    let dir = tempfile::tempdir().unwrap();
    let img = dir.path().join("ok.png");
    std::fs::write(&img, png_bytes()).unwrap();
    let refs = vec![img.to_string_lossy().into_owned()];

    let mut msgs = vec![user_msg("看图"), tool_msg("t1", "out"), user_msg("再来")];
    rehydrate_last_user_images(&mut msgs, &refs);
    assert_eq!(msgs[2].images.len(), 1);
    assert_eq!(msgs[2].images[0].path, img.to_string_lossy());
    assert_eq!(msgs[0].images.len(), 0, "only LAST user turn is hydrated");

    // 再次重水合：占位行不重复追加（去重）。
    let content_before = msgs[2].content.clone();
    rehydrate_last_user_images(&mut msgs, &refs);
    assert_eq!(
        msgs[2].content, content_before,
        "placeholder must not stack"
    );
}

/// 文件缺失 → 占位文本进 content（诚实缺省），images 空。
#[test]
fn rehydrate_last_user_images_missing_file_gets_placeholder() {
    let mut msgs = vec![user_msg("看图")];
    rehydrate_last_user_images(&mut msgs, &["Z:/no/such.png".to_string()]);
    assert!(msgs[0].images.is_empty());
    assert!(msgs[0].content.contains("图片已失效") || msgs[0].content.len() > "看图".len());
}

// ---------------------------------------------------------------------------
// derive_image_refs_by_user_turn / rehydrate_images_by_user_turn
// ---------------------------------------------------------------------------

#[test]
fn derive_refs_by_user_turn_and_rehydrate_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let img = dir.path().join("turn.png");
    std::fs::write(&img, png_bytes()).unwrap();
    let img_ref = img.to_string_lossy().into_owned();

    // 两 user 轮（第一轮带图）+ 中间 tool 轮。
    let mut hydrated = vec![
        user_msg("第一轮"),
        tool_msg("t1", "out"),
        user_msg("第二轮"),
    ];
    rehydrate_last_user_images(&mut hydrated, std::slice::from_ref(&img_ref));
    let refs_by_turn = derive_image_refs_by_user_turn(&hydrated);
    assert_eq!(refs_by_turn.len(), 2, "per-user-turn projection");
    assert_eq!(refs_by_turn[0].len(), 0);
    assert_eq!(refs_by_turn[1], vec![img_ref.clone()]);

    // 按轮重水合（磁盘恢复路径）：全量恢复。
    let mut restored = vec![
        user_msg("第一轮"),
        tool_msg("t1", "out"),
        user_msg("第二轮"),
    ];
    rehydrate_images_by_user_turn(&mut restored, &refs_by_turn);
    assert!(restored[0].images.is_empty());
    assert_eq!(restored[2].images.len(), 1);

    // 快照条目少于 user 轮数（手改/损坏）→ 多出的轮诚实缺省；
    // 空条目轮跳过（continue）。
    let mut restored = vec![user_msg("第一轮"), user_msg("第二轮")];
    let empty_then_missing: Vec<Vec<String>> = vec![Vec::new()];
    rehydrate_images_by_user_turn(&mut restored, &empty_then_missing);
    assert!(restored[0].images.is_empty(), "empty refs turn skipped");
    assert!(
        restored[1].images.is_empty(),
        "missing tail turn stays empty"
    );

    // 空 refs_by_turn → 整体早退。
    let mut untouched = vec![user_msg("第一轮")];
    rehydrate_images_by_user_turn(&mut untouched, &[]);
    assert!(untouched[0].images.is_empty());
}

// ---------------------------------------------------------------------------
// project_messages_for_no_vision
// ---------------------------------------------------------------------------

#[test]
fn project_no_vision_notes_last_user_differently() {
    let mut msgs = vec![user_msg("第一轮"), user_msg("")];
    // 给两条 user 消息都挂一张（已水合）图。
    let fake = crate::image_attach::LlmImage {
        path: "x.png".to_string(),
        media_type: "image/png".to_string(),
        data: "AAAA".to_string(),
    };
    msgs[0].images.push(fake.clone());
    msgs[1].images.push(fake);

    project_messages_for_no_vision(&mut msgs);

    // 非最后 user 轮 → 「已省略」注记；最后 user 轮 → 「vision=no」注记。
    assert!(msgs[0].images.is_empty() && msgs[1].images.is_empty());
    assert!(
        msgs[0].content.contains("图片已省略"),
        "got: {}",
        msgs[0].content
    );
    assert!(
        msgs[1].content.contains("当前模型 vision=no"),
        "got: {}",
        msgs[1].content
    );
    // 空内容不加前导换行。
    assert!(msgs[1].content.starts_with("[图片未发送"));

    // 幂等：重复投影不堆叠注记。
    let before = msgs[1].content.clone();
    project_messages_for_no_vision(&mut msgs);
    assert_eq!(msgs[1].content, before);
}

// ---------------------------------------------------------------------------
// ContinuationStore::stale_task_ids
// ---------------------------------------------------------------------------

#[test]
fn stale_task_ids_tables() {
    // base_dir 不存在 → 读目录失败 → 空（不 panic）。
    let ghost = std::env::temp_dir().join(format!("cov-no-dir-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ghost);
    let store = ContinuationStore::new(&ghost);
    assert!(store.stale_task_ids(Duration::from_secs(3600)).is_empty());

    // 正常目录：新快照 + 非 json 文件。
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());
    let snapshot = ContinuationSnapshot {
        task_id: "cov-stale-1".to_string(),
        messages: "[]".to_string(),
        tool_call_id: "tc".to_string(),
        channel: "web".to_string(),
        chat_id: "c".to_string(),
        session_key: String::new(),
        peer_id: String::new(),
        image_refs: Vec::new(),
        image_refs_by_user_turn: Vec::new(),
        created_at: chrono::Local::now().to_rfc3339(),
        final_persisted: false,
    };
    store.save(&snapshot).expect("save snapshot");
    std::fs::write(dir.path().join("not-a-snapshot.txt"), b"skip me").unwrap();

    // max_age 极大 → cutoff 远古 → 刚写的快照比 cutoff 新 → 不 stale。
    assert!(
        store
            .stale_task_ids(Duration::from_secs(24 * 3600))
            .is_empty()
    );

    // 让文件 mtime 落后于 cutoff（max_age=10ms）→ 命中 stale；.txt 不入选。
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(
        store.stale_task_ids(Duration::from_millis(10)),
        vec!["cov-stale-1".to_string()]
    );
    store.delete("cov-stale-1");
    assert!(
        store
            .stale_task_ids(Duration::from_secs(24 * 3600))
            .is_empty()
    );
}

// ---------------------------------------------------------------------------
// save/load：磁盘快照剥 base64 字节、保留路径引用
// ---------------------------------------------------------------------------

#[tokio::test]
async fn disk_snapshot_strips_image_bytes_keeps_refs() {
    let dir = tempfile::tempdir().unwrap();
    let img = dir.path().join("snap.png");
    std::fs::write(&img, png_bytes()).unwrap();
    let img_ref = img.to_string_lossy().into_owned();

    // 构造已水合消息（images[].data 非空）。
    let mut msg = user_msg("带图消息");
    msg.images.push(crate::image_attach::LlmImage {
        path: img_ref.clone(),
        media_type: "image/png".to_string(),
        data: "QUFBQQ==".to_string(),
    });

    let ws = dir.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let mgr = ContinuationManager::with_disk_store(&ws);
    mgr.save_continuation_with_images(
        "cov-strip-1",
        vec![msg],
        "tc-1",
        "web",
        "chat-1",
        "agent:main:session:covcont",
        "peer-1",
        std::slice::from_ref(&img_ref),
    )
    .await;

    // 盘上快照：messages 无 images 字节，image_refs 保留。
    let disk = ContinuationStore::new(&ws);
    let snap = disk.load("cov-strip-1").expect("snapshot on disk");
    let parsed: Vec<serde_json::Value> = serde_json::from_str(&snap.messages).unwrap();
    assert_eq!(parsed.len(), 1);
    assert!(
        parsed[0].get("images").is_none()
            || parsed[0]["images"]
                .as_array()
                .map(|a| a.is_empty())
                .unwrap_or(true),
        "image bytes must be stripped: {}",
        snap.messages
    );
    assert_eq!(snap.image_refs, vec![img_ref.clone()]);
    assert_eq!(snap.image_refs_by_user_turn.len(), 1);
    assert_eq!(snap.image_refs_by_user_turn[0], vec![img_ref]);
}

// ---------------------------------------------------------------------------
// 内存 map 同步入口 + wait_for_continuation 就绪双检
// ---------------------------------------------------------------------------

#[test]
fn sync_map_helpers_and_bg_prefix_filter() {
    let mgr = ContinuationManager::new();
    assert!(!mgr.has_continuation_sync("bg_1"));
    mgr.insert_continuation_sync("bg_1".to_string(), cont_data(false));
    mgr.insert_continuation_sync("plain".to_string(), cont_data(false));
    assert!(mgr.has_continuation_sync("bg_1"));
    assert!(mgr.has_continuation_sync("plain"));

    let bg = mgr.list_bg_spawn_pending_sync();
    assert_eq!(bg, vec!["bg_1".to_string()], "only BG prefix listed");
}

/// 已就绪 → 立即返回（快路径）；未就绪 → 等 notify 后返回（重检路径）。
#[tokio::test]
async fn wait_for_continuation_ready_paths() {
    let mgr = ContinuationManager::new();
    mgr.insert_continuation_sync("cov-ready".to_string(), cont_data(true));
    let got = mgr.wait_for_continuation("cov-ready").await;
    assert!(got.is_some(), "ready entry returns immediately");

    // 未就绪：50ms 后置 ready + notify → wait 返回 Some。
    let data = cont_data(false);
    mgr.insert_continuation_sync("cov-late".to_string(), data.clone());
    {
        let data = data.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            data.ready_flag.store(true, AtomicOrdering::Release);
            data.ready.notify_waiters();
        });
    }
    let got = tokio::time::timeout(
        Duration::from_secs(2),
        mgr.wait_for_continuation("cov-late"),
    )
    .await
    .expect("must wake before timeout");
    assert!(got.is_some());
}

// ---------------------------------------------------------------------------
// wave5c：by-turn 重水合占位/短快照臂 + cleanup_old_snapshots 两臂 +
// has_continuation_sync 锁争用臂
// ---------------------------------------------------------------------------

/// 按 user 轮重水合：缺文件 → 占位文本追加进 content；
/// 快照引用轮数少于 user 轮数 → 多出的轮诚实缺省（不恢复、不改写）。
#[test]
fn rehydrate_by_user_turn_placeholder_and_short_snapshot() {
    // 缺文件 → 占位追加（images 空，content 变长）。
    let mut msgs = vec![user_msg("第一轮带图")];
    rehydrate_images_by_user_turn(&mut msgs, &[vec!["Z:/no/such.png".to_string()]]);
    assert!(msgs[0].images.is_empty());
    assert!(
        msgs[0].content.len() > "第一轮带图".len(),
        "placeholder appended: {}",
        msgs[0].content
    );

    // 快照只有一轮引用 → 第二条 user 轮不恢复、内容原样。
    let mut two = vec![user_msg("a"), user_msg("b")];
    rehydrate_images_by_user_turn(&mut two, &[vec!["Z:/x.png".to_string()]]);
    assert!(two[1].images.is_empty());
    assert_eq!(
        two[1].content, "b",
        "second turn untouched (snapshot shorter)"
    );
}

/// cleanup_old_snapshots：无盘 store → 0（no-op）；有 store + 过期 →
/// 内存 + 盘一起回收。
#[tokio::test]
async fn cleanup_old_snapshots_removes_stale_and_handles_missing_store() {
    // 无 disk store。
    let mgr = ContinuationManager::new();
    assert_eq!(
        mgr.cleanup_old_snapshots(Duration::from_secs(3600)).await,
        0
    );

    // 有 disk store：过期快照被清。
    let ws = tempfile::tempdir().unwrap();
    let mgr = ContinuationManager::with_disk_store(ws.path());
    mgr.save_continuation("cov-clean-1", vec![user_msg("m")], "tc", "web", "c", "", "")
        .await;
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(
        mgr.cleanup_old_snapshots(Duration::from_millis(10)).await,
        1
    );
    assert!(
        !mgr.has_continuation_sync("cov-clean-1"),
        "memory twin evicted"
    );
    assert_eq!(
        mgr.cleanup_old_snapshots(Duration::from_millis(10)).await,
        0,
        "second pass finds nothing"
    );
}

/// has_continuation_sync：锁被占用（try_lock 失败）→ false，绝不阻塞。
#[tokio::test]
async fn has_continuation_sync_contended_lock_returns_false() {
    let mgr = ContinuationManager::new();
    mgr.insert_continuation_sync("cov-lock".to_string(), cont_data(false));
    {
        let _guard = mgr.continuations.lock().await;
        assert!(!mgr.has_continuation_sync("cov-lock"), "contended → false");
    }
    assert!(mgr.has_continuation_sync("cov-lock"), "uncontended → true");
}
