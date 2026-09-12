//! Envelope 单测：v4 CMS SignedData（S2-1）+ v4 locator footer（S2-3）+ ELF/raw
//! 载体（S3-4）。（v3 字节结构测试段随 v3 代码块 S5-3 整体删除——v3 形态的
//! 破坏性钉死改由 verify/view tests 的 footer 字节字面量用例承担。）

use super::*;

// ---------------------------------------------------------------------------
// S2-1：CMS SignedData（Authenticode 形态；构造 → 解析 roundtrip + 密码学自洽）
// ---------------------------------------------------------------------------

use cms::content_info::ContentInfo;
use cms::signed_data::{SignedData, SignerIdentifier};
use der::{Encode, Tagged};
use sha2::Digest;

fn cms_test_digest() -> [u8; 32] {
    sha2::Sha256::digest(b"S2-1 CMS roundtrip test content").into()
}

fn cms_test_build() -> (Vec<u8>, crate::keygen::KeyHierarchy) {
    let h = crate::keygen::generate().expect("keygen hierarchy");
    let der = build_signed_data(
        &cms_test_digest(),
        &h.leaf_sk,
        1_700_000_000,
        &h.chain(),
        Some("NemesisBot Code Signing"),
        Some("https://nemesisbot.example"),
    )
    .expect("build_signed_data");
    (der, h)
}

/// 解析 ContentInfo → SignedData（content 是 [0] EXPLICIT 剥壳后的 ANY——
/// to_der() 重组完整 TLV 才能喂 from_der，S0-6 坑位 5e）。
fn cms_parse(der: &[u8]) -> SignedData {
    let ci = ContentInfo::from_der(der).expect("ContentInfo parse");
    assert_eq!(
        ci.content_type, OID_SIGNED_DATA,
        "outer contentType = signedData"
    );
    SignedData::from_der(ci.content.to_der().unwrap().as_slice()).expect("SignedData parse")
}

#[test]
fn signed_data_roundtrip_authenticode_form() {
    let (der, _h) = cms_test_build();
    let sd = cms_parse(&der);

    // Authenticode 钉 version=1（cms builder 原生 V3 → 构造后类型化改写）
    assert_eq!(
        format!("{:?}", sd.version),
        "V1",
        "SignedData.version pinned V1"
    );
    assert_eq!(
        sd.encap_content_info.econtent_type, OID_SPC_INDIRECT_DATA,
        "eContentType = SPC_INDIRECT_DATA_OBJID"
    );
    // MS eContent 形态：[0] 里直接是 SpcIndirectDataContent SEQUENCE（无 OCTET STRING 包装）
    let econtent = sd
        .encap_content_info
        .econtent
        .as_ref()
        .expect("eContent present");
    assert_eq!(
        der::Tagged::tag(econtent),
        der::Tag::Sequence,
        "MS form: no OCTET STRING wrapper"
    );
    // P0 实证约束（S0-4）：证书集必须含全部三级（含根）
    assert_eq!(
        sd.certificates.as_ref().map(|c| c.0.as_slice().len()),
        Some(3usize),
        "certificate set must contain leaf+issuing+root"
    );
    assert_eq!(
        sd.signer_infos.0.as_slice().len(),
        1,
        "exactly one signer info"
    );
    let si = sd.signer_infos.0.iter().next().expect("signer info");
    assert!(
        matches!(si.sid, SignerIdentifier::IssuerAndSerialNumber(_)),
        "Authenticode requires IssuerAndSerialNumber sid"
    );

    // 认证属性四件套：contentType / messageDigest / signingTime / SPC_SP_OPUS_INFO
    let attrs = si.signed_attrs.as_ref().expect("signed attrs present");
    let oids: Vec<String> = attrs.iter().map(|a| a.oid.to_string()).collect();
    for want in [
        OID_ATTR_CONTENT_TYPE.to_string(),
        OID_ATTR_MESSAGE_DIGEST.to_string(),
        OID_ATTR_SIGNING_TIME.to_string(),
        OID_SPC_SP_OPUS_INFO.to_string(),
    ] {
        assert!(
            oids.contains(&want),
            "signed attr {want} missing, got {oids:?}"
        );
    }

    // messageDigest 语义 = SHA256(eContent 内容体无头)（osslsigncode 同口径，S0-6 实测命中）
    let md = attrs
        .iter()
        .find(|a| a.oid == OID_ATTR_MESSAGE_DIGEST)
        .expect("messageDigest attr");
    let attr_digest = md.values.iter().next().expect("digest value");
    assert_eq!(
        attr_digest.value(),
        sha2::Sha256::digest(econtent.value()).as_slice(),
        "messageDigest == SHA256(eContent value octets)"
    );

    // SpcIndirectDataContent roundtrip：data.type + digest 原值回读
    let econtent_der = econtent.to_der().unwrap();
    let spc = SpcIndirectDataContent::from_der(econtent_der.as_slice())
        .expect("SpcIndirectDataContent parse");
    assert_eq!(spc.data.type_, OID_SPC_PE_IMAGE_DATA);
    assert_eq!(spc.message_digest.digest.as_bytes(), &cms_test_digest()[..]);
    // digestAlgorithm 带 NULL parameters（osslsigncode 形态）
    assert_eq!(spc.message_digest.digest_algorithm.oid, OID_SHA256);
    assert!(
        spc.message_digest
            .digest_algorithm
            .parameters
            .as_ref()
            .map(|p| p.tag() == der::Tag::Null)
            .unwrap_or(false),
        "sha256 alg params = NULL"
    );
}

