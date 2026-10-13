//! U-B 封存状态机 + 中断档案单测。
//!
//! 状态机测试**自建实例**（`UnavailableState::new`），不碰进程级单例——
//! 并行测试互不污染（estop 纯函数内核同款纪律）。全局壳函数（frozen_now /
//! record_llm_failure）不在此测（它们只是委托 + 日志）。

use std::sync::Arc;

use super::UnavailableState;
use crate::handlers::board_unavailable::{
    archive_interruption, interruption_path, read_pending_interruptions, unavailable_cfg,
    write_interruption,
};
use nemesis_board::store::BoardStore;

fn cfg(freeze: bool, threshold: u32, probe: u64) -> nemesis_config::BoardUnavailableConfig {
    nemesis_config::BoardUnavailableConfig {
        freeze,
        connect_fail_threshold: threshold,
        probe_interval_secs: probe,
    }
}

fn default_cfg() -> nemesis_config::BoardUnavailableConfig {
    nemesis_config::BoardUnavailableConfig::default()
}

const CONNECT_ERR: &str =
    "error sending request for url (http://127.0.0.1:15721/v1): tcp connect error";
const TIMEOUT_ERR: &str =
    "error sending request for url (http://127.0.0.1:15721/v1): operation timed out";

#[test]
fn trip_at_threshold_only_on_edge() {
    let st = UnavailableState::new();
    // 默认阈值 3："x" 非连接类不计入，连接失败第 1/2 次未封存，第 3 次恰好
    // 触发（边沿 true）。
    assert!(!st.record_failure("x", &default_cfg()));
    assert!(!st.is_frozen());
    assert!(!st.record_failure(CONNECT_ERR, &default_cfg()));
    assert!(!st.is_frozen());
    assert!(!st.record_failure(CONNECT_ERR, &default_cfg()));
    assert!(!st.is_frozen());
    assert!(st.record_failure(CONNECT_ERR, &default_cfg()));
    assert!(st.is_frozen());
    // 封存持续期后续失败：不再触发边沿。
    assert!(!st.record_failure(CONNECT_ERR, &default_cfg()));
    assert!(st.is_frozen());
    assert_eq!(st.streak(), 4);
    let snap = st.snapshot();
    assert_eq!(snap["frozen"], serde_json::json!(true));
    assert!(snap["tripped_at"].as_str().is_some());
    assert_eq!(snap["reason"], serde_json::json!(CONNECT_ERR));
}

#[test]
fn non_connect_failures_never_count() {
    let st = UnavailableState::new();
    // 超时（慢模型）/5xx/解析失败不计入——U-B 设计红线。
    assert!(!st.record_failure(TIMEOUT_ERR, &default_cfg()));
    assert!(!st.record_failure("HTTP status 503 service unavailable", &default_cfg()));
    assert!(!st.record_failure("评审输出连续 3 轮无法解析：missing verdict", &default_cfg()));
    assert_eq!(st.streak(), 0);
    assert!(!st.is_frozen());
    // 同一批里夹一次连接失败也只算一次。
    assert!(!st.record_failure(CONNECT_ERR, &default_cfg()));
    assert_eq!(st.streak(), 1);
}

#[test]
fn success_resets_streak_and_releases_on_edge() {
    let st = UnavailableState::new();
    assert!(!st.record_failure(CONNECT_ERR, &default_cfg()));
    assert!(!st.record_failure(CONNECT_ERR, &default_cfg()));
    assert!(st.record_failure(CONNECT_ERR, &default_cfg()));
    assert!(st.is_frozen());
    // 成功清零连击 + 恰好解封（边沿 true）。
    assert!(st.record_success(&default_cfg()));
    assert!(!st.is_frozen());
    assert_eq!(st.streak(), 0);
    // 连击重新累计。
    assert!(!st.record_failure(CONNECT_ERR, &default_cfg()));
    assert!(!st.record_failure(CONNECT_ERR, &default_cfg()));
    assert!(st.record_failure(CONNECT_ERR, &default_cfg()));
    assert!(st.is_frozen());
    // 封存中成功 = 恰好解封（边沿 true；探针/手动成功路径消费）。
    assert!(st.record_success(&default_cfg()));
    assert!(!st.is_frozen());
    // 已解封再成功：false。
    assert!(!st.record_success(&default_cfg()));
}

#[test]
fn freeze_off_pins_legacy_behavior() {
    // freeze=false = 旧行为 pin：连接失败不累计不封存（各环节各自 fail_class
    // 处理），成功也永不产生解封边沿。
    let st = UnavailableState::new();
    let off = cfg(false, 3, 300);
    for _ in 0..5 {
        assert!(!st.record_failure(CONNECT_ERR, &off));
    }
    assert_eq!(st.streak(), 0);
    assert!(!st.is_frozen());
    assert!(!st.record_success(&off));
}

#[test]
fn threshold_zero_treated_as_one() {
    let st = UnavailableState::new();
    let aggressive = cfg(true, 0, 300);
    assert!(st.record_failure(CONNECT_ERR, &aggressive));
    assert!(st.is_frozen());
}

