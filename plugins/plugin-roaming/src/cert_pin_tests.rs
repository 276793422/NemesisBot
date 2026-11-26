//! cert_pin 单测（2026-11-26 打洞通路激活计划 §4.5；复查①混合验证器 +
//! 复查二轮① SNI 判别）：指纹归一 + 验证器判定面（真实握手路径由 Phase 0
//! 沙盒 V1/V2 活体验证——本文件只钉纯函数与判定逻辑，不起网络）。

use std::sync::Arc;

use rustls::client::danger::ServerCertVerifier;
use rustls::crypto::ring::default_provider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use sha2::Digest;

use crate::cert_pin::{RelayCertPinVerifier, normalize_fingerprint};

/// 自签一张测试证书（rcgen 同款——纯内存，不落盘）。
fn self_signed_cert_der() -> CertificateDer<'static> {
    let key = rcgen::generate_simple_self_signed(vec!["nb-relay-test".to_string()])
        .expect("self-signed cert generation");
    key.cert.der().to_vec().into()
}

/// 归一化测试值。
#[test]
fn normalize_accepts_and_strips_noise() {
    // 64 hex 字符混入大小写/空白/冒号 → 归一后解码成功。
    let noisy = "AB:cd ef01".to_string() + &"0123456789abcdef".repeat(3) + "01234567";
    let fp = normalize_fingerprint(&noisy).expect("64 hex chars after strip");
    assert_eq!(fp.len(), 32); // [u8;32] 常量长度——此断言只为消费值防 dead_code
    // 等价输入（大写↔小写、空白形态不同）产出相同字节。
    let fp2 = normalize_fingerprint(&noisy.to_uppercase().replace(':', " ")).expect("same bytes");
    assert_eq!(fp, fp2);
}

#[test]
fn normalize_rejects_empty_and_wrong_length() {
    assert!(normalize_fingerprint("").is_err(), "空 = 未配置，必须 Err");
    assert!(normalize_fingerprint("   \t\n").is_err(), "纯空白同空");
    assert!(normalize_fingerprint("abcd").is_err(), "非 64 字符");
    // 64 字符但含非 hex 字符。
    let bad = "g".repeat(64);
    assert!(normalize_fingerprint(&bad).is_err(), "非 hex 字符");
}

/// pin 命中臂（复查①混合语义）：握手证书 hash 恰好等于配置指纹 → 直接
/// 接受（本中继自签证书；不经 webpki——自签不被公网根信任；命中臂不看
/// SNI——指纹命中即短路）。
#[test]
fn verifier_pin_hit_accepts() {
    let cert = self_signed_cert_der();
    let own_fp: [u8; 32] = sha2::Sha256::digest(cert.as_ref()).into();
    let provider = Arc::new(default_provider());
    let relay_host = ServerName::try_from("localhost".to_string()).expect("dns name");

    let verifier =
        RelayCertPinVerifier::new(own_fp, relay_host, provider.clone()).expect("hybrid verifier");
    let ok = verifier.verify_server_cert(
        &cert,
        &[],
        &ServerName::try_from("localhost".to_string()).expect("dns name"),
        &[],
        UnixTime::now(),
    );
    assert!(ok.is_ok(), "指纹匹配必须通过（pin 臂短路，不经 webpki）");
}

