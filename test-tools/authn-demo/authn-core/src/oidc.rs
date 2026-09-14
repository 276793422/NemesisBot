//! OIDC 协议引擎（验证项 ①）：发现 → PKCE → 授权码流程 → 令牌校验 → claims → 角色提取。
//!
//! 设计：
//! - [`begin`] / [`complete`] 两段式 API——纯库形态，单元测试不用真浏览器。
//!   `complete` 内部重新做一次 discovery 重建 client（openidconnect 4.x 的
//!   typestate client 不跨函数存；多一次 HTTP 往返，换 API 干净 + 元数据新鲜）。
//! - 角色提取只吃**已经库内签名校验的 id_token**：验证通过后再解码同一份
//!   JWT 载荷读 `groups` / `realm_access.roles`（同一字节流二次解码，无未验证输入）。
//! - [`password_grant_token`] ROPC 脚本化路径——无浏览器确定性测试用
//!   （OAuth 已不推荐 ROPC，Keycloak 里对应 directAccessGrants，仅测试用途）。
//!
//! 浏览器回调 HTTP 服务器不在库内——库保持无 transport-server 依赖，服务器在 demo bin。
//!
//! 对接 Keycloak：issuer 形如 `http://localhost:8088/realms/demo`。

use std::time::Duration;

use base64::Engine;
use serde::Serialize;
use serde_json::Value;
use thiserror::Error;

use openidconnect::core::{
    CoreAuthenticationFlow, CoreClient, CoreIdTokenClaims, CoreProviderMetadata,
};
// openidconnect 有自己的 TokenResponse（提供 id_token()）；oauth2 的同名 trait
// 被重命名为 OAuth2TokenResponse（提供 access_token()）——两个都要在作用域。
use openidconnect::{
    AuthorizationCode, ClientId, ClientSecret, CsrfToken, IssuerUrl, Nonce, OAuth2TokenResponse,
    PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope, TokenResponse,
};

/// discovery 后的 client 精确 typestate 类型（openidconnect 4.x 把端点状态编码进
/// 类型系统；`CoreClient` 别名是全 EndpointNotSet 的"空"态，不适用于已 discovery 的实例）。
type DiscoveredClient = openidconnect::Client<
    openidconnect::EmptyAdditionalClaims,
    openidconnect::core::CoreAuthDisplay,
    openidconnect::core::CoreGenderClaim,
    openidconnect::core::CoreJweContentEncryptionAlgorithm,
    openidconnect::core::CoreJsonWebKey,
    openidconnect::core::CoreAuthPrompt,
    openidconnect::StandardErrorResponse<openidconnect::core::CoreErrorResponseType>,
    openidconnect::StandardTokenResponse<
        openidconnect::IdTokenFields<
            openidconnect::EmptyAdditionalClaims,
            openidconnect::EmptyExtraTokenFields,
            openidconnect::core::CoreGenderClaim,
            openidconnect::core::CoreJweContentEncryptionAlgorithm,
            openidconnect::core::CoreJwsSigningAlgorithm,
        >,
        openidconnect::core::CoreTokenType,
    >,
    openidconnect::StandardTokenIntrospectionResponse<
        openidconnect::EmptyExtraTokenFields,
        openidconnect::core::CoreTokenType,
    >,
    openidconnect::core::CoreRevocableToken,
    openidconnect::StandardErrorResponse<openidconnect::RevocationErrorResponseType>,
    openidconnect::EndpointSet,
    openidconnect::EndpointNotSet,
    openidconnect::EndpointNotSet,
    openidconnect::EndpointNotSet,
    openidconnect::EndpointMaybeSet,
    openidconnect::EndpointMaybeSet,
>;

use crate::identity::{AuthnSource, Identity};

