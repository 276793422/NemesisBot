//! P2/P3（能力扩展 WS3，2026-09-25）：诊断回灌触发臂扩展 + 会话级跨文件
//! 聚合 + stale 过滤（版本感知竞速）测试。
//!
//! fake gopls / PATH 辅助复用 [`super::diagnostics_feedback_tests`]（PATH
//! 是进程全局态，跨模块必须共用同一把 env 锁——否则并行测试互踩，TOCTOU
//! 见同文件 `no_server_on_path_returns_unchanged` 注释）。纯逻辑测试
//! （路径提取 / registry cap 与隔离 / 锚点）不依赖真服务器，确定性覆盖；
//! 真机闭环（rust-analyzer）门禁沿用原测试文件的 `#[ignore]` 用例。

use std::path::PathBuf;
use std::time::Duration;

use super::AgentLoop;
use super::config_watch::{DiagnosticsTouchRegistry, extract_diag_feedback_paths};
use super::diagnostics_feedback_tests::{cfg_enabled, plant_fake_gopls};
use nemesis_config::DiagnosticsLoopConfig;

// ---------------------------------------------------------------------------
// P2：触发臂扩展（append_file / multiedit）
// ---------------------------------------------------------------------------

/// append_file 命中触发臂：ERROR 照常回灌（与 write_file/edit_file 同款
/// touch→wait→回灌路径）。
#[tokio::test]
async fn append_file_triggers_feedback() {
    let (dir, _path) = plant_fake_gopls("error");
    let mgr = nemesis_lsp::LspManager::new(Some(Duration::from_secs(30)), None);
    let go = dir.path().join("main.go");

    let (out, _) = AgentLoop::apply_diagnostics_feedback(
        Some(&mgr),
        cfg_enabled(),
        "append_file",
        &[go.to_str().unwrap()],
        &[],
        "appended",
    )
    .await;

    assert!(out.starts_with("appended"), "原文必须保留: {out}");
    assert!(
        out.contains("[LSP] 1 error(s) detected in"),
        "缺回灌头: {out}"
    );
    let _ = mgr.shutdown_all().await;
}

/// multiedit 命中触发臂（args 形态是 edits[].path，本用例验证工具名闸门）。
#[tokio::test]
async fn multiedit_triggers_feedback() {
    let (dir, _path) = plant_fake_gopls("error");
    let mgr = nemesis_lsp::LspManager::new(Some(Duration::from_secs(30)), None);
    let go = dir.path().join("main.go");

    let (out, _) = AgentLoop::apply_diagnostics_feedback(
        Some(&mgr),
        cfg_enabled(),
        "multiedit",
        &[go.to_str().unwrap()],
        &[],
        "multi applied",
    )
    .await;

    assert!(out.contains("[LSP] 1 error(s) detected in"), "{out}");
    let _ = mgr.shutdown_all().await;
}

// ---------------------------------------------------------------------------
// P2：路径提取（纯函数，确定性）
// ---------------------------------------------------------------------------

/// 单文件工具取顶层 `path`。
#[test]
fn extract_paths_single_file_tool() {
    let args = serde_json::json!({"path": "src/main.rs", "content": "x"});
    assert_eq!(
        extract_diag_feedback_paths("write_file", &args),
        vec!["src/main.rs".to_string()]
    );
    assert_eq!(
        extract_diag_feedback_paths("edit_file", &args),
        vec!["src/main.rs".to_string()]
    );
    assert_eq!(
        extract_diag_feedback_paths("append_file", &args),
        vec!["src/main.rs".to_string()]
    );
}

/// multiedit 取 `edits[].path`，保序去重（同文件多条编辑只登记一次）。
#[test]
fn extract_paths_multiedit_dedups_preserving_order() {
    let args = serde_json::json!({
        "edits": [
            {"path": "b.rs", "old_text": "1", "new_text": "2"},
            {"path": "a.rs", "old_text": "3", "new_text": "4"},
            {"path": "b.rs", "old_text": "5", "new_text": "6"}
        ]
    });
    assert_eq!(
        extract_diag_feedback_paths("multiedit", &args),
        vec!["b.rs".to_string(), "a.rs".to_string()]
    );
}

