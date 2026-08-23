//! Tests for the shared CLI-delegation layer (T5 / U13 A7+A9).

use super::*;

#[test]
fn test_resolve_cc_permission_mode_tiers() {
    // Explicit tiers pass through.
    assert_eq!(resolve_cc_permission_mode("default"), "default");
    assert_eq!(resolve_cc_permission_mode("accept_edits"), "accept_edits");
    assert_eq!(resolve_cc_permission_mode("plan"), "plan");
    assert_eq!(
        resolve_cc_permission_mode("bypass_permissions"),
        "bypass_permissions"
    );
    // Empty (absent config) and unknown values fall back to the default.
    assert_eq!(resolve_cc_permission_mode(""), "accept_edits");
    assert_eq!(resolve_cc_permission_mode("yolo"), "accept_edits");
    // Case-sensitive: a typo'd casing is unknown, not accepted.
    assert_eq!(resolve_cc_permission_mode("Plan"), "accept_edits");
}

#[test]
fn test_resolve_codex_sandbox_and_kebab_mapping() {
    assert_eq!(resolve_codex_sandbox("read_only"), "read_only");
    assert_eq!(resolve_codex_sandbox("workspace_write"), "workspace_write");
    assert_eq!(resolve_codex_sandbox("danger_full_access"), "danger_full_access");
    assert_eq!(resolve_codex_sandbox(""), "read_only");
    assert_eq!(resolve_codex_sandbox("full"), "read_only");

    // snake_case (config) → kebab-case (codex CLI flag value).
    assert_eq!(codex_sandbox_kebab("read_only"), "read-only");
    assert_eq!(codex_sandbox_kebab("workspace_write"), "workspace-write");
    assert_eq!(codex_sandbox_kebab("danger_full_access"), "danger-full-access");
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
            std::fs::write(&p, format!("@echo off\r\necho %* > \"{}\"\r\necho FAKE_RESULT\r\n", m))
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
