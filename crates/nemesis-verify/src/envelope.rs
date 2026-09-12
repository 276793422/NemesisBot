//! Envelope v4（Authenticode 对齐）：CMS SignedData 签名块 + v4 locator footer。
//!
//! # v4 CMS 层（S2-1 起，本文件上半段）
//! 签名载荷 = **CMS SignedData**（PKCS#7，Authenticode 形态，微软工具可解析）：
//! - eContentType = SPC_INDIRECT_DATA_OBJID，eContent = `[0]` 直接包
//!   SpcIndirectDataContent SEQUENCE（**无 OCTET STRING 包装**，MS 对 RFC 5652 的知名偏离）
//! - SpcIndirectDataContent = `{ data SpcAttributeTypeAndOptionalValue, messageDigest DigestInfo }`
//!   （data 在前；data.value = SpcPeImageData；DigestInfo.digestAlgorithm 带 NULL parameters）
//! - SignerInfo 认证属性：contentType / messageDigest / signingTime / SPC_SP_OPUS_INFO
//! - SignedData.version 钉 **V1**（Authenticode 约定）；证书集**含全部三级证书（含根）**
//!   （S0-4 实证：缺根 → Windows 链引擎报 CERT_E_CHAINING 而非 UNTRUSTEDROOT；嵌入根≠信任根）
//! - 构造经 `cms` crate builder（S0-6 选型）+ 类型化后处理（V1 改写，version 不在签名覆盖面）
//! - 载体：PE → Certificate Table（归 S3）；ELF/raw → locator footer v4（S2-3，见下）
//!
//! ## v4 FOOTER（ELF/raw 载体，明文 64B，文件/overlay 末尾）
//! 布局 `[ 原内容 ][ CMS ContentInfo DER ][ v4 footer ]`（无对齐要求；
//! 被保护内容 = 文件 `[0, content_len)`，摘要经 SpcIndirectDataContent.messageDigest 进 CMS）。
//!
//! | 偏移 | 长度 | 字段 |
//! |------|------|------|
//! | 0  | 8  | magic `NMBSIG\x04\x00` |
//! | 8  | 1  | format_ver（=4）|
//! | 9  | 1  | sig_algo（2=ecdsa-p256）|
//! | 10 | 1  | format_tag（2=ELF / 3=Raw；1=PE 走 Certificate Table 不用 footer）|
//! | 11 | 1  | reserved（0）|
//! | 12 | 4  | reserved（0）|
//! | 16 | 8  | content_len（被保护原始内容长度，u64 LE）|
//! | 24 | 8  | cms_off（CMS DER 文件内绝对偏移，u64 LE）|
//! | 32 | 4  | cms_len（CMS DER 长度，u32 LE）|
//! | 36 | 4  | footer_crc32（footer\[0..36\] 校验，LE）|
//! | 40 | 24 | reserved（0）|
//!
//! # 历史（v3 NMBSIG，S5-3 已删除）
//! v3 envelope（Ed25519→P-256 时代的 footer+body 自定义布局）随最后一个生产
//! 消费方迁移整体退役（S5-3）：v4 管线对 v3 签名文件 = NoSignature（S4-1 破坏性
//! 声明），v3 形态只剩测试里的 footer 字节字面量（破坏性钉死用）。footer 定位
//! 机制（magic 末尾扫描 + overlay 下界 + 排除区）由 v4 [`find_footer_v4`] 延续。

use crate::codec::{FORMAT_TAG_PE, detect_codec, detect_format};
use anyhow::{Result, anyhow};

/// footer 固定长度（v3/v4 同长——从末尾扫描的定位机制语义一致）。
pub const FOOTER_LEN: usize = 64;

/// sig_algo：ECDSA P-256 + SHA-256。
pub const SIG_ALGO_ECDSA_P256: u8 = 2;

// footer 字段偏移（v3/v4 共用前 12 字节形态：magic + format_ver + sig_algo + format_tag）
const OFF_MAGIC: usize = 0;
const OFF_FORMAT_VER: usize = 8;
const OFF_SIG_ALGO: usize = 9;
const OFF_FORMAT_TAG: usize = 10;

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

// ============================================================================
// v4 CMS SignedData 层（S2-1，Authenticode 形态；S0-6 选型 = cms crate builder
// + 类型化后处理。载荷假设全部经 S0-6 spike 实证，见 goal §七 S0-6 五条结论）
// ============================================================================

use cms::builder::{SignedDataBuilder, SignerInfoBuilder};
use cms::cert::{CertificateChoices, IssuerAndSerialNumber};
use cms::content_info::{CmsVersion, ContentInfo};
use cms::signed_data::{
    EncapsulatedContentInfo, SignedData, SignerIdentifier, SignerInfo, SignerInfos,
};
use der::asn1::{
    BitStringRef, BmpString, Ia5String, ObjectIdentifier, OctetStringRef, SetOfVec, UtcTime,
};
use der::{Any, Choice, Decode, Encode, Sequence, Tag};
use p256::ecdsa::SigningKey;
use x509_cert::attr::Attribute;
use x509_cert::spki::AlgorithmIdentifierOwned;

/// SPC_INDIRECT_DATA_OBJID（eContentType，Authenticode 签名载荷类型）。
pub const OID_SPC_INDIRECT_DATA: ObjectIdentifier =
    ObjectIdentifier::new_unwrap("1.3.6.1.4.1.311.2.1.4");
/// SPC_PE_IMAGE_DATA_OBJID（data.type，PE 图像数据）。
pub const OID_SPC_PE_IMAGE_DATA: ObjectIdentifier =
    ObjectIdentifier::new_unwrap("1.3.6.1.4.1.311.2.1.15");
/// SPC_SP_OPUS_INFO_OBJID（SignerInfo 认证属性：发布者信息）。
pub const OID_SPC_SP_OPUS_INFO: ObjectIdentifier =
    ObjectIdentifier::new_unwrap("1.3.6.1.4.1.311.2.1.12");
/// SPC_STATEMENT_TYPE_OBJID（SignerInfo 认证属性：声明类型；值域同一 OID 族）。
pub const OID_SPC_STATEMENT_TYPE: ObjectIdentifier =
    ObjectIdentifier::new_unwrap("1.3.6.1.4.1.311.2.1.11");
/// SPC_INDIVIDUAL_SP_KEY_PURPOSE_OBJID（MS_INDIVIDUAL_CODE_SIGNING 声明）。
pub const OID_SPC_INDIVIDUAL_CODE_SIGNING: ObjectIdentifier =
    ObjectIdentifier::new_unwrap("1.3.6.1.4.1.311.2.1.11");
