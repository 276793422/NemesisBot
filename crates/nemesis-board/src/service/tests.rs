//! `BoardService` 角色/句柄测试。

use super::*;
use crate::store::BoardStore;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn temp_service(name: &str, role: NodeRole) -> (BoardService, PathBuf) {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "nemesis-board-svctest-{}-{name}-{n}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let store = BoardStore::open(&dir.join("board.db"), "NB").expect("open store");
    (BoardService::new(std::sync::Arc::new(store), role), dir)
}

fn cleanup(dir: &PathBuf) {
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn test_role_predicates() {
    let (svc, dir) = temp_service("coordinator", NodeRole::Coordinator);
    assert!(svc.is_coordinator());
    assert_eq!(svc.role(), NodeRole::Coordinator);
    svc.store()
        .create_issue(crate::NewIssue {
            title: "权威节点可写（store 层不做角色拦截）".into(),
            ..Default::default()
        })
        .unwrap();
    cleanup(&dir);

    let (svc, dir) = temp_service("worker", NodeRole::Worker);
    assert!(!svc.is_coordinator());
    cleanup(&dir);
}

#[test]
fn test_service_is_clone_and_shares_store() {
    let (svc, dir) = temp_service("clone", NodeRole::Coordinator);
    let cloned = svc.clone();
    let issue = svc
        .store()
        .create_issue(crate::NewIssue {
            title: "克隆共享".into(),
            ..Default::default()
        })
        .unwrap();
    assert!(cloned.store().get_issue(issue.id).is_ok());
    cleanup(&dir);
}

#[test]
fn test_asset_secret_injection_and_default_none() {
    // 默认无密钥（未配资产服务语义）。
    let (svc, dir) = temp_service("nosecret", NodeRole::Coordinator);
    assert!(svc.asset_secret().is_none());
    cleanup(&dir);

    // builder 注入后可读回；Clone 共享同一密钥。
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let dir2 = std::env::temp_dir().join(format!("nemesis-board-svctest-secret-{n}",));
    let _ = std::fs::remove_dir_all(&dir2);
    let store = BoardStore::open(&dir2.join("board.db"), "NB").unwrap();
    let svc2 = BoardService::new(std::sync::Arc::new(store), NodeRole::Worker)
        .with_asset_secret(vec![1, 2, 3, 4]);
    assert_eq!(svc2.asset_secret(), Some(&[1u8, 2, 3, 4][..]));
    let cloned = svc2.clone();
    assert_eq!(cloned.asset_secret(), svc2.asset_secret());
    cleanup(&dir2);
}