#[test]
fn signed_data_deterministic() {
    // RFC 6979 确定式签名 + signingTime 钉参数 + 无随机序列号 → 两次构造字节全同
    let h = crate::keygen::generate().expect("keygen");
    let a = build_signed_data(
        &cms_test_digest(),
        &h.leaf_sk,
        1_700_000_000,
        &h.chain(),
        Some("acme"),
        None,
    )
    .expect("build a");
    let b = build_signed_data(
        &cms_test_digest(),
        &h.leaf_sk,
        1_700_000_000,
        &h.chain(),
        Some("acme"),
        None,
    )
    .expect("build b");
    assert_eq!(a, b, "build_signed_data must be deterministic");
}

#[test]
fn cms_signature_verifies_over_signed_attrs() {
    // RFC 5652 §5.4：SignerInfo.signature 对 signedAttrs 验签，签名消息 = SET OF
    // （0x31）形态的 attrs DER。cms 的 SetOfAttrs::to_der() 直接产 SET OF 形态
    //（IMPLICIT [0] 只在 SignerInfo 编码层），无需手工换 tag。
    let (der, h) = cms_test_build();
    let sd = cms_parse(&der);
    let si = sd.signer_infos.0.iter().next().expect("signer info");
    let attrs = si.signed_attrs.as_ref().expect("signed attrs present");
    let msg = attrs.to_der().expect("attrs to_der");
    assert_eq!(
        msg[0], 0x31,
        "signedAttrs as SET OF (signature message form)"
    );

    let sig_bytes = si.signature.as_bytes();
    let der_sig = <ecdsa::der::Signature<p256::NistP256>>::try_from(sig_bytes)
        .expect("DER ECDSA signature parse");
    use ecdsa::signature::Verifier;
    h.leaf_vk()
        .verify(&msg, &der_sig)
        .expect("CMS signature must verify over signed attrs with signer pubkey");
}

#[test]
fn signed_data_without_opus_omits_opus_attr() {
    let h = crate::keygen::generate().expect("keygen");
    let der = build_signed_data(
        &cms_test_digest(),
        &h.leaf_sk,
        1_700_000_000,
        &h.chain(),
        None,
        None,
    )
    .expect("build without opus");
    let sd = cms_parse(&der);
    let si = sd.signer_infos.0.iter().next().expect("signer info");
    let attrs = si.signed_attrs.as_ref().expect("signed attrs present");
    assert!(
        !attrs.iter().any(|a| a.oid == OID_SPC_SP_OPUS_INFO),
        "opus attr absent when both fields None"
    );
    // 其余三件套仍在
    assert!(attrs.iter().any(|a| a.oid == OID_ATTR_MESSAGE_DIGEST));
    assert!(attrs.iter().any(|a| a.oid == OID_ATTR_SIGNING_TIME));
    assert!(attrs.iter().any(|a| a.oid == OID_ATTR_CONTENT_TYPE));
}

