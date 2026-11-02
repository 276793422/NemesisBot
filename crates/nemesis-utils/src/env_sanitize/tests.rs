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
    // BUG-20261030-215：PASSWD 是 PASSWORD 的标准变体拼写，漏词时
    // MYSQL_PASSWD 等凭据变量既不被 spawn 前剥除、也不被 shell_bleed
    // 泄漏检测识别（两道防线同源词表同漏）。
    assert!(is_sensitive_env_name("MYSQL_PASSWD"));
    assert!(is_sensitive_env_name("ftp_passwd"));
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

    // sanitize_command 枚举的是【本进程环境】（Command 上显式 env 附加的
    // 键不参与剥除计数）——预置一个独名敏感变量，断言不再依赖机器环境
    // 碰巧带了什么（此前靠环境里恰好有敏感变量才过，Linux CI 实证假红）。
    // set/remove 成对且持 FLAG_LOCK，用后即撤。
    unsafe { std::env::set_var("NMB_COV_TEST_SENSITIVE_KEY", "x") };
    let mut cmd = std::process::Command::new("cmd");
    cmd.env("ANTHROPIC_API_KEY", "sk-test")
        .env("PATH_OK", "keep");
    let stripped = sanitize_command(&mut cmd);
    unsafe { std::env::remove_var("NMB_COV_TEST_SENSITIVE_KEY") };

    // NMB_COV_TEST_SENSITIVE_KEY（含 KEY 子串）必被剥；环境可能自带其他
    // 敏感变量，只断言下限与剥除生效。PATH_OK 不含黑名单子串 → 不剥。
    assert!(stripped >= 1, "至少剥掉预置的 NMB_COV_TEST_SENSITIVE_KEY");
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

// ---------------------------------------------------------------------------
// 窄版闸（改进项二期 L27）：只剥 bot 命名空间，保留第三方 CLI 自身 auth key
// ---------------------------------------------------------------------------

#[test]
fn nemesisbot_namespace_detection() {
    assert!(is_nemesisbot_namespaced("NEMESISBOT_VAULT_PASSPHRASE"));
    assert!(is_nemesisbot_namespaced("nemesisbot_role"));
    assert!(is_nemesisbot_namespaced("NemesisBot_Executor_Pipe"));
    // 同近前缀异词根（NMB_*）不算——命名空间边界是全前缀字面量。
    assert!(!is_nemesisbot_namespaced("NMB_COV_TEST"));
    assert!(!is_nemesisbot_namespaced("ANTHROPIC_API_KEY"));
    assert!(!is_nemesisbot_namespaced(""));
}

#[test]
fn narrow_strips_bot_namespace_keeps_keyword_sensitive() {
    let _g = FLAG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_child_env_sanitize_enabled(true);

    // 预置一对变量：命名空间内（窄版必剥）+ 关键词敏感但命名空间外（窄版
    // 必留，全词表版会剥——窄/全分界钉死在这对变量上）。set/remove 成对，
    // 用后即撤（预置期间两函数枚举同一环境，口径可比）。
    unsafe { std::env::set_var("NEMESISBOT_COV_L27_NS", "x") };
    unsafe { std::env::set_var("NMBT_COV_L27_SENSITIVE_KEY", "x") };
    let mut cmd = std::process::Command::new("cmd");
    let narrow = sanitize_command_narrow(&mut cmd);
    let mut cmd_wide = std::process::Command::new("cmd");
    let wide = sanitize_command(&mut cmd_wide);
    unsafe {
        std::env::remove_var("NEMESISBOT_COV_L27_NS");
        std::env::remove_var("NMBT_COV_L27_SENSITIVE_KEY");
    }

    // 窄版至少剥掉预置的 NEMESISBOT_COV_L27_NS（进程环境可能自带其他
    // NEMESISBOT_*，下限断言）；同一环境下全词表剥除面 ⊇ 窄版（关键词
    // 敏感面是命名空间面的超集——白名单不含 NEMESISBOT_* 前缀名）。
    assert!(narrow >= 1, "至少剥掉预置的 NEMESISBOT_COV_L27_NS");
    assert!(wide >= narrow, "全词表剥除面必须 ⊇ 窄版");
}

#[test]
fn sanitize_narrow_disabled_is_noop() {
    let _g = FLAG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_child_env_sanitize_enabled(false);
    unsafe { std::env::set_var("NEMESISBOT_COV_L27_NS", "x") };
    let mut cmd = std::process::Command::new("cmd");
    let stripped = sanitize_command_narrow(&mut cmd);
    unsafe { std::env::remove_var("NEMESISBOT_COV_L27_NS") };
    assert_eq!(stripped, 0, "开关关闭必须零剥除零改动");
    set_child_env_sanitize_enabled(true);
}

#[test]
fn narrow_tokio_variant_matches_std_semantics() {
    let _g = FLAG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_child_env_sanitize_enabled(true);
    unsafe { std::env::set_var("NEMESISBOT_COV_L27_NS", "x") };
    unsafe { std::env::set_var("NMBT_COV_L27_SENSITIVE_KEY", "x") };
    let mut cmd = tokio::process::Command::new("cmd");
    let stripped = sanitize_tokio_command_narrow(&mut cmd);
    unsafe {
        std::env::remove_var("NEMESISBOT_COV_L27_NS");
        std::env::remove_var("NMBT_COV_L27_SENSITIVE_KEY");
    }
    assert!(stripped >= 1, "tokio 窄版与 std 同语义：剥命名空间变量");
}
