use super::*;

/// 开启态 check 命中 / 关闭态不命中 / 未知会话不命中。
#[test]
fn check_follows_enabled_state() {
    let state = SessionAllowAllState::new();
    assert!(!state.check("s1"), "未知会话不得放行");

    state.set("s1", true);
    assert!(state.check("s1"), "开启后命中");

    state.set("s1", false);
    assert!(!state.check("s1"), "关闭后不得放行");
}

/// set 返回 (since, count)：开=非空 since+计数清零；关=空 since+保留计数。
#[test]
fn set_returns_since_and_count() {
    let state = SessionAllowAllState::new();
    state.set("s1", true);
    state.bump("s1");
    state.bump("s1");

    let (since_on, count_on) = state.set("s1", true);
    assert!(!since_on.is_empty(), "重开给新 since");
    assert_eq!(count_on, 0, "重开计数清零（「本次」语义）");

    state.bump("s1");
    let (since_off, count_off) = state.set("s1", false);
    assert!(since_off.is_empty(), "关=空 since");
    assert_eq!(count_off, 1, "关闭保留本期计数供回看");
}

/// status 快照三字段；无条目 None；关闭后 enabled=false 但 count 保留。
#[test]
fn status_snapshot_shape() {
    let state = SessionAllowAllState::new();
    assert!(state.status("s1").is_none());

    state.set("s1", true);
    state.bump("s1");
    let (enabled, since, count) = state.status("s1").unwrap();
    assert!(enabled);
    assert!(!since.is_empty());
    assert_eq!(count, 1);

    state.set("s1", false);
    let (enabled, _, count) = state.status("s1").unwrap();
    assert!(!enabled);
    assert_eq!(count, 1, "关闭不清计数");
}

/// hydrate 种启用条目；已有条目（运行期 set 过）不被覆盖（or_insert 语义）。
#[test]
fn hydrate_seeds_without_overwriting_runtime_entries() {
    let state = SessionAllowAllState::new();
    state.hydrate(vec![
        ("s1".to_string(), "2026-10-24T08:00:00+08:00".to_string()),
        ("s2".to_string(), String::new()),
    ]);
    assert!(state.check("s1"));
    assert!(state.check("s2"));
    assert_eq!(
        state.status("s1").unwrap().1,
        "2026-10-24T08:00:00+08:00",
        "hydrate 保留 sidecar since"
    );

    // 运行期关掉的会话，重复 hydrate 不得复活（启动只跑一次，防御性钉死）。
    state.set("s1", false);
    state.hydrate(vec![("s1".to_string(), "x".to_string())]);
    assert!(!state.check("s1"), "hydrate 不得覆盖运行期状态");
}

/// enabled_keys 只列启用中的会话。
#[test]
fn enabled_keys_lists_only_active() {
    let state = SessionAllowAllState::new();
    state.set("a", true);
    state.set("b", true);
    state.set("c", false); // 关闭态条目
    let mut keys = state.enabled_keys();
    keys.sort();
    assert_eq!(keys, vec!["a".to_string(), "b".to_string()]);
}
