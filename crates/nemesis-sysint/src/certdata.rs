//! 证书载荷解析（DER/PEM → DER）+ 指纹（sha256 hex）——证书库安装/卸载/
//! 查询按同一指纹对账的单一真相源。
//!
//! v1 诚实边界：只认 PEM `CERTIFICATE` 块与裸 DER（SEQUENCE 头启发）；
//! PKCS#12/多证书链包不支持（明确报错，不做静默截取）。

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

const PEM_BEGIN: &str = "-----BEGIN CERTIFICATE-----";
const PEM_END: &str = "-----END CERTIFICATE-----";

/// 裸 DER 启发判定：SEQUENCE 标签（0x30）+ 长度头形态可信。
pub fn is_der_certificate(bytes: &[u8]) -> bool {
    bytes.len() > 4 && bytes[0] == 0x30
}

/// 从 PEM 文本提取第一个证书块的 DER 体（base64 容忍换行/空白）。
pub fn extract_pem_block(text: &str) -> Option<Vec<u8>> {
    let start = text.find(PEM_BEGIN)? + PEM_BEGIN.len();
    let end = text.find(PEM_END)?;
    if end < start {
        return None;
    }
    let body: String = text[start..end]
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    use base64::Engine;
    let der = base64::engine::general_purpose::STANDARD
        .decode(body)
        .ok()?;
    if !is_der_certificate(&der) {
        return None;
    }
    Some(der)
}

/// 从文件字节提取证书 DER（PEM 含 BEGIN 标记优先按 PEM 解；否则按裸 DER；
/// 都不是 = 明确报错，不做静默截取）。
pub fn cert_der_from_bytes(bytes: &[u8]) -> Result<Vec<u8>> {
    if let Ok(text) = std::str::from_utf8(bytes)
        && text.contains(PEM_BEGIN)
        && let Some(der) = extract_pem_block(text)
    {
        return Ok(der);
    }
    if is_der_certificate(bytes) {
        return Ok(bytes.to_vec());
    }
    bail!("证书格式未识别（支持 PEM CERTIFICATE 块或裸 DER）")
}

/// 从证书文件提取 DER（带路径上下文的错误）。
pub fn cert_der_from_file(path: &std::path::Path) -> Result<Vec<u8>> {
    let bytes =
        std::fs::read(path).with_context(|| format!("读证书文件失败: {}", path.display()))?;
    cert_der_from_bytes(&bytes)
}

/// 证书指纹（sha256 hex 小写）——安装/卸载/查询三方对账键。
pub fn fingerprint(der: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(der);
    hex::encode(h.finalize())
}
