//! Tests for the shared CLI-delegation layer (T5 / U13 A7+A9).

use super::*;

#[test]
fn test_resolve_cc_permission_mode_tiers() {
    // Exact camelCase values (claude CLI 2.1.240 实测合法集) pass through.
    assert_eq!(resolve_cc_permission_mode("acceptEdits"), "acceptEdits");
    assert_eq!(resolve_cc_permission_mode("auto"), "auto");
    assert_eq!(
        resolve_cc_permission_mode("bypassPermissions"),
        "bypassPermissions"
    );
    assert_eq!(resolve_cc_permission_mode("manual"), "manual");
    assert_eq!(resolve_cc_permission_mode("dontAsk"), "dontAsk");
    assert_eq!(resolve_cc_permission_mode("plan"), "plan");
    // Legacy snake_case（T5 时代错误值集）→ camelCase 映射。
    assert_eq!(resolve_cc_permission_mode("accept_edits"), "acceptEdits");
    assert_eq!(resolve_cc_permission_mode("default"), "acceptEdits");
    assert_eq!(
        resolve_cc_permission_mode("bypass_permissions"),
        "bypassPermissions"
    );
    // Empty (absent config) and unknown values fall back to the default.
    assert_eq!(resolve_cc_permission_mode(""), "acceptEdits");
    assert_eq!(resolve_cc_permission_mode("yolo"), "acceptEdits");
    // Case-sensitive: a typo'd casing is unknown, not accepted.
    assert_eq!(resolve_cc_permission_mode("AcceptEdits"), "acceptEdits");
}

#[test]
fn test_resolve_codex_sandbox_and_kebab_mapping() {
    assert_eq!(resolve_codex_sandbox("read_only"), "read_only");
    assert_eq!(resolve_codex_sandbox("workspace_write"), "workspace_write");
    assert_eq!(
        resolve_codex_sandbox("danger_full_access"),
        "danger_full_access"
    );
    assert_eq!(resolve_codex_sandbox(""), "read_only");
    assert_eq!(resolve_codex_sandbox("full"), "read_only");

    // snake_case (config) → kebab-case (codex CLI flag value).
    assert_eq!(codex_sandbox_kebab("read_only"), "read-only");
    assert_eq!(codex_sandbox_kebab("workspace_write"), "workspace-write");
    assert_eq!(
        codex_sandbox_kebab("danger_full_access"),
        "danger-full-access"
    );
    // Unknown input maps to the default tier's CLI form.
    assert_eq!(codex_sandbox_kebab("garbage"), "read-only");
}

#[test]
fn test_delegation_cwd_fallbacks() {
    // Non-path session key → process cwd (no panic).
    let cwd = delegation_cwd("web:chat123");
    assert!(cwd.is_dir(), "resolved cwd must exist: {:?}", cwd);

    // Session key whose parent EXISTS on disk → that parent.
    let dir = tempfile::tempdir().unwrap();
    let key = dir.path().join("session").to_string_lossy().to_string();
    let cwd = delegation_cwd(&key);
    assert_eq!(cwd, dir.path());
}

/// A fake CLI that echoes its args to a marker file and prints a fixed
/// result — lets arg-assembly tests assert what the child actually received.
pub(crate) struct FakeArgEchoCli {
    pub script: std::path::PathBuf,
    pub marker: std::path::PathBuf,
}

impl FakeArgEchoCli {
    /// `name` only affects the temp file name (fake_claude.bat etc).
    pub(crate) fn new(dir: &Path, name: &str) -> Self {
        let marker = dir.join(format!("{}_args.txt", name));
        let script = if cfg!(windows) {
            let p = dir.join(format!("{}.bat", name));
            let m = marker.to_string_lossy().replace('\\', "/");
            std::fs::write(
                &p,
                format!("@echo off\r\necho %* > \"{}\"\r\necho FAKE_RESULT\r\n", m),
            )
            .unwrap();
            p
        } else {
            let p = dir.join(format!("{}.sh", name));
            std::fs::write(
                &p,
                format!(
                    "#!/bin/sh\necho \"$@\" > \"{}\"\necho FAKE_RESULT\n",
                    marker.to_string_lossy()
                ),
            )
            .unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            p
        };
        Self { script, marker }
    }

    pub(crate) fn cli_path(&self) -> String {
        self.script.to_string_lossy().to_string()
    }