/// 与 S0-3 osslsigncode 产物字段级对比（goal S2-1 判据之二）。
///
/// 默认路径 = 本机 spike 产物（仓库外）；可用 `NEMESIS_OSSLSIG_DER` 覆盖。
/// 显式运行：`cargo test -p nemesis-verify compare_with_osslsigncode -- --ignored`
#[test]
#[ignore = "需要本机 S0-3 spike 产物 osslsig.der（仓库外）；--ignored 显式运行"]
fn compare_with_osslsigncode_artifact() {
    let path = std::env::var("NEMESIS_OSSLSIG_DER").unwrap_or_else(|_| {
        "C:\\AI\\NemesisBot\\Logs\\2026-09-12_authenticode-spike\\osslsig.der".to_string()
    });
    let bytes =
        std::fs::read(&path).unwrap_or_else(|e| panic!("osslsig.der 不可读（{path}）: {e}"));
    let sd = cms_parse(&bytes);

    // 结构形态逐字段对齐（S0-6 asn1parse 对比结论的结构化复刻）
    assert_eq!(
        format!("{:?}", sd.version),
        "V1",
        "osslsigncode: version V1"
    );
    assert_eq!(
        sd.encap_content_info.econtent_type, OID_SPC_INDIRECT_DATA,
        "eContentType = SPC_INDIRECT_DATA"
    );
    let econtent = sd.encap_content_info.econtent.as_ref().expect("eContent");
    assert_eq!(
        der::Tagged::tag(econtent),
        der::Tag::Sequence,
        "MS form: SEQUENCE direct"
    );
    // osslsig.der = S0-3 主样本形态：osslsigncode 全链签名（leaf+issuing+root，
    // spike README S0-3 行「全链签名」；S0-4 缺根对照是另做的 test-signed.exe）
    assert_eq!(
        sd.certificates.as_ref().map(|c| c.0.as_slice().len()),
        Some(3usize),
        "osslsigncode artifact = full chain leaf+issuing+root"
    );
    assert_eq!(sd.signer_infos.0.as_slice().len(), 1);
    let si = sd.signer_infos.0.iter().next().expect("signer info");
    assert!(matches!(si.sid, SignerIdentifier::IssuerAndSerialNumber(_)));
    let attrs = si.signed_attrs.as_ref().expect("signed attrs");
    let oids: Vec<String> = attrs.iter().map(|a| a.oid.to_string()).collect();
    for want in [
        "1.2.840.113549.1.9.3",
        "1.2.840.113549.1.9.4",
        "1.2.840.113549.1.9.5",
        "1.3.6.1.4.1.311.2.1.12",
    ] {
        assert!(
            oids.iter().any(|x| x == want),
            "attr {want} missing in {oids:?}"
        );
    }
    // messageDigest 语义命中：SHA256(eContent 内容体) == 属性值（S0-6 实测 7624F011... 同口径）
    let md = attrs
        .iter()
        .find(|a| a.oid.to_string() == "1.2.840.113549.1.9.4")
        .expect("messageDigest attr");
    let v = md.values.iter().next().expect("value");
    assert_eq!(
        v.value(),
        sha2::Sha256::digest(econtent.value()).as_slice(),
        "osslsigncode messageDigest == SHA256(eContent value)"
    );
    let econtent_der = econtent.to_der().unwrap();
    let spc = SpcIndirectDataContent::from_der(econtent_der.as_slice())
        .expect("SpcIndirectDataContent parse");
    assert_eq!(spc.data.type_, OID_SPC_PE_IMAGE_DATA);
    println!("OK: field-level comparison with osslsigncode artifact at {path}");
}

// ---------------------------------------------------------------------------
// S2-2：SignedData 解析（结构 + 自洽；畸形输入诚实失败）
// ---------------------------------------------------------------------------

use cms::content_info::CmsVersion;
use cms::signed_data::SignerInfos;
use der::asn1::SetOfVec;

/// 解析 → 类型化突变 → 重编码（构造畸形输入的通用路径；突变会破坏 CMS 签名
/// 本身，但 parse_signed_data 只做结构与自洽校验、不做密码学验签，正好隔离测试）。
fn reencode_sd(sd: &SignedData) -> Vec<u8> {
    let ci = ContentInfo {
        content_type: OID_SIGNED_DATA,
        content: der::Any::from_der(sd.to_der().unwrap().as_slice()).unwrap(),
    };
    ci.to_der().unwrap()
}

fn assert_malformed(der: &[u8], needle: &str) {
    let err = parse_signed_data(der).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("malformed SignedData"),
        "应报 Malformed，实际：{msg}"
    );
    assert!(msg.contains(needle), "应含「{needle}」，实际：{msg}");
}

fn assert_unsupported(der: &[u8], needle: &str) {
    let err = parse_signed_data(der).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("unsupported SignedData"),
        "应报 Unsupported，实际：{msg}"
    );
    assert!(msg.contains(needle), "应含「{needle}」，实际：{msg}");
}

