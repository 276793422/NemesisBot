//! NBSETUP1 尾注：安装包自描述定位层（计划 §3）。
//!
//! 安装包布局（pack 产出 / ⑦ 签名后）：
//!
//! ```text
//! [0, payload_offset)                骨架本体（未签名原始 exe）
//! [payload_offset, +payload_len)     zstd 帧（tar 流）
//! [manifest_offset, +manifest_len)   manifest blob（JSON，签名形态再追加 v4 footer）
//! [manifest_offset+manifest_len]     NBSETUP1 尾注（56B）
//! [..., EOF]                         外壳签名（⑦ 追加：PE 证书表 / ELF v4 footer）
//! ```
//!
//! **尾注不在固定偏移**：⑦ 外壳签名会把证书表 / v4 footer 追加到 EOF 区，
//! 所以读取侧必须反向扫描定位。误判防线 = 位置交叉核验：
//! `manifest_offset + manifest_len == 尾注位置`（zstd 压缩数据出现 8 字节
//! magic 碰撞的概率 ~2^-64，且必须恰好落在正确位置才可能通过核验）。

use anyhow::{Result, anyhow};

/// 尾注 magic（8 字节 ASCII）。
pub const FOOTER_MAGIC: [u8; 8] = *b"NBSETUP1";

/// 尾注总长 = magic(8) + manifest_offset(u64) + manifest_len(u64) + manifest_sha256(32)。
pub const FOOTER_SIZE: usize = 8 + 8 + 8 + 32;

/// 反向扫描窗口（从 EOF 往前最多看 1MB——尾注永远在文件末尾 1MB 内）。
pub const SCAN_WINDOW: u64 = 1024 * 1024;

/// NBSETUP1 尾注解析产物。
#[derive(Debug, Clone, PartialEq)]
pub struct Footer {
    /// manifest blob 起始绝对偏移。
    pub manifest_offset: u64,
    /// manifest blob 长度（含签名形态的 v4 footer；未签形态 = 纯 JSON 长）。
    pub manifest_len: u64,
    /// manifest blob 整体 sha256（完整性锚：blob 被改一个字节即红）。
    pub manifest_sha256: [u8; 32],
}

impl Footer {
    /// 编码为 56 字节尾注（全部小端）。
    pub fn encode(&self) -> [u8; FOOTER_SIZE] {
        let mut buf = [0u8; FOOTER_SIZE];
        buf[..8].copy_from_slice(&FOOTER_MAGIC);
        buf[8..16].copy_from_slice(&self.manifest_offset.to_le_bytes());
        buf[16..24].copy_from_slice(&self.manifest_len.to_le_bytes());
        buf[24..56].copy_from_slice(&self.manifest_sha256);
        buf
    }

    /// 从恰好 56 字节缓冲解码（magic 不匹配 = 结构损坏）。
    pub fn decode(buf: &[u8]) -> Result<Footer> {
        if buf.len() != FOOTER_SIZE {
            return Err(anyhow!("尾注长度 {} ≠ {FOOTER_SIZE}", buf.len()));
        }
        if buf[..8] != FOOTER_MAGIC {
            return Err(anyhow!("尾注 magic 不匹配"));
        }
        Ok(Footer {
            manifest_offset: u64::from_le_bytes(buf[8..16].try_into().unwrap()),
            manifest_len: u64::from_le_bytes(buf[16..24].try_into().unwrap()),
            manifest_sha256: buf[24..56].try_into().unwrap(),
        })
    }
}

/// 从文件尾部反向扫描定位尾注。
///
/// 返回 `(footer, footer 绝对偏移)`。找不到 magic 或位置交叉核验不过 =
/// `Err`（调用方按「非安装包 / 结构损坏」硬失败，计划 §4a 完整性域）。
pub fn locate_footer(file_len: u64, tail: &[u8]) -> Result<(Footer, u64)> {
    if (tail.len() as u64) > file_len {
        return Err(anyhow!("尾部窗口 {} 大于文件长 {file_len}", tail.len()));
    }
    let tail_start = file_len - tail.len() as u64;
    if tail.len() < FOOTER_SIZE {
        return Err(anyhow!("文件过小，不足以容纳尾注"));
    }
    // rfind：取最靠近 EOF 的 magic 出现（跳过压缩数据里理论碰撞的唯一手段
    // 是位置核验——碰撞落在别处直接被下面的等式排除）。
    let pos = tail
        .windows(8)
        .rposition(|w| w == FOOTER_MAGIC)
        .ok_or_else(|| anyhow!("未找到 NBSETUP1 尾注（非安装包或已损坏）"))?;
    if pos + FOOTER_SIZE > tail.len() {
        return Err(anyhow!("尾注被 EOF 截断"));
    }
    let footer = Footer::decode(&tail[pos..pos + FOOTER_SIZE])?;
    let footer_abs = tail_start + pos as u64;
    // 位置交叉核验：manifest blob 必须紧贴尾注。
    if footer.manifest_offset.checked_add(footer.manifest_len) != Some(footer_abs) {
        return Err(anyhow!(
            "尾注位置核验失败：manifest_offset+len = {} ≠ 尾注位置 {footer_abs}（结构损坏）",
            footer.manifest_offset + footer.manifest_len
        ));
    }
    Ok((footer, footer_abs))
}

#[cfg(test)]
mod tests;