/// SHA-256（digest 算法）。
pub const OID_SHA256: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.2.1");
/// signedData（外层 ContentInfo.contentType）。
pub const OID_SIGNED_DATA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.7.2");
/// contentType（认证属性）。
pub const OID_ATTR_CONTENT_TYPE: ObjectIdentifier =
    ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.3");
/// messageDigest（认证属性）。
pub const OID_ATTR_MESSAGE_DIGEST: ObjectIdentifier =
    ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.4");
/// signingTime（认证属性）。
pub const OID_ATTR_SIGNING_TIME: ObjectIdentifier =
    ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.5");
/// SPC_NESTED_SIGNATURE（SignerInfo **无认证**属性：嵌套完整签名 ContentInfo）。
///
/// 追加多签名的微软真实载体——2026-09-12 signtool `/as` 双签名对照实验实证
/// （spike dual1.exe asn1parse）：signtool 双签名 = **单** WIN_CERTIFICATE 条目
/// 单 CMS，第二个签名以本属性挂在主 SignerInfo 的 unauthenticatedAttributes
/// （属性值 = SET { 完整嵌套 ContentInfo DER }），signtool verify 的
/// 「Signature Index」从属性树枚举；**多 WIN_CERTIFICATE 条目形态 Windows
/// 主签名枚举不可见，v4 否决**（原 S3-3 多条目方案因此推翻）。
pub const OID_SPC_NESTED_SIGNATURE: ObjectIdentifier =
    ObjectIdentifier::new_unwrap("1.3.6.1.4.1.311.2.4.1");

/// SpcIndirectDataContent ::= SEQUENCE { data SpcAttributeTypeAndOptionalValue,
/// messageDigest DigestInfo }——真 MS 形态（[MS-OSHARED] §2.1.3 + osslsig.der 实测；
/// S0-6 推翻「DigestInfo 在前」的初版假设）。
#[derive(Sequence)]
pub struct SpcIndirectDataContent<'a> {
    pub data: SpcAttributeTypeAndOptionalValue,
    pub message_digest: DigestInfo<'a>,
}

/// SpcAttributeTypeAndOptionalValue ::= SEQUENCE { type OID, value ANY DEFINED BY type }。
/// value 不加 `#[asn1(type=...)]`：der_derive 0.7 非 optional type-attr 字段按值 binding
/// 会撞 E0308（S0-6 坑位 5a），Any 字段经 from_der 显式装入。
#[derive(Sequence)]
pub struct SpcAttributeTypeAndOptionalValue {
    pub type_: ObjectIdentifier,
    pub value: Any,
}

/// DigestInfo ::= SEQUENCE { digestAlgorithm AlgorithmIdentifier, digest OCTET STRING }。
/// digestAlgorithm 带 NULL parameters（osslsigncode 形态，S0-6 实证）。
#[derive(Sequence)]
pub struct DigestInfo<'a> {
    pub digest_algorithm: AlgorithmIdentifierOwned,
    pub digest: OctetStringRef<'a>,
}

/// SpcPeImageData ::= SEQUENCE { flags BIT STRING, file [0] EXPLICIT SpcLink OPTIONAL }。
/// flags = `03 02 07 80`（bit0 置位，osslsig.der 逐字节一致）；file = "<<<Obsolete>>>"。
#[derive(Sequence)]
pub struct SpcPeImageData<'a> {
    #[asn1(type = "BIT STRING")]
    pub flags: BitStringRef<'a>,
    #[asn1(context_specific = "0", tag_mode = "EXPLICIT", optional = "true")]
    pub file: Option<SpcLink>,
}

/// SpcLink ::= CHOICE { url [0] IMPLICIT IA5String, moniker [1], file [2] EXPLICIT SpcString }。
#[derive(Choice)]
pub enum SpcLink {
    #[asn1(context_specific = "0", tag_mode = "IMPLICIT", type = "IA5String")]
    Url(Ia5String),
    #[asn1(context_specific = "2", tag_mode = "EXPLICIT")]
    File(SpcString),
}

/// SpcString ::= CHOICE { unicode [0] IMPLICIT BMPString, ascii [1] IMPLICIT IA5String }。
#[derive(Choice)]
pub enum SpcString {
    #[asn1(context_specific = "0", tag_mode = "IMPLICIT")]
    Unicode(BmpString),
    #[asn1(context_specific = "1", tag_mode = "IMPLICIT", type = "IA5String")]
    Ascii(Ia5String),
}

/// SpcSpOpusInfo ::= SEQUENCE { programName [0] EXPLICIT SpcString OPTIONAL,
/// moreInfo [1] EXPLICIT SpcLink OPTIONAL }（osslsigncode 形态：ascii 名 + url）。
#[derive(Sequence)]
pub struct SpcSpOpusInfo {
    #[asn1(context_specific = "0", tag_mode = "EXPLICIT", optional = "true")]
    pub program_name: Option<SpcString>,
    #[asn1(context_specific = "1", tag_mode = "EXPLICIT", optional = "true")]
    pub more_info: Option<SpcLink>,
}

/// 构造 sha256 AlgorithmIdentifier（带 NULL parameters，osslsigncode 形态）。
pub fn sha256_algorithm() -> Result<AlgorithmIdentifierOwned> {
    Ok(AlgorithmIdentifierOwned {
        oid: OID_SHA256,
        parameters: Some(Any::new(Tag::Null, Vec::new())?),
    })
}

