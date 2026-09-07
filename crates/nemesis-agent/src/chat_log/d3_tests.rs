//! D3（devtool-upgrade 阶段 5）：消息↔文件变更映射的 chat_log 侧测试。
//!
//! - `append_chat_log_meta` 全字段入口：`file_changes` 字段写入形状
//!   （`[{path, kind}]`），空清单/旧签名行**不带**该键（宽容读契约）。
//! - `dedup_file_changes` 投影：按 path 去重、首次出现顺序、kind 取最后
//!   一次声明（M3 依赖此形状）。
//! - 与 checkpoint 语义正交：这里只钉 jsonl 行形状，恢复语义在 checkpoint
//!   自己的测试里。

use super::*;
use crate::r#loop::FileChangeKind;

fn d3_uniq_key(tag: &str) -> String {
    format!(
        "test:d3:{}:{}",
        tag,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn fc(path: &str, kind: FileChangeKind) -> FileChange {
    FileChange {
        path: path.to_string(),
        kind,
    }
}

/// 全字段入口写 file_changes；旧签名行不带该键（缺字段 = 无变更的宽容读
/// 契约，与 model/cron/images 同一形态）。
#[test]
fn test_d3_meta_entry_writes_file_changes_legacy_rows_omit() {
    let key = d3_uniq_key("write");
    delete_chat_log(&key);

    let changes = vec![
        fc("src/lib.rs", FileChangeKind::Modify),
        fc("src/new.rs", FileChangeKind::Create),
    ];
    append_chat_log_meta(
        &key,
        "assistant",
        "edit done",
        &ChatLogMeta {
            model: Some("prov/model"),
            file_changes: &changes,
            ..Default::default()
        },
    );
    // 同一 turn 的 user 行走旧签名——不得带 file_changes。
    append_chat_log(&key, "user", "do the edit");

    let (rows, total, _, _) = read_chat_log(&key, 10, None);
    assert_eq!(total, 2);

    let a = &rows[0];
    assert_eq!(a["role"].as_str(), Some("assistant"));
    assert_eq!(a["model"].as_str(), Some("prov/model"));
    let arr = a["file_changes"].as_array().expect("file_changes array");
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["path"].as_str(), Some("src/lib.rs"));
    assert_eq!(arr[0]["kind"].as_str(), Some("Modify"));
    assert_eq!(arr[1]["path"].as_str(), Some("src/new.rs"));
    assert_eq!(arr[1]["kind"].as_str(), Some("Create"));

    assert!(
        rows[1].get("file_changes").is_none(),
        "旧签名行不得带 file_changes 键"
    );

    delete_chat_log(&key);
}

/// 空清单不写字段（jsonl 行与旧形态逐字段一致）。
#[test]
fn test_d3_empty_file_changes_omits_field() {
    let key = d3_uniq_key("empty");
    delete_chat_log(&key);

    append_chat_log_meta(&key, "assistant", "plain reply", &ChatLogMeta::default());
    let (rows, _, _, _) = read_chat_log(&key, 10, None);
    assert_eq!(rows.len(), 1);
    assert!(rows[0].get("file_changes").is_none());
    assert!(rows[0].get("model").is_none());

    delete_chat_log(&key);
}

/// dedup_file_changes：同 path 合并为一条、kind 取最后一次声明、保留首次
/// 出现顺序；不同 path 互不影响；空输入直通。
#[test]
fn test_d3_dedup_last_kind_wins_first_order_kept() {
    let deduped = dedup_file_changes(vec![
        fc("a.rs", FileChangeKind::Create),
        fc("b.rs", FileChangeKind::Modify),
        fc("a.rs", FileChangeKind::Modify), // a 第二次声明：kind 覆盖
        fc("c.rs", FileChangeKind::Delete),
        fc("b.rs", FileChangeKind::Delete), // b 第三次声明
    ]);
    let paths: Vec<&str> = deduped.iter().map(|c| c.path.as_str()).collect();
    assert_eq!(paths, vec!["a.rs", "b.rs", "c.rs"], "首次出现顺序");
    assert_eq!(deduped[0].kind, FileChangeKind::Modify, "a 最后声明 Modify");
    assert_eq!(deduped[1].kind, FileChangeKind::Delete, "b 最后声明 Delete");
    assert_eq!(deduped[2].kind, FileChangeKind::Delete);

    assert!(dedup_file_changes(Vec::new()).is_empty());
}
