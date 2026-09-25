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
        node_name: String::new(),
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

// ===========================================================================
// Coverage 追加（2026-09-24）：ingest_landed 守卫臂与收件凭据、
// note_overlimit 出卡与 Done 回滚、note_missing_block、sweep_missing_archives
// 双腿（fake transport 注入）、ghost 基线、冻结期交付登记（FrozenDeferred）、
// 停车臂（目录缺失/仓库 ensure 失败/变更集损坏）、冲突明细渲染、retry 守卫
// 臂、replay_pending_merges 全矩阵 + 再冻结。
// 纪律：MERGE_DEPS / install_merge_deps 是进程级 OnceLock——本套件任何测试
// 都不安装（装了会跨测试串 store，见文件头注释）；外入口
// merge_and_maybe_review 与 install_merge_deps 两行保持未覆盖（豁免项）。
// ===========================================================================

use std::collections::VecDeque;

use nemesis_board::git_repo::ConflictFile;
use nemesis_cluster::outbox::TransferTransport;
use nemesis_cluster::transfer::TransferOverlimit;

use super::{
    ACTION_ARCHIVE_ORPHANED, ACTION_ARCHIVE_OVERLIMIT, ACTION_MERGE_DEFERRED, finish_merged_review,
    note_missing_block, note_overlimit, render_conflict_detail, replay_pending_merges,
    sweep_missing_archives,
};

/// 脚本化传输（sweep_missing_archives 腿 2 的 pull 注入）：按序弹出回复，
/// 耗尽回落 fallback；记录 (peer, action) 调用流水。
struct FakeTransport {
    replies: std::sync::Mutex<VecDeque<Result<serde_json::Value, String>>>,
    fallback: Result<serde_json::Value, String>,
    calls: std::sync::Mutex<Vec<(String, String)>>,
}

