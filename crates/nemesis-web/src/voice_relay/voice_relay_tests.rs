//! voice_relay 热态/注册槽/接管判定测试（跨平台纯逻辑；OnceLock 全局态经
//! 进程内共享，用例共享同一组 atomics/注册槽 → 模块级锁串行化）。

use super::*;
use std::sync::atomic::Ordering;

/// 全局态串行化锁（同 crate handlers 测试的 voice_state_lock 同款模式）。
static HOT_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn apply_updates_flags_and_defaults_off() {
    let _serial = HOT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let h = realtime_hot_state();
    // 默认全关（首次触达）
    apply_realtime_config(&serde_json::json!({
        "enabled": false, "stream_stt": false, "two_pass": false,
        "tts_relay": false, "barge_in": false, "spoken_form": false,
        "voice_prompt": false,
    }));
    assert!(!h.enabled.load(Ordering::Relaxed));
    assert!(!h.tts_relay.load(Ordering::Relaxed));

    apply_realtime_config(&serde_json::json!({
        "enabled": true, "stream_stt": true, "two_pass": true,
        "tts_relay": true, "barge_in": true, "spoken_form": true,
        "voice_prompt": true,
    }));
    assert!(h.enabled.load(Ordering::Relaxed));
    assert!(h.stream_stt.load(Ordering::Relaxed));
    assert!(h.two_pass.load(Ordering::Relaxed));
    assert!(h.tts_relay.load(Ordering::Relaxed));
    assert!(h.barge_in.load(Ordering::Relaxed));
    assert!(h.spoken_form.load(Ordering::Relaxed));
    // voice_prompt 镜像同步（AgentLoop 持有的 Arc 读到同一值）
    assert!(voice_prompt_flag().load(Ordering::Relaxed));

    // 收尾回默认，避免污染同进程其他用例
    apply_realtime_config(&serde_json::json!({
        "enabled": false, "stream_stt": false, "two_pass": false,
        "tts_relay": false, "barge_in": false, "spoken_form": false,
        "voice_prompt": false,
    }));
    assert!(!voice_prompt_flag().load(Ordering::Relaxed));
}

#[test]
fn apply_ignores_missing_keys() {
    let _serial = HOT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let h = realtime_hot_state();
    apply_realtime_config(&serde_json::json!({ "enabled": true }));
    assert!(h.enabled.load(Ordering::Relaxed));
    // 缺键不翻其他位：未写 stream_stt 则维持原值（这里此前为 false）
    assert!(!h.stream_stt.load(Ordering::Relaxed));
    apply_realtime_config(&serde_json::json!({ "enabled": false }));
    assert!(!h.enabled.load(Ordering::Relaxed));
}

#[test]
fn takeover_requires_hot_and_assistant_and_match() {
    let _serial = HOT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_relay_registration(None);
    // 无注册：热态开也不接管
    apply_realtime_config(&serde_json::json!({ "enabled": true, "tts_relay": true }));
    assert!(!relay_should_take_over(
        Some("agent:main:session:s1"),
        "assistant",
        "你好"
    ));

    // 注册了别会话：不接管
    set_relay_registration(Some(Arc::new(RelayRegistration {
        session_key: "agent:main:session:s1".to_string(),
        voice_dir: std::path::PathBuf::from("."),
        config_dir: std::path::PathBuf::from("."),
        agent_loop: None,
    })));
    assert!(!relay_should_take_over(
        Some("agent:main:session:s2"),
        "assistant",
        "你好"
    ));

    // 匹配会话 + assistant 非空：接管
    assert!(relay_should_take_over(
        Some("agent:main:session:s1"),
        "assistant",
        "你好"
    ));

    // user 角色 / 空内容：不接管
    assert!(!relay_should_take_over(
        Some("agent:main:session:s1"),
        "user",
        "你好"
    ));
    assert!(!relay_should_take_over(
        Some("agent:main:session:s1"),
        "assistant",
        "   "
    ));
    // 无 session_key 的旧路径帧：不接管
    assert!(!relay_should_take_over(None, "assistant", "你好"));

    // 热态关：不接管
    apply_realtime_config(&serde_json::json!({ "enabled": false }));
    assert!(!relay_should_take_over(
        Some("agent:main:session:s1"),
        "assistant",
        "你好"
    ));

    // 只关 tts_relay：不接管
    apply_realtime_config(&serde_json::json!({ "enabled": true, "tts_relay": false }));
    assert!(!relay_should_take_over(
        Some("agent:main:session:s1"),
        "assistant",
        "你好"
    ));

    // 收尾
    apply_realtime_config(&serde_json::json!({ "enabled": false, "tts_relay": false }));
    set_relay_registration(None);
}