#[test]
fn parsed_signature_roundtrip_fields() {
    let (der, h) = cms_test_build();
    let p = parse_signed_data(&der).expect("valid sig must parse");

    assert_eq!(p.content_digest, cms_test_digest());
    assert_eq!(p.content_type, OID_SPC_PE_IMAGE_DATA);
    assert_eq!(p.certs.len(), 3, "证书集全三级（S0-4 约束）");

    // 签名者定位信息 = leaf 证书（serial + issuer DER 逐字节一致）
    let leaf = h.chain()[0].parsed().expect("leaf parse");
    assert_eq!(
        p.signer_serial,
        leaf.tbs_certificate.serial_number.as_bytes()
    );
    assert_eq!(
        p.signer_issuer_der,
        leaf.tbs_certificate.issuer.to_der().unwrap()
    );

    // 验签消息 = SET OF (0x31) 形态；签名可按 DER ECDSA 解析
    assert_eq!(p.signature_message[0], 0x31);
    assert!(
        <ecdsa::der::Signature<p256::NistP256>>::try_from(p.signature.as_slice()).is_ok(),
        "signature 字段须为 DER ECDSA 形态"
    );

    // messageDigest 属性 = SHA256(eContent Any 内容体（剥头，der Any.value() 语义）)；
    // 与 content_digest（SPC 内嵌入的摘要字段）是两个不同层面。osslsig.der 同口径
    // （S2-1 对比测试钉死），构造/解析两侧 der-crate 语义天然一致
    let spc_der = build_spc_indirect_data(&cms_test_digest()).unwrap();
    let any = der::Any::from_der(spc_der.as_slice()).unwrap();
    let expect_md: [u8; 32] = sha2::Sha256::digest(any.value()).into();
    assert_eq!(p.message_digest, expect_md);
    assert_ne!(p.message_digest, p.content_digest);

    assert_eq!(p.signing_time, Some(1_700_000_000));
    assert_eq!(p.program_name.as_deref(), Some("NemesisBot Code Signing"));
    assert_eq!(p.more_info.as_deref(), Some("https://nemesisbot.example"));
}

#[test]
fn parse_rejects_truncated_der() {
    let (der, _) = cms_test_build();
    assert_malformed(&der[..der.len() - 5], "ContentInfo DER");
}

#[test]
fn parse_rejects_garbage_bytes() {
    assert_malformed(&[0x02, 0x03, 0x04], "ContentInfo DER");
}

#[test]
fn parse_rejects_wrong_outer_oid() {
    let (der, _) = cms_test_build();
    let ci = ContentInfo::from_der(&der).unwrap();
    let wrong = ContentInfo {
        content_type: OID_SPC_PE_IMAGE_DATA,
        content: ci.content,
    };
    assert_malformed(&wrong.to_der().unwrap(), "非 signedData");
}

#[test]
fn parse_rejects_v3_version() {
    let (der, _) = cms_test_build();
    let mut sd = cms_parse(&der);
    sd.version = CmsVersion::V3;
    assert_unsupported(&reencode_sd(&sd), "version");
}

#[test]
fn parse_rejects_wrong_econtent_type() {
    let (der, _) = cms_test_build();
    let mut sd = cms_parse(&der);
    sd.encap_content_info.econtent_type = OID_SPC_PE_IMAGE_DATA;
    assert_malformed(&reencode_sd(&sd), "非 SPC_INDIRECT_DATA");
}

#[test]
fn parse_rejects_digest_attr_mismatch() {
    // 换 eContent（嵌不同 digest），messageDigest 属性仍是旧值 → 自洽闸拦截
    let (der, _) = cms_test_build();
    let mut sd = cms_parse(&der);
    let other: [u8; 32] = sha2::Sha256::digest(b"other content").into();
    let spc = build_spc_indirect_data(&other).unwrap();
    sd.encap_content_info.econtent = Some(der::Any::from_der(spc.as_slice()).unwrap());
    assert_malformed(&reencode_sd(&sd), "不一致");
}

#[test]
fn parse_rejects_missing_certificates() {
    let (der, _) = cms_test_build();
    let mut sd = cms_parse(&der);
    sd.certificates = None;
    assert_malformed(&reencode_sd(&sd), "证书集缺失");
}

#[test]
fn parse_rejects_empty_signer_infos() {
    let (der, _) = cms_test_build();
    let mut sd = cms_parse(&der);
    sd.signer_infos.0 = SetOfVec::default();
    assert_malformed(&reencode_sd(&sd), "signerInfos 数 = 0");
}

