//! Swarm M3（§5.4/D6）：资产下载凭据——HMAC 签名 token + 引用束。
//!
//! web token 不跨节点（worker 没有 master 的 dashboard 凭据），资产下载的
//! 凭据随引用走：引用下发时签发方用**本节点密钥**签 HMAC-SHA256(ref,
//! expiry)，消费方拿 bundle 对提供方 gateway 的公开端点 `GET
//! /api/board/asset/{ref}` 直接拉取——零额外握手，密钥永不出节点
//! （签发者=提供者=验证者，天然对称，worker 产物反走同一条路）。
//!
//! 端点只验签名+过期，**不认 dashboard web token**；过期重拉=重新向
//! 提供方要一次引用（board.sync / 评论响应均可携带新 bundle）。

use chrono::Utc;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::sync::Arc;

/// 引用缺省有效期（§5.4「有效期如 1h」）；过期重拉即重新签发。
pub const DEFAULT_TOKEN_TTL_SECS: i64 = 3600;

type HmacSha256 = Hmac<Sha256>;

/// 资产引用束（dispatch payload / 评论内容里随 `asset_ref` 一起流转的
/// 下载凭据；serde 形状即信封/HTTP query 的单一真相源）。§5.4 层 1
/// 「引用 = {asset_ref, sha256, size}」+ 下载凭据四件套——消费方
/// （board_asset 工具）把字段逐字抄进 fetch 参数即可，sha256 是完整性
/// 校验的期望值，必须随引用走而不是靠截断展示。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetTokenBundle {
    /// 资产引用名（= asset 表 ref 列 = 提供方落盘文件名）。
    pub asset_ref: String,
    /// HMAC-SHA256 签名（hex，小写）。
    pub asset_token: String,
    /// 过期时刻（unix 秒；含）。
    pub expires_at: i64,
    /// 提供方 gateway 下载基址（如 `http://192.168.1.10:49100`）——
    /// 节点表只有 RPC 地址没有 HTTP 端口，bundle 自带完整寻址，
    /// 消费方拿来即用（反向对称：worker 产物 bundle 带自己地址）。
    pub node_url: String,
    /// 期望 sha256（hex 小写，64 字符）——下载后完整性校验的基准。
    pub sha256: String,
    /// 字节大小（登记值；供展示/预检，校验以 sha256 为准）。
    pub size: i64,
}

/// token 校验失败的两形态（区分「签名对但过期」与「签名不对」——
/// 过期是正常生命周期，篡改是异常事件，日志/测试都按此分叉）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetTokenError {
    /// 签名有效但已过 `expires_at`。
    Expired,
    /// 签名不匹配（密钥错 / ref 或 expiry 被改 / token 被篡改）。
    Invalid,
}

impl std::fmt::Display for AssetTokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AssetTokenError::Expired => write!(f, "asset token expired"),
            AssetTokenError::Invalid => write!(f, "asset token invalid"),
        }
    }
}

/// 签名内容：`ref\nexpires_at`。换行分隔消除拼接歧义（ref 经
/// [`sanitize_asset_ref`] 不含控制字符，但 `:` 等出现在 ref 里时
/// `\n` 分隔仍无二义）。
fn sign_message(ref_name: &str, expires_at: i64) -> String {
    format!("{ref_name}\n{expires_at}")
}

/// 计算资产 token（hex 小写）。secret 是签发节点的资产密钥（32 字节
/// 级熵；gateway 启动时加载/生成，见 nemesisbot 侧装配）。
pub fn sign_asset_token(secret: &[u8], ref_name: &str, expires_at: i64) -> String {
    let mut mac = HmacSha256::new_from_slice(secret).expect("hmac accepts any key length");
    mac.update(sign_message(ref_name, expires_at).as_bytes());
    hex_encode(&mac.finalize().into_bytes())
}