/// pin 未命中 + 非中继 SNI（复查①混合语义 + 复查二轮① SNI 判别）：自签
/// 证书不被 pin 命中且 SNI 不是中继身份 → 落回标准 webpki 验证 → 自签不被
/// 公网根信任 → loud Err（UnknownIssuer 族）。这是证书轮换后旧指纹客户端的
/// 可观测降级路径；错误不再是 pin 比对文案（旧实现的「fingerprint/指纹」
/// 断言随混合语义一并退役）。
#[test]
fn verifier_pin_miss_falls_back_to_webpki() {
    let cert = self_signed_cert_der();
    let mut wrong_fp: [u8; 32] = sha2::Sha256::digest(cert.as_ref()).into();
    wrong_fp[0] ^= 0xff;
    let provider = Arc::new(default_provider());
    // SNI（localhost）≠ 中继身份（relay.example.com）→ 走 fallback 臂。
    let relay_host = ServerName::try_from("relay.example.com".to_string()).expect("dns name");

    let verifier =
        RelayCertPinVerifier::new(wrong_fp, relay_host, provider).expect("hybrid verifier");
    let bad = verifier.verify_server_cert(
        &cert,
        &[],
        &ServerName::try_from("localhost".to_string()).expect("dns name"),
        &[],
        UnixTime::now(),
    );
    let msg = format!("{bad:?}");
    assert!(bad.is_err(), "pin 未命中 + 自签不被公网根信任 = loud 拒绝");
    assert!(
        msg.contains("UnknownIssuer") || msg.contains("unknown") || msg.contains("Unknown"),
        "错误应来自 webpki fallback 臂（UnknownIssuer 族），实际: {msg}"
    );
}

/// pin 未命中 + 中继 SNI（复查二轮①核心臂）：**不落 webpki，loud 拒绝**——
/// 指纹是中继身份的唯一信任锚，持中继域名公网合法证书的 MITM 不得在此绕过
/// pin（错误必须是 pin 比对文案而非 UnknownIssuer）。DNS 名与 IP 字面量两
/// 形态都钉（生产 relay_url 是 IP 形态——IpAddress 比较臂同验）。
#[test]
fn verifier_pin_miss_on_relay_host_is_loud() {
    let cert = self_signed_cert_der();
    let mut wrong_fp: [u8; 32] = sha2::Sha256::digest(cert.as_ref()).into();
    wrong_fp[0] ^= 0xff;
    let provider = Arc::new(default_provider());

    // DNS 形态中继身份。
    let relay_host = ServerName::try_from("localhost".to_string()).expect("dns name");
    let verifier =
        RelayCertPinVerifier::new(wrong_fp, relay_host, provider).expect("hybrid verifier");
    let bad = verifier.verify_server_cert(
        &cert,
        &[],
        &ServerName::try_from("localhost".to_string()).expect("dns name"),
        &[],
        UnixTime::now(),
    );
    let msg = format!("{bad:?}");
    assert!(bad.is_err(), "中继名上指纹未命中必须 loud 拒绝");
    assert!(
        msg.contains("指纹"),
        "错误必须来自 pin 判别臂（SNI 判别先于 fallback），实际: {msg}"
    );
    assert!(
        !msg.contains("UnknownIssuer"),
        "中继名上的未命中绝不落 webpki——公网证书不得绕过 pin，实际: {msg}"
    );

    // IP 字面量形态中继身份（生产形态——IpAddress 按值比较臂）。
    let relay_host_ip = ServerName::try_from("42.193.96.45".to_string()).expect("ip name");
    let verifier2 =
        RelayCertPinVerifier::new(wrong_fp, relay_host_ip, Arc::new(default_provider()))
            .expect("hybrid verifier");
    let bad2 = verifier2.verify_server_cert(
        &cert,
        &[],
        &ServerName::try_from("42.193.96.45".to_string()).expect("ip name"),
        &[],
        UnixTime::now(),
    );
    assert!(bad2.is_err(), "IP 形态中继名未命中同样 loud");
    assert!(
        format!("{bad2:?}").contains("指纹"),
        "IP 臂错误同样必须来自 pin 判别臂"
    );
}

/// 混合臂构造：`new` 走 webpki-roots 公网根库 fallback——构造失败即 loud
/// （provider 缺签名验签算法表等装配缺陷）；本测试只钉构造路径可用。
#[test]
fn hybrid_verifier_constructs_with_ring_provider() {
    let provider = Arc::new(default_provider());
    let relay_host = ServerName::try_from("nb-relay-test".to_string()).expect("dns name");
    let verifier = RelayCertPinVerifier::new([0u8; 32], relay_host, provider);
    assert!(verifier.is_ok(), "ring provider 构造混合验证器必须成功");
}
