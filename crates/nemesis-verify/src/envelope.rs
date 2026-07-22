//! Envelope v3（签名块）字节结构、读写与定位。
//!
//! envelope 追加在可执行文件 overlay 区末尾（Raw 格式则在文件末尾），
//! 整体对齐到 4KB 倍数。布局：`[ 明文 body ][ padding ][ 明文 footer ]`，
//! footer 固定 64B 在 envelope 末尾，作定位锚点。
//!
//! # v3 vs v1/v2
//! - **去 AEAD**：body 明文（不再 ChaCha20-Poly1305 加密），footer 去 nonce 字段
//! - **公钥随签名走**：body 加 `pubkey`(32B) 字段；`cert_chain` 走 TLV
//! - magic 升 `NMBSIG\x03\x00`；format_ver=3；body_ver=2
//! - 破坏性升级，不兼容 v1/v2
//!
//! # FOOTER（明文 64B）
//! | 偏移 | 长度 | 字段 |
//! |------|------|------|
//! | 0  | 8  | magic `NMBSIG\x03\x00` |
//! | 8  | 1  | format_ver（=3）|
//! | 9  | 1  | sig_algo（1=ed25519）|
//! | 10 | 1  | format_tag（1=PE / 2=ELF / 3=Raw）|
//! | 11 | 1  | reserved（0）|
//! | 12 | 4  | total_len（envelope 总长，4KB 倍数，LE）|
//! | 16 | 4  | body_len（明文 body 长，LE）|
//! | 20 | 4  | content_len（被保护原始内容长度，LE）|
//! | 24 | 4  | footer_crc32（footer\[0..24\] 校验，LE）|
//! | 28 | 36 | reserved（0）|
//!
//! # BODY（明文；固定前缀 172B + TLV 扩展）
//! | 偏移 | 长度 | 字段 |
//! |------|------|------|
//! | 0   | 1  | body_ver（=2）|
//! | 1   | 1  | reserved |
//! | 2   | 2  | flags |
//! | 4   | 8  | signed_at（Unix epoch，LE）|
//! | 12  | 32 | key_fp（公钥 SHA-256 指纹，CRL/trusted 索引）|
//! | 44  | 32 | content_hash（程序内容 SHA-256）|
//! | 76  | 32 | **pubkey**（签名公钥，随签名走）★ |
//! | 108 | 64 | signature（Ed25519 签名）|
//! | 172 | .. | TLV 扩展（cert_chain / publisher / key_not_after / sig_hash / ts_token）|
//!
//! # signed_meta（Ed25519 签名覆盖范围）
//! `format_ver | sig_algo | format_tag | flags | signed_at | key_fp | content_hash |
//!  pubkey | cert_chain_hash | has_key_not_after | key_not_after | publisher_len | publisher`
//! 签名消息 = `DOMAIN ++ signed_meta`。覆盖 pubkey + cert_chain_hash 防 metadata 篡改/降级。

use anyhow::{Result, anyhow};

/// domain 前缀（41 ASCII + 0x01 = 42 字节）。签名消息以此开头做 domain separation。
pub const DOMAIN: &[u8] = b"NEMESIS-BOT-276793422-ZHAO-SAN-KE-BIN-SIG\x01";
/// envelope 末尾 magic（8B，v3）。
pub const TRAILER_MAGIC: [u8; 8] = *b"NMBSIG\x03\x00";

/// footer 固定长度。
pub const FOOTER_LEN: usize = 64;
/// footer_crc32 覆盖范围（footer[0..24]，即 magic..content_len）。
pub const FOOTER_CRC_LEN: usize = 24;
/// envelope 总长对齐粒度（内存页）。
pub const ENVELOPE_ALIGN: usize = 4096;

/// envelope 格式版本（v3）。
pub const FORMAT_VER: u8 = 3;
pub const SIG_ALGO_ED25519: u8 = 1;
/// body 固定前缀长度（v3：含 pubkey）。
pub const BODY_FIXED_LEN: usize = 172;
pub const BODY_VER: u8 = 2;

pub const ED25519_SIG_LEN: usize = 64;
pub const PUBKEY_LEN: usize = 32;

/// body TLV 类型。
/// cert_chain：完整证书链（leaf + intermediates，序列化字节）。
pub const TLV_CERT_CHAIN: u16 = 0x20;
/// publisher：发布者字符串。
pub const TLV_PUBLISHER: u16 = 0x10;
/// key_not_after：value = 1B has_flag + 8B u64 LE。
pub const TLV_KEY_NOT_AFTER: u16 = 0x13;
/// sig_hash：value = 32B SHA-256(signature)，供云端吊销查。
pub const TLV_SIG_HASH: u16 = 0x14;
/// ts_token：TSA 时间戳（D4，可选）。
pub const TLV_TS_TOKEN: u16 = 0x15;