#[test]
fn parse_rejects_missing_signed_attrs() {
    let (der, _) = cms_test_build();
    let mut sd = cms_parse(&der);
    let mut si = sd.signer_infos.0.as_slice()[0].clone();
    si.signed_attrs = None;
    sd.signer_infos = SignerInfos::try_from(vec![si]).unwrap();
    assert_malformed(&reencode_sd(&sd), "signedAttrs 缺失");
}

#[test]
fn parse_rejects_non_der_ecdsa_signature() {
    let (der, _) = cms_test_build();
    let mut sd = cms_parse(&der);
    let mut si = sd.signer_infos.0.as_slice()[0].clone();
    si.signature = der::asn1::OctetString::new(vec![0xAA; 64]).unwrap();
    sd.signer_infos = SignerInfos::try_from(vec![si]).unwrap();
    assert_malformed(&reencode_sd(&sd), "signature 非 DER ECDSA");
}

// ---------------------------------------------------------------------------
// S2-3：v4 locator footer（ELF/raw 载体 roundtrip + crafted-footer 诚实失败）
// ---------------------------------------------------------------------------

#[test]
fn v4_footer_roundtrip_fields() {
    let f = build_footer_v4(FORMAT_TAG_ELF, 123_456, 123_456, 3_789);
    let p = parse_footer_v4(&f).expect("built v4 footer must parse");
    assert_eq!(p.format_ver, FORMAT_VER_V4);
    assert_eq!(p.sig_algo, SIG_ALGO_ECDSA_P256);
    assert_eq!(p.format_tag, FORMAT_TAG_ELF);
    assert_eq!(p.content_len, 123_456);
    assert_eq!(p.cms_off, 123_456);
    assert_eq!(p.cms_len, 3_789);
}

#[test]
fn v4_parse_rejects_v3_magic() {
    // v3 footer（magic NMBSIG\x03\x00）不是 v4 footer（v3 构造器已随 S5-3 删除，
    // 钉死用字节内联）
    let mut f = [0u8; 64];
    f[0..8].copy_from_slice(b"NMBSIG\x03\x00");
    let err = parse_footer_v4(&f).unwrap_err();
    assert!(format!("{err:#}").contains("magic"), "{err:#}");
}

#[test]
fn v4_parse_rejects_crc_mismatch() {
    let mut f = build_footer_v4(FORMAT_TAG_RAW, 100, 100, 200);
    // 改 content_len 一个字节（在 CRC 覆盖 [0..36) 内）但不重算 CRC
    f[V4_OFF_CONTENT_LEN] ^= 0x01;
    let err = parse_footer_v4(&f).unwrap_err();
    assert!(format!("{err:#}").contains("crc32"), "{err:#}");
}

/// 共用：真实 keygen 链 + 内容 SHA-256 → CMS SignedData → attach_v4
fn v4_attach(content: &[u8], tag: u8) -> Vec<u8> {
    let h = crate::keygen::generate().expect("keygen");
    let digest: [u8; 32] = sha2::Sha256::digest(content).into();
    let cms = build_signed_data(
        &digest,
        &h.leaf_sk,
        1_700_000_000,
        &h.chain(),
        Some("NemesisBot Code Signing"),
        Some("https://nemesisbot.example"),
    )
    .expect("build_signed_data");
    attach_v4(content, &cms, tag, content.len())
}

#[test]
fn v4_roundtrip_raw() {
    let content: Vec<u8> = (0..10_000u32).map(|i| (i % 251) as u8).collect();
    let file = v4_attach(&content, FORMAT_TAG_RAW);
    // 原内容前缀必须逐字节不动（Raw 载体 = 纯追加）
    assert_eq!(&file[..content.len()], &content[..]);

    let fo = find_footer_v4(&file, 0, &[]).expect("v4 footer found");
    assert_eq!(fo + FOOTER_LEN, file.len(), "footer 在文件末尾");
    let (cl, cms_der) = extract_v4(&file, fo).expect("extract");
    assert_eq!(cl, content.len());
    assert_eq!(
        cl + cms_der.len() + FOOTER_LEN,
        file.len(),
        "布局 = [content][cms][footer] 无缝隙"
    );

    let sig = parse_signed_data(cms_der).expect("CMS parse");
    let expect: [u8; 32] = s34_digest(&content);
    assert_eq!(
        sig.content_digest, expect,
        "content_digest = SHA256(原内容)"
    );
    assert_eq!(sig.program_name.as_deref(), Some("NemesisBot Code Signing"));
}

