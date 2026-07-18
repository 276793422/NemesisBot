//! Envelope（签名块）字节结构、读写与定位。
//!
//! envelope 追加在可执行文件 overlay 区末尾（Raw 格式则在文件末尾），
//! 整体对齐到 4KB 倍数。布局：`[ 密文 body ][ padding ][ 明文 footer ]`，
//! footer 固定 64B 在 envelope 末尾，作定位锚点。
//!
//! # FOOTER（明文 64B）
//! | 偏移 | 长度 | 字段 |
//! |------|------|------|
//! | 0  | 8  | magic `NMBSIG\x02\x00` |
//! | 8  | 1  | format_ver（=2）|
//! | 9  | 1  | sig_algo（1=ed25519）|
//! | 10 | 1  | enc_algo（1=chacha20-poly1305）|
//! | 11 | 1  | format_tag（1=PE / 2=ELF / 3=Raw）|
//! | 12 | 4  | total_len（envelope 总长，4KB 倍数，LE）|
//! | 16 | 4  | body_len（密文 body 含 16B tag，LE）|
//! | 20 | 4  | content_len（被保护原始内容长度，LE）|
//! | 24 | 12 | nonce（ChaCha20-Poly1305，明文）|
//! | 36 | 4  | footer_crc32（footer\[0..36\] 校验，LE）|
//! | 40 | 24 | reserved |
//!
//! # BODY（明文，加密前；固定前缀 144B + 可选 TLV 扩展）
//! | 偏移 | 长度 | 字段 |
//! |------|------|------|
//! | 0   | 1  | body_ver（=1）|
//! | 1   | 1  | reserved |
//! | 2   | 2  | flags |
//! | 4   | 8  | signed_at（Unix epoch，LE）|
//! | 12  | 4  | key_id（公钥标识，提示用，不作安全决策）|
//! | 16  | 32 | key_fp（公钥 SHA-256 指纹）|
//! | 48  | 64 | signature（Ed25519 签名）|
//! | 112 | 32 | content_hash（程序内容 SHA-256，冗余早判）|
//! | 144 | .. | TLV 扩展（type 2B \| len 2B \| value）|
//! | ... | .. | padding |
//!
//! # signed_meta（Ed25519 签名覆盖范围，82B）
//! `format_ver | sig_algo | enc_algo | format_tag | flags | signed_at | key_id | key_fp | content_hash`
//! 签名消息 = `DOMAIN ++ signed_meta`。

use anyhow::{anyhow, Result};

/// domain 前缀（41 ASCII + 0x01 = 42 字节）。签名消息以此开头做 domain separation。
pub const DOMAIN: &[u8] = b"NEMESIS-BOT-276793422-ZHAO-SAN-KE-BIN-SIG\x01";
/// envelope 末尾 magic（8B）。
pub const TRAILER_MAGIC: [u8; 8] = *b"NMBSIG\x02\x00";

/// footer 固定长度。
pub const FOOTER_LEN: usize = 64;
/// footer 作为 AEAD AAD 的前缀长度（`footer[0..36]`，即 magic..crc）。
pub const FOOTER_AAD_LEN: usize = 36;
/// envelope 总长对齐粒度（内存页）。
pub const ENVELOPE_ALIGN: usize = 4096;

/// envelope 格式版本。
pub const FORMAT_VER: u8 = 2;
pub const SIG_ALGO_ED25519: u8 = 1;
pub const ENC_ALGO_CHACHA20: u8 = 1;
pub const BODY_VER: u8 = 1;

pub const NONCE_LEN: usize = 12;
pub const AEAD_TAG_LEN: usize = 16;
pub const ED25519_SIG_LEN: usize = 64;
pub const PUBKEY_LEN: usize = 32;
/// body 明文固定前缀长度。
pub const BODY_FIXED_LEN: usize = 144;
// signed_meta 变长（C1 起含 publisher/expires_at），不再有固定长度常量。