/// 构造 SpcIndirectDataContent DER（eContent 载荷；digest = PE byte-range 摘要，
/// S3-1 前 Raw 场景用整内容 SHA-256。opus 信息不属于此结构——它是 SignerInfo 认证属性）。
///
/// file 链接字段**手工编码**（osslsigncode 产物逐字节形态）：
/// `30 26 | 03 02 07 80 | A0 20 | A2 1E | 80 1C | <28B BMP UTF-16BE "<<<Obsolete>>>">`
/// 即 SpcPeImageData { flags, file [0] EXPLICIT { [2] CONSTRUCTED { [0] BMP } } }。
/// 不能走 [`SpcLink::File`] der_derive 编码：Choice 型 variant 上的
/// `tag_mode="EXPLICIT"` 被 der_derive 按 IMPLICIT 式标签替换处理
/// （选中 tag `0x80`｜`0x02` = `0x82`，丢 constructed 位）→ 非法 ASN.1；
/// Windows 解码 SpcIndirectData 中断 → 文件摘要比对失败报 HashMismatch
/// （S3-2 对照实验实证：我方产物与 osslsigncode 产物仅差 0x82/0xA2 一字节）。
/// 各段长度均 < 128，单字节长度形式。
pub fn build_spc_indirect_data(content_digest: &[u8; 32]) -> Result<Vec<u8>> {
    const OBSOLETE_NAME: &str = "<<<Obsolete>>>";
    let mut bmp = Vec::with_capacity(OBSOLETE_NAME.len() * 2);
    for u in OBSOLETE_NAME.encode_utf16() {
        bmp.extend_from_slice(&u.to_be_bytes());
    }
    debug_assert_eq!(bmp.len(), 28);
    // SpcLink 层：A2 1E { 80 1C <BMP> }（[2] constructed { [0] IMPLICIT BMPString }）
    let mut link = Vec::with_capacity(4 + bmp.len());
    link.extend_from_slice(&[0xA2, (bmp.len() + 2) as u8, 0x80, bmp.len() as u8]);
    link.extend_from_slice(&bmp);
    // file [0] EXPLICIT：A0 len { link }
    let mut file_field = Vec::with_capacity(2 + link.len());
    file_field.extend_from_slice(&[0xA0, link.len() as u8]);
    file_field.extend_from_slice(&link);
    // SpcPeImageData value：30 len { 03 02 07 80, file [0] }
    let mut value = vec![0x30, (4 + file_field.len()) as u8, 0x03, 0x02, 0x07, 0x80];
    value.extend_from_slice(&file_field);

    let spc = SpcIndirectDataContent {
        data: SpcAttributeTypeAndOptionalValue {
            type_: OID_SPC_PE_IMAGE_DATA,
            value: Any::from_der(&value)?,
        },
        message_digest: DigestInfo {
            digest_algorithm: sha256_algorithm()?,
            digest: OctetStringRef::new(content_digest)?,
        },
    };
    Ok(spc.to_der()?)
}

/// signingTime 认证属性（UTCTime；不用 cms 的 create_signing_time_attribute——
/// 那个取真实时钟，本实现钉 signed_at 参数保证构造确定性）。
fn signing_time_attribute(signed_at: u64) -> Result<Attribute> {
    let t = UtcTime::from_unix_duration(std::time::Duration::from_secs(signed_at))
        .map_err(|e| anyhow!("signingTime UtcTime（UTCTime 域 1950-2049）: {}", e))?;
    let mut values = SetOfVec::new();
    // Any 需完整 TLV：UtcTime::to_der 给全 TLV，from_der 剥成 Any
    values.insert(Any::from_der(t.to_der()?.as_slice())?)?;
    Ok(Attribute {
        oid: OID_ATTR_SIGNING_TIME,
        values,
    })
}

/// 构造 Authenticode 形态 CMS SignedData，返回完整 ContentInfo DER。
///
/// - `content_digest`：被签内容摘要（S3-1 起 = PE byte-range digest；过渡期 Raw = 整内容 SHA-256）
/// - `certs`：证书链（**必须含全部三级含根**，leaf 在前——S0-4 实证约束；
///   签名者取 certs[0]）
/// - `program_name` / `more_info_url`：SPC_SP_OPUS_INFO 字段（两者全 None 时省略该属性）
///
/// MS 载荷形态（S0-6 实证）：eContent `[0]` 直接包 SEQUENCE；SignedData.version 钉 V1
/// （cms builder 按 RFC 5652 强制 V3，构造后类型化改写；version 不在签名覆盖面）。
pub fn build_signed_data(
    content_digest: &[u8; 32],
    sk: &SigningKey,
    signed_at: u64,
    certs: &[crate::cert::Certificate],
    program_name: Option<&str>,
    more_info_url: Option<&str>,
) -> Result<Vec<u8>> {
    let leaf = certs
        .first()
        .ok_or_else(|| anyhow!("certs 不能为空（签名者 = certs[0]）"))?;
    let cms_err = |e: cms::builder::Error| anyhow!("cms builder: {:?}", e);

    let eci = EncapsulatedContentInfo {
        econtent_type: OID_SPC_INDIRECT_DATA,
        econtent: Some(Any::from_der(
            build_spc_indirect_data(content_digest)?.as_slice(),
        )?),
    };

    let mut sdb = SignedDataBuilder::new(&eci);
    sdb.add_digest_algorithm(sha256_algorithm()?)
        .map_err(cms_err)?;
    for c in certs {
        sdb.add_certificate(CertificateChoices::Certificate(c.parsed()?))
            .map_err(cms_err)?;
    }
    // Authenticode 要求 sid = IssuerAndSerialNumber（非 SubjectKeyIdentifier，S0-6 实证）
    let leaf_x509 = leaf.parsed()?;
    let sid = SignerIdentifier::IssuerAndSerialNumber(IssuerAndSerialNumber {
        issuer: leaf_x509.tbs_certificate.issuer.clone(),
        serial_number: leaf_x509.tbs_certificate.serial_number.clone(),
    });

    let mut sib = SignerInfoBuilder::new(sk, sid, sha256_algorithm()?, &eci, None)
        .map_err(|e| anyhow!("cms signer info builder: {:?}", e))?;
    // messageDigest / contentType 由 builder finalize 自动补齐；signingTime +
    // statementType + opus 手动加。statementType：signtool / osslsigncode 默认都带；
    // S3-2 对照实验实证缺失时 Windows 报 HashMismatch（同证书同钥匙，ossl builder
    // 产物通过、我方 builder 唯一结构性差异即此属性）。
    sib.add_signed_attribute(signing_time_attribute(signed_at)?)
        .map_err(cms_err)?;
    {
        // SpcStatementType ::= SEQUENCE { OID MS_INDIVIDUAL_CODE_SIGNING }
        //（值 = 单元素 SEQUENCE 包 OID，osslsigncode 产物逐字节同形态）
        let oid_b = OID_SPC_INDIVIDUAL_CODE_SIGNING.as_bytes();
        let mut stmt_der = vec![0x30, (oid_b.len() + 2) as u8, 0x06, oid_b.len() as u8];
        stmt_der.extend_from_slice(oid_b);
        let mut values = SetOfVec::new();
        values.insert(Any::from_der(&stmt_der)?)?;
        sib.add_signed_attribute(Attribute {
            oid: OID_SPC_STATEMENT_TYPE,
            values,
        })
        .map_err(cms_err)?;
    }
    if program_name.is_some() || more_info_url.is_some() {
        let pn = match program_name {
            Some(n) => Some(SpcString::Ascii(Ia5String::new(n)?)),
            None => None,
        };
        let mi = match more_info_url {
            Some(u) => Some(SpcLink::Url(Ia5String::new(u)?)),
            None => None,
        };
        let opus = SpcSpOpusInfo {
            program_name: pn,
            more_info: mi,
        };
        let mut values = SetOfVec::new();
        values.insert(Any::from_der(opus.to_der()?.as_slice())?)?;
        sib.add_signed_attribute(Attribute {
            oid: OID_SPC_SP_OPUS_INFO,
            values,
        })
        .map_err(cms_err)?;
    }
    // cms 的 SignatureBitStringEncoding 挂在 ecdsa::der::Signature（DER 包装类型）
    // 上，非根 ecdsa::Signature（S0-6 坑位 5b）
    sdb.add_signer_info::<SigningKey, ecdsa::der::Signature<p256::NistP256>>(sib)
        .map_err(cms_err)?;

    let content_info = sdb.build().map_err(cms_err)?;
    // Authenticode 钉 SignedData.version = V1：parse → set → re-encode（类型化库调用，
    // 非字节级手撸；version 不在签名覆盖面内，改写不破签名）
    let mut sd = SignedData::from_der(content_info.content.to_der()?.as_slice())?;
    sd.version = CmsVersion::V1;
    let ci = ContentInfo {
        content_type: content_info.content_type,
        content: Any::from_der(sd.to_der()?.as_slice())?,
    };
    Ok(ci.to_der()?)
}