#[derive(Debug, Error)]
pub enum OidcError {
    #[error("issuer URL 非法: {0}")]
    IssuerUrl(String),
    #[error("redirect URL 构造失败: {0}")]
    RedirectUrl(String),
    #[error("发现（discovery）失败——IdP 不可达或 .well-known/openid-configuration 异常: {0}")]
    Discovery(String),
    #[error("IdP 元数据缺 token endpoint")]
    NoTokenEndpoint,
    #[error("授权码换令牌失败: {0}")]
    TokenExchange(String),
    #[error("IdP 未返回 id_token")]
    NoIdToken,
    #[error("id_token 校验失败（签名/签发方/audience/nonce/有效期）: {0}")]
    ClaimsInvalid(String),
    #[error("CSRF state 不匹配——回调请求可能被伪造")]
    StateMismatch,
    #[error("授权回调缺少 code 参数")]
    MissingCode,
    #[error("JWT 载荷解码失败: {0}")]
    JwtDecode(String),
    #[error("HTTP 客户端构造失败: {0}")]
    HttpClient(String),
}

/// OIDC 客户端配置（builder 式——复用边界 #2，绝不读产品配置）。
#[derive(Debug, Clone)]
pub struct OidcConfig {
    /// 如 `http://localhost:8088/realms/demo`（尾部斜杠会自动去掉）
    pub issuer: String,
    pub client_id: String,
    /// 公共客户端（Keycloak public client）可不填
    pub client_secret: Option<String>,
    /// 本机回调端口（回调 URL = http://localhost:{port}/callback）
    pub redirect_port: u16,
    /// 额外 scope（`openid` 恒带；默认追加 `profile`）
    pub extra_scopes: Vec<String>,
}

impl OidcConfig {
    pub fn new(issuer: impl Into<String>, client_id: impl Into<String>) -> Self {
        Self {
            issuer: issuer.into(),
            client_id: client_id.into(),
            client_secret: None,
            redirect_port: 18081,
            extra_scopes: vec!["profile".to_string()],
        }
    }

    fn normalized_issuer(&self) -> String {
        self.issuer.trim_end_matches('/').to_string()
    }
}

/// 完整登录结果。`raw_claims` 是已验证 id_token 的载荷原文（demo 透明展示用）。
#[derive(Debug, Clone, Serialize)]
pub struct OidcLoginResult {
    pub identity: Identity,
    pub id_token: String,
    pub access_token: String,
    pub raw_claims: Value,
}

/// 两段式流程的中间态——全是可序列化的普通字符串（typestate client 不出 `begin` 作用域）。
/// 序列化设计让未来产品侧可把 pending flow 放进会话存储（而非进程内存）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OidcPendingFlow {
    /// 浏览器要打开的授权 URL
    pub auth_url: String,
    /// CSRF state——回调必须原样带回，`complete` 会先校验它
    pub csrf_state: String,
    nonce: String,
    pkce_verifier: String,
    issuer: String,
    client_id: String,
    client_secret: Option<String>,
    redirect_port: u16,
}

fn http_client() -> Result<reqwest::Client, OidcError> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| OidcError::HttpClient(e.to_string()))
}

/// discovery + 构造带 redirect_uri 的 client（typestate 全部留在本函数内推断）。
async fn discover_client(config: &OidcConfig) -> Result<DiscoveredClient, OidcError> {
    let issuer = IssuerUrl::new(config.normalized_issuer())
        .map_err(|e| OidcError::IssuerUrl(e.to_string()))?;
    let provider_meta = CoreProviderMetadata::discover_async(issuer, &http_client()?)
        .await
        .map_err(|e| OidcError::Discovery(e.to_string()))?;

    let client = CoreClient::from_provider_metadata(
        provider_meta,
        ClientId::new(config.client_id.clone()),
        config
            .client_secret
            .as_ref()
            .map(|s| ClientSecret::new(s.clone())),
    );
    let redirect = RedirectUrl::new(format!(
        "http://localhost:{}/callback",
        config.redirect_port
    ))
    .map_err(|e| OidcError::RedirectUrl(e.to_string()))?;
    Ok(client.set_redirect_uri(redirect))
}

