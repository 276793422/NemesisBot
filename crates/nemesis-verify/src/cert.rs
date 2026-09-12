//! 证书与证书链（**v4：X.509 v3 解析 + 链验证**，Authenticode 对齐）。
//!
//! # 信任链（D4）
//! ```text
//! 根私钥（离线，30y 自签）→ 根证书（默认不安装，SHA-256 指纹作编译期锚）
//!   └签→ 发行锚 CA（10y，opt-in 可安装）→ leaf 代码签名证书（3y，EKU codeSigning）
//!           └签→ exe（CMS SignedData 证书集带全链，P2）
//! ```
//!
//! # 链终止条件（S4-2 根锚口径）
//! **SHA-256(根证书 DER) == 编译期根锚指纹**。S0-4/S0-5 实证：嵌入根 ≠ 信任根
//! （Windows 链引擎只认 Root store 中的自签根），全链嵌入只为链引擎提供终止点，
//! 信任由锚指纹钉死——自方 verifier 恒锚定编译期根，与目标机安装状态无关。
//!
//! # 逐级验证内容（`verify_chain`）
//! - 有效期 `not_before <= now <= not_after`
//! - AKI/SKI 匹配（两者在场时必须一致；X.509 扩展可省略，省略时跳过匹配）
//! - 逐级 ECDSA-P256/SHA-256 签名（只认自家套件 `1.2.840.10045.4.3.2`）
//! - 根必须自签（AKI==SKI 或 AKI 缺省 + 自签名验过）且 SHA-256(DER) 匹配锚
//! - leaf 必带 codeSigning EKU `1.3.6.1.5.5.7.3.3`（D4）
//!
//! # 签名验证字节口径
//! 对证书 DER 中 **TBS 原始字节段**（byte-exact 切片，不重编码——防非规范 DER
//! 重编码破坏对第三方签发证书的签名覆盖面）。
//!
//! # 依赖
//! `x509-cert` 0.2（re-export `der`/`spki`）声明式类型 + `p256` 原语；零字节级手撸
//! DER（D8），零 C ABI。

use der::asn1::{GeneralizedTime, ObjectIdentifier, OctetString, Utf8StringRef};
use der::{Any, Decode, Encode, Header, Reader, SliceReader};
use p256::ecdsa::{Signature, SigningKey, VerifyingKey};
use p256::elliptic_curve::sec1::ToEncodedPoint;
use sha2::Digest;
use x509_cert::attr::AttributeTypeAndValue;
use x509_cert::certificate::{Certificate as X509Certificate, TbsCertificate, Version};
use x509_cert::ext::Extension;
use x509_cert::ext::pkix::{
    AuthorityKeyIdentifier, BasicConstraints, ExtendedKeyUsage, KeyUsage, KeyUsages,
    SubjectKeyIdentifier,
};
use x509_cert::name::{Name, RdnSequence, RelativeDistinguishedName};
use x509_cert::serial_number::SerialNumber;
use x509_cert::spki::AlgorithmIdentifierOwned;
use x509_cert::spki::SubjectPublicKeyInfo;
use x509_cert::time::{Time, Validity};

use crate::crypto;

/// ecdsa-with-SHA256（唯一接受的证书签名套件，D2）。
pub const OID_ECDSA_WITH_SHA256: ObjectIdentifier =
    ObjectIdentifier::new_unwrap("1.2.840.10045.4.3.2");
/// id-ecPublicKey。
pub const OID_EC_PUBLIC_KEY: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.2.1");
/// secp256r1 / prime256v1（P-256 命名曲线参数）。
pub const OID_P256: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.3.1.7");
/// id-kp-codeSigning（D4 leaf 必带）。
pub const OID_EKU_CODE_SIGNING: ObjectIdentifier =
    ObjectIdentifier::new_unwrap("1.3.6.1.5.5.7.3.3");
/// id-at-commonName（2.5.4.3）。
pub const OID_AT_CN: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.4.3");
/// id-at-organizationName（2.5.4.10）。
pub const OID_AT_O: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.4.10");
/// id-ce-basicConstraints（2.5.29.19）。
pub const OID_CE_BASIC_CONSTRAINTS: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.29.19");
/// id-ce-keyUsage（2.5.29.15）。
pub const OID_CE_KEY_USAGE: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.29.15");
/// id-ce-subjectKeyIdentifier（2.5.29.14）。
pub const OID_CE_SUBJECT_KEY_IDENTIFIER: ObjectIdentifier =
    ObjectIdentifier::new_unwrap("2.5.29.14");
