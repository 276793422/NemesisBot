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

// ---------------------------------------------------------------------------
// Wave4 覆盖批次（2026-09-25）：资产目录/讨论桥注入面与就绪判定。
// ---------------------------------------------------------------------------

use std::sync::Arc as StdArc;

/// 最小 DiscussionIngress 桩：记录调用并返回固定 JSON。
struct RecordingIngress {
    hit: std::sync::atomic::AtomicUsize,
}

impl crate::service::DiscussionIngress for RecordingIngress {
    fn post(
        &self,
        _sender: &crate::assignment::Actor,
        _thread_kind: &str,
        _thread_id: i64,
        _client_msg_id: &str,
        _content: &str,
        _reply_to: Option<i64>,
        _kind_tag: &str,
    ) -> Result<serde_json::Value, String> {
        self.hit.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(serde_json::json!({ "comment_id": 1 }))
    }
}

#[test]
fn asset_serving_requires_both_secret_and_dir() {
    let (svc, dir) = temp_service("asset-ready", NodeRole::Worker);
    assert!(!svc.asset_serving_ready(), "裸服务未就绪");
    assert!(svc.assets_dir().is_none());
    assert!(svc.asset_secret().is_none());

    let svc = svc.with_asset_secret(vec![1, 2, 3]);
    assert!(!svc.asset_serving_ready(), "只有密钥仍不就绪");
    assert_eq!(svc.asset_secret(), Some(&[1u8, 2, 3][..]));

    let assets = dir.join("assets");
    std::fs::create_dir_all(&assets).unwrap();
    let svc = svc.with_assets_dir(assets.clone());
    assert!(svc.asset_serving_ready(), "密钥+目录齐备即就绪");
    assert_eq!(svc.assets_dir(), Some(assets.as_path()));
    cleanup(&dir);
}

#[test]
fn discussion_bridge_injection_and_dispatch() {
    let (svc, dir) = temp_service("discuss", NodeRole::Coordinator);
    assert!(svc.discussion().is_none(), "未注入 = None");

    let rec = StdArc::new(RecordingIngress {
        hit: std::sync::atomic::AtomicUsize::new(0),
    });
    let svc = svc.with_discussion(rec.clone());
    let bridge = svc.discussion().expect("注入后可取回");
    let out = bridge
        .post(
            &crate::assignment::Actor::admin("admin"),
            crate::models::thread_kind::CHANNEL,
            1,
            "client-1",
            "大家好",
            None,
            "discussion",
        )
        .unwrap();
    assert_eq!(out["comment_id"], 1);
    assert_eq!(
        rec.hit.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "桩被真实调用一次"
    );
    cleanup(&dir);
}
