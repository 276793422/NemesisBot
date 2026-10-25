//! W4（2026-10-25 加固）：memory_forget 谱系传导
//! 测试——删除源条目时 merge 派生条目连带**归档**（`META_DREAM_ARCHIVED`，
//! 归档非删除）；审批 preview 批准前列出派生明细；拒绝则什么都不动。
//!
//! 独立测试文件（仓库纪律）。fixture 与 dreaming_tests 同款（内存态
//! LocalStore MemoryManager，无 vector）。

use crate::dreaming::{META_DREAM_ARCHIVED, META_DREAM_MERGED_FROM};
use crate::manager::Config;
use crate::manager::MemoryManager;
use crate::memory_tools::{MemoryApprovalGate, MemoryToolExecutor};
use crate::types::{Entry, MemoryType};
use std::sync::Arc;
use tempfile::tempdir;

/// 内存态 MemoryManager（LocalStore，无 vector）。
fn mem_manager() -> MemoryManager {
    let dir = tempdir().expect("tempdir");
    MemoryManager::new(&Config::new(dir.path()))
}

fn entry(content: &str) -> Entry {
    Entry::new(MemoryType::ShortTerm, content.to_string())
}

/// 捕获 preview + 可编程结论的审批闸。
struct MockGate {
    approved: bool,
    previews: parking_lot::Mutex<Vec<String>>,
}

impl MockGate {
    fn approved() -> Self {
        Self {
            approved: true,
            previews: parking_lot::Mutex::new(Vec::new()),
        }
    }
    fn denied() -> Self {
        Self {
            approved: false,
            previews: parking_lot::Mutex::new(Vec::new()),
        }
    }
    fn forget_previews(&self) -> Vec<String> {
        self.previews.lock().clone()
    }
}

#[async_trait::async_trait]
impl MemoryApprovalGate for MockGate {
    async fn approve_store(&self, _preview: &str) -> bool {
        true
    }
    async fn approve_forget(&self, preview: &str) -> bool {
        self.previews.lock().push(preview.to_string());
        self.approved
    }
}

/// 造谱系：源 A + 活派生 B（merged_from=A）+ 已归档派生 C（merged_from=A
/// + archived）。返回 (mgr, id_a, id_b, id_c)。
async fn lineage() -> (MemoryManager, String, String, String) {
    let mgr = mem_manager();
    let id_a = mgr
        .store_entry(entry("源条目 A：用户偏好深色主题"))
        .await
        .unwrap();
    let mut b = entry("派生条目 B：合并自 A 的 UI 偏好");
    b.metadata
        .insert(META_DREAM_MERGED_FROM.to_string(), id_a.clone());
    let id_b = mgr.store_entry(b).await.unwrap();
    let mut c = entry("派生条目 C：早前已归档的合并产物");
    c.metadata
        .insert(META_DREAM_MERGED_FROM.to_string(), id_a.clone());
    c.metadata
        .insert(META_DREAM_ARCHIVED.to_string(), "true".to_string());
    let id_c = mgr.store_entry(c).await.unwrap();
    (mgr, id_a, id_b, id_c)
}

// ---------------------------------------------------------------------------
// manager 层：反查 + 级联归档
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_derived_matches_multi_source_and_skips_archived() {
    let (mgr, id_a, id_b, id_c) = lineage().await;
    let derived = mgr.list_derived_of(&id_a).await.unwrap();
    assert_eq!(
        derived.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
        vec![id_b.as_str()],
        "只列未归档派生；已归档 C 不重复处理"
    );

    // 多源逗号列表：merged_from="other,{id_a},third" 也命中（按段精确匹配）。
    let mut d = entry("派生条目 D：多源合并产物");
    d.metadata.insert(
        META_DREAM_MERGED_FROM.to_string(),
        format!("other-id,{},third", id_a),
    );
    let id_d = mgr.store_entry(d).await.unwrap();
    let derived = mgr.list_derived_of(&id_a).await.unwrap();
    let ids: Vec<_> = derived.iter().map(|e| e.id.as_str()).collect();
    assert!(ids.contains(&id_b.as_str()) && ids.contains(&id_d.as_str()));

    // 前缀陷阱：merged_from="{id_a}x" 不得命中（按段精确匹配）。
    let mut trap = entry("前缀陷阱条目");
    trap.metadata
        .insert(META_DREAM_MERGED_FROM.to_string(), format!("{}x", id_a));
    mgr.store_entry(trap).await.unwrap();
    let derived = mgr.list_derived_of(&id_a).await.unwrap();
    assert_eq!(
        derived.len(),
        2,
        "B+D 命中，前缀陷阱不命中: {:?}",
        derived.iter().map(|e| &e.id).collect::<Vec<_>>()
    );
    let _ = id_c;
}

#[tokio::test]
async fn forget_with_cascade_archives_derived_and_reports() {
    let (mgr, id_a, id_b, _id_c) = lineage().await;
    let report = mgr.forget_with_cascade(&id_a).await.unwrap();
    assert!(report.source_deleted, "源条目已删除");
    assert_eq!(
        report
            .archived_derived
            .iter()
            .map(|(i, _)| i.as_str())
            .collect::<Vec<_>>(),
        vec![id_b.as_str()],
        "恰好连带归档活派生 B"
    );
    assert!(
        report.archived_derived[0].1.contains("UI 偏好"),
        "报告带正文摘要: {:?}",
        report.archived_derived[0]
    );
    // 派生 B：归档盖章在库（update 非 delete——条目可查证）。
    let b_after = mgr.get(&id_b).await.unwrap().expect("B 仍在（归档非删除）");
    assert_eq!(
        b_after
            .metadata
            .get(META_DREAM_ARCHIVED)
            .map(String::as_str),
        Some("true")
    );
    // 源 A：真删除。
    assert!(mgr.get(&id_a).await.unwrap().is_none(), "源条目已删");
}

