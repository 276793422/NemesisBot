// continuation_store.rs 覆盖率补充测试（remove 磁盘删除失败 warn /
// cleanup_old 过期快照清理 / list_pending 磁盘独有条目 /
// recover_from_disk 手工文件恢复 + 读取失败跳过）。
//
// 豁免：213 区域的 `?` 上抛臂（cleanup_old 的 read_dir/metadata 错误——
// 目录在扫描中消失的竞态面，单测无法确定性构造）。

use super::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static SEQ: AtomicU64 = AtomicU64::new(0);

fn temp_cache(tag: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("nmb-cstore-cov-{}-{tag}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn snap(id: &str) -> ContinuationSnapshot {
    ContinuationSnapshot {
        task_id: id.into(),
        messages: serde_json::json!([{"role": "user", "content": "cov"}]),
        tool_call_id: "call-1".into(),
        channel: "rpc".into(),
        chat_id: "chat-1".into(),
        ready: true,
        created_at: chrono::Local::now().to_rfc3339(),
    }
}

/// remove：快照文件被无 DELETE 共享的句柄占用 → 磁盘删除失败只 warn，
/// 内存照常移除、返回 true（156；Windows 共享语义）。
#[cfg(windows)]
#[tokio::test]
async fn remove_disk_delete_failure_warns() {
    use std::os::windows::fs::OpenOptionsExt;

    let dir = temp_cache("remove-locked");
    let store = ContinuationStore::new(&dir);
    store.save(snap("t-locked")).await.unwrap();

    let path = dir.join("t-locked.json");
    assert!(path.exists());
    let _guard = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0x0001 | 0x0002) // 无 FILE_SHARE_DELETE
        .open(&path)
        .unwrap();

    assert!(store.remove("t-locked").await, "内存条目在 → true");
    assert!(!store.contains("t-locked"));
    assert!(path.exists(), "磁盘文件删除失败留在原地（warn 臂）");
}

/// cleanup_old：mtime 超龄的快照（内存 + 磁盘）被清，新鲜快照保留（213）。
#[tokio::test]
async fn cleanup_old_removes_stale_keeps_fresh() {
    let dir = temp_cache("cleanup");
    let store = ContinuationStore::new(&dir);
    store.save(snap("t-stale")).await.unwrap();
    store.save(snap("t-fresh")).await.unwrap();

    // 把 t-stale 的 mtime 回拨 3 天。
    let stale_path = dir.join("t-stale.json");
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3 * 24 * 3600);
    {
        let f = std::fs::File::options()
            .write(true)
            .open(&stale_path)
            .unwrap();
        f.set_times(std::fs::FileTimes::new().set_modified(old))
            .unwrap();
    }

    let removed = store
        .cleanup_old(std::time::Duration::from_secs(24 * 3600))
        .await
        .unwrap();

    assert_eq!(removed, 1, "只清超龄的 t-stale");
    assert!(!stale_path.exists());
    assert!(!store.contains("t-stale"), "内存同步清掉");
    assert!(store.contains("t-fresh"), "新鲜快照保留");
    assert!(dir.join("t-fresh.json").exists());
}

/// list_pending：磁盘独有（内存没有）的快照 id 也入列，且不重复（250）。
#[tokio::test]
async fn list_pending_includes_disk_only_entries() {
    let dir = temp_cache("list");
    let store = ContinuationStore::new(&dir);

    // 内存 + 磁盘各一：t-mem 走 save（内存+磁盘），t-disk 手工落盘。
    store.save(snap("t-mem")).await.unwrap();
    std::fs::write(
        dir.join("t-disk.json"),
        serde_json::to_string_pretty(&snap("t-disk")).unwrap(),
    )
    .unwrap();
    // 非 json 文件不算 pending。
    std::fs::write(dir.join("loose.txt"), "x").unwrap();

    let mut ids = store.list_pending().await;
    ids.sort();
    assert_eq!(
        ids,
        vec!["t-disk".to_string(), "t-mem".to_string()],
        "{ids:?}"
    );
}

/// recover_from_disk：手工落盘的合法快照恢复入内存（294）。
#[tokio::test]
async fn recover_from_disk_restores_manual_file() {
    let dir = temp_cache("recover-ok");
    let store = ContinuationStore::new(&dir);
    std::fs::write(
        dir.join("t-recover.json"),
        serde_json::to_string_pretty(&snap("t-recover")).unwrap(),
    )
    .unwrap();

    let n = store.recover_from_disk().await.unwrap();

    assert_eq!(n, 1);
    assert!(store.contains("t-recover"));
    let got = store.load("t-recover").await.unwrap();
    assert_eq!(got.task_id, "t-recover");
}

/// recover_from_disk：快照文件被无 READ 共享的句柄占用 → 读取失败跳过
/// 不 panic（302/308；Windows 共享语义）。
#[cfg(windows)]
#[tokio::test]
async fn recover_from_disk_read_failure_skips() {
    use std::os::windows::fs::OpenOptionsExt;

    let dir = temp_cache("recover-locked");
    let store = ContinuationStore::new(&dir);
    std::fs::write(
        dir.join("t-locked.json"),
        serde_json::to_string_pretty(&snap("t-locked")).unwrap(),
    )
    .unwrap();

    let path = dir.join("t-locked.json");
    // 占住文件：share_mode 只给写/删、不给 FILE_SHARE_READ → 读取必败。
    let _guard = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0x0002 | 0x0004)
        .open(&path)
        .unwrap();

    let n = store.recover_from_disk().await.unwrap();

    assert_eq!(n, 0, "读取失败臂跳过");
    assert!(!store.contains("t-locked"));
}