impl FakeTransport {
    fn new(replies: Vec<Result<serde_json::Value, String>>) -> Self {
        Self {
            replies: std::sync::Mutex::new(replies.into_iter().collect()),
            fallback: Ok(serde_json::json!({"status": "queued"})),
            calls: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}

#[async_trait::async_trait]
impl TransferTransport for FakeTransport {
    async fn call(
        &self,
        peer: &str,
        action: &str,
        _payload: serde_json::Value,
        _timeout: std::time::Duration,
    ) -> Result<serde_json::Value, String> {
        self.calls
            .lock()
            .unwrap()
            .push((peer.to_string(), action.to_string()));
        match self.replies.lock().unwrap().pop_front() {
            Some(r) => r,
            None => self.fallback.clone(),
        }
    }
}

fn overlimit_req(task_id: &str) -> TransferOverlimit {
    TransferOverlimit {
        task_id: task_id.to_string(),
        source_node: "node-b".to_string(),
        total_bytes: 99,
        limit: 9,
        kind: "execution_records".to_string(),
    }
}

/// 最小收件箱（files/ 子目录 + 可选凭据）。
fn make_inbox(dir: &Path, name: &str, with_manifest: bool, with_landed: bool) -> PathBuf {
    let inbox = dir.join("inbox").join(name);
    std::fs::create_dir_all(inbox.join("files")).unwrap();
    std::fs::write(inbox.join("files").join("log.md"), b"x").unwrap();
    if with_manifest {
        std::fs::write(inbox.join("manifest.json"), b"{}").unwrap();
    }
    if with_landed {
        std::fs::write(inbox.join("landed.json"), b"{}").unwrap();
    }
    inbox
}

/// ingest_landed 守卫矩阵：无派发绑定 / issue 未绑项目 / 项目无目录 /
/// 脚手架缺失；凭据缺失变体逐一记入 missing_blocks（happy 安置继续）。
#[test]
fn sweep_ingest_landed_guards_and_missing_blocks() {
    let f = fixture("ing-guard");
    let issue_number = f.store.get_issue(f.issue_id).unwrap().number;

    // (a) 无派发绑定 → None，收件箱原样保留。
    let inbox_a = make_inbox(&f.deps.workspace, "t-ga", true, true);
    assert!(ingest_landed(&f.store, "t-ga", &inbox_a).is_none());
    assert!(inbox_a.exists(), "无绑定的档案必须留在收件箱");

    // (b) issue 未绑项目 → orphan 留痕。
    let issue_b = f
        .store
        .create_issue(NewIssue {
            title: "no project".into(),
            creator: Actor::agent("node-a"),
            ..Default::default()
        })
        .unwrap();
    f.store
        .insert_dispatch("t-gb", issue_b.id, "node-b", &Actor::agent("node-a"))
        .unwrap();
    let inbox_b = make_inbox(&f.deps.workspace, "t-gb", true, true);
    assert!(ingest_landed(&f.store, "t-gb", &inbox_b).is_none());
    assert!(
        f.store
            .list_activity(issue_b.id)
            .unwrap()
            .iter()
            .any(|a| a.action == ACTION_ARCHIVE_ORPHANED)
    );

    // (c) 项目无档案目录（存量项目）→ 静默跳过。
    let project_c = f
        .store
        .create_project("p-guard", "d", None, "", "", None)
        .unwrap();
    let issue_c = f
        .store
        .create_issue(NewIssue {
            title: "no dir".into(),
            creator: Actor::agent("node-a"),
            project_id: Some(project_c.id),
            ..Default::default()
        })
        .unwrap();
    f.store
        .insert_dispatch("t-gc", issue_c.id, "node-b", &Actor::agent("node-a"))
        .unwrap();
    let inbox_c = make_inbox(&f.deps.workspace, "t-gc", true, true);
    assert!(ingest_landed(&f.store, "t-gc", &inbox_c).is_none());

    // (d)(e)(f) 凭据缺失变体（主项目 fixture：安置继续 + missing_blocks 记账）。
    f.store
        .insert_dispatch("t-gd", f.issue_id, "node-b", &Actor::agent("node-a"))
        .unwrap();
    f.store
        .insert_dispatch("t-ge", f.issue_id, "node-b", &Actor::agent("node-a"))
        .unwrap();
    f.store
        .insert_dispatch("t-gf", f.issue_id, "node-b", &Actor::agent("node-a"))
        .unwrap();

    // e: 只缺 manifest → "manifest"。
    let inbox_e = make_inbox(&f.deps.workspace, "t-ge", false, true);
    let target_e = ingest_landed(&f.store, "t-ge", &inbox_e);
    assert!(target_e.is_some(), "凭据缺失不阻断安置");

    // f: 两者都缺 → "manifest+landed"。
    let inbox_f = make_inbox(&f.deps.workspace, "t-gf", false, false);
    assert!(ingest_landed(&f.store, "t-gf", &inbox_f).is_some());

    let blocks = nemesis_board::archive::read_manifest(&f.project_root)
        .unwrap()
        .missing_blocks;
    assert!(
        blocks
            .iter()
            .any(|b| b == &format!("{issue_number}:manifest")),
        "blocks: {blocks:?}"
    );
    assert!(
        blocks
            .iter()
            .any(|b| b == &format!("{issue_number}:manifest+landed")),
        "blocks: {blocks:?}"
    );
    assert!(!inbox_e.exists() && !inbox_f.exists(), "安置成功清收件箱");

    // g: 只缺 landed → "landed"。
    let inbox_g = make_inbox(&f.deps.workspace, "t-gg", true, false);
    f.store
        .insert_dispatch("t-gg", f.issue_id, "node-b", &Actor::agent("node-a"))
        .unwrap();
    assert!(ingest_landed(&f.store, "t-gg", &inbox_g).is_some());
    let blocks = nemesis_board::archive::read_manifest(&f.project_root)
        .unwrap()
        .missing_blocks;
    assert!(
        blocks
            .iter()
            .any(|b| b == &format!("{issue_number}:landed"))
    );

    // (h) 脚手架缺失（project.json 被手删）→ orphan 留痕。
    std::fs::remove_file(f.project_root.join("project.json")).unwrap();
    let inbox_h = make_inbox(&f.deps.workspace, "t-gh", true, true);
    f.store
        .insert_dispatch("t-gh", f.issue_id, "node-b", &Actor::agent("node-a"))
        .unwrap();
    assert!(ingest_landed(&f.store, "t-gh", &inbox_h).is_none());
    assert!(inbox_h.exists(), "无处安置的档案留在收件箱");
}

/// note_missing_block：project.json 不可读 = WARN 让位；可读 = 去重追加。
#[test]
fn sweep_note_missing_block_dedup_and_unreadable() {
    let f = fixture("missing-block");
    note_missing_block(&f.project_root, "NB-1:block-a");
    note_missing_block(&f.project_root, "NB-1:block-a"); // 去重
    note_missing_block(&f.project_root, "NB-1:block-b");
    let blocks = nemesis_board::archive::read_manifest(&f.project_root)
        .unwrap()
        .missing_blocks;
    assert_eq!(
        blocks.iter().filter(|b| **b == "NB-1:block-a").count(),
        1,
        "同 block 只记一次: {blocks:?}"
    );
    assert!(blocks.contains(&"NB-1:block-b".to_string()));

    // 无 project.json（不可读）→ WARN 不落不炸。
    let bare = f.deps.workspace.join("bare-root");
    std::fs::create_dir_all(&bare).unwrap();
    note_missing_block(&bare, "NB-1:x");
}

/// note_overlimit：无绑定仅 WARN；未收货出三卡不回滚；Done 单强制回滚
/// in_review（终态 bypass，T-XFER-5 语义）。
#[test]
fn sweep_note_overlimit_cards_and_done_rollback() {
    let f = fixture("overlimit");

    // (a) 无派发绑定 → 不炸不出卡。
    note_overlimit(&f.store, &overlimit_req("t-ol-none"));

    // (b) 在途派发：决策流卡 + ⛔ 评论；状态不动。
    f.store
        .insert_dispatch("t-ol-1", f.issue_id, "node-b", &Actor::agent("node-a"))
        .unwrap();
    f.store
        .transition_issue(f.issue_id, IssueStatus::InProgress, &Actor::agent("node-a"))
        .unwrap();
    note_overlimit(&f.store, &overlimit_req("t-ol-1"));
    assert!(
        actions_of(&f).contains(&ACTION_ARCHIVE_OVERLIMIT.to_string()),
        "超限必须落决策流卡"
    );
    assert!(
        comments_of(&f)
            .iter()
            .any(|c| c.contains("⛔") && c.contains("99 字节超护栏 9 字节"))
    );

    // (c) Done 单：出卡 + 终态回滚 in_review（评审转人工）。
    f.store
        .transition_issue(f.issue_id, IssueStatus::InReview, &Actor::agent("node-a"))
        .unwrap();
    f.store
        .transition_issue(f.issue_id, IssueStatus::Done, &Actor::agent("node-a"))
        .unwrap();
    f.store
        .insert_dispatch("t-ol-2", f.issue_id, "node-b", &Actor::agent("node-a"))
        .unwrap();
    note_overlimit(&f.store, &overlimit_req("t-ol-2"));
    assert_eq!(
        f.store.get_issue(f.issue_id).unwrap().status,
        IssueStatus::InReview,
        "超限必须把已收货单回滚 in_review"
    );
}

/// ghost 基线（有基线行无派发行的数据异常形态）→ NotArchivePipeline 不接管。
#[test]
fn sweep_ghost_baseline_without_dispatch_row() {
    let f = fixture("ghost-baseline");
    f.store
        .set_dispatch_baseline("t-ghost-bl", &f.base_commit)
        .unwrap();
    assert_eq!(
        merge_and_maybe_review_with(&f.deps, "t-ghost-bl"),
        MergeAttempt::NotArchivePipeline
    );
}

/// P5/F3 冻结期交付：变更集不合入，登记待补合并队列 + ⏸ 评论 + 状态不动。
#[test]
fn sweep_frozen_project_defers_changeset() {
    let f = fixture("frozen-defer");
    let pid = f.store.get_issue(f.issue_id).unwrap().project_id.unwrap();
    f.store.set_project_conflict_frozen(pid, true).unwrap();
    setup_done_dispatch(&f, "t-frz-1");
    let content = b"line0\nfrozen-delivery\n";
    let placed = make_placed_with_changeset(&f.deps.workspace, "t-frz-1", &f.base_commit, content);
    register_placed("t-frz-1", &placed);

    assert_eq!(
        merge_and_maybe_review_with(&f.deps, "t-frz-1"),
        MergeAttempt::FrozenDeferred
    );
    assert!(actions_of(&f).contains(&ACTION_MERGE_DEFERRED.to_string()));
    assert!(
        comments_of(&f)
            .iter()
            .any(|c| c.contains("⏸") && c.contains("解冻后自动补合并")),
        "冻结期交付必须有 ⏸ 评论"
    );
    assert_eq!(
        f.store.get_issue(f.issue_id).unwrap().status,
        IssueStatus::InProgress,
        "冻结期交付不得推进状态"
    );
    assert_eq!(
        std::fs::read(f.project_root.join("common.h")).unwrap(),
        b"line0\nline1\n",
        "冻结期变更集绝不入仓库"
    );
}

/// 合并停车臂：项目目录缺失 / 档案仓库 ensure 失败 / 变更集损坏。
#[test]
fn sweep_merge_park_arms() {
    // (a) 项目档案目录缺失（存量项目）。
    let f = fixture("park-nodir");
    let project = f
        .store
        .create_project("p-nodir", "d", None, "", "", None)
        .unwrap();
    let issue = f
        .store
        .create_issue(NewIssue {
            title: "park nodir".into(),
            creator: Actor::agent("node-a"),
            project_id: Some(project.id),
            ..Default::default()
        })
        .unwrap();
    f.store
        .insert_dispatch("t-park-a", issue.id, "node-b", &Actor::agent("node-a"))
        .unwrap();
    f.store
        .transition_issue(issue.id, IssueStatus::InProgress, &Actor::agent("node-a"))
        .unwrap();
    f.store
        .finish_dispatch("t-park-a", dispatch_state::DONE)
        .unwrap();
    f.store
        .set_dispatch_baseline("t-park-a", &f.base_commit)
        .unwrap();
    let placed = make_placed_with_changeset(&f.deps.workspace, "t-park-a", &f.base_commit, b"x\n");
    register_placed("t-park-a", &placed);
    match merge_and_maybe_review_with(&f.deps, "t-park-a") {
        MergeAttempt::Parked { reason } => assert!(reason.contains("项目档案目录缺失"), "{reason}"),
        other => panic!("应停车，实得 {other:?}"),
    }

    // (b) 档案仓库 ensure 失败（directory 指向普通文件）。
    let f2 = fixture("park-ensure");
    let not_a_repo = f2.deps.workspace.join("not-a-repo.txt");
    std::fs::write(&not_a_repo, b"x").unwrap();
    let project2 = f2
        .store
        .create_project(
            "p-file",
            "d",
            None,
            "",
            "",
            Some(not_a_repo.to_str().unwrap()),
        )
        .unwrap();
    let issue2 = f2
        .store
        .create_issue(NewIssue {
            title: "park ensure".into(),
            creator: Actor::agent("node-a"),
            project_id: Some(project2.id),
            ..Default::default()
        })
        .unwrap();
    f2.store
        .insert_dispatch("t-park-b", issue2.id, "node-b", &Actor::agent("node-a"))
        .unwrap();
    f2.store
        .transition_issue(issue2.id, IssueStatus::InProgress, &Actor::agent("node-a"))
        .unwrap();
    f2.store
        .finish_dispatch("t-park-b", dispatch_state::DONE)
        .unwrap();
    f2.store
        .set_dispatch_baseline("t-park-b", &f2.base_commit)
        .unwrap();
    let placed_b =
        make_placed_with_changeset(&f2.deps.workspace, "t-park-b", &f2.base_commit, b"x\n");
    register_placed("t-park-b", &placed_b);
    match merge_and_maybe_review_with(&f2.deps, "t-park-b") {
        MergeAttempt::Parked { reason } => {
            assert!(reason.contains("档案仓库 ensure 失败"), "{reason}")
        }
        other => panic!("应停车，实得 {other:?}"),
    }

    // (c) 变更集损坏（changeset.json 非法 JSON）。
    let f3 = fixture("park-corrupt");
    setup_done_dispatch(&f3, "t-park-c");
    let corrupt = f3.deps.workspace.join("placed-corrupt");
    std::fs::create_dir_all(corrupt.join("changeset")).unwrap();
    std::fs::write(
        corrupt.join("changeset").join("changeset.json"),
        "{bad json",
    )
    .unwrap();
    register_placed("t-park-c", &corrupt);
    match merge_and_maybe_review_with(&f3.deps, "t-park-c") {
        MergeAttempt::Parked { reason } => assert!(reason.contains("变更集损坏"), "{reason}"),
        other => panic!("应停车，实得 {other:?}"),
    }
    assert!(
        actions_of(&f3).contains(&ACTION_MERGE_PARKED.to_string()),
        "损坏停车必须落决策流卡"
    );
}

/// render_conflict_detail：二进制字节摘要 / 文本双侧新增行摘要 / 无新增行 /
/// 超 8 行截断统计。
#[test]
fn sweep_render_conflict_detail_matrix() {
    // 二进制。
    let bin = ConflictFile {
        path: "bin.dat".into(),
        binary: true,
        ours: Some(vec![0, 1, 2]),
        theirs: Some(vec![9]),
        ancestor: None,
    };
    let out = render_conflict_detail(&[bin]);
    assert!(out.contains("`bin.dat`"), "{out}");
    assert!(out.contains("二进制"), "{out}");
    assert!(out.contains("我方 3B / 对方 1B"), "{out}");

    // 文本：双侧各有新增行。
    let text = ConflictFile {
        path: "code.rs".into(),
        binary: false,
        ours: Some(b"a\nb\nours1\n".to_vec()),
        theirs: Some(b"a\nb\ntheirs1\ntheirs2\n".to_vec()),
        ancestor: Some(b"a\nb\n".to_vec()),
    };
    let out = render_conflict_detail(&[text]);
    assert!(
        out.contains("（我方 3 行 / 对方 4 行 / 基线 2 行）"),
        "{out}"
    );
    assert!(out.contains("+ ours1"), "{out}");
    assert!(out.contains("+ theirs1"), "{out}");
    assert!(out.contains("+ theirs2"), "{out}");

    // 无新增行（双方只删/改基线行为空集差）。
    let noop = ConflictFile {
        path: "same.txt".into(),
        binary: false,
        ours: Some(b"a\n".to_vec()),
        theirs: Some(b"a\n".to_vec()),
        ancestor: Some(b"a\n".to_vec()),
    };
    let out = render_conflict_detail(&[noop]);
    assert!(out.contains("相对基线无新增行"), "{out}");

    // 超 8 行新增 → 截断 + 总数统计。
    let mut big = String::from("a\n");
    for i in 0..10 {
        big.push_str(&format!("added{i}\n"));
    }
    let many = ConflictFile {
        path: "big.txt".into(),
        binary: false,
        ours: Some(big.clone().into_bytes()),
        theirs: Some(big.into_bytes()),
        ancestor: Some(b"a\n".to_vec()),
    };
    let out = render_conflict_detail(&[many]);
    assert!(out.contains("（共 10 行新增）"), "{out}");
}

/// retry_merge_for_issue 守卫臂：estop / 未绑项目 / 项目无目录 / 执行目录
/// 不可读 / 凭据不可读 / 凭据解析失败。
#[test]
fn sweep_retry_merge_for_issue_guards() {
    let f = fixture("retry-guards");

    // estop 中拒绝。
    f.deps.estop.trigger();
    let issue = f.store.get_issue(f.issue_id).unwrap();
    assert!(
        retry_merge_for_issue(&f.deps, &issue)
            .unwrap_err()
            .contains("急停")
    );
    f.deps.estop.release();

    // 未绑项目。
    let issue_np = f
        .store
        .create_issue(NewIssue {
            title: "np".into(),
            creator: Actor::agent("node-a"),
            ..Default::default()
        })
        .unwrap();
    let issue_np = f.store.get_issue(issue_np.id).unwrap();
    assert!(
        retry_merge_for_issue(&f.deps, &issue_np)
            .unwrap_err()
            .contains("未绑定项目")
    );

    // 项目无目录。
    let project_nd = f
        .store
        .create_project("p-nd", "d", None, "", "", None)
        .unwrap();
    let issue_nd = f
        .store
        .create_issue(NewIssue {
            title: "nd".into(),
            creator: Actor::agent("node-a"),
            project_id: Some(project_nd.id),
            ..Default::default()
        })
        .unwrap();
    let issue_nd = f.store.get_issue(issue_nd.id).unwrap();
    assert!(
        retry_merge_for_issue(&f.deps, &issue_nd)
            .unwrap_err()
            .contains("未绑定档案目录")
    );

    // 执行目录不可读（records/ 不存在）。
    let issue = f.store.get_issue(f.issue_id).unwrap();
    assert!(
        retry_merge_for_issue(&f.deps, &issue)
            .unwrap_err()
            .contains("档案执行目录不可读")
    );

    // 凭据缺失 / 解析失败 → 逐目录 skipped 明细。
    let exec = f
        .project_root
        .join("records")
        .join(&issue.number)
        .join("execution");
    std::fs::create_dir_all(exec.join("20260924_010000_000")).unwrap();
    let bad = exec.join("20260924_020000_000");
    std::fs::create_dir_all(&bad).unwrap();
    std::fs::write(bad.join("manifest.json"), b"{not json").unwrap();
    let out = retry_merge_for_issue(&f.deps, &issue).unwrap();
    let rows = out["retried"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{out}");
    assert!(
        rows[0]["skipped"]
            .as_str()
            .unwrap()
            .contains("manifest.json 不可读")
    );
    assert!(rows[1]["skipped"].as_str().unwrap().contains("解析失败"));
}

/// sweep_missing_archives 腿 1：收件箱滞留档案重灌安置（成功清收件箱 +
/// PLACED 登记），records 就位后腿 2 不空拉。
#[tokio::test]
async fn sweep_sweep_missing_archives_leg1_reingest() {
    let f = fixture("d5-leg1");
    f.store
        .insert_dispatch("t-leg1", f.issue_id, "node-b", &Actor::agent("node-a"))
        .unwrap();
    let inbox = make_inbox(&f.deps.workspace.join("cluster"), "t-leg1", true, true);

    let fake = FakeTransport::new(vec![]);
    let seen = tokio::sync::Mutex::<std::collections::HashSet<String>>::default();
    sweep_missing_archives(&f.store, &f.deps.workspace, &fake, &seen).await;

    assert!(!inbox.exists(), "腿 1 重灌成功必须清收件箱");
    let exec = f.project_root.join("records");
    assert!(exec.exists(), "档案应安置到 records/ 下");
    assert_eq!(fake.call_count(), 0, "records 已就位，腿 2 不得空拉");
}

/// sweep_missing_archives 腿 2：queued 重推 / no_archive 记账去重 / 离线
/// Err 重试；records 在场、无终态派发、无项目、无目录全部跳过。
#[tokio::test]
async fn sweep_sweep_missing_archives_leg2_transport_matrix() {
    // (a) queued：worker 已重推。
    let f = fixture("d5-queued");
    f.store
        .insert_dispatch("t-d5q", f.issue_id, "node-b", &Actor::agent("node-a"))
        .unwrap();
    f.store
        .finish_dispatch("t-d5q", dispatch_state::DONE)
        .unwrap();
    let fake = FakeTransport::new(vec![Ok(serde_json::json!({"status": "queued"}))]);
    let seen = tokio::sync::Mutex::<std::collections::HashSet<String>>::default();
    sweep_missing_archives(&f.store, &f.deps.workspace, &fake, &seen).await;
    assert_eq!(fake.call_count(), 1);
    assert_eq!(
        fake.calls.lock().unwrap()[0],
        ("node-b".to_string(), "transfer_pull".to_string())
    );
    assert!(seen.lock().await.is_empty(), "queued 不记 no_archive");

    // (b) no_archive：记账去重，第二轮不再拉。
    let f = fixture("d5-noarchive");
    f.store
        .insert_dispatch("t-d5n", f.issue_id, "node-b", &Actor::agent("node-a"))
        .unwrap();
    f.store
        .finish_dispatch("t-d5n", dispatch_state::DONE)
        .unwrap();
    let fake = FakeTransport::new(vec![Ok(serde_json::json!({"status": "no_archive"}))]);
    let seen = tokio::sync::Mutex::<std::collections::HashSet<String>>::default();
    sweep_missing_archives(&f.store, &f.deps.workspace, &fake, &seen).await;
    assert_eq!(fake.call_count(), 1);
    assert!(seen.lock().await.contains("t-d5n"), "no_archive 必须记账");
    sweep_missing_archives(&f.store, &f.deps.workspace, &fake, &seen).await;
    assert_eq!(fake.call_count(), 1, "记账后不再空拉");

    // (c) 离线 Err：不炸不记账（下轮重试）。
    let f = fixture("d5-offline");
    f.store
        .insert_dispatch("t-d5e", f.issue_id, "node-b", &Actor::agent("node-a"))
        .unwrap();
    f.store
        .finish_dispatch("t-d5e", dispatch_state::DONE)
        .unwrap();
    let fake = FakeTransport::new(vec![Err("connection refused".to_string())]);
    let seen = tokio::sync::Mutex::<std::collections::HashSet<String>>::default();
    sweep_missing_archives(&f.store, &f.deps.workspace, &fake, &seen).await;
    assert_eq!(fake.call_count(), 1);
    assert!(seen.lock().await.is_empty(), "离线不记 no_archive");

    // (d) 无终态派发 → 不拉。
    let f = fixture("d5-inflight");
    f.store
        .insert_dispatch("t-d5i", f.issue_id, "node-b", &Actor::agent("node-a"))
        .unwrap();
    let fake = FakeTransport::new(vec![]);
    let seen = tokio::sync::Mutex::<std::collections::HashSet<String>>::default();
    sweep_missing_archives(&f.store, &f.deps.workspace, &fake, &seen).await;
    assert_eq!(fake.call_count(), 0, "在途派发本来就没有档案");
}

/// replay_pending_merges 守卫臂：estop / 未冻结 / 冻结但无目录。
#[test]
fn sweep_replay_pending_guards() {
    let f = fixture("replay-guards");
    f.deps.estop.trigger();
    assert!(
        replay_pending_merges(&f.deps, 1)
            .unwrap_err()
            .contains("急停")
    );
    f.deps.estop.release();

    // 未冻结 → unfrozen:false 短路。
    let pid = f.store.get_issue(f.issue_id).unwrap().project_id.unwrap();
    let out = replay_pending_merges(&f.deps, pid).unwrap();
    assert_eq!(out["unfrozen"], serde_json::json!(false));
    assert_eq!(out["merged"], serde_json::json!(0));

    // 冻结但项目无目录 → Err。
    let project = f
        .store
        .create_project("p-nd2", "d", None, "", "", None)
        .unwrap();
    f.store
        .set_project_conflict_frozen(project.id, true)
        .unwrap();
    assert!(
        replay_pending_merges(&f.deps, project.id)
            .unwrap_err()
            .contains("未绑定档案目录")
    );
}

/// 解冻补合并全矩阵：ghost issue / 空集宽容 / 真合并 / 损坏停车 / 无基线
/// 停车 / 迟到 superseded，一条汇总不连坐。
#[test]
fn sweep_replay_pending_full_matrix() {
    let f = fixture("replay-full");
    let pid = f.store.get_issue(f.issue_id).unwrap().project_id.unwrap();
    f.store.set_project_conflict_frozen(pid, true).unwrap();
    // 正常流形态：冲突冻结发生在执行期，issue 已在途；补合并收口要推进
    // InReview，Backlog→InReview 非法跳转会被 store 拒绝（让位语义）。
    f.store
        .transition_issue(f.issue_id, IssueStatus::InProgress, &Actor::agent("node-a"))
        .unwrap();

    let empty_dir = f.deps.workspace.join("placed-rp-empty");
    std::fs::create_dir_all(&empty_dir).unwrap();
    std::fs::write(empty_dir.join("log.md"), b"log").unwrap();
    let good = make_placed_with_changeset(
        &f.deps.workspace,
        "t-rp-good",
        &f.base_commit,
        b"line0\nreplayed-good\n",
    );
    let corrupt = f.deps.workspace.join("placed-rp-corrupt");
    std::fs::create_dir_all(corrupt.join("changeset")).unwrap();
    std::fs::write(corrupt.join("changeset").join("changeset.json"), "{bad").unwrap();
    let nobase = make_placed_with_changeset(
        &f.deps.workspace,
        "t-rp-nobase",
        &f.base_commit,
        b"line0\nreplayed-nobase\n",
    );
    let stale = make_placed_with_changeset(
        &f.deps.workspace,
        "t-rp-stale",
        "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
        b"stale\n",
    );

    // 派发行：good（真合并）/ nobase（无基线行）/ stale（基线失配）需要派发行；
    // good/stale 另有基线行，nobase 刻意没有（停车臂形态）。
    f.store
        .insert_dispatch("t-rp-good", f.issue_id, "node-b", &Actor::agent("node-a"))
        .unwrap();
    f.store
        .set_dispatch_baseline("t-rp-good", &f.base_commit)
        .unwrap();
    f.store
        .insert_dispatch("t-rp-nobase", f.issue_id, "node-b", &Actor::agent("node-a"))
        .unwrap();
    f.store
        .insert_dispatch("t-rp-stale", f.issue_id, "node-b", &Actor::agent("node-a"))
        .unwrap();
    f.store
        .set_dispatch_baseline("t-rp-stale", &f.base_commit)
        .unwrap();

    let entry = |task: &str, issue_id: i64, dir: &Path| nemesis_board::models::PendingMerge {
        task_id: task.to_string(),
        issue_id,
        placement_dir: dir.display().to_string(),
        reason: "conflict_freeze".to_string(),
        parked_at_ms: 0,
    };
    f.store
        .append_pending_merge(pid, entry("t-rp-ghost", 999_999, &empty_dir))
        .unwrap();
    f.store
        .append_pending_merge(pid, entry("t-rp-empty", f.issue_id, &empty_dir))
        .unwrap();
    f.store
        .append_pending_merge(pid, entry("t-rp-good", f.issue_id, &good))
        .unwrap();
    f.store
        .append_pending_merge(pid, entry("t-rp-corrupt", f.issue_id, &corrupt))
        .unwrap();
    f.store
        .append_pending_merge(pid, entry("t-rp-nobase", f.issue_id, &nobase))
        .unwrap();
    f.store
        .append_pending_merge(pid, entry("t-rp-stale", f.issue_id, &stale))
        .unwrap();

    let out = replay_pending_merges(&f.deps, pid).unwrap();
    assert_eq!(out["merged"], serde_json::json!(2), "{out}");
    assert_eq!(out["superseded"], serde_json::json!(1), "{out}");
    assert_eq!(out["parked"].as_array().unwrap().len(), 3, "{out}");
    assert_eq!(out["refrozen"], serde_json::json!(false));
    assert_eq!(out["unfrozen"], serde_json::json!(true));

    // 真合并真实落盘 + 空集宽容照常进评审。
    assert_eq!(
        std::fs::read(f.project_root.join("common.h")).unwrap(),
        b"line0\nreplayed-good\n"
    );
    assert_eq!(
        f.store.get_issue(f.issue_id).unwrap().status,
        IssueStatus::InReview
    );
    assert!(actions_of(&f).contains(&ACTION_MERGED.to_string()));
    assert!(
        actions_of(&f)
            .iter()
            .any(|a| *a == super::ACTION_ARCHIVE_SUPERSEDED),
        "迟到交付必须落 superseded 审计"
    );
}

/// 补合并再冲突 → 重新冻结 + 剩余队列原地保留（resume 回人工，不进 auto）。
#[test]
fn sweep_replay_refreezes_on_new_conflict() {
    let f = fixture("replay-refreeze");
    let pid = f.store.get_issue(f.issue_id).unwrap().project_id.unwrap();
    f.store.set_project_conflict_frozen(pid, true).unwrap();

    let first = make_placed_with_changeset(
        &f.deps.workspace,
        "t-rf-1",
        &f.base_commit,
        b"line0\nfrom-replay-one\n",
    );
    let second = make_placed_with_changeset(
        &f.deps.workspace,
        "t-rf-2",
        &f.base_commit,
        b"line0\nfrom-replay-two\n",
    );
    for t in ["t-rf-1", "t-rf-2"] {
        f.store
            .insert_dispatch(t, f.issue_id, "node-b", &Actor::agent("node-a"))
            .unwrap();
        f.store.set_dispatch_baseline(t, &f.base_commit).unwrap();
    }
    let entry = |task: &str, dir: &Path| nemesis_board::models::PendingMerge {
        task_id: task.to_string(),
        issue_id: f.issue_id,
        placement_dir: dir.display().to_string(),
        reason: "conflict_freeze".to_string(),
        parked_at_ms: 0,
    };
    f.store
        .append_pending_merge(pid, entry("t-rf-1", &first))
        .unwrap();
    f.store
        .append_pending_merge(pid, entry("t-rf-2", &second))
        .unwrap();

    let out = replay_pending_merges(&f.deps, pid).unwrap();
    assert_eq!(out["merged"], serde_json::json!(1), "{out}");
    assert_eq!(out["refrozen"], serde_json::json!(true), "{out}");
    assert_eq!(out["unfrozen"], serde_json::json!(false));
    let parked = out["parked"].as_array().unwrap();
    assert!(
        parked
            .iter()
            .any(|p| p.as_str().unwrap().contains("t-rf-2")
                && p.as_str().unwrap().contains("重新冻结")),
        "{out}"
    );
    assert!(
        f.store.get_project(pid).unwrap().conflict_frozen,
        "再冲突必须重新置位冻结"
    );
}

/// finish_merged_review：单据已被别处推进（已是 in_review）→ transition
/// 失败让位，仍返回 Merged 不炸（P5 冲突硬解共用收口的让位语义）。
#[test]
fn sweep_finish_merged_review_yields_when_already_in_review() {
    let f = fixture("finish-yield");
    f.store
        .transition_issue(f.issue_id, IssueStatus::InProgress, &Actor::agent("node-a"))
        .unwrap();
    f.store
        .transition_issue(f.issue_id, IssueStatus::InReview, &Actor::agent("node-a"))
        .unwrap();
    let issue = f.store.get_issue(f.issue_id).unwrap();
    assert_eq!(
        finish_merged_review(&f.store, &issue, "直调收口（让位形态）", None),
        MergeAttempt::Merged
    );
    assert!(comments_of(&f).iter().any(|c| c.contains("直调收口")));
    assert_eq!(
        f.store.get_issue(f.issue_id).unwrap().status,
        IssueStatus::InReview,
        "已推进的单据不被二次翻动"
    );
}

// ===========================================================================
// wave4 追加（coverage）：残余臂——WaitingChangeset 内入口臂、ingest 档案
// 复制失败 orphan、D5「收件箱滞留算在路上」不空拉、冲突 auto 档移交硬解
// （含旗标读取失败回落 human）、三方合并执行失败停车、replay 未知项目 Err。
// ===========================================================================

/// 派发已 done、变更集未落地（PLACED 未登记）→ 等落地腿触发。
#[test]
fn sweep_waiting_changeset_when_placed_not_yet_registered() {
    let f = fixture("wait-changeset");
    let tid = "t-wait-cs-1";
    setup_done_dispatch(&f, tid);
    assert_eq!(
        merge_and_maybe_review_with(&f.deps, tid),
        MergeAttempt::WaitingChangeset,
        "交付落定但变更集未到 = 等落地腿，PLACED 未登记不得误合并"
    );
}

/// 收件箱有凭据但 files/ 缺失（上游异常形态）→ 安置在复制一步失败 →
/// orphan 留痕 + 档案留在收件箱。
#[test]
fn sweep_ingest_copy_failure_orphans_and_keeps_inbox() {
    let f = fixture("copy-fail");
    let tid = "t-copy-fail-1";
    f.store
        .insert_dispatch(tid, f.issue_id, "node-b", &Actor::agent("node-a"))
        .unwrap();
    let inbox = f.deps.workspace.join("inbox").join(tid);
    std::fs::create_dir_all(&inbox).unwrap();
    std::fs::write(inbox.join("manifest.json"), b"{}").unwrap();
    std::fs::write(inbox.join("landed.json"), b"{}").unwrap();
    // 刻意不建 files/ —— copy_dir_recursive 读不到源目录即失败。

    assert!(ingest_landed(&f.store, tid, &inbox).is_none());
    let orphaned: Vec<_> = f
        .store
        .list_activity(f.issue_id)
        .unwrap()
        .into_iter()
        .filter(|a| a.action == super::ACTION_ARCHIVE_ORPHANED)
        .collect();
    assert!(
        orphaned
            .iter()
            .any(|a| a.details.as_deref().unwrap_or("").contains("档案复制失败")),
        "复制失败必须 orphan 留痕: {:?}",
        orphaned
            .iter()
            .map(|a| a.details.clone())
            .collect::<Vec<_>>()
    );
    assert!(inbox.exists(), "无处安置的档案留在收件箱");
}

/// D5 腿 2：master 收件箱还滞留着该任务的档案 =「在路上」，腿 1 已处理，
/// 不对 worker 空拉。
#[tokio::test]
async fn sweep_d5_skips_pull_when_inbox_still_holding_archive() {
    let f = fixture("d5-inbox-holding");
    let tid = "t-d5-hold-1";
    f.store
        .insert_dispatch(tid, f.issue_id, "node-b", &Actor::agent("node-a"))
        .unwrap();
    f.store.finish_dispatch(tid, dispatch_state::DONE).unwrap();
    // 脚手架移除 → 腿 1 重灌 orphan（档案留收件箱），腿 2 records 缺失成立。
    std::fs::remove_file(f.project_root.join("project.json")).unwrap();
    let inbox = make_inbox(&f.deps.workspace.join("cluster"), tid, true, true);

    let fake = FakeTransport::new(vec![]);
    let seen = tokio::sync::Mutex::<std::collections::HashSet<String>>::default();
    sweep_missing_archives(&f.store, &f.deps.workspace, &fake, &seen).await;

    assert!(inbox.exists(), "腿 1 无处安置，档案必须留在收件箱");
    assert_eq!(
        fake.call_count(),
        0,
        "收件箱滞留 = 在路上，不得对 worker 空拉"
    );
    assert!(seen.lock().await.is_empty());
}

/// 冲突 auto 档：conflict_auto_resolve=true → 移交 AI 硬解执行体
/// （AutoResolveStarted）；主 agent 未装配时硬解任务诚实回落 human 档
/// （冻结 + 审计 + 停车）。旗标读取失败 = 按 human 档停车（不放大权限）。
#[tokio::test]
async fn sweep_auto_resolve_hands_off_and_flag_failure_falls_back() {
    // (a) auto 档正常移交。
    let f = fixture("auto-resolve");
    let t1 = "t-auto-1";
    setup_done_dispatch(&f, t1);
    let p1 = make_placed_with_changeset(
        &f.deps.workspace,
        t1,
        &f.base_commit,
        b"line0\nauto-side-one\n",
    );
    register_placed(t1, &p1);
    assert_eq!(
        merge_and_maybe_review_with(&f.deps, t1),
        MergeAttempt::Merged
    );

    let t2 = "t-auto-2";
    setup_done_dispatch(&f, t2);
    let p2 = make_placed_with_changeset(
        &f.deps.workspace,
        t2,
        &f.base_commit,
        b"line0\nauto-side-two\n",
    );
    register_placed(t2, &p2);
    std::fs::write(
        f.deps.home.join("config.json"),
        r#"{"board": {"conflict_auto_resolve": true}}"#,
    )
    .unwrap();
    assert_eq!(
        merge_and_maybe_review_with(&f.deps, t2),
        MergeAttempt::AutoResolveStarted,
        "auto 档真冲突必须同步移交硬解执行体"
    );
    // 硬解任务主 agent 未装配 → 回落 human 档（冻结 + 审计）。
    let pid = f.store.get_issue(f.issue_id).unwrap().project_id.unwrap();
    let mut frozen = false;
    for _ in 0..40 {
        if f.store.get_project(pid).unwrap().conflict_frozen {
            frozen = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(frozen, "硬解任务回落必须冻结项目（human 档兜底）");
    assert!(
        f.store
            .list_activity(f.issue_id)
            .unwrap()
            .iter()
            .any(|a| a.action == "auto_decide"
                && a.details
                    .as_deref()
                    .unwrap_or("")
                    .contains("\"decision\":\"conflict\""),),
        "冲突审计必须落 auto_decide 流水"
    );

    // (b) 旗标读取失败（config.json 损坏）→ human 档停车。
    let f2 = fixture("auto-badflag");
    let u1 = "t-badflag-1";
    setup_done_dispatch(&f2, u1);
    let q1 = make_placed_with_changeset(
        &f2.deps.workspace,
        u1,
        &f2.base_commit,
        b"line0\nbad-side-one\n",
    );
    register_placed(u1, &q1);
    assert_eq!(
        merge_and_maybe_review_with(&f2.deps, u1),
        MergeAttempt::Merged
    );
    let u2 = "t-badflag-2";
    setup_done_dispatch(&f2, u2);
    let q2 = make_placed_with_changeset(
        &f2.deps.workspace,
        u2,
        &f2.base_commit,
        b"line0\nbad-side-two\n",
    );
    register_placed(u2, &q2);
    std::fs::write(f2.deps.home.join("config.json"), "{ not json").unwrap();
    match merge_and_maybe_review_with(&f2.deps, u2) {
        MergeAttempt::Parked { reason } => {
            assert!(reason.contains("三方合并冲突"), "{reason}")
        }
        other => panic!("旗标读取失败必须按 human 档停车，实得 {other:?}"),
    }
    let pid2 = f2.store.get_issue(f2.issue_id).unwrap().project_id.unwrap();
    assert!(
        f2.store.get_project(pid2).unwrap().conflict_frozen,
        "human 档必须冻结项目"
    );
}

/// 三方合并执行失败（基线 commit 不在本仓库——DB 基线与仓库历史失配的
/// 数据异常形态）→ 停车诚实暴露。
#[test]
fn sweep_merge_execution_error_parks() {
    let f = fixture("merge-err");
    let tid = "t-merge-err-1";
    f.store
        .insert_dispatch(tid, f.issue_id, "node-b", &Actor::agent("node-a"))
        .unwrap();
    f.store
        .transition_issue(f.issue_id, IssueStatus::InProgress, &Actor::agent("node-a"))
        .unwrap();
    f.store.finish_dispatch(tid, dispatch_state::DONE).unwrap();
    // DB 基线 = 变更集申报基线 = 合法格式但不存在的 commit —— E8 通过，
    // merge_changeset 在 find_commit 一步失败。
    let ghost = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
    f.store.set_dispatch_baseline(tid, ghost).unwrap();
    let placed = make_placed_with_changeset(&f.deps.workspace, tid, ghost, b"ghost baseline\n");
    register_placed(tid, &placed);

    match merge_and_maybe_review_with(&f.deps, tid) {
        MergeAttempt::Parked { reason } => {
            assert!(reason.contains("三方合并执行失败"), "{reason}")
        }
        other => panic!("合并执行失败应停车，实得 {other:?}"),
    }
    assert!(
        actions_of(&f).contains(&ACTION_MERGE_PARKED.to_string()),
        "执行失败停车必须落决策流卡"
    );
}

/// replay_pending_merges：未知项目 id → store Err 透传（? 传播臂）。
#[test]
fn sweep_replay_unknown_project_errors() {
    let f = fixture("replay-unknown-pid");
    assert!(replay_pending_merges(&f.deps, 999_999_999).is_err());
}

// ---------------------------------------------------------------------------
// wave5 round2（2026-09-25）：ingest_landed / note_missing_block / sweep 腿 2
// / 合并前置 commit 失败 / resume 人工解 commit 失败的 Err 臂补测。
// 纯文件系统故障注入（记录文件改名/加锁/只读/index.lock），零进程零端口。
// ---------------------------------------------------------------------------

mod w5r2 {
    use super::*;

    /// (a) `<root>/records` 被换成普通文件 → create_dir_all 失败 →
    /// orphan 留痕「创建安置目录失败」+ None（档案留收件箱）。
    #[test]
    fn w5_ingest_records_is_file_parks_orphan() {
        let f = fixture("w5b-recfile");
        f.store
            .insert_dispatch("t-w5b-rf", f.issue_id, "node-b", &Actor::agent("node-a"))
            .unwrap();
        let records = f.project_root.join("records");
        if records.is_dir() {
            std::fs::remove_dir_all(&records).unwrap();
        }
        std::fs::write(&records, b"not a dir").unwrap();
        let inbox = make_inbox(&f.deps.workspace, "t-w5b-rf", true, true);
        assert!(ingest_landed(&f.store, "t-w5b-rf", &inbox).is_none());
        assert!(inbox.exists(), "安置失败档案必须留在收件箱");
        assert!(
            actions_of(&f)
                .iter()
                .any(|a| a == super::ACTION_ARCHIVE_ORPHANED),
            "必须有 orphan 审计"
        );
        assert!(
            f.store.list_activity(f.issue_id).unwrap().iter().any(|a| a
                .details
                .as_deref()
                .unwrap_or("")
                .contains("创建安置目录失败")),
            "orphan 审计必须写明原因"
        );
    }

    /// (b) manifest.json 是目录 → 凭据复制失败 warn（安置照常推进）；
    /// (c) project.json 只读 → note_missing_block 写回失败 warn 让位。
    #[test]
    fn w5_ingest_cred_copy_fail_and_missing_block_write_fail() {
        let f = fixture("w5b-credfail");

        // (b) manifest.json 为目录：from.exists()=true 但 copy 目录→文件必炸。
        f.store
            .insert_dispatch("t-w5b-cd", f.issue_id, "node-b", &Actor::agent("node-a"))
            .unwrap();
        let inbox = make_inbox(&f.deps.workspace, "t-w5b-cd", false, true);
        std::fs::create_dir_all(inbox.join("manifest.json")).unwrap();
        let target = ingest_landed(&f.store, "t-w5b-cd", &inbox).expect("凭据复制失败不阻断安置");
        assert!(target.join("landed.json").exists(), "landed 凭据随行");
        assert!(
            !target.join("manifest.json").exists(),
            "manifest 复制失败的凭据不得假造"
        );

        // (c) project.json 只读 + 凭据缺失 → note_missing_block 读成功、写回
        // 失败（warn 让位，安置照常）。
        let proj = f.project_root.join("project.json");
        let mut perms = std::fs::metadata(&proj).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&proj, perms).unwrap();
        f.store
            .insert_dispatch("t-w5b-ro", f.issue_id, "node-b", &Actor::agent("node-a"))
            .unwrap();
        let inbox2 = make_inbox(&f.deps.workspace, "t-w5b-ro", true, false);
        let t2 = ingest_landed(&f.store, "t-w5b-ro", &inbox2);
        assert!(t2.is_some(), "写回失败不阻断安置");
        // missing_blocks 写不进去（只读），但函数必须不炸。
        // 恢复可写，避免残留只读文件影响临时目录清理。
        let mut perms = std::fs::metadata(&proj).unwrap().permissions();
        perms.set_readonly(false);
        std::fs::set_permissions(&proj, perms).unwrap();
    }

    /// (d) timeline.jsonl 被换成目录 → append_timeline 打开失败 → warn
    /// 不阻塞，安置照常完成。
    #[test]
    fn w5_ingest_timeline_open_fail_nonblocking() {
        let f = fixture("w5b-timeline");
        f.store
            .insert_dispatch("t-w5b-tl", f.issue_id, "node-b", &Actor::agent("node-a"))
            .unwrap();
        let tl = f.project_root.join("timeline.jsonl");
        std::fs::remove_file(&tl).unwrap();
        std::fs::create_dir_all(&tl).unwrap();
        let inbox = make_inbox(&f.deps.workspace, "t-w5b-tl", true, true);
        let target = ingest_landed(&f.store, "t-w5b-tl", &inbox).expect("timeline 失败不阻断");
        assert!(target.exists());
        assert!(!inbox.exists(), "安置成功照常清收件箱");
    }

    /// (e) 收件箱内非拷贝面文件被零共享模式句柄锁住 → remove_dir_all 失败
    /// → warn 留痕（收件箱保留），安置结果不受影响。仅 Windows（share_mode）。
    #[cfg(windows)]
    #[test]
    fn w5_ingest_inbox_cleanup_fail_locked_stray_file() {
        use std::os::windows::fs::OpenOptionsExt;
        let f = fixture("w5b-inblock");
        f.store
            .insert_dispatch("t-w5b-ib", f.issue_id, "node-b", &Actor::agent("node-a"))
            .unwrap();
        let inbox = make_inbox(&f.deps.workspace, "t-w5b-ib", true, true);
        // 顶层杂物文件（不在 files/ 拷贝面、非凭据），零共享锁住。
        let stray = inbox.join("stray.txt");
        std::fs::write(&stray, b"lock me").unwrap();
        let _lock = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0) // 不共享任何访问 → 删除必被拒
            .open(&stray)
            .expect("open stray with no sharing");
        let target = ingest_landed(&f.store, "t-w5b-ib", &inbox).expect("清理失败不阻断安置");
        assert!(target.exists());
        assert!(inbox.exists(), "锁住的收件箱必须保留（诚实清理失败）");
    }

    /// (f) sweep 腿 2 跳过矩阵：issue 未绑项目（无 project_id）与项目无档案
    /// 目录 → continue，不产生任何 pull 调用。
    #[tokio::test]
    async fn w5_sweep_leg2_skips_unbound_issue_and_dirless_project() {
        let f = fixture("w5b-sweep");
        // issue 无项目。
        let i_np = f
            .store
            .create_issue(NewIssue {
                title: "w5b no project".into(),
                creator: Actor::agent("node-a"),
                ..Default::default()
            })
            .unwrap();
        f.store
            .insert_dispatch("t-w5b-np", i_np.id, "node-b", &Actor::agent("node-a"))
            .unwrap();
        f.store
            .finish_dispatch("t-w5b-np", dispatch_state::DONE)
            .unwrap();
        // issue 绑定无目录项目（存量项目）。
        let p_nd = f
            .store
            .create_project("p-w5b-nd", "d", None, "", "", None)
            .unwrap();
        let i_nd = f
            .store
            .create_issue(NewIssue {
                title: "w5b no dir".into(),
                creator: Actor::agent("node-a"),
                project_id: Some(p_nd.id),
                ..Default::default()
            })
            .unwrap();
        f.store
            .insert_dispatch("t-w5b-nd", i_nd.id, "node-b", &Actor::agent("node-a"))
            .unwrap();
        f.store
            .finish_dispatch("t-w5b-nd", dispatch_state::DONE)
            .unwrap();

        let transport = FakeTransport::new(vec![]);
        let seen = tokio::sync::Mutex::new(std::collections::HashSet::new());
        sweep_missing_archives(&f.store, &f.deps.workspace, &transport, &seen).await;
        assert_eq!(
            transport.call_count(),
            0,
            "无项目/无目录的终态派发必须被跳过，不得空拉"
        );
    }

    /// (g) 合并前置 commit 失败（.git/index.lock 占位）→ 停车「合并前工作集
    /// commit 失败」。
    #[test]
    fn w5_merge_premerge_commit_fail_parks() {
        let f = fixture("w5b-prelock");
        let t = "t-w5b-pre";
        setup_done_dispatch(&f, t);
        let placed =
            make_placed_with_changeset(&f.deps.workspace, t, &f.base_commit, b"line0\nprelock\n");
        register_placed(t, &placed);
        std::fs::write(f.project_root.join(".git").join("index.lock"), b"{}").unwrap();
        match merge_and_maybe_review_with(&f.deps, t) {
            MergeAttempt::Parked { reason } => {
                assert!(reason.contains("合并前工作集 commit 失败"), "{reason}")
            }
            other => panic!("index.lock 占位必须让前置 commit 失败停车，实得 {other:?}"),
        }
    }

    /// (h) resume 人工解 commit 失败（.git 换成垃圾文件，git 全链失效）→
    /// Err「人工解 commit 失败」。
    #[test]
    fn w5_resume_manual_commit_fail_reports_err() {
        let f = fixture("w5b-resume");
        let pid = f.store.get_issue(f.issue_id).unwrap().project_id.unwrap();
        f.store.set_project_conflict_frozen(pid, true).unwrap();
        let dir = f.deps.workspace.join("placed-w5b-resume");
        std::fs::create_dir_all(&dir).unwrap();
        f.store
            .append_pending_merge(
                pid,
                nemesis_board::models::PendingMerge {
                    task_id: "t-w5b-rs".into(),
                    issue_id: f.issue_id,
                    placement_dir: dir.display().to_string(),
                    reason: "conflict_freeze".into(),
                    parked_at_ms: 0,
                },
            )
            .unwrap();
        // .git 目录换成普通文件 → ensure_repo/commit 链路必炸。
        let git = f.project_root.join(".git");
        std::fs::remove_dir_all(&git).unwrap();
        std::fs::write(&git, b"junk").unwrap();
        let err = replay_pending_merges(&f.deps, pid).unwrap_err();
        assert!(err.contains("人工解 commit 失败"), "{err}");
    }
}