// ---------------------------------------------------------------------------
// S3-3：嵌套签名层（追加多签名的微软真实形态：SPC_NESTED_SIGNATURE 属性）
// ---------------------------------------------------------------------------

/// 嵌套展开深度上限（signtool /as 实测一层；防御性上限防构造环）。
const MAX_NESTED_DEPTH: usize = 8;

/// S3-3：把 `nested_cms`（我方完整 ContentInfo DER）作为 **SPC_NESTED_SIGNATURE
/// 无认证属性**挂到 `host_cms`（他方 CMS）主 SignerInfo 的 unauthenticatedAttributes。
///
/// 返回变长后的完整 host ContentInfo DER。核心不变量（测试钉死）：
/// **主签名字节零变化**——unauthenticatedAttributes 不在签名覆盖面（RFC 5652
/// §5.4/§5.6：签名只盖 signedAttrs），host 的 signedAttrs/signature 原样保留，
/// 主签名验签结果不变。类型化 round-trip（parse → 改 → re-encode）对 DER
/// 规范形字节等价，唯一新增 = unauthAttrs 一条属性。
///
/// 诚实失败：host 非 signedData / signerInfos ≠ 1（多 SignerInfo host 形态未
/// 实证）/ host 主签名已含 SPC_NESTED_SIGNATURE（多层嵌套未实证，v4 一层）/
/// nested_cms 非合法 DER。
pub fn append_nested_signature(host_cms: &[u8], nested_cms: &[u8]) -> Result<Vec<u8>> {
    let ci = ContentInfo::from_der(host_cms).map_err(|e| anyhow!("host ContentInfo DER: {e}"))?;
    if ci.content_type != OID_SIGNED_DATA {
        anyhow::bail!("host contentType 非 signedData：不可挂嵌套签名");
    }
    let mut sd = SignedData::from_der(ci.content.to_der()?.as_slice())
        .map_err(|e| anyhow!("host SignedData DER: {e}"))?;
    if sd.signer_infos.0.len() != 1 {
        anyhow::bail!(
            "host signerInfos = {}（v4 只支持单 SignerInfo 主签名）",
            sd.signer_infos.0.len()
        );
    }
    let mut main: SignerInfo = sd.signer_infos.0.iter().next().expect("len==1").clone();
    if let Some(attrs) = &main.unsigned_attrs
        && attrs.iter().any(|a| a.oid == OID_SPC_NESTED_SIGNATURE)
    {
        anyhow::bail!("host 主签名已含 SPC_NESTED_SIGNATURE：多层嵌套 v4 不支持");
    }
    let nested_any =
        Any::from_der(nested_cms).map_err(|e| anyhow!("nested ContentInfo DER: {e}"))?;
    let attr = Attribute {
        oid: OID_SPC_NESTED_SIGNATURE,
        values: SetOfVec::try_from(vec![nested_any])?,
    };
    match main.unsigned_attrs {
        Some(ref mut attrs) => attrs.insert(attr)?,
        None => main.unsigned_attrs = Some(SetOfVec::try_from(vec![attr])?),
    }
    sd.signer_infos = SignerInfos(SetOfVec::try_from(vec![main])?);

    let new_sd_der = sd.to_der()?;
    Ok(ContentInfo {
        content_type: OID_SIGNED_DATA,
        content: Any::from_der(new_sd_der.as_slice())?,
    }
    .to_der()?)
}

/// S3-3：枚举一份 CMS 里的全部签名 = 主签名 + 递归展开 SPC_NESTED_SIGNATURE
/// 嵌套链（先序：signtool「Signature Index」同序——Index 0 = 主签名）。
///
/// S4-1 verify 管线的枚举原语（逐个签名验链/digest，按根锚定位自方）；
/// 单签名 CMS → `[cms]`。解析失败的嵌套体诚实报错（不静默吞）。
pub fn enumerate_signatures(cms: &[u8]) -> Result<Vec<Vec<u8>>> {
    enumerate_signatures_inner(cms, 0)
}

fn enumerate_signatures_inner(cms: &[u8], depth: usize) -> Result<Vec<Vec<u8>>> {
    if depth > MAX_NESTED_DEPTH {
        anyhow::bail!("嵌套签名深度超上限 {MAX_NESTED_DEPTH}");
    }
    let mut out = vec![cms.to_vec()];
    for nested in nested_signature_children(cms)? {
        out.extend(enumerate_signatures_inner(&nested, depth + 1)?);
    }
    Ok(out)
}

/// 取 host CMS 主 SignerInfo unauthenticatedAttributes 中的 SPC_NESTED_SIGNATURE
/// 属性值（完整嵌套 ContentInfo DER 列表；`Any` 保留原编码）。
fn nested_signature_children(cms: &[u8]) -> Result<Vec<Vec<u8>>> {
    let ci = ContentInfo::from_der(cms).map_err(|e| anyhow!("ContentInfo DER: {e}"))?;
    if ci.content_type != OID_SIGNED_DATA {
        anyhow::bail!("contentType 非 signedData：无嵌套签名可展开");
    }
    let sd = SignedData::from_der(ci.content.to_der()?.as_slice())
        .map_err(|e| anyhow!("SignedData DER: {e}"))?;
    let mut out = Vec::new();
    for signer in sd.signer_infos.0.iter() {
        let Some(attrs) = &signer.unsigned_attrs else {
            continue;
        };
        for attr in attrs.iter() {
            if attr.oid == OID_SPC_NESTED_SIGNATURE {
                for v in attr.values.iter() {
                    out.push(v.to_der()?);
                }
            }
        }
    }
    Ok(out)
}