/// id-ce-authorityKeyIdentifier（2.5.29.35）。
pub const OID_CE_AUTHORITY_KEY_IDENTIFIER: ObjectIdentifier =
    ObjectIdentifier::new_unwrap("2.5.29.35");
/// id-ce-extKeyUsage（2.5.29.37）。
pub const OID_CE_EXT_KEY_USAGE: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.29.37");

/// X.509 证书（DER 保存 + 访问器）。原始 DER 是唯一真相源（访问器每次独立解析）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Certificate {
    /// 完整 DER 编码（指纹计算 / 回传 / 重解析的唯一真相源）。
    der: Vec<u8>,
}

/// 证书链验证错误（v4）。verify 层（S4-1）把它们映射进 `VerifyOutcome`。
#[derive(Debug, PartialEq)]
pub enum ChainError {
    Empty,
    /// 结构坏：DER 解析失败 / TBS 跨界 / 扩展坏。
    Malformed(String),
    /// 签名算法不在接受集（只认 ecdsa-with-SHA256）。
    UnsupportedAlgorithm,
    /// 公钥不是 P-256 或形状非法。
    InvalidKey,
    /// 链断裂：cert[i] 的 AKI 与 certs[i+1] 的 SKI 不匹配。
    BrokenChain,
    /// 未到受信根：根非自签 / 根指纹与编译期锚不匹配。
    NoRootForIssuer,
    /// 证书不在有效期内。
    Expired,
    /// 证书签名验不过。
    BadSignature,
    /// leaf 缺 codeSigning EKU（D4）。
    MissingCodeSigningEku,
}

impl std::fmt::Display for ChainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChainError::Empty => write!(f, "empty certificate chain"),
            ChainError::Malformed(e) => write!(f, "malformed certificate: {e}"),
            ChainError::UnsupportedAlgorithm => write!(f, "unsupported signature algorithm"),
            ChainError::InvalidKey => write!(f, "invalid public key"),
            ChainError::BrokenChain => write!(f, "broken certificate chain"),
            ChainError::NoRootForIssuer => write!(f, "no trusted root for issuer"),
            ChainError::Expired => write!(f, "certificate expired"),
            ChainError::BadSignature => write!(f, "bad certificate signature"),
            ChainError::MissingCodeSigningEku => write!(f, "missing codeSigning EKU"),
        }
    }
}

impl std::error::Error for ChainError {}

impl Certificate {
    /// 从 DER 解析（结构合法性在此把关）。
    pub fn from_der(bytes: &[u8]) -> Result<Self, ChainError> {
        X509Certificate::from_der(bytes)
            .map_err(|e| ChainError::Malformed(format!("DER parse: {e}")))?;
        Ok(Certificate {
            der: bytes.to_vec(),
        })
    }

    /// 原始 DER。
    pub fn to_der(&self) -> &[u8] {
        &self.der
    }

    /// 证书 SHA-256 指纹（根锚口径：SHA-256(根证书 DER)，S4-2）。
    pub fn sha256_fingerprint(&self) -> [u8; 32] {
        sha2::Sha256::digest(&self.der).into()
    }

    /// 解析为 x509-cert 结构（每次独立解析，无缓存——证书量小，成本可忽略）。
    pub fn parsed(&self) -> Result<X509Certificate, ChainError> {
        X509Certificate::from_der(&self.der).map_err(|e| ChainError::Malformed(format!("{e}")))
    }