/// multiedit 畸形 args（缺 edits / 空 / 条目缺 path）→ 空表（诚实跳过）。
#[test]
fn extract_paths_multiedit_malformed_returns_empty() {
    assert!(extract_diag_feedback_paths("multiedit", &serde_json::json!({})).is_empty());
    assert!(extract_diag_feedback_paths("multiedit", &serde_json::json!({"edits": []})).is_empty());
    assert!(
        extract_diag_feedback_paths(
            "multiedit",
            &serde_json::json!({"edits": [{"old_text": "x", "new_text": "y"}]})
        )
        .is_empty()
    );
}

/// 非 multiedit 工具缺顶层 `path` → 空表。
#[test]
fn extract_paths_missing_path_returns_empty() {
    assert!(extract_diag_feedback_paths("write_file", &serde_json::json!({})).is_empty());
    assert!(extract_diag_feedback_paths("write_file", &serde_json::json!({"path": 42})).is_empty());
}

// ---------------------------------------------------------------------------
// P3：跨文件聚合 + stale 过滤
// ---------------------------------------------------------------------------

/// 会话内先写过 B（采集周期登记锚点），再写 A：A 的回灌聚合出 B 的错误
/// （计划验收：写 A 时已 open 的 B 文件错误也出现）。多文件输出格式 =
/// 每条带 path 前缀。
#[tokio::test]
async fn cross_file_aggregation_reports_previously_touched_file() {
    let (dir, _path) = plant_fake_gopls("error");
    let mgr = nemesis_lsp::LspManager::new(Some(Duration::from_secs(30)), None);
    let a = dir.path().join("main.go");
    let b = dir.path().join("second.go");
    std::fs::write(&b, "package main\n\nfunc b() {}\n").unwrap();

    // 模拟「先前写 B」的采集周期：touch + wait → 缓存里有 B 的 ERROR。
    mgr.touch_file(&b).await.unwrap();
    mgr.wait_for_diagnostics(&b, 150, 5_000).await;
    let b_anchor = std::fs::metadata(&b).unwrap().modified().unwrap();
    let prev = vec![(b.clone(), b_anchor)];

    let (out, anchors) = AgentLoop::apply_diagnostics_feedback(
        Some(&mgr),
        cfg_enabled(),
        "edit_file",
        &[a.to_str().unwrap()],
        &prev,
        "r",
    )
    .await;

    assert!(
        out.contains("[LSP] 2 error(s) detected in 2 files, please fix:"),
        "应为跨文件聚合头: {out}"
    );
    assert!(out.contains("main.go:L3:5"), "编辑文件错误在列: {out}");
    assert!(out.contains("second.go:L3:5"), "已登记文件错误在列: {out}");
    // 锚点只登记本轮编辑集（B 未重采集，原锚点保持）。
    assert_eq!(anchors.len(), 1);
    assert_eq!(anchors[0].0, a);
    let _ = mgr.shutdown_all().await;
}

/// stale 丢弃：B 登记后被外部改写（exec / 编辑器，不经本回灌管线），
/// 其缓存诊断早于最后一次写入 → 不回灌（宁缺勿假），只剩 A 的错误。
#[tokio::test]
async fn stale_diag_of_externally_modified_file_dropped() {
    let (dir, _path) = plant_fake_gopls("error");
    let mgr = nemesis_lsp::LspManager::new(Some(Duration::from_secs(30)), None);
    let a = dir.path().join("main.go");
    let b = dir.path().join("second.go");
    std::fs::write(&b, "package main\n\nfunc b() {}\n").unwrap();

    mgr.touch_file(&b).await.unwrap();
    mgr.wait_for_diagnostics(&b, 150, 5_000).await;
    let b_anchor = std::fs::metadata(&b).unwrap().modified().unwrap();

    // B 被外部改写（mtime 变化），服务器缓存里的诊断成为 stale。
    std::thread::sleep(std::time::Duration::from_millis(30));
    std::fs::write(&b, "package main\n\nfunc b_fixed() {}\n").unwrap();

    let (out, anchors) = AgentLoop::apply_diagnostics_feedback(
        Some(&mgr),
        cfg_enabled(),
        "edit_file",
        &[a.to_str().unwrap()],
        &[(b.clone(), b_anchor)],
        "r",
    )
    .await;

    assert!(
        out.contains("[LSP] 1 error(s) detected in"),
        "stale 文档应被丢弃，只回灌编辑文件: {out}"
    );
    assert!(!out.contains("second.go"), "stale 文档错误不得出现: {out}");
    assert_eq!(anchors.len(), 1);
    assert_eq!(anchors[0].0, a);
    let _ = mgr.shutdown_all().await;
}