/// body TLV 类型（body 固定前缀后的 TLV 扩展区）。
pub const TLV_PUBLISHER: u16 = 0x10;
/// expires_at TLV：value = 1B has_flag + 8B u64 LE。
pub const TLV_EXPIRES_AT: u16 = 0x13;
/// sig_hash TLV：value = 32B SHA-256(signature)。
pub const TLV_SIG_HASH: u16 = 0x14;

// footer 字段偏移
const OFF_MAGIC: usize = 0;
const OFF_FORMAT_VER: usize = 8;
const OFF_SIG_ALGO: usize = 9;
const OFF_ENC_ALGO: usize = 10;
const OFF_FORMAT_TAG: usize = 11;
const OFF_TOTAL_LEN: usize = 12;
const OFF_BODY_LEN: usize = 16;
const OFF_CONTENT_LEN: usize = 20;
const OFF_NONCE: usize = 24;
const OFF_CRC: usize = 36;

// body 字段偏移
const BODY_OFF_VER: usize = 0;
const BODY_OFF_FLAGS: usize = 2;
const BODY_OFF_SIGNED_AT: usize = 4;
const BODY_OFF_KEY_ID: usize = 12;
const BODY_OFF_KEY_FP: usize = 16;
const BODY_OFF_SIG: usize = 48;
const BODY_OFF_CONTENT_HASH: usize = 112;

/// IEEE 802.3 CRC32（用于 footer 完整性快速校验，非密码学用途）。
fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// 向上对齐到 `align` 的倍数。
pub fn align_up(n: usize, align: usize) -> usize {
    (n + align - 1) / align * align
}

/// 读 LE u32（从固定偏移）。
fn rd_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

/// 读 LE u16。
fn rd_u16(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}

/// 读 LE u64。
fn rd_u64(b: &[u8], off: usize) -> u64 {
    u64::from_le_bytes([
        b[off], b[off + 1], b[off + 2], b[off + 3], b[off + 4], b[off + 5], b[off + 6], b[off + 7],
    ])
}

/// 构造 footer。
pub fn build_footer(
    format_tag: u8,
    total_len: usize,
    body_len: usize,
    content_len: usize,
    nonce: &[u8; NONCE_LEN],
) -> [u8; FOOTER_LEN] {
    let mut f = [0u8; FOOTER_LEN];
    f[OFF_MAGIC..OFF_MAGIC + 8].copy_from_slice(&TRAILER_MAGIC);
    f[OFF_FORMAT_VER] = FORMAT_VER;
    f[OFF_SIG_ALGO] = SIG_ALGO_ED25519;
    f[OFF_ENC_ALGO] = ENC_ALGO_CHACHA20;
    f[OFF_FORMAT_TAG] = format_tag;
    f[OFF_TOTAL_LEN..OFF_TOTAL_LEN + 4].copy_from_slice(&(total_len as u32).to_le_bytes());
    f[OFF_BODY_LEN..OFF_BODY_LEN + 4].copy_from_slice(&(body_len as u32).to_le_bytes());
    f[OFF_CONTENT_LEN..OFF_CONTENT_LEN + 4].copy_from_slice(&(content_len as u32).to_le_bytes());
    f[OFF_NONCE..OFF_NONCE + NONCE_LEN].copy_from_slice(nonce);
    let crc = crc32(&f[0..OFF_CRC]);
    f[OFF_CRC..OFF_CRC + 4].copy_from_slice(&crc.to_le_bytes());
    f
}

/// 解析后的 footer。
#[derive(Debug)]
pub struct ParsedFooter {
    pub format_ver: u8,
    pub sig_algo: u8,
    pub enc_algo: u8,
    pub format_tag: u8,
    pub total_len: usize,
    pub body_len: usize,
    pub content_len: usize,
    pub nonce: [u8; NONCE_LEN],
}