    pub(crate) fn received_args(&self) -> String {
        std::fs::read_to_string(&self.marker).unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// find_cli_on_path Windows 候选挑选（V4 真机 bug 的回归测试）
// ---------------------------------------------------------------------------

/// npm 布局：`where claude` 第一行是无扩展名 POSIX shim（Windows 上
/// `Command::new` 必 os error 193），第二行才是可 spawn 的 `.cmd`。
/// pick_windows_exec_candidate 必须跳过 shim 选 `.cmd`。
#[test]
#[cfg(windows)]
fn test_pick_windows_exec_candidate_prefers_cmd_over_shim() {
    let lines = [r"C:\AI\node\claude", r"C:\AI\node\claude.cmd"];
    assert_eq!(
        pick_windows_exec_candidate(&lines),
        Some(r"C:\AI\node\claude.cmd".to_string())
    );
}

/// exe 优先级与大小写：`.EXE` 大写扩展名同样可执行。
#[test]
#[cfg(windows)]
fn test_pick_windows_exec_candidate_accepts_uppercase_ext() {
    let lines = [r"C:\tools\shim\claude", r"C:\Windows\CLAUDE.EXE"];
    assert_eq!(
        pick_windows_exec_candidate(&lines),
        Some(r"C:\Windows\CLAUDE.EXE".to_string())
    );
}

/// 全部候选都无扩展名 → None（调用方回退第一行，让 spawn 错误如实暴露，
/// 而不是误报「未安装」）。
#[test]
#[cfg(windows)]
fn test_pick_windows_exec_candidate_all_shims_returns_none() {
    let lines = [r"C:\a\claude", r"C:\b\claude"];
    assert_eq!(pick_windows_exec_candidate(&lines), None);
}

/// 第一个带可执行扩展名的候选命中（保持 `where` 的顺序语义）。
#[test]
#[cfg(windows)]
fn test_pick_windows_exec_candidate_first_exec_wins() {
    let lines = [r"C:\x\claude", r"C:\y\claude.bat", r"C:\z\claude.exe"];
    assert_eq!(
        pick_windows_exec_candidate(&lines),
        Some(r"C:\y\claude.bat".to_string())
    );
    let lines2 = [r"C:\x\claude.exe", r"C:\y\claude.bat"];
    assert_eq!(
        pick_windows_exec_candidate(&lines2),
        Some(r"C:\x\claude.exe".to_string())
    );
}

// ---------------------------------------------------------------------------
// run_cli_delegation 执行结果三臂 + find_cli_on_path 负例（M7 补测）
// ---------------------------------------------------------------------------

/// 非零退出是 **Ok**（工具层契约：CLI 失败也要把 stdout/stderr 原样带回
/// 给模型，让它自己看到 CLI 的报错，而不是整轮工具调用失败）。
#[tokio::test]
async fn test_run_cli_delegation_nonzero_exit_returns_structured_ok() {
    let dir = tempfile::tempdir().unwrap();
    // fake CLI: 打一行 stdout 再 exit 2
    let script = if cfg!(windows) {
        let p = dir.path().join("fake_fail.bat");
        std::fs::write(
            &p,
            "@echo off\r\necho boom on stdout\r\necho boom on stderr 1>&2\r\nexit /b 2\r\n",
        )
        .unwrap();
        p
    } else {
        let p = dir.path().join("fake_fail.sh");
        std::fs::write(
            &p,
            "#!/bin/sh\necho boom on stdout\necho boom on stderr 1>&2\nexit 2\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        p
    };
    let spec = CliDelegationSpec {
        cli: &script.to_string_lossy(),
        cli_label: "fake-fail",
        args: vec![],
        timeout_secs: 30,
        cwd: dir.path(),
    };
    let out = run_cli_delegation(spec).await.unwrap();
    assert!(out.starts_with("Error: fake-fail exited with"), "{out}");
    assert!(out.contains("boom on stdout"), "{out}");
    assert!(out.contains("boom on stderr"), "{out}");
}

/// spawn 失败（可执行文件不存在）→ Err，报 spawn 而非超时/退出码。
#[tokio::test]
async fn test_run_cli_delegation_spawn_failure_is_err() {
    let missing = if cfg!(windows) {
        r"Z:\definitely\missing\cli.xyz"
    } else {
        "/definitely/missing/cli.xyz"
    };
    let spec = CliDelegationSpec {
        cli: missing,
        cli_label: "ghost-cli",
        args: vec![],
        timeout_secs: 30,
        cwd: std::path::Path::new("."),
    };
    let err = run_cli_delegation(spec).await.unwrap_err();
    assert!(err.contains("failed to spawn ghost-cli"), "{err}");
}

/// PATH 上没有的命令 → None（注册层的「未安装就不注册工具」依据）。
#[test]
fn test_find_cli_on_path_missing_command_is_none() {
    assert!(find_cli_on_path("nemesis-no-such-cli-xyz").is_none());
}