    /// TBS 原始字节段（byte-exact 切片，签名覆盖面的唯一口径；不重编码）。
    fn tbs_raw(&self) -> Result<&[u8], ChainError> {
        // 游标式解析（der 0.7 无 from_bytes 逐头前进 API；SliceReader 逐头剥）：
        // 外层 Certificate SEQUENCE 头 → 剥掉 → 第二个 TLV 即 TBS。
        let mut reader = SliceReader::new(&self.der)
            .map_err(|e| ChainError::Malformed(format!("outer header: {e}")))?;
        Header::decode(&mut reader)
            .map_err(|e| ChainError::Malformed(format!("outer header: {e}")))?;
        let tbs_start: usize = reader
            .position()
            .try_into()
            .map_err(|_| ChainError::Malformed("position overflow".into()))?;
        let tbs_header: Header = reader
            .decode()
            .map_err(|e| ChainError::Malformed(format!("tbs header: {e}")))?;
        let tbs_val_start: usize = reader
            .position()
            .try_into()
            .map_err(|_| ChainError::Malformed("position overflow".into()))?;
        let tbs_value = reader
            .read_slice(tbs_header.length)
            .map_err(|e| ChainError::Malformed(format!("tbs span: {e}")))?;
        let end = tbs_val_start + tbs_value.len();
        if end > self.der.len() {
            return Err(ChainError::Malformed(
                "TBS extends past certificate end".into(),
            ));
        }
        Ok(&self.der[tbs_start..end])
    }

    /// 主体公钥（P-256，SEC1 点取自 SPKI BIT STRING；算法必须 id-ecPublicKey + P-256）。
    pub fn subject_public_key(&self) -> Result<VerifyingKey, ChainError> {
        let cert = self.parsed()?;
        let spki = &cert.tbs_certificate.subject_public_key_info;
        if spki.algorithm.oid != OID_EC_PUBLIC_KEY {
            return Err(ChainError::InvalidKey);
        }
        let curve: ObjectIdentifier = match spki.algorithm.parameters.as_ref() {
            Some(p) => {
                let pder = p
                    .to_der()
                    .map_err(|e| ChainError::Malformed(format!("curve der: {e}")))?;
                ObjectIdentifier::from_der(&pder)
                    .map_err(|e| ChainError::Malformed(format!("curve param: {e}")))?
            }
            None => return Err(ChainError::InvalidKey),
        };
        if curve != OID_P256 {
            return Err(ChainError::InvalidKey);
        }
        VerifyingKey::from_sec1_bytes(spki.subject_public_key.raw_bytes())
            .map_err(|_| ChainError::InvalidKey)
    }

    /// SKI 扩展值（在场时）。
    pub fn ski(&self) -> Result<Option<Vec<u8>>, ChainError> {
        let cert = self.parsed()?;
        match cert
            .tbs_certificate
            .get::<SubjectKeyIdentifier>()
            .map_err(|e| ChainError::Malformed(format!("SKI ext: {e}")))?
        {
            Some((_, ski)) => Ok(Some(ski.0.as_bytes().to_vec())),
            None => Ok(None),
        }
    }

    /// AKI 扩展 keyIdentifier（在场时）。
    pub fn aki(&self) -> Result<Option<Vec<u8>>, ChainError> {
        let cert = self.parsed()?;
        match cert
            .tbs_certificate
            .get::<AuthorityKeyIdentifier>()
            .map_err(|e| ChainError::Malformed(format!("AKI ext: {e}")))?
        {
            Some((_, aki)) => Ok(aki.key_identifier.map(|k| k.as_bytes().to_vec())),
            None => Ok(None),
        }
    }

    /// 签名算法是否为接受的套件（ecdsa-with-SHA256）。
    pub fn signature_algorithm_ok(&self) -> bool {
        self.parsed()
            .map(|c| c.signature_algorithm.oid == OID_ECDSA_WITH_SHA256)
            .unwrap_or(false)
    }

    /// 是否在有效期 `[not_before, not_after]` 内（unix 秒）。
    pub fn is_valid_at(&self, now_unix: u64) -> Result<bool, ChainError> {
        let cert = self.parsed()?;
        let validity = &cert.tbs_certificate.validity;
        let nb = validity.not_before.to_unix_duration().as_secs();
        let na = validity.not_after.to_unix_duration().as_secs();
        Ok(now_unix >= nb && now_unix <= na)
    }

    /// 是否带 codeSigning EKU（D4 leaf 必带）。
    pub fn has_code_signing_eku(&self) -> Result<bool, ChainError> {
        let cert = self.parsed()?;
        match cert
            .tbs_certificate
            .get::<ExtendedKeyUsage>()
            .map_err(|e| ChainError::Malformed(format!("EKU ext: {e}")))?
        {
            Some((_, eku)) => Ok(eku.0.contains(&OID_EKU_CODE_SIGNING)),
            None => Ok(false),
        }
    }

