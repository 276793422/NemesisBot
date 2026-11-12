//! 复制体内 nemesisbot 依赖换指：stub 槽位 → 私有实现挂载点。
//!
//! 只作用于 `assembly/` 复制体（公开工作区零写入）：`nemesisbot/Cargo.toml`
//! 的 `nemesis-taiwei` path 从 stub 槽位改指 submodule 挂载点里的真实现
//!（feature `taiwei` 透传的 `geoloc-win` 在真 manifest 里存在；stub 侧误开
//! 由 compile_error 拦下）。精确全文匹配 + 命中数 ==1 校验——公开仓措辞
//! 漂移时 loud 拒绝，绝不静默换错对象。

use anyhow::{Context, Result, bail};
use std::path::Path;

/// 公开仓 stub 形态（nemesisbot/Cargo.toml 依赖行，逐字节）。
pub const STUB_DEP: &str = r#"nemesis-taiwei = { path = "../slots/taiwei", optional = true }"#;
/// 装配形态：path 改指 submodule 挂载点内的实现 crate。
pub const IMPL_DEP: &str =
    r#"nemesis-taiwei = { path = "../slots/taiwei/impl/crates/taiwei-impl", optional = true }"#;

/// 全文换指。命中数 != 1 一律拒绝（0 = 措辞漂移或复制体已是实现形态；
/// >1 = 依赖行重复，须人工核查）。
pub fn swap(manifest: &str) -> Result<String> {
    let hits = manifest.matches(STUB_DEP).count();
    if hits != 1 {
        bail!(
            "换指失败：期望恰 1 处 stub 依赖行，实际 {hits} 处。\n\
             期望行：{STUB_DEP}\n\
             （0 = 公开仓措辞已漂移或复制体已是实现形态；>1 = 依赖行重复，人工核查）"
        );
    }
    Ok(manifest.replace(STUB_DEP, IMPL_DEP))
}

/// 读文件 → 换指 → 写回（复制体 nemesisbot/Cargo.toml）。
pub fn apply_to_file(path: &Path) -> Result<()> {
    let src = std::fs::read_to_string(path)
        .with_context(|| format!("读取复制体 manifest {}", path.display()))?;
    let swapped = swap(&src)?;
    std::fs::write(path, swapped).with_context(|| format!("写回复制体 manifest {}", path.display()))
}

#[cfg(test)]
mod tests;