#[test]
fn v4_roundtrip_elf_overlay() {
    // ELF 形态建模：envelope 追加在 overlay（overlay_start = 原文件长度 L = content_len；
    // 真实 L 由 codec 计算，S3/S4 接线）
    let content: Vec<u8> = (0..5_000u32).map(|i| (i % 241) as u8).collect();
    let file = v4_attach(&content, FORMAT_TAG_ELF);
    let fo = find_footer_v4(&file, content.len(), &[]).expect("v4 footer found");
    let (cl, cms_der) = extract_v4(&file, fo).expect("extract");
    assert_eq!(cl, content.len());

    let sig = parse_signed_data(cms_der).expect("CMS parse");
    let expect: [u8; 32] = s34_digest(&content);
    assert_eq!(sig.content_digest, expect);
    assert_eq!(sig.certs.len(), 3, "证书集全三级随 CMS 走");
}

#[test]
fn v4_find_respects_excludes_and_overlay() {
    let content = vec![7u8; 300];
    let file = v4_attach(&content, FORMAT_TAG_RAW);
    let fo = find_footer_v4(&file, 0, &[]).expect("footer found");
    // 排除区盖住 footer → 找不到；不覆盖时照常命中
    assert_eq!(find_footer_v4(&file, 0, &[(fo, file.len())]), None);
    assert_eq!(find_footer_v4(&file, 0, &[(0, fo)]), Some(fo));
    // overlay 下界高于 footer 位置 → 找不到
    assert_eq!(find_footer_v4(&file, fo + 1, &[]), None);
    // 文件短于 overlay_start + FOOTER_LEN → None
    assert_eq!(find_footer_v4(&[0u8; 10], 0, &[]), None);
}

#[test]
fn v4_extract_rejects_crafted_cms_range() {
    let content = vec![0u8; 1_000];
    let file = v4_attach(&content, FORMAT_TAG_RAW);
    let fo = find_footer_v4(&file, 0, &[]).unwrap();
    // 抬高 cms_len 到 u32 上限：footer 本身合法（CRC 重建过），但 CMS 区间越过 footer
    // → extract 诚实报错而非 panic / 误提取
    let bad = build_footer_v4(FORMAT_TAG_RAW, content.len(), fo, u32::MAX as usize);
    let mut crafted = file[..fo].to_vec();
    crafted.extend_from_slice(&bad);
    let err = extract_v4(&crafted, fo).unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("crafted footer"), "{msg}");
    assert!(msg.contains("越过"), "{msg}");
}

#[test]
fn v4_extract_rejects_zero_cms_len() {
    let content = vec![0u8; 100];
    let file = v4_attach(&content, FORMAT_TAG_RAW);
    let fo = find_footer_v4(&file, 0, &[]).unwrap();
    let bad = build_footer_v4(FORMAT_TAG_RAW, content.len(), fo, 0);
    let mut crafted = file[..fo].to_vec();
    crafted.extend_from_slice(&bad);
    let err = extract_v4(&crafted, fo).unwrap_err();
    assert!(format!("{err:#}").contains("cms_len = 0"), "{err:#}");
}

#[test]
fn v4_extract_rejects_content_overlapping_cms() {
    let content = vec![0u8; 100];
    let file = v4_attach(&content, FORMAT_TAG_RAW);
    let fo = find_footer_v4(&file, 0, &[]).unwrap();
    // content_len 抬到 footer 位置（> 真实 cms_off）→ 内容压进 CMS 区
    let bad = build_footer_v4(FORMAT_TAG_RAW, fo, content.len(), 200);
    let mut crafted = file[..fo].to_vec();
    crafted.extend_from_slice(&bad);
    let err = extract_v4(&crafted, fo).unwrap_err();
    assert!(format!("{err:#}").contains("content_len"), "{err:#}");
}

#[test]
fn v4_extract_rejects_footer_offset_out_of_bounds() {
    let content = vec![0u8; 100];
    let file = v4_attach(&content, FORMAT_TAG_RAW);
    // footer 偏移越界（声称的 footer 不在文件内）
    let err = extract_v4(&file, file.len() + 1).unwrap_err();
    assert!(format!("{err:#}").contains("越界"), "{err:#}");
}

// ---------------------------------------------------------------------------
// S3-4：ELF/raw 载体接 v4 footer（codec 接线：sign_carrier_v4 / extract_carrier_v4
// 签名→验证闭环；ELF64 LE / ELF32 BE 双轴 + overlay 域外性 + 载体契约诚实失败）
// ---------------------------------------------------------------------------

use crate::codec::ExecutableCodec;