    /// 用签发者公钥验证本证书签名（TBS 原始字节 + ecdsa-with-SHA256）。
    pub fn verify_signature_by(&self, issuer_vk: &VerifyingKey) -> Result<(), ChainError> {
        use p256::ecdsa::signature::Verifier;
        let cert = self.parsed()?;
        if cert.signature_algorithm.oid != OID_ECDSA_WITH_SHA256 {
            return Err(ChainError::UnsupportedAlgorithm);
        }
        // BIT STRING 内是 DER 编码的 ECDSA-Sig-Value (r, s)。
        let der_sig = ecdsa::der::Signature::from_der(cert.signature.raw_bytes())
            .map_err(|_| ChainError::BadSignature)?;
        let fixed: Signature = der_sig.try_into().map_err(|_| ChainError::BadSignature)?;
        issuer_vk
            .verify(self.tbs_raw()?, &fixed)
            .map_err(|_| ChainError::BadSignature)
    }

    /// 是否自签（根判定）：AKI==SKI（或 AKI 缺省）且自签名验过。
    pub fn is_self_signed(&self) -> Result<bool, ChainError> {
        let (aki, ski, vk) = (self.aki()?, self.ski()?, self.subject_public_key()?);
        let ids_match = match (&aki, &ski) {
            (Some(a), Some(s)) => a == s,
            // AKI 缺省 = 自签惯例形态（RFC 5280 4.2.1.1）；有 AKI 无 SKI / 两者皆缺
            // → 不可判定，不认。
            (None, Some(_)) => true,
            _ => false,
        };
        if !ids_match {
            return Ok(false);
        }
        Ok(self.verify_signature_by(&vk).is_ok())
    }

    /// 主体 CN（展示用 best-effort：ATV oid==CN 取值字节按 UTF-8 容错解码；
    /// 无 CN ATV 返回 None）。
    pub fn subject_cn(&self) -> Result<Option<String>, ChainError> {
        let cert = self.parsed()?;
        for rdn in cert.tbs_certificate.subject.0.iter() {
            for atv in rdn.0.iter() {
                if atv.oid == OID_AT_CN {
                    return Ok(Some(
                        String::from_utf8_lossy(atv.value.value()).into_owned(),
                    ));
                }
            }
        }
        Ok(None)
    }
}

/// 验证证书链（v4）。
///
/// `chain` = `[leaf, issuing_ca, ..., root]`（leaf 在前，**含根**——S0-4 实证约束：
/// 嵌入根 ≠ 信任根，仅为链引擎提供终止点）。终止条件 = SHA-256(根 DER) 匹配
/// `root_anchor_fp`（编译期根锚，S4-2 口径）。
pub fn verify_chain(
    chain: &[Certificate],
    root_anchor_fp: &[u8; 32],
    now_unix: u64,
) -> Result<(), ChainError> {
    if chain.is_empty() {
        return Err(ChainError::Empty);
    }

    // 逐级：有效期 + AKI/SKI + 逐级签名。
    for (i, cert) in chain.iter().enumerate() {
        if !cert.is_valid_at(now_unix)? {
            return Err(ChainError::Expired);
        }
        if i + 1 < chain.len() {
            let issuer = &chain[i + 1];
            // AKI/SKI 匹配（cert 无 AKI 时跳过——X.509 扩展可省略）。
            if let Some(aki) = cert.aki()? {
                match issuer.ski()? {
                    Some(ski) if ski == aki => {}
                    _ => return Err(ChainError::BrokenChain),
                }
            }
            cert.verify_signature_by(&issuer.subject_public_key()?)?;
        }
    }

    // 终止条件：末级必须是自签根且指纹钉死到编译期锚。
    let root = chain.last().expect("non-empty checked above");
    if !root.is_self_signed()? {
        return Err(ChainError::NoRootForIssuer);
    }
    if &root.sha256_fingerprint() != root_anchor_fp {
        return Err(ChainError::NoRootForIssuer);
    }

    // leaf 职责约束：codeSigning EKU（D4）。
    if !chain[0].has_code_signing_eku()? {
        return Err(ChainError::MissingCodeSigningEku);
    }
    Ok(())
}

