//! manager.rs 覆盖率收尾（Wave6B）：无活服务器可达的三个纯分支——
//! request_with_recovery 的文件存在性卫兵（376-377）、close_session 的
//! 未命中臂（582）、kill_tree 的已退出子进程臂（618）。
//!
//! 其余幸存 miss 归豁免：spec_for-None 三处（371-374 / 525-528 / 716）
//! 是防御臂——Lang 五变体在 SERVERS 全有规格，恒 Some；method_noun 的
//! Rename/CodeAction 臂是穷尽性死分支（query 有显式守卫，in-code 注释
//! 已声明）；rename 空编辑 / 泵传输错误恢复（404-414 / 451-454 / 473 /
//! 477）需真实语言服务器活会话。

use super::*;

/// 支持语言但文件不存在 → "file does not exist"（376-377）。
#[tokio::test]
async fn query_missing_file_reports_file_not_exist() {
    let mgr = LspManager::new(Some(Duration::from_secs(5)), Some(Duration::from_secs(60)));
    let missing = std::path::Path::new("Z:/covw6b_missing_project/lib.rs");
    let err = mgr
        .query(LspOp::Definition, missing, 0, 0)
        .await
        .expect_err("不存在的文件必须报错");
    assert!(
        err.contains("file does not exist"),
        "必须命中存在性卫兵: {err}"
    );
}

/// close_session 未命中（无该会话）→ false（581-582）。
#[tokio::test]
async fn close_session_unknown_key_returns_false() {
    let mgr = LspManager::new(Some(Duration::from_secs(5)), Some(Duration::from_secs(60)));
    let key = (
        Lang::Rust,
        std::path::PathBuf::from("Z:/covw6b_no_such_root"),
    );
    assert!(!mgr.close_session(&key).await);
}

/// kill_tree 对**已退出**子进程：id() 为 None → 跳过 taskkill（610 收口
/// false 臂 618）→ 兜底 kill 幂等不 panic。
#[tokio::test]
async fn kill_tree_on_already_exited_child_is_noop() {
    let mut child = tokio::process::Command::new("cmd")
        .args(["/C", "exit", "0"])
        .spawn()
        .expect("spawn cmd");
    let _ = child.wait().await; // 收割 → id() 归 None
    kill_tree(&mut child).await;
}