/// 校验资产 token：先常量时间比对签名（错 → [`AssetTokenError::Invalid`]），
/// 再查过期（对但过期 → [`AssetTokenError::Expired`]）——签名错的一律
/// Invalid，哪怕同时已过期（篡改优先于生命周期上报）。
pub fn verify_asset_token(
    secret: &[u8],
    ref_name: &str,
    expires_at: i64,
    token: &str,
) -> Result<(), AssetTokenError> {
    let expected = sign_asset_token(secret, ref_name, expires_at);
    let provided = token.trim();
    // 常量时间比较（nemesis-web workflow webhook 先例同款）。
    let expected_bytes = expected.as_bytes();
    let provided_bytes = provided.as_bytes();
    if expected_bytes.len() != provided_bytes.len() {
        return Err(AssetTokenError::Invalid);
    }
    let mut diff: u8 = 0;
    for (a, b) in expected_bytes.iter().zip(provided_bytes.iter()) {
        diff |= a ^ b;
    }
    if diff != 0 {
        return Err(AssetTokenError::Invalid);
    }
    if Utc::now().timestamp() > expires_at {
        return Err(AssetTokenError::Expired);
    }
    Ok(())
}

/// 签发一份引用束（dispatch attach / 评论贴引用共用的入口）。
#[allow(clippy::too_many_arguments)]
pub fn issue_asset_bundle(
    secret: &[u8],
    ref_name: &str,
    sha256: &str,
    size: i64,
    node_url: &str,
    ttl_secs: i64,
) -> AssetTokenBundle {
    let expires_at = Utc::now().timestamp() + ttl_secs;
    AssetTokenBundle {
        asset_ref: ref_name.to_string(),
        asset_token: sign_asset_token(secret, ref_name, expires_at),
        expires_at,
        node_url: node_url.trim_end_matches('/').to_string(),
        sha256: sha256.to_string(),
        size,
    }
}

/// 引用名合法性（路径穿越防御的单一真相源；注册登记与下载端点两侧
/// 都过这道）。白名单字符集：字母/数字/`. _ -`，且不得以 `.` 开头
/// （拦 `..`、隐藏文件）、不得为空、长度 ≤ 200。ref 落盘即文件名，
/// 含路径分隔符或 `..` 序列的引用一律拒绝。
pub fn sanitize_asset_ref(ref_name: &str) -> Result<(), String> {
    if ref_name.is_empty() {
        return Err("asset ref must not be empty".to_string());
    }
    if ref_name.len() > 200 {
        return Err("asset ref too long (max 200 bytes)".to_string());
    }
    if ref_name.starts_with('.') {
        return Err("asset ref must not start with '.'".to_string());
    }
    if !ref_name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err(
            "asset ref may only contain ASCII letters, digits, '.', '_' and '-'".to_string(),
        );
    }
    Ok(())
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 计算字节串 sha256（hex 小写）。下载端校验（board_asset 工具 fetch）与
/// 登记索引（publish）共用——asset 完整性的单一真相源。
pub fn sha256_bytes(data: &[u8]) -> String {
    use sha2::Digest;
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex_encode(&hasher.finalize())
}

/// 计算文件 sha256（hex 小写）；读不到/不是文件 → Err（诚实上抛）。
pub fn sha256_file(path: &std::path::Path) -> Result<String, String> {
    let data = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    Ok(sha256_bytes(&data))
}

fn hex_decode_32(hex: &str) -> Result<Vec<u8>, String> {
    let hex = hex.trim();
    if hex.len() != 64 {
        return Err(format!("expected 64 hex chars, got {}", hex.len()));
    }
    (0..32)
        .map(|i| {
            u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
                .map_err(|e| format!("bad hex at byte {i}: {e}"))
        })
        .collect()
}

