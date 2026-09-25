// install.rs 覆盖率补充测试（TypeScript 安装命令臂 63-68、run_install
// spawn NotFound 的诚实文案 158）。
//
// 平台豁免：76-81（Python 非 Windows npm 形态）/ 97-102（C 的 sudo apt
// 形态）带 cfg!(windows) 反向守卫，Windows 测试进程不可达；235-247
// auto_install_missing 会真实执行 npm/winget 安装器（环境副作用），不测。

use super::*;
use std::time::Duration;

/// TypeScript 的安装命令四元组（63-68）。
#[test]
fn install_command_typescript_uses_global_npm() {
    let cmd = install_command(Lang::TypeScript);
    // Windows 上 npm 走 cmd /C 包装（.cmd 不是 PE）。
    if cfg!(windows) {
        assert_eq!(cmd.program, "cmd");
        assert_eq!(
            cmd.args,
            vec![
                "/C",
                "npm",
                "install",
                "-g",
                "typescript",
                "typescript-language-server"
            ]
        );
    } else {
        assert_eq!(cmd.program, "npm");
        assert_eq!(
            cmd.args,
            vec!["install", "-g", "typescript", "typescript-language-server"]
        );
    }
    assert!(cmd.display.contains("npm install -g"), "{}", cmd.display);
    assert!(!cmd.needs_interactive);
}

/// 不存在的程序 → run_process 的 NotFound 诚实文案（158）。
#[tokio::test]
async fn run_install_reports_program_not_found() {
    let cmd = InstallCommand {
        display: "nmb-cov-nonexistent-lsp --version".to_string(),
        program: "nmb-cov-nonexistent-lsp-bin".to_string(),
        args: vec!["--version".to_string()],
        needs_interactive: false,
    };
    let err = run_install(&cmd, Duration::from_secs(10))
        .await
        .unwrap_err();
    assert!(err.contains("program not found"), "{err}");
    assert!(err.contains("nmb-cov-nonexistent-lsp-bin"), "{err}");
}

/// spawn 失败但**不是** NotFound（程序路径是个目录 → PermissionDenied）→
/// 走 "spawn {program} failed" 诚实文案臂（157-158）。
#[tokio::test]
async fn run_process_reports_non_notfound_spawn_error() {
    let dir = tempfile::tempdir().unwrap();
    let cmd = InstallCommand {
        display: dir.path().display().to_string(),
        program: dir.path().display().to_string(),
        args: vec![],
        needs_interactive: false,
    };
    let err = run_install(&cmd, Duration::from_secs(10))
        .await
        .unwrap_err();
    assert!(err.contains("spawn"), "{err}");
    assert!(err.contains("failed"), "{err}");
}