/// S3-3：已签名文件策略入口（三态）——未签名首签 / 自方旧签名替换 / 他方签名
/// 嵌套共存。`our_cms` 须以**当前 `pe_bytes` 的 Authenticode digest** 构造
/// （S3-1 [`crate::pe::authenticode_digest`]；表/嵌套操作不动内容区 → 三态
/// 之下 digest 恒定）。`root_anchor_fp` = 我方根证书 SHA-256 指纹（定位自方）。
///
/// 判定（读证书表 [`crate::pe::read_certificate_entries`]）：
/// - 无条目 → 未签名 → [`crate::pe::append_certificate_table`] 首签；
/// - 全部条目皆自方（证书集含根锚）→ 剥表重签（[`crate::pe::replace_certificate_table`]，
///   旧自方签名整体消失）；
/// - 存在他方条目（且无自方条目）→ 主条目（首条目，Windows primary）挂我方
///   嵌套（[`append_nested_signature`]）+ 表重写——他方签名字节不变，我方签名
///   Windows「Signature Index」可见（signtool /as 同构形态）。
///
/// 边界（诚实失败）：自方+他方混合再重签（拆我方留他方）v4 不支持；主条目
/// 非 PKCS_SIGNED_DATA（无法挂嵌套）；多层嵌套（host 已含嵌套属性）。
pub fn resign_pe_file(
    pe_bytes: &[u8],
    our_cms: &[u8],
    root_anchor_fp: &[u8; 32],
) -> Result<Vec<u8>> {
    use crate::pe::{
        WIN_CERT_TYPE_PKCS_SIGNED_DATA, append_certificate_table, read_certificate_entries,
        replace_certificate_table,
    };
    let contains_anchor = |cms_der: &[u8]| -> bool {
        parse_signed_data(cms_der)
            .map(|p| {
                p.certs
                    .iter()
                    .any(|c| c.sha256_fingerprint() == *root_anchor_fp)
            })
            .unwrap_or(false)
    };
    let entries = read_certificate_entries(pe_bytes).map_err(|e| anyhow!("读取证书表: {e}"))?;
    if entries.is_empty() {
        return append_certificate_table(pe_bytes, our_cms).map_err(|e| anyhow!("首签: {e}"));
    }
    let all_own = entries.iter().all(|e| contains_anchor(&e.certificate));
    if all_own {
        return replace_certificate_table(pe_bytes, our_cms).map_err(|e| anyhow!("自方重签: {e}"));
    }
    let any_own = entries.iter().any(|e| contains_anchor(&e.certificate));
    if any_own {
        anyhow::bail!("自方+他方混合签名再重签（拆我方留他方）v4 不支持");
    }
    let host = entries.first().expect("entries 非空");
    if host.cert_type != WIN_CERT_TYPE_PKCS_SIGNED_DATA {
        anyhow::bail!(
            "主条目类型 {:#06x} 非 PKCS_SIGNED_DATA：无法挂嵌套签名",
            host.cert_type
        );
    }
    let nested = append_nested_signature(&host.certificate, our_cms)?;
    replace_certificate_table(pe_bytes, &nested).map_err(|e| anyhow!("嵌套后表重写: {e}"))
}

// ---------------------------------------------------------------------------
// S2-2：SignedData 解析（verify/view 的消费面；S4-1 接线到九态管线）
// ---------------------------------------------------------------------------

/// 结构坏（DER 形态 / OID 不符 / 自洽校验失败）——S4 映射 `VerifyOutcome::Malformed`。
fn malformed(msg: impl std::fmt::Display) -> anyhow::Error {
    anyhow!("malformed SignedData: {msg}")
}

/// 结构合法但本实现不接受的版本/套件——S4 映射 `VerifyOutcome::UnsupportedVersion`。
fn unsupported(msg: impl std::fmt::Display) -> anyhow::Error {
    anyhow!("unsupported SignedData: {msg}")
}

/// 解析后的 CMS 签名（结构 + 自洽信息；**不含任何信任判断**——链/EKU/根锚/吊销归 S4 管线）。
#[derive(Debug)]
pub struct ParsedSignature {
    /// SpcIndirectDataContent.message_digest.digest（S4 与重算的 byte-range digest 比对，
    /// 不等 → Tampered）。
    pub content_digest: [u8; 32],
    /// data.type（PE = SPC_PE_IMAGE_DATA）。
    pub content_type: ObjectIdentifier,
    /// 证书集（leaf 在前、含根——S0-4 约束；解析只要求非空可解析，含根检查归链验证）。
    pub certs: Vec<crate::cert::Certificate>,
    /// 签名者序列号（SerialNumber 原始整数字节，S4 与 leaf 证书比对定位签名者）。
    pub signer_serial: Vec<u8>,
    /// 签名者 issuer（Name DER，同上）。
    pub signer_issuer_der: Vec<u8>,
    /// 验签消息 = signedAttrs 的 SET OF（0x31）DER（RFC 5652 §5.4 口径，S2-1 实测）。
    pub signature_message: Vec<u8>,
    /// SignerInfo.signature（DER ECDSA 包装，S4 解为 `ecdsa::der::Signature` 后验签）。
    pub signature: Vec<u8>,
    /// messageDigest 属性值（解析期已核对 == SHA256(eContent 内容体)，自洽）。
    pub message_digest: [u8; 32],
    /// signingTime 属性（epoch 秒；缺失 = None，不做新鲜度判断）。
    pub signing_time: Option<u64>,
    /// SPC_SP_OPUS_INFO（view 展示；缺席 = None）。
    pub program_name: Option<String>,
    pub more_info: Option<String>,
}