/// S3-4 夹具：最小 ELF（头 + 1 个 PT_LOAD 覆盖结构区）→ L = 结构区末尾，
/// 追加 pattern overlay（overlay 不入保护域）。返回 (elf, L)。
fn s34_min_elf(is64: bool, le: bool) -> (Vec<u8>, usize) {
    fn put16(b: &mut [u8], off: usize, v: u16, le: bool) {
        let a = if le { v.to_le_bytes() } else { v.to_be_bytes() };
        b[off..off + 2].copy_from_slice(&a);
    }
    fn put32(b: &mut [u8], off: usize, v: u32, le: bool) {
        let a = if le { v.to_le_bytes() } else { v.to_be_bytes() };
        b[off..off + 4].copy_from_slice(&a);
    }
    fn put64(b: &mut [u8], off: usize, v: u64, le: bool) {
        let a = if le { v.to_le_bytes() } else { v.to_be_bytes() };
        b[off..off + 8].copy_from_slice(&a);
    }
    let hdr: usize = if is64 { 64 } else { 52 };
    let phe: usize = if is64 { 56 } else { 32 };
    let l = hdr + phe;
    let mut b = vec![0u8; l];
    b[0..4].copy_from_slice(b"\x7fELF");
    b[4] = if is64 { 2 } else { 1 };
    b[5] = if le { 1 } else { 2 };
    if is64 {
        put64(&mut b, 32, hdr as u64, le); // e_phoff
        put64(&mut b, 40, 0, le); // e_shoff = 0（无 section 表）
        put16(&mut b, 54, phe as u16, le);
        put16(&mut b, 56, 1, le); // e_phnum
        put16(&mut b, 58, 64, le);
        put16(&mut b, 60, 0, le);
        put32(&mut b, hdr, 1, le); // p_type = PT_LOAD
        put64(&mut b, hdr + 8, 0, le); // p_offset
        put64(&mut b, hdr + 32, l as u64, le); // p_filesz = 结构区全长
    } else {
        put32(&mut b, 28, hdr as u32, le); // e_phoff
        put32(&mut b, 32, 0, le); // e_shoff
        put16(&mut b, 42, phe as u16, le);
        put16(&mut b, 44, 1, le);
        put16(&mut b, 46, 40, le);
        put16(&mut b, 48, 0, le);
        put32(&mut b, hdr, 1, le); // p_type = PT_LOAD
        put32(&mut b, hdr + 4, 0, le); // p_offset
        put32(&mut b, hdr + 16, l as u32, le); // p_filesz
    }
    // pattern overlay（签名后追加态的建模；不入保护域）
    b.extend((0..80u32).map(|i| (i % 7 + 0xC0) as u8));
    (b, l)
}

/// S3-4 夹具：内容 digest → 真 CMS（同 v4_attach，但 digest 由调用方定）。
fn s34_cms_for(digest: &[u8; 32]) -> Vec<u8> {
    let h = crate::keygen::generate().expect("keygen");
    build_signed_data(
        digest,
        &h.leaf_sk,
        1_800_000_000,
        &h.chain(),
        Some("NemesisBot Code Signing"),
        Some("https://nemesisbot.example"),
    )
    .expect("build_signed_data")
}

/// s34_cms_for 的 digest 参数便捷构造（`sha2 digest → [u8; 32]` 显式落型防 Into 歧义）。
fn s34_digest(b: &[u8]) -> [u8; 32] {
    sha2::Sha256::digest(b).into()
}

#[test]
fn carrier_sign_extract_roundtrip_raw() {
    let content: Vec<u8> = (0..5_000u32).map(|i| (i % 253) as u8).collect();
    let cms = s34_cms_for(&s34_digest(&content));

    let file = sign_carrier_v4(&content, &cms).expect("sign raw carrier");
    // 原内容前缀逐字节不动 + 布局无缝隙
    assert_eq!(&file[..content.len()], &content[..]);
    let v = extract_carrier_v4(&file).expect("extract raw carrier");
    assert_eq!(v.content_len, content.len(), "Raw 保护域 = 全文件");
    assert_eq!(v.cms_der, cms, "CMS 原样入载原样出");
    assert_eq!(v.content_digest, s34_digest(&content));
    assert!(v.cms_digest_matches, "结构层闭环：重算 digest == CMS 内嵌");
    assert_eq!(
        v.footer_offset + FOOTER_LEN,
        file.len(),
        "footer 在文件末尾"
    );
}

