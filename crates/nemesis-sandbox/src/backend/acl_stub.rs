//! P24 Windows ACL 档的 **stub 编译形态**（非 Windows 平台，或 Windows 上
//! 关掉 `acl` feature 的裁剪构建）。
//!
//! 契约：与 `acl_impl` 完全一致的公共 API 面，行为全部诚实失败——
//! `availability()` = `Unavailable`、`apply_to_self` / 自由函数 = `Err`。
//! 调用方拿到 Err 走既有降级链（warn + 无盒继续；strict 模式拒绝执行），
//! 绝不假装隔离成功。跨平台测试（`acl_tests.rs` 的 stub 臂）钉这个契约。

use std::path::Path;

use super::{Availability, BackendForm, Enforcement, SandboxBackend, SandboxConf};

/// stub 统一失败文案（诚实点名缺什么，不含糊）。
fn stub_err() -> String {
    "Windows ACL 沙盒档未编译：需要 Windows 平台 + `acl` cargo feature（\
     本机当前构建不含其真实现）"
        .to_string()
}

/// 支持的完整性级别（与 acl_impl 同形——仅为 API 面一致而存在）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrityLevel {
    /// S-1-16-4096。
    Low,
    /// S-1-16-8192。
    Medium,
}

impl IntegrityLevel {
    pub fn rid(self) -> u32 {
        match self {
            IntegrityLevel::Low => 4096,
            IntegrityLevel::Medium => 8192,
        }
    }

    pub fn sid_string(self) -> &'static str {
        match self {
            IntegrityLevel::Low => "S-1-16-4096",
            IntegrityLevel::Medium => "S-1-16-8192",
        }
    }

    pub fn from_rid(rid: u32) -> Option<IntegrityLevel> {
        match rid {
            4096 => Some(IntegrityLevel::Low),
            8192 => Some(IntegrityLevel::Medium),
            _ => None,
        }
    }
}

pub fn set_integrity_label(_path: &Path, _level: IntegrityLevel) -> Result<(), String> {
    Err(stub_err())
}

pub fn get_integrity_label(_path: &Path) -> Result<Option<u32>, String> {
    Err(stub_err())
}

pub fn remove_integrity_label(_path: &Path) -> Result<(), String> {
    Err(stub_err())
}

pub fn label_tree(
    _root: &Path,
    _level: IntegrityLevel,
    _max_files: usize,
) -> Result<usize, String> {
    Err(stub_err())
}

pub fn current_process_integrity() -> Result<u32, String> {
    Err(stub_err())
}

pub fn lower_current_process_integrity(_level: IntegrityLevel) -> Result<(), String> {
    Err(stub_err())
}

pub fn add_deny_write_ace(_path: &Path, _sid: &str) -> Result<(), String> {
    Err(stub_err())
}

pub fn revoke_ace(_path: &Path, _sid: &str) -> Result<(), String> {
    Err(stub_err())
}

/// standing GRANT ACE 权限位（与 acl_impl 同值：FILE_GENERIC_ALL &
/// !(WRITE_DAC|WRITE_OWNER)，DELETE 保含——rename/git/cargo 基线能力）
/// ——仅为 API 面一致而存在。
pub const GRANT_MASK: u32 = 0x001F_01FF & !(0x0004_0000 | 0x0008_0000);

/// DACL 定向档树遍历（stub = 诚实失败，契约同其余自由函数）。
pub fn ensure_grant_ace_tree(
    _root: &Path,
    _workspace_sid: &str,
    _max_files: usize,
) -> Result<usize, String> {
    Err(stub_err())
}

/// DACL 定向档状态面只读探针（stub = 诚实失败——状态面据此显示不可用，
/// 不假造达标态）。
pub fn root_standing_ace_state(
    _root: &Path,
    _workspace_sid: &str,
) -> Result<(bool, bool, bool), String> {
    Err(stub_err())
}

/// stub 后端：探测诚实报 Unavailable（选型决策表据此把它排除）。
pub struct AclBackend;

impl AclBackend {
    pub fn new() -> Self {
        Self
    }
}

impl Default for AclBackend {
    fn default() -> Self {
        Self
    }
}

impl SandboxBackend for AclBackend {
    fn name(&self) -> &str {
        "acl"
    }

    fn form(&self) -> BackendForm {
        BackendForm::SelfApply
    }

    fn availability(&self) -> Availability {
        Availability::Unavailable(stub_err())
    }

    fn apply_to_self(&self, _conf: &SandboxConf) -> Result<Enforcement, String> {
        Err(stub_err())
    }
}
