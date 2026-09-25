//! 密钥分包与类型化材料装载（CI v4 签名链路的密钥分层形态）。
//!
//! # 背景
//! 全量 keys.json 捆绑三级私钥（root/issuing/leaf），只适合本机密钥仪式。
//! CI 签名链路要求分层：根私钥离线、中间 CA 进 Secrets、叶现铸即弃——
//! 每个环节只该看到自己需要的材料。本模块按「哪些私钥真实在场」把 keys.json
//! v2 分为四种形态，并为每种消费场景提供**类型化材料结构**：
//!
//! | 形态 | 私钥在场 | 典型产出方 | 消费方 |
//! |---|---|---|---|
//! | [`BundleKind::Full`] | 三级俱全 | `keygen` | revoke-server（签发+吊销记账） |
//! | [`BundleKind::SignOnly`] | 仅 leaf | `mint-leaf`（CI 现铸） | `sign`（签名产物） |
//! | [`BundleKind::IssuingOnly`] | 仅 issuing | `split-keys` / CI Secrets 组装 | `mint-leaf`（铸叶） |
//! | [`BundleKind::RootOnly`] | 仅 root | `split-keys` | 离线冷存 / 轮换 issuing |
//!
//! 证书字段：sign / issuing 形态都携带全链三证书（leaf+issuing+root，签名时随
//! CMS 走）；root 形态只带根证书。空串字段 = 该私钥不在场。
//!
//! # 装载即校验
//! 每个类型化装载器（`SigningMaterial::load` / `IssuingMaterial::load` /
//! `RootMaterial::load`）都做私钥↔证书公钥匹配 + 链校验，坏包**装载即失败**
//! （不给下游拿坏材料签出废证书的机会）。`from_json` + `validate` 分离是为了
//! 测试可注入时钟。
//!
//! # 与 [`KeyHierarchy`](crate::keygen::KeyHierarchy) 的关系
//! 全量装载语义不变（严格三级俱全，空字段诚实拒绝）——revoke-server 等既有
//! 消费方零改动。分包是纯增量。

use crate::cert::{self, Certificate, TbsInput};
use crate::crypto;
use crate::hex_util::{hex_decode_vec, hex_encode};
use crate::keygen::{
    self, CN_ISSUING, KeyHierarchy, KeyHierarchyJson, NOT_BEFORE_BACKDATE_SECS, ORG, issue_x509,
};
use anyhow::{Result, anyhow, bail};
use p256::ecdsa::SigningKey;
use std::time::{SystemTime, UNIX_EPOCH};

/// keys.json 的分包形态（按「哪些私钥真实在场」分类；空串字段 = 不在场）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleKind {
    /// 三级私钥俱全（keygen 产物；revoke-server 启动要求）。
    Full,
    /// 仅 leaf 私钥在场（mint-leaf 产物；CI 签名包，job 结束即湮灭）。
    SignOnly,
    /// 仅中间 CA 私钥在场（split-keys 拆出 / CI Secrets 组装；铸叶材料）。
    IssuingOnly,
    /// 仅根私钥在场（split-keys 拆出；离线冷存）。
    RootOnly,
}

impl KeyHierarchyJson {
    /// 从文件装载 JSON 形态（不校验一致性——校验在类型化材料装载器）。
    pub fn load(path: &str) -> Result<Self> {
        let data = std::fs::read(path)?;
        serde_json::from_slice(&data).map_err(|e| anyhow!("keys 包解析失败（{path}）: {e}"))
    }

    /// 分包形态分类（私钥字段组合非法 → 诚实拒绝）。
    pub fn bundle_kind(&self) -> Result<BundleKind> {
        let (root, issuing, leaf) = (
            !self.root_sk.is_empty(),
            !self.issuing_sk.is_empty(),
            !self.leaf_sk.is_empty(),
        );
        match (root, issuing, leaf) {
            (true, true, true) => Ok(BundleKind::Full),
            (false, false, true) => Ok(BundleKind::SignOnly),
            (false, true, false) => Ok(BundleKind::IssuingOnly),
            (true, false, false) => Ok(BundleKind::RootOnly),
            _ => bail!(
                "keys.json 私钥字段组合非法: root_sk={root} issuing_sk={issuing} leaf_sk={leaf}\
——合法形态为全有（Full）/ 仅 leaf（SignOnly）/ 仅 issuing（IssuingOnly）/ 仅 root（RootOnly）"
            ),
        }
    }
}

