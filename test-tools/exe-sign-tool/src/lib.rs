//! exe-sign-tool 核心库：可执行文件自验签名（PE / ELF / Raw）。
//!
//! 自定义 Ed25519 trailer 签名（非 Authenticode），用 ChaCha20-Poly1305 加密
//! envelope 抬高攻击成本。详见各模块文档与
//! `docs/PLAN/2026-07-18_exe-self-signature.md`。
//!
//! # 模块概览
//! - [`codec`]：格式抽象（trait + 三 codec + detect）
//! - [`pe`] / [`elf`]：PE / ELF 解析（多源综合算 L + content_hash）
//! - [`envelope`]：签名块字节结构、读写、定位
//! - [`crypto`]：ChaCha20-Poly1305 + Ed25519 原语
//! - [`api`]：sign / verify / 自检 主流程
//! - [`hex_util`]：hex 编解码

pub mod api;
pub mod cloud;
pub mod policy;
pub mod status;

// codec/envelope/crypto/hex_util 已移至 revoke-common 共用；re-export 保持 exe_sign_tool::* 可用
pub use revoke_common::{codec, crypto, envelope, hex_util};

pub use api::{load_signing_key, sign_executable, verify_current_exe, verify_executable};
pub use cloud::CloudClient;
pub use revoke_common::crypto::{generate_key_pair, get_sym_key, KeyPair};
pub use policy::RevocationPolicy;
pub use status::{Code, CloudState, SignatureStatus, Source};

#[cfg(test)]
mod tests;
