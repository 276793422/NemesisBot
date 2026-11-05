//! 本地派发交付捕获回归锁（BUG 2026-11-05 根修）。
//!
//! - `pin_local_baseline`：真 git 夹具返 Some（HEAD commit）；非 git 目录 /
//!   文件形态 .git（worktree 链接）/ 空仓（无 HEAD commit）三臂 None。
//! - `capture_and_record_local_delivery`：真 TaskDb + 真 git 夹具（基线后
//!   改文件）→ true + `delivery_by_key("{task_id}:branch")` 命中 +
//!   payload `repo_base_sha` == 钉住基线（agent 合法 commit 移动 HEAD 后
//!   HEAD 当基线会失真的 T1 同族根修）+ patch 非空 + tree 在场；非 git
//!   目录 → false（capture 缺席诚实上报）。

use super::*;
use std::path::Path;

/// 临时 git 仓库 + 首个 commit（unborn HEAD → 有 HEAD），返回 HEAD sha。
fn git_repo_with_seed_commit(dir: &Path) -> String {
    git2::Repository::init(dir).unwrap();
    std::fs::write(dir.join("seed.txt"), "seed\n").unwrap();
    let repo = git2::Repository::open(dir).unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(Path::new("seed.txt")).unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let sig = git2::Signature::now("t", "t@t").unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "seed", &tree, &[])
        .unwrap()
        .to_string()
}

#[test]
fn pin_baseline_happy_returns_head_commit() {
    let dir = tempfile::tempdir().unwrap();
    let sha = git_repo_with_seed_commit(dir.path());
    let b = pin_local_baseline(dir.path(), "loc-pin-1").expect("git 仓库应钉到 HEAD 基线");
    assert_eq!(b.baseline_commit, sha);
    assert_eq!(b.repo_dir, dir.path().to_path_buf());
}

#[test]
fn pin_baseline_non_git_dir_is_none() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        pin_local_baseline(dir.path(), "loc-pin-2").is_none(),
        "非 git 目录 → None（交付捕获缺席可观测）"
    );
}

#[test]
fn pin_baseline_file_form_dot_git_is_none() {
    let dir = tempfile::tempdir().unwrap();
    // 文件形态 .git（worktree 链接指针，不是目录）——镜像 ensure_exec_repo
    // 对文件形态 .git 的裁决：不打开（open 沿 gitdir 指针会落到别处）。
    std::fs::write(dir.path().join(".git"), "gitdir: elsewhere.git\n").unwrap();
    assert!(pin_local_baseline(dir.path(), "loc-pin-3").is_none());
}

#[test]
fn pin_baseline_unborn_head_is_none() {
    let dir = tempfile::tempdir().unwrap();
    git2::Repository::init(dir.path()).unwrap();
    assert!(
        pin_local_baseline(dir.path(), "loc-pin-4").is_none(),
        "空仓（无 HEAD commit）→ None"
    );
}

#[test]
fn capture_records_branch_delivery_with_pinned_baseline() {
    let proj = tempfile::tempdir().unwrap();
    let baseline = git_repo_with_seed_commit(proj.path());

    // agent 干活：改旧文件 + 增新文件（未 commit——工作区改动也在捕获面）。
    std::fs::write(proj.path().join("seed.txt"), "fixed\n").unwrap();
    std::fs::write(proj.path().join("extra.txt"), "new file\n").unwrap();

    let ws = tempfile::tempdir().unwrap();
    let db = nemesis_taskstore::TaskDb::open(&nemesis_path::resolve_tasks_db_path_in_workspace(
        ws.path(),
    ))
    .expect("临时 workspace TaskDb 打开");

    let ok = capture_and_record_local_delivery(
        Some(&db),
        proj.path(),
        ws.path(),
        "loc-cap-1",
        "node-self",
        &baseline,
        #[cfg(feature = "security")]
        None,
    );
    assert!(ok, "捕获 + 落账应成功");

    let record = db
        .delivery_by_key("loc-cap-1:branch")
        .expect("账本查询")
        .expect("deliveries 表应有 {task_id}:branch 键（publish 查账键）");
    assert_eq!(record.kind, "branch");
    assert_eq!(record.run_id.as_deref(), Some("loc-cap-1"));

    let payload = &record.payload;
    assert_eq!(
        payload.get("repo_base_sha").and_then(|v| v.as_str()),
        Some(baseline.as_str()),
        "版本链基线 = 发车前钉住的 HEAD（不是捕获时刻 HEAD）"
    );
    assert!(
        payload
            .get("patch")
            .and_then(|v| v.as_str())
            .is_some_and(|p| !p.trim().is_empty()),
        "patch 非空（改动被 diff 进交付）"
    );
    assert!(
        payload
            .get("tree")
            .and_then(|v| v.as_str())
            .is_some_and(|t| !t.is_empty()),
        "交付 tree 在场（already_merged 判定的比对面）"
    );
    assert_eq!(
        payload.get("branch").and_then(|v| v.as_str()),
        Some("run-loc-cap-1")
    );
}