#[test]
fn carrier_sign_extract_roundtrip_elf64_le_overlay() {
    let (elf, l) = s34_min_elf(true, true);
    assert_eq!(
        crate::codec::ElfCodec.compute_l(&elf).unwrap(),
        Some(l),
        "夹具自检：L = 结构区末尾（overlay 前）"
    );
    let cms = s34_cms_for(&s34_digest(&elf[..l]));

    let file = sign_carrier_v4(&elf, &cms).expect("sign elf carrier");
    let v = extract_carrier_v4(&file).expect("extract elf carrier");
    assert_eq!(v.content_len, l, "ELF 保护域 = [0, L)，overlay 不入域");
    assert_eq!(v.cms_der, cms);
    assert_eq!(v.content_digest, s34_digest(&elf[..l]));
    assert!(v.cms_digest_matches);
    // overlay 原样保留在 [L, cms_off)
    assert_eq!(&file[l..l + 80], &elf[l..]);

    // 域外性负控：篡改 overlay 字节 → 载体完整性判定不受影响
    let mut ov_tampered = file.clone();
    let ov_off = l + 7;
    ov_tampered[ov_off] ^= 0xFF;
    assert!(
        extract_carrier_v4(&ov_tampered)
            .expect("overlay 篡改仍可提取")
            .cms_digest_matches,
        "overlay 不在保护域：digest 不受 overlay 篡改影响"
    );
}

#[test]
fn carrier_roundtrip_elf32_be() {
    // 双轴第二轴：ELF32 大端（字段读取按 EI_DATA，摘要对原始字节计算、字节序无关）
    let (elf, l) = s34_min_elf(false, false);
    assert_eq!(crate::codec::ElfCodec.compute_l(&elf).unwrap(), Some(l));
    let cms = s34_cms_for(&s34_digest(&elf[..l]));
    let file = sign_carrier_v4(&elf, &cms).expect("sign elf32 be");
    let v = extract_carrier_v4(&file).expect("extract elf32 be");
    assert_eq!(v.content_len, l);
    assert!(v.cms_digest_matches);
}

#[test]
fn carrier_detects_content_tampering() {
    // 负控：保护域内 1 字节翻转 → cms_digest_matches = false（诚实报告非 Err）
    let (elf, l) = s34_min_elf(true, true);
    let cms = s34_cms_for(&s34_digest(&elf[..l]));
    let mut file = sign_carrier_v4(&elf, &cms).expect("sign");
    file[l - 1] ^= 0x01; // 结构区末字节
    let v = extract_carrier_v4(&file).expect("篡改不阻断提取");
    assert!(!v.cms_digest_matches, "保护域内容被篡改必须被载体层检出");
}

#[test]
fn carrier_honest_failures() {
    // PE 魔数输入 → 诚实拒绝（PE 载体走 Certificate Table）
    let cms = s34_cms_for(&[0u8; 32]);
    let mut pe_like = vec![0u8; 300];
    pe_like[0] = b'M';
    pe_like[1] = b'Z';
    let err = sign_carrier_v4(&pe_like, &cms).unwrap_err();
    assert!(format!("{err:#}").contains("Certificate Table"), "{err:#}");

    // 未签名 ELF（无 footer）→ 诚实报错
    let (elf, _) = s34_min_elf(true, true);
    let err = extract_carrier_v4(&elf).unwrap_err();
    assert!(format!("{err:#}").contains("无 v4 footer"), "{err:#}");

    // 载体契约违约：footer content_len ≠ ELF L → 拒绝（保护域声明与结构不符）
    let (elf2, l2) = s34_min_elf(true, true);
    let cms2 = s34_cms_for(&s34_digest(&elf2[..l2]));
    let file = sign_carrier_v4(&elf2, &cms2).expect("sign");
    let fo = find_footer_v4(&file, l2, &[]).expect("footer");
    let bad = build_footer_v4(FORMAT_TAG_ELF, l2 + 1, elf2.len(), cms2.len());
    let mut crafted = file[..fo].to_vec();
    crafted.extend_from_slice(&bad);
    let err = extract_carrier_v4(&crafted).unwrap_err();
    assert!(format!("{err:#}").contains("契约违约"), "{err:#}");

    // content_len 抬进 CMS 区（extract_v4 越界防钳经载体入口兜底）
    let bad2 = build_footer_v4(FORMAT_TAG_RAW, 5000, 150, 40);
    let mut garbage = vec![0x41u8; 200];
    garbage.extend_from_slice(&bad2);
    let err = extract_carrier_v4(&garbage).unwrap_err();
    assert!(format!("{err:#}").contains("content_len"), "{err:#}");
}
