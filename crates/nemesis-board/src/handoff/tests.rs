//! U-A handoff 单元测试（临时 store + 临时目录，零网络零 LLM）。

use super::*;
use crate::assignment::Actor;
use crate::models::NewIssue;
use crate::store::BoardStore;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn temp_dir(name: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "nemesis-board-handofftest-{}-{name}-{n}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn spec() -> HandoffSpec {
    HandoffSpec {
        stop_kind: "fail_streak",
        suggested_action: "manual_check",
        hint: "同因熔断：先修 worker 环境再复评".into(),
        resume_entry: "review.retrigger（number=NB-1）".into(),
        environment: serde_json::json!({ "last_worker": "node-b" }),
        attempted: vec![serde_json::json!({ "action": "redispatch", "result": "第 3 轮 FAIL" })],
    }
}

#[test]
fn test_write_read_roundtrip_preserves_fields() {
    let ws = temp_dir("roundtrip");
    let rec = spec();
    let chain = vec![serde_json::json!({
        "at": 1_700_000_000i64,
        "decision": "redispatch",
        "verdict": "FAIL",
        "details": { "decision": "redispatch", "verdict": "FAIL", "round": 1 },
    })];
    let path = write_handoff(&ws, "NB-7", 7, &rec, chain.clone()).expect("write");
    assert_eq!(path, handoff_path(&ws, "NB-7"));
    assert!(path.file_name().unwrap() == "NB-7.json");

    let doc = read_handoff(&ws, "NB-7").expect("read");
    assert_eq!(doc["schema"], 1);
    assert_eq!(doc["issue"], "NB-7");
    assert_eq!(doc["issue_id"], 7);
    assert_eq!(doc["stop_kind"], "fail_streak");
    assert_eq!(doc["suggested_action"], "manual_check");
    assert_eq!(doc["hint"], "同因熔断：先修 worker 环境再复评");
    assert_eq!(doc["resume_entry"], "review.retrigger（number=NB-1）");
    assert_eq!(doc["environment"]["last_worker"], "node-b");
    assert_eq!(doc["attempted"].as_array().unwrap().len(), 1);
    assert_eq!(doc["verdict_chain"], serde_json::Value::Array(chain));
    assert!(doc["stopped_at"].as_str().unwrap().contains("T"));
    // 覆盖写：二次停车最新态胜出（verdict_chain 由调用方传全程）。
    let rec2 = HandoffSpec {
        stop_kind: "unsure_streak",
        ..spec()
    };
    write_handoff(&ws, "NB-7", 7, &rec2, Vec::new()).expect("overwrite");
    let doc2 = read_handoff(&ws, "NB-7").expect("read2");
    assert_eq!(doc2["stop_kind"], "unsure_streak");
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn test_read_missing_handoff_is_honest_error() {
    let ws = temp_dir("missing");
    assert!(read_handoff(&ws, "NB-404").is_err());
    let _ = std::fs::remove_dir_all(&ws);
}

/// verdict_chain 提取：auto_decide 活动行全程链（决策词/verdict 从
/// details JSON 解出）；非 auto_decide 活动行与评论不混入。
#[test]
fn test_verdict_chain_from_timeline_aggregate() {
    let dir = temp_dir("chain");
    let store = BoardStore::open(&dir.join("board.db"), "NB").expect("open store");
    let issue = store
        .create_issue(NewIssue {
            title: "链".into(),
            creator: Actor::admin("tester"),
            ..NewIssue::default()
        })
        .unwrap();
    store
        .add_activity(
            issue.id,
            &Actor::admin("board"),
            "auto_decide",
            Some(r#"{"decision":"redispatch","verdict":"FAIL","round":1}"#),
        )
        .unwrap();
    store
        .add_activity(
            issue.id,
            &Actor::admin("tester"),
            "status_changed",
            Some("todo→done"),
        )
        .unwrap();
    store
        .add_activity(
            issue.id,
            &Actor::admin("board"),
            "auto_decide",
            Some(r#"{"decision":"escalate_human","verdict":"FAIL","reason":"fail_signature_streak"}"#),
        )
        .unwrap();

    let chain = verdict_chain_from(&store, issue.id);
    assert_eq!(chain.len(), 2, "chain: {chain:?}");
    assert_eq!(chain[0]["decision"], "redispatch");
    assert_eq!(chain[0]["verdict"], "FAIL");
    assert_eq!(chain[1]["decision"], "escalate_human");
    assert_eq!(chain[1]["details"]["reason"], "fail_signature_streak");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_write_failure_on_file_workspace_path() {
    // workspace 指向文件 → create_dir_all 失败 → 诚实 Err（调用方降级）。
    let dir = temp_dir("badws");
    let blocker = dir.join("blocker");
    std::fs::write(&blocker, b"x").unwrap();
    assert!(write_handoff(&blocker, "NB-1", 1, &spec(), Vec::new()).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// 归档生命周期（决策可见性批 2026-10-15）：恢复类动作成功后 archive_handoff
// 把活跃档案移入 archive/ ——「在等人」集合清空、追溯保留。
// ---------------------------------------------------------------------------

#[test]
fn test_archive_moves_file_into_archive_dir() {
    let ws = temp_dir("archive");
    let rec = spec();
    write_handoff(&ws, "NB-9", 9, &rec, Vec::new()).expect("write");
    assert!(handoff_path(&ws, "NB-9").is_file());

    let dest = archive_handoff(&ws, "NB-9")
        .expect("archive")
        .expect("moved");
    let dest_name = dest.file_name().unwrap().to_string_lossy().to_string();
    assert!(
        dest_name.starts_with("NB-9.") && dest_name.ends_with(".json"),
        "归档名带单号前缀与时间戳：{dest_name}"
    );
    assert!(
        dest.starts_with(ws.join("board").join("handoffs").join("archive")),
        "必须落在 archive/ 子目录：{}",
        dest.display()
    );
    // 活跃位消失（issue.list needs_user 徽标熄灭的数据前提）。
    assert!(!handoff_path(&ws, "NB-9").exists());
    // 归档件可读、内容保留（追溯面）。
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&dest).unwrap()).unwrap();
    assert_eq!(doc["issue"], "NB-9");
    assert_eq!(doc["stop_kind"], "fail_streak");
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn test_archive_missing_handoff_is_ok_none() {
    let ws = temp_dir("archive-miss");
    // 无档案（从未停车 / 已归档）：Ok(None) 幂等，best-effort 调用方零分支。
    assert!(matches!(archive_handoff(&ws, "NB-404"), Ok(None)));
    // 二次归档同样幂等。
    write_handoff(&ws, "NB-2", 2, &spec(), Vec::new()).unwrap();
    assert!(archive_handoff(&ws, "NB-2").unwrap().is_some());
    assert!(matches!(archive_handoff(&ws, "NB-2"), Ok(None)));
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn test_archive_never_touches_other_issues_files() {
    let ws = temp_dir("archive-isolation");
    write_handoff(&ws, "NB-3", 3, &spec(), Vec::new()).unwrap();
    write_handoff(&ws, "NB-4", 4, &spec(), Vec::new()).unwrap();
    archive_handoff(&ws, "NB-3").unwrap().unwrap();
    assert!(!handoff_path(&ws, "NB-3").exists());
    assert!(handoff_path(&ws, "NB-4").exists(), "他单档案不受牵连");
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn test_stop_kind_label_covers_catalog_and_falls_back() {
    // 13 个 stop_kind 全目录 + 未知兜底。
    assert_eq!(stop_kind_label("unsure_streak"), "连续不确定");
    assert_eq!(stop_kind_label("fail_streak"), "连续失败熔断");
    assert_eq!(stop_kind_label("budget"), "预算耗尽");
    assert_eq!(stop_kind_label("retry_exhausted"), "重试耗尽");
    assert_eq!(stop_kind_label("worker_loop"), "worker 循环异常");
    assert_eq!(stop_kind_label("capability_fail"), "能力不足");
    assert_eq!(stop_kind_label("infra"), "基础设施故障");
    assert_eq!(stop_kind_label("defect_gate"), "缺陷闸拦截");
    assert_eq!(stop_kind_label("parent_unresolved"), "父单收口未决");
    assert_eq!(stop_kind_label("project_unresolved"), "项目收口未决");
    assert_eq!(stop_kind_label("no_match"), "无匹配节点");
    assert_eq!(stop_kind_label("plan_exhausted"), "拆解重试耗尽");
    assert_eq!(stop_kind_label("llm_unavailable"), "LLM 不可用");
    assert_eq!(stop_kind_label(""), "停车待处理");
    assert_eq!(stop_kind_label("whatever_new"), "停车待处理");
}

/// BUG-20261029-170 红证+回归锁：`stop_kind_label` 词表必须覆盖**生产写入
/// 面**的全部 stop_kind 值——board_review.rs EscalateHuman 臂的 `unsure` /
/// `redispatch_budget`（verdict 诚实分型，注释明示刻意区别于 unsure_streak /
/// budget）与必需检查契约闸的 `required_checks` 此前词表缺席，三类高频
/// 停车（转人工停车场常客）的 needs_user 徽标/详情页全部落兜底「停车待
/// 处理」，语义丢失；模块注释声称的枚举稳定词表（供 Forge Reflect 远期
/// 消费）与生产漂移。本测试钉住生产值域 ⊆ 词表，防再漂移。
#[test]
fn test_stop_kind_label_covers_production_kinds() {
    const PRODUCTION_KINDS: [(&str, &str); 16] = [
        ("unsure_streak", "连续不确定"),
        ("fail_streak", "连续失败熔断"),
        ("budget", "预算耗尽"),
        ("retry_exhausted", "重试耗尽"),
        ("worker_loop", "worker 循环异常"),
        ("capability_fail", "能力不足"),
        ("infra", "基础设施故障"),
        ("defect_gate", "缺陷闸拦截"),
        ("parent_unresolved", "父单收口未决"),
        ("project_unresolved", "项目收口未决"),
        ("no_match", "无匹配节点"),
        ("plan_exhausted", "拆解重试耗尽"),
        ("llm_unavailable", "LLM 不可用"),
        // EscalateHuman 臂 verdict 分型（board_review.rs 停车漏斗）：
        ("unsure", "单轮评审不确定"),
        ("redispatch_budget", "重派预算耗尽"),
        // 必需检查契约闸（2026-10-22 拍板项 B）：
        ("required_checks", "必需检查未满足"),
    ];
    for (kind, label) in PRODUCTION_KINDS {
        assert_eq!(
            stop_kind_label(kind),
            label,
            "stop_kind {kind} 必须有专属标签（不得落兜底）"
        );
    }
    // 真未知值兜底语义保留（防御网不拆）。
    assert_eq!(stop_kind_label("something_new"), "停车待处理");
}