// footer 字段偏移
const OFF_MAGIC: usize = 0;
const OFF_FORMAT_VER: usize = 8;
const OFF_SIG_ALGO: usize = 9;
const OFF_FORMAT_TAG: usize = 10;
const OFF_TOTAL_LEN: usize = 12;
const OFF_BODY_LEN: usize = 16;
const OFF_CONTENT_LEN: usize = 20;
const OFF_CRC: usize = 24;

// body 字段偏移
const BODY_OFF_VER: usize = 0;
const BODY_OFF_FLAGS: usize = 2;
const BODY_OFF_SIGNED_AT: usize = 4;
const BODY_OFF_KEY_FP: usize = 12;
const BODY_OFF_CONTENT_HASH: usize = 44;
const BODY_OFF_PUBKEY: usize = 76;
const BODY_OFF_SIG: usize = 108;

/// IEEE 802.3 CRC32（footer 完整性快速校验，非密码学用途）。
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

fn rd_u16(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}
fn rd_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}
fn rd_u64(b: &[u8], off: usize) -> u64 {
    u64::from_le_bytes([
        b[off],
        b[off + 1],
        b[off + 2],
        b[off + 3],
        b[off + 4],
        b[off + 5],
        b[off + 6],
        b[off + 7],
    ])
}

/// 构造 footer（v3，无 nonce）。
pub fn build_footer(
    format_tag: u8,
    total_len: usize,
    body_len: usize,
    content_len: usize,
) -> [u8; FOOTER_LEN] {
    let mut f = [0u8; FOOTER_LEN];
    f[OFF_MAGIC..OFF_MAGIC + 8].copy_from_slice(&TRAILER_MAGIC);
    f[OFF_FORMAT_VER] = FORMAT_VER;
    f[OFF_SIG_ALGO] = SIG_ALGO_ED25519;
    f[OFF_FORMAT_TAG] = format_tag;
    // OFF 11 reserved = 0
    f[OFF_TOTAL_LEN..OFF_TOTAL_LEN + 4].copy_from_slice(&(total_len as u32).to_le_bytes());
    f[OFF_BODY_LEN..OFF_BODY_LEN + 4].copy_from_slice(&(body_len as u32).to_le_bytes());
    f[OFF_CONTENT_LEN..OFF_CONTENT_LEN + 4].copy_from_slice(&(content_len as u32).to_le_bytes());
    let crc = crc32(&f[0..FOOTER_CRC_LEN]);
    f[OFF_CRC..OFF_CRC + 4].copy_from_slice(&crc.to_le_bytes());
    f
}

/// 解析后的 footer。
#[derive(Debug)]
pub struct ParsedFooter {
    pub format_ver: u8,
    pub sig_algo: u8,
    pub format_tag: u8,
    pub total_len: usize,
    pub body_len: usize,
    pub content_len: usize,
}

/// 解析 footer（校验 magic + crc32）。
pub fn parse_footer(bytes: &[u8; FOOTER_LEN]) -> Result<ParsedFooter> {
    if &bytes[OFF_MAGIC..OFF_MAGIC + 8] != &TRAILER_MAGIC {
        return Err(anyhow!("footer magic mismatch"));
    }
    let stored = rd_u32(bytes, OFF_CRC);
    let calc = crc32(&bytes[0..FOOTER_CRC_LEN]);
    if stored != calc {
        return Err(anyhow!("footer crc32 mismatch"));
    }
    Ok(ParsedFooter {
        format_ver: bytes[OFF_FORMAT_VER],
        sig_algo: bytes[OFF_SIG_ALGO],
        format_tag: bytes[OFF_FORMAT_TAG],
        total_len: rd_u32(bytes, OFF_TOTAL_LEN) as usize,
        body_len: rd_u32(bytes, OFF_BODY_LEN) as usize,
        content_len: rd_u32(bytes, OFF_CONTENT_LEN) as usize,
    })
}

/// 给定 footer 偏移与解析结果，返回明文 body 的 `[start, end)`。
pub fn envelope_body_range(footer_offset: usize, parsed: &ParsedFooter) -> (usize, usize) {
    let envelope_start = footer_offset + FOOTER_LEN - parsed.total_len;
    (envelope_start, envelope_start + parsed.body_len)
}

fn write_tlv(buf: &mut Vec<u8>, t: u16, value: &[u8]) {
    buf.extend_from_slice(&t.to_le_bytes());
    buf.extend_from_slice(&(value.len() as u16).to_le_bytes());
    buf.extend_from_slice(value);
}

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

