//! 安装包文件读取管线：尾注定位 → manifest 完整性 → 验签分级 → 结构校验。
//!
//! 校验顺序（失败语义严格二分，计划 §4a）：
//! 1. **结构/完整性域（硬失败，不可越过）**：尾注定位与位置核验、
//!    manifest blob sha256 对尾注、manifest 解析与结构约束、payload
//!    区域 sha256（释放前流水校验）——损坏的包装出来就是坏的；
//! 2. **信任域（分级处置）**：manifest blob 的 v4 验签九态 → 绿/黄/红。

use std::collections::HashMap;
use std::io::{Read, Seek};
use std::path::Path;

use anyhow::{Result, anyhow};
use nemesis_verify::verify::VerifyOutcome;
use sha2::{Digest, Sha256};

use crate::format::{self, Footer};
use crate::manifest::Manifest;
use crate::sig::{self, SigGrade};

/// 安装包读取产物。
#[derive(Debug)]
pub struct PackageInfo {
    pub manifest: Manifest,
    /// manifest 是否带 v4 签名（blob 含 NMBSIG v4 footer）。
    pub manifest_signed: bool,
    /// 验签九态（未签名 = NoSignature；与 `grade` 搭配分级）。
    pub sig: VerifyOutcomeDisplay,
    /// 安装包文件总长。
    pub file_len: u64,
    /// NBSETUP1 尾注（释放引擎复用定位）。
    pub footer: Footer,
}

/// VerifyOutcome 的可拷贝投影（九态 → 分级 + 人读文本 + 关键明细），
/// 避免把非 Copy 枚举到处搬。
#[derive(Debug, Clone, PartialEq)]
pub struct VerifyOutcomeDisplay {
    pub grade: SigGrade,
    pub text: String,
}

impl VerifyOutcomeDisplay {
    fn from(outcome: &VerifyOutcome) -> Self {
        VerifyOutcomeDisplay {
            grade: sig::grade(outcome),
            text: sig::outcome_text(outcome),
        }
    }
}

/// 读安装包元信息（不碰 payload 区域——快速，供磁盘预检/信任分级先行）。
pub fn inspect(path: &Path) -> Result<PackageInfo> {
    let file = std::fs::File::open(path)?;
    let file_len = file.metadata()?.len();
    if file_len < (format::FOOTER_SIZE as u64) {
        return Err(anyhow!("文件过小，非安装包: {}", path.display()));
    }

    // ① 尾注定位（反向扫描 + 位置交叉核验）。
    let window = file_len.min(format::SCAN_WINDOW);
    let mut tail = vec![0u8; window as usize];
    let mut reader = std::io::BufReader::new(file);
    reader.seek(std::io::SeekFrom::Start(file_len - window))?;
    reader.read_exact(&mut tail)?;
    let (footer, _footer_pos) = format::locate_footer(file_len, &tail)?;

    // ② manifest blob 完整性（硬失败域）。
    if footer.manifest_offset + footer.manifest_len > file_len {
        return Err(anyhow!("manifest 区域越界（结构损坏）"));
    }
    reader.seek(std::io::SeekFrom::Start(footer.manifest_offset))?;
    let mut blob = vec![0u8; footer.manifest_len as usize];
    reader.read_exact(&mut blob)?;
    let blob_hash = Sha256::digest(&blob);
    if blob_hash[..] != footer.manifest_sha256 {
        return Err(anyhow!("manifest sha256 对尾注不符（内容损坏或被改）"));
    }

    // ③ 分离 v4 签名（blob 带 NMBSIG footer → 签名态；不带 → 未签态）。
    // 载体布局 = [content][CMS DER][v4 footer]——明文长度用 content_len
    // （footer_offset 含 CMS DER 段，切在那里会把 DER 带进 JSON）。
    let (plain, sig_outcome, manifest_signed) =
        match nemesis_verify::envelope::extract_carrier_v4(&blob) {
            Ok(v) => {
                let plain = &blob[..v.content_len];
                let outcome =
                    nemesis_verify::verify::verify_bytes(&blob, &sig::anchors(), now_unix());
                (plain.to_vec(), outcome, true)
            }
            Err(_) => {
                // 无 v4 footer = 疑似未签名形态。用 verify_bytes 确认它真是
                // NoSignature（而非结构错误把 footer 弄丢——诚实区分：
                // 结构异常走硬失败，不落信任分级域）。
                let outcome =
                    nemesis_verify::verify::verify_bytes(&blob, &sig::anchors(), now_unix());
                if !matches!(outcome, VerifyOutcome::NoSignature) {
                    return Err(anyhow!(
                        "manifest 结构异常: {}",
                        sig::outcome_text(&outcome)
                    ));
                }
                (blob.clone(), outcome, false)
            }
        };

    // ④ manifest 解析 + 结构约束（硬失败域）。
    let manifest = Manifest::from_json_slice(&plain)?;
    manifest.validate(footer.manifest_offset)?;

    Ok(PackageInfo {
        manifest_signed,
        sig: VerifyOutcomeDisplay::from(&sig_outcome),
        manifest,
        file_len,
        footer,
    })
}

/// payload 区域 sha256 流水校验（释放前硬闸；带进度回调）。
pub fn verify_payload_region(file: &mut std::fs::File, manifest: &Manifest) -> Result<()> {
    let total = manifest.payload_len;
    let mut reader = take_region(file, manifest.payload_offset, total)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 256 * 1024];
    let mut done: u64 = 0;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        done += n as u64;
    }
    if done != total {
        return Err(anyhow!("payload 区域长度 {done} ≠ manifest 记载 {total}"));
    }
    let got = hex::encode(hasher.finalize());
    if got != manifest.payload_sha256 {
        return Err(anyhow!(
            "payload sha256 不符（载荷被改或损坏）：manifest={} 实际={got}",
            manifest.payload_sha256
        ));
    }
    Ok(())
}

/// 取 [offset, offset+len) 区间的受限读取器（流出式，不整体驻内存）。
pub fn take_region(
    file: &mut std::fs::File,
    offset: u64,
    len: u64,
) -> Result<std::io::Take<&mut std::fs::File>> {
    use std::io::Seek;
    file.seek(std::io::SeekFrom::Start(offset))?;
    Ok(file.take(len))
}

/// tar 条目路径安全归一：反斜杠 → 正斜杠，拒绝绝对路径 / `..` / 盘符。
/// 返回归一后的相对路径。
pub fn sanitize_entry_path(raw: &std::path::Path) -> Result<String> {
    let s = raw.to_string_lossy().replace('\\', "/");
    if s.is_empty() {
        return Err(anyhow!("空条目路径"));
    }
    if s.starts_with('/') || s.contains(':') {
        return Err(anyhow!("条目路径含绝对/盘符形态: {s}"));
    }
    for comp in s.split('/') {
        if comp == ".." || comp.is_empty() {
            return Err(anyhow!("条目路径含非法段「{comp}」: {s}"));
        }
    }
    Ok(s)
}

/// 当前 Unix 秒（验签 now 参数）。
pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 进度事件（释放引擎 → 静默 stdout / GUI 进度条共用）。
#[derive(Debug, Clone)]
pub enum Progress {
    /// payload 哈希校验中（0.0..1.0）。
    Hashing(f32),
    /// 释放文件中（已完成条目数 / 总条目数）。
    Extracting(usize, usize),
}

/// 安装包文件清单索引（释放引擎防「清单外条目」用）。
pub type EntryIndex<'a> = HashMap<&'a str, &'a crate::manifest::ManifestFileEntry>;

#[cfg(test)]
mod tests;