/// 聚合 cap：跨文件总量受 max_errors 约束，编辑文件优先占预算（其余文件
/// 预算耗尽后整文件退出列表）。
#[tokio::test]
async fn aggregate_cap_edited_files_take_priority() {
    let (dir, _path) = plant_fake_gopls("flood");
    let mgr = nemesis_lsp::LspManager::new(Some(Duration::from_secs(30)), None);
    let a = dir.path().join("main.go");
    let b = dir.path().join("second.go");
    std::fs::write(&b, "package main\n\nfunc b() {}\n").unwrap();

    mgr.touch_file(&b).await.unwrap();
    mgr.wait_for_diagnostics(&b, 150, 5_000).await;
    let b_anchor = std::fs::metadata(&b).unwrap().modified().unwrap();

    let (out, _) = AgentLoop::apply_diagnostics_feedback(
        Some(&mgr),
        cfg_enabled(), // max_errors = 20
        "edit_file",
        &[a.to_str().unwrap()],
        &[(b, b_anchor)],
        "r",
    )
    .await;

    // 编辑文件 A（25 条）先占满 20 预算 → B（余 0）整文件退出 → 单文件格式。
    assert_eq!(out.matches("\n- ").count(), 20, "总量截到 20: {out}");
    assert!(
        out.contains("[LSP] 20 error(s) detected in"),
        "头部计数 = cap: {out}"
    );
    assert!(!out.contains("second.go"), "预算耗尽文件不出现: {out}");
    let _ = mgr.shutdown_all().await;
}

/// multiedit 多文件聚合：同轮编辑的两个文件错误都进列表（多文件格式）。
#[tokio::test]
async fn multiedit_multiple_files_aggregated() {
    let (dir, _path) = plant_fake_gopls("error");
    let mgr = nemesis_lsp::LspManager::new(Some(Duration::from_secs(30)), None);
    let a = dir.path().join("main.go");
    let b = dir.path().join("second.go");
    std::fs::write(&b, "package main\n\nfunc b() {}\n").unwrap();

    let (out, anchors) = AgentLoop::apply_diagnostics_feedback(
        Some(&mgr),
        cfg_enabled(),
        "multiedit",
        &[a.to_str().unwrap(), b.to_str().unwrap()],
        &[],
        "r",
    )
    .await;

    assert!(out.contains("2 error(s) detected in 2 files"), "{out}");
    assert!(out.contains("main.go:L3:5"), "{out}");
    assert!(out.contains("second.go:L3:5"), "{out}");
    assert_eq!(anchors.len(), 2, "两个编辑文件都登记锚点");
    let _ = mgr.shutdown_all().await;
}

/// 干净编辑（无 ERROR）也产生采集锚点——后续别的文件写入触发聚合时，
/// 该文档才有据可查（锚点 = 采集时 mtime）。
#[tokio::test]
async fn clean_edit_still_records_anchor() {
    let (dir, _path) = plant_fake_gopls("warn");
    let mgr = nemesis_lsp::LspManager::new(Some(Duration::from_secs(30)), None);
    let a = dir.path().join("main.go");

    let (out, anchors) = AgentLoop::apply_diagnostics_feedback(
        Some(&mgr),
        cfg_enabled(),
        "write_file",
        &[a.to_str().unwrap()],
        &[],
        "written ok",
    )
    .await;

    assert_eq!(out, "written ok", "无 ERROR 原样返回");
    assert_eq!(anchors.len(), 1, "干净文档仍登记锚点");
    assert_eq!(anchors[0].0, a);
    assert_eq!(
        anchors[0].1,
        std::fs::metadata(&a).unwrap().modified().unwrap(),
        "锚点 = 当前 mtime（文件未被改动，二者一致）"
    );
    let _ = mgr.shutdown_all().await;
}

// ---------------------------------------------------------------------------
// P3：DiagnosticsTouchRegistry 纯逻辑（会话隔离 / cap / upsert 移尾）
// ---------------------------------------------------------------------------