/// 阶段一：discovery + 生成授权 URL（浏览器打开它）。
/// 返回的 [`OidcPendingFlow`] 交给回调方保管；回调带回 code+state 后调 [`complete`]。
pub async fn begin(config: &OidcConfig) -> Result<OidcPendingFlow, OidcError> {
    let client = discover_client(config).await?;

    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
    // authorize_url 已自动带 openid scope（use_openid_scope 默认开）；extra_scopes
    // 由配置方追加（`OidcConfig::new` 默认追加 profile）
    let (url, csrf, nonce) = config
        .extra_scopes
        .iter()
        .fold(
            client.authorize_url(
                CoreAuthenticationFlow::AuthorizationCode,
                CsrfToken::new_random,
                Nonce::new_random,
            ),
            |req, s| req.add_scope(Scope::new(s.clone())),
        )
        .set_pkce_challenge(pkce_challenge)
        .url();

    Ok(OidcPendingFlow {
        auth_url: url.to_string(),
        csrf_state: csrf.secret().to_string(),
        nonce: nonce.secret().to_string(),
        pkce_verifier: pkce_verifier.secret().to_string(),
        issuer: config.normalized_issuer(),
        client_id: config.client_id.clone(),
        client_secret: config.client_secret.clone(),
        redirect_port: config.redirect_port,
    })
}

/// 阶段二：拿回调带回的 code + state 换令牌并校验。
/// 先验 CSRF state，再走 PKCE 授权码交换，然后库内验证 id_token 签名
/// /issuer/audience/nonce/有效期，最后从已验证载荷提取身份与角色。
pub async fn complete(
    flow: OidcPendingFlow,
    code: &str,
    state: &str,
) -> Result<OidcLoginResult, OidcError> {
    if state != flow.csrf_state {
        return Err(OidcError::StateMismatch);
    }
    if code.is_empty() {
        return Err(OidcError::MissingCode);
    }

    let config = OidcConfig {
        issuer: flow.issuer.clone(),
        client_id: flow.client_id.clone(),
        client_secret: flow.client_secret.clone(),
        redirect_port: flow.redirect_port,
        extra_scopes: Vec::new(),
    };
    let client = discover_client(&config).await?;

    let token_response = client
        .exchange_code(AuthorizationCode::new(code.to_string()))
        .map_err(|e| OidcError::TokenExchange(e.to_string()))?
        .set_pkce_verifier(PkceCodeVerifier::new(flow.pkce_verifier.clone()))
        .request_async(&http_client()?)
        .await
        .map_err(|e| OidcError::TokenExchange(e.to_string()))?;

    let id_token = token_response.id_token().ok_or(OidcError::NoIdToken)?;
    // nonce 以 &Nonce 传入（openidconnect 4.x 的 NonceVerifier 形态）；
    // 签名/issuer/audience/有效期校验由 verifier 完成，返回已验证 claims 的引用。
    let claims = id_token
        .claims(&client.id_token_verifier(), &Nonce::new(flow.nonce.clone()))
        .map_err(|e| OidcError::ClaimsInvalid(e.to_string()))?;

    // 签名校验已通过——解码同一份 JWT 载荷读基础 claims（name / preferred_username）
    let id_token_str = id_token.to_string();
    let raw_claims = decode_jwt_payload(&id_token_str)?;

    // 角色提取合并 id_token + access_token 两个来源：Keycloak 默认把 realm 角色
    // 放进 access_token 的 realm_access 而 id_token 不带；其他 IdP（Auth0/Azure AD）
    // 惯例是 id_token 带 groups。access_token 也是 token endpoint 直连 TLS 响应
    // （可信渠道）；opaque 形态解码失败则跳过该来源，只认 id_token。
    let access_token = token_response.access_token().secret().to_string();
    let access_claims = decode_jwt_payload(&access_token).ok();
    let mut roles = extract_roles(&raw_claims);
    if let Some(ac) = &access_claims {
        roles.extend(extract_roles(ac));
    }
    roles.sort();
    roles.dedup();

    Ok(OidcLoginResult {
        identity: identity_from_verified_claims(&claims, &flow.issuer, &flow.client_id, roles),
        id_token: id_token_str,
        access_token,
        raw_claims,
    })
}