/// 单值 RDN（UTF8String ATV）。
fn rd(oid: ObjectIdentifier, value: &str) -> Result<RelativeDistinguishedName, ChainError> {
    let utf8 =
        Utf8StringRef::new(value).map_err(|e| ChainError::Malformed(format!("DN utf8: {e}")))?;
    let any = Any::from_der(
        &utf8
            .to_der()
            .map_err(|e| ChainError::Malformed(format!("DN der: {e}")))?,
    )
    .map_err(|e| ChainError::Malformed(format!("DN any: {e}")))?;
    RelativeDistinguishedName::try_from(vec![AttributeTypeAndValue { oid, value: any }])
        .map_err(|e| ChainError::Malformed(format!("RDN: {e}")))
}

/// 主体/签发者 DN（CN + 可选 O）。
pub fn distinguished_name(cn: &str, org: Option<&str>) -> Result<Name, ChainError> {
    let mut rdns = vec![rd(OID_AT_CN, cn)?];
    if let Some(o) = org {
        rdns.push(rd(OID_AT_O, o)?);
    }
    Ok(RdnSequence(rdns))
}

/// TBS 构造输入（keygen 积木；测试用它组合失败形态证书）。
pub struct TbsInput<'a> {
    /// 主体 CN。
    pub subject_cn: &'a str,
    /// 主体 O。
    pub subject_org: Option<&'a str>,
    /// 签发者 CN。
    pub issuer_cn: &'a str,
    /// 签发者 O。
    pub issuer_org: Option<&'a str>,
    /// CA 证书。
    pub is_ca: bool,
    /// basicConstraints pathLenConstraint（仅 is_ca 时生效）。
    pub path_len: Option<u8>,
    /// KU digitalSignature。
    pub ku_digital_signature: bool,
    /// KU keyCertSign。
    pub ku_key_cert_sign: bool,
    /// KU crlSign。
    pub ku_crl_sign: bool,
    /// EKU codeSigning。
    pub eku_code_signing: bool,
    /// 有效期起（unix 秒）。
    pub not_before_unix: u64,
    /// 有效期止（unix 秒）。
    pub not_after_unix: u64,
}

/// 构造未签名 TBS：随机序列号、SKP/AKI 由密钥注入（keygen 积木）。
///
/// `subject_vk` 取主体公钥；`issuer_ski` = 签发者 SKI 值（自签时 = 主体自身 SKI）。
pub fn build_tbs(
    subject_vk: &VerifyingKey,
    issuer_ski: &[u8],
    serial: &[u8],
    input: &TbsInput<'_>,
) -> Result<TbsCertificate, ChainError> {
    let subject_ski = ski_value(subject_vk)?;
    let mut extensions: Vec<Extension> = Vec::new();

    // BasicConstraints（critical；RFC 5280 CA 证书必带，终端实体推荐带）。
    let bc = BasicConstraints {
        ca: input.is_ca,
        path_len_constraint: if input.is_ca { input.path_len } else { None },
    };
    extensions.push(Extension {
        extn_id: OID_CE_BASIC_CONSTRAINTS,
        critical: true,
        extn_value: OctetString::new(
            bc.to_der()
                .map_err(|e| ChainError::Malformed(format!("BC der: {e}")))?,
        )
        .map_err(|e| ChainError::Malformed(format!("BC octet: {e}")))?,
    });

    // KeyUsage（critical）。
    let ku = ku_flagset(input)?;
    extensions.push(Extension {
        extn_id: OID_CE_KEY_USAGE,
        critical: true,
        extn_value: OctetString::new(
            ku.to_der()
                .map_err(|e| ChainError::Malformed(format!("KU der: {e}")))?,
        )
        .map_err(|e| ChainError::Malformed(format!("KU octet: {e}")))?,
    });

    // SKI（非 critical）= SHA-256(主体公钥 uncompressed 65B)，与 key_fp 同口径。
    let ski_ext = SubjectKeyIdentifier(
        OctetString::new(subject_ski.clone())
            .map_err(|e| ChainError::Malformed(format!("SKI octet: {e}")))?,
    );
    extensions.push(Extension {
        extn_id: OID_CE_SUBJECT_KEY_IDENTIFIER,
        critical: false,
        extn_value: OctetString::new(
            ski_ext
                .to_der()
                .map_err(|e| ChainError::Malformed(format!("SKI der: {e}")))?,
        )
        .map_err(|e| ChainError::Malformed(format!("SKI ext octet: {e}")))?,
    });

    // AKI（非 critical；keyIdentifier = 签发者 SKI）。
    let aki = AuthorityKeyIdentifier {
        key_identifier: Some(
            OctetString::new(issuer_ski.to_vec())
                .map_err(|e| ChainError::Malformed(format!("AKI octet: {e}")))?,
        ),
        authority_cert_issuer: None,
        authority_cert_serial_number: None,
    };
    extensions.push(Extension {
        extn_id: OID_CE_AUTHORITY_KEY_IDENTIFIER,
        critical: false,
        extn_value: OctetString::new(
            aki.to_der()
                .map_err(|e| ChainError::Malformed(format!("AKI der: {e}")))?,
        )
        .map_err(|e| ChainError::Malformed(format!("AKI ext octet: {e}")))?,
    });

    // EKU（非 critical；codeSigning）。
    if input.eku_code_signing {
        let eku = ExtendedKeyUsage(vec![OID_EKU_CODE_SIGNING]);
        extensions.push(Extension {
            extn_id: OID_CE_EXT_KEY_USAGE,
            critical: false,
            extn_value: OctetString::new(
                eku.to_der()
                    .map_err(|e| ChainError::Malformed(format!("EKU der: {e}")))?,
            )
            .map_err(|e| ChainError::Malformed(format!("EKU octet: {e}")))?,
        });
    }

    Ok(TbsCertificate {
        version: Version::V3,
        serial_number: SerialNumber::new(serial)
            .map_err(|e| ChainError::Malformed(format!("serial: {e}")))?,
        signature: ecdsa_sha256_alg_id(),
        issuer: distinguished_name(input.issuer_cn, input.issuer_org)?,
        validity: Validity {
            not_before: gt_time(input.not_before_unix)?,
            not_after: gt_time(input.not_after_unix)?,
        },
        subject: distinguished_name(input.subject_cn, input.subject_org)?,
        subject_public_key_info: subject_public_key_info(subject_vk)?,
        issuer_unique_id: None,
        subject_unique_id: None,
        extensions: Some(extensions),
    })
}