/// 形态名（错误消息用；非法组合诚实呈现原错误）。
fn kind_str(j: &KeyHierarchyJson) -> String {
    j.bundle_kind()
        .map(|k| format!("{k:?}"))
        .unwrap_or_else(|e| format!("非法形态（{e}）"))
}

fn check_version(j: &KeyHierarchyJson) -> Result<()> {
    if j.version != keygen::KEYS_JSON_VERSION {
        bail!(
            "keys.json 版本不支持: {}（期望 {}）",
            j.version,
            keygen::KEYS_JSON_VERSION
        );
    }
    Ok(())
}

fn now_unix() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| anyhow!("系统时钟早于 UNIX_EPOCH: {e}"))?
        .as_secs())
}

/// 私钥 ↔ 证书公钥匹配检查（所有材料校验的公共项）。
fn check_sk_cert_match(sk: &SigningKey, cert: &Certificate, what: &str) -> Result<()> {
    let in_cert = cert
        .subject_public_key()
        .map_err(|e| anyhow!("{what}: 证书公钥解析失败: {e}"))?;
    if sk.verifying_key() != &in_cert {
        bail!("{what}: 私钥与证书公钥不匹配");
    }
    Ok(())
}

/// 证书有效期断言。
fn check_valid_at(c: &Certificate, now: u64, what: &str) -> Result<()> {
    if !c
        .is_valid_at(now)
        .map_err(|e| anyhow!("{what}: 有效期检查失败: {e}"))?
    {
        bail!("{what}: 证书不在有效期（now={now}）");
    }
    Ok(())
}

/// 含私钥材料的 JSON 落盘（unix 0600；Windows 沿用用户目录 ACL 默认收敛）。
fn save_private_json(j: &KeyHierarchyJson, path: &str) -> Result<()> {
    let json = serde_json::to_vec_pretty(j)?;
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(&json)?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, json)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 签名材料（SignOnly | Full）——`sign` 路径的全部输入
// ---------------------------------------------------------------------------

/// 签名材料：leaf 私钥 + 全链三证书。上游私钥不在场——签不了新证书（权限收敛）。
pub struct SigningMaterial {
    pub leaf_sk: SigningKey,
    pub leaf_cert: Certificate,
    pub issuing_cert: Certificate,
    pub root_cert: Certificate,
}

impl SigningMaterial {
    /// 装载并校验（时间基准 = 当前时钟）。
    pub fn load(path: &str) -> Result<Self> {
        let j = KeyHierarchyJson::load(path)?;
        let mat = Self::from_json(&j).map_err(|e| anyhow!("{path}: {e}"))?;
        mat.validate(now_unix()?)?;
        Ok(mat)
    }

    /// 从 JSON 组装（容忍 SignOnly / Full 形态；不做链校验——分离以便测试注入时钟）。
    pub fn from_json(j: &KeyHierarchyJson) -> Result<Self> {
        check_version(j)?;
        if j.leaf_sk.is_empty() {
            bail!("sign 材料要求 leaf_sk 在场（包形态 {}）", kind_str(j));
        }
        Ok(SigningMaterial {
            leaf_sk: crypto::signing_key_from_hex(&j.leaf_sk)?,
            leaf_cert: Self::cert_from(j, "leaf_cert")?,
            issuing_cert: Self::cert_from(j, "issuing_cert")?,
            root_cert: Self::cert_from(j, "root_cert")?,
        })
    }

