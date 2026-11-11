//! setup-core：NBSETUP1 安装包格式的打包 / 检视 / 安装 / 卸载核心库。
//!
//! 独立 workspace 成员（installer/ 隔离纪律，计划 §2）：唯一外部 path dep
//! 是 `nemesis-verify`（签名验证 v4——单向依赖，不回指主仓任何其他 crate）。
//!
//! 模块地图：
//! - [`format`]：NBSETUP1 尾注（反向扫描定位 + 位置交叉核验）
//! - [`manifest`]：payload 权威清单（sha256 + 结构约束）
//! - [`version`]：点分版本比较（Fresh/Repair/Upgrade/Downgrade 裁决输入）
//! - [`fsutil`]：韧性文件操作（AV/索引器瞬时句柄重试）
//! - [`sig`]：v4 验签九态 → 绿/黄/红分级（计划 §4a）
//! - [`pkgfile`]：安装包读取管线（完整性硬域 → 信任分级域）
//! - [`pack`]：打包 + 外壳签名 + keygen（§7a ⑥⑦）
//! - [`install`]：安装引擎（staging 原子提交）+ 卸载
//! - [`procs`]：运行实例路径级识别与强制停止编排（平台中立核心）
//! - [`console`]：Windows 控制台 UTF-8 代码页（GBK 乱码修复）

pub mod console;
pub mod format;
pub mod fsutil;
pub mod install;
pub mod manifest;
pub mod pack;
pub mod pkgfile;
pub mod procs;
pub mod sig;
pub mod version;

/// 包格式版本（manifest.format_version；读取侧强制一致——过新/过旧 loud 拒绝）。
pub const FORMAT_VERSION: u32 = 1;

#[cfg(test)]
mod tests;
