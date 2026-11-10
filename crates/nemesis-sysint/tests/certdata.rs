//! 证书载荷解析 + 指纹测试（跨平台可跑）。

use nemesis_sysint::certdata::{
    cert_der_from_bytes, cert_der_from_file, extract_pem_block, fingerprint, is_der_certificate,
};

/// 任意确定性字节（指纹测试载荷，无需真证书）。
fn fake_der(tag: u8) -> Vec<u8> {
    let mut v = vec![0x30, 0x82, 0x01, 0x00, tag];
    v.extend(0u8..=250u8);
    v
}

#[test]
fn fingerprint_is_deterministic_hex_sha256() {
    let a = fake_der(1);
    let fa = fingerprint(&a);
    assert_eq!(fa.len(), 64, "sha256 hex 长度");
    assert!(
        fa.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    );
    assert_eq!(fingerprint(&a), fa, "确定性");
    assert_ne!(fingerprint(&fake_der(2)), fa, "不同载荷不同指纹");
    // 空输入也稳定（指纹函数不做格式判断）。
    assert_eq!(fingerprint(&[]).len(), 64);
}

#[test]
fn is_der_certificate_heuristic() {
    assert!(is_der_certificate(&[0x30, 0x82, 0x01, 0x00, 0x00]));
    assert!(!is_der_certificate(&[0x30, 0x82])); // 过短
    assert!(!is_der_certificate(b"-----BEGIN")); // PEM 文本不是裸 DER
    assert!(!is_der_certificate(&[]));
}

#[test]
fn extract_pem_block_roundtrip_and_edge_cases() {
    let der = fake_der(7);
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&der);
    // 64 列折行（真实 PEM 形态）。
    let mut wrapped = String::new();
    for chunk in b64.as_bytes().chunks(64) {
        wrapped.push_str(std::str::from_utf8(chunk).unwrap_or_default());
        wrapped.push('\n');
    }
    let pem = format!("-----BEGIN CERTIFICATE-----\n{wrapped}-----END CERTIFICATE-----\n");
    assert_eq!(extract_pem_block(&pem), Some(der.clone()), "折行 PEM 提取");
    // 前后噪声（链文件多证书形态：取第一块）。
    let noisy = format!("noise\n{pem}\nmore noise");
    assert_eq!(extract_pem_block(&noisy), Some(der.clone()));
    // 畸形：有 BEGIN 无 END。
    let broken = format!("-----BEGIN CERTIFICATE-----\n{b64}\n");
    assert_eq!(extract_pem_block(&broken), None);
    // base64 体不是证书形状（SEQUENCE 头缺失）。
    let not_der = format!(
        "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n",
        base64::engine::general_purpose::STANDARD.encode(b"not a cert at all.........")
    );
    assert_eq!(extract_pem_block(&not_der), None);
}

#[test]
fn cert_der_from_bytes_matrix() {
    let der = fake_der(3);
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&der);
    let pem = format!("-----BEGIN CERTIFICATE-----\n{b64}\n-----END CERTIFICATE-----\n");
    // PEM → DER。
    assert_eq!(cert_der_from_bytes(pem.as_bytes()).unwrap(), der);
    // 裸 DER 原样返回。
    assert_eq!(cert_der_from_bytes(&der).unwrap(), der);
    // 垃圾 → 明确报错（不静默截取）。
    let garbage = b"hello world, definitely not a certificate";
    assert!(cert_der_from_bytes(garbage).is_err());
}

#[test]
fn cert_der_from_file_reads_pem() -> anyhow::Result<()> {
    let tmp = tempfile::tempdir()?;
    let der = fake_der(9);
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&der);
    let pem = format!("-----BEGIN CERTIFICATE-----\n{b64}\n-----END CERTIFICATE-----\n");
    let p = tmp.path().join("root.pem");
    std::fs::write(&p, &pem)?;
    assert_eq!(cert_der_from_file(&p)?, der);
    // 缺席文件报错带路径上下文。
    assert!(cert_der_from_file(&tmp.path().join("missing.pem")).is_err());
    Ok(())
}