/// 解析 footer（校验 magic + crc32）。
pub fn parse_footer(bytes: &[u8; FOOTER_LEN]) -> Result<ParsedFooter> {
    if &bytes[OFF_MAGIC..OFF_MAGIC + 8] != &TRAILER_MAGIC {
        return Err(anyhow!("footer magic mismatch"));
    }
    let stored = rd_u32(bytes, OFF_CRC);
    let calc = crc32(&bytes[0..OFF_CRC]);
    if stored != calc {
        return Err(anyhow!("footer crc32 mismatch"));
    }
    let mut nonce = [0u8; NONCE_LEN];
    nonce.copy_from_slice(&bytes[OFF_NONCE..OFF_NONCE + NONCE_LEN]);
    Ok(ParsedFooter {
        format_ver: bytes[OFF_FORMAT_VER],
        sig_algo: bytes[OFF_SIG_ALGO],
        enc_algo: bytes[OFF_ENC_ALGO],
        format_tag: bytes[OFF_FORMAT_TAG],
        total_len: rd_u32(bytes, OFF_TOTAL_LEN) as usize,
        body_len: rd_u32(bytes, OFF_BODY_LEN) as usize,
        content_len: rd_u32(bytes, OFF_CONTENT_LEN) as usize,
        nonce,
    })
}

/// 给定 footer 偏移与解析结果，返回密文 body 的 `[start, end)`。
pub fn envelope_body_range(footer_offset: usize, parsed: &ParsedFooter) -> (usize, usize) {
    let envelope_start = footer_offset + FOOTER_LEN - parsed.total_len;
    (envelope_start, envelope_start + parsed.body_len)
}

/// 追加一条 TLV（type u16 LE + len u16 LE + value）。
fn write_tlv(buf: &mut Vec<u8>, t: u16, value: &[u8]) {
    buf.extend_from_slice(&t.to_le_bytes());
    buf.extend_from_slice(&(value.len() as u16).to_le_bytes());
    buf.extend_from_slice(value);
}

/// 解析 TLV 序列 → (type, value) 列表。忽略越界/不完整的尾部。
fn parse_tlvs(data: &[u8]) -> Vec<(u16, Vec<u8>)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 4 <= data.len() {
        let t = u16::from_le_bytes([data[i], data[i + 1]]);
        let l = u16::from_le_bytes([data[i + 2], data[i + 3]]) as usize;
        if i + 4 + l > data.len() {
            break;
        }
        out.push((t, data[i + 4..i + 4 + l].to_vec()));
        i += 4 + l;
    }
    out
}

/// 构造 body 明文 = 固定前缀(144B) + TLV 扩展（publisher/expires_at/sig_hash）。
pub fn build_body_plaintext(
    flags: u16,
    signed_at: u64,
    key_id: u32,
    key_fp: &[u8; PUBKEY_LEN],
    signature: &[u8; ED25519_SIG_LEN],
    content_hash: &[u8; 32],
    publisher: Option<&str>,
    expires_at: Option<u64>,
    sig_hash: &[u8; 32],
) -> Vec<u8> {
    let mut b = vec![0u8; BODY_FIXED_LEN];
    b[BODY_OFF_VER] = BODY_VER;
    b[BODY_OFF_FLAGS..BODY_OFF_FLAGS + 2].copy_from_slice(&flags.to_le_bytes());
    b[BODY_OFF_SIGNED_AT..BODY_OFF_SIGNED_AT + 8].copy_from_slice(&signed_at.to_le_bytes());
    b[BODY_OFF_KEY_ID..BODY_OFF_KEY_ID + 4].copy_from_slice(&key_id.to_le_bytes());
    b[BODY_OFF_KEY_FP..BODY_OFF_KEY_FP + PUBKEY_LEN].copy_from_slice(key_fp);
    b[BODY_OFF_SIG..BODY_OFF_SIG + ED25519_SIG_LEN].copy_from_slice(signature);
    b[BODY_OFF_CONTENT_HASH..BODY_OFF_CONTENT_HASH + 32].copy_from_slice(content_hash);
    // TLV 扩展（C1）
    if let Some(p) = publisher {
        write_tlv(&mut b, TLV_PUBLISHER, p.as_bytes());
    }
    let mut exp_buf = Vec::with_capacity(9);
    exp_buf.push(if expires_at.is_some() { 1u8 } else { 0u8 });
    exp_buf.extend_from_slice(&expires_at.unwrap_or(0).to_le_bytes());
    write_tlv(&mut b, TLV_EXPIRES_AT, &exp_buf);
    write_tlv(&mut b, TLV_SIG_HASH, sig_hash);
    b
}

