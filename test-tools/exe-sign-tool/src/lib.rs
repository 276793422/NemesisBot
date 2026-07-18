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
pub mod codec;
pub mod crypto;
pub mod elf;
pub mod envelope;
pub mod hex_util;
pub mod pe;

pub use api::{
    load_signing_key, sign_executable, verify_current_exe, verify_executable, VerifyOutcome,
};
pub use crypto::{generate_key_pair, get_sym_key, KeyPair};

#[cfg(test)]
mod tests;
