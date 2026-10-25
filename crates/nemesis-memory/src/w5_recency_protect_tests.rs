//! W5（2026-10-25 加固）：dreaming 近期召回硬保护
//! 测试——最近被召回（≤ [`RECALL_PROTECT_WINDOW_DAYS`] 天）或累计召回
//! ≥ [`RECALL_PROTECT_MIN_RECALLS`] 次的条目排除出 expire（merge/promote
//! 仍可参与）；边界（恰在窗口上）钉死。

use crate::dreaming::{
    DreamAction, DreamDecision, RECALL_PROTECT_MIN_RECALLS, RECALL_PROTECT_WINDOW_DAYS,
    apply_decision, is_recency_protected, validate_decision,
};
use crate::manager::{Config, META_LAST_RECALL, META_RECALL_COUNT, MemoryManager};
use crate::types::{Entry, MemoryType};
use chrono::{Duration, Local};
use std::collections::HashSet;
use tempfile::tempdir;

fn mem_manager() -> MemoryManager {
    let dir = tempdir().expect("tempdir");
    MemoryManager::new(&Config::new(dir.path()))
}

fn entry(content: &str) -> Entry {
    Entry::new(MemoryType::ShortTerm, content.to_string())
}

/// 盖召回记账（直接写 metadata，语义与 record_recall 落盘键一致）。
fn stamp_recall(e: &mut Entry, last_recall: Option<chrono::DateTime<chrono::Local>>, count: u64) {
    if let Some(t) = last_recall {
        e.metadata
            .insert(META_LAST_RECALL.to_string(), t.to_rfc3339());
    }
    e.metadata
        .insert(META_RECALL_COUNT.to_string(), count.to_string());
}

fn expire_decision(id: &str) -> DreamDecision {
    DreamDecision {
        action: DreamAction::Expire,
        source_ids: vec![id.to_string()],
        reason: "test".to_string(),
    }
}

// ---------------------------------------------------------------------------
// 纯函数：is_recency_protected（窗口/次数/边界）
// ---------------------------------------------------------------------------

#[test]
fn window_boundary_exactly_at_limit_is_protected() {
    let now = Local::now();
    // 窗口从常量推导（钉死常量本尊，改常量测试跟着走）。
    let window = Duration::seconds((RECALL_PROTECT_WINDOW_DAYS * 86_400.0) as i64);
    // 恰在窗口边界上（now - 7d）：天数 = 7.0 ≤ 7.0 → 保护。
    let mut at_limit = entry("恰在保护窗边界");
    stamp_recall(&mut at_limit, Some(now - window), 0);
    assert!(is_recency_protected(&at_limit, now));

    // 边界外一秒（now - 7d - 1s）：天数 > 7.0 → 不保护（召回次数 0）。
    let mut outside = entry("窗外一秒");
    stamp_recall(&mut outside, Some(now - window - Duration::seconds(1)), 0);
    assert!(!is_recency_protected(&outside, now));
}

#[test]
fn never_recalled_low_count_is_not_protected() {
    let now = Local::now();
    let mut fresh = entry("从未召回且次数为 0");
    stamp_recall(&mut fresh, None, 0);
    assert!(!is_recency_protected(&fresh, now));

    // 完全无记账 metadata（老条目常态）同样不保护。
    assert!(!is_recency_protected(&entry("无记账条目"), now));
}

#[test]
fn count_threshold_boundary_protects_despite_stale_recall() {
    let now = Local::now();
    // 召回很旧（窗外）但累计 ≥ 阈值 → 保护。
    let mut veteran = entry("老召回高次数");
    stamp_recall(
        &mut veteran,
        Some(now - Duration::days(30)),
        RECALL_PROTECT_MIN_RECALLS,
    );
    assert!(is_recency_protected(&veteran, now));

    // 阈值边界：恰好 threshold-1 次 → 不保护。
    let mut below = entry("低于阈值");
    stamp_recall(
        &mut below,
        Some(now - Duration::days(30)),
        RECALL_PROTECT_MIN_RECALLS - 1,
    );
    assert!(!is_recency_protected(&below, now));
}