/// 构造 body 明文 = 固定前缀(172B) + TLV 扩展。
///
/// `cert_chain`：完整证书链序列化字节（None = 无链，单根自签场景）。
pub fn build_body(
    flags: u16,
    signed_at: u64,
    key_fp: &[u8; PUBKEY_LEN],
    content_hash: &[u8; 32],
    pubkey: &[u8; PUBKEY_LEN],
    signature: &[u8; ED25519_SIG_LEN],
    cert_chain: Option<&[u8]>,
    publisher: Option<&str>,
    key_not_after: Option<u64>,
    ts_token: Option<&[u8]>,
) -> Vec<u8> {
    let mut b = vec![0u8; BODY_FIXED_LEN];
    b[BODY_OFF_VER] = BODY_VER;
    b[BODY_OFF_FLAGS..BODY_OFF_FLAGS + 2].copy_from_slice(&flags.to_le_bytes());
    b[BODY_OFF_SIGNED_AT..BODY_OFF_SIGNED_AT + 8].copy_from_slice(&signed_at.to_le_bytes());
    b[BODY_OFF_KEY_FP..BODY_OFF_KEY_FP + PUBKEY_LEN].copy_from_slice(key_fp);
    b[BODY_OFF_CONTENT_HASH..BODY_OFF_CONTENT_HASH + 32].copy_from_slice(content_hash);
    b[BODY_OFF_PUBKEY..BODY_OFF_PUBKEY + PUBKEY_LEN].copy_from_slice(pubkey);
    b[BODY_OFF_SIG..BODY_OFF_SIG + ED25519_SIG_LEN].copy_from_slice(signature);

    // sig_hash = SHA-256(signature)，供云端吊销查
    use sha2::Digest;
    let sig_hash: [u8; 32] = sha2::Sha256::digest(signature).into();
    write_tlv(&mut b, TLV_SIG_HASH, &sig_hash);

    if let Some(chain) = cert_chain {
        write_tlv(&mut b, TLV_CERT_CHAIN, chain);
    }
    if let Some(p) = publisher {
        write_tlv(&mut b, TLV_PUBLISHER, p.as_bytes());
    }
    let mut kna_buf = Vec::with_capacity(9);
    kna_buf.push(if key_not_after.is_some() { 1u8 } else { 0u8 });
    kna_buf.extend_from_slice(&key_not_after.unwrap_or(0).to_le_bytes());
    write_tlv(&mut b, TLV_KEY_NOT_AFTER, &kna_buf);
    if let Some(ts) = ts_token {
        write_tlv(&mut b, TLV_TS_TOKEN, ts);
    }
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
    let mut content_hash = [0u8; 32];
    content_hash.copy_from_slice(&plaintext[BODY_OFF_CONTENT_HASH..BODY_OFF_CONTENT_HASH + 32]);
    let mut pubkey = [0u8; PUBKEY_LEN];
    pubkey.copy_from_slice(&plaintext[BODY_OFF_PUBKEY..BODY_OFF_PUBKEY + PUBKEY_LEN]);
    let mut signature = [0u8; ED25519_SIG_LEN];
    signature.copy_from_slice(&plaintext[BODY_OFF_SIG..BODY_OFF_SIG + ED25519_SIG_LEN]);

    let mut publisher: Option<String> = None;
    let mut key_not_after: Option<u64> = None;
    let mut sig_hash = [0u8; 32];
    let mut cert_chain: Option<Vec<u8>> = None;
    let mut ts_token: Option<Vec<u8>> = None;
    for (t, v) in parse_tlvs(&plaintext[BODY_FIXED_LEN..]) {
        match t {
            TLV_SIG_HASH if v.len() == 32 => {
                sig_hash.copy_from_slice(&v);
            }
            TLV_CERT_CHAIN => cert_chain = Some(v),
            TLV_PUBLISHER => {
                publisher =
                    Some(String::from_utf8(v).map_err(|e| anyhow!("publisher utf8: {}", e))?);
            }
            TLV_KEY_NOT_AFTER if v.len() == 9 => {
                if v[0] == 1 {
                    key_not_after = Some(u64::from_le_bytes([
                        v[1], v[2], v[3], v[4], v[5], v[6], v[7], v[8],
                    ]));
                }
            }
            TLV_TS_TOKEN => ts_token = Some(v),
            _ => {} // 忽略未知 TLV（向前兼容）
        }
    }
    Ok(ParsedBody {
        body_ver: plaintext[BODY_OFF_VER],
        flags: rd_u16(plaintext, BODY_OFF_FLAGS),
        signed_at: rd_u64(plaintext, BODY_OFF_SIGNED_AT),
        key_fp,
        content_hash,
        pubkey,
        signature,
        sig_hash,
        cert_chain,
        publisher,
        key_not_after,
        ts_token,
    })
}

