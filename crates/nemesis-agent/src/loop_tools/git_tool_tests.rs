// D1 (2026-09-04) — GitTool 写操作测试。
//
// 覆盖：add/commit/branch_create/checkout/restore/stash 往返（真实 temp 仓）、
// push/reset --hard 等高危动作不可达（enum 白名单外）、stash op 白名单、
// 写 action 拒绝自由 `args`（防 flag 走私）、缺参诚实报错。
// git 不在测试机时逐测试 skip（沿用 tests.rs fresh-repo 约定）。

use super::*;
use tempfile::TempDir;

fn ctx() -> RequestContext {
    RequestContext::new("web", "chat1", "user1", "sess1")
}

/// init 一个带本地身份的 temp 仓（commit 可用），git 不可用返回 false（skip）。
fn init_repo(dir: &std::path::Path) -> bool {
    let ok = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(dir)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !ok {
        return false;
    }
    // 本地身份 + 关 gpgsign/autocrlf，保证 commit 在任何机器可用、
    // checkout/restore 写回的内容与提交字节一致（不受测试机全局
    // autocrlf=true 影响）。
    for cfg in [
        ["user.email", "test@example.com"],
        ["user.name", "Test"],
        ["commit.gpgsign", "false"],
        ["core.autocrlf", "false"],
    ] {
        let _ = std::process::Command::new("git")
            .args(["config", cfg[0], cfg[1]])
            .current_dir(dir)
            .output();
    }
    true
}

fn write_file(dir: &std::path::Path, rel: &str, content: &str) {
    let p = dir.join(rel);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(p, content).unwrap();
}

async fn run(tool: &GitTool, args: &str) -> Result<String, String> {
    tool.execute(args, &ctx()).await
}

// --- 高危动作不可达（enum 白名单外）-----------------------------------------