#[test]
fn future_last_recall_is_protected_conservatively() {
    // 时钟偏移导致 last_recall 在未来：保护方向宁严勿松 → 保护。
    let now = Local::now();
    let mut skewed = entry("未来时间戳");
    stamp_recall(&mut skewed, Some(now + Duration::hours(2)), 0);
    assert!(is_recency_protected(&skewed, now));
}

// ---------------------------------------------------------------------------
// 执行闸：apply_decision Expire 臂
// ---------------------------------------------------------------------------

#[tokio::test]
async fn expire_on_protected_entry_rejected_and_untouched() {
    let mgr = mem_manager();
    let now = Local::now();
    let mut e = entry("近期召回过的活跃条目");
    stamp_recall(&mut e, Some(now - Duration::days(1)), 1);
    let id = mgr.store_entry(e).await.unwrap();

    let known: HashSet<String> = [id.clone()].into_iter().collect();
    // 决策本身合法（协议层过闸），执行层硬拦。
    let decision = validate_decision(
        &serde_json::json!({"action": "expire", "source_ids": [id], "reason": "r"}),
        &known,
    )
    .unwrap();
    let err = apply_decision(&mgr, &decision, now).await.unwrap_err();
    assert!(err.contains("近期召回保护"), "{err}");

    // 条目原样：未归档、未 processed。
    let after = mgr.get(&id).await.unwrap().expect("条目仍在");
    assert!(
        !after
            .metadata
            .contains_key(crate::dreaming::META_DREAM_ARCHIVED)
    );
    assert!(
        !after
            .metadata
            .contains_key(crate::dreaming::META_DREAM_PROCESSED_AT)
    );
}

#[tokio::test]
async fn expire_on_unprotected_entry_still_applies() {
    let mgr = mem_manager();
    let now = Local::now();
    let mut e = entry("久未召回的低频条目");
    stamp_recall(&mut e, Some(now - Duration::days(30)), 1);
    let id = mgr.store_entry(e).await.unwrap();

    let res = apply_decision(&mgr, &expire_decision(&id), now)
        .await
        .unwrap();
    assert_eq!(res, "expire");
    let after = mgr.get(&id).await.unwrap().expect("归档非删除");
    assert_eq!(
        after
            .metadata
            .get(crate::dreaming::META_DREAM_ARCHIVED)
            .map(String::as_str),
        Some("true")
    );
}

#[tokio::test]
async fn merge_and_promote_still_allowed_on_protected() {
    let mgr = mem_manager();
    let now = Local::now();
    let mut a = entry("受保护源 A：用户偏好深色主题");
    stamp_recall(&mut a, Some(now - Duration::days(1)), 1);
    let id_a = mgr.store_entry(a).await.unwrap();
    let mut b = entry("受保护源 B：用户偏好深色 UI 主题");
    stamp_recall(&mut b, None, RECALL_PROTECT_MIN_RECALLS);
    let id_b = mgr.store_entry(b).await.unwrap();

    // merge：两条受保护源照常参与（保护的是消失，不是演化）。
    let merged = DreamDecision {
        action: DreamAction::Merge,
        source_ids: vec![id_a.clone(), id_b.clone()],
        reason: "test".to_string(),
    };
    assert_eq!(apply_decision(&mgr, &merged, now).await.unwrap(), "merge");
    assert!(
        mgr.list_all_entries().await.unwrap().len() >= 3,
        "合并产物已建"
    );

    // promote：单条受保护源照常晋升。
    let mut c = entry("受保护源 C");
    stamp_recall(&mut c, Some(now - Duration::hours(1)), 1);
    let id_c = mgr.store_entry(c).await.unwrap();
    let promote = DreamDecision {
        action: DreamAction::Promote,
        source_ids: vec![id_c.clone()],
        reason: "test".to_string(),
    };
    assert_eq!(
        apply_decision(&mgr, &promote, now).await.unwrap(),
        "promote"
    );
    let c_after = mgr.get(&id_c).await.unwrap().unwrap();
    assert_eq!(c_after.typ, MemoryType::LongTerm);
}
