//! store 模块测试：canonical 往返、旧 CLI 裸数组迁移（跨层回归锁的库侧
//! 一半）、损坏拒绝、父目录创建。生产行为对应 docs/BUG/2026-10-13_
//! cron-store-format-split-cli-vs-gateway.md 的修复。

use super::*;
use tempfile::TempDir;

/// 旧 CLI 手拼形态的完整条目（含 display 字段——迁移应忽略它）。
fn legacy_interval_json() -> String {
    r#"[
  {
    "id": "abc12345",
    "name": "old-cli-job",
    "message": "do the thing",
    "schedule": { "kind": "interval", "every_ms": 60000, "display": "every 60s" },
    "deliver": true,
    "to": "user1",
    "channel": "web",
    "enabled": false
  }
]"#
    .to_string()
}

#[test]
fn canonical_roundtrip_preserves_jobs() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("jobs.json");
    let now = Local::now().timestamp_millis();
    let store = CronStoreData {
        version: 1,
        jobs: vec![CronJob {
            id: "jid0001".to_string(),
            name: "n".to_string(),
            enabled: true,
            schedule: CronSchedule {
                kind: "every".to_string(),
                at_ms: None,
                every_ms: Some(5000),
                expr: None,
                tz: None,
            },
            payload: CronPayload {
                kind: "agent_turn".to_string(),
                message: "m".to_string(),
                command: None,
                deliver: false,
                channel: None,
                to: None,
                session_key: None,
                max_rounds: None,
            },
            state: CronJobState {
                next_run_at_ms: Some(now + 5000),
                last_run_at_ms: None,
                last_status: None,
                last_error: None,
                history: vec![],
            },
            created_at_ms: now,
            updated_at_ms: now,
            delete_after_run: false,
        }],
    };
    save(&path, &store).unwrap();
    let loaded = load(&path).unwrap();
    assert_eq!(loaded.version, 1);
    assert_eq!(loaded.jobs.len(), 1);
    assert_eq!(loaded.jobs[0].id, "jid0001");
    assert_eq!(loaded.jobs[0].schedule.kind, "every");
    assert_eq!(loaded.jobs[0].payload.message, "m");
}

#[test]
fn missing_file_loads_fresh_store() {
    let tmp = TempDir::new().unwrap();
    let store = load(&tmp.path().join("absent.json")).unwrap();
    assert_eq!(store.version, 1);
    assert!(store.jobs.is_empty());
}

#[test]
fn legacy_cli_array_migrates_to_canonical_jobs() {
    // 修复核心：gateway 从前对这种文件永远 parse 失败（invalid type: map,
    // expected i32）。现在必须迁移成可用的 CronJob。
    let parsed = parse(&legacy_interval_json()).unwrap();
    assert_eq!(parsed.version, 1);
    assert_eq!(parsed.jobs.len(), 1);
    let job = &parsed.jobs[0];
    assert_eq!(job.id, "abc12345");
    assert_eq!(job.name, "old-cli-job");
    assert!(!job.enabled, "旧条目的 enabled 必须保留");
    assert_eq!(job.schedule.kind, "every", "旧 kind=interval 映射为 every");
    assert_eq!(job.schedule.every_ms, Some(60000));
    assert_eq!(job.payload.kind, "agent_turn");
    assert_eq!(job.payload.message, "do the thing");
    assert!(job.payload.deliver);
    assert_eq!(job.payload.to.as_deref(), Some("user1"));
    assert_eq!(job.payload.channel.as_deref(), Some("web"));
    assert!(job.state.history.is_empty(), "迁移条目从零历史开始");
    assert!(!job.delete_after_run);
}

#[test]
fn legacy_cron_kind_entry_migrates_with_expr() {
    let text = r#"[
      {"id":"c1","name":"cj","message":"m",
       "schedule":{"kind":"cron","expr":"*/5 * * * *","display":"cron: */5 * * * *"},
       "enabled":true}
    ]"#;
    let parsed = parse(text).unwrap();
    assert_eq!(parsed.jobs[0].schedule.kind, "cron");
    assert_eq!(parsed.jobs[0].schedule.expr.as_deref(), Some("*/5 * * * *"));
}

#[test]
fn legacy_interval_without_every_ms_is_rejected_with_position() {
    let text = r#"[{"id":"x","name":"broken","message":"m","schedule":{"kind":"interval"}}]"#;
    let err = parse(text).expect_err("interval 缺 every_ms 必须拒绝");
    assert!(err.contains("broken"), "错误必须能定位条目: {err}");
    assert!(err.contains("every_ms"), "错误必须说明原因: {err}");
}

#[test]
fn legacy_cron_without_expr_is_rejected() {
    let text = r#"[{"id":"x","name":"broken2","message":"m","schedule":{"kind":"cron"}}]"#;
    let err = parse(text).expect_err("cron 缺 expr 必须拒绝");
    assert!(err.contains("broken2"), "错误必须能定位条目: {err}");
}

#[test]
fn legacy_unknown_schedule_kind_is_rejected() {
    let text = r#"[{"id":"x","name":"weird","message":"m","schedule":{"kind":"lunar"}}]"#;
    let err = parse(text).expect_err("未知 schedule.kind 必须拒绝");
    assert!(err.contains("lunar"), "错误必须点出未知 kind: {err}");
}

#[test]
fn corrupt_text_is_rejected_not_silently_emptied() {
    let err = parse("not-a-json-array {{{").expect_err("损坏文本必须 Err");
    assert!(!err.is_empty());
    // 对象但字段残缺（gateway 曾写出的形态不会被误判为 legacy）。
    assert!(parse(r#"{"nope":1}"#).is_err(), "残缺 envelope 必须拒绝");
}

#[test]
fn save_creates_parent_dirs() {
    let tmp = TempDir::new().unwrap();
    let nested = tmp.path().join("a/b/c/cron.json");
    save(&nested, &CronStoreData::new_v1()).unwrap();
    assert!(nested.exists());
    let loaded = load(&nested).unwrap();
    assert_eq!(loaded.version, 1);
}

#[test]
fn migrated_file_resaves_as_canonical_envelope() {
    // 迁移后写盘 → 文件从裸数组愈合为 envelope，再次读取不再走 legacy 臂。
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("jobs.json");
    std::fs::write(&path, legacy_interval_json()).unwrap();
    let migrated = load(&path).unwrap();
    save(&path, &migrated).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.trim_start().starts_with('{'),
        "落盘必须是 envelope 形态，实际开头: {}",
        &text.chars().take(8).collect::<String>()
    );
    let reloaded = load(&path).unwrap();
    assert_eq!(reloaded.jobs.len(), 1);
    assert_eq!(reloaded.jobs[0].id, "abc12345");
}
