//! plugin-vm 库查找 + v4 签名闸——**兼容薄壳**（QUIC 插件化 M1 泛化）。
//!
//! 真相源已上移 [`crate::plugin_lib`]（按 [`PluginLibSpec`] 参数化：
//! VM / Roaming 两个插件臂共用查找链与签名裁决）；本模块只保留 VM
//! 形态的无参函数，既有消费臂（`vm_display_loader` / `vm_resources`）
//! 零改动。

use std::path::PathBuf;

pub use crate::plugin_lib::{
    PluginLibSpec, SignatureVerdict, unsigned_dev_allow, verify_signature,
};

/// VM 插件库文件查找（= [`PluginLibSpec::VM`].find_path()）。
pub fn find_plugin_path() -> Option<PathBuf> {
    PluginLibSpec::VM.find_path()
}