/// 解析 Authenticode 形态 CMS SignedData（ContentInfo DER → [`ParsedSignature`]）。
///
/// 结构级诚实失败：
/// - 外层 DER 坏 / contentType 非 signedData / SignedData DER 坏 → Malformed
/// - version != V1（Authenticode 钉死）→ Unsupported
/// - eContentType 非 SPC_INDIRECT_DATA / eContent 缺失 / SPC 载荷坏 / 内容摘要
///   算法非 SHA-256 → Malformed 或 Unsupported
/// - 证书集缺失 / signerInfos 数 != 1 / sid 非 IssuerAndSerialNumber /
///   签名套件非 ecdsa-with-SHA256 / id-ecPublicKey（signtool ECDSA 怪癖）/
///   signedAttrs 缺失 / messageDigest 属性缺席
///   或与 eContent 内容体 SHA-256 不一致 / 签名非 DER ECDSA 形态 → Malformed
pub fn parse_signed_data(der: &[u8]) -> Result<ParsedSignature> {
    let ci = ContentInfo::from_der(der).map_err(|e| malformed(format!("ContentInfo DER: {e}")))?;
    if ci.content_type != OID_SIGNED_DATA {
        return Err(malformed(format!(
            "contentType {} 非 signedData",
            ci.content_type
        )));
    }
    let sd = SignedData::from_der(ci.content.to_der()?.as_slice())
        .map_err(|e| malformed(format!("SignedData DER: {e}")))?;
    if !matches!(sd.version, CmsVersion::V1) {
        return Err(unsupported(format!(
            "version {:?}（Authenticode 钉 V1）",
            sd.version
        )));
    }
    if sd.encap_content_info.econtent_type != OID_SPC_INDIRECT_DATA {
        return Err(malformed(format!(
            "eContentType {} 非 SPC_INDIRECT_DATA",
            sd.encap_content_info.econtent_type
        )));
    }
    let econtent = sd
        .encap_content_info
        .econtent
        .as_ref()
        .ok_or_else(|| malformed("eContent 缺失"))?;
    let econtent_der = econtent.to_der()?;
    let spc = SpcIndirectDataContent::from_der(econtent_der.as_slice())
        .map_err(|e| malformed(format!("SpcIndirectDataContent DER: {e}")))?;
    if spc.message_digest.digest_algorithm.oid != OID_SHA256 {
        return Err(unsupported("内容摘要算法非 SHA-256"));
    }
    let mut content_digest = [0u8; 32];
    if spc.message_digest.digest.as_bytes().len() != 32 {
        return Err(malformed("内容摘要长度非 32B"));
    }
    content_digest.copy_from_slice(spc.message_digest.digest.as_bytes());

    // 证书集：必须存在且逐张可解析（嵌入≠信任；含根约束在 S4 链验证层）
    let cert_set = sd
        .certificates
        .as_ref()
        .ok_or_else(|| malformed("证书集缺失"))?;
    let mut certs = Vec::with_capacity(cert_set.0.as_slice().len());
    for c in cert_set.0.iter() {
        match c {
            CertificateChoices::Certificate(x) => certs.push(
                crate::cert::Certificate::from_der(x.to_der()?.as_slice())
                    .map_err(|e| malformed(format!("证书 DER: {e}")))?,
            ),
            _ => return Err(malformed("非 Certificate 类型的证书项")),
        }
    }

    // signerInfos：恰一个；sid = IssuerAndSerialNumber（Authenticode 要求，S0-6 实证）
    let sis = sd.signer_infos.0.as_slice();
    if sis.len() != 1 {
        return Err(malformed(format!(
            "signerInfos 数 = {}（须恰 1）",
            sis.len()
        )));
    }
    let si = &sis[0];
    let (issuer, serial) = match &si.sid {
        SignerIdentifier::IssuerAndSerialNumber(iasn) => (&iasn.issuer, &iasn.serial_number),
        _ => {
            return Err(malformed(
                "sid 非 IssuerAndSerialNumber（Authenticode 要求）",
            ));
        }
    };
    // CMS SignerInfo 双算法字段：digest_alg = 纯哈希（SHA-256）；
    // signature_algorithm = 组合签名套件。接受两个 OID：
    // - ecdsa-with-SHA256（RFC 5758 标准，osslsigncode 所写）；
    // - id-ecPublicKey（signtool 对 ECDSA 证书的现实怪癖：复用证书 SPKI 的
    //   算法标识符，Windows 原样接受——S3-1 参照交叉验证实证）。
    // 曲线归属在 S4 验签时由签名者证书 SPKI 决定，此处不重复判断。
    if si.digest_alg.oid != OID_SHA256 {
        return Err(unsupported("SignerInfo 摘要算法非 SHA-256"));
    }
    if si.signature_algorithm.oid != crate::cert::OID_ECDSA_WITH_SHA256
        && si.signature_algorithm.oid != crate::cert::OID_EC_PUBLIC_KEY
    {
        return Err(unsupported(
            "签名套件非 ecdsa-with-SHA256 / id-ecPublicKey（D2 接受集）",
        ));
    }

    // signedAttrs：Authenticode 必有；messageDigest 必须与 eContent 内容体自洽
    let attrs = si
        .signed_attrs
        .as_ref()
        .ok_or_else(|| malformed("signedAttrs 缺失（Authenticode 必有）"))?;
    let mut message_digest: Option<[u8; 32]> = None;
    let mut signing_time = None;
    let mut program_name = None;
    let mut more_info = None;
    for a in attrs.iter() {
        let v = a
            .values
            .iter()
            .next()
            .ok_or_else(|| malformed(format!("属性 {} 空值集", a.oid)))?;
        let v_der = v.to_der()?;
        if a.oid == OID_ATTR_CONTENT_TYPE {
            let oid = ObjectIdentifier::from_der(v_der.as_slice())
                .map_err(|e| malformed(format!("contentType 属性值: {e}")))?;
            if oid != OID_SPC_INDIRECT_DATA {
                return Err(malformed("contentType 属性与 eContentType 不一致"));
            }
        } else if a.oid == OID_ATTR_MESSAGE_DIGEST {
            let oct = der::asn1::OctetString::from_der(v_der.as_slice())
                .map_err(|e| malformed(format!("messageDigest 属性值: {e}")))?;
            if oct.as_bytes().len() != 32 {
                return Err(malformed("messageDigest 属性长度非 32B"));
            }
            let mut md = [0u8; 32];
            md.copy_from_slice(oct.as_bytes());
            message_digest = Some(md);
        } else if a.oid == OID_ATTR_SIGNING_TIME {
            let t = UtcTime::from_der(v_der.as_slice())
                .map_err(|e| malformed(format!("signingTime 属性值: {e}")))?;
            signing_time = Some(t.to_unix_duration().as_secs());
        } else if a.oid == OID_SPC_SP_OPUS_INFO {
            let opus = SpcSpOpusInfo::from_der(v_der.as_slice())
                .map_err(|e| malformed(format!("SPC_SP_OPUS_INFO 属性值: {e}")))?;
            program_name = opus.program_name.map(spc_string_to_string);
            more_info = opus.more_info.and_then(spc_link_to_string);
        }
    }
    let message_digest = message_digest.ok_or_else(|| malformed("messageDigest 属性缺席"))?;
    // 自洽闸：messageDigest == SHA256(eContent 内容体)（MS 口径，S0-6 实测命中）
    {
        use sha2::Digest;
        let expect: [u8; 32] = sha2::Sha256::digest(econtent.value()).into();
        if message_digest != expect {
            return Err(malformed(
                "messageDigest 属性与 eContent 内容体 SHA-256 不一致",
            ));
        }
    }

    // 签名必须可按 DER ECDSA 解析（早期诚实失败；密码学验签归 S4）
    if <ecdsa::der::Signature<p256::NistP256>>::try_from(si.signature.as_bytes()).is_err() {
        return Err(malformed("signature 非 DER ECDSA 形态"));
    }

    Ok(ParsedSignature {
        content_digest,
        content_type: spc.data.type_,
        certs,
        signer_serial: serial.as_bytes().to_vec(),
        signer_issuer_der: issuer.to_der()?,
        signature_message: attrs.to_der()?,
        signature: si.signature.as_bytes().to_vec(),
        message_digest,
        signing_time,
        program_name,
        more_info,
    })
}

