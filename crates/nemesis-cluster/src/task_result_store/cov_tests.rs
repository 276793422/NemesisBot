// task_result_store.rs 覆盖率补充测试（safe_task_file_name 消毒 warn /
// sweep 非 json 跳过 + 过期删除 / 容量逐出连带磁盘删除 /
// async delete 磁盘删除失败 warn）。
//
// 豁免：328-329 / 542-543 / 550（write_to_disk 与 write_to_disk_async 的
// serde 序列化失败臂——TaskResult 全字段可序列化，to_string_pretty 数学上
// 不可能失败，死防御）。

use super::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static SEQ: AtomicU64 = AtomicU64::new(0);

fn temp_cache(tag: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let dir =
        std::env::temp_dir().join(format!("nmb-trstore-cov-{}-{tag}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// store_success 带 unsafe task_id：消毒落盘 + WARN 留痕（29），
/// 内存索引仍用原 id。
#[test]
fn store_success_sanitizes_unsafe_task_id() {
    let dir = temp_cache("sanitize");
    let store = TaskResultStore::with_disk_persistence(8, &dir);

    store.store_success("a/b:c", "query", serde_json::json!({"ok": true}));

    // 内存索引原 id 可取。
    assert!(store.get("a/b:c").is_some());
    // 磁盘文件名是消毒后的安全段。
    let safe = dir.join("a_b_c.json");
    assert!(
        safe.exists(),
        "消毒文件名落盘：{:?}",
        std::fs::read_dir(&dir).map(|rd| rd.flatten().map(|e| e.file_name()).collect::<Vec<_>>())
    );
}

/// sweep_older_than：非 json 文件跳过（251）；过期 json 删除计数（263）。
#[test]
fn sweep_skips_non_json_and_removes_expired() {
    let dir = temp_cache("sweep");
    let store = TaskResultStore::with_disk_persistence(8, &dir);

    store.store_success("t-expire", "query", serde_json::json!({"v": 1}));
    std::fs::write(dir.join("loose.txt"), "not json").unwrap();

    // 把 t-expire 的 stored_at 改成 3 天前 → 相对 1 天闸为过期。
    let path = dir.join("t-expire.json");
    let mut val: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    val["stored_at"] =
        serde_json::json!((chrono::Local::now() - chrono::Duration::days(3)).to_rfc3339());
    std::fs::write(&path, val.to_string()).unwrap();
    // 回灌内存（load 覆盖写）→ 内存条目也带旧时间戳，sweep 的内存裁剪臂
    // 才有可裁对象。
    store.load_from_disk();

    let removed = store.sweep_older_than(chrono::Duration::days(1));

    assert_eq!(removed, 1, "只删过期的 t-expire");
    assert!(!path.exists(), "过期文件已删");
    assert!(dir.join("loose.txt").exists(), "非 json 原样保留");
    assert!(store.get("t-expire").is_none(), "内存同步清掉");
}

/// 容量逐出：满员再存 → 逐出最旧条目并连带删其磁盘文件（297）。
#[test]
fn store_evicts_oldest_and_deletes_from_disk() {
    let dir = temp_cache("evict");
    let store = TaskResultStore::with_disk_persistence(1, &dir);

    store.store_success("t-first", "query", serde_json::json!(1));
    store.store_success("t-second", "query", serde_json::json!(2));

    assert_eq!(store.len(), 1, "容量 1");
    assert!(store.get("t-first").is_none(), "先入者被逐出");
    assert!(store.get("t-second").is_some());
    assert!(
        !dir.join("t-first.json").exists(),
        "被逐出条目的磁盘文件连带删除"
    );
    assert!(dir.join("t-second.json").exists());
}

/// cleanup_delivered_async：结果文件被无 DELETE 共享的句柄占用 →
/// 磁盘删除失败只 warn，内存照常移除（550/565；Windows 共享语义）。
#[cfg(windows)]
#[tokio::test]
async fn cleanup_delivered_async_delete_failure_warns() {
    use std::os::windows::fs::OpenOptionsExt;

    let dir = temp_cache("locked");
    let store = AsyncTaskResultStore::with_disk_persistence(8, &dir);

    store
        .store_success_async("t-locked", "query", serde_json::json!({"x": 1}))
        .await;
    let path = dir.join("t-locked.json");
    assert!(path.exists());

    // 占住文件：share_mode 只给读写、不给 FILE_SHARE_DELETE → remove 必败。
    let _guard = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0x0001 | 0x0002)
        .open(&path)
        .unwrap();

    let existed = store.cleanup_delivered_async("t-locked").await;

    assert!(existed, "内存里有 → true");
    assert!(store.get_async("t-locked").is_none(), "内存条目移除");
    assert!(path.exists(), "磁盘文件删除失败留在原地（warn 臂）");
}
