//! 查看接口（离线展示签名 + 证书链，**不下结论**）。
//!
//! 对应"查看 vs 验证分离"的查看侧（Windows 文件属性→数字签名 tab 的"查看证书"）：
//! 纯本地解析，不联网、不查吊销、不下 Valid/Revoked 结论——只展示签名元数据 + 证书链。

use crate::{cert, codec, envelope};

/// 单签名摘要（列表项）。
#[derive(Debug)]
pub struct SigInfo {
    /// 索引（0 = 最近签名，从文件末尾往前）。
    pub index: usize,
    pub signed_at: u64,
    pub key_fp: [u8; 32],
    pub pubkey: [u8; 32],
}

/// 单签名详情（含证书链逐级）。
#[derive(Debug)]
pub struct SigDetail {
    pub info: SigInfo,
    /// 证书链（leaf + intermediates，envelope.cert_chain 解析；空 = 无链/单根自签）。
    pub certs: Vec<cert::Certificate>,
    /// publisher（签给谁/发布者，envelope body）
    pub publisher: Option<String>,
}

/// 列所有签名（多签名场景，索引 0 = 最近）。
pub fn list_signatures(bytes: &[u8]) -> Vec<SigInfo> {
    let (overlay_start, excludes) = overlay_context(bytes);
    envelope::find_all_footers(bytes, overlay_start, &excludes)
        .iter()
        .enumerate()
        .filter_map(|(i, &foff)| parse_sig_info(bytes, foff, i))
        .collect()
}

/// 单签名详情（含 cert chain）。
pub fn get_signature_detail(bytes: &[u8], index: usize) -> Option<SigDetail> {
    let (overlay_start, excludes) = overlay_context(bytes);
    let footers = envelope::find_all_footers(bytes, overlay_start, &excludes);
    let foff = *footers.get(index)?;
    let mut footer = [0u8; envelope::FOOTER_LEN];
    footer.copy_from_slice(&bytes[foff..foff + envelope::FOOTER_LEN]);
    let pf = envelope::parse_footer(&footer).ok()?;
    let (bs, be) = envelope::envelope_body_range(foff, &pf);
    let body = envelope::parse_body(&bytes[bs..be]).ok()?;
    let certs = body
        .cert_chain
        .as_deref()
        .and_then(|c| cert::parse_chain(c).ok())
        .unwrap_or_default();
    Some(SigDetail {
        info: SigInfo {
            index,
            signed_at: body.signed_at,
            key_fp: body.key_fp,
            pubkey: body.pubkey,
        },
        certs,
        publisher: body.publisher,
    })
}

/// 提取最新签名的 sig_hash（= SHA-256(signature)，envelope body 的 sig_hash TLV）。
/// 供 revoke-server registry 登记 / CRL 单签名吊销维度。
pub fn latest_sig_hash(bytes: &[u8]) -> Option<[u8; 32]> {
    let (overlay_start, excludes) = overlay_context(bytes);
    let foff = envelope::find_our_footer(bytes, overlay_start, &excludes)?;
    let mut footer = [0u8; envelope::FOOTER_LEN];
    footer.copy_from_slice(&bytes[foff..foff + envelope::FOOTER_LEN]);
    let pf = envelope::parse_footer(&footer).ok()?;
    let (bs, be) = envelope::envelope_body_range(foff, &pf);
    let body = envelope::parse_body(&bytes[bs..be]).ok()?;
    Some(body.sig_hash)
}

fn overlay_context(bytes: &[u8]) -> (usize, Vec<(usize, usize)>) {
    let codec = codec::detect_codec(bytes);
    let overlay_start = codec.compute_l(bytes).ok().flatten().unwrap_or(0);
    (overlay_start, codec.overlay_excludes(bytes))
}

fn parse_sig_info(bytes: &[u8], foff: usize, index: usize) -> Option<SigInfo> {
    let mut footer = [0u8; envelope::FOOTER_LEN];
    footer.copy_from_slice(&bytes[foff..foff + envelope::FOOTER_LEN]);
    let pf = envelope::parse_footer(&footer).ok()?;
    let (bs, be) = envelope::envelope_body_range(foff, &pf);
    let body = envelope::parse_body(&bytes[bs..be]).ok()?;
    Some(SigInfo {
        index,
        signed_at: body.signed_at,
        key_fp: body.key_fp,
        pubkey: body.pubkey,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keygen::generate_hierarchy;
    use crate::verify;

    #[test]
    fn list_single_signature() {
        let h = generate_hierarchy(0, u64::MAX);
        let signed = verify::sign_content(
            b"view-test-payload",
            &h.issuer_sk,
            12345,
            Some(&h.issuer_chain_bytes),
            None,
            None,
            None,
        )
        .unwrap();
        let list = list_signatures(&signed);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].signed_at, 12345);
        assert_eq!(list[0].pubkey, h.issuer_vk.to_bytes());
    }

    #[test]
    fn detail_includes_cert_chain() {
        let h = generate_hierarchy(0, u64::MAX);
        let signed = verify::sign_content(
            b"detail-test",
            &h.issuer_sk,
            999,
            Some(&h.issuer_chain_bytes),
            None,
            None,
            None,
        )
        .unwrap();
        let detail = get_signature_detail(&signed, 0).unwrap();
        // chain = [issuer_cert, ca_cert]（2 级）
        assert_eq!(detail.certs.len(), 2);
        assert_eq!(detail.certs[0].subject_pubkey, h.issuer_vk.to_bytes()); // leaf = issuer
        assert_eq!(detail.certs[1].subject_pubkey, h.ca_vk.to_bytes()); // intermediate = CA
    }

    #[test]
    fn no_signature_empty_list() {
        let list = list_signatures(b"plain bytes no signature");
        assert!(list.is_empty());
    }

    #[test]
    fn latest_sig_hash_extracted() {
        let h = generate_hierarchy(0, u64::MAX);
        let signed = verify::sign_content(
            b"sig-hash-test",
            &h.issuer_sk,
            111,
            Some(&h.issuer_chain_bytes),
            None,
            None,
            None,
        )
        .unwrap();
        let sig_hash = latest_sig_hash(&signed).unwrap();
        // build_body 时填入 SHA-256(signature)，非全 0
        assert_ne!(sig_hash, [0u8; 32]);
        // 无签名文件 → None
        assert!(latest_sig_hash(b"no signature here").is_none());
    }
}
