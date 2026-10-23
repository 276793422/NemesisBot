//! 会话级全部放行（2026-10-24 长任务无人值守计划）：session_allow 双写 /
//! session_allow_status 双臂 / 会话键推导与 chat.sync 公式字节一致。

use super::*;

/// 槽触碰类测试统一在锁内跑（进程级共享态串行纪律）。
fn with_slot<T>(f: impl FnOnce() -> T) -> T {
    let _serial = SESSION_ALLOW_ALL_TEST_LOCK.lock();
    set_session_allow_all_for_test(None);
    let out = f();
    set_session_allow_all_for_test(None);
    out
}

/// 测试会话 meta 清理（唯一 sid + 删 sidecar，不留测试残留）。
fn cleanup_meta(session_key: &str) {
    let safe = nemesis_utils::sanitize::sanitize_path_segment(session_key);
    let p = nemesis_path::default_path_manager()
        .sessions_log_dir()
        .join(format!("{safe}.meta.json"));
    let _ = std::fs::remove_file(p);
}

/// 会话键推导与 chat.rs `chat.sync` 同一公式字节一致（**硬契约**：两处口径
/// 不一致 = 开关静默不命中——auditor 收到的 ctx.session_key 由 dispatch 侧
/// 用本公式生成）。
#[test]
fn derive_session_key_matches_chat_sync_formula() {
    // 复刻 chat.rs:61-63 的公式（字面同构，非调 helper——防两处被一起改错）。
    let sid = "sess-abc_123";
    let expected = format!(
        "agent:main:session:{}",
        nemesis_agent::session::SessionStore::sanitize_session_id(sid)
    );
    assert_eq!(derive_session_key(sid), expected);

    // 常规形态显式钉死（回归锚）。
    assert_eq!(
        derive_session_key("my-session"),
        "agent:main:session:my-session"
    );
}

#[test]
fn session_allow_uninjected_reports_injected_false() {
    with_slot(|| {
        let handler = SecurityHandler::new();
        let v = handler.session_allow("s1", true).unwrap().unwrap();
        assert_eq!(v["injected"], false, "未装配 = 降级形态诚实标注");
        let v = SecurityHandler::session_allow_status("s1");
        assert_eq!(v["injected"], false);
    });
}

#[test]
fn session_allow_set_dual_writes_memory_and_sidecar() {
    with_slot(|| {
        let _home = crate::test_home::lock_home();
        let sid = format!("test-allowall-{}", std::process::id());
        let handler = SecurityHandler::new();
        let state =
            std::sync::Arc::new(nemesis_security::session_allow_all::SessionAllowAllState::new());
        install_session_allow_all(state.clone());

        // 开：内存态命中 + sidecar 持久位记真。
        let v = handler.session_allow(&sid, true).unwrap().unwrap();
        let key = derive_session_key(&sid);
        assert_eq!(v["injected"], true);
        assert_eq!(v["enabled"], true);
        assert_eq!(v["session_key"], key.as_str());
        assert!(!v["since"].as_str().unwrap().is_empty(), "开=返回 since");
        assert!(state.check(&key), "内存态即时生效");
        let (persisted, since) =
            nemesis_agent::chat_log::read_session_allow_all(&key).expect("sidecar 应存在");
        assert!(persisted, "sidecar 持久位记真");
        assert_eq!(since, v["since"].as_str().unwrap_or_default());

        // 关：内存态失效 + sidecar 摘除。
        let v = handler.session_allow(&sid, false).unwrap().unwrap();
        assert_eq!(v["enabled"], false);
        assert!(!state.check(&key), "关闭即时生效");
        let (persisted, _) =
            nemesis_agent::chat_log::read_session_allow_all(&key).expect("sidecar 应存在");
        assert!(!persisted, "sidecar 持久位摘除");

        cleanup_meta(&key);
    });
}

#[test]
fn session_allow_status_reports_entry_and_default() {
    with_slot(|| {
        let _home = crate::test_home::lock_home();
        let sid = format!("test-allowstatus-{}", std::process::id());
        let state =
            std::sync::Arc::new(nemesis_security::session_allow_all::SessionAllowAllState::new());
        install_session_allow_all(state.clone());
        let key = derive_session_key(&sid);

        // 无条目：enabled=false 缺省形态（不是 injected:false）。
        let v = SecurityHandler::session_allow_status(&sid);
        assert_eq!(v["injected"], true);
        assert_eq!(v["enabled"], false);
        assert_eq!(v["count"], 0);

        // 开 + 计数：状态透传三字段。
        state.set(&key, true);
        state.bump(&key);
        let v = SecurityHandler::session_allow_status(&sid);
        assert_eq!(v["enabled"], true);
        assert_eq!(v["count"], 1, "计数同源透传");
        assert!(!v["since"].as_str().unwrap().is_empty());

        cleanup_meta(&key);
    });
}

#[test]
fn session_allow_persisted_flag_survives_state_rebuild() {
    // 重启恢复语义：sidecar 真 → 新建的运行态 hydrate 后命中（gateway 启动
    // 链的内存等价物；真 hydrate 扫描逻辑由 chat_log::allow_all_sessions
    // 测试覆盖，这里钉 handler 双写确实落了盘）。
    with_slot(|| {
        let _home = crate::test_home::lock_home();
        let sid = format!("test-allowhydrate-{}", std::process::id());
        let handler = SecurityHandler::new();
        let state =
            std::sync::Arc::new(nemesis_security::session_allow_all::SessionAllowAllState::new());
        install_session_allow_all(state);
        handler.session_allow(&sid, true).unwrap().unwrap();
        let key = derive_session_key(&sid);

        // 模拟重启：新运行态从 sidecar 扫描回载。
        let state2 =
            std::sync::Arc::new(nemesis_security::session_allow_all::SessionAllowAllState::new());
        state2.hydrate(nemesis_agent::chat_log::allow_all_sessions());
        assert!(state2.check(&key), "hydrate 后必须命中（重启恢复语义）");

        // 清理：关 + 删 sidecar。
        handler.session_allow(&sid, false).unwrap().unwrap();
        cleanup_meta(&key);
    });
}
