//! P4/E4 合并触发单测（看板项目档案 goal 合并批）。
//!
//! 场景矩阵：非管线不接管 / 在途等待→写回腿收敛 / failed superseded /
//! 真合并 common 文件 / 空集宽容 / E8 基线失配丢弃 / 冲突停车 / estop
//! 挂起→release 补跑 / ingest 登记→触发全链。
//!
//! 隔离：PLACED/MERGED 是模块级 static（跨测试共享同进程）——各测试用
//! 唯一 task_id 前缀防串扰；他人残留的 PLACED 条目在本测试 store 里查无
//! 基线行 → NotArchivePipeline 静默跳过，互不污染。MERGE_DEPS 保持未装
//! （OnceLock 进程级单份，装了会跨测试串 store）——finish_merged_review
//! 的评审 spawn 在单测里自然跳过，断言聚焦 store/仓库/评论状态。

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use nemesis_board::assignment::Actor;
use nemesis_board::models::{IssueStatus, NewIssue, dispatch_state};
use nemesis_cluster::changeset::{
    CHANGESET_VERSION, ChangesetContent, ChangesetManifest, ChangesetUpsert, write_changeset,
};
use nemesis_cluster::cluster::Cluster;
use nemesis_cluster::transfer::sha256_hex;
use nemesis_cluster::types::ClusterConfig;

use super::{
    ACTION_ARCHIVE_SUPERSEDED, ACTION_MERGE_PARKED, ACTION_MERGED, MergeAttempt, ingest_landed,
    merge_and_maybe_review_with, register_placed, retry_merge_for_issue, retry_placed_merges,
};
use crate::board_review::{BoardReviewDeps, SelfcheckRegistry};

// ---------------------------------------------------------------------------

struct Fixture {
    deps: BoardReviewDeps,
    store: Arc<nemesis_board::BoardStore>,
    project_root: PathBuf,
    issue_id: i64,
    base_commit: String,
}

