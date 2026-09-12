//! PE（Windows）格式 codec。
//!
//! 职责：
//! 1. 多源综合计算原始内容末尾 L（overlay 起点）= `max(section raw end)`，
//!    并与 Certificate Table（`DataDirectory[4]`）做交叉一致性校验。
//! 2. 定位 Authenticode 证书表区域（`DataDirectory[4]` 指向），供 envelope
//!    扫描时跳过——通过 [`ExecutableCodec::overlay_excludes`] 暴露给外层。
//! 3. [`ExecutableCodec::content_hash`]：SHA-256 over `[0, L)`，排除
//!    `CheckSum`(4B) 与 `DataDirectory[4]`(8B) 两个易变字段。
//!
//! # 偏移依据（PE/COFF 规范，逐字段核对）
//! 设 `P = e_lfanew`（u32 LE @ 文件偏移 `0x3C`）：
//! - PE 签名 `"PE\0\0"` @ `P`
//! - COFF header @ `P+4`：`NumberOfSections`@`P+6`、`SizeOfOptionalHeader`@`P+20`
//! - Optional header @ `P+24`：`Magic`@`P+24`（`0x10b`=PE32 / `0x20b`=PE32+）
//! - `CheckSum` @ `P+88`（PE32 与 PE32+ 偏移相同：BaseOfData+ImageBase 共 8B
//!   vs ImageBase 8B 抵消）
//! - `NumberOfRvaAndSizes` @ `P+116`(PE32) / `P+132`(PE32+)
//! - DataDirectory 起点 @ `P+120`(PE32) / `P+136`(PE32+)（差 16B：stack/heap
//!   reserve+commit 字段 4B vs 8B）
//! - `DataDirectory[4]`（Security）@ 起点+32 = `P+152`(PE32) / `P+168`(PE32+)
//! - section table @ `P+24+SizeOfOptionalHeader`，每项 40B：
//!   `SizeOfRawData`@+16、`PointerToRawData`@+20
//!
//! PE 所有多字节字段一律 little-endian（PE/COFF 规范强制，与运行平台无关）。

use crate::codec::{CodecError, ExecutableCodec, PeCodec};
use sha2::{Digest, Sha256};

/// PE 布局解析结果。
pub(crate) struct PeLayout {
    /// 原始可执行内容末尾（overlay 起点）。
    pub l: usize,
    /// `CheckSum` 字段区间 `[start, end)`（content_hash 排除）。
    pub checksum_range: (usize, usize),
    /// `DataDirectory[4]`（Security 目录项）区间，仅当 `NumberOfRvaAndSizes ≥ 5`。
    pub security_dir_range: Option<(usize, usize)>,
    /// Authenticode 证书表区间 `[VA, VA+Size)`，仅当 `Size > 0`（扫描时跳过）。
    pub auth_region: Option<(usize, usize)>,
}

