//! macOS Seatbelt（sandbox-exec）包装式后端（U11）。
//!
//! ⚠️ 诚实边界（goal 拍板 2026-08-23）：**无真机验证**。SBPL profile 由
//! `super::seatbelt_profile` 纯函数生成（跨平台单测钉形状），调用形态按
//! Apple `sandbox-exec(1)` 手册实现：`sandbox-exec -p '<profile>' <cmd ...>`。
//! B7 的 mac 半边保留欠账——有 mac 真机后跑与 Linux 同款的写外拒/写内
//! 放行验收再销账。
//!
//! 另注：macOS Sonoma 起 `sandbox-exec` 标记 deprecated（仍可用）；Apple
//! 内部 API 不对第三方开放，Seatbelt 仍是公开可用的唯一系统沙盒入口。

use std::path::PathBuf;
use std::process::Command;

use super::{Availability, SandboxBackend, SandboxConf, seatbelt_profile};

/// Seatbelt 包装式后端（构造时探测 /usr/bin/sandbox-exec，一次）。
pub struct SeatbeltBackend {
    sandbox_exec: Option<PathBuf>,
}

impl SeatbeltBackend {
    pub fn new() -> Self {
        Self {
            sandbox_exec: which_sandbox_exec(),
        }
    }
}

impl Default for SeatbeltBackend {
    fn default() -> Self {
        Self::new()
    }
}

fn which_sandbox_exec() -> Option<PathBuf> {
    for candidate in ["/usr/bin/sandbox-exec"] {
        if std::path::Path::new(candidate).exists() {
            return Some(PathBuf::from(candidate));
        }
    }
    None
}

impl SandboxBackend for SeatbeltBackend {
    fn name(&self) -> &str {
        "seatbelt"
    }

    fn form(&self) -> super::BackendForm {
        super::BackendForm::WrapCommand
    }

    fn availability(&self) -> Availability {
        match &self.sandbox_exec {
            Some(_) => Availability::Full,
            None => Availability::Unavailable(
                "sandbox-exec not found at /usr/bin/sandbox-exec — unexpected on macOS".to_string(),
            ),
        }
    }

    fn wrap_command(&self, conf: &SandboxConf, cmd: &Command) -> Result<Command, String> {
        let exe = self
            .sandbox_exec
            .as_ref()
            .ok_or_else(|| "sandbox-exec not found".to_string())?;
        let mut wrapped = Command::new(exe);
        wrapped.arg("-p").arg(seatbelt_profile(conf));
        // 原命令作为 payload（program + args + env + cwd 原样搬运）。
        wrapped.arg(cmd.get_program());
        for arg in cmd.get_args() {
            wrapped.arg(arg);
        }
        if let Some(dir) = cmd.get_current_dir() {
            wrapped.current_dir(dir);
        }
        for (key, val) in cmd.get_envs() {
            match val {
                Some(v) => {
                    wrapped.env(key, v);
                }
                None => {
                    wrapped.env_remove(key);
                }
            }
        }
        Ok(wrapped)
    }
}