/// 从**已验证**的 id_token claims 构造通用身份（角色由调用方合并后传入）。
/// 展示名：name > preferred_username > subject。
fn identity_from_verified_claims(
    claims: &CoreIdTokenClaims,
    issuer: &str,
    client_id: &str,
    roles: Vec<String>,
) -> Identity {
    let subject = claims.subject().as_str().to_string();
    let display_name = claims
        .name()
        .and_then(|n| n.get(None))
        .map(|v| v.as_str().to_string())
        .or_else(|| claims.preferred_username().map(|p| p.as_str().to_string()))
        .unwrap_or_else(|| subject.clone());
    Identity {
        subject,
        display_name,
        roles,
        source: AuthnSource::Oidc {
            issuer: issuer.to_string(),
            client_id: client_id.to_string(),
        },
    }
}

/// ROPC（resource owner password credentials）脚本化路径。
/// **仅测试用**：Keycloak 的 directAccessGrants；无浏览器、确定性、可自动化。
/// token endpoint 从 discovery 拿（协议正确姿势，不硬编码 Keycloak 路径）。
/// 返回原始 token 响应 JSON（含 access_token / id_token 等）。
pub async fn password_grant_token(
    config: &OidcConfig,
    username: &str,
    password: &str,
) -> Result<Value, OidcError> {
    let http = http_client()?;
    let issuer = IssuerUrl::new(config.normalized_issuer())
        .map_err(|e| OidcError::IssuerUrl(e.to_string()))?;
    let provider_meta = CoreProviderMetadata::discover_async(issuer, &http)
        .await
        .map_err(|e| OidcError::Discovery(e.to_string()))?;
    let token_endpoint = provider_meta
        .token_endpoint()
        .ok_or(OidcError::NoTokenEndpoint)?
        .url()
        .to_string();

    let mut form = vec![
        ("grant_type", "password"),
        ("client_id", config.client_id.as_str()),
        ("username", username),
        ("password", password),
        ("scope", "openid profile"),
    ];
    if let Some(secret) = &config.client_secret {
        form.push(("client_secret", secret));
    }

    let resp = http
        .post(&token_endpoint)
        .form(&form)
        .send()
        .await
        .map_err(|e| OidcError::TokenExchange(e.to_string()))?;
    let status = resp.status();
    let body: Value = resp
        .json()
        .await
        .map_err(|e| OidcError::TokenExchange(format!("响应非 JSON: {e}")))?;
    if !status.is_success() {
        return Err(OidcError::TokenExchange(format!(
            "token endpoint {status}: {}",
            body.get("error_description")
                .and_then(|v| v.as_str())
                .unwrap_or("?")
        )));
    }
    Ok(body)
}

/// 解码 JWT 载荷（第二段，base64url 无填充）。**仅在签名校验通过后调用**。
pub fn decode_jwt_payload(jwt: &str) -> Result<Value, OidcError> {
    let seg = jwt
        .split('.')
        .nth(1)
        .ok_or_else(|| OidcError::JwtDecode("缺少 payload 段".into()))?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(seg)
        .map_err(|e| OidcError::JwtDecode(e.to_string()))?;
    serde_json::from_slice(&bytes).map_err(|e| OidcError::JwtDecode(e.to_string()))
}

/// 角色提取：`groups` 数组 + `realm_access.roles` 数组合并去重排序。
pub fn extract_roles(raw_claims: &Value) -> Vec<String> {
    let mut roles: Vec<String> = Vec::new();
    if let Some(groups) = raw_claims.get("groups").and_then(|v| v.as_array()) {
        roles.extend(groups.iter().filter_map(|g| g.as_str().map(str::to_string)));
    }
    if let Some(realm) = raw_claims
        .pointer("/realm_access/roles")
        .and_then(|v| v.as_array())
    {
        roles.extend(realm.iter().filter_map(|r| r.as_str().map(str::to_string)));
    }
    roles.sort();
    roles.dedup();
    roles
}

/// 展示名提取（demo/测试可见的纯函数）：name > preferred_username > fallback。
pub fn extract_display_name(raw_claims: &Value, fallback: &str) -> String {
    raw_claims
        .get("name")
        .and_then(|v| v.as_str())
        .or_else(|| {
            raw_claims
                .get("preferred_username")
                .and_then(|v| v.as_str())
        })
        .unwrap_or(fallback)
        .to_string()
}

#[cfg(test)]
mod tests;
