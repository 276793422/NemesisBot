//! C6 install 模块测试：目录覆盖不变量（平台无关）+ run_process 平台孪生
//! 测试（2026-09-02 纪律：Windows 形态测试挂 `#[cfg(windows)]`——Linux 上
//! 编译期消失而非运行期跳过）。

use std::time::Duration;

use super::{InstallCommand, install_command, pending_auto_installs, run_install, run_process};
use crate::registry::{Lang, SERVERS};

/// 目录覆盖全部注册语言，且字段非空。
#[test]
fn install_command_covers_every_registry_language() {
    for spec in SERVERS {
        let cmd = install_command(spec.lang);
        assert!(
            !cmd.display.is_empty(),
            "{}: empty display",
            spec.lang.label()
        );
        assert!(
            !cmd.program.is_empty(),
            "{}: empty program",
            spec.lang.label()
        );
        assert!(!cmd.args.is_empty(), "{}: empty args", spec.lang.label());
    }
}

/// 四个语言全平台非交互；C（clangd）只在 Windows（winget）非交互，
/// Unix 走 sudo = 交互（一键执行诚实拒绝）。
#[test]
fn install_command_interactive_matrix() {
    for lang in [Lang::Rust, Lang::Go, Lang::TypeScript, Lang::Python] {
        assert!(
            !install_command(lang).needs_interactive,
            "{} should be non-interactive",
            lang.label()
        );
    }
    assert_eq!(install_command(Lang::C).needs_interactive, !cfg!(windows));
}

/// 目录命令都是官方安装通道（白名单锚点——新增语言必须显式扩这里）。
#[test]
fn install_command_program_whitelist() {
    for spec in SERVERS {
        let cmd = install_command(spec.lang);
        let leaf = cmd.program.rsplit(['\\', '/']).next().unwrap();
        let ok = match leaf {
            "rustup" | "go" | "winget" | "sudo" => true,
            // Windows 上 npm 经 cmd /C 包装；其他平台直呼 npm。
            "npm" | "cmd" => true,
            _ => false,
        };
        assert!(
            ok,
            "{}: unexpected installer program `{}`",
            spec.lang.label(),
            cmd.program
        );
    }
}

/// Windows 上 npm 条目必须经 cmd /C 包装（.cmd 不是 PE）。
#[cfg(windows)]
#[test]
fn install_command_npm_wrapped_in_cmd_on_windows() {
    for lang in [Lang::TypeScript, Lang::Python] {
        let cmd = install_command(lang);
        assert_eq!(cmd.program, "cmd");
        assert_eq!(cmd.args.first().map(String::as_str), Some("/C"));
    }
}

/// pending_auto_installs 的构造不变量：只含注册语言且全部非交互。
/// （「缺失」属性由实现里的单次 probe filter 保证——这里**不做第二次
/// live probe 复核**：满载下 which() 有瞬时失败可能，两次探测不一致就是
/// 假红，首版双探针断言在全量套件并行下抓过一次。）
#[test]
fn pending_auto_installs_is_noninteractive_registry_subset() {
    for (lang, cmd) in pending_auto_installs() {
        assert!(
            SERVERS.iter().any(|s| s.lang == lang),
            "{lang:?} not in registry"
        );
        assert!(!cmd.needs_interactive, "{lang:?} pending but interactive");
    }
}

fn echo_cmd() -> InstallCommand {
    if cfg!(windows) {
        InstallCommand {
            display: "cmd /C echo nb-c6-ok".into(),
            program: "cmd".into(),
            args: ["/C", "echo", "nb-c6-ok"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            needs_interactive: false,
        }
    } else {
        InstallCommand {
            display: "echo nb-c6-ok".into(),
            program: "echo".into(),
            args: vec!["nb-c6-ok".to_string()],
            needs_interactive: false,
        }
    }
}

/// echo 成功路径：exit 0 + 输出捕获。
#[tokio::test]
async fn run_install_echo_reports_success_with_output() {
    let out = run_install(&echo_cmd(), Duration::from_secs(30))
        .await
        .unwrap();
    assert!(out.contains("installed via"));
    assert!(out.contains("nb-c6-ok"));
}

/// 程序不存在 = 诚实 Err（含程序名）。
#[tokio::test]
async fn run_install_missing_program_is_honest_error() {
    let cmd = InstallCommand {
        display: "nb-c6-no-such-binary".into(),
        program: "nb-c6-no-such-binary".into(),
        args: vec![],
        needs_interactive: false,
    };
    let err = run_install(&cmd, Duration::from_secs(30))
        .await
        .unwrap_err();
    assert!(err.contains("nb-c6-no-such-binary"), "err: {err}");
}

/// 超时 = 诚实 Err（kill_on_drop 杀掉子进程）。平台孪生：
/// Windows `ping -n`、Unix `sleep`。
#[cfg(windows)]
#[tokio::test]
async fn run_install_timeout_is_honest_error() {
    let cmd = InstallCommand {
        display: "ping -n 5 127.0.0.1".into(),
        program: "ping".into(),
        args: ["-n", "5", "127.0.0.1"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        needs_interactive: false,
    };
    let err = run_install(&cmd, Duration::from_millis(300))
        .await
        .unwrap_err();
    assert!(err.contains("timed out"), "err: {err}");
}

#[cfg(not(windows))]
#[tokio::test]
async fn run_install_timeout_is_honest_error() {
    let cmd = InstallCommand {
        display: "sleep 5".into(),
        program: "sleep".into(),
        args: vec!["5".to_string()],
        needs_interactive: false,
    };
    let err = run_install(&cmd, Duration::from_millis(300))
        .await
        .unwrap_err();
    assert!(err.contains("timed out"), "err: {err}");
}

/// 非零退出 = Err 含输出尾（安装器报错原文可见）。
#[tokio::test]
async fn run_install_nonzero_exit_carries_output_tail() {
    let cmd = if cfg!(windows) {
        InstallCommand {
            display: "cmd /C exit 7".into(),
            program: "cmd".into(),
            args: ["/C", "exit", "7"].iter().map(|s| s.to_string()).collect(),
            needs_interactive: false,
        }
    } else {
        InstallCommand {
            display: "false".into(),
            program: "false".into(),
            args: vec![],
            needs_interactive: false,
        }
    };
    let err = run_install(&cmd, Duration::from_secs(30))
        .await
        .unwrap_err();
    assert!(err.contains("exited with"), "err: {err}");
}

/// tail_string：超长截尾保留尾部且不劈多字节字符（CJK 输出场景）。
#[test]
fn tail_string_respects_char_boundaries() {
    let s = "中".repeat(3000); // 9000 字节
    let t = super::tail_string(&s, 4096);
    // 尾段 ≤ max_bytes + (3 字节字符最多回退 2 字节)；"…" 前缀 3 字节。
    assert!(t.len() <= 4096 + 2 + 3, "t.len()={}", t.len());
    assert!(t.contains('…'));
    // 尾段仍是合法 char 序列（不 panic 即边界安全）。
    let _ = t.chars().count();
    // 短文本原样返回。
    assert_eq!(super::tail_string("short", 4096), "short");
}

/// run_process 烟测（直接调，不经 run_install 包装）。
#[tokio::test]
async fn run_process_captures_success() {
    let out = run_process(
        &echo_cmd().program,
        &echo_cmd().args,
        Duration::from_secs(30),
    )
    .await
    .unwrap();
    assert!(out.success);
    assert!(out.output_tail.contains("nb-c6-ok"));
}
