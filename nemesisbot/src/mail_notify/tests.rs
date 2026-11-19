//! mail_notify 单测（安装门控 + 白名单闸——插件缺席即发信失败，warn 路径
//! 不可断言；本文件钉死的是**无插件也可断言的静态槽契约**）。

use std::path::PathBuf;

use super::*;

fn temp_home_with_config(body: &str) -> (tempfile::TempDir, PathBuf) {
    let home = tempfile::tempdir().unwrap();
    let cfg_dir = home.path().join("workspace").join("config");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    let path = cfg_dir.join("config.mail.json");
    std::fs::write(&path, body).unwrap();
    (home, path)
}

/// 收尾摘钩子（static 跨测试共享——不留钩子污染后续用例）。
fn reset_hook() {
    *HOOK.write() = None;
}

/// 静态槽跨测试共享 → 用例串行（background_registry TEST_LOCK 同款纪律——
/// cargo 并行调度下 A 用例 install 后 B 用例的 is_none 断言会吃到 A 的钩子）。
static TEST_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

#[test]
fn install_absent_section_stays_silent() {
    let _serial = TEST_LOCK.lock();
    reset_hook();
    // 缺 config.mail.json = notify 缺段 = 全关（缺省静默，零行为差异）。
    let home = tempfile::tempdir().unwrap();
    install(home.path());
    assert!(HOOK.read().is_none(), "缺 notify 段不应安装钩子");
    // 有 config.mail.json 但无 notify 段 = 同款。
    let (h, _) = temp_home_with_config(r#"{"enabled":true}"#);
    install(h.path());
    assert!(HOOK.read().is_none(), "无 notify 段不应安装钩子");
    // notify 段在但 on 空 = 同款。
    let (h, _) = temp_home_with_config(r#"{"enabled":true,"notify":{"on":[],"to":"ops@x"}}"#);
    install(h.path());
    assert!(HOOK.read().is_none(), "on 空数组不应安装钩子");
    reset_hook();
}

#[test]
fn install_on_without_to_warns_and_skips() {
    let _serial = TEST_LOCK.lock();
    reset_hook();
    // on 有值但 to 空 = 安装时 warn 跳过（发信需收件人）。
    let (h, _) = temp_home_with_config(r#"{"enabled":true,"notify":{"on":["estop"],"to":""}}"#);
    install(h.path());
    assert!(HOOK.read().is_none(), "to 空不应安装钩子");
    // to 缺键 = 同款。
    let (h, _) = temp_home_with_config(r#"{"enabled":true,"notify":{"on":["estop"]}}"#);
    install(h.path());
    assert!(HOOK.read().is_none(), "to 缺键不应安装钩子");
    reset_hook();
}

#[test]
fn install_whitelist_gates_unknown_kind() {
    let _serial = TEST_LOCK.lock();
    reset_hook();
    let (h, _) = temp_home_with_config(
        r#"{"enabled":true,"notify":{"on":["estop"],"to":"ops@corp.example"}}"#,
    );
    install(h.path());
    assert!(HOOK.read().is_some(), "on+to 齐全应安装钩子");
    // 白名单外 kind：闸在 Handle::try_current 之前——无 runtime 的同步测试
    // 上下文里调用不炸也不落「无 runtime」warn（静默 no-op）。
    try_notify("unknown_kind", "subj", "body");
    // 白名单内 kind 但无 runtime：诚实 warn 跳过（不炸调用方——熔断/停车
    // 主流程不被邮件阻塞的兜底臂）。
    try_notify("estop", "急停测试", "body");
    reset_hook();
}

#[tokio::test]
async fn try_notify_whitelisted_spawns_and_survives_plugin_absent() {
    let _serial = TEST_LOCK.lock();
    reset_hook();
    let (h, _) = temp_home_with_config(
        r#"{"enabled":true,"notify":{"on":["estop","board_budget"],"to":"ops@corp.example"}}"#,
    );
    install(h.path());
    // 插件缺席 → call_mail 诚实 Err → spawn 内 warn（不炸、不 panic）——
    // 尽力而为语义。非白名单 kind 在闸处短路（连 spawn 都不发）。
    try_notify("estop", "⛔ NemesisBot 急停触发", "test body");
    try_notify("board_budget", "看板预算熔断", "test body");
    try_notify("not_in_list", "不应发出", "body");
    reset_hook();
}