    fn cert_from(j: &KeyHierarchyJson, field: &str) -> Result<Certificate> {
        let hex = match field {
            "leaf_cert" => &j.leaf_cert,
            "issuing_cert" => &j.issuing_cert,
            _ => &j.root_cert,
        };
        if hex.is_empty() {
            bail!(
                "{field}: 字段为空——sign 材料要求全链三证书（包形态 {}）",
                kind_str(j)
            );
        }
        let der = hex_decode_vec(hex).map_err(|e| anyhow!("{field}: {e}"))?;
        Certificate::from_der(&der).map_err(|e| anyhow!("{field}: {e}"))
    }

    /// 私钥↔证书匹配 + 全链校验（含 leaf codeSigning EKU 与有效期窗口）。
    pub fn validate(&self, now_unix: u64) -> Result<()> {
        check_sk_cert_match(&self.leaf_sk, &self.leaf_cert, "leaf")?;
        let anchor = self.root_anchor();
        cert::verify_chain(&self.chain(), &anchor, now_unix)
            .map_err(|e| anyhow!("leaf 链校验失败: {e}"))?;
        Ok(())
    }

    /// 签发链 `[leaf, issuing, root]`（sign_content_v4 的证书集入参）。
    pub fn chain(&self) -> Vec<Certificate> {
        vec![
            self.leaf_cert.clone(),
            self.issuing_cert.clone(),
            self.root_cert.clone(),
        ]
    }

    /// 信任锚 = SHA-256(根证书 DER)。
    pub fn root_anchor(&self) -> [u8; 32] {
        self.root_cert.sha256_fingerprint()
    }

    /// 序列化为 sign-only 包（root_sk / issuing_sk 空串 = 不在场）。
    pub fn to_json(&self) -> KeyHierarchyJson {
        KeyHierarchyJson {
            version: keygen::KEYS_JSON_VERSION,
            root_sk: String::new(),
            root_cert: hex_encode(self.root_cert.to_der()),
            issuing_sk: String::new(),
            issuing_cert: hex_encode(self.issuing_cert.to_der()),
            leaf_sk: hex_encode(self.leaf_sk.to_bytes().as_ref()),
            leaf_cert: hex_encode(self.leaf_cert.to_der()),
        }
    }

    /// 落盘（含 leaf 私钥——CI 侧落 runner 临时目录，0600）。
    pub fn save(&self, path: &str) -> Result<()> {
        save_private_json(&self.to_json(), path)
    }
}

// ---------------------------------------------------------------------------
// 中间 CA 材料（IssuingOnly | Full）——`mint-leaf` 的全部输入
// ---------------------------------------------------------------------------

/// 中间 CA 材料：issuing 私钥 + issuing/root 证书。能铸叶，不能签根（权限收敛）。
pub struct IssuingMaterial {
    pub issuing_sk: SigningKey,
    pub issuing_cert: Certificate,
    pub root_cert: Certificate,
}

impl IssuingMaterial {
    /// 装载并校验（时间基准 = 当前时钟）。
    pub fn load(path: &str) -> Result<Self> {
        let j = KeyHierarchyJson::load(path)?;
        let mat = Self::from_json(&j).map_err(|e| anyhow!("{path}: {e}"))?;
        mat.validate(now_unix()?)?;
        Ok(mat)
    }

    /// 从 JSON 组装（容忍 IssuingOnly / Full 形态；leaf 字段若有则忽略）。
    pub fn from_json(j: &KeyHierarchyJson) -> Result<Self> {
        check_version(j)?;
        if j.issuing_sk.is_empty() {
            bail!("issuing 材料要求 issuing_sk 在场（包形态 {}）", kind_str(j));
        }
        if j.issuing_cert.is_empty() || j.root_cert.is_empty() {
            bail!(
                "issuing 材料要求 issuing_cert + root_cert 在场（包形态 {}）",
                kind_str(j)
            );
        }
        let issuing_cert = Certificate::from_der(
            &hex_decode_vec(&j.issuing_cert).map_err(|e| anyhow!("issuing_cert: {e}"))?,
        )
        .map_err(|e| anyhow!("issuing_cert: {e}"))?;
        let root_cert = Certificate::from_der(
            &hex_decode_vec(&j.root_cert).map_err(|e| anyhow!("root_cert: {e}"))?,
        )
        .map_err(|e| anyhow!("root_cert: {e}"))?;
        Ok(IssuingMaterial {
            issuing_sk: crypto::signing_key_from_hex(&j.issuing_sk)?,
            issuing_cert,
            root_cert,
        })
    }