fn fixture(name: &str) -> Fixture {
    let dir = std::env::temp_dir().join(format!("nmb-archive-merge-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = Arc::new(nemesis_board::BoardStore::open(&dir.join("board.db"), "NB").unwrap());
    store.ensure_default_channels().unwrap();

    // 项目档案目录：脚手架 + git 仓库 + 基线文件 common.h（init commit =
    // E2 派发下发的基线）。
    let project_root = dir.join("proj");
    nemesis_board::archive::ensure_scaffold(&project_root, 1, "p", "active").unwrap();
    nemesis_board::git_repo::ensure_repo(&project_root).unwrap();
    std::fs::write(project_root.join("common.h"), b"line0\nline1\n").unwrap();
    let base_commit = nemesis_board::git_repo::commit_worktree(&project_root, "init baseline")
        .unwrap()
        .unwrap();

    let project = store
        .create_project("p", "d", None, "", "", Some(project_root.to_str().unwrap()))
        .unwrap();
    let issue = store
        .create_issue(NewIssue {
            title: format!("merge {name}"),
            description: String::new(),
            priority: 2,
            creator: Actor::agent("node-a"),
            project_id: Some(project.id),
            ..Default::default()
        })
        .unwrap();

    let cluster = Arc::new(Cluster::new(ClusterConfig {
        node_id: "node-a".to_string(),
        bind_address: "127.0.0.1:0".to_string(),
        peers: vec![],
    }));
    let deps = BoardReviewDeps {
        store: store.clone(),
        workspace: dir.clone(),
        home: dir,
        moderator_loop: Arc::new(OnceLock::new()),
        cluster,
        estop: Arc::new(nemesis_agent::estop::EstopState::new()),
        estop_parked: Arc::new(std::sync::Mutex::new(Vec::new())),
        selfcheck: SelfcheckRegistry::new(),
    };
    Fixture {
        deps,
        store,
        project_root,
        issue_id: issue.id,
        base_commit,
    }
}

/// 登记派发（done 终态 + 基线行 + 单据 InProgress——派发核心在生产路径
/// 的产物形态）。
fn setup_done_dispatch(f: &Fixture, task_id: &str) {
    f.store
        .insert_dispatch(task_id, f.issue_id, "node-b", &Actor::agent("node-a"))
        .unwrap();
    f.store
        .set_dispatch_baseline(task_id, &f.base_commit)
        .unwrap();
    f.store
        .transition_issue(f.issue_id, IssueStatus::InProgress, &Actor::agent("node-a"))
        .unwrap();
    f.store
        .finish_dispatch(task_id, dispatch_state::DONE)
        .unwrap();
}

/// 造变更集安置目录（placed_dir/changeset/{changeset.json, files/common.h}）。
fn make_placed_with_changeset(
    dir: &Path,
    task_id: &str,
    base_commit: &str,
    new_content: &[u8],
) -> PathBuf {
    let placed = dir.join(format!("placed-{task_id}"));
    std::fs::create_dir_all(&placed).unwrap();
    std::fs::write(placed.join("log.md"), b"execution log").unwrap();
    let manifest = ChangesetManifest {
        version: CHANGESET_VERSION,
        base_commit: base_commit.to_string(),
        upserts: vec![ChangesetUpsert {
            path: "common.h".to_string(),
            sha256: sha256_hex(new_content),
            size: new_content.len() as u64,
            executable: false,
        }],
        deletions: vec![],
    };
    write_changeset(
        &placed.join("changeset"),
        &manifest,
        &[ChangesetContent {
            path: "common.h".to_string(),
            content: new_content.to_vec(),
            executable: false,
        }],
    )
    .unwrap();
    placed
}

fn actions_of(f: &Fixture) -> Vec<String> {
    f.store
        .list_activity(f.issue_id)
        .unwrap()
        .into_iter()
        .map(|a| a.action)
        .collect()
}

fn comments_of(f: &Fixture) -> Vec<String> {
    f.store
        .list_comments(f.issue_id)
        .unwrap()
        .into_iter()
        .map(|c| c.content)
        .collect()
}

// ---------------------------------------------------------------------------

/// 非档案管线（无基线行）不接管——写回腿走既有 in_review 路径。
#[test]
fn not_archive_pipeline_untouched() {
    let f = fixture("legacy");
    let tid = "t-legacy-1";
    register_placed(tid, &f.project_root);
    assert_eq!(
        merge_and_maybe_review_with(&f.deps, tid),
        MergeAttempt::NotArchivePipeline
    );
    assert_eq!(
        f.store.get_issue(f.issue_id).unwrap().status,
        IssueStatus::Backlog,
        "非管线单据状态不得被合并触发触碰"
    );
}

/// 双路径收敛：落地腿先到（派发在途 → WaitingDispatch，条目保留），写回
/// 腿后到（done → 立即合并）。
#[test]
fn waiting_dispatch_then_writeback_converges() {
    let f = fixture("wait");
    let tid = "t-wait-1";
    f.store
        .insert_dispatch(tid, f.issue_id, "node-b", &Actor::agent("node-a"))
        .unwrap();
    f.store.set_dispatch_baseline(tid, &f.base_commit).unwrap();
    f.store
        .transition_issue(f.issue_id, IssueStatus::InProgress, &Actor::agent("node-a"))
        .unwrap();
    let content = b"line0\nwait-merged\n";
    let placed = make_placed_with_changeset(&f.deps.workspace, tid, &f.base_commit, content);
    register_placed(tid, &placed);

    assert_eq!(
        merge_and_maybe_review_with(&f.deps, tid),
        MergeAttempt::WaitingDispatch,
        "派发在途 = 等写回腿触发，PLACED 条目保留"
    );

    f.store.finish_dispatch(tid, dispatch_state::DONE).unwrap();
    assert_eq!(
        merge_and_maybe_review_with(&f.deps, tid),
        MergeAttempt::Merged
    );
    assert_eq!(
        std::fs::read(f.project_root.join("common.h")).unwrap(),
        content,
        "写回腿触发的合并必须真实落盘"
    );
    assert_eq!(
        f.store.get_issue(f.issue_id).unwrap().status,
        IssueStatus::InReview
    );
}

/// 终态失败派发的变更集按 E8 superseded 丢弃 + 审计（决策流卡在案）。
#[test]
fn failed_dispatch_superseded_discards_changeset() {
    let f = fixture("failed");
    let tid = "t-failed-1";
    f.store
        .insert_dispatch(tid, f.issue_id, "node-b", &Actor::agent("node-a"))
        .unwrap();
    f.store.set_dispatch_baseline(tid, &f.base_commit).unwrap();
    f.store
        .transition_issue(f.issue_id, IssueStatus::InProgress, &Actor::agent("node-a"))
        .unwrap();
    f.store
        .finish_dispatch(tid, dispatch_state::FAILED)
        .unwrap();
    let placed =
        make_placed_with_changeset(&f.deps.workspace, tid, &f.base_commit, b"late content\n");
    register_placed(tid, &placed);

    assert_eq!(
        merge_and_maybe_review_with(&f.deps, tid),
        MergeAttempt::Superseded
    );
    assert!(
        actions_of(&f).contains(&ACTION_ARCHIVE_SUPERSEDED.to_string()),
        "superseded 丢弃必须落决策流审计卡"
    );
    assert_eq!(
        std::fs::read(f.project_root.join("common.h")).unwrap(),
        b"line0\nline1\n",
        "失败派发的变更集绝不入仓库"
    );
    // 重复触发幂等：条目已消费，不再重复出卡。
    assert_eq!(
        merge_and_maybe_review_with(&f.deps, tid),
        MergeAttempt::Superseded
    );
    assert_eq!(
        actions_of(&f)
            .iter()
            .filter(|a| **a == ACTION_ARCHIVE_SUPERSEDED)
            .count(),
        1,
        "superseded 卡只出一次"
    );
}

/// 真合并：基线上的改动落仓库 + 转 in_review + 系统评论 + 审计；重入幂等
/// （MERGED 集合防线——D5 重推/重复触发不再二次合并二次评审）。
#[test]
fn changeset_merges_and_enters_review_idempotent() {
    let f = fixture("merge");
    let tid = "t-merge-1";
    setup_done_dispatch(&f, tid);
    let content = b"line0\nline1-modified\nline2-added\n";
    let placed = make_placed_with_changeset(&f.deps.workspace, tid, &f.base_commit, content);
    register_placed(tid, &placed);

    assert_eq!(
        merge_and_maybe_review_with(&f.deps, tid),
        MergeAttempt::Merged
    );
    assert_eq!(
        std::fs::read(f.project_root.join("common.h")).unwrap(),
        content
    );
    assert_eq!(
        f.store.get_issue(f.issue_id).unwrap().status,
        IssueStatus::InReview
    );
    assert!(comments_of(&f).iter().any(|c| c.contains("🔀")));
    assert!(actions_of(&f).contains(&ACTION_MERGED.to_string()));

    // 幂等重入：同任务档案重推（重登记）→ 直接 Merged，状态不再翻动。
    register_placed(tid, &placed);
    assert_eq!(
        merge_and_maybe_review_with(&f.deps, tid),
        MergeAttempt::Merged
    );
    assert_eq!(
        f.store.get_issue(f.issue_id).unwrap().status,
        IssueStatus::InReview,
        "重入不得二次推动状态"
    );
}

/// 空集宽容：无变更集（非文件型任务）→ 直接进评审，仓库 no-op。
#[test]
fn empty_changeset_tolerated() {
    let f = fixture("empty");
    let tid = "t-empty-1";
    setup_done_dispatch(&f, tid);
    let placed = f.deps.workspace.join("placed-empty");
    std::fs::create_dir_all(&placed).unwrap();
    std::fs::write(placed.join("log.md"), b"log only").unwrap();
    register_placed(tid, &placed);

    assert_eq!(
        merge_and_maybe_review_with(&f.deps, tid),
        MergeAttempt::Merged
    );
    assert_eq!(
        f.store.get_issue(f.issue_id).unwrap().status,
        IssueStatus::InReview
    );
    assert!(
        comments_of(&f).iter().any(|c| c.contains("📦 无变更集")),
        "空集宽容必须有可读系统评论"
    );
    assert_eq!(
        nemesis_board::git_repo::head_commit_hex(&f.project_root)
            .unwrap()
            .unwrap(),
        f.base_commit,
        "空集宽容不产 commit（仓库 no-op）"
    );
}

/// E8 归属校验：变更集申报基线 ≠ 派发基线（旧轮次/换人迟到）→ 丢弃+审计。
#[test]
fn e8_baseline_mismatch_discarded() {
    let f = fixture("e8");
    let tid = "t-e8-1";
    setup_done_dispatch(&f, tid);
    let placed = make_placed_with_changeset(
        &f.deps.workspace,
        tid,
        "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
        b"stale content\n",
    );
    register_placed(tid, &placed);

    assert_eq!(
        merge_and_maybe_review_with(&f.deps, tid),
        MergeAttempt::Superseded
    );
    assert!(
        actions_of(&f).contains(&ACTION_ARCHIVE_SUPERSEDED.to_string()),
        "基线失配必须落 superseded 审计卡"
    );
    assert_eq!(
        std::fs::read(f.project_root.join("common.h")).unwrap(),
        b"line0\nline1\n",
        "失配变更集绝不入仓库"
    );
}

/// 三方合并撞同线冲突 → 停车（决策流卡 + ⚠ 评论），单据**不进** in_review。
#[test]
fn conflict_parks_issue_without_review() {
    let f = fixture("conflict");
    // 任务 1：改 line1 → 合并成功（HEAD 推进）。
    let t1 = "t-conflict-1";
    setup_done_dispatch(&f, t1);
    let p1 = make_placed_with_changeset(
        &f.deps.workspace,
        t1,
        &f.base_commit,
        b"line0\nfrom-task-one\n",
    );
    register_placed(t1, &p1);
    assert_eq!(
        merge_and_maybe_review_with(&f.deps, t1),
        MergeAttempt::Merged
    );

    // 任务 2：同一基线、同一行不同改动 → 真冲突。
    let t2 = "t-conflict-2";
    setup_done_dispatch(&f, t2);
    let p2 = make_placed_with_changeset(
        &f.deps.workspace,
        t2,
        &f.base_commit,
        b"line0\nfrom-task-two\n",
    );
    register_placed(t2, &p2);
    match merge_and_maybe_review_with(&f.deps, t2) {
        MergeAttempt::Parked { reason } => {
            assert!(
                reason.contains("三方合并冲突"),
                "停车原因应含冲突明细: {reason}"
            );
        }
        other => panic!("同行冲突应停车，实得 {other:?}"),
    }
    assert!(
        actions_of(&f).contains(&ACTION_MERGE_PARKED.to_string()),
        "冲突停车必须落决策流卡"
    );
    assert!(
        comments_of(&f).iter().any(|c| c.contains("⚠ 合并停车")),
        "冲突停车必须有 ⚠ 评论"
    );
    assert_eq!(
        f.store.get_issue(f.issue_id).unwrap().status,
        IssueStatus::InProgress,
        "goal E4：合并失败单据不进 in_review"
    );
}

/// estop 挂起：合并被拒 + PLACED 条目保留；release 后 retry_placed_merges
/// 补跑完成合并（护栏三不变：estop 下合并一律拒绝）。
#[test]
fn estop_halts_merge_then_release_retries() {
    let f = fixture("estop");
    let tid = "t-estop-1";
    setup_done_dispatch(&f, tid);
    let content = b"line0\npost-estop\n";
    let placed = make_placed_with_changeset(&f.deps.workspace, tid, &f.base_commit, content);
    register_placed(tid, &placed);

    f.deps.estop.trigger();
    assert_eq!(
        merge_and_maybe_review_with(&f.deps, tid),
        MergeAttempt::EstopHalted,
        "急停中合并必须被拒"
    );
    assert_eq!(
        f.store.get_issue(f.issue_id).unwrap().status,
        IssueStatus::InProgress
    );

    f.deps.estop.release();
    retry_placed_merges(&f.deps);
    assert_eq!(
        f.store.get_issue(f.issue_id).unwrap().status,
        IssueStatus::InReview,
        "release 补跑应完成合并并进评审"
    );
    assert_eq!(
        std::fs::read(f.project_root.join("common.h")).unwrap(),
        content
    );
}

/// ingest 全链：ingest_landed 安置 + 登记 + 触发（落地腿）——MERGE_DEPS 未
/// 装（单测态）时外入口诚实延后，内入口直驱完成合并；收件箱清空。
#[test]
fn ingest_registers_and_merge_converges() {
    let f = fixture("ingest");
    let tid = "t-ingest-1";
    setup_done_dispatch(&f, tid);

    // 造收件箱（transfer.end 落地形态：files/ + manifest.json + landed.json）。
    let inbox = f.deps.workspace.join("inbox").join(tid);
    let files = inbox.join("files");
    std::fs::create_dir_all(&files).unwrap();
    std::fs::write(files.join("log.md"), b"execution log").unwrap();
    let content = b"line0\nvia-ingest\n";
    let manifest = ChangesetManifest {
        version: CHANGESET_VERSION,
        base_commit: f.base_commit.clone(),
        upserts: vec![ChangesetUpsert {
            path: "common.h".to_string(),
            sha256: sha256_hex(content),
            size: content.len() as u64,
            executable: false,
        }],
        deletions: vec![],
    };
    write_changeset(
        &files.join("changeset"),
        &manifest,
        &[ChangesetContent {
            path: "common.h".to_string(),
            content: content.to_vec(),
            executable: false,
        }],
    )
    .unwrap();
    std::fs::write(inbox.join("manifest.json"), b"{}").unwrap();
    std::fs::write(inbox.join("landed.json"), b"{}").unwrap();

    ingest_landed(&f.store, tid, &inbox);
    assert!(!inbox.exists(), "安置成功后收件箱必须清理（传输闭环）");

    assert_eq!(
        merge_and_maybe_review_with(&f.deps, tid),
        MergeAttempt::Merged
    );
    assert_eq!(
        std::fs::read(f.project_root.join("common.h")).unwrap(),
        content
    );
    assert_eq!(
        f.store.get_issue(f.issue_id).unwrap().status,
        IssueStatus::InReview
    );
}

/// F-U6-1 orphan 去重：无处安置的滞留档案每轮 D5 sweep 重灌重试，同
/// (task, reason) 只记首次——重复 ingest 不再重复入账（U6 实测每分钟 13
/// 条刷屏 965 条的根修）。
#[test]
fn orphan_note_deduped_across_sweep_retries() {
    let f = fixture("orphan-dedup");
    // issue 未绑项目（project_id: None）→ ingest 走 orphan_note 路径。
    let issue = f
        .store
        .create_issue(NewIssue {
            title: "orphan dedup".to_string(),
            description: String::new(),
            priority: 2,
            creator: Actor::agent("node-a"),
            project_id: None,
            ..Default::default()
        })
        .unwrap();
    let tid = "t-orphan-dedup-1";
    f.store
        .insert_dispatch(tid, issue.id, "node-b", &Actor::agent("node-a"))
        .unwrap();

    // 收件箱（最小形态：只有 files/，凭据缺失只影响 missing_blocks）。
    let inbox = f.deps.workspace.join("inbox").join(tid);
    std::fs::create_dir_all(inbox.join("files")).unwrap();
    std::fs::write(inbox.join("files").join("log.md"), b"x").unwrap();

    // 模拟 D5 sweep 每轮重灌：同 task 连续 ingest 两次。
    ingest_landed(&f.store, tid, &inbox);
    ingest_landed(&f.store, tid, &inbox);

    let orphans: Vec<_> = f
        .store
        .list_activity(issue.id)
        .unwrap()
        .into_iter()
        .filter(|a| a.action == super::ACTION_ARCHIVE_ORPHANED)
        .collect();
    assert_eq!(orphans.len(), 1, "重复重灌只入账一次（刷屏根修）");
    // 档案仍在收件箱（不删不弃语义不变）。
    assert!(
        inbox.exists(),
        "无处安置的档案留在收件箱（后补绑项目可重灌）"
    );
}

// ---- S-O1 合并停车人工重试（2026-09-16 showcase 复跑实证）----

/// 造真实安置形态的 placement 目录：records/<n>/execution/<ts>/ 根下直接
/// 是 transfer manifest.json（TransferBegin 凭据）+ changeset/ + 执行记录。
/// 与 make_placed_with_changeset 的区别：不加 placed- 子目录层（retry 扫描
/// 的是 execution/<ts>/ 本身）。
fn place_ts_dir(
    project_root: &Path,
    number: &str,
    ts: &str,
    task_id: &str,
    base_commit: &str,
    new_content: &[u8],
) -> PathBuf {
    use nemesis_cluster::transfer::TransferBegin;
    let placed = project_root
        .join("records")
        .join(number)
        .join("execution")
        .join(ts);
    std::fs::create_dir_all(&placed).unwrap();
    std::fs::write(placed.join("log.md"), b"execution log").unwrap();
    let manifest = ChangesetManifest {
        version: CHANGESET_VERSION,
        base_commit: base_commit.to_string(),
        upserts: vec![ChangesetUpsert {
            path: "common.h".to_string(),
            sha256: sha256_hex(new_content),
            size: new_content.len() as u64,
            executable: false,
        }],
        deletions: vec![],
    };
    write_changeset(
        &placed.join("changeset"),
        &manifest,
        &[ChangesetContent {
            path: "common.h".to_string(),
            content: new_content.to_vec(),
            executable: false,
        }],
    )
    .unwrap();
    let begin = TransferBegin {
        transfer_id: format!("t-{task_id}"),
        task_id: task_id.to_string(),
        kind: "execution_records".to_string(),
        source_node: "node-b".to_string(),
        total_bytes: new_content.len() as u64,
        chunk_size: 65536,
        chunk_count: 1,
        files: vec![],
        content_hash: "x".to_string(),
    };
    std::fs::write(
        placed.join("manifest.json"),
        serde_json::to_string_pretty(&begin).unwrap(),
    )
    .unwrap();
    placed
}

/// 模拟重启后形态：placement 目录在档案树里（records/<n>/execution/<ts>/，
/// 内含 transfer manifest.json 凭据 + changeset/），但 PLACED/MERGED 注册表
/// 均为空——retry_merge_for_issue 凭凭据反查 task 重新走合并触发。
#[test]
fn retry_merge_recovers_parked_changeset_after_registry_loss() {
    let f = fixture("s-o1-retry");
    setup_done_dispatch(&f, "task-s01");

    // placement 目录（真实安置形态），不登记 PLACED——模拟重启后注册表清零。
    let new_content = b"line0
retried!
";
    place_ts_dir(
        &f.project_root,
        "NB-1",
        "20260916_010000_000",
        "task-s01",
        &f.base_commit,
        new_content,
    );

    let issue = f.store.get_issue(f.issue_id).unwrap();
    let out = retry_merge_for_issue(&f.deps, &issue).expect("重试必须成功返回");
    let rows = out["retried"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "恰好扫到 1 个 placement: {out}");
    assert!(
        rows[0]["attempt"].as_str().unwrap().contains("Merged"),
        "停车变更集必须经重试合并成功: {out}"
    );
    // 合并真实发生：MERGED 活动 + 单据进 in_review + 内容落盘。
    assert!(actions_of(&f).contains(&ACTION_MERGED.to_string()));
    assert_eq!(
        f.store.get_issue(f.issue_id).unwrap().status,
        IssueStatus::InReview
    );
    assert_eq!(
        std::fs::read(f.project_root.join("common.h")).unwrap(),
        new_content,
        "重试合并必须真实落盘（非空集宽容）"
    );

    // 幂等：再跑一次 → 已合并跳过，不二次合并（活动不再新增 MERGED）。
    let merged_before = actions_of(&f)
        .iter()
        .filter(|a| **a == ACTION_MERGED)
        .count();
    let out2 = retry_merge_for_issue(&f.deps, &issue).unwrap();
    let rows2 = out2["retried"].as_array().unwrap();
    assert!(
        !rows2[0]["skipped"].is_null(),
        "第二次重试必须幂等跳过: {out2}"
    );
    let merged_after = actions_of(&f)
        .iter()
        .filter(|a| **a == ACTION_MERGED)
        .count();
    assert_eq!(merged_before, merged_after, "幂等重试不得二次合并");
}

/// 目录串号防护：档案树里的 placement 凭据 task 绑定别的单据 → 跳过不合并。
#[test]
fn retry_merge_skips_foreign_task_bindings() {
    let f = fixture("s-o1-foreign");
    setup_done_dispatch(&f, "task-mine");

    // 放一个 task_id 指向不存在派发的 placement（串号/残留形态）。
    place_ts_dir(
        &f.project_root,
        "NB-1",
        "20260916_020000_000",
        "task-other",
        &f.base_commit,
        b"data",
    );

    let issue = f.store.get_issue(f.issue_id).unwrap();
    let out = retry_merge_for_issue(&f.deps, &issue).unwrap();
    let rows = out["retried"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{out}");
    assert!(
        !rows[0]["skipped"].is_null(),
        "外来 task 绑定必须跳过: {out}"
    );
    assert!(
        !actions_of(&f).contains(&ACTION_MERGED.to_string()),
        "串号变更集绝不能合并"
    );
}
