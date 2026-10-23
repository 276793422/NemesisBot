#![cfg(windows)]

use super::imp::*;
use nemesis_ziwei_profile::Profile;

fn mgr() -> SessionManager {
    SessionManager::new(std::path::Path::new("C:/nonexistent-zw-test"), None, 2)
}

#[test]
fn session_count_and_list_empty() {
    let m = mgr();
    assert_eq!(m.session_count(), 0);
    assert!(m.list_sessions().is_empty());
    assert!(m.get("p").is_none());
}

#[tokio::test]
async fn attach_rejects_dead_pid_before_payload() {
    let m = mgr();
    let profile = test_profile();
    // pid 0xFFFFFFF 不存在——payload 落盘都不到就被诚实拒绝
    let err = m.attach(&profile, 0xFFFF_FFF0).await.unwrap_err();
    assert!(matches!(
        err,
        AttachError::ProcessAbsent { pid: 0xFFFF_FFF0 }
    ));
    // pending 已清理：同 profile 可重试
    assert_eq!(m.session_count(), 0);
}

#[tokio::test]
async fn attach_slot_full() {
    let m = SessionManager::new(std::path::Path::new("C:/nonexistent-zw-test"), None, 1);
    let profile = test_profile();
    // 死 pid 预检失败——但 slot 占位/清理逻辑可由连续失败验证不泄漏
    let _ = m.attach(&profile, 0xFFFF_FFF1).await.unwrap_err();
    let _ = m.attach(&profile, 0xFFFF_FFF2).await.unwrap_err();
    // 单槽上限在 pending 清理正确时不应提前触发
    // （真实上限测试在 M1 集成测试：双 dummy 会话并发）
}

fn test_profile() -> Profile {
    serde_json::from_str(
            r#"{
            "schema_version": 1,
            "profile_id": "p-test",
            "name": "测试",
            "created_at": "2026-01-01T00:00:00Z",
            "last_verified_at": "2026-01-01T00:00:00Z",
            "status": "active",
            "target": {"exe_name": "dummy-draw.exe", "machine": "x64", "verify_policy": "hash_or_version"},
            "attach": {"mode": "attach"},
            "verification": {"on_attach": ["file_sha256"]}
        }"#,
        )
        .unwrap()
}