#[tokio::test]
async fn forget_with_cascade_no_derived_is_plain_forget() {
    let mgr = mem_manager();
    let id = mgr.store_entry(entry("无谱系普通条目")).await.unwrap();
    let report = mgr.forget_with_cascade(&id).await.unwrap();
    assert!(report.source_deleted);
    assert!(report.archived_derived.is_empty(), "无派生 → 报告为空");
    assert!(mgr.get(&id).await.unwrap().is_none());
}

// ---------------------------------------------------------------------------
// 工具层：preview 组装 + 审批拒绝零副作用 + 分诊边界
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tool_forget_general_entry_previews_derived_then_archives() {
    let (mgr, id_a, id_b, id_c) = lineage().await;
    let mgr = Arc::new(mgr);
    let gate = Arc::new(MockGate::approved());
    let exec = MemoryToolExecutor::new(mgr.clone());
    exec.set_approval_gate(gate.clone());

    let res = exec
        .execute(
            "memory_forget",
            &serde_json::json!({"action": "delete_by_id", "id": id_a}),
        )
        .await;
    assert!(
        res.content.contains("deleted successfully"),
        "{:?}",
        res.content
    );
    assert!(
        res.content.contains(&id_b),
        "结果点名连带归档的派生: {:?}",
        res.content
    );

    // preview 批准前列出派生明细（含活派生 B 的 id）。
    let previews = gate.forget_previews();
    assert_eq!(previews.len(), 1, "一次 forget 一次 preview");
    assert!(
        previews[0].contains("一般记忆条目"),
        "分诊注记: {}",
        previews[0]
    );
    assert!(
        previews[0].contains("将连带归档 1 条"),
        "派生计数在 preview: {}",
        previews[0]
    );
    assert!(
        previews[0].contains(&id_b),
        "派生 id 在 preview: {}",
        previews[0]
    );
    assert!(
        !previews[0].contains(&id_c),
        "已归档派生不得再进 preview: {}",
        previews[0]
    );

    // 批准后：源删除 + 派生归档。
    assert!(mgr.get(&id_a).await.unwrap().is_none());
    let b_after = mgr.get(&id_b).await.unwrap().expect("B 仍在");
    assert_eq!(
        b_after
            .metadata
            .get(META_DREAM_ARCHIVED)
            .map(String::as_str),
        Some("true")
    );
}

#[tokio::test]
async fn tool_forget_denial_touches_nothing() {
    let (mgr, id_a, id_b, _id_c) = lineage().await;
    let mgr = Arc::new(mgr);
    let gate = Arc::new(MockGate::denied());
    let exec = MemoryToolExecutor::new(mgr.clone());
    exec.set_approval_gate(gate.clone());

    let res = exec
        .execute(
            "memory_forget",
            &serde_json::json!({"action": "delete_by_id", "id": id_a}),
        )
        .await;
    assert!(res.content.contains("denied"), "{:?}", res.content);
    assert!(!gate.forget_previews().is_empty(), "preview 已组装送审");
    // 拒绝 → 什么都不动：源在、派生未归档。
    assert!(mgr.get(&id_a).await.unwrap().is_some(), "源条目保留");
    let b_after = mgr.get(&id_b).await.unwrap().expect("B 仍在");
    assert!(
        !b_after.metadata.contains_key(META_DREAM_ARCHIVED),
        "派生未被归档"
    );
}

#[tokio::test]
async fn tool_forget_plain_entry_previews_no_derived_note() {
    let mgr = Arc::new(mem_manager());
    let gate = Arc::new(MockGate::approved());
    let exec = MemoryToolExecutor::new(mgr.clone());
    exec.set_approval_gate(gate.clone());

    // 无派生的一般条目：preview 明示「无 merge 派生条目」（行为不变面）。
    let plain_id = mgr.store_entry(entry("无谱系普通条目")).await.unwrap();
    let res = exec
        .execute(
            "memory_forget",
            &serde_json::json!({"action": "delete_by_id", "id": plain_id}),
        )
        .await;
    assert!(
        res.content.contains("deleted successfully"),
        "{:?}",
        res.content
    );
    let previews = gate.forget_previews();
    assert!(
        previews[0].contains("无 merge 派生条目"),
        "无派生注记: {}",
        previews[0]
    );
}

#[tokio::test]
async fn tool_forget_unknown_id_honest_not_found_without_cascade_note() {
    // general miss（也无 episodic）→ 不误入 general 级联注记，
    // delete_by_id 原路径诚实 not-found。
    let mgr = Arc::new(mem_manager());
    let gate = Arc::new(MockGate::approved());
    let exec = MemoryToolExecutor::new(mgr.clone());
    exec.set_approval_gate(gate.clone());

    let res = exec
        .execute(
            "memory_forget",
            &serde_json::json!({"action": "delete_by_id", "id": "no-such-id"}),
        )
        .await;
    assert!(
        res.content.contains("No memory entry found"),
        "未知 id 诚实 not-found: {:?}",
        res.content
    );
    let previews = gate.forget_previews();
    assert!(
        !previews[0].contains("一般记忆条目"),
        "general miss 不得注入级联注记: {}",
        previews[0]
    );
}