    /// issuing↔根 手动链校验（`cert::verify_chain` 硬性要求链头带 codeSigning EKU，
    /// 只适用于签名链；CA 材料在此做等价四项：匹配 + 有效期 + AKI/SKI 链接 + 签名）。
    pub fn validate(&self, now_unix: u64) -> Result<()> {
        check_sk_cert_match(&self.issuing_sk, &self.issuing_cert, "issuing")?;
        check_valid_at(&self.issuing_cert, now_unix, "issuing")?;
        check_valid_at(&self.root_cert, now_unix, "root")?;
        let issuing_aki = self
            .issuing_cert
            .aki()
            .map_err(|e| anyhow!("issuing AKI 解析失败: {e}"))?;
        let root_ski = self
            .root_cert
            .ski()
            .map_err(|e| anyhow!("根 SKI 解析失败: {e}"))?;
        if issuing_aki != root_ski {
            bail!("issuing 与根证书不链（issuing AKI ≠ 根 SKI）");
        }
        let root_vk = self
            .root_cert
            .subject_public_key()
            .map_err(|e| anyhow!("根证书公钥解析失败: {e}"))?;
        self.issuing_cert
            .verify_signature_by(&root_vk)
            .map_err(|e| anyhow!("issuing 证书签名非该根所签: {e}"))?;
        if !self
            .root_cert
            .is_self_signed()
            .map_err(|e| anyhow!("根自签检查失败: {e}"))?
        {
            bail!("根证书非自签形态");
        }
        Ok(())
    }

    /// 信任锚 = SHA-256(根证书 DER)。
    pub fn root_anchor(&self) -> [u8; 32] {
        self.root_cert.sha256_fingerprint()
    }

    /// 序列化为 IssuingOnly 包（root_sk / leaf 字段空串）。
    pub fn to_json(&self) -> KeyHierarchyJson {
        KeyHierarchyJson {
            version: keygen::KEYS_JSON_VERSION,
            root_sk: String::new(),
            root_cert: hex_encode(self.root_cert.to_der()),
            issuing_sk: hex_encode(self.issuing_sk.to_bytes().as_ref()),
            issuing_cert: hex_encode(self.issuing_cert.to_der()),
            leaf_sk: String::new(),
            leaf_cert: String::new(),
        }
    }

    /// 落盘（含中间 CA 私钥——进 Secrets 前的本机临时态，0600）。
    pub fn save(&self, path: &str) -> Result<()> {
        save_private_json(&self.to_json(), path)
    }
}

// ---------------------------------------------------------------------------
// 根材料（仅 RootOnly 严格形态）——离线冷存形态
// ---------------------------------------------------------------------------

/// 根材料：根私钥 + 根证书。信任根本体——泄漏即灾难，只应存在于离线介质。
pub struct RootMaterial {
    pub root_sk: SigningKey,
    pub root_cert: Certificate,
}

impl RootMaterial {
    /// 装载并校验（时间基准 = 当前时钟）。
    pub fn load(path: &str) -> Result<Self> {
        let j = KeyHierarchyJson::load(path)?;
        let mat = Self::from_json(&j).map_err(|e| anyhow!("{path}: {e}"))?;
        mat.validate()?;
        Ok(mat)
    }

