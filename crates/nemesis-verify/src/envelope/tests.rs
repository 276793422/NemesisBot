//! Envelope v3 字节结构单测（S6 覆盖率批次，quality-hardening goal 2026-08-25）。
//!
//! 覆盖：footer magic/CRC 拒绝、body 全字段往返（含 ts_token / key_not_after）、
//! body 过短、截断 TLV 容忍、footer 定位（overlay 下界 / 耗尽 / 排除区 / 多签名）、
//! assemble + envelope_body_range 拼装一致性、align_up。

use super::*;

fn sample_footer(tag: u8) -> [u8; FOOTER_LEN] {
    build_footer(tag, ENVELOPE_ALIGN, 300, 12345)
}

fn sample_body() -> Vec<u8> {
    build_body(
        0x0001,
        424242,
        &[7u8; PUBKEY_LEN],
        &[9u8; 32],
        &[3u8; PUBKEY_LEN],
        &[5u8; ED25519_SIG_LEN],
        Some(b"chain-bytes"),
        Some("acme"),
        Some(9_000_000_000),
        Some(b"tsa-token"),
    )
}

// ---------------------------------------------------------------------------
// footer
// ---------------------------------------------------------------------------

#[test]
fn footer_roundtrip_fields() {
    let f = sample_footer(1);
    let p = parse_footer(&f).expect("built footer must parse");
    assert_eq!(p.format_ver, FORMAT_VER);
    assert_eq!(p.sig_algo, SIG_ALGO_ED25519);
    assert_eq!(p.format_tag, 1);
    assert_eq!(p.total_len, ENVELOPE_ALIGN);
    assert_eq!(p.body_len, 300);
    assert_eq!(p.content_len, 12345);
}

#[test]
fn parse_footer_rejects_wrong_magic() {
    let mut f = sample_footer(2);
    f[0] = b'X';
    let err = parse_footer(&f).unwrap_err();
    assert!(format!("{err:#}").contains("magic"), "{err:#}");
}

#[test]
fn parse_footer_rejects_crc_mismatch() {
    let mut f = sample_footer(3);
    // 改 total_len 一个字节（在 CRC 覆盖范围 [0..24) 内）但不重算 CRC。
    f[OFF_TOTAL_LEN] ^= 0x01;
    let err = parse_footer(&f).unwrap_err();
    assert!(format!("{err:#}").contains("crc32"), "{err:#}");
}

// ---------------------------------------------------------------------------
// body
// ---------------------------------------------------------------------------

#[test]
fn body_roundtrip_all_optional_fields() {
    let body = sample_body();
    let p = parse_body(&body).expect("parse");
    assert_eq!(p.body_ver, BODY_VER);
    assert_eq!(p.flags, 0x0001);
    assert_eq!(p.signed_at, 424242);
    assert_eq!(p.key_fp, [7u8; PUBKEY_LEN]);
    assert_eq!(p.content_hash, [9u8; 32]);
    assert_eq!(p.pubkey, [3u8; PUBKEY_LEN]);
    assert_eq!(p.signature, [5u8; ED25519_SIG_LEN]);
    assert_eq!(p.cert_chain.as_deref(), Some(b"chain-bytes".as_slice()));
    assert_eq!(p.publisher.as_deref(), Some("acme"));
    assert_eq!(p.key_not_after, Some(9_000_000_000));
    assert_eq!(p.ts_token.as_deref(), Some(b"tsa-token".as_slice()));
    // sig_hash TLV = SHA-256(signature)
    use sha2::Digest;
    let expect: [u8; 32] = sha2::Sha256::digest([5u8; ED25519_SIG_LEN]).into();
    assert_eq!(p.sig_hash, expect);
}

#[test]
fn body_without_optionals_parses_none() {
    let body = build_body(
        0,
        1,
        &[0u8; PUBKEY_LEN],
        &[0u8; 32],
        &[1u8; PUBKEY_LEN],
        &[2u8; ED25519_SIG_LEN],
        None,
        None,
        None,
        None,
    );
    let p = parse_body(&body).unwrap();
    assert!(p.cert_chain.is_none());
    assert!(p.publisher.is_none());
    assert!(p.key_not_after.is_none(), "kna flag=0 → None");
    assert!(p.ts_token.is_none());
}

#[test]
fn parse_body_too_short_errors() {
    let short = vec![0u8; BODY_FIXED_LEN - 1];
    let err = parse_body(&short).unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("body too short"), "{msg}");
}

#[test]
fn parse_body_ignores_truncated_trailing_tlv() {
    // build_body 把 ts_token 放在最后；截掉其 value 尾部 2 字节 →
    // parse_tlvs 的 `i + 4 + l > len` break（容忍截断，不报错、不误读）。
    let mut body = sample_body();
    body.truncate(body.len() - 2);
    let p = parse_body(&body).expect("truncated tail TLV must be ignored");
    // ts TLV 解不出来 → None；前面的字段不受影响。
    assert!(p.ts_token.is_none(), "截断的 ts_token 必须丢弃");
    assert_eq!(p.publisher.as_deref(), Some("acme"));
    assert_eq!(p.key_not_after, Some(9_000_000_000));
}

#[test]
fn parse_body_ignores_unknown_tlv() {
    // 手工在合法 body 后追加一个未知类型 TLV → 忽略（向前兼容）。
    let mut body = sample_body();
    write_tlv(&mut body, 0x7F7F, b"future");
    let p = parse_body(&body).expect("unknown TLV must be ignored");
    assert_eq!(p.publisher.as_deref(), Some("acme"));
    assert_eq!(p.ts_token.as_deref(), Some(b"tsa-token".as_slice()));
}