/// 加载（或首次生成）本节点资产密钥。文件格式：64 hex chars = 32 字节
/// 熵；布局在 `<workspace>/config/asset_secret.key`（gateway 装配时调用，
/// 路径由调用方解析——crate 不依赖 nemesis-path）。
///
/// 损坏处理是**诚实 loud**：读到的内容不是合法 64 hex → Err，绝不静默
/// 重置（重置=作废所有已签发 token + 已登记引用，用户必须知道）。
pub fn load_or_create_secret(path: &std::path::Path) -> Result<Vec<u8>, String> {
    if let Ok(existing) = std::fs::read_to_string(path) {
        return hex_decode_32(&existing)
            .map_err(|e| format!("asset secret file {} corrupt: {e}", path.display()));
    }
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|e| format!("generate asset secret: {e}"))?;
    let hex = hex_encode(&bytes);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    std::fs::write(path, &hex).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(bytes.to_vec())
}

/// dispatch 侧资产签发上下文：secret + 本节点对外 web 基址槽。
/// gateway 装配时构造并 set 进 [`crate::store::BoardStore`]（dispatch 链
/// 全部函数已持有 store——资产段渲染零参数蔓延）。
#[derive(Clone)]
pub struct AssetSignContext {
    pub secret: Vec<u8>,
    /// 对外基址槽（如 `http://192.168.1.10:49100`）。未 set = web 地址
    /// 还没解析出来，签发诚实跳过该资产（不编 URL）。
    pub node_url: AdvertisedUrl,
}

/// 本节点对外 web 基址句柄（可更新）。G9（2026-09-09）：基址不再是一次
/// 性写死的 OnceLock——多重网卡机器的对外 NIC、以及 DHCP 换 IP，都由
/// gateway 的自愈任务随集群注册表知识更新（更新之后 bundle 重签即走
/// HTTP 老路）。
#[derive(Clone, Default)]
pub struct AdvertisedUrl(Arc<std::sync::RwLock<Option<String>>>);

impl AdvertisedUrl {
    pub fn set(&self, url: String) {
        *self.0.write().unwrap() = Some(url);
    }

    pub fn get(&self) -> Option<String> {
        self.0.read().unwrap().clone()
    }

    pub fn is_set(&self) -> bool {
        self.0.read().unwrap().is_some()
    }
}

impl AssetSignContext {
    /// 为单个引用签发 bundle；基址未就绪 → None（调用方诚实跳过）。
    pub fn sign_for(
        &self,
        ref_name: &str,
        sha256: &str,
        size: i64,
        ttl_secs: i64,
    ) -> Option<AssetTokenBundle> {
        let url = self.node_url.get()?;
        Some(issue_asset_bundle(
            &self.secret, ref_name, sha256, size, &url, ttl_secs,
        ))
    }

    /// 是否就绪（secret 恒有；基址已 set）。
    pub fn ready(&self) -> bool {
        self.node_url.is_set()
    }
}

/// 渲染 dispatch prompt 的「## 任务资产」段（§5.4 层 1：引用随包走）。
/// 无资产 → None（prompt 不加段）；基址未就绪的资产逐条诚实注记。
pub fn render_assets_section(
    signing: &AssetSignContext,
    assets: &[crate::models::BoardAsset],
    ttl_secs: i64,
) -> Option<String> {
    if assets.is_empty() {
        return None;
    }
    let mut lines = String::from(
        "\n## 任务资产\n\n\
         以下文件在本节点（提供方）可下载：HTTP GET 下述 url，凭据随引用走\
         （不认 dashboard 登录）；下载后按 sha256 校验。token 过期时向 master \
         重新索取引用即可（board.sync / 评论回复均可携带新引用）。\n",
    );
    for a in assets {
        match signing.sign_for(&a.ref_name, &a.sha256, a.size, ttl_secs) {
            Some(b) => {
                lines.push_str(&format!(
                    "- `{}`（{} 字节，sha256 见 bundle）\n  ```json\n  {}\n  ```\n",
                    a.ref_name,
                    a.size,
                    serde_json::to_string(&b).unwrap_or_default()
                ));
            }
            None => lines.push_str(&format!(
                "- `{}`（提供方下载地址未就绪，本轮未签发——需要时向 master 索取）\n",
                a.ref_name
            )),
        }
    }
    Some(lines)
}

#[cfg(test)]
mod tests;