    /// 从 JSON 组装（**严格 RootOnly 形态**——混入任何其他私钥/证书字段即拒绝：
    /// 根材料的保管纪律要求包内不出现多余密钥面）。
    pub fn from_json(j: &KeyHierarchyJson) -> Result<Self> {
        check_version(j)?;
        if !j.issuing_sk.is_empty()
            || !j.issuing_cert.is_empty()
            || !j.leaf_sk.is_empty()
            || !j.leaf_cert.is_empty()
        {
            bail!(
                "根材料应只含 root_sk + root_cert；检测到多余字段（包形态 {}）",
                kind_str(j)
            );
        }
        if j.root_sk.is_empty() {
            bail!("根材料要求 root_sk 在场");
        }
        if j.root_cert.is_empty() {
            bail!("根材料要求 root_cert 在场");
        }
        let root_cert = Certificate::from_der(
            &hex_decode_vec(&j.root_cert).map_err(|e| anyhow!("root_cert: {e}"))?,
        )
        .map_err(|e| anyhow!("root_cert: {e}"))?;
        Ok(RootMaterial {
            root_sk: crypto::signing_key_from_hex(&j.root_sk)?,
            root_cert,
        })
    }

    /// 私钥↔证书匹配 + 根自签形态。
    pub fn validate(&self) -> Result<()> {
        check_sk_cert_match(&self.root_sk, &self.root_cert, "root")?;
        if !self
            .root_cert
            .is_self_signed()
            .map_err(|e| anyhow!("根自签检查失败: {e}"))?
        {
            bail!("根证书非自签形态");
        }
        Ok(())
    }

    /// 信任锚 = SHA-256(根证书 DER)。
    pub fn root_anchor(&self) -> [u8; 32] {
        self.root_cert.sha256_fingerprint()
    }

    /// 序列化为 RootOnly 包。
    pub fn to_json(&self) -> KeyHierarchyJson {
        KeyHierarchyJson {
            version: keygen::KEYS_JSON_VERSION,
            root_sk: hex_encode(self.root_sk.to_bytes().as_ref()),
            root_cert: hex_encode(self.root_cert.to_der()),
            issuing_sk: String::new(),
            issuing_cert: String::new(),
            leaf_sk: String::new(),
            leaf_cert: String::new(),
        }
    }

    /// 落盘（含根私钥——信任根本体，0600；长期归宿是密码管理器/离线冷备）。
    pub fn save(&self, path: &str) -> Result<()> {
        save_private_json(&self.to_json(), path)
    }
}

// ---------------------------------------------------------------------------
// 分包操作与一致性校验
// ---------------------------------------------------------------------------

/// 全量包三级一致性校验（keygen 后 / revoke-server 启动 / split-keys 前调用——
/// 防止「生成/装载即坏」的密钥体系流到下游）。
pub fn validate_full_consistency(h: &KeyHierarchy, now_unix: u64) -> Result<()> {
    check_sk_cert_match(&h.root_sk, &h.root_cert, "root")?;
    check_sk_cert_match(&h.issuing_sk, &h.issuing_cert, "issuing")?;
    check_sk_cert_match(&h.leaf_sk, &h.leaf_cert, "leaf")?;
    cert::verify_chain(&h.chain(), &h.root_anchor_fingerprint(), now_unix)
        .map_err(|e| anyhow!("链校验失败: {e}"))?;
    Ok(())
}

/// 签名一致性校验（leaf ↔ 证书 + 全链）——`KeyHierarchy` 的 sign 前检查
/// （与 [`SigningMaterial::validate`] 同源逻辑，全量包形态的入口）。
pub fn validate_signing_consistency(h: &KeyHierarchy, now_unix: u64) -> Result<()> {
    check_sk_cert_match(&h.leaf_sk, &h.leaf_cert, "leaf")?;
    cert::verify_chain(&h.chain(), &h.root_anchor_fingerprint(), now_unix)
        .map_err(|e| anyhow!("leaf 链校验失败: {e}"))?;
    Ok(())
}