fn anchor(name: &str, offset_ms: u64) -> (PathBuf, std::time::SystemTime) {
    (
        PathBuf::from(name),
        std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(offset_ms),
    )
}

/// 会话隔离：s1 登记的文件对 s2 不可见（防跨会话/跨请求泄漏）。
#[test]
fn registry_isolates_sessions() {
    let mut reg = DiagnosticsTouchRegistry::default();
    reg.record_anchors("s1", &[anchor("a.go", 1)]);
    assert_eq!(reg.snapshot_session("s1").len(), 1);
    assert!(reg.snapshot_session("s2").is_empty(), "跨会话不可见");
    assert!(reg.snapshot_session("").is_empty());
}

/// 单会话文件 cap（64）：超限淘汰最旧登记。
#[test]
fn registry_caps_files_per_session_evicting_oldest() {
    let mut reg = DiagnosticsTouchRegistry::default();
    let keys: Vec<_> = (0..70)
        .map(|i| anchor(&format!("f{i:03}.go"), i as u64))
        .collect();
    reg.record_anchors("s", &keys);
    let snap = reg.snapshot_session("s");
    assert_eq!(snap.len(), 64, "cap 到 64");
    assert_eq!(snap[0].0, PathBuf::from("f006.go"), "最旧 6 条被淘汰");
    assert_eq!(snap[63].0, PathBuf::from("f069.go"), "最新保留");
}

/// upsert：重登记的文件刷新锚点并移到尾部（最近采集序）。
#[test]
fn registry_upsert_refreshes_and_moves_to_tail() {
    let mut reg = DiagnosticsTouchRegistry::default();
    reg.record_anchors("s", &[anchor("a.go", 1), anchor("b.go", 2)]);
    reg.record_anchors("s", &[anchor("c.go", 3)]);
    reg.record_anchors("s", &[anchor("a.go", 9)]);
    let snap = reg.snapshot_session("s");
    let names: Vec<_> = snap.iter().map(|(p, _)| p.display().to_string()).collect();
    assert_eq!(names, vec!["b.go", "c.go", "a.go"], "a.go 移到尾部");
    assert_eq!(snap[2].1, anchor("a.go", 9).1, "锚点刷新");
}

/// 会话数 cap（16）：新会话加入时淘汰旧会话条目，总数不超限。
#[test]
fn registry_caps_session_count() {
    let mut reg = DiagnosticsTouchRegistry::default();
    for i in 0..16 {
        reg.record_anchors(&format!("s{i}"), &[anchor("a.go", i as u64)]);
    }
    reg.record_anchors("s16", &[anchor("a.go", 99)]);
    assert!(reg.snapshot_session("s16").len() == 1, "新会话必在");
    let total: usize = (0..17)
        .map(|i| reg.snapshot_session(&format!("s{i}")).len())
        .sum();
    assert_eq!(total, 16, "总会话条目数收到 cap 内");
}

/// 空锚点回写是 no-op（不建会话条目）。
#[test]
fn registry_empty_anchor_write_is_noop() {
    let mut reg = DiagnosticsTouchRegistry::default();
    reg.record_anchors("s", &[]);
    assert!(reg.snapshot_session("s").is_empty());
}

/// max_errors=0 的诚实边界：聚合全空 → 原样返回（等价关闭回灌但锚点
/// 照常登记——采集本身发生过）。
#[tokio::test]
async fn zero_budget_returns_unchanged_but_records_anchor() {
    let (dir, _path) = plant_fake_gopls("error");
    let mgr = nemesis_lsp::LspManager::new(Some(Duration::from_secs(30)), None);
    let a = dir.path().join("main.go");
    let cfg = DiagnosticsLoopConfig {
        enabled: true,
        max_errors: 0,
        wait_max_ms: 5_000,
    };

    let (out, anchors) = AgentLoop::apply_diagnostics_feedback(
        Some(&mgr),
        cfg,
        "edit_file",
        &[a.to_str().unwrap()],
        &[],
        "quiet",
    )
    .await;

    assert_eq!(out, "quiet");
    assert_eq!(anchors.len(), 1, "采集发生过，锚点照常登记");
    let _ = mgr.shutdown_all().await;
}
