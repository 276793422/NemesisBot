//! Linux bubblewrap（bwrap）包装式后端（U11 链条第二档）。
//!
//! 定位：landlock 不可用（老内核 / 容器禁用）时的兜底，以及**唯一能真禁网**
//! 的档（`--unshare-net`）。包装式语义与 Sandboxie Start.exe 同构：父进程
//! 组好沙盒参数再 spawn 子进程。
//!
//! ⚠️ bwrap 的 mount-namespace 隔离需要用户命名空间支持（Ubuntu 24.04 默认
//! 开；受限容器里可能被 sysctl `kernel.unprivileged_userns_clone=0` 关掉）——
//! `wrap_command` 只是组参数，真正失败会出现在 spawn 时（ENOENT/EPERM），
//! 调用方按降级路径处理。

use std::path::PathBuf;
use std::process::Command;

use super::{Availability, SandboxBackend, SandboxConf, bwrap_args};

/// bubblewrap 包装式后端（构造时探测 bwrap 二进制，一次）。
pub struct BwrapBackend {
    bwrap_path: Option<PathBuf>,
}

impl BwrapBackend {
    pub fn new() -> Self {
        Self {
            bwrap_path: which_bwrap(),
        }
    }

    /// 探测到的 bwrap 路径（诊断用；lib 非测试构建无调用方，allow 死码）。
    #[allow(dead_code)]
    pub fn path(&self) -> Option<&PathBuf> {
        self.bwrap_path.as_ref()
    }
}

impl Default for BwrapBackend {
    fn default() -> Self {
        Self::new()
    }
}

fn which_bwrap() -> Option<PathBuf> {
    // 常见显式路径优先（PATH 可能被清洗过），再扫 PATH。
    for candidate in ["/usr/bin/bwrap", "/bin/bwrap", "/usr/local/bin/bwrap"] {
        if std::path::Path::new(candidate).exists() {
            return Some(PathBuf::from(candidate));
        }
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let p = dir.join("bwrap");
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

impl SandboxBackend for BwrapBackend {
    fn name(&self) -> &str {
        "bwrap"
    }

    fn form(&self) -> super::BackendForm {
        super::BackendForm::WrapCommand
    }

    fn availability(&self) -> Availability {
        match &self.bwrap_path {
            Some(_) => Availability::Full,
            None => Availability::Unavailable(
                "bwrap binary not found (install bubblewrap: apt install bubblewrap)".to_string(),
            ),
        }
    }

    fn wrap_command(&self, conf: &SandboxConf, cmd: &Command) -> Result<Command, String> {
        let bwrap = self
            .bwrap_path
            .as_ref()
            .ok_or_else(|| "bwrap binary not found".to_string())?;
        let mut wrapped = Command::new(bwrap);
        for arg in bwrap_args(conf) {
            wrapped.arg(arg);
        }
        // 原命令作为 bwrap 的 payload（program + args + env + cwd 原样搬运）。
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
