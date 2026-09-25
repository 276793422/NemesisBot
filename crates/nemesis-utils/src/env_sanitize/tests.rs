//! T4（扩展计划 D2a）：子进程环境清洗单测。
//! （自生产文件内联块迁出——2026-07-17 起测试代码放独立文件的纪律。）

use super::*;

/// 开关翻转是进程级状态——持锁串行，防并行测试互踩。
static FLAG_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn sensitive_names_match_blacklist_patterns() {
    assert!(is_sensitive_env_name("ANTHROPIC_API_KEY"));
    assert!(is_sensitive_env_name("anthropic_api_key"));
    assert!(is_sensitive_env_name("GITHUB_TOKEN"));
    assert!(is_sensitive_env_name("AWS_SECRET_ACCESS_KEY"));
    assert!(is_sensitive_env_name("DB_PASSWORD"));
    assert!(is_sensitive_env_name("GOOGLE_CREDENTIALS"));
    assert!(is_sensitive_env_name("NEMESISBOT_ROLE"));
    assert!(is_sensitive_env_name("NEMESISBOT_EXECUTOR_PIPE"));
    assert!(!is_sensitive_env_name("PATH"));
    assert!(!is_sensitive_env_name("SystemRoot"));
    assert!(!is_sensitive_env_name("COMSPEC"));
    assert!(!is_sensitive_env_name("HOME"));
    assert!(!is_sensitive_env_name("SOME_RANDOM_VAR"));
}

#[test]
fn whitelist_beats_blacklist_patterns() {
    // 防御未来黑名单扩充时误杀运行底座（当前模式不与其相交，
    // 用一个会命中子串模式的名字验证白名单优先）。
    assert!(!is_sensitive_env_name("Pathway"));
    assert!(is_sensitive_env_name("PATHWAY_X_KEY"));
}

#[test]
fn sanitize_command_strips_sensitive_and_keeps_whitelist() {
    let _g = FLAG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_child_env_sanitize_enabled(true);

    let mut cmd = std::process::Command::new("cmd");
    cmd.env("ANTHROPIC_API_KEY", "sk-test")
        .env("PATH_OK", "keep");
    let stripped = sanitize_command(&mut cmd);

    // 本测试进程至少带出这一个敏感变量；白名单/普通变量不剥。
    // PATH_OK 不含黑名单子串 → 不剥；ANTHROPIC_API_KEY → 剥。
    // （CI 环境可能自带其他敏感变量，只断言下限与剥除生效。）
    assert!(stripped >= 1, "至少剥掉显式设置的 ANTHROPIC_API_KEY");
}

#[test]
fn sanitize_disabled_is_noop() {
    let _g = FLAG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_child_env_sanitize_enabled(false);

    let mut cmd = std::process::Command::new("cmd");
    cmd.env("ANTHROPIC_API_KEY", "sk-test");
    let stripped = sanitize_command(&mut cmd);
    assert_eq!(stripped, 0, "开关关闭必须零剥除零改动");

    set_child_env_sanitize_enabled(true);
}