/// SpcString → String（ascii 直取；unicode = BMP UTF-16BE 解码）。
fn spc_string_to_string(s: SpcString) -> String {
    match s {
        SpcString::Ascii(a) => a.to_string(),
        SpcString::Unicode(b) => b.chars().collect(),
    }
}

/// SpcLink → String（url 直取；file 剥一层 SpcString）。
fn spc_link_to_string(l: SpcLink) -> Option<String> {
    match l {
        SpcLink::Url(u) => Some(u.to_string()),
        SpcLink::File(s) => Some(spc_string_to_string(s)),
    }
}

// ---------------------------------------------------------------------------
// S2-3：v4 locator footer（ELF/raw 载体；PE 走 Certificate Table 归 S3）
// ---------------------------------------------------------------------------

/// envelope 末尾 magic（8B，v4）。
pub const TRAILER_MAGIC_V4: [u8; 8] = *b"NMBSIG\x04\x00";
/// envelope 格式版本（v4）。
pub const FORMAT_VER_V4: u8 = 4;
/// v4 footer crc32 覆盖范围（footer\[0..36\]，即 magic..cms_len）。
pub const FOOTER_CRC_LEN_V4: usize = 36;
/// format_tag：ELF（footer 载体）。
pub const FORMAT_TAG_ELF: u8 = 2;
/// format_tag：Raw（footer 载体）。
pub const FORMAT_TAG_RAW: u8 = 3;

// v4 footer 字段偏移（64B 定长，与 v3 同长——从末尾扫描的定位机制语义一致）
const V4_OFF_CONTENT_LEN: usize = 16;
const V4_OFF_CMS_OFF: usize = 24;
const V4_OFF_CMS_LEN: usize = 32;
const V4_OFF_CRC: usize = 36;

/// 读 u64 LE 字段并收窄到平台 usize（32 位目标上诚实失败，非静默截断）。
fn rd_usz(b: &[u8], off: usize) -> Result<usize> {
    let v = rd_u64(b, off);
    usize::try_from(v).map_err(|_| anyhow!("footer 字段 {v} 超出平台 usize"))
}

/// 构造 v4 footer。
///
/// - `content_len`：被保护原始内容长度（文件 `[0, content_len)`）
/// - `cms_off` / `cms_len`：CMS ContentInfo DER 在文件内的绝对偏移与长度
pub fn build_footer_v4(
    format_tag: u8,
    content_len: usize,
    cms_off: usize,
    cms_len: usize,
) -> [u8; FOOTER_LEN] {
    // CMS DER 超 u32 上限（>4GB 签名块）为不可能形态，诚实 panic 而非静默截断
    let cms_len_u32 =
        u32::try_from(cms_len).expect("cms_len 超出 u32（CMS DER > 4GB，不可能形态）");
    let mut f = [0u8; FOOTER_LEN];
    f[OFF_MAGIC..OFF_MAGIC + 8].copy_from_slice(&TRAILER_MAGIC_V4);
    f[OFF_FORMAT_VER] = FORMAT_VER_V4;
    f[OFF_SIG_ALGO] = SIG_ALGO_ECDSA_P256;
    f[OFF_FORMAT_TAG] = format_tag;
    // OFF 11..16 reserved = 0
    f[V4_OFF_CONTENT_LEN..V4_OFF_CONTENT_LEN + 8]
        .copy_from_slice(&(content_len as u64).to_le_bytes());
    f[V4_OFF_CMS_OFF..V4_OFF_CMS_OFF + 8].copy_from_slice(&(cms_off as u64).to_le_bytes());
    f[V4_OFF_CMS_LEN..V4_OFF_CMS_LEN + 4].copy_from_slice(&cms_len_u32.to_le_bytes());
    let crc = crc32(&f[0..FOOTER_CRC_LEN_V4]);
    f[V4_OFF_CRC..V4_OFF_CRC + 4].copy_from_slice(&crc.to_le_bytes());
    f
}

/// 解析后的 v4 footer。
#[derive(Debug)]
pub struct ParsedFooterV4 {
    pub format_ver: u8,
    pub sig_algo: u8,
    pub format_tag: u8,
    pub content_len: usize,
    pub cms_off: usize,
    pub cms_len: usize,
}

/// 解析 v4 footer（校验 magic + crc32）。
pub fn parse_footer_v4(bytes: &[u8; FOOTER_LEN]) -> Result<ParsedFooterV4> {
    if bytes[OFF_MAGIC..OFF_MAGIC + 8] != TRAILER_MAGIC_V4 {
        return Err(anyhow!("footer magic mismatch（非 v4 footer）"));
    }
    let stored = rd_u32(bytes, V4_OFF_CRC);
    let calc = crc32(&bytes[0..FOOTER_CRC_LEN_V4]);
    if stored != calc {
        return Err(anyhow!("footer crc32 mismatch"));
    }
    Ok(ParsedFooterV4 {
        format_ver: bytes[OFF_FORMAT_VER],
        sig_algo: bytes[OFF_SIG_ALGO],
        format_tag: bytes[OFF_FORMAT_TAG],
        content_len: rd_usz(bytes, V4_OFF_CONTENT_LEN)?,
        cms_off: rd_usz(bytes, V4_OFF_CMS_OFF)?,
        // cms_len 布局是 u32（32..36，紧邻 36..40 的 CRC）——按 u32 读再无损加宽；
        // 误按 u64 读会把 CRC 拼进低位段
        cms_len: rd_u32(bytes, V4_OFF_CMS_LEN) as usize,
    })
}

/// 从文件末尾往前扫描，定位最近的 v4 footer（返回 footer 偏移）。
///
/// 只认 v4 magic；`overlay_start`：ELF 为 L（overlay 起点，之下不可见），Raw 传 0；
/// `excludes` 区间内的命中跳过（排除区——如 PE 证书表域）。
pub fn find_footer_v4(
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
        if !in_exclude && bytes.get(pos..pos + 8) == Some(&TRAILER_MAGIC_V4[..]) {
            return Some(pos);
        }
        if pos == 0 {
            break;
        }
        pos -= 1;
    }
    None
}