/// 拆分全量包 →（根离线材料, 中间 CA 材料）。
///
/// 前置：`h` 先过 [`validate_full_consistency`]（本函数内做，坏包拆不出来）。
/// 输入文件不删——私钥销毁由操作方显式执行，工具不替用户做。
pub fn split_keys(h: &KeyHierarchy, now_unix: u64) -> Result<(RootMaterial, IssuingMaterial)> {
    validate_full_consistency(h, now_unix)?;
    Ok((
        RootMaterial {
            root_sk: h.root_sk.clone(),
            root_cert: h.root_cert.clone(),
        },
        IssuingMaterial {
            issuing_sk: h.issuing_sk.clone(),
            issuing_cert: h.issuing_cert.clone(),
            root_cert: h.root_cert.clone(),
        },
    ))
}

/// 从中间 CA 材料现铸一枚叶代码签名证书 → 签名材料（CI 每次构建调用；叶即用即弃）。
///
/// 有效期 `days` 天（CI 用 365——v4 无 RFC 3161 时间戳，短叶会让旧 release 验签
/// 报 Expired）；`cn` 用构建标识；起点回拨 1h 容时钟偏差。铸后自检
/// （私钥↔证书匹配 + 全链 + EKU），坏铸诚实失败。
pub fn mint_leaf(
    issuing: &IssuingMaterial,
    days: u64,
    cn: &str,
    now_unix: u64,
) -> Result<SigningMaterial> {
    // 铸叶前校验 issuing 材料本身（防御性：绕过 load 直接构造的调用方也有闸）。
    issuing.validate(now_unix)?;
    let nb = now_unix.saturating_sub(NOT_BEFORE_BACKDATE_SECS);
    let leaf_sk = crypto::signing_key_from_hex(&crypto::generate_key_pair().private_key)?;
    let leaf_vk = leaf_sk.verifying_key();
    let leaf_cert = issue_x509(
        leaf_vk,
        &issuing.issuing_sk,
        &cert::ski_value(issuing.issuing_sk.verifying_key())?,
        TbsInput {
            subject_cn: cn,
            subject_org: Some(ORG),
            issuer_cn: CN_ISSUING,
            issuer_org: Some(ORG),
            is_ca: false,
            path_len: None,
            ku_digital_signature: true,
            ku_key_cert_sign: false,
            ku_crl_sign: false,
            eku_code_signing: true,
            not_before_unix: nb,
            not_after_unix: nb.saturating_add(days.saturating_mul(86400)),
        },
    )
    .map_err(|e| anyhow!("叶证书签发失败: {e}"))?;
    let mat = SigningMaterial {
        leaf_sk,
        leaf_cert,
        issuing_cert: issuing.issuing_cert.clone(),
        root_cert: issuing.root_cert.clone(),
    };
    mat.validate(now_unix)?; // 铸后自检：链 + EKU + 匹配
    Ok(mat)
}

/// 从任意含根证书的 keys 包提取信任锚（`verify --keys` 用）——不要求任何私钥在场。
pub fn load_root_anchor(path: &str) -> Result<[u8; 32]> {
    let j = KeyHierarchyJson::load(path)?;
    if j.root_cert.is_empty() {
        bail!("root_cert: 字段为空——锚提取要求包内带根证书（{path}）");
    }
    let der = hex_decode_vec(&j.root_cert).map_err(|e| anyhow!("root_cert: {e}"))?;
    let c = Certificate::from_der(&der).map_err(|e| anyhow!("root_cert: {e}"))?;
    Ok(c.sha256_fingerprint())
}

/// 从根证书 DER 文件算信任锚（`verify --root-cert` 用）——纯公开材料，零私钥验证入口。
pub fn root_anchor_from_der_file(path: &str) -> Result<[u8; 32]> {
    let der = std::fs::read(path).map_err(|e| anyhow!("根证书文件读取失败（{path}）: {e}"))?;
    let c = Certificate::from_der(&der).map_err(|e| anyhow!("根证书解析失败（{path}）: {e}"))?;
    Ok(c.sha256_fingerprint())
}

#[cfg(test)]
mod tests;
