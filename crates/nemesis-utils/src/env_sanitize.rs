//! 子进程环境凭据清洗（扩展计划 T4 / D2a）。
//!
//! LLM 驱动的 exec 类工具 spawn 子进程时环境**默认全继承**——agent 进程里
//! 可见的模型 API key / vault 口令等凭据原样进入子进程环境（`exec` 里
//! `echo %ANTHROPIC_API_KEY%` 即可读出）。本模块在 spawn 前按黑名单剥敏感
//! 变量名；`security.sanitize_child_env` 可关（默认开，逃生值）。
//!
//! 白名单是安全底座（PATH/SYSTEMROOT/COMSPEC 等——Windows `cmd /C` 缺了
//! 起不来）：当前黑名单模式均不与其相交，列入只为防御未来模式扩充时误杀
//! 运行底座。

use std::sync::atomic::{AtomicBool, Ordering};

/// 进程级开关：`security.sanitize_child_env`（默认开）。装配期由
/// security_setup 按配置同步；三处 spawn 点（agent loop 的 exec/run_checks
/// 共用闸 + nemesis-tools shell/async_shell）直接查本静态——免逐工具穿
/// config（工具结构体无 config 通路）。
static CHILD_ENV_SANITIZE_ENABLED: AtomicBool = AtomicBool::new(true);

/// 装配期同步开关（security_setup.rs 构造期调用）。
pub fn set_child_env_sanitize_enabled(enabled: bool) {
    CHILD_ENV_SANITIZE_ENABLED.store(enabled, Ordering::Relaxed);
}

/// 查询当前开关。
pub fn child_env_sanitize_enabled() -> bool {
    CHILD_ENV_SANITIZE_ENABLED.load(Ordering::Relaxed)
}

/// 白名单：运行底座变量，永不剥（全大写精确匹配）。
const WHITELIST: &[&str] = &[
    "PATH",
    "PATHEXT",
    "HOME",
    "HOMEDRIVE",
    "HOMEPATH",
    "SYSTEMROOT",
    "SYSTEMDRIVE",
    "COMSPEC",
    "WINDIR",
    "TMP",
    "TEMP",
    "PWD",
    "LANG",
    "TERM",
];

/// 黑名单子串（大小写不敏感）：变量名含任一即剥。
const BLACKLIST_SUBSTRINGS: &[&str] = &["KEY", "TOKEN", "SECRET", "PASSWORD", "CREDENTIAL"];

/// 变量名是否敏感（应剥）。大小写不敏感；白名单优先。
pub fn is_sensitive_env_name(name: &str) -> bool {
    let upper = name.to_uppercase();
    if WHITELIST.iter().any(|w| *w == upper) {
        return false;
    }
    // 执行体运输层内部变量（NEMESISBOT_ROLE / EXECUTOR_WORKSPACE /
    // EXECUTOR_PIPE 等）一律剥——子进程不需要知道自己是执行体。
    if upper.starts_with("NEMESISBOT_") {
        return true;
    }
    BLACKLIST_SUBSTRINGS.iter().any(|s| upper.contains(s))
}

/// std Command 清洗：枚举本进程环境，敏感变量逐个 env_remove。
/// 返回剥除数（开关关时恒 0，且不做任何改动）。
///
/// 用 `vars_os` 而非 `vars`：后者遇非 UTF-8 变量名会 panic——spawn 路径
/// 不引入 panic 面；非 UTF-8 名按 lossy 形态匹配后以 OsStr 原样剥除。
pub fn sanitize_command(cmd: &mut std::process::Command) -> usize {
    if !child_env_sanitize_enabled() {
        return 0;
    }
    let mut stripped = 0;
    for (name, _) in std::env::vars_os() {
        if is_sensitive_env_name(&name.to_string_lossy()) {
            cmd.env_remove(&name);
            stripped += 1;
        }
    }
    stripped
}

/// tokio Command 清洗（同 [`sanitize_command`]）。
pub fn sanitize_tokio_command(cmd: &mut tokio::process::Command) -> usize {
    if !child_env_sanitize_enabled() {
        return 0;
    }
    let mut stripped = 0;
    for (name, _) in std::env::vars_os() {
        if is_sensitive_env_name(&name.to_string_lossy()) {
            cmd.env_remove(&name);
            stripped += 1;
        }
    }
    stripped
}

#[cfg(test)]
mod tests;
