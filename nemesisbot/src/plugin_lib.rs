//! 插件 cdylib 库文件查找 + v4 签名闸——**单一真相源**（QUIC 插件化 M1
//! 泛化：从 vm_plugin_lib 上移，按 [`PluginLibSpec`] 参数化插件名）。
//!
//! 消费臂：
//! - VM（[`PluginLibSpec::VM`]）：`vm-display` 装载臂（[`crate::vm_display_loader`]）
//!   + `vm` 资源释放臂（[`crate::vm_resources`]）——兼容薄壳
//!   [`crate::vm_plugin_lib`] 转发；
//! - Roaming（[`PluginLibSpec::ROAMING`]）：QUIC 漫游装载臂
//!   （[`crate::roaming_plugin_loader`]）。
//!
//! 同族纪律：同一查找链（exe 旁 `plugins/` 优先 → exe 同目录回落）+ 同一
//! 信任裁决（v4 签名口径）——每个插件臂各写一份迟早漂移，本模块收口。

use std::path::PathBuf;

/// 插件库规格（三平台产物文件名 + 查找/签名共享逻辑）。
pub struct PluginLibSpec {
    /// Windows 产物名（`{base}.dll`）。
    pub file_windows: &'static str,
    /// macOS 产物名（`lib{base}.dylib`）。
    pub file_macos: &'static str,
    /// Linux 产物名（`lib{base}.so`）。
    pub file_unix: &'static str,
}

impl PluginLibSpec {
    /// plugin-vm（显示中继 + 内嵌资源）。
    pub const VM: Self = Self {
        file_windows: "plugin_vm.dll",
        file_macos: "libplugin_vm.dylib",
        file_unix: "libplugin_vm.so",
    };

    /// plugin-roaming（QUIC 中继 + rendezvous 注册表）。
    pub const ROAMING: Self = Self {
        file_windows: "plugin_roaming.dll",
        file_macos: "libplugin_roaming.dylib",
        file_unix: "libplugin_roaming.so",
    };

    /// 当前平台的库文件名。
    pub fn filename(&self) -> &'static str {
        if cfg!(windows) {
            self.file_windows
        } else if cfg!(target_os = "macos") {
            self.file_macos
        } else {
            self.file_unix
        }
    }

    /// 库文件查找：exe 旁 `plugins/` 子目录优先（部署权威布局，2026-10-22
    /// 布局统一——与其他插件同住 plugins/），exe 同目录回落（旧部署兼容）。
    pub fn find_path(&self) -> Option<PathBuf> {
        let name = self.filename();
        let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
        [exe_dir.join("plugins").join(name), exe_dir.join(name)]
            .into_iter()
            .find(|p| p.is_file())
    }
}

/// Unsigned（无签名/无从验证）闸口径：dev（debug_assertions）warn 放行 /
/// release 拒。抽成函数 = 决策单一真相源（各装载臂消费，单测钉口径防
/// 漂移——dev 跑与 release CI 各验半边，改常数即红半边）。
pub fn unsigned_dev_allow() -> bool {
    cfg!(debug_assertions)
}

/// 签名闸裁决（留痕由调用方做——把裁决与原因一次说清）。
pub enum SignatureVerdict {
    Valid,
    /// 无签名或无从验证（verify feature 关 / 锚缺席）：dev warn 放行 /
    /// release 拒绝。
    Unsigned,
    /// 五类硬拒：签名在但不可信/被改/被吊销/过期。
    HardReject(String),
}

#[cfg(feature = "verify")]
pub fn verify_signature(path: &std::path::Path) -> SignatureVerdict {
    use nemesis_verify::verify;
    let run = || -> Result<SignatureVerdict, String> {
        let Some(anchor_hex) = crate::verify_policy::ROOT_ANCHOR else {
            return Ok(SignatureVerdict::Unsigned);
        };
        let anchor = nemesis_verify::hex_util::hex_decode_32(anchor_hex)
            .map_err(|e| format!("锚指纹解码失败: {e}"))?;
        let bytes = std::fs::read(path)
            .map_err(|e| format!("读取插件库失败（{}）：{e}", path.display()))?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Ok(match verify::verify_bytes(&bytes, &[anchor], now) {
            nemesis_verify::verify::VerifyOutcome::Valid { .. } => SignatureVerdict::Valid,
            nemesis_verify::verify::VerifyOutcome::NoSignature => SignatureVerdict::Unsigned,
            other => SignatureVerdict::HardReject(format!("{other:?}")),
        })
    };
    match run() {
        Ok(v) => v,
        Err(e) => SignatureVerdict::HardReject(e),
    }
}

#[cfg(not(feature = "verify"))]
pub fn verify_signature(_path: &std::path::Path) -> SignatureVerdict {
    // verify feature 关：无从验证 = 无签名同分支（dev warn / release 拒）。
    SignatureVerdict::Unsigned
}

#[cfg(test)]
mod tests;