#[test]
fn capture_on_non_git_dir_is_false() {
    let proj = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let db = nemesis_taskstore::TaskDb::open(&nemesis_path::resolve_tasks_db_path_in_workspace(
        ws.path(),
    ))
    .unwrap();
    let ok = capture_and_record_local_delivery(
        Some(&db),
        proj.path(),
        ws.path(),
        "loc-cap-2",
        "node-self",
        &"0".repeat(40),
        #[cfg(feature = "security")]
        None,
    );
    assert!(
        !ok,
        "非 git 目录 capture 缺席 → false（publish 诚实报无可发布物）"
    );
    assert!(db.delivery_by_key("loc-cap-2:branch").unwrap().is_none());
}

/// Fix 1 回归锁（NB-1 占位回复误判）：busy 排队回执（receipt=true）不得
/// 被当终局回复捕获——会话忙时首条出站是 ⏳ 排队回执 + 真回复随后，
/// capture_reply 必须跳过回执等到真回复。NB-1 实录：queue-ack 被当终局
/// → 57ms 空交付 completed，真回复随后无人认领成孤儿。
#[tokio::test]
async fn capture_reply_skips_receipt_and_returns_final() {
    let (tx, mut rx) = tokio::sync::broadcast::channel(8);
    let session_key = "board_dispatch:t-receipt";
    let queued_ack = OutboundMessage {
        channel: "web".to_string(),
        chat_id: "board".to_string(),
        content: "⏳ 当前正在处理上一条消息。你的消息已排队，将在本轮结束后继续处理。".to_string(),
        message_type: String::new(),
        meta: nemesis_types::channel::OutboundMeta {
            session_key: Some(session_key.to_string()),
            receipt: true,
            ..Default::default()
        },
    };
    let final_reply = OutboundMessage {
        channel: "web".to_string(),
        chat_id: "board".to_string(),
        content: "修复完成：add() 已恢复加法 (DEMO-2 / NB-1)".to_string(),
        message_type: String::new(),
        meta: nemesis_types::channel::OutboundMeta {
            session_key: Some(session_key.to_string()),
            receipt: false,
            ..Default::default()
        },
    };
    tx.send(queued_ack).unwrap();
    tx.send(final_reply).unwrap();
    let got = capture_reply(&mut rx, session_key).await;
    assert_eq!(
        got.as_deref(),
        Some("修复完成：add() 已恢复加法 (DEMO-2 / NB-1)"),
        "排队回执必须被跳过，捕获到的必须是终局回复"
    );
}

/// 反证：无回执标记（receipt=false，旧出站形态）时首条 session_key 匹配
/// 即返回——正常终局捕获语义不变。
#[tokio::test]
async fn capture_reply_first_final_match_returns_immediately() {
    let (tx, mut rx) = tokio::sync::broadcast::channel(8);
    let session_key = "board_dispatch:t-final";
    let reply = OutboundMessage {
        channel: "web".to_string(),
        chat_id: "board".to_string(),
        content: "终局回复".to_string(),
        message_type: String::new(),
        meta: nemesis_types::channel::OutboundMeta {
            session_key: Some(session_key.to_string()),
            receipt: false,
            ..Default::default()
        },
    };
    tx.send(reply).unwrap();
    let got = capture_reply(&mut rx, session_key).await;
    assert_eq!(got.as_deref(), Some("终局回复"));
}