/// 解析 body 明文（固定前缀 + TLV 扩展）。
pub fn parse_body(plaintext: &[u8]) -> Result<ParsedBody> {
    if plaintext.len() < BODY_FIXED_LEN {
        return Err(anyhow!(
            "body too short: {} < {}",
            plaintext.len(),
            BODY_FIXED_LEN
        ));
    }
    let mut key_fp = [0u8; PUBKEY_LEN];
    key_fp.copy_from_slice(&plaintext[BODY_OFF_KEY_FP..BODY_OFF_KEY_FP + PUBKEY_LEN]);
    let mut signature = [0u8; ED25519_SIG_LEN];
    signature.copy_from_slice(&plaintext[BODY_OFF_SIG..BODY_OFF_SIG + ED25519_SIG_LEN]);
    let mut content_hash = [0u8; 32];
    content_hash.copy_from_slice(&plaintext[BODY_OFF_CONTENT_HASH..BODY_OFF_CONTENT_HASH + 32]);

    // TLV 扩展（C1）
    let mut publisher: Option<String> = None;
    let mut expires_at: Option<u64> = None;
    let mut sig_hash = [0u8; 32];
    for (t, v) in parse_tlvs(&plaintext[BODY_FIXED_LEN..]) {
        match t {
            TLV_PUBLISHER => {
                publisher = Some(String::from_utf8(v).map_err(|e| anyhow!("publisher utf8: {}", e))?);
            }
            TLV_EXPIRES_AT if v.len() == 9 => {
                if v[0] == 1 {
                    expires_at = Some(u64::from_le_bytes([
                        v[1], v[2], v[3], v[4], v[5], v[6], v[7], v[8],
                    ]));
                }
            }
            TLV_SIG_HASH if v.len() == 32 => {
                sig_hash.copy_from_slice(&v);
            }
            _ => {} // 忽略未知 TLV（向前兼容）
        }
    }
    Ok(ParsedBody {
        flags: rd_u16(plaintext, BODY_OFF_FLAGS),
        signed_at: rd_u64(plaintext, BODY_OFF_SIGNED_AT),
        key_id: rd_u32(plaintext, BODY_OFF_KEY_ID),
        key_fp,
        signature,
        content_hash,
        publisher,
        expires_at,
        sig_hash,
    })
}

/// 解析后的 body。
#[derive(Debug)]
pub struct ParsedBody {
    pub flags: u16,
    pub signed_at: u64,
    pub key_id: u32,
    pub key_fp: [u8; PUBKEY_LEN],
    pub signature: [u8; ED25519_SIG_LEN],
    pub content_hash: [u8; 32],
    /// 发布者（C1，TLV；旧签名可能 None）。
    pub publisher: Option<String>,
    /// 过期时间（C1，TLV；None=无过期）。
    pub expires_at: Option<u64>,
    /// sig_hash = SHA-256(signature)（C1，TLV；供云端吊销查；旧签名可能全 0）。
    pub sig_hash: [u8; 32],
}