#[test]
fn threshold_one_trips_immediately() {
    let st = UnavailableState::new();
    let tight = cfg(true, 1, 300);
    assert!(st.record_failure(CONNECT_ERR, &tight));
    assert!(st.is_frozen());
}

#[test]
fn manual_release_regardless_of_config() {
    let st = UnavailableState::new();
    assert!(!st.manual_release()); // 未封存时放行 = false（幂等）
    let off = cfg(false, 3, 300);
    // freeze=false 时即便 record 永不封存，手动放行仍可用（人工意志优先）。
    assert!(!st.manual_release());
    let _ = off;
    let on = cfg(true, 1, 300);
    assert!(st.record_failure(CONNECT_ERR, &on));
    assert!(st.is_frozen());
    assert!(st.manual_release());
    assert!(!st.is_frozen());
    assert_eq!(st.streak(), 0);
}

#[test]
fn unavailable_cfg_defaults_on_missing_home() {
    // home 缺失/读取失败 = 缺省值（freeze=true——封存是无人值守默认正确行为）。
    let c = unavailable_cfg(None);
    assert_eq!(c, default_cfg());
    let c = unavailable_cfg(Some(std::path::Path::new("Z:/definitely-not-a-home-xyz")));
    assert_eq!(c, default_cfg());
}

// ---- 中断档案 ----

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!(
        "ub-unavail-{}-{}-{}",
        tag,
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    std::fs::create_dir_all(&d).expect("create temp dir");
    d
}

fn make_issue(store: &BoardStore) -> nemesis_board::models::Issue {
    store
        .create_issue(nemesis_board::models::NewIssue {
            title: "U-B 封存恢复测试单".into(),
            ..Default::default()
        })
        .expect("create issue")
}

#[test]
fn interruption_write_read_archive_roundtrip() {
    let ws = temp_dir("roundtrip");
    let store = BoardStore::open(&ws.join("board.db"), "NB").expect("open store");
    let issue = make_issue(&store);

    let path = write_interruption(
        &ws,
        &store,
        &issue,
        "review",
        "评审挂起：LLM 端点不可用封存中".into(),
        serde_json::json!({ "k": "v" }),
    );
    let path = path.expect("write should succeed");

    // 单一待续档案可读回，字段齐全。
    let pend = read_pending_interruptions(&ws);
    assert_eq!(pend.len(), 1);
    assert_eq!(pend[0]["schema"], serde_json::json!(1));
    assert_eq!(pend[0]["issue"], serde_json::json!(issue.number));
    assert_eq!(pend[0]["pending_action"], serde_json::json!("review"));
    assert_eq!(pend[0]["stop_kind"], serde_json::json!("llm_unavailable"));
    assert_eq!(pend[0]["environment"]["k"], serde_json::json!("v"));
    assert!(pend[0]["resume_entry"].as_str().is_some());
    assert_eq!(path, interruption_path(&ws, &issue.number));

    // 同单重写 = 覆盖（不刷屏）。
    write_interruption(
        &ws,
        &store,
        &issue,
        "review",
        "重挂起".into(),
        serde_json::json!({}),
    );
    assert_eq!(read_pending_interruptions(&ws).len(), 1);

    // 归档：pending 清空、archive 有带时戳副本。
    assert!(archive_interruption(&ws, &path));
    assert!(read_pending_interruptions(&ws).is_empty());
    let archive_dir = ws.join("board").join("interruptions").join("archive");
    let archived: Vec<_> = std::fs::read_dir(&archive_dir)
        .expect("archive dir")
        .flatten()
        .collect();
    assert_eq!(archived.len(), 1);
    assert!(
        archived[0]
            .file_name()
            .to_string_lossy()
            .contains(&issue.number)
    );

    std::fs::remove_dir_all(&ws).ok();
}

#[test]
fn interruption_read_tolerates_missing_dir_and_bad_json() {
    let ws = temp_dir("tolerant");
    assert!(read_pending_interruptions(&ws).is_empty());
    let dir = ws.join("board").join("interruptions");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("NB-999.json"), "{not json").unwrap();
    std::fs::write(dir.join("ignore.txt"), "x").unwrap();
    assert!(read_pending_interruptions(&ws).is_empty());
    std::fs::remove_dir_all(&ws).ok();
}

#[test]
fn concurrent_state_instances_are_independent() {
    // Arc 共享同一实例计账一致（生产 shared path）：三次失败跨 a/b 句柄
    // 连续累计，第三次恰好 trip。
    let a = Arc::new(UnavailableState::new());
    let b = Arc::clone(&a);
    assert!(!b.record_failure(CONNECT_ERR, &default_cfg()));
    assert!(!b.record_failure(CONNECT_ERR, &default_cfg()));
    assert!(a.record_failure(CONNECT_ERR, &default_cfg()));
    assert!(a.is_frozen());
    // 不同实例互不影响：新实例零计数不封存，独立按自身阈值 trip，释放也
    // 不波及 a。
    let c = UnavailableState::new();
    assert_eq!(c.streak(), 0);
    assert!(!c.is_frozen());
    assert!(c.record_failure(CONNECT_ERR, &cfg(true, 1, 300)));
    assert!(c.is_frozen());
    assert!(c.manual_release());
    assert!(!c.is_frozen());
    assert!(a.is_frozen());
}
