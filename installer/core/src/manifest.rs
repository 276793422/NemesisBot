//! 安装包 manifest：payload 权威清单（计划 §3）。
//!
//! manifest 是 payload 完整性的单一真相源：每文件 sha256 + payload 整体
//! sha256 都记在这里；manifest 自身被 v4 签名锚定（签名形态）或被 NBSETUP1
//! 尾注的 manifest_sha256 锚定（两道都过才放行）。
//!
//! 字段约束（读取侧强制）：
//! - `payload_offset + payload_len == manifest_offset`（三区严格相邻）
//! - `files` 按 path 排序（确定性输出；tar 收录顺序同序）

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};

/// 平台字段值域（pack --platform 校验）。
pub const PLATFORMS: [&str; 2] = ["windows", "linux"];

/// 单文件条目。`path` = 相对安装根的正斜杠路径；`mode` = Unix 权限位
/// （0o755 / 0o644；Windows 侧仅承载语义，不做强制）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ManifestFileEntry {
    pub path: String,
    pub sha256: String,
    pub size: u64,
    pub mode: u32,
}

/// 安装包 manifest（JSON 落盘形态）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub format_version: u32,
    pub bot_version: String,
    pub platform: String,
    /// pack 时刻 Unix 秒。
    pub created_at: u64,
    /// zstd 帧起始绝对偏移 = 骨架长度。
    pub payload_offset: u64,
    /// zstd 帧长度。
    pub payload_len: u64,
    /// zstd 帧（原始压缩字节）整体 sha256 hex。
    pub payload_sha256: String,
    /// 解压后总字节数（磁盘空间预检用）。
    pub total_size: u64,
    /// 权威文件清单（按 path 排序）。
    pub files: Vec<ManifestFileEntry>,
}

impl Manifest {
    /// 校验结构性约束（读取侧统一入口）。
    pub fn validate(&self, manifest_offset: u64) -> Result<()> {
        if self.format_version != crate::FORMAT_VERSION {
            return Err(anyhow!(
                "manifest 格式版本 {} ≠ {}（安装包过新或过旧）",
                self.format_version,
                crate::FORMAT_VERSION
            ));
        }
        if !PLATFORMS.contains(&self.platform.as_str()) {
            return Err(anyhow!("platform 值域非法: {}", self.platform));
        }
        if self.payload_offset.checked_add(self.payload_len) != Some(manifest_offset) {
            // 错误文案用 u128 算和——溢出样本（u64 边界）进错误路径不能再溢出 panic。
            let sum = (self.payload_offset as u128) + (self.payload_len as u128);
            return Err(anyhow!(
                "三区不相邻：payload_offset+len = {sum} ≠ manifest_offset {manifest_offset}"
            ));
        }
        if self.files.is_empty() {
            return Err(anyhow!("manifest 文件清单为空"));
        }
        let sorted = self.files.windows(2).all(|w| w[0].path <= w[1].path);
        if !sorted {
            return Err(anyhow!("files 未按 path 排序（非确定性包）"));
        }
        Ok(())
    }

    /// path → 条目查找表。
    pub fn entry_map(&self) -> std::collections::HashMap<&str, &ManifestFileEntry> {
        self.files.iter().map(|f| (f.path.as_str(), f)).collect()
    }

    pub fn to_json_vec(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec_pretty(self)?)
    }

    pub fn from_json_slice(bytes: &[u8]) -> Result<Manifest> {
        Ok(serde_json::from_slice(bytes)?)
    }
}

#[cfg(test)]
mod tests;