/// 构造 signed_meta（Ed25519 签名覆盖范围，变长）。
///
/// C1 起含 publisher / expires_at（防 metadata 篡改/降级）。
pub fn build_signed_meta(
    format_tag: u8,
    flags: u16,
    signed_at: u64,
    key_id: u32,
    key_fp: &[u8; PUBKEY_LEN],
    content_hash: &[u8; 32],
    publisher: Option<&str>,
    expires_at: Option<u64>,
) -> Vec<u8> {
    let mut m = Vec::new();
    m.push(FORMAT_VER);
    m.push(SIG_ALGO_ED25519);
    m.push(ENC_ALGO_CHACHA20);
    m.push(format_tag);
    m.extend_from_slice(&flags.to_le_bytes());
    m.extend_from_slice(&signed_at.to_le_bytes());
    m.extend_from_slice(&key_id.to_le_bytes());
    m.extend_from_slice(key_fp);
    m.extend_from_slice(content_hash);
    // expires_at: 1B has + 8B value
    m.push(if expires_at.is_some() { 1u8 } else { 0u8 });
    m.extend_from_slice(&expires_at.unwrap_or(0).to_le_bytes());
    // publisher: 2B len + bytes（len=0 表示无）
    if let Some(p) = publisher {
        m.extend_from_slice(&(p.len() as u16).to_le_bytes());
        m.extend_from_slice(p.as_bytes());
    } else {
        m.extend_from_slice(&0u16.to_le_bytes());
    }
    m
}

/// 拼装完整 envelope：`[ 密文 body ][ padding ][ footer ]`，total_len 从 footer 读取。
///
/// footer 由调用方预先构造（签名流程需先用 `footer[..FOOTER_AAD_LEN]` 作 AEAD AAD
/// 再加密 body，故 footer 必须在加密前就绪）。
///
/// 注意：padding 字节值不被 AEAD 覆盖（无意义填充，改动不影响安全性）；但 padding
/// **长度**由 footer 的 total_len / body_len 派生，footer 经 AEAD AAD 保护，故 padding
/// 长度受保护（攻击者无法通过改 footer 改 padding 长度而不被察觉）。
pub fn assemble_envelope(ciphertext: &[u8], footer: &[u8; FOOTER_LEN]) -> Vec<u8> {
    let parsed = parse_footer(footer).expect("assemble_envelope: footer must be pre-built and valid");
    let total_len = parsed.total_len;
    let body_len = ciphertext.len();
    debug_assert_eq!(body_len, parsed.body_len);
    let padding = total_len - body_len - FOOTER_LEN;
    let mut env = Vec::with_capacity(total_len);
    env.extend_from_slice(ciphertext);
    env.extend(std::iter::repeat(0u8).take(padding));
    env.extend_from_slice(footer);
    env
}

/// 在 overlay 区从末尾往前扫描，定位最近的一个我们的 footer magic（返回其偏移）。
///
/// - `bytes`：完整文件字节
/// - `overlay_start`：L（overlay 起点，PE/ELF）；Raw 传 0
/// - `excludes`：扫描时跳过的区间（PE 的 Authenticode 区域）
///
/// 只匹配 8B magic、不在此处验 crc——交给调用方 `parse_footer`，以便 crc 不符时
/// 报 [`VerifyOutcome::Tampered`](crate::VerifyOutcome::Tampered) 而非静默跳过（误判为无签名）。
/// 后续 footer_crc32 + AEAD + Ed25519 多重校验防 magic 误匹配。
pub fn find_our_footer(
    bytes: &[u8],
    overlay_start: usize,
    excludes: &[(usize, usize)],
) -> Option<usize> {
    if bytes.len() < overlay_start + FOOTER_LEN {
        return None;
    }
    let mut pos = bytes.len() - FOOTER_LEN;
    loop {
        if pos < overlay_start {
            break;
        }
        let in_exclude = excludes.iter().any(|(s, e)| pos >= *s && pos < *e);
        if !in_exclude && bytes.get(pos..pos + 8) == Some(&TRAILER_MAGIC[..]) {
            return Some(pos);
        }
        if pos == 0 {
            break;
        }
        pos -= 1;
    }
    None
}