/// 读 little-endian u16。
fn u16le(b: &[u8], off: usize, name: &'static str) -> Result<u16, CodecError> {
    let s = b
        .get(off..off + 2)
        .ok_or(CodecError::FieldOutOfBounds(name))?;
    Ok(u16::from_le_bytes([s[0], s[1]]))
}
/// 读 little-endian u32。
fn u32le(b: &[u8], off: usize, name: &'static str) -> Result<u32, CodecError> {
    let s = b
        .get(off..off + 4)
        .ok_or(CodecError::FieldOutOfBounds(name))?;
    Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// DER 外层 TLV 总长（头 + 内容；短式与最长 4 字节长式；不支持 0x80 不定长）。
/// 证书表条目的 bCertificate 带对齐 pad，提取须按此裁剪（S3-1 实证）。
fn der_tlv_len(b: &[u8]) -> Option<usize> {
    if b.len() < 2 {
        return None;
    }
    let l0 = b[1] as usize;
    if l0 < 0x80 {
        return Some(2 + l0);
    }
    let n = l0 & 0x7F;
    if n == 0 || n > 4 || b.len() < 2 + n {
        return None;
    }
    let mut len = 0usize;
    for &x in &b[2..2 + n] {
        len = (len << 8) | x as usize;
    }
    Some(2 + n + len)
}

/// 解析 PE 布局（多源综合 + 交叉校验）。
fn parse_pe(bytes: &[u8]) -> Result<PeLayout, CodecError> {
    if bytes.len() < 0x40 {
        return Err(CodecError::Truncated);
    }
    if &bytes[0..2] != b"MZ" {
        return Err(CodecError::NotAnExecutable);
    }
    let p = u32le(bytes, 0x3C, "e_lfanew")? as usize;
    // PE 签名 "PE\0\0"
    if bytes.get(p..p + 4) != Some(b"PE\0\0") {
        return Err(CodecError::NotAnExecutable);
    }
    // COFF header
    let num_sections = u16le(bytes, p + 6, "NumberOfSections")? as usize;
    let size_of_opt = u16le(bytes, p + 20, "SizeOfOptionalHeader")? as usize;
    // Optional header magic → PE32 / PE32+
    let magic = u16le(bytes, p + 24, "Magic")?;
    let is_plus = match magic {
        0x10b => false,
        0x20b => true,
        _ => return Err(CodecError::UnknownOptionalHeaderMagic(magic)),
    };
    // CheckSum @ P+88（PE32 与 PE32+ 相同）
    let checksum_range = (p + 88, p + 92);

    // NumberOfRvaAndSizes 与 DataDirectory 起点（PE32 / PE32+ 不同）
    let (nrva_off, datadir_start) = if is_plus {
        (p + 132, p + 136)
    } else {
        (p + 116, p + 120)
    };
    let nrva = u32le(bytes, nrva_off, "NumberOfRvaAndSizes")? as usize;
    // DataDirectory[4]（Security）仅当数组 ≥ 5 项时存在
    let security_dir_range = if nrva >= 5 {
        let off = datadir_start + 4 * 8;
        Some((off, off + 8))
    } else {
        None
    };

    // Authenticode 证书表区域 = DataDirectory[4] 的 VA（文件偏移）+ Size
    let auth_region = if nrva >= 5 {
        let off = datadir_start + 4 * 8;
        let va = u32le(bytes, off, "Security.VA")? as usize;
        let size = u32le(bytes, off + 4, "Security.Size")? as usize;
        if size > 0 {
            Some((va, va + size))
        } else {
            None
        }
    } else {
        None
    };

    // L = max(section PointerToRawData + SizeOfRawData)，仅算 SizeOfRawData>0 的 section
    let sec_tbl = p + 24 + size_of_opt;
    let mut l: usize = 0;
    for i in 0..num_sections {
        let s = sec_tbl + i * 40;
        let size_raw = u32le(bytes, s + 16, "SizeOfRawData")? as usize;
        let ptr_raw = u32le(bytes, s + 20, "PointerToRawData")? as usize;
        if size_raw > 0 {
            l = l.max(ptr_raw + size_raw);
        }
    }
    // 保险：L 至少覆盖到 section table 末尾（确保所有 headers 在 [0, L) 内）。
    let sec_tbl_end = sec_tbl + num_sections * 40;
    l = l.max(sec_tbl_end);
    // L 不超过文件长度
    l = l.min(bytes.len());

    // 交叉一致性校验：Authenticode 区域应在 overlay 内、不越界
    if let Some((va, end)) = auth_region {
        if va < l {
            return Err(CodecError::Malformed(format!(
                "Authenticode VA {} < overlay start L {}",
                va, l
            )));
        }
        if end > bytes.len() {
            return Err(CodecError::Malformed(format!(
                "Authenticode end {} > file len {}",
                end,
                bytes.len()
            )));
        }
    }

    Ok(PeLayout {
        l,
        checksum_range,
        security_dir_range,
        auth_region,
    })
}

/// Authenticode byte-range digest（S3-1，v4 判据载体；SHA-256）。
///
/// [MS-Authcode] / PE 规范语义（signtool / osslsigncode 同口径，参照交叉验证钉死）：
/// 1. `CheckSum`（4B）**整体跳过**（不参与哈希——实证：喂零与跳过在
///    字段原值为 0 的文件上可区分，参照签名内嵌 digest 只与跳过变体一致）；
/// 2. `DataDirectory[4]`（Security 表项，8B）**整体跳过**（nrva ≥ 5 时）；
/// 3. 证书数据块 `[VA, VA+Size)` **整体排除**（Size > 0 时）；
/// 4. 其余全文件参与——**无 L 上界，overlay 全含**（与 v3 [`ExecutableCodec::content_hash`]
///    的差异只在无 L 截断 + 证书块排除；字段跳过语义两者相同。
///    v3 路径 S4-1 消费方切换后删除）。
///
/// 参照交叉验证：`pe/tests.rs` S3-1 段对本机 signtool/osslsigncode 签的参照 PE
/// （含 overlay 大 / mingw 异构形态），本实现结果与签名内嵌 messageDigest 逐字节
/// 比对（`cargo test -p nemesis-verify -- --ignored` 显式跑，仓库外参照物）。
pub fn authenticode_digest(bytes: &[u8]) -> Result<[u8; 32], CodecError> {
    let layout = parse_pe(bytes)?;
    let mut h = Sha256::new();
    let (cs, ce) = layout.checksum_range;
    h.update(&bytes[..cs]);
    // CheckSum 4B 整体跳过（Authenticode 实证语义：skip ≠ 置零参与）
    match layout.security_dir_range {
        Some((ss, se)) => {
            if ss < ce {
                return Err(CodecError::Malformed(format!(
                    "Security Directory 项 @{ss} 与 CheckSum 区间 [{cs},{ce}) 重叠"
                )));
            }
            if ss > ce {
                h.update(&bytes[ce..ss]);
            }
            // Security 表项 8B 整体跳过
            match layout.auth_region {
                Some((va, vend)) => {
                    // 证书表 VA ≥ L ≥ section table 末尾 > Security 表项末尾（parse_pe 保证），
                    // 此处 va ≥ se 恒成立，直接喂中间段
                    h.update(&bytes[se..va]);
                    if vend < bytes.len() {
                        // 证书表之后还有尾巴（多 WIN_CERTIFICATE 链被截 / 附加数据）→ 全参与
                        h.update(&bytes[vend..]);
                    }
                }
                None => {
                    if se < bytes.len() {
                        h.update(&bytes[se..]);
                    }
                }
            }
        }
        None => {
            // 无 Security 目录项（nrva < 5）：CheckSum 之后全文件参与
            if ce < bytes.len() {
                h.update(&bytes[ce..]);
            }
        }
    }
    Ok(h.finalize().into())
}

/// WIN_CERTIFICATE 头部长度（dwLength + wRevision + wCertificateType）。
const WIN_CERT_HEADER_LEN: usize = 8;
/// wRevision = 0x0200（WIN_CERT_REVISION_2_0，Authenticode 唯一在用版本）。
const WIN_CERT_REVISION_2: u16 = 0x0200;
/// wCertificateType = 0x0002（PKCS_SIGNED_DATA，bCertificate = CMS DER）。
pub const WIN_CERT_TYPE_PKCS_SIGNED_DATA: u16 = 0x0002;

/// WIN_CERTIFICATE 条目字节：dwLength(u32 LE) + wRevision + wCertificateType
/// + bCertificate + 尾部零 pad 到 8 对齐（dwLength 含 pad）。
///
/// [`append_certificate_table`] 的条目序列化层。
fn win_cert_entry_bytes(cms_der: &[u8]) -> Result<Vec<u8>, CodecError> {
    let round8 = |x: usize| x.div_ceil(8) * 8;
    let entry_len = WIN_CERT_HEADER_LEN + round8(cms_der.len());
    if entry_len > u32::MAX as usize {
        return Err(CodecError::Malformed(format!(
            "CMS DER {} 条目化后超 u32：dwLength 无法表达",
            cms_der.len()
        )));
    }
    let mut e = Vec::with_capacity(entry_len);
    e.extend_from_slice(&(entry_len as u32).to_le_bytes());
    e.extend_from_slice(&WIN_CERT_REVISION_2.to_le_bytes());
    e.extend_from_slice(&WIN_CERT_TYPE_PKCS_SIGNED_DATA.to_le_bytes());
    e.extend_from_slice(cms_der);
    e.resize(entry_len, 0);
    Ok(e)
}

/// S3-2：把 CMS DER 作为 Authenticode 证书表追加到**未签名** PE（写入路径）。
///
/// 布局（signtool / osslsigncode 参照实证同口径，见 pe/tests.rs S3-1 交叉验证）：
/// `[原文件][零 pad 到 8 对齐][WIN_CERTIFICATE 条目]`
/// 条目 = `dwLength`(u32 LE) + `wRevision`(u16 LE = 0x0200) +
/// `wCertificateType`(u16 LE = 0x0002) + bCertificate（CMS DER，尾部零 pad 到
/// 8 对齐，dwLength 含 pad）；Security 表项写入（VA=新表文件偏移、Size=条目全
/// 长度）；文件恰止于表尾（与全部参照一致）。
///
/// 边界（诚实失败）：
/// - nrva < 5（无 Security 表项可写）→ Malformed（扩 Optional header 需搬
///   section table，不支持）；
/// - Security Size > 0（已有证书表）→ Malformed——已签名文件策略（替换自方 /
///   追加共存）归 S3-3，本函数只接受未签名 PE；
/// - VA / 条目长度超 u32（PE DataDirectory 域宽）→ Malformed；
/// - CheckSum 字段**不重算**：Authenticode digest 跳过该字段、Windows 加载器
///   仅对驱动类映像强制校验（osslsigncode 会重算，非兼容必需，v4 明确不做）。
pub fn append_certificate_table(bytes: &[u8], cms_der: &[u8]) -> Result<Vec<u8>, CodecError> {
    let layout = parse_pe(bytes)?;
    let Some((entry_off, _)) = layout.security_dir_range else {
        return Err(CodecError::Malformed(
            "NumberOfRvaAndSizes < 5：无 Security 表项可写，无法承载证书表".to_string(),
        ));
    };
    if let Some((_, end)) = layout.auth_region {
        return Err(CodecError::Malformed(format!(
            "已存在证书表（截至 {end}）：本函数只接受未签名 PE，替换/共存策略归上层"
        )));
    }
    if cms_der.is_empty() {
        return Err(CodecError::Malformed("CMS DER 为空".to_string()));
    }
    let round8 = |x: usize| x.div_ceil(8) * 8;
    let va = round8(bytes.len());
    if va > u32::MAX as usize {
        return Err(CodecError::Malformed(format!(
            "文件长度 {} 8 对齐后超 u32：证书表 VA 无法表达",
            bytes.len()
        )));
    }
    let entry = win_cert_entry_bytes(cms_der)?;
    let entry_len = entry.len();
    let mut out = Vec::with_capacity(va + entry_len);
    out.extend_from_slice(bytes);
    out.resize(va, 0); // 对齐 pad（全零，参与 Authenticode digest）
    out.extend_from_slice(&entry);
    // Security 表项 = (VA, Size)，文件偏移域（非 RVA——Security 表项是 DataDirectory
    // 中唯一用文件偏移的条目，PE/COFF 规范明文）
    out[entry_off..entry_off + 4].copy_from_slice(&(va as u32).to_le_bytes());
    out[entry_off + 4..entry_off + 8].copy_from_slice(&(entry_len as u32).to_le_bytes());
    Ok(out)
}

/// S3-3：证书表条目的解析视图（[`read_certificate_entries`] 产物）。
pub struct WinCertEntry {
    /// 条目起始文件偏移（Security.VA 或前序条目 8 对齐步进落点）。
    pub offset: usize,
    /// `dwLength`（含头与对齐 pad）。
    pub entry_len: usize,
    /// `wRevision`（Authenticode 在用值 0x0200）。
    pub revision: u16,
    /// `wCertificateType`（0x0002 = PKCS_SIGNED_DATA / CMS）。
    pub cert_type: u16,
    /// bCertificate。PKCS_SIGNED_DATA 条目按外层 DER TLV 长度裁掉对齐 pad
    /// （osslsigncode 条目带 pad，严格 from_der 拒 trailing data——S3-1 实证）；
    /// 其他类型条目原样保留原始 blob。
    pub certificate: Vec<u8>,
}

/// S3-3：读取证书表全部 WIN_CERTIFICATE 条目。
///
/// **主签名 = 首条目**（Windows Authenticode 语义：主签名枚举只认首条目，
/// 追加多签名的微软真实形态 = SPC_NESTED_SIGNATURE 嵌套属性而非多条目——
/// 2026-09-12 signtool /as 对照实验实证，见 envelope[`crate::envelope::append_nested_signature`]；
/// 多条目只在畸形/非微软工具产物中出现，读取层照常遍历）。
///
/// 步进语义与 Windows 加载器 / S3-1 参照提取辅助同口径：从 Security.VA 起，
/// 按 dwLength 前进、落点 8 对齐；`dwLength < 8` 或越过表尾即停（视作尾部
/// padding，非错误）。未签名（无证书表）返回空 Vec。PKCS_SIGNED_DATA 条目的
/// bCertificate 必须可解析出合法 DER TLV，否则 Malformed（诚实失败——带此
/// 类型标记却非 DER 的表是损坏表）。
pub fn read_certificate_entries(bytes: &[u8]) -> Result<Vec<WinCertEntry>, CodecError> {
    let layout = parse_pe(bytes)?;
    let Some((va, end)) = layout.auth_region else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    let mut pos = va;
    while pos + WIN_CERT_HEADER_LEN <= end {
        let dwlen = u32le(bytes, pos, "WIN_CERTIFICATE.dwLength")? as usize;
        if dwlen < WIN_CERT_HEADER_LEN || pos + dwlen > end {
            break; // 尾部 padding / 非规范尾项：停止（与 Windows 步进行为一致）
        }
        let revision = u16le(bytes, pos + 4, "wRevision")?;
        let cert_type = u16le(bytes, pos + 6, "wCertificateType")?;
        let blob = &bytes[pos + WIN_CERT_HEADER_LEN..pos + dwlen];
        let certificate = if cert_type == WIN_CERT_TYPE_PKCS_SIGNED_DATA {
            let tlv = der_tlv_len(blob).ok_or_else(|| {
                CodecError::Malformed("bCertificate 外层 DER 长度头损坏".to_string())
            })?;
            if tlv > blob.len() {
                return Err(CodecError::Malformed(format!(
                    "bCertificate DER 声称 {tlv} 超出条目 {}",
                    blob.len()
                )));
            }
            blob[..tlv].to_vec()
        } else {
            blob.to_vec()
        };
        out.push(WinCertEntry {
            offset: pos,
            entry_len: dwlen,
            revision,
            cert_type,
            certificate,
        });
        pos = (pos + dwlen + 7) & !7;
    }
    Ok(out)
}

/// S3-3：已签名文件策略之一——**替换证书表**（自方重签 / 嵌套签名后表重写共用）。
///
/// 剥掉现有整张证书表（truncate 回表起点）+ Security 表项清零，再走
/// [`append_certificate_table`] 重新追加。内容区 `[0, VA)` 不动 → 替换前后
/// Authenticode digest 不变（摘要只覆盖内容；表重写吞下的只是表自身字节）。
///
/// 两个消费场景：
/// - **自方重签**：表中只有自方签名时整体替换（旧签名消失）；含他方条目时
///   不可用（会剥掉他方签名）——策略裁决在 [`crate::envelope::resign_pe_file`]；
/// - **嵌套签名后表重写**：他方 CMS 挂 SPC_NESTED_SIGNATURE 属性后 DER 变长，
///   以替换方式写回单条目表（他方签名在 CMS 内字节不变）。
///
/// 边界（诚实失败）：未签名（无表）→ Malformed（首次签名走 append）；
/// 证书表之后存在数据 → Malformed（truncate 会吞数据，不支持）；空 DER → Malformed。
pub fn replace_certificate_table(bytes: &[u8], cms_der: &[u8]) -> Result<Vec<u8>, CodecError> {
    let layout = parse_pe(bytes)?;
    let Some((va, end)) = layout.auth_region else {
        return Err(CodecError::Malformed(
            "未签名 PE（无证书表）：无旧签名可替换，首次签名走 append_certificate_table"
                .to_string(),
        ));
    };
    if end != bytes.len() {
        return Err(CodecError::Malformed(format!(
            "证书表后存在 {} 字节数据：替换需 truncate 表区间，会吞数据，拒绝",
            bytes.len() - end
        )));
    }
    if cms_der.is_empty() {
        return Err(CodecError::Malformed("CMS DER 为空".to_string()));
    }
    let (entry_off, _) = layout
        .security_dir_range
        .expect("auth_region 在场 ⇒ nrva ≥ 5 ⇒ Security 表项在场");
    let mut stripped = bytes[..va].to_vec();
    stripped[entry_off..entry_off + 8].fill(0); // 表项清零（digest 跳过该字段，值无关）
    append_certificate_table(&stripped, cms_der)
}

impl ExecutableCodec for PeCodec {
    fn compute_l(&self, bytes: &[u8]) -> Result<Option<usize>, CodecError> {
        let layout = parse_pe(bytes)?;
        Ok(Some(layout.l))
    }

    fn content_hash(&self, content: &[u8], l: usize) -> Result<[u8; 32], CodecError> {
        if l > content.len() {
            return Err(CodecError::Malformed(format!(
                "content_len {} > bytes len {}",
                l,
                content.len()
            )));
        }
        let layout = parse_pe(content)?;
        // 收集排除区间（均在 headers 内、属 [0, l)）
        let mut excludes: Vec<(usize, usize)> = vec![layout.checksum_range];
        if let Some(r) = layout.security_dir_range {
            excludes.push(r);
        }
        // 排序 + 顺序游标，分段喂 SHA-256（跳过排除区间，裁剪到 [0, l)）
        excludes.sort_by_key(|r| r.0);
        let mut hasher = Sha256::new();
        let mut cursor = 0usize;
        for (start, end) in excludes {
            let start = start.min(l);
            let end = end.min(l);
            if start > cursor {
                hasher.update(&content[cursor..start]);
            }
            if end > cursor {
                cursor = end;
            }
        }
        if cursor < l {
            hasher.update(&content[cursor..l]);
        }
        Ok(hasher.finalize().into())
    }

    fn overlay_excludes(&self, bytes: &[u8]) -> Vec<(usize, usize)> {
        // 暴露 Authenticode 证书表区域，供 envelope 扫描时跳过；解析失败容错返回空。
        parse_pe(bytes)
            .ok()
            .and_then(|layout| layout.auth_region)
            .into_iter()
            .collect()
    }
}

#[cfg(test)]
mod tests;
