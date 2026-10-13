//! N5（2026-10-13 机会项）：交付物登记的 chat_log 侧测试。
//!
//! - `append_chat_log_meta` 全字段入口：`deliverables` 字段写入形状
//!   （`[{path, title, note}]`），空清单/旧签名行**不带**该键（宽容读
//!   契约，与 file_changes/images 同一形态）。
//! - `dedup_presented_files` 投影：按 path 去重、保留首次出现顺序、
//!   title/note 取最后一次声明（与 `dedup_file_changes` 同构语义）。
//! - serde roundtrip： PresentedFile 与前端/AgentEvent 载荷同形（缺省
//!   title/note 反序列化为空串——`#[serde(default)]` 契约）。

use super::*;

fn n5_uniq_key(tag: &str) -> String {
    format!(
        "test:n5:{}:{}",
        tag,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn pf(path: &str, title: &str, note: &str) -> PresentedFile {
    PresentedFile {
        path: path.to_string(),
        title: title.to_string(),
        note: note.to_string(),
    }
}

/// 全字段入口写 deliverables；旧签名行不带该键（缺字段 = 无交付物的宽容读
/// 契约，与 model/cron/images/file_changes 同一形态）。
#[test]
fn test_n5_meta_entry_writes_deliverables_legacy_rows_omit() {
    let key = n5_uniq_key("write");
    delete_chat_log(&key);

    let files = vec![
        pf("reports/daily.md", "日报", "今日汇总"),
        pf("out/chart.png", "趋势图", ""),
    ];
    append_chat_log_meta(
        &key,
        "assistant",
        "done, here are your deliverables",
        &ChatLogMeta {
            deliverables: &files,
            ..Default::default()
        },
    );
    // 同一 turn 的 user 行走旧签名——不得带 deliverables。
    append_chat_log(&key, "user", "给我出个日报");

    let (rows, total, _, _) = read_chat_log(&key, 10, None);
    assert_eq!(total, 2);

    let a = &rows[0];
    assert_eq!(a["role"].as_str(), Some("assistant"));
    let arr = a["deliverables"].as_array().expect("deliverables array");
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["path"].as_str(), Some("reports/daily.md"));
    assert_eq!(arr[0]["title"].as_str(), Some("日报"));
    assert_eq!(arr[0]["note"].as_str(), Some("今日汇总"));
    assert_eq!(arr[1]["path"].as_str(), Some("out/chart.png"));
    assert_eq!(arr[1]["title"].as_str(), Some("趋势图"));
    assert_eq!(
        arr[1]["note"].as_str(),
        Some(""),
        "note 空串照写（serde default 形态）"
    );

    assert!(
        rows[1].get("deliverables").is_none(),
        "旧签名行不得带 deliverables 键"
    );

    delete_chat_log(&key);
}

/// 空清单不写字段（jsonl 行与旧形态逐字段一致）。
#[test]
fn test_n5_empty_deliverables_omit_field() {
    let key = n5_uniq_key("empty");
    delete_chat_log(&key);

    append_chat_log_meta(&key, "assistant", "plain reply", &ChatLogMeta::default());
    let (rows, _, _, _) = read_chat_log(&key, 10, None);
    assert_eq!(rows.len(), 1);
    assert!(rows[0].get("deliverables").is_none());

    delete_chat_log(&key);
}

/// dedup_presented_files：同 path 合并为一条、title/note 取最后一次声明、
/// 保留首次出现顺序；不同 path 互不影响；空输入直通。
#[test]
fn test_n5_dedup_last_meta_wins_first_order_kept() {
    let deduped = dedup_presented_files(vec![
        pf("a.md", "v1", "note1"),
        pf("b.md", "B", ""),
        pf("a.md", "v2", "note2"), // a 第二次声明：title/note 覆盖
        pf("c.md", "C", "C-note"),
        pf("b.md", "B2", "B2-note"), // b 第二次声明
    ]);
    let paths: Vec<&str> = deduped.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, vec!["a.md", "b.md", "c.md"], "首次出现顺序");
    assert_eq!(deduped[0].title, "v2", "a 的 title 取最后声明");
    assert_eq!(deduped[0].note, "note2");
    assert_eq!(deduped[1].title, "B2");
    assert_eq!(deduped[1].note, "B2-note");
    assert_eq!(deduped[2].title, "C");

    assert!(dedup_presented_files(Vec::new()).is_empty());
}

/// serde roundtrip：title/note 缺省反序列化为空串（`#[serde(default)]`，
/// AgentEvent 载荷/前端字段对齐契约）；全字段形态无损回读。
#[test]
fn test_n5_presented_file_serde_defaults() {
    let minimal: PresentedFile =
        serde_json::from_str(r#"{"path": "only/path.txt"}"#).expect("path-only 反序列化");
    assert_eq!(minimal.path, "only/path.txt");
    assert_eq!(minimal.title, "", "缺省 title = 空串（前端取文件名）");
    assert_eq!(minimal.note, "", "缺省 note = 空串（前端隐藏）");

    let full: PresentedFile = serde_json::from_value(serde_json::json!({
        "path": "r.md", "title": "T", "note": "N"
    }))
    .expect("全字段反序列化");
    assert_eq!(full, pf("r.md", "T", "N"));
}
