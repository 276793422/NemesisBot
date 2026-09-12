//! v4 测试夹具（S4-1）：verify / c_abi / revocation 三处测试共用的真链 CMS 签名器。
//!
//! 仅测试构建参与编译（lib.rs `#[cfg(test)] mod fixtures;` 声明 + 本独立文件），
//! 不含任何 `#[test]`。信任锚 = 根证书 SHA-256 指纹（S4-2 口径，S0-5 实测：
//! 发行锚不可作信任锚，必须自签根）。

use crate::cert::Certificate;
use crate::envelope;
use crate::keygen::{KeyHierarchy, generate};
use p256::ecdsa::SigningKey;
use sha2::Digest;

/// 真三级密钥体系 + raw 载体 v4 签名助手。
pub struct V4Harness {
    /// keygen 真实三级体系（root / issuing / leaf，leaf 带 codeSigning EKU）。
    pub h: KeyHierarchy,
}

impl V4Harness {
    pub fn new() -> Self {
        Self {
            h: generate().expect("keygen generate"),
        }
    }

    /// 信任锚集合 = 自签根证书 SHA-256 指纹。
    pub fn anchor_fps(&self) -> Vec<[u8; 32]> {
        vec![self.h.root_anchor_fingerprint()]
    }

    /// 构造 Authenticode CMS（ContentInfo DER）：leaf 签名 + 全链证书集。
    /// `sk` / `certs` 可拆（SignatureInvalid 用错配 sk；Untrusted 用裁剪证书集）。
    pub fn build_cms(
        &self,
        content: &[u8],
        signed_at: u64,
        sk: &SigningKey,
        certs: &[Certificate],
    ) -> Vec<u8> {
        let digest: [u8; 32] = sha2::Sha256::digest(content).into();
        envelope::build_signed_data(&digest, sk, signed_at, certs, None, None)
            .expect("build_signed_data")
    }

    /// raw 载体签名（content 全文件 = 内容域）+ 定制 sk / 证书集。
    pub fn sign_raw_with(
        &self,
        content: &[u8],
        signed_at: u64,
        sk: &SigningKey,
        certs: &[Certificate],
    ) -> Vec<u8> {
        let cms = self.build_cms(content, signed_at, sk, certs);
        envelope::sign_carrier_v4(content, &cms).expect("sign_carrier_v4")
    }

    /// raw 载体签名（leaf 签全链，happy path）。
    pub fn sign_raw(&self, content: &[u8], signed_at: u64) -> Vec<u8> {
        let h = &self.h;
        self.sign_raw_with(content, signed_at, &h.leaf_sk, &h.chain())
    }

    /// 同 build_cms，但可带 opus（programName / moreInfo URL）——view publisher
    /// 展示与 c_abi 截断测试用。
    pub fn build_cms_opus(
        &self,
        content: &[u8],
        signed_at: u64,
        sk: &SigningKey,
        certs: &[Certificate],
        program_name: Option<&str>,
        more_info: Option<&str>,
    ) -> Vec<u8> {
        let digest: [u8; 32] = sha2::Sha256::digest(content).into();
        envelope::build_signed_data(&digest, sk, signed_at, certs, program_name, more_info)
            .expect("build_signed_data")
    }

    /// raw 载体签名 + opus（view publisher 穿透 / 截断测试用）。
    pub fn sign_raw_opus(
        &self,
        content: &[u8],
        signed_at: u64,
        sk: &SigningKey,
        certs: &[Certificate],
        program_name: Option<&str>,
        more_info: Option<&str>,
    ) -> Vec<u8> {
        let cms = self.build_cms_opus(content, signed_at, sk, certs, program_name, more_info);
        envelope::sign_carrier_v4(content, &cms).expect("sign_carrier_v4")
    }
}

/// 测试用「当前时间」= 真实墙钟（Unix 秒）。
///
/// verify 的 `now` 参数必须落在新签链的有效期内（keygen 在生成时刻回拨 1h 起，
/// leaf +3y 止）。固定魔数（如 1_900_000_000）会随墙钟推进撞上 Expired——
/// 本 helper 让「生成」与「验证」取同一时间基，永不腐坏。
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("post-1970 clock")
        .as_secs()
}