#[test]
fn parse_body_tlv_short_value_skips_branches() {
    // kna TLV value != 9 字节 → 整个分支跳过（guard v.len()==9）。
    let mut body = build_body(
        0,
        1,
        &[0u8; PUBKEY_LEN],
        &[0u8; 32],
        &[1u8; PUBKEY_LEN],
        &[2u8; ED25519_SIG_LEN],
        None,
        None,
        None,
        None,
    );
    // 覆盖掉末尾追加的畸形 kna：直接再写一个 9!=len 的 kna TLV。
    write_tlv(&mut body, TLV_KEY_NOT_AFTER, &[1u8, 2, 3]); // len=3 ≠ 9
    let p = parse_body(&body).unwrap();
    assert!(p.key_not_after.is_none(), "非 9B 的 kna TLV 必须忽略");
}

// ---------------------------------------------------------------------------
// assemble + range
// ---------------------------------------------------------------------------

#[test]
fn assemble_and_body_range_roundtrip() {
    let body = sample_body();
    let total = align_up(body.len() + FOOTER_LEN, ENVELOPE_ALIGN);
    let footer = build_footer(3, total, body.len(), 98765);
    let env = assemble_envelope(&body, &footer);
    assert_eq!(env.len(), total, "envelope 总长 = 4KB 倍数");

    // footer 落在 envelope 末尾。
    let mut f = [0u8; FOOTER_LEN];
    f.copy_from_slice(&env[env.len() - FOOTER_LEN..]);
    let parsed = parse_footer(&f).unwrap();

    // body_range 指向的切片必须与原 body 字节一致。
    let (s, e) = envelope_body_range(env.len() - FOOTER_LEN, &parsed);
    assert_eq!(&env[s..e], &body[..]);
    // envelope 起点 = total - total_len = 0（env 本身就是完整 envelope）。
    assert_eq!(s, 0);
    assert_eq!(e, body.len());
}

#[test]
fn align_up_semantics() {
    assert_eq!(align_up(0, ENVELOPE_ALIGN), 0);
    assert_eq!(align_up(1, ENVELOPE_ALIGN), ENVELOPE_ALIGN);
    assert_eq!(align_up(ENVELOPE_ALIGN, ENVELOPE_ALIGN), ENVELOPE_ALIGN);
    assert_eq!(
        align_up(ENVELOPE_ALIGN + 1, ENVELOPE_ALIGN),
        ENVELOPE_ALIGN * 2
    );
}

// ---------------------------------------------------------------------------
// footer 定位
// ---------------------------------------------------------------------------

fn with_footer_at(content_len: usize, footer_pos: usize) -> Vec<u8> {
    let mut v = vec![0u8; content_len + FOOTER_LEN];
    v[footer_pos..footer_pos + 8].copy_from_slice(&TRAILER_MAGIC);
    v
}

#[test]
fn find_our_footer_finds_magic_before_overlay_bound() {
    // magic 在 60，overlay 从 0 扫描起点在 len-64=64 → 命中 60。
    let bytes = with_footer_at(120, 60);
    assert_eq!(find_our_footer(&bytes, 0, &[]), Some(60));
}

#[test]
fn find_our_footer_none_when_magic_below_overlay_start() {
    // magic 在 50，overlay_start=60：扫描到 60 就触底 break，永远看不到 50。
    let bytes = with_footer_at(120, 50);
    assert_eq!(find_our_footer(&bytes, 60, &[]), None);
}

#[test]
fn find_our_footer_exhausts_to_zero_when_no_magic() {
    let bytes = vec![0x41u8; 100]; // 无 magic，overlay_start=0 → 扫到 0 退出。
    assert_eq!(find_our_footer(&bytes, 0, &[]), None);
    assert!(bytes.len() >= FOOTER_LEN, "len>=64 才能进入扫描");
}

#[test]
fn find_our_footer_skips_excluded_region() {
    // magic 在 60，但 (0, 100) 全在排除区 → 找不到。
    let bytes = with_footer_at(120, 60);
    assert_eq!(find_our_footer(&bytes, 0, &[(0, 100)]), None);
    // 排除区不覆盖 60 时照常命中。
    assert_eq!(find_our_footer(&bytes, 0, &[(0, 50)]), Some(60));
}

#[test]
fn find_our_footer_too_short_returns_none() {
    let bytes = vec![0u8; 10];
    assert_eq!(find_our_footer(&bytes, 0, &[]), None);
}

#[test]
fn find_all_footers_short_bytes_is_empty() {
    // len < FOOTER_LEN → checked_sub None → 空。
    assert!(find_all_footers(&[0u8; 63], 0, &[]).is_empty());
}

#[test]
fn find_all_footers_returns_newest_first() {
    // 扫描窗口从 len-FOOTER_LEN 起往下：magic 只放在可达窗口内（10 与 50）。
    // 返回 [50, 10]（索引 0 = 最近签名）。
    let mut bytes = vec![0u8; 120];
    bytes[10..18].copy_from_slice(&TRAILER_MAGIC);
    bytes[50..58].copy_from_slice(&TRAILER_MAGIC);
    let found = find_all_footers(&bytes, 0, &[]);
    assert_eq!(found, vec![50, 10]);
}

#[test]
fn find_all_footers_respects_overlay_and_excludes() {
    let mut bytes = vec![0u8; 120];
    bytes[10..18].copy_from_slice(&TRAILER_MAGIC);
    bytes[50..58].copy_from_slice(&TRAILER_MAGIC);
    // overlay 之下（10）不可见 → 只剩 50。
    assert_eq!(find_all_footers(&bytes, 40, &[]), vec![50]);
    // 排除区盖住 50 → 只剩 10。
    assert_eq!(find_all_footers(&bytes, 0, &[(45, 100)]), vec![10]);
}
