//! 自建中继证书指纹 pin（QUIC relay 臂的 TLS 信任锚，方案 A）。
//!
//! 信任模型（2026-11-26 打洞通路激活计划 §2.2，用户质询后定案）：客户端
//! config 的 `relay_cert_fp`（SHA-256 hex，与 cluster token 同通道人工分发）
//! 是唯一信任锚。TLS 握手服务器必然出示证书（协议必经步骤——证书天然公开
//! 送达，无需任何分发动作），验证器现场对**完整证书 DER** 算 SHA-256 与配置
//! 比对：匹配才过，不匹配 loud 拒绝。零新增端点、零文件分发。
//!
//! 选完整证书 DER 而非 SPKI 的理由：中继证书由 rcgen 整体重生成（每次全新
//! 密钥对），SPKI pin 的「重签发存活」特性在此无意义；全证书 hash 无需
//! ASN.1 解析依赖，服务器侧计算同样简单（对证书 DER 字节直接 hash，启动
//! 日志透出）。两者安全属性对本威胁模型（检测任何证书替换）完全等价。
//!
//! 范围（复查①修正——原「验证器只会见到中继证书」断言不成立）：iroh 的
//! `ca_tls_config` custom verifier 是**全部外部 TLS**（iroh relays / pkarr /
//! DoH——iroh-relay tls.rs 文档原话）的唯一验证器；本端点 presets::N0 基座
//! 带 pkarr n0 发布/解析（HTTPS 到 iroh.link），这些流量同样经此验证器。
//! iroh 原生节点间连接依赖自家密码学认证、不经此路（不受影响）。
//!
//! 因此验证器是**混合语义 + SNI 判别**（复查二轮①）：握手证书指纹命中配置
//! = 本中继自签证书，直接接受（pin 只替代「信任根判定」，不替代「签名有效
//! 性」——签名算法校验始终委托 provider 原生实现）；未命中时分两臂——
//! SNI == 配置的中继 host → **loud 拒绝不落 webpki**（指纹是中继身份的
//! 唯一信任锚，公网合法证书不得在中继身份上绕过 pin）；其余 SNI（pkarr/
//! DoH 等非中继外部 TLS）→ 落回标准 webpki 验证（webpki-roots 公网根库，
//! 与 iroh 默认 embedded 臂同源构造）——pkarr/DoH 照常工作，中继证书轮换
//! 后自签不被公网根信任 → loud 失败（降级 qad_v4:false 可观测）。
//!
//! 生命周期：指纹绑定证书文件。证书重新生成**仅在删除 relay 目录 / 证书
//! 文件后发生**（ensure_self_signed_cert 对已存在 PEM 无条件复用——改
//! external_host 重启不会重生成、指纹不变，复查⑥修正）→ 指纹变 → 所有
//! 客户端 `relay_cert_fp` 必须同步更新，否则中继握手降级到标准验证（自签
//! 不被公网根信任）→ QUIC 臂 loud 失败（HTTP transport 不受影响）。

use std::fmt;
use std::io;
use std::sync::Arc;

use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{
    CryptoProvider, WebPkiSupportedAlgorithms, verify_tls12_signature, verify_tls13_signature,
};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, Error, RootCertStore, SignatureScheme};
use sha2::{Digest, Sha256};

