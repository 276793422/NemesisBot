//! 内嵌资源表（2026-10-22 拍板：VM 资源——除三方件外——全部打进插件库
//! 本体，用的时候由宿主释放；**不再有 plugin_vm/ 资源目录**）。
//!
//! - [`IMAGE_SCRIPT`]：镜像构建脚本（include_str!，仓库内常驻文件）。
//! - [`GUEST_X86_64`] / [`GUEST_AARCH64`]：guest 双架构 musl ELF
//!   （include_bytes!，各挂 build.rs 探测发射的 cfg——构建时产物不在场
//!   则该资源编译期不存在，`resource()` 对应名字返回 None，运行期宿主
//!   诚实报「资源未内嵌」）。
//!
//! 资源名字值域（宿主 `plugin_vm_resource_get` 的 wire 契约，宿主侧
//! vm_resources 模块按同名字符串取）：
//! [`RESOURCE_SCRIPT`] / [`RESOURCE_GUEST_X86_64`] / [`RESOURCE_GUEST_AARCH64`]。
//! 释放落盘布局镜像 build-guest.sh 的 `bin/bin_guest/<短arch>/` 惯例
//! （`guest/<arch>/nemesis-guest`），查找链测试同款语义。

/// 资源名：镜像构建脚本。
pub const RESOURCE_SCRIPT: &str = "build-vm-image.sh";
/// 资源名：x86_64 guest ELF。
pub const RESOURCE_GUEST_X86_64: &str = "guest/x86_64/nemesis-guest";
/// 资源名：aarch64 guest ELF。
pub const RESOURCE_GUEST_AARCH64: &str = "guest/aarch64/nemesis-guest";

/// 镜像构建脚本（编译期内嵌——脚本与库同版本，永无错配）。
pub static IMAGE_SCRIPT: &str = include_str!("../build-vm-image.sh");

/// x86_64 guest ELF（构建时在场才存在，见 build.rs）。
#[cfg(embed_guest_x86_64)]
pub static GUEST_X86_64: &[u8] = include_bytes!("../../../bin/bin_guest/x86_64/nemesis-guest");

/// aarch64 guest ELF（构建时在场才存在，见 build.rs）。
#[cfg(embed_guest_aarch64)]
pub static GUEST_AARCH64: &[u8] = include_bytes!("../../../bin/bin_guest/aarch64/nemesis-guest");

/// 资源名 → 字节（None = 名未知或该资源构建时未内嵌）。
pub fn resource(name: &str) -> Option<&'static [u8]> {
    match name {
        RESOURCE_SCRIPT => Some(IMAGE_SCRIPT.as_bytes()),
        #[cfg(embed_guest_x86_64)]
        RESOURCE_GUEST_X86_64 => Some(GUEST_X86_64),
        #[cfg(embed_guest_aarch64)]
        RESOURCE_GUEST_AARCH64 => Some(GUEST_AARCH64),
        _ => None,
    }
}