/// 组装完整证书 DER：TBS + 签发者签名（ecdsa-with-SHA256）。
pub fn seal_certificate(
    tbs: TbsCertificate,
    issuer_sk: &SigningKey,
) -> Result<Certificate, ChainError> {
    let tbs_der = tbs
        .to_der()
        .map_err(|e| ChainError::Malformed(format!("tbs der: {e}")))?;
    let sig = crypto::p256_sign(issuer_sk, &tbs_der);
    let sig_der = ecdsa::der::Signature::from(
        Signature::from_slice(&sig).map_err(|_| ChainError::BadSignature)?,
    )
    .to_der()
    .map_err(|e| ChainError::Malformed(format!("sig der: {e}")))?;
    let cert = X509Certificate {
        tbs_certificate: tbs,
        signature_algorithm: ecdsa_sha256_alg_id(),
        signature: der::asn1::BitString::new(0, sig_der)
            .map_err(|e| ChainError::Malformed(format!("sig bitstring: {e}")))?,
    };
    let der = cert
        .to_der()
        .map_err(|e| ChainError::Malformed(format!("cert der: {e}")))?;
    Certificate::from_der(&der)
}

/// KU 三布尔 → [`KeyUsage`]（本系统 profile 有限，穷举组合避免空集；
/// `KeyUsages::BitOr` 产出 FlagSet，链式组合见 x509-cert builder 同款写法）。
fn ku_flagset(input: &TbsInput<'_>) -> Result<KeyUsage, ChainError> {
    let ku = match (
        input.ku_digital_signature,
        input.ku_key_cert_sign,
        input.ku_crl_sign,
    ) {
        (true, true, true) => {
            KeyUsages::DigitalSignature | KeyUsages::KeyCertSign | KeyUsages::CRLSign
        }
        (true, true, false) => KeyUsages::DigitalSignature | KeyUsages::KeyCertSign,
        (true, false, true) => KeyUsages::DigitalSignature | KeyUsages::CRLSign,
        (true, false, false) => KeyUsages::DigitalSignature.into(),
        (false, true, true) => KeyUsages::KeyCertSign | KeyUsages::CRLSign,
        (false, true, false) => KeyUsages::KeyCertSign.into(),
        (false, false, true) => KeyUsages::CRLSign.into(),
        (false, false, false) => {
            return Err(ChainError::Malformed("KU 全空（profile 非法）".into()));
        }
    };
    Ok(KeyUsage(ku))
}