/// ELF/raw 载体装配：`[ 原内容 ][ CMS ContentInfo DER ][ v4 footer ]`。
///
/// PE 载体不用本函数（Certificate Table，S3-2）。`content_len` = 被保护内容
/// 长度，由调用方按载体契约给定——经 [`sign_carrier_v4`] 接线时：ELF =
/// codec L（overlay 不入保护域）、Raw = 全文件。
pub fn attach_v4(bytes: &[u8], cms_der: &[u8], format_tag: u8, content_len: usize) -> Vec<u8> {
    let cms_off = bytes.len();
    let footer = build_footer_v4(format_tag, content_len, cms_off, cms_der.len());
    let mut out = Vec::with_capacity(bytes.len() + cms_der.len() + FOOTER_LEN);
    out.extend_from_slice(bytes);
    out.extend_from_slice(cms_der);
    out.extend_from_slice(&footer);
    out
}

/// S3-4：ELF/raw 载体签名装配（codec 接线单一入口）。
///
/// 保护域由 codec 裁决：ELF = `[0, L)`（compute_l；overlay 不入保护域），
/// Raw = 全文件（compute_l = None）。format_tag 按魔数
/// 探测（[`crate::codec::detect_format`]），不接受调用方指定；PE 魔数输入
/// 诚实拒绝——PE 载体走 Certificate Table 路径（S3-2/S3-3），footer 形态
/// 不产双重载体。
///
/// CMS 的 content_digest 由调用方按同一保护域预计算（[`crate::pe::authenticode_digest`]
/// 对应 PE；此处对应 [`crate::codec::ExecutableCodec::content_hash`]），
/// 本函数只管载体布局与 footer 记账。
pub fn sign_carrier_v4(bytes: &[u8], cms_der: &[u8]) -> Result<Vec<u8>> {
    let tag = detect_format(bytes);
    if tag == FORMAT_TAG_PE {
        anyhow::bail!("PE 载体签名走 Certificate Table（S3-2/S3-3），不接受 v4 footer 形态");
    }
    let content_len = detect_codec(bytes).compute_l(bytes)?.unwrap_or(bytes.len());
    Ok(attach_v4(bytes, cms_der, tag, content_len))
}

/// S3-4：v4 载体验证侧提取结果（codec 接线闭环的字段面）。
#[derive(Debug)]
pub struct V4CarrierVerbatim {
    /// 定位到的 v4 footer 起始偏移。
    pub footer_offset: usize,
    /// footer 记录的被保护内容长度（ELF = L，Raw = 签名时全文件长）。
    pub content_len: usize,
    /// CMS ContentInfo DER（自载体提取，owned）。
    pub cms_der: Vec<u8>,
    /// 按载体契约重算的 `[0, content_len)` digest（codec 裁决规则）。
    pub content_digest: [u8; 32],
    /// 重算 digest == CMS 内嵌 content_digest（结构层闭环；密码学验签归 S4-1）。
    pub cms_digest_matches: bool,
}

/// S3-4：v4 载体验证侧提取（codec 接线）：定位 footer → 提取 → 重算 digest 对齐。
///
/// 交叉校验（诚实失败）：ELF 的 footer content_len 必须 == codec L（保护域 =
/// 结构区，违约 = 载体被外部改写或畸形）；content_len 超文件长由 content_hash
/// 的 Malformed 兜住。CMS 不可解析 → Err（结构损坏）；内容被篡改不 Err——
/// 以 `cms_digest_matches = false` 诚实报告（载体层可检出的完整性失败）。
pub fn extract_carrier_v4(bytes: &[u8]) -> Result<V4CarrierVerbatim> {
    let codec = detect_codec(bytes);
    let l = codec.compute_l(bytes)?;
    let overlay_start = l.unwrap_or(0); // Raw：全文件扫描
    let footer_offset = find_footer_v4(bytes, overlay_start, &codec.overlay_excludes(bytes))
        .ok_or_else(|| anyhow!("无 v4 footer（未签名或载体损坏）"))?;
    let (content_len, cms) = extract_v4(bytes, footer_offset)?;
    if let Some(l_actual) = l
        && content_len != l_actual
    {
        return Err(anyhow!(
            "footer content_len {} ≠ ELF 结构区 L {}（载体契约违约）",
            content_len,
            l_actual
        ));
    }
    let content_digest = codec.content_hash(bytes, content_len)?;
    let cms_digest_matches = parse_signed_data(cms)
        .map(|p| p.content_digest == content_digest)
        .unwrap_or(false);
    Ok(V4CarrierVerbatim {
        footer_offset,
        content_len,
        cms_der: cms.to_vec(),
        content_digest,
        cms_digest_matches,
    })
}

/// 定位校验后的提取：返回 `(content_len, CMS DER 切片)`。
///
/// crafted footer 越界防钳（BUG #22 家族纪律）：
/// cms 区间溢出 / 越过 footer / 空区间 / content_len 压到 CMS 区 → 诚实报错（非 panic）。
pub fn extract_v4(bytes: &[u8], footer_offset: usize) -> Result<(usize, &[u8])> {
    let fend = footer_offset
        .checked_add(FOOTER_LEN)
        .ok_or_else(|| anyhow!("crafted footer：footer 偏移溢出"))?;
    if fend > bytes.len() {
        return Err(anyhow!(
            "crafted footer：footer 偏移 {} 越界（文件 {}）",
            footer_offset,
            bytes.len()
        ));
    }
    let mut fb = [0u8; FOOTER_LEN];
    fb.copy_from_slice(&bytes[footer_offset..fend]);
    let p = parse_footer_v4(&fb)?;
    if p.cms_len == 0 {
        return Err(anyhow!("crafted footer：cms_len = 0"));
    }
    let cms_end = p
        .cms_off
        .checked_add(p.cms_len)
        .ok_or_else(|| anyhow!("crafted footer：cms 区间溢出"))?;
    if cms_end > footer_offset {
        return Err(anyhow!(
            "crafted footer：CMS 区间 [{}, {}) 越过 footer @{}",
            p.cms_off,
            cms_end,
            footer_offset
        ));
    }
    if p.content_len > p.cms_off {
        return Err(anyhow!(
            "crafted footer：content_len {} > cms_off {}（内容压到 CMS 区）",
            p.content_len,
            p.cms_off
        ));
    }
    Ok((p.content_len, &bytes[p.cms_off..cms_end]))
}

#[cfg(test)]
mod tests;