/// 归一证书指纹：去空白/冒号、lowercase、hex 解码为 32 字节（SHA-256 摘要）。
///
/// 空/长度错/非 hex 一律 `Err`（loud——配置不完整绝不静默放行）。
pub fn normalize_fingerprint(raw: &str) -> Result<[u8; 32], String> {
    let cleaned: String = raw
        .chars()
        .filter(|c| !matches!(c, ' ' | '\t' | '\r' | '\n' | ':'))
        .collect();
    if cleaned.is_empty() {
        return Err("未配置（QUIC 臂必须配证书指纹 pin——不给无校验 TLS 留口子）".to_string());
    }
    if cleaned.len() != 64 {
        return Err(format!(
            "须为 64 位 hex（SHA-256），去空白后得到 {} 字符",
            cleaned.len()
        ));
    }
    let bytes = hex::decode(cleaned.as_bytes()).map_err(|e| format!("hex 解码失败: {e}"))?;
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

/// 指纹 pin 混合验证器（复查①；复查二轮①加 SNI 判别）：`verify_server_cert`
/// 先对端点实体证书 DER 现场算 SHA-256 与配置指纹比对——命中 = 本中继自签
/// 证书，接受；未命中按 SNI 分两臂：`relay_host`（构造时传入的中继身份）
/// → loud 拒绝（指纹是中继身份唯一信任锚）；其余 → 落回 `fallback`（标准
/// webpki 验证，公网根库）。签名算法校验始终委托 rustls provider 原生实现
/// （pin 只替代「信任根判定」，不替代「签名有效性」）。
pub struct RelayCertPinVerifier {
    fp: [u8; 32],
    /// 中继身份（SNI 判别锚；自 relay_url host 提取，DNS 名或 IP 字面量）。
    /// 指纹未命中且 SNI 与此相等 = 中继连接上的指纹漂移 → loud 拒绝。
    relay_host: ServerName<'static>,
    algs: WebPkiSupportedAlgorithms,
    /// pin 未命中且**非中继 SNI**时的标准验证器（webpki-roots 公网根库——
    /// iroh 默认 embedded 臂同款构造，iroh-relay tls.rs:177-180）。非中继
    /// 外部 TLS（pkarr/DoH）经此正常通过；中继自身连接不经此（SNI 判别臂
    /// 拦下）。
    fallback: Arc<WebPkiServerVerifier>,
}

impl RelayCertPinVerifier {
    /// 从 provider 构造（`fp` 为归一后的 32 字节摘要；`relay_host` 为中继
    /// 身份；provider 同时供给签名算法表与 fallback 验证器）。
    pub fn new(
        fp: [u8; 32],
        relay_host: ServerName<'static>,
        provider: Arc<CryptoProvider>,
    ) -> io::Result<Self> {
        let roots = Arc::new(RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        });
        let fallback = WebPkiServerVerifier::builder_with_provider(roots, provider.clone())
            .build()
            .map_err(io::Error::other)?;
        let algs = provider.signature_verification_algorithms;
        Ok(Self {
            fp,
            relay_host,
            algs,
            fallback,
        })
    }
}

impl fmt::Debug for RelayCertPinVerifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 只打指纹，不打算法表（表很大且无诊断价值）。
        write!(
            f,
            "RelayCertPinVerifier(fp={}, hybrid)",
            hex::encode(self.fp)
        )
    }
}

impl ServerCertVerifier for RelayCertPinVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        let digest: [u8; 32] = Sha256::digest(end_entity.as_ref()).into();
        if digest == self.fp {
            return Ok(ServerCertVerified::assertion());
        }
        // 复查二轮①：SNI 判别——中继自身连接上的指纹未命中 = loud 拒绝，
        // **绝不落 webpki**（持中继域名公网合法证书的 MITM 不得在中继身份
        // 上绕过指纹锚；`to_owned()` 归一生命周期后比较——DNS 名 ascii 大
        // 小写不敏感、IP 按值，pki-types DnsNameInner::eq 契约）。
        if server_name.to_owned() == self.relay_host {
            return Err(Error::General(
                "relay 证书指纹不匹配（relay_cert_fp）——中继身份必须指纹锚定，拒绝公网根回退"
                    .into(),
            ));
        }
        // pin 未命中 + 非中继 SNI → 标准 webpki 验证（复查①）：pkarr/DoH
        // 用公网根库正常通过。
        self.fallback
            .verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls12_signature(message, cert, dss, &self.algs)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls13_signature(message, cert, dss, &self.algs)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algs.supported_schemes()
    }
}