/// ecdsa-with-SHA256 算法标识（parameters 缺省，RFC 5280）。
fn ecdsa_sha256_alg_id() -> AlgorithmIdentifierOwned {
    AlgorithmIdentifierOwned {
        oid: OID_ECDSA_WITH_SHA256,
        parameters: None,
    }
}

/// 有效期时点（GeneralizedTime，覆盖 >2049 年份；RFC 5280 允许）。
fn gt_time(unix_secs: u64) -> Result<Time, ChainError> {
    Ok(Time::GeneralTime(
        GeneralizedTime::from_unix_duration(std::time::Duration::from_secs(unix_secs))
            .map_err(|e| ChainError::Malformed(format!("time: {e}")))?,
    ))
}

/// 主体公钥 SPKI（id-ecPublicKey + P-256 命名曲线 + uncompressed 点）。
fn subject_public_key_info(
    vk: &VerifyingKey,
) -> Result<SubjectPublicKeyInfo<Any, der::asn1::BitString>, ChainError> {
    let point = vk.as_affine().to_encoded_point(false);
    Ok(SubjectPublicKeyInfo {
        algorithm: AlgorithmIdentifierOwned {
            oid: OID_EC_PUBLIC_KEY,
            parameters: Some(
                Any::from_der(
                    &OID_P256
                        .to_der()
                        .map_err(|e| ChainError::Malformed(format!("curve der: {e}")))?,
                )
                .map_err(|e| ChainError::Malformed(format!("curve any: {e}")))?,
            ),
        },
        subject_public_key: der::asn1::BitString::new(0, point.as_bytes().to_vec())
            .map_err(|e| ChainError::Malformed(format!("spki bitstring: {e}")))?,
    })
}

/// SKI 值（SHA-256(公钥 uncompressed 65B)——与 [`crate::crypto::key_fp`] 同口径，
/// AKI/SKI 匹配内洽）。
pub fn ski_value(vk: &VerifyingKey) -> Result<Vec<u8>, ChainError> {
    let point = vk.as_affine().to_encoded_point(false);
    Ok(crypto::key_fp(point.as_bytes()).to_vec())
}

/// 解析 DER 列表为证书链（每项独立 DER 校验）。
pub fn parse_chain(ders: &[Vec<u8>]) -> Result<Vec<Certificate>, ChainError> {
    ders.iter().map(|d| Certificate::from_der(d)).collect()
}

/// 证书链序列化为单 blob（envelope cert_chain TLV 载荷形态，v3 起沿用）：
/// `u16 LE 证书数 + 每张 [u32 LE 长度 + DER]`。
pub fn serialize_chain(chain: &[Certificate]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(chain.len() as u16).to_le_bytes());
    for c in chain {
        let der = c.to_der();
        out.extend_from_slice(&(der.len() as u32).to_le_bytes());
        out.extend_from_slice(der);
    }
    out
}

/// 解析单 blob 为证书链（[`serialize_chain`] 逆操作；每张 DER 独立校验）。
pub fn parse_chain_blob(bytes: &[u8]) -> Result<Vec<Certificate>, ChainError> {
    if bytes.len() < 2 {
        return Err(ChainError::Malformed("chain blob too short".into()));
    }
    let n = u16::from_le_bytes([bytes[0], bytes[1]]) as usize;
    let mut ders = Vec::with_capacity(n);
    let mut i = 2;
    for _ in 0..n {
        if i + 4 > bytes.len() {
            return Err(ChainError::Malformed("chain blob len header".into()));
        }
        let l = u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]) as usize;
        i += 4;
        if i + l > bytes.len() {
            return Err(ChainError::Malformed("chain blob cert span".into()));
        }
        ders.push(bytes[i..i + l].to_vec());
        i += l;
    }
    parse_chain(&ders)
}

/// 签发随机序列号（16B，最高位清零保证正整数，不允许全零）。
pub fn random_serial() -> Vec<u8> {
    use rand::RngCore;
    let mut b = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut b);
    b[0] &= 0x7f;
    if b.iter().all(|&x| x == 0) {
        b[15] = 1;
    }
    b.to_vec()
}

#[cfg(test)]
mod tests;