/// 解析后的 body（v3）。
#[derive(Debug)]
pub struct ParsedBody {
    pub body_ver: u8,
    pub flags: u16,
    pub signed_at: u64,
    pub key_fp: [u8; PUBKEY_LEN],
    pub content_hash: [u8; 32],
    /// 签名公钥（随签名走；验证用它验签，链验证确认其可信）。
    pub pubkey: [u8; PUBKEY_LEN],
    pub signature: [u8; ED25519_SIG_LEN],
    pub sig_hash: [u8; 32],
    /// 完整证书链（leaf + intermediates，除根）。None = 无链（单根自签）。
    pub cert_chain: Option<Vec<u8>>,
    pub publisher: Option<String>,
    pub key_not_after: Option<u64>,
    pub ts_token: Option<Vec<u8>>,
}

/// 构造 signed_meta（Ed25519 签名覆盖范围）。
///
/// `cert_chain_hash`：SHA-256(cert_chain)，无链则传 `[0u8;32]`。
pub fn build_signed_meta(
    format_tag: u8,
    flags: u16,
    signed_at: u64,
    key_fp: &[u8; PUBKEY_LEN],
    content_hash: &[u8; 32],
    pubkey: &[u8; PUBKEY_LEN],
    cert_chain_hash: &[u8; 32],
    key_not_after: Option<u64>,
    publisher: Option<&str>,
) -> Vec<u8> {
    let mut m = Vec::new();
    m.push(FORMAT_VER);
    m.push(SIG_ALGO_ED25519);
    m.push(format_tag);
    m.extend_from_slice(&flags.to_le_bytes());
    m.extend_from_slice(&signed_at.to_le_bytes());
    m.extend_from_slice(key_fp);
    m.extend_from_slice(content_hash);
    m.extend_from_slice(pubkey);
    m.extend_from_slice(cert_chain_hash);
    // key_not_after: 1B has + 8B value
    m.push(if key_not_after.is_some() { 1u8 } else { 0u8 });
    m.extend_from_slice(&key_not_after.unwrap_or(0).to_le_bytes());
    // publisher: 2B len + bytes
    if let Some(p) = publisher {
        m.extend_from_slice(&(p.len() as u16).to_le_bytes());
        m.extend_from_slice(p.as_bytes());
    } else {
        m.extend_from_slice(&0u16.to_le_bytes());
    }
    m
}

/// 拼装完整 envelope：`[ 明文 body ][ padding ][ footer ]`。
///
/// `total_len` 从 footer 读取（调用方须先按 `align_up(body_len + FOOTER_LEN, 4096)` 算好并传入 footer）。
pub fn assemble_envelope(body: &[u8], footer: &[u8; FOOTER_LEN]) -> Vec<u8> {
    let parsed =
        parse_footer(footer).expect("assemble_envelope: footer must be pre-built and valid");
    let total_len = parsed.total_len;
    let padding = total_len - body.len() - FOOTER_LEN;
    let mut env = Vec::with_capacity(total_len);
    env.extend_from_slice(body);
    env.extend(std::iter::repeat(0u8).take(padding));
    env.extend_from_slice(footer);
    env
}

/// 在 overlay 区从末尾往前扫描，定位最近的一个我们的 footer magic（返回其偏移）。
///
/// - `bytes`：完整文件字节
/// - `overlay_start`：L（overlay 起点，PE/ELF）；Raw 传 0
/// - `excludes`：扫描时跳过的区间（PE 的 Authenticode 区域）
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

/// 扫描 overlay 区**所有** footer magic（多签名场景，返回所有偏移；顺序：从文件末尾往前，
/// 即索引 0 = 最近签名）。
pub fn find_all_footers(
    bytes: &[u8],
    overlay_start: usize,
    excludes: &[(usize, usize)],
) -> Vec<usize> {
    let mut found = Vec::new();
    let mut pos = match bytes.len().checked_sub(FOOTER_LEN) {
        Some(p) => p,
        None => return found,
    };
    loop {
        if pos < overlay_start {
            break;
        }
        let in_exclude = excludes.iter().any(|(s, e)| pos >= *s && pos < *e);
        if !in_exclude && bytes.get(pos..pos + 8) == Some(&TRAILER_MAGIC[..]) {
            found.push(pos);
        }
        if pos == 0 {
            break;
        }
        pos -= 1;
    }
    found
}
