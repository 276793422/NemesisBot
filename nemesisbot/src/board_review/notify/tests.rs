//! 停车推送（批三）发射面门控测试：enabled 齐备才调 hook / 关或目标空
//! 不调 / hook 未注册 no-op。hook 是进程级全局槽，全部测试持
//! HOOK_TEST_LOCK 串行（防并行互扰——A 的 hook 收到 B 的 emit）。

use super::{HOOK_TEST_LOCK, ParkNotice, emit_park_notice, set_park_notify_hook};
use std::sync::{Arc, Mutex};

fn notice() -> ParkNotice {
    ParkNotice {
        issue_number: "NB-7".into(),
        issue_id: 7,
        stop_kind: "fail_streak",
        suggested_action: "manual_check",
        hint: "同签名 3 连".into(),
        resume_entry: "review.retrigger".into(),
    }
}

/// temp home + `board.notify` 配置写入。
fn home_with_notify(json: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "nemesis-board-notify-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.json"), format!(r#"{{"board":{json}}}"#)).unwrap();
    dir
}

type Collected = Arc<Mutex<Vec<(String, String, String, String)>>>;

fn install_collector() -> Collected {
    let collected: Collected = Arc::new(Mutex::new(Vec::new()));
    let sink = collected.clone();
    set_park_notify_hook(Some(Arc::new(
        move |n: &ParkNotice, channel: &str, chat_id: &str| {
            sink.lock().unwrap().push((
                n.issue_number.clone(),
                n.stop_kind.to_string(),
                channel.to_string(),
                chat_id.to_string(),
            ));
        },
    )));
    collected
}

#[test]
fn emit_fires_hook_when_enabled_and_targets_set() {
    let _guard = HOOK_TEST_LOCK.blocking_lock();
    let home = home_with_notify(
        r#"{"notify": {"enabled": true, "channel": "telegram", "chat_id": "42"}}"#,
    );
    let collected = install_collector();

    emit_park_notice(&home, &notice());

    let got = collected.lock().unwrap();
    assert_eq!(got.len(), 1, "enabled+目标齐备必须触发一次");
    assert_eq!(got[0].0, "NB-7");
    assert_eq!(got[0].1, "fail_streak");
    assert_eq!(got[0].2, "telegram");
    assert_eq!(got[0].3, "42");
    set_park_notify_hook(None);
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn emit_is_noop_when_disabled() {
    let _guard = HOOK_TEST_LOCK.blocking_lock();
    let home = home_with_notify(
        r#"{"notify": {"enabled": false, "channel": "telegram", "chat_id": "42"}}"#,
    );
    let collected = install_collector();

    emit_park_notice(&home, &notice());

    assert!(
        collected.lock().unwrap().is_empty(),
        "disabled = 不触发（默认关，现网行为不变）"
    );
    set_park_notify_hook(None);
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn emit_is_noop_when_channel_or_chat_id_empty() {
    let _guard = HOOK_TEST_LOCK.blocking_lock();
    let home = home_with_notify(r#"{"notify": {"enabled": true}}"#);
    let collected = install_collector();

    emit_park_notice(&home, &notice());

    assert!(
        collected.lock().unwrap().is_empty(),
        "enabled 但目标为空 = 不触发（诚实 no-op）"
    );
    set_park_notify_hook(None);
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn emit_is_noop_without_hook_registered() {
    let _guard = HOOK_TEST_LOCK.blocking_lock();
    set_park_notify_hook(None);
    let home =
        home_with_notify(r#"{"notify": {"enabled": true, "channel": "webhook", "chat_id": "x"}}"#);

    // 不 panic 即通过（hook 缺席 = 发布面未装配，发射面静默 no-op）。
    emit_park_notice(&home, &notice());

    let _ = std::fs::remove_dir_all(&home);
}
