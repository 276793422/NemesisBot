//! 会话级全部放行（2026-10-24 长任务无人值守计划）：session_allow 双写 /
//! session_allow_status 双臂 / 会话键推导与 chat.sync 公式字节一致。

use super::*;

/// 槽触碰类测试统一在锁内跑（进程级共享态串行纪律）。
/// 同步 #[test] 形态：tokio Mutex 的 `blocking_lock()` 在无运行时的测试
/// 线程上停等获取——此前 `lock()` 不 await 只拿到未 poll 的 Future，串行
/// 从未生效（2026-10-24 外评 T0）。切勿在 async 运行时内调用本 helper
/// （blocking_lock 会 loud panic）；异步用例直接 `LOCK.lock().await`
/// （handlers/tests.rs sessions 投影用例即此形态）。
fn with_slot<T>(f: impl FnOnce() -> T) -> T {
    let _serial = SESSION_ALLOW_ALL_TEST_LOCK.blocking_lock();
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

// --- R1 回归锁（2026-10-24 外评探针反转）：持久化失败必须诚实响应 ---
//
// 旧形态：write_session_allow_all 吞错返 ()，handler 无视结果照常返回普通
// 成功——「关闭放行」持久化失败后磁盘残留 true，重启 hydrate 把放行复活。
// 修复后：persisted:false + warning；内存态按当次意志保留（关=安全方向）。
// share_mode(1)（仅 FILE_SHARE_READ）锁 meta 文件逼原子替换失败。Windows
// 独有 API，非 Windows 不编译（探针原样 cfg(windows)）。

#[cfg(windows)]
#[test]
fn session_allow_disable_persist_failure_is_honest() {
    use std::os::windows::fs::OpenOptionsExt;
    with_slot(|| {
        let _home = crate::test_home::lock_home();
        let sid = format!("test-allowpersist-{}", std::process::id());
        let handler = SecurityHandler::new();
        let state =
            std::sync::Arc::new(nemesis_security::session_allow_all::SessionAllowAllState::new());
        install_session_allow_all(state.clone());
        let key = derive_session_key(&sid);
        let safe = nemesis_utils::sanitize::sanitize_path_segment(&key);
        let meta_path = nemesis_path::default_path_manager()
            .sessions_log_dir()
            .join(format!("{safe}.meta.json"));

        // 开：正常路径持久化成功。
        let v = handler.session_allow(&sid, true).unwrap().unwrap();
        assert_eq!(v["persisted"], true, "正常路径 persisted=true");

        // 锁住 meta 文件（拒写/删/替换）。
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&meta_path)
            .expect("打开锁句柄");

        // 关：持久化失败 → persisted:false + warning；内存态安全方向（关）。
        let v = handler.session_allow(&sid, false).unwrap().unwrap();
        assert_eq!(v["enabled"], false);
        assert_eq!(v["persisted"], false, "持久化失败必须诚实标注");
        assert!(
            !v["warning"].as_str().unwrap_or_default().is_empty(),
            "必须带 warning"
        );
        assert!(!state.check(&key), "内存态安全方向：关闭即时生效");
        let (persisted, _) =
            nemesis_agent::chat_log::read_session_allow_all(&key).expect("meta 仍在");
        assert!(
            persisted,
            "磁盘残留旧 true——正是 warning 声明的「重启将恢复放行」"
        );

        // 解锁重试：关闭成功落盘。
        drop(lock);
        let v = handler.session_allow(&sid, false).unwrap().unwrap();
        assert_eq!(v["persisted"], true, "解锁后重试成功");
        let (persisted, _) = nemesis_agent::chat_log::read_session_allow_all(&key).unwrap();
        assert!(!persisted);

        // 重启恢复面：hydrate 后不命中（撤销最终收敛）。
        let state2 =
            std::sync::Arc::new(nemesis_security::session_allow_all::SessionAllowAllState::new());
        state2.hydrate(nemesis_agent::chat_log::allow_all_sessions());
        assert!(!state2.check(&key), "hydrate 后必须不命中");

        cleanup_meta(&key);
    });
}

#[cfg(windows)]
#[test]
fn session_allow_enable_persist_failure_keeps_memory_intent() {
    use std::os::windows::fs::OpenOptionsExt;
    with_slot(|| {
        let _home = crate::test_home::lock_home();
        let sid = format!("test-allowpersist2-{}", std::process::id());
        let handler = SecurityHandler::new();
        let state =
            std::sync::Arc::new(nemesis_security::session_allow_all::SessionAllowAllState::new());
        install_session_allow_all(state.clone());
        let key = derive_session_key(&sid);
        let safe = nemesis_utils::sanitize::sanitize_path_segment(&key);
        let meta_path = nemesis_path::default_path_manager()
            .sessions_log_dir()
            .join(format!("{safe}.meta.json"));

        // 预置 meta（enable 走 upsert 需对既有文件做原子替换）。
        std::fs::create_dir_all(meta_path.parent().unwrap()).unwrap();
        std::fs::write(&meta_path, "{}").unwrap();

        // 锁住 → 开：持久化失败，内存保留用户当次意志。
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&meta_path)
            .expect("打开锁句柄");
        let v = handler.session_allow(&sid, true).unwrap().unwrap();
        assert_eq!(v["enabled"], true);
        assert_eq!(v["persisted"], false, "enable 持久化失败同样诚实标注");
        assert!(
            !v["warning"].as_str().unwrap_or_default().is_empty(),
            "必须带 warning"
        );
        assert!(state.check(&key), "内存态保留用户当次意志（开）");
        let (persisted, _) = nemesis_agent::chat_log::read_session_allow_all(&key).unwrap();
        assert!(!persisted, "磁盘未落 allow_all");

        // 解锁后关：收敛干净。
        drop(lock);
        let v = handler.session_allow(&sid, false).unwrap().unwrap();
        assert_eq!(v["persisted"], true);
        cleanup_meta(&key);
    });
}