#[tokio::test]
async fn dangerous_actions_are_unreachable() {
    for action in ["push", "reset", "clean", "rebase", "merge", "cherry-pick"] {
        let tmp = TempDir::new().unwrap();
        let tool = GitTool::new(tmp.path().to_string_lossy().to_string());
        let err = run(&tool, &format!(r#"{{"action":"{action}"}}"#))
            .await
            .expect_err("dangerous action must be unreachable");
        assert!(
            err.contains("not exposed"),
            "action {action} must hit the not-exposed error, got: {err}"
        );
        assert!(
            err.contains("use exec"),
            "error must redirect to exec (security pipeline), got: {err}"
        );
    }
}

#[tokio::test]
async fn stash_op_whitelist_rejects_drop_and_flags() {
    let tmp = TempDir::new().unwrap();
    assert!(init_repo(tmp.path()), "git must be available");
    let tool = GitTool::new(tmp.path().to_string_lossy().to_string());

    // drop / clear / apply / 带 flag 的自由串全部拒绝。
    for op in ["drop", "clear", "apply", "branch", "-p", "push -m sneaky"] {
        let err = run(&tool, &format!(r#"{{"action":"stash","args":"{op}"}}"#))
            .await
            .expect_err("non-whitelisted stash op must error");
        assert!(
            err.contains("unsupported stash op"),
            "op {op} must be rejected, got: {err}"
        );
    }
}

// --- 写 action 拒绝自由 args（防 flag 走私）+ 缺参诚实报错 --------------------

#[tokio::test]
async fn write_actions_reject_freeform_args_and_missing_params() {
    let tmp = TempDir::new().unwrap();
    assert!(init_repo(tmp.path()), "git must be available");
    let tool = GitTool::new(tmp.path().to_string_lossy().to_string());

    // 自由 args 在写 action 上是诚实错误（点出专用参数形状）。
    let err = run(&tool, r#"{"action":"add","args":"--dry-run ."}"#)
        .await
        .expect_err("freeform args on 'add' must error");
    assert!(err.contains("dedicated params"), "got: {err}");

    let err = run(&tool, r#"{"action":"commit","args":"--amend"}"#)
        .await
        .expect_err("--amend smuggling via args must error");
    assert!(err.contains("dedicated params"), "got: {err}");

    // 缺参 / 空参诚实报错。
    for (args_json, key) in [
        (r#"{"action":"add"}"#, "paths"),
        (r#"{"action":"add","paths":"  "}"#, "paths"),
        (r#"{"action":"commit"}"#, "message"),
        (r#"{"action":"commit","message":""}"#, "message"),
        (r#"{"action":"branch_create"}"#, "name"),
        (r#"{"action":"checkout"}"#, "ref"),
        (r#"{"action":"restore"}"#, "paths"),
    ] {
        let err = run(&tool, args_json)
            .await
            .expect_err("missing required param must error");
        assert!(
            err.contains(key),
            "error must name the missing param '{key}', got: {err}"
        );
    }
}

// --- add/commit 往返 --------------------------------------------------------

#[tokio::test]
async fn add_commit_roundtrip_shows_last_commit() {
    let tmp = TempDir::new().unwrap();
    assert!(init_repo(tmp.path()), "git must be available");
    write_file(tmp.path(), "hello.txt", "hi d1\n");
    let tool = GitTool::new(tmp.path().to_string_lossy().to_string());

    // add 后 status 应显示新文件。
    let added = run(&tool, r#"{"action":"add","paths":"hello.txt"}"#)
        .await
        .expect("add must succeed");
    let _ = added;
    let status = run(&tool, r#"{"action":"status"}"#).await.unwrap();
    assert!(
        status.contains("A"),
        "staged file must show as A, got: {status}"
    );

    // commit 成功 → 输出附 Last commit（hash 可见）。
    let commit = run(
        &tool,
        r#"{"action":"commit","message":"d1 roundtrip commit"}"#,
    )
    .await
    .expect("commit must succeed");
    assert!(
        commit.contains("Last commit:"),
        "commit output must append the oneline, got: {commit}"
    );
    assert!(
        commit.contains("d1 roundtrip commit"),
        "oneline must contain the subject, got: {commit}"
    );

    // 提交后 status 干净（无 M/A 条目，只剩 branch 头）。
    let status = run(&tool, r#"{"action":"status"}"#).await.unwrap();
    assert!(
        !status.contains('\t') && !status.contains(" M "),
        "worktree must be clean after commit, got: {status}"
    );
}

#[tokio::test]
async fn commit_without_changes_is_an_honest_error() {
    let tmp = TempDir::new().unwrap();
    assert!(init_repo(tmp.path()), "git must be available");
    let tool = GitTool::new(tmp.path().to_string_lossy().to_string());

    let err = run(&tool, r#"{"action":"commit","message":"nothing"}"#)
        .await
        .expect_err("commit on empty repo must error");
    assert!(err.contains("git commit failed"), "got: {err}");
}

// --- branch_create/checkout 往返 ---------------------------------------------

#[tokio::test]
async fn branch_create_and_checkout_roundtrip() {
    let tmp = TempDir::new().unwrap();
    assert!(init_repo(tmp.path()), "git must be available");
    write_file(tmp.path(), "f.txt", "x\n");
    let tool = GitTool::new(tmp.path().to_string_lossy().to_string());
    run(&tool, r#"{"action":"add","paths":"f.txt"}"#)
        .await
        .unwrap();
    run(&tool, r#"{"action":"commit","message":"base"}"#)
        .await
        .unwrap();

    let created = run(&tool, r#"{"action":"branch_create","name":"feature/d1"}"#)
        .await
        .expect("branch_create must succeed");
    let _ = created;

    run(&tool, r#"{"action":"checkout","ref":"feature/d1"}"#)
        .await
        .expect("checkout must succeed");

    // branch -vv 输出应显示 * feature/d1。
    let branch = run(&tool, r#"{"action":"branch"}"#).await.unwrap();
    assert!(
        branch.contains("* feature/d1"),
        "checked-out branch must be starred, got: {branch}"
    );
}

// --- restore 往返 ------------------------------------------------------------

#[tokio::test]
async fn restore_discards_working_changes() {
    let tmp = TempDir::new().unwrap();
    assert!(init_repo(tmp.path()), "git must be available");
    write_file(tmp.path(), "f.txt", "original\n");
    let tool = GitTool::new(tmp.path().to_string_lossy().to_string());
    run(&tool, r#"{"action":"add","paths":"f.txt"}"#)
        .await
        .unwrap();
    run(&tool, r#"{"action":"commit","message":"base"}"#)
        .await
        .unwrap();

    // 修改 → restore → 内容回滚到已提交状态。
    write_file(tmp.path(), "f.txt", "changed but not committed\n");
    let restored = run(&tool, r#"{"action":"restore","paths":"f.txt"}"#)
        .await
        .expect("restore must succeed");
    let _ = restored;
    let content = std::fs::read_to_string(tmp.path().join("f.txt")).unwrap();
    assert_eq!(
        content, "original\n",
        "restore must discard working changes"
    );

    let status = run(&tool, r#"{"action":"status"}"#).await.unwrap();
    assert!(
        !status.contains(" M "),
        "worktree must be clean after restore, got: {status}"
    );
}

// --- stash 往返 --------------------------------------------------------------

#[tokio::test]
async fn stash_push_pop_roundtrip() {
    let tmp = TempDir::new().unwrap();
    assert!(init_repo(tmp.path()), "git must be available");
    write_file(tmp.path(), "f.txt", "base\n");
    let tool = GitTool::new(tmp.path().to_string_lossy().to_string());
    run(&tool, r#"{"action":"add","paths":"f.txt"}"#)
        .await
        .unwrap();
    run(&tool, r#"{"action":"commit","message":"base"}"#)
        .await
        .unwrap();

    // 空 stash：list 走 no-changes 臂。
    let empty = run(&tool, r#"{"action":"stash"}"#).await.unwrap();
    assert!(
        empty.contains("(no changes / empty)"),
        "empty stash list must hit the no-changes arm, got: {empty}"
    );

    // 修改 → stash push → 工作区干净。
    write_file(tmp.path(), "f.txt", "wip change\n");
    run(&tool, r#"{"action":"stash","args":"push","message":"wip"}"#)
        .await
        .expect("stash push must succeed");
    let status = run(&tool, r#"{"action":"status"}"#).await.unwrap();
    assert!(
        !status.contains(" M "),
        "worktree must be clean after stash push, got: {status}"
    );

    // list 显示条目；pop 恢复修改。
    let list = run(&tool, r#"{"action":"stash","args":"list"}"#)
        .await
        .unwrap();
    assert!(
        list.contains("wip"),
        "stash list must contain the message, got: {list}"
    );

    run(&tool, r#"{"action":"stash","args":"pop"}"#)
        .await
        .expect("stash pop must succeed");
    let content = std::fs::read_to_string(tmp.path().join("f.txt")).unwrap();
    assert_eq!(
        content, "wip change\n",
        "pop must restore the working change"
    );
}
