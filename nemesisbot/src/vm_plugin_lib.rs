//! plugin-vm 库文件查找 + v4 签名闸（`vm` feature 门控）——**单一真相
//! 源**，两个消费臂共用：
//! - `vm-display` 装载臂（[`crate::vm_display_loader`]）：dlopen 显示中继；
//! - `vm` 资源释放臂（[`crate::vm_resources`]）：取内嵌资源释放落盘。
//!
//! 同一个 DLL，同一份信任裁决（在场布局 + 签名口径）——两臂各写一份
//! 迟早漂移，本模块收口。

use std::path::PathBuf;

/// 在场库文件名（平台分派）。
pub fn plugin_filenames() -> &'static [&'static str] {
    if cfg!(windows) {
        &["plugin_vm.dll"]
    } else if cfg!(target_os = "macos") {
        &["libplugin_vm.dylib"]
    } else {
        &["libplugin_vm.so"]
    }
}

/// 库文件查找：exe 旁 `plugins/` 子目录优先（部署权威布局，2026-10-22
/// 布局统一——与其他插件同住 plugins/），exe 同目录回落（旧部署兼容）。
pub fn find_plugin_path() -> Option<PathBuf> {
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    plugin_filenames()
        .iter()
        .flat_map(|name| [exe_dir.join("plugins").join(name), exe_dir.join(name)])
        .find(|p| p.is_file())
}

/// Unsigned（无签名/无从验证）闸口径：dev（debug_assertions）warn 放行 /
/// release 拒。抽成函数 = 决策单一真相源（两装载臂消费，单测钉口径防
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

#[cfg(all(test, feature = "vm"))]
mod tests;
