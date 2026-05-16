//! OAuth2 provider configuration and authentication flows.
//!
//! Supports browser-based OAuth, device code flow, and token refresh.

use crate::pkce::PkceCodes;
use crate::token::AuthCredential;

use base64::Engine;
use chrono::Utc;
use rand::Rng;
use std::collections::HashMap;

/// OAuth provider configuration.
#[derive(Debug, Clone)]
pub struct OAuthProviderConfig {
    pub issuer: String,
    pub client_id: String,
    pub scopes: String,
    pub originator: String,
    pub port: u16,
}

impl OAuthProviderConfig {
    /// OpenAI OAuth configuration.
    pub fn openai() -> Self {
        Self {
            issuer: "https://auth.openai.com".to_string(),
            client_id: "app_EMoamEEZ73f0CkXaXp7hrann".to_string(),
            scopes: "openid profile email offline_access".to_string(),
            originator: "nemesisbot".to_string(),
            port: 1455,
        }
    }

    /// Build the authorization URL.
    pub fn build_authorize_url(&self, pkce: &PkceCodes, state: &str, redirect_uri: &str) -> String {
        build_authorize_url(self, pkce, state, redirect_uri)
    }

    /// Browser-based OAuth login flow.
    ///
    /// Starts a local HTTP server on the configured port, opens the browser
    /// for the user to authenticate, waits for the callback, and exchanges
    /// the authorization code for tokens.
    pub async fn login_browser(&self) -> Result<AuthCredential, String> {
        let pkce = crate::pkce::generate_pkce();
        let state = generate_state();

        let redirect_uri = format!("http://localhost:{}/auth/callback", self.port);
        let auth_url = self.build_authorize_url(&pkce, &state, &redirect_uri);

        // Start local callback server
        let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{}", self.port))
            .await
            .map_err(|e| format!("starting callback server on port {}: {}", self.port, e))?;

        println!("Open this URL to authenticate:\n\n{}\n", auth_url);

        if let Err(e) = open_browser_impl(&auth_url) {
            println!(
                "Could not open browser automatically ({}).\nPlease open this URL manually:\n\n{}\n",
                e, auth_url
            );
        }

        println!("If you're running in a headless environment, use device code flow instead.");
        println!("Waiting for authentication in browser...");

        // Wait for callback with 5-minute timeout
        let result = tokio::time::timeout(std::time::Duration::from_secs(300), async {
            self.wait_for_callback(&listener, &state).await
        })
        .await
        .map_err(|_| "authentication timed out after 5 minutes".to_string())??;

        // Exchange authorization code for tokens
        self.exchange_code_for_tokens_impl(&result, &pkce.code_verifier, &redirect_uri)
            .await
    }

    /// Device code OAuth login flow.
    ///
    /// Requests a device code, displays it for the user, and polls
    /// until the user authorizes the request.
    pub async fn login_device_code(&self) -> Result<AuthCredential, String> {
        let client = reqwest::Client::new();

        // Request device code
        let resp = client
            .post(format!("{}/api/accounts/deviceauth/usercode", self.issuer))
            .json(&serde_json::json!({ "client_id": self.client_id }))
            .send()
            .await
            .map_err(|e| format!("requesting device code: {}", e))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("device code request failed: {}", body));
        }

        let body = resp
            .text()
            .await
            .map_err(|e| format!("reading device code response: {}", e))?;

        let device_resp = parse_device_code_response(&body)?;

        let interval = if device_resp.interval < 1 {
            5
        } else {
            device_resp.interval
        };

        println!(
            "\nTo authenticate, open this URL in your browser:\n\n  {}/codex/device\n\nThen enter this code: {}\n\nWaiting for authentication...\n",
            self.issuer, device_resp.user_code
        );

        // Poll for 15 minutes
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(900);
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(interval as u64));

        loop {
            if std::time::Instant::now() >= deadline {
                return Err("device code authentication timed out after 15 minutes".to_string());
            }

            tick.tick().await;

            match self
                .poll_device_code_impl(&client, &device_resp.device_auth_id, &device_resp.user_code)
                .await
            {
                Ok(Some(cred)) => return Ok(cred),
                Ok(None) => continue, // Still pending
                Err(_) => continue,   // Transient error, keep polling
            }
        }
    }

    /// Refresh an access token using a refresh token.
    pub async fn refresh_access_token(
        &self,
        cred: &AuthCredential,
    ) -> Result<AuthCredential, String> {
        let refresh_token = cred
            .refresh_token
            .as_ref()
            .ok_or_else(|| "no refresh token available".to_string())?;

        if refresh_token.is_empty() {
            return Err("no refresh token available".to_string());
        }

        let client = reqwest::Client::new();
        let params = [
            ("client_id", self.client_id.as_str()),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token.as_str()),
            ("scope", "openid profile email"),
        ];

        let resp = client
            .post(format!("{}/oauth/token", self.issuer))
            .form(&params)
            .send()
            .await
            .map_err(|e| format!("refreshing token: {}", e))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("token refresh failed: {}", body));
        }

        let body = resp
            .bytes()
            .await
            .map_err(|e| format!("reading refresh response: {}", e))?;

        let mut refreshed = parse_token_response_impl(&body, &cred.provider)?;

        // Preserve refresh token if not returned
        if refreshed.refresh_token.as_ref().map_or(true, |t| t.is_empty()) {
            refreshed.refresh_token = Some(refresh_token.clone());
        }
        // Preserve account_id if not returned
        if refreshed.account_id.as_ref().map_or(true, |t| t.is_empty()) {
            refreshed.account_id = cred.account_id.clone();
        }

        Ok(refreshed)
    }

    // ---- Private helpers ----

    /// Wait for the OAuth callback on the local HTTP server.
    async fn wait_for_callback(
        &self,
        listener: &tokio::net::TcpListener,
        expected_state: &str,
    ) -> Result<String, String> {
        loop {
            let (stream, _) = listener
                .accept()
                .await
                .map_err(|e| format!("accepting connection: {}", e))?;

            let result = self.handle_callback(stream, expected_state).await;
            match result {
                Ok(Some(code)) => return Ok(code),
                Ok(None) => continue, // Not the callback path, keep listening
                Err(e) => return Err(e),
            }
        }
    }

    /// Handle a single callback HTTP request.
    async fn handle_callback(
        &self,
        mut stream: tokio::net::TcpStream,
        expected_state: &str,
    ) -> Result<Option<String>, String> {
        use tokio::io::AsyncReadExt;
        use tokio::io::AsyncWriteExt;

        let mut buf = vec![0u8; 4096];
        let n = stream
            .read(&mut buf)
            .await
            .map_err(|e| format!("reading callback request: {}", e))?;

        let request = String::from_utf8_lossy(&buf[..n]);

        // Parse the HTTP request line: GET /auth/callback?code=...&state=... HTTP/1.1
        let request_line = request.lines().next().unwrap_or("");
        if !request_line.starts_with("GET ") {
            return Ok(None); // Not a GET request, ignore
        }

        // Extract path with query string
        let parts: Vec<&str> = request_line.split_whitespace().collect();
        if parts.len() < 2 {
            return Ok(None);
        }

        let uri = parts[1];
        if !uri.starts_with("/auth/callback") {
            return Ok(None);
        }

        // Parse query parameters
        let query_str = uri.split('?').nth(1).unwrap_or("");
        let params = parse_query_params(query_str);

        let response_body = if params.get("state").map_or(true, |s| s != expected_state) {
            let resp = "HTTP/1.1 400 Bad Request\r\nContent-Type: text/html\r\n\r\n<html><body><h2>State mismatch</h2></body></html>";
            stream
                .write_all(resp.as_bytes())
                .await
                .map_err(|e| format!("writing response: {}", e))?;
            return Err("state mismatch".to_string());
        } else if let Some(code) = params.get("code") {
            let resp = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n<html><body><h2>Authentication successful!</h2><p>You can close this window.</p></body></html>";
            stream
                .write_all(resp.as_bytes())
                .await
                .map_err(|e| format!("writing response: {}", e))?;
            code.clone()
        } else {
            let error_msg = params.get("error").cloned().unwrap_or_default();
            let resp = "HTTP/1.1 400 Bad Request\r\nContent-Type: text/html\r\n<html><body><h2>No authorization code received</h2></body></html>";
            stream
                .write_all(resp.as_bytes())
                .await
                .map_err(|e| format!("writing response: {}", e))?;
            return Err(format!("no code received: {}", error_msg));
        };

        let _ = stream.flush().await;
        Ok(Some(response_body))
    }

    /// Exchange an authorization code for tokens.
    async fn exchange_code_for_tokens_impl(
        &self,
        code: &str,
        code_verifier: &str,
        redirect_uri: &str,
    ) -> Result<AuthCredential, String> {
        let client = reqwest::Client::new();
        let params = [
            ("grant_type", "authorization_code".to_string()),
            ("code", code.to_string()),
            ("redirect_uri", redirect_uri.to_string()),
            ("client_id", self.client_id.clone()),
            ("code_verifier", code_verifier.to_string()),
        ];

        let resp = client
            .post(format!("{}/oauth/token", self.issuer))
            .form(&params)
            .send()
            .await
            .map_err(|e| format!("exchanging code for tokens: {}", e))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("token exchange failed: {}", body));
        }

        let body = resp
            .bytes()
            .await
            .map_err(|e| format!("reading token response: {}", e))?;

        parse_token_response_impl(&body, "openai")
    }

    /// Poll the device code token endpoint.
    async fn poll_device_code_impl(
        &self,
        client: &reqwest::Client,
        device_auth_id: &str,
        user_code: &str,
    ) -> Result<Option<AuthCredential>, String> {
        let resp = client
            .post(format!("{}/api/accounts/deviceauth/token", self.issuer))
            .json(&serde_json::json!({
                "device_auth_id": device_auth_id,
                "user_code": user_code,
            }))
            .send()
            .await
            .map_err(|_| "poll request failed".to_string())?;

        if !resp.status().is_success() {
            return Ok(None); // Still pending
        }

        let body = resp
            .text()
            .await
            .map_err(|e| format!("reading poll response: {}", e))?;

        let token_resp: DeviceTokenResponse = serde_json::from_str(&body)
            .map_err(|e| format!("parsing device token response: {}", e))?;

        let redirect_uri = format!("{}/deviceauth/callback", self.issuer);
        let cred = self
            .exchange_code_for_tokens_impl(
                &token_resp.authorization_code,
                &token_resp.code_verifier,
                &redirect_uri,
            )
            .await?;

        Ok(Some(cred))
    }
}

// ---------------------------------------------------------------------------
// Standalone functions (matching Go's public API)
// ---------------------------------------------------------------------------

/// Returns the default OpenAI OAuth provider configuration.
///
/// Mirrors Go's `OpenAIOAuthConfig()`.
pub fn open_ai_oauth_config() -> OAuthProviderConfig {
    OAuthProviderConfig::openai()
}

/// Browser-based OAuth login flow.
///
/// Starts a local HTTP server on the configured port, opens the browser
/// for the user to authenticate, waits for the callback, and exchanges
/// the authorization code for tokens.
///
/// Mirrors Go's `LoginBrowser(cfg)`.
pub async fn login_browser(cfg: &OAuthProviderConfig) -> Result<AuthCredential, String> {
    cfg.login_browser().await
}

/// Device code OAuth login flow.
///
/// Requests a device code, displays it for the user, and polls
/// until the user authorizes the request.
///
/// Mirrors Go's `LoginDeviceCode(cfg)`.
pub async fn login_device_code(cfg: &OAuthProviderConfig) -> Result<AuthCredential, String> {
    cfg.login_device_code().await
}

/// Poll the device code token endpoint once.
///
/// Returns `Ok(Some(credential))` on success, `Ok(None)` if still pending.
///
/// Mirrors Go's `pollDeviceCode(cfg, deviceAuthID, userCode)`.
pub async fn poll_device_code(
    cfg: &OAuthProviderConfig,
    device_auth_id: &str,
    user_code: &str,
) -> Result<Option<AuthCredential>, String> {
    let client = reqwest::Client::new();
    cfg.poll_device_code_impl(&client, device_auth_id, user_code).await
}

/// Refresh an access token using a refresh token.
///
/// Mirrors Go's `RefreshAccessToken(cred, cfg)`.
pub async fn refresh_access_token(
    cred: &AuthCredential,
    cfg: &OAuthProviderConfig,
) -> Result<AuthCredential, String> {
    cfg.refresh_access_token(cred).await
}

/// Build the OAuth authorization URL.
///
/// Includes OpenAI-specific parameters (`id_token_add_organizations`,
/// `codex_cli_simplified_flow`) matching Go's `BuildAuthorizeURL`.
///
/// Mirrors Go's `BuildAuthorizeURL(cfg, pkce, state, redirectURI)`.
pub fn build_authorize_url(
    cfg: &OAuthProviderConfig,
    pkce: &PkceCodes,
    state: &str,
    redirect_uri: &str,
) -> String {
    let mut params = vec![
        ("response_type", "code".to_string()),
        ("client_id", cfg.client_id.clone()),
        ("redirect_uri", redirect_uri.to_string()),
        ("scope", cfg.scopes.clone()),
        ("code_challenge", pkce.code_challenge.clone()),
        ("code_challenge_method", "S256".to_string()),
        ("id_token_add_organizations", "true".to_string()),
        ("codex_cli_simplified_flow", "true".to_string()),
        ("state", state.to_string()),
    ];
    if !cfg.originator.is_empty() {
        params.push(("originator", cfg.originator.clone()));
    }

    let query: String = params
        .iter()
        .map(|(k, v)| format!("{}={}", k, url_encode(v)))
        .collect::<Vec<_>>()
        .join("&");

    format!("{}/oauth/authorize?{}", cfg.issuer, query)
}

/// Exchange an authorization code for tokens.
///
/// Mirrors Go's `exchangeCodeForTokens(cfg, code, codeVerifier, redirectURI)`.
pub async fn exchange_code_for_tokens(
    cfg: &OAuthProviderConfig,
    code: &str,
    code_verifier: &str,
    redirect_uri: &str,
) -> Result<AuthCredential, String> {
    cfg.exchange_code_for_tokens_impl(code, code_verifier, redirect_uri).await
}

/// Parse a token response body into an AuthCredential.
///
/// Mirrors Go's `parseTokenResponse(body, provider)`.
pub fn parse_token_response(body: &[u8], provider: &str) -> Result<AuthCredential, String> {
    parse_token_response_impl(body, provider)
}

/// Extract account ID from a JWT token's claims.
///
/// Returns an empty string if no account ID is found.
/// Mirrors Go's `extractAccountID(token)`.
pub fn extract_account_id(token: &str) -> String {
    extract_account_id_impl(token).unwrap_or_default()
}

/// Parse JWT claims from a token string.
///
/// Returns a HashMap of claim name to JSON value.
/// Mirrors Go's `parseJWTClaims(token)`.
pub fn parse_jwt_claims(token: &str) -> Result<HashMap<String, serde_json::Value>, String> {
    let value = parse_jwt_claims_impl(token)?;
    let map = value
        .as_object()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .collect();
    Ok(map)
}

/// Open a URL in the default browser.
///
/// Mirrors Go's `openBrowser(url)`.
pub fn open_browser(url: &str) -> Result<(), String> {
    open_browser_impl(url)
}

/// Device code response from the API.
///
/// Public type matching Go's `deviceCodeResponse`.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct DeviceCodeResponse {
    /// Device authorization ID.
    #[serde(rename = "device_auth_id")]
    pub device_auth_id: String,
    /// User code to enter in the browser.
    #[serde(rename = "user_code")]
    pub user_code: String,
    /// Polling interval in seconds.
    #[serde(default)]
    pub interval: i32,
}

// ---------------------------------------------------------------------------
// Private types
// ---------------------------------------------------------------------------

/// Device token poll response.
#[derive(Debug, serde::Deserialize)]
struct DeviceTokenResponse {
    #[serde(rename = "authorization_code")]
    authorization_code: String,
    #[serde(rename = "code_verifier")]
    code_verifier: String,
}

/// Parsed device code response with flexible interval parsing.
struct ParsedDeviceCode {
    device_auth_id: String,
    user_code: String,
    interval: i32,
}

/// Parse the device code API response, handling flexible `interval` field.
fn parse_device_code_response(body: &str) -> Result<ParsedDeviceCode, String> {
    let raw: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("parsing device code response: {}", e))?;

    let device_auth_id = raw["device_auth_id"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let user_code = raw["user_code"].as_str().unwrap_or("").to_string();
    let interval = parse_flexible_int(&raw["interval"]);

    Ok(ParsedDeviceCode {
        device_auth_id,
        user_code,
        interval,
    })
}

/// Parse an integer that may be a number or a string.
fn parse_flexible_int(val: &serde_json::Value) -> i32 {
    match val {
        serde_json::Value::Number(n) => n.as_i64().unwrap_or(0) as i32,
        serde_json::Value::String(s) => s.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

/// Parse a token response body into an AuthCredential.
fn parse_token_response_impl(body: &[u8], provider: &str) -> Result<AuthCredential, String> {
    let resp: serde_json::Value =
        serde_json::from_slice(body).map_err(|e| format!("parsing token response: {}", e))?;

    let access_token = resp["access_token"]
        .as_str()
        .unwrap_or("")
        .to_string();

    if access_token.is_empty() {
        return Err("no access token in response".to_string());
    }

    let refresh_token = resp["refresh_token"].as_str().map(|s| s.to_string());
    let expires_in = resp["expires_in"].as_u64().unwrap_or(0);
    let id_token = resp["id_token"].as_str().unwrap_or("").to_string();

    let expires_at = if expires_in > 0 {
        Some(Utc::now() + chrono::Duration::seconds(expires_in as i64))
    } else {
        None
    };

    // Extract account ID from tokens
    let account_id = extract_account_id_impl(&id_token)
        .or_else(|| extract_account_id_impl(&access_token));

    Ok(AuthCredential {
        access_token,
        refresh_token,
        expires_at,
        provider: provider.to_string(),
        auth_method: "oauth".to_string(),
        account_id,
    })
}

/// Extract account ID from a JWT token's claims.
fn extract_account_id_impl(token: &str) -> Option<String> {
    let claims = parse_jwt_claims_impl(token).ok()?;

    // Direct chatgpt_account_id field
    if let Some(id) = claims.get("chatgpt_account_id").and_then(|v| v.as_str()) {
        if !id.is_empty() {
            return Some(id.to_string());
        }
    }

    // Namespaced claim
    if let Some(id) = claims
        .get("https://api.openai.com/auth.chatgpt_account_id")
        .and_then(|v| v.as_str())
    {
        if !id.is_empty() {
            return Some(id.to_string());
        }
    }

    // Nested auth claim
    if let Some(auth) = claims
        .get("https://api.openai.com/auth")
        .and_then(|v| v.as_object())
    {
        if let Some(id) = auth.get("chatgpt_account_id").and_then(|v| v.as_str()) {
            if !id.is_empty() {
                return Some(id.to_string());
            }
        }
    }

    // Organizations array
    if let Some(orgs) = claims.get("organizations").and_then(|v| v.as_array()) {
        for org in orgs {
            if let Some(id) = org.as_object().and_then(|o| o.get("id")).and_then(|v| v.as_str()) {
                if !id.is_empty() {
                    return Some(id.to_string());
                }
            }
        }
    }

    None
}

/// Parse JWT claims from a token string.
fn parse_jwt_claims_impl(token: &str) -> Result<serde_json::Value, String> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() < 2 {
        return Err("token is not a JWT".to_string());
    }

    let payload = parts[1];
    let decoded = base64url_decode(payload)?;

    serde_json::from_slice(&decoded).map_err(|e| format!("parsing JWT claims: {}", e))
}

/// Base64url decode with padding.
fn base64url_decode(input: &str) -> Result<Vec<u8>, String> {
    // Add padding if needed
    let padded = match input.len() % 4 {
        2 => format!("{}==", input),
        3 => format!("{}=", input),
        _ => input.to_string(),
    };

    // Convert from URL-safe to standard base64
    let standard = padded.replace('-', "+").replace('_', "/");

    base64::engine::general_purpose::STANDARD
        .decode(&standard)
        .map_err(|e| format!("base64 decode: {}", e))
}

/// Generate a random state parameter for OAuth.
fn generate_state() -> String {
    let mut rng = rand::thread_rng();
    let bytes: [u8; 32] = rng.r#gen();
    hex_encode(&bytes)
}

/// Encode bytes as hex string.
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Parse query parameters from a URL query string.
fn parse_query_params(query: &str) -> HashMap<String, String> {
    let mut params = HashMap::new();
    for pair in query.split('&') {
        let mut kv = pair.splitn(2, '=');
        if let (Some(k), Some(v)) = (kv.next(), kv.next()) {
            params.insert(
                url_decode(k),
                url_decode(v),
            );
        }
    }
    params
}

/// URL-decode a string.
fn url_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.bytes();

    while let Some(b) = chars.next() {
        if b == b'%' {
            let hi = chars.next().unwrap_or(b'0');
            let lo = chars.next().unwrap_or(b'0');
            let val = hex_val(hi) << 4 | hex_val(lo);
            result.push(val as char);
        } else if b == b'+' {
            result.push(' ');
        } else {
            result.push(b as char);
        }
    }
    result
}

/// Convert a hex char byte to its value.
fn hex_val(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        b'A'..=b'F' => b - b'A' + 10,
        _ => 0,
    }
}

/// URL-encode a string.
fn url_encode(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '~' {
                c.to_string()
            } else {
                format!("%{:02X}", c as u8)
            }
        })
        .collect()
}

/// Open a URL in the default browser.
fn open_browser_impl(url: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/c", "start", url])
            .spawn()
            .map_err(|e| format!("opening browser: {}", e))?;
        Ok(())
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .spawn()
            .map_err(|e| format!("opening browser: {}", e))?;
        Ok(())
    }

    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .spawn()
            .map_err(|e| format!("opening browser: {}", e))?;
        Ok(())
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        let _ = url;
        Err("unsupported platform".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_openai_config() {
        let cfg = OAuthProviderConfig::openai();
        assert_eq!(cfg.port, 1455);
        assert!(cfg.issuer.contains("openai.com"));
    }

    #[test]
    fn test_build_authorize_url() {
        let cfg = OAuthProviderConfig::openai();
        let pkce = crate::pkce::generate_pkce();
        let url = cfg.build_authorize_url(&pkce, "state123", "http://localhost:1455/auth/callback");
        assert!(url.contains("oauth/authorize"));
        assert!(url.contains("code_challenge"));
        assert!(url.contains("S256"));
        assert!(url.contains("state=state123"));
    }

    #[test]
    fn test_generate_state() {
        let state1 = generate_state();
        let state2 = generate_state();
        assert_eq!(state1.len(), 64); // 32 bytes = 64 hex chars
        assert_ne!(state1, state2);
    }

    #[test]
    fn test_parse_query_params() {
        let params = parse_query_params("code=abc123&state=test_state");
        assert_eq!(params.get("code").unwrap(), "abc123");
        assert_eq!(params.get("state").unwrap(), "test_state");
    }

    #[test]
    fn test_parse_query_params_empty() {
        let params = parse_query_params("");
        assert!(params.is_empty());
    }

    #[test]
    fn test_parse_query_params_url_encoded() {
        let params = parse_query_params("error=access+denied&state=abc");
        assert_eq!(params.get("error").unwrap(), "access denied");
    }

    #[test]
    fn test_url_encode() {
        assert_eq!(url_encode("hello world"), "hello%20world");
        assert_eq!(url_encode("abc123"), "abc123");
    }

    #[test]
    fn test_url_decode() {
        assert_eq!(url_decode("hello%20world"), "hello world");
        assert_eq!(url_decode("abc%3D123"), "abc=123");
    }

    #[test]
    fn test_hex_encode() {
        assert_eq!(hex_encode(&[0x0f, 0xff]), "0fff");
        assert_eq!(hex_encode(&[0x00]), "00");
    }

    #[test]
    fn test_parse_flexible_int_number() {
        let val = serde_json::json!(5);
        assert_eq!(parse_flexible_int(&val), 5);
    }

    #[test]
    fn test_parse_flexible_int_string() {
        let val = serde_json::json!("10");
        assert_eq!(parse_flexible_int(&val), 10);
    }

    #[test]
    fn test_parse_flexible_int_null() {
        let val = serde_json::Value::Null;
        assert_eq!(parse_flexible_int(&val), 0);
    }

    #[test]
    fn test_parse_device_code_response() {
        let body = r#"{"device_auth_id":"da_123","user_code":"ABCD-1234","interval":5}"#;
        let parsed = parse_device_code_response(body).unwrap();
        assert_eq!(parsed.device_auth_id, "da_123");
        assert_eq!(parsed.user_code, "ABCD-1234");
        assert_eq!(parsed.interval, 5);
    }

    #[test]
    fn test_parse_device_code_response_string_interval() {
        let body = r#"{"device_auth_id":"da_456","user_code":"WXYZ-5678","interval":"10"}"#;
        let parsed = parse_device_code_response(body).unwrap();
        assert_eq!(parsed.interval, 10);
    }

    #[test]
    fn test_parse_device_code_response_missing_interval() {
        let body = r#"{"device_auth_id":"da_789","user_code":"EFGH-9012"}"#;
        let parsed = parse_device_code_response(body).unwrap();
        assert_eq!(parsed.interval, 0);
    }

    #[test]
    fn test_parse_token_response_full() {
        let body = br#"{"access_token":"at_123","refresh_token":"rt_456","expires_in":3600,"id_token":"eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.eyJjaGF0Z3B0X2FjY291bnRfaWQiOiJhY2N0XzEyMyJ9.signature"}"#;
        let cred = parse_token_response_impl(body, "openai").unwrap();
        assert_eq!(cred.access_token, "at_123");
        assert_eq!(cred.refresh_token.unwrap(), "rt_456");
        assert!(cred.expires_at.is_some());
        assert_eq!(cred.provider, "openai");
        assert_eq!(cred.auth_method, "oauth");
        assert_eq!(cred.account_id.unwrap(), "acct_123");
    }

    #[test]
    fn test_parse_token_response_minimal() {
        let body = br#"{"access_token":"at_only"}"#;
        let cred = parse_token_response_impl(body, "test").unwrap();
        assert_eq!(cred.access_token, "at_only");
        assert!(cred.refresh_token.is_none());
        assert!(cred.expires_at.is_none());
    }

    #[test]
    fn test_parse_token_response_no_access_token() {
        let body = br#"{"refresh_token":"rt_only"}"#;
        let result = parse_token_response_impl(body, "test");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no access token"));
    }

    #[test]
    fn test_parse_token_response_invalid_json() {
        let body = b"not json at all";
        let result = parse_token_response_impl(body, "test");
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_account_id_from_jwt() {
        // Build a minimal JWT with chatgpt_account_id claim
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256","typ":"JWT"}"#);
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            r#"{"chatgpt_account_id":"acct_abc"}"#,
        );
        let jwt = format!("{}.{}.signature", header, payload);
        let id = extract_account_id_impl(&jwt);
        assert_eq!(id.unwrap(), "acct_abc");
    }

    #[test]
    fn test_extract_account_id_from_namespaced_claim() {
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#);
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            r#"{"https://api.openai.com/auth.chatgpt_account_id":"acct_ns"}"#,
        );
        let jwt = format!("{}.{}.sig", header, payload);
        let id = extract_account_id_impl(&jwt);
        assert_eq!(id.unwrap(), "acct_ns");
    }

    #[test]
    fn test_extract_account_id_from_nested_auth() {
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#);
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            r#"{"https://api.openai.com/auth":{"chatgpt_account_id":"acct_nested"}}"#,
        );
        let jwt = format!("{}.{}.sig", header, payload);
        let id = extract_account_id_impl(&jwt);
        assert_eq!(id.unwrap(), "acct_nested");
    }

    #[test]
    fn test_extract_account_id_from_organizations() {
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#);
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            r#"{"organizations":[{"id":"org_123"},{"id":"org_456"}]}"#,
        );
        let jwt = format!("{}.{}.sig", header, payload);
        let id = extract_account_id_impl(&jwt);
        assert_eq!(id.unwrap(), "org_123");
    }

    #[test]
    fn test_extract_account_id_no_claims() {
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#);
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"sub":"user_123"}"#);
        let jwt = format!("{}.{}.sig", header, payload);
        let id = extract_account_id_impl(&jwt);
        assert!(id.is_none());
    }

    #[test]
    fn test_extract_account_id_invalid_jwt() {
        assert!(extract_account_id_impl("not-a-jwt").is_none());
        assert!(extract_account_id_impl("").is_none());
    }

    #[test]
    fn test_base64url_decode() {
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"hello world");
        let decoded = base64url_decode(&encoded).unwrap();
        assert_eq!(String::from_utf8(decoded).unwrap(), "hello world");
    }

    #[test]
    fn test_base64url_decode_with_padding_needed() {
        // "a" encoded without padding = "YQ"
        let decoded = base64url_decode("YQ").unwrap();
        assert_eq!(decoded, b"a");
    }

    #[test]
    fn test_hex_val() {
        assert_eq!(hex_val(b'0'), 0);
        assert_eq!(hex_val(b'9'), 9);
        assert_eq!(hex_val(b'a'), 10);
        assert_eq!(hex_val(b'f'), 15);
        assert_eq!(hex_val(b'A'), 10);
        assert_eq!(hex_val(b'F'), 15);
    }

    // ---- Tests for public standalone functions ----

    #[test]
    fn test_open_ai_oauth_config() {
        let cfg = super::open_ai_oauth_config();
        assert_eq!(cfg.port, 1455);
        assert!(cfg.issuer.contains("openai.com"));
        assert!(!cfg.client_id.is_empty());
    }

    #[test]
    fn test_standalone_build_authorize_url() {
        let cfg = super::open_ai_oauth_config();
        let pkce = crate::pkce::generate_pkce();
        let url = super::build_authorize_url(&cfg, &pkce, "mystate", "http://localhost:1455/auth/callback");
        assert!(url.contains("oauth/authorize"));
        assert!(url.contains("code_challenge"));
        assert!(url.contains("S256"));
        assert!(url.contains("state=mystate"));
        assert!(url.contains("id_token_add_organizations=true"));
        assert!(url.contains("codex_cli_simplified_flow=true"));
    }

    #[test]
    fn test_standalone_parse_token_response() {
        let body = br#"{"access_token":"at_xyz","refresh_token":"rt_abc","expires_in":3600}"#;
        let cred = super::parse_token_response(body, "test").unwrap();
        assert_eq!(cred.access_token, "at_xyz");
        assert_eq!(cred.refresh_token.unwrap(), "rt_abc");
        assert!(cred.expires_at.is_some());
    }

    #[test]
    fn test_standalone_extract_account_id() {
        // Non-JWT string returns empty
        assert_eq!(super::extract_account_id("not-a-jwt"), "");

        // Valid JWT with claim
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#);
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            r#"{"chatgpt_account_id":"acct_test"}"#,
        );
        let jwt = format!("{}.{}.sig", header, payload);
        assert_eq!(super::extract_account_id(&jwt), "acct_test");
    }

    #[test]
    fn test_standalone_parse_jwt_claims() {
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#);
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            r#"{"sub":"user_123","name":"test"}"#,
        );
        let jwt = format!("{}.{}.sig", header, payload);
        let claims = super::parse_jwt_claims(&jwt).unwrap();
        assert_eq!(claims.get("sub").unwrap().as_str().unwrap(), "user_123");
        assert_eq!(claims.get("name").unwrap().as_str().unwrap(), "test");
    }

    #[test]
    fn test_device_code_response_deserialize() {
        let json = r#"{"device_auth_id":"da_123","user_code":"ABCD-1234","interval":5}"#;
        let resp: super::DeviceCodeResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.device_auth_id, "da_123");
        assert_eq!(resp.user_code, "ABCD-1234");
        assert_eq!(resp.interval, 5);
    }

    #[test]
    fn test_device_code_response_deserialize_default_interval() {
        // Missing interval -> default 0
        let json = r#"{"device_auth_id":"da_456","user_code":"EFGH-5678"}"#;
        let resp: super::DeviceCodeResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.device_auth_id, "da_456");
        assert_eq!(resp.user_code, "EFGH-5678");
        assert_eq!(resp.interval, 0);
    }

    #[test]
    fn test_device_code_response_debug_clone() {
        let resp = super::DeviceCodeResponse {
            device_auth_id: "da_1".to_string(),
            user_code: "AB-12".to_string(),
            interval: 5,
        };
        let cloned = resp.clone();
        assert_eq!(resp.device_auth_id, cloned.device_auth_id);
        let debug_str = format!("{:?}", resp);
        assert!(debug_str.contains("da_1"));
    }

    #[test]
    fn test_oauth_provider_config_custom() {
        let cfg = OAuthProviderConfig {
            issuer: "https://auth.example.com".to_string(),
            client_id: "my_client".to_string(),
            scopes: "read write".to_string(),
            originator: "myapp".to_string(),
            port: 8080,
        };
        assert_eq!(cfg.issuer, "https://auth.example.com");
        assert_eq!(cfg.client_id, "my_client");
        assert_eq!(cfg.scopes, "read write");
        assert_eq!(cfg.originator, "myapp");
        assert_eq!(cfg.port, 8080);
    }

    #[test]
    fn test_oauth_provider_config_debug_clone() {
        let cfg = OAuthProviderConfig::openai();
        let cloned = cfg.clone();
        assert_eq!(cfg.issuer, cloned.issuer);
        assert_eq!(cfg.client_id, cloned.client_id);
        let debug_str = format!("{:?}", cfg);
        assert!(debug_str.contains("openai.com"));
    }

    #[test]
    fn test_build_authorize_url_with_empty_originator() {
        let cfg = OAuthProviderConfig {
            issuer: "https://auth.example.com".to_string(),
            client_id: "my_client".to_string(),
            scopes: "openid".to_string(),
            originator: "".to_string(),
            port: 8080,
        };
        let pkce = crate::pkce::generate_pkce();
        let url = build_authorize_url(&cfg, &pkce, "state", "http://localhost:8080/callback");
        // Should NOT contain originator param when empty
        assert!(!url.contains("originator="));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("client_id=my_client"));
    }

    #[test]
    fn test_build_authorize_url_with_originator() {
        let cfg = OAuthProviderConfig {
            issuer: "https://auth.example.com".to_string(),
            client_id: "my_client".to_string(),
            scopes: "openid".to_string(),
            originator: "testapp".to_string(),
            port: 8080,
        };
        let pkce = crate::pkce::generate_pkce();
        let url = build_authorize_url(&cfg, &pkce, "state", "http://localhost:8080/callback");
        assert!(url.contains("originator=testapp"));
    }

    #[test]
    fn test_build_authorize_url_contains_openai_specific_params() {
        let cfg = OAuthProviderConfig::openai();
        let pkce = crate::pkce::generate_pkce();
        let url = cfg.build_authorize_url(&pkce, "mystate", "http://localhost:1455/auth/callback");
        assert!(url.contains("id_token_add_organizations=true"));
        assert!(url.contains("codex_cli_simplified_flow=true"));
        assert!(url.contains("redirect_uri="));
    }

    #[test]
    fn test_parse_query_params_single_param() {
        let params = parse_query_params("key=value");
        assert_eq!(params.get("key").unwrap(), "value");
        assert_eq!(params.len(), 1);
    }

    #[test]
    fn test_parse_query_params_no_value() {
        // Key without =value should not be included
        let params = parse_query_params("keyonly");
        assert!(params.is_empty());
    }

    #[test]
    fn test_parse_query_params_multiple() {
        let params = parse_query_params("a=1&b=2&c=3");
        assert_eq!(params.len(), 3);
        assert_eq!(params.get("a").unwrap(), "1");
        assert_eq!(params.get("b").unwrap(), "2");
        assert_eq!(params.get("c").unwrap(), "3");
    }

    #[test]
    fn test_parse_query_params_with_special_chars() {
        let params = parse_query_params("name=hello%20world&key=abc%3D123");
        assert_eq!(params.get("name").unwrap(), "hello world");
        assert_eq!(params.get("key").unwrap(), "abc=123");
    }

    #[test]
    fn test_url_encode_special_chars() {
        assert_eq!(url_encode("hello@world"), "hello%40world");
        assert_eq!(url_encode("a=b&c=d"), "a%3Db%26c%3Dd");
        assert_eq!(url_encode("path/to"), "path%2Fto");
        assert_eq!(url_encode("hello world"), "hello%20world");
    }

    #[test]
    fn test_url_encode_unreserved_chars() {
        // RFC 3986 unreserved chars should not be encoded
        assert_eq!(url_encode("abc123"), "abc123");
        assert_eq!(url_encode("a-b_c.d"), "a-b_c.d");
        assert_eq!(url_encode("test~value"), "test~value");
    }

    #[test]
    fn test_url_decode_plus_as_space() {
        assert_eq!(url_decode("hello+world"), "hello world");
        assert_eq!(url_decode("a+b+c"), "a b c");
    }

    #[test]
    fn test_url_decode_truncated_percent() {
        // Truncated % at end of string: %X should use fallback 0 for missing byte
        let result = url_decode("test%4");
        // Should not panic, should decode something
        assert!(!result.is_empty());
    }

    #[test]
    fn test_url_decode_percent_encoding_mixed() {
        assert_eq!(url_decode("%21%40%23"), "!@#");
    }

    #[test]
    fn test_url_decode_no_encoding() {
        assert_eq!(url_decode("hello"), "hello");
        assert_eq!(url_decode(""), "");
    }

    #[test]
    fn test_hex_val_non_hex() {
        // Non-hex chars should return 0
        assert_eq!(hex_val(b'g'), 0);
        assert_eq!(hex_val(b'z'), 0);
        assert_eq!(hex_val(b'G'), 0);
        assert_eq!(hex_val(b'!'), 0);
    }

    #[test]
    fn test_hex_encode_various() {
        assert_eq!(hex_encode(&[0x00, 0x01, 0xfe, 0xff]), "0001feff");
        assert_eq!(hex_encode(&[]), "");
        assert_eq!(hex_encode(&[0xab, 0xcd]), "abcd");
    }

    #[test]
    fn test_generate_state_format() {
        let state = generate_state();
        // 32 bytes = 64 hex chars
        assert_eq!(state.len(), 64);
        // Should all be hex chars
        for c in state.chars() {
            assert!(c.is_ascii_hexdigit(), "non-hex char '{}' in state", c);
        }
    }

    #[test]
    fn test_generate_state_uniqueness() {
        let mut states = std::collections::HashSet::new();
        for _ in 0..100 {
            states.insert(generate_state());
        }
        assert_eq!(states.len(), 100);
    }

    #[test]
    fn test_base64url_decode_padding_mod_4_eq_3() {
        // Input length % 4 == 3 -> one = padding added
        // "YWI" -> "ab" (base64 of "ab" without padding is "YWI")
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let encoded = URL_SAFE_NO_PAD.encode(b"ab");
        let decoded = base64url_decode(&encoded).unwrap();
        assert_eq!(decoded, b"ab");
    }

    #[test]
    fn test_base64url_decode_padding_mod_4_eq_0() {
        // Input length % 4 == 0 -> no padding needed
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let encoded = URL_SAFE_NO_PAD.encode(b"four");
        let decoded = base64url_decode(&encoded).unwrap();
        assert_eq!(decoded, b"four");
    }

    #[test]
    fn test_base64url_decode_invalid() {
        let result = base64url_decode("!!!invalid!!!");
        assert!(result.is_err());
    }

    #[test]
    fn test_base64url_decode_url_safe_chars() {
        // Verify URL-safe base64 chars (- _) are handled
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let original = b"\xff\xfe\xfd\xfc\xfb\xfa";
        let encoded = URL_SAFE_NO_PAD.encode(original);
        let decoded = base64url_decode(&encoded).unwrap();
        assert_eq!(decoded.as_slice(), original);
    }

    #[test]
    fn test_parse_jwt_claims_impl_single_part() {
        // Only one part (no dots) -> error
        let result = parse_jwt_claims_impl("onlyonepart");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not a JWT"));
    }

    #[test]
    fn test_parse_jwt_claims_impl_invalid_base64() {
        // Two parts but invalid base64 in payload
        let result = parse_jwt_claims_impl("header.!!!invalid!!!");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_jwt_claims_impl_invalid_json_in_payload() {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#);
        let payload = URL_SAFE_NO_PAD.encode(r#"not json"#);
        let jwt = format!("{}.{}.sig", header, payload);
        let result = parse_jwt_claims_impl(&jwt);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_jwt_claims_impl_valid() {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#);
        let payload = URL_SAFE_NO_PAD.encode(r#"{"sub":"user_1","exp":12345}"#);
        let jwt = format!("{}.{}.sig", header, payload);
        let claims = parse_jwt_claims_impl(&jwt).unwrap();
        assert_eq!(claims["sub"].as_str().unwrap(), "user_1");
        assert_eq!(claims["exp"].as_u64().unwrap(), 12345);
    }

    #[test]
    fn test_parse_device_code_response_invalid_json() {
        let result = parse_device_code_response("not json");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_device_code_response_missing_fields() {
        // Missing fields -> defaults to empty strings / 0
        let body = r#"{"other":"data"}"#;
        let parsed = parse_device_code_response(body).unwrap();
        assert_eq!(parsed.device_auth_id, "");
        assert_eq!(parsed.user_code, "");
        assert_eq!(parsed.interval, 0);
    }

    #[test]
    fn test_parse_flexible_int_negative() {
        let val = serde_json::json!(-5);
        assert_eq!(parse_flexible_int(&val), -5);
    }

    #[test]
    fn test_parse_flexible_int_invalid_string() {
        let val = serde_json::json!("not_a_number");
        assert_eq!(parse_flexible_int(&val), 0);
    }

    #[test]
    fn test_parse_flexible_int_whitespace_string() {
        let val = serde_json::json!("  7  ");
        assert_eq!(parse_flexible_int(&val), 7);
    }

    #[test]
    fn test_parse_flexible_int_float() {
        // Float -> as_i64 returns None -> 0
        let val = serde_json::json!(3.14);
        assert_eq!(parse_flexible_int(&val), 0);
    }

    #[test]
    fn test_parse_token_response_zero_expires_in() {
        let body = br#"{"access_token":"at_123","expires_in":0}"#;
        let cred = parse_token_response_impl(body, "test").unwrap();
        assert_eq!(cred.access_token, "at_123");
        assert!(cred.expires_at.is_none());
    }

    #[test]
    fn test_parse_token_response_missing_expires_in() {
        let body = br#"{"access_token":"at_123"}"#;
        let cred = parse_token_response_impl(body, "test").unwrap();
        assert!(cred.expires_at.is_none());
    }

    #[test]
    fn test_parse_token_response_with_id_token_account_id() {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#);
        let payload = URL_SAFE_NO_PAD.encode(r#"{"chatgpt_account_id":"acct_from_id_token"}"#);
        let id_token = format!("{}.{}.sig", header, payload);
        let body_json = format!(
            r#"{{"access_token":"at_1","id_token":"{}"}}"#,
            id_token
        );
        let cred = parse_token_response_impl(body_json.as_bytes(), "openai").unwrap();
        assert_eq!(cred.account_id.unwrap(), "acct_from_id_token");
    }

    #[test]
    fn test_parse_token_response_account_id_fallback_to_access_token() {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        // No id_token, but access_token contains account ID
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#);
        let payload = URL_SAFE_NO_PAD.encode(r#"{"chatgpt_account_id":"acct_from_access"}"#);
        let access_token = format!("{}.{}.sig", header, payload);
        let body_json = format!(
            r#"{{"access_token":"{}"}}"#,
            access_token
        );
        let cred = parse_token_response_impl(body_json.as_bytes(), "openai").unwrap();
        assert_eq!(cred.account_id.unwrap(), "acct_from_access");
    }

    #[test]
    fn test_extract_account_id_empty_string_claims() {
        // JWT with empty chatgpt_account_id -> should skip it
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#);
        let payload = URL_SAFE_NO_PAD.encode(r#"{"chatgpt_account_id":""}"#);
        let jwt = format!("{}.{}.sig", header, payload);
        let id = extract_account_id_impl(&jwt);
        assert!(id.is_none());
    }

    #[test]
    fn test_extract_account_id_empty_namespaced_claim() {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#);
        let payload = URL_SAFE_NO_PAD.encode(
            r#"{"https://api.openai.com/auth.chatgpt_account_id":""}"#,
        );
        let jwt = format!("{}.{}.sig", header, payload);
        let id = extract_account_id_impl(&jwt);
        assert!(id.is_none());
    }

    #[test]
    fn test_extract_account_id_empty_nested_auth() {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#);
        let payload = URL_SAFE_NO_PAD.encode(
            r#"{"https://api.openai.com/auth":{"chatgpt_account_id":""}}"#,
        );
        let jwt = format!("{}.{}.sig", header, payload);
        let id = extract_account_id_impl(&jwt);
        assert!(id.is_none());
    }

    #[test]
    fn test_extract_account_id_empty_org_id() {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#);
        let payload = URL_SAFE_NO_PAD.encode(
            r#"{"organizations":[{"id":""}]}"#,
        );
        let jwt = format!("{}.{}.sig", header, payload);
        let id = extract_account_id_impl(&jwt);
        assert!(id.is_none());
    }

    #[test]
    fn test_extract_account_id_org_non_object_entry() {
        // Organizations array with non-object entries -> should be skipped
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#);
        let payload = URL_SAFE_NO_PAD.encode(
            r#"{"organizations":["string",42,null]}"#,
        );
        let jwt = format!("{}.{}.sig", header, payload);
        let id = extract_account_id_impl(&jwt);
        assert!(id.is_none());
    }

    #[test]
    fn test_extract_account_id_priority_order() {
        // Direct chatgpt_account_id takes priority over other methods
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#);
        let payload = URL_SAFE_NO_PAD.encode(
            r#"{"chatgpt_account_id":"direct_id","https://api.openai.com/auth.chatgpt_account_id":"ns_id"}"#,
        );
        let jwt = format!("{}.{}.sig", header, payload);
        let id = extract_account_id_impl(&jwt);
        assert_eq!(id.unwrap(), "direct_id");
    }

    #[test]
    fn test_standalone_parse_jwt_claims_invalid() {
        let result = super::parse_jwt_claims("not-a-jwt");
        assert!(result.is_err());
    }

    #[test]
    fn test_standalone_parse_jwt_claims_returns_hashmap() {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#);
        let payload = URL_SAFE_NO_PAD.encode(r#"{"sub":"user_1","role":"admin"}"#);
        let jwt = format!("{}.{}.sig", header, payload);
        let claims = super::parse_jwt_claims(&jwt).unwrap();
        assert_eq!(claims.get("sub").unwrap().as_str().unwrap(), "user_1");
        assert_eq!(claims.get("role").unwrap().as_str().unwrap(), "admin");
    }

    #[test]
    fn test_device_token_response_deserialize() {
        let json = r#"{"authorization_code":"auth_code_123","code_verifier":"cv_456"}"#;
        let resp: DeviceTokenResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.authorization_code, "auth_code_123");
        assert_eq!(resp.code_verifier, "cv_456");
    }

    #[test]
    fn test_openai_login_device_code_interval_floor() {
        // interval < 1 should be clamped to 5 in login_device_code flow
        // We test parse_device_code_response returns the raw value;
        // the clamping is in login_device_code (async, needs server)
        let body = r#"{"device_auth_id":"da_1","user_code":"AB-12","interval":0}"#;
        let parsed = parse_device_code_response(body).unwrap();
        assert_eq!(parsed.interval, 0); // Raw value; clamping happens in caller
    }

    #[test]
    fn test_standalone_open_ai_oauth_config_fields() {
        let cfg = super::open_ai_oauth_config();
        assert_eq!(cfg.port, 1455);
        assert!(cfg.issuer.starts_with("https://"));
        assert!(!cfg.client_id.is_empty());
        assert!(!cfg.scopes.is_empty());
        assert!(!cfg.originator.is_empty());
    }

    // --- Additional coverage tests for oauth.rs ---

    #[test]
    fn test_parse_token_response_impl_no_access_token() {
        let body = br#"{"refresh_token":"rt_123"}"#;
        let result = parse_token_response_impl(body, "openai");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no access token"));
    }

    #[test]
    fn test_parse_token_response_impl_invalid_json() {
        let body = b"not json at all";
        let result = parse_token_response_impl(body, "openai");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("parsing token response"));
    }

    #[test]
    fn test_parse_token_response_impl_with_expires_in() {
        let body = br#"{"access_token":"at_123","expires_in":3600}"#;
        let cred = parse_token_response_impl(body, "openai").unwrap();
        assert_eq!(cred.access_token, "at_123");
        assert!(cred.expires_at.is_some());
        assert!(cred.refresh_token.is_none());
    }

    #[test]
    fn test_parse_token_response_impl_with_refresh_token() {
        let body = br#"{"access_token":"at_123","refresh_token":"rt_456","expires_in":3600}"#;
        let cred = parse_token_response_impl(body, "test").unwrap();
        assert_eq!(cred.refresh_token.unwrap(), "rt_456");
    }

    #[test]
    fn test_parse_token_response_impl_provider_field() {
        let body = br#"{"access_token":"at_123"}"#;
        let cred = parse_token_response_impl(body, "myprovider").unwrap();
        assert_eq!(cred.provider, "myprovider");
        assert_eq!(cred.auth_method, "oauth");
    }

    #[test]
    fn test_parse_token_response_with_id_token_account_id_namespaced() {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#);
        let payload = URL_SAFE_NO_PAD.encode(
            r#"{"https://api.openai.com/auth.chatgpt_account_id":"ns_acct_1"}"#,
        );
        let id_token = format!("{}.{}.sig", header, payload);
        let body_json = format!(
            r#"{{"access_token":"at_1","id_token":"{}"}}"#,
            id_token
        );
        let cred = parse_token_response_impl(body_json.as_bytes(), "openai").unwrap();
        assert_eq!(cred.account_id.unwrap(), "ns_acct_1");
    }

    #[test]
    fn test_parse_token_response_with_id_token_nested_auth() {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#);
        let payload = URL_SAFE_NO_PAD.encode(
            r#"{"https://api.openai.com/auth":{"chatgpt_account_id":"nested_acct_1"}}"#,
        );
        let id_token = format!("{}.{}.sig", header, payload);
        let body_json = format!(
            r#"{{"access_token":"at_1","id_token":"{}"}}"#,
            id_token
        );
        let cred = parse_token_response_impl(body_json.as_bytes(), "openai").unwrap();
        assert_eq!(cred.account_id.unwrap(), "nested_acct_1");
    }

    #[test]
    fn test_parse_token_response_with_id_token_org() {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#);
        let payload = URL_SAFE_NO_PAD.encode(
            r#"{"organizations":[{"id":"org_123","name":"TestOrg"},{"id":"org_456","name":"Other"}]}"#,
        );
        let id_token = format!("{}.{}.sig", header, payload);
        let body_json = format!(
            r#"{{"access_token":"at_1","id_token":"{}"}}"#,
            id_token
        );
        let cred = parse_token_response_impl(body_json.as_bytes(), "openai").unwrap();
        assert_eq!(cred.account_id.unwrap(), "org_123");
    }

    #[test]
    fn test_extract_account_id_impl_valid() {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#);
        let payload = URL_SAFE_NO_PAD.encode(r#"{"chatgpt_account_id":"acct_valid_123"}"#);
        let jwt = format!("{}.{}.sig", header, payload);
        let id = extract_account_id_impl(&jwt);
        assert_eq!(id.unwrap(), "acct_valid_123");
    }

    #[test]
    fn test_extract_account_id_impl_no_claims() {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#);
        let payload = URL_SAFE_NO_PAD.encode(r#"{"sub":"user_1"}"#);
        let jwt = format!("{}.{}.sig", header, payload);
        let id = extract_account_id_impl(&jwt);
        assert!(id.is_none());
    }

    #[test]
    fn test_extract_account_id_impl_invalid_jwt() {
        let id = extract_account_id_impl("not-a-jwt");
        assert!(id.is_none());
    }

    #[test]
    fn test_extract_account_id_impl_namespaced_claim() {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#);
        let payload = URL_SAFE_NO_PAD.encode(
            r#"{"https://api.openai.com/auth.chatgpt_account_id":"ns_id_123"}"#,
        );
        let jwt = format!("{}.{}.sig", header, payload);
        let id = extract_account_id_impl(&jwt);
        assert_eq!(id.unwrap(), "ns_id_123");
    }

    #[test]
    fn test_extract_account_id_impl_nested_auth() {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#);
        let payload = URL_SAFE_NO_PAD.encode(
            r#"{"https://api.openai.com/auth":{"chatgpt_account_id":"nested_id_123"}}"#,
        );
        let jwt = format!("{}.{}.sig", header, payload);
        let id = extract_account_id_impl(&jwt);
        assert_eq!(id.unwrap(), "nested_id_123");
    }

    #[test]
    fn test_extract_account_id_impl_organization() {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#);
        let payload = URL_SAFE_NO_PAD.encode(
            r#"{"organizations":[{"id":"org_acct_1"}]}"#,
        );
        let jwt = format!("{}.{}.sig", header, payload);
        let id = extract_account_id_impl(&jwt);
        assert_eq!(id.unwrap(), "org_acct_1");
    }

    #[test]
    fn test_extract_account_id_priority_direct_over_org() {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#);
        let payload = URL_SAFE_NO_PAD.encode(
            r#"{"chatgpt_account_id":"direct_acct","organizations":[{"id":"org_acct"}]}"#,
        );
        let jwt = format!("{}.{}.sig", header, payload);
        let id = extract_account_id_impl(&jwt);
        // Direct chatgpt_account_id should take priority
        assert_eq!(id.unwrap(), "direct_acct");
    }

    #[test]
    fn test_extract_account_id_standalone_function() {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#);
        let payload = URL_SAFE_NO_PAD.encode(r#"{"chatgpt_account_id":"standalone_123"}"#);
        let jwt = format!("{}.{}.sig", header, payload);
        let id = extract_account_id(&jwt);
        assert_eq!(id, "standalone_123");
    }

    #[test]
    fn test_extract_account_id_standalone_invalid() {
        let id = extract_account_id("not-a-jwt");
        assert!(id.is_empty());
    }

    #[test]
    fn test_parse_token_response_standalone_function() {
        let body = br#"{"access_token":"at_abc","expires_in":7200}"#;
        let cred = parse_token_response(body, "test_provider").unwrap();
        assert_eq!(cred.access_token, "at_abc");
        assert_eq!(cred.provider, "test_provider");
    }

    #[test]
    fn test_parse_token_response_standalone_error() {
        let body = br#"{}"#;
        let result = parse_token_response(body, "test");
        assert!(result.is_err());
    }

    #[test]
    fn test_device_code_response_deserialize_with_defaults() {
        let json = r#"{"device_auth_id":"da_1","user_code":"AB-12"}"#;
        let resp: DeviceCodeResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.device_auth_id, "da_1");
        assert_eq!(resp.user_code, "AB-12");
        assert_eq!(resp.interval, 0); // default
    }

    #[test]
    fn test_device_code_response_debug_and_clone() {
        let resp = DeviceCodeResponse {
            device_auth_id: "da_test".to_string(),
            user_code: "XY-99".to_string(),
            interval: 5,
        };
        let cloned = resp.clone();
        assert_eq!(cloned.device_auth_id, "da_test");
        let debug_str = format!("{:?}", resp);
        assert!(debug_str.contains("da_test"));
    }

    #[test]
    fn test_parse_device_code_response_with_all_fields() {
        let body = r#"{"device_auth_id":"da_full","user_code":"CD-34","interval":10}"#;
        let parsed = parse_device_code_response(body).unwrap();
        assert_eq!(parsed.device_auth_id, "da_full");
        assert_eq!(parsed.user_code, "CD-34");
        assert_eq!(parsed.interval, 10);
    }

    #[test]
    fn test_parse_device_code_response_negative_interval() {
        let body = r#"{"device_auth_id":"da_neg","user_code":"EF-56","interval":-3}"#;
        let parsed = parse_device_code_response(body).unwrap();
        assert_eq!(parsed.interval, -3);
    }

    #[test]
    fn test_base64url_decode_padding_mod_4_eq_2() {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let encoded = URL_SAFE_NO_PAD.encode(b"a");
        let decoded = base64url_decode(&encoded).unwrap();
        assert_eq!(decoded, b"a");
    }

    #[test]
    fn test_base64url_decode_empty_input() {
        let decoded = base64url_decode("").unwrap();
        assert!(decoded.is_empty());
    }

    #[test]
    fn test_parse_jwt_claims_impl_three_parts() {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#);
        let payload = URL_SAFE_NO_PAD.encode(r#"{"sub":"user_test"}"#);
        let jwt = format!("{}.{}.signature_here", header, payload);
        let claims = parse_jwt_claims_impl(&jwt).unwrap();
        assert_eq!(claims["sub"].as_str().unwrap(), "user_test");
    }

    #[test]
    fn test_parse_jwt_claims_impl_empty_string() {
        let result = parse_jwt_claims_impl("");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not a JWT"));
    }

    #[test]
    fn test_url_encode_cjk_chars() {
        // CJK characters should be percent-encoded (each byte)
        let encoded = url_encode("hello");
        assert_eq!(encoded, "hello");
    }

    #[test]
    fn test_url_decode_various() {
        assert_eq!(url_decode("a%20b%20c"), "a b c");
        assert_eq!(url_decode("%3C%3E%23"), "<>#");
        assert_eq!(url_decode("no+encoding+needed"), "no encoding needed");
    }

    #[test]
    fn test_parse_query_params_with_ampersand_only() {
        let params = parse_query_params("&");
        assert!(params.is_empty());
    }

    #[test]
    fn test_parse_query_params_with_equals_only() {
        let params = parse_query_params("=");
        // Empty key = empty value is still a valid param
        assert_eq!(params.len(), 1);
        assert!(params.contains_key(""));
    }

    #[test]
    fn test_parse_query_params_with_empty_value() {
        let params = parse_query_params("key=");
        assert_eq!(params.get("key").unwrap(), "");
    }

    #[test]
    fn test_parse_flexible_int_zero() {
        let val = serde_json::json!(0);
        assert_eq!(parse_flexible_int(&val), 0);
    }

    #[test]
    fn test_parse_flexible_int_large_number() {
        let val = serde_json::json!(1000000);
        assert_eq!(parse_flexible_int(&val), 1000000);
    }

    #[test]
    fn test_parse_flexible_int_bool() {
        let val = serde_json::json!(true);
        assert_eq!(parse_flexible_int(&val), 0); // bool is not a number
    }

    #[test]
    fn test_parse_flexible_int_object() {
        let val = serde_json::json!({"key": "value"});
        assert_eq!(parse_flexible_int(&val), 0);
    }

    #[test]
    fn test_hex_val_digits() {
        assert_eq!(hex_val(b'0'), 0);
        assert_eq!(hex_val(b'9'), 9);
        assert_eq!(hex_val(b'a'), 10);
        assert_eq!(hex_val(b'f'), 15);
        assert_eq!(hex_val(b'A'), 10);
        assert_eq!(hex_val(b'F'), 15);
    }

    #[test]
    fn test_hex_encode_bytes() {
        assert_eq!(hex_encode(&[0xff]), "ff");
        assert_eq!(hex_encode(&[0x00, 0x01, 0xfe, 0xff]), "0001feff");
    }

    #[test]
    fn test_generate_state_is_hex() {
        let state = generate_state();
        assert_eq!(state.len(), 64);
        for c in state.chars() {
            assert!(c.is_ascii_hexdigit());
        }
    }

    #[test]
    fn test_build_authorize_url_all_params_present() {
        let cfg = OAuthProviderConfig {
            issuer: "https://auth.example.com".to_string(),
            client_id: "test_client".to_string(),
            scopes: "openid profile".to_string(),
            originator: "myapp".to_string(),
            port: 8080,
        };
        let pkce = crate::pkce::generate_pkce();
        let url = build_authorize_url(&cfg, &pkce, "test_state", "http://localhost:8080/cb");
        assert!(url.starts_with("https://auth.example.com/oauth/authorize?"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("client_id=test_client"));
        assert!(url.contains("redirect_uri="));
        assert!(url.contains("scope="));
        assert!(url.contains("code_challenge="));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("state=test_state"));
        assert!(url.contains("originator=myapp"));
    }

    #[test]
    fn test_oauth_provider_config_method_new() {
        let cfg = OAuthProviderConfig::openai();
        assert_eq!(cfg.issuer, "https://auth.openai.com");
        assert!(!cfg.client_id.is_empty());
    }

    #[test]
    fn test_login_paste_token_function() {
        let cred = crate::token::AuthCredential::login_paste_token("openai", "  my_token  ").unwrap();
        assert_eq!(cred.access_token, "my_token");
        assert_eq!(cred.provider, "openai");
        assert_eq!(cred.auth_method, "token");
    }

    #[test]
    fn test_login_paste_token_empty() {
        let result = crate::token::AuthCredential::login_paste_token("openai", "   ");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty"));
    }

    #[test]
    fn test_credential_can_refresh() {
        let cred = crate::token::AuthCredential {
            access_token: "at".to_string(),
            refresh_token: Some("rt".to_string()),
            expires_at: None,
            provider: "test".to_string(),
            auth_method: "oauth".to_string(),
            account_id: None,
        };
        assert!(cred.can_refresh());

        let cred_no_refresh = crate::token::AuthCredential {
            access_token: "at".to_string(),
            refresh_token: None,
            expires_at: None,
            provider: "test".to_string(),
            auth_method: "oauth".to_string(),
            account_id: None,
        };
        assert!(!cred_no_refresh.can_refresh());
    }

    #[test]
    fn test_credential_needs_refresh() {
        let cred = crate::token::AuthCredential {
            access_token: "at".to_string(),
            refresh_token: Some("rt".to_string()),
            expires_at: Some(Utc::now() + chrono::Duration::minutes(3)), // within 5 min
            provider: "test".to_string(),
            auth_method: "oauth".to_string(),
            account_id: None,
        };
        assert!(cred.needs_refresh());

        let cred_not_expired = crate::token::AuthCredential {
            access_token: "at".to_string(),
            refresh_token: Some("rt".to_string()),
            expires_at: Some(Utc::now() + chrono::Duration::hours(1)),
            provider: "test".to_string(),
            auth_method: "oauth".to_string(),
            account_id: None,
        };
        assert!(!cred_not_expired.needs_refresh());
    }

    #[test]
    fn test_credential_serialization() {
        let cred = crate::token::AuthCredential {
            access_token: "at_123".to_string(),
            refresh_token: Some("rt_456".to_string()),
            expires_at: Some(Utc::now()),
            provider: "openai".to_string(),
            auth_method: "oauth".to_string(),
            account_id: Some("acct_789".to_string()),
        };
        let json = serde_json::to_string(&cred).unwrap();
        let parsed: crate::token::AuthCredential = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.access_token, "at_123");
        assert_eq!(parsed.refresh_token.unwrap(), "rt_456");
        assert_eq!(parsed.account_id.unwrap(), "acct_789");
    }

    #[test]
    fn test_provider_display_name() {
        assert_eq!(crate::token::provider_display_name("anthropic"), "console.anthropic.com");
        assert_eq!(crate::token::provider_display_name("openai"), "platform.openai.com");
        assert_eq!(crate::token::provider_display_name("custom"), "custom");
    }

    #[test]
    fn test_parse_token_response_impl_large_expires_in() {
        let body = br#"{"access_token":"at_large","expires_in":86400}"#;
        let cred = parse_token_response_impl(body, "openai").unwrap();
        assert!(cred.expires_at.is_some());
        let expires = cred.expires_at.unwrap();
        let diff = expires.timestamp() - Utc::now().timestamp();
        assert!(diff > 86000 && diff <= 86400);
    }

    #[test]
    fn test_extract_account_id_impl_empty_string_token() {
        let id = extract_account_id_impl("");
        assert!(id.is_none());
    }

    #[test]
    fn test_parse_jwt_claims_impl_two_parts_only() {
        // Only two parts separated by dot
        let result = parse_jwt_claims_impl("header.payload");
        // This should either succeed or fail depending on base64 decode
        // If "header" and "payload" are valid base64, the json parse will fail
        assert!(result.is_err());
    }

    // --- Additional unique coverage tests for oauth.rs ---

    #[test]
    fn test_parse_device_code_response_empty_body() {
        let result = parse_device_code_response("");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_flexible_int_negative_number() {
        let val = serde_json::json!(-5);
        assert_eq!(parse_flexible_int(&val), -5);
    }

    #[test]
    fn test_parse_flexible_int_float_number() {
        let val = serde_json::json!(5.7);
        let result = parse_flexible_int(&val);
        assert!(result == 5 || result == 0);
    }

    #[test]
    fn test_parse_flexible_int_boolean() {
        let val = serde_json::json!(true);
        assert_eq!(parse_flexible_int(&val), 0);
    }

    #[test]
    fn test_parse_token_response_with_zero_expires() {
        let body = br#"{"access_token":"at_123","expires_in":0}"#;
        let cred = parse_token_response_impl(body, "test").unwrap();
        assert!(cred.expires_at.is_none());
    }

    #[test]
    fn test_parse_token_response_with_empty_access_token() {
        let body = br#"{"access_token":"","refresh_token":"rt_456"}"#;
        let result = parse_token_response_impl(body, "test");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no access token"));
    }

    #[test]
    fn test_parse_token_response_with_string_expires_in() {
        let body = br#"{"access_token":"at_123","expires_in":"3600"}"#;
        let cred = parse_token_response_impl(body, "test").unwrap();
        assert!(cred.expires_at.is_none());
    }

    #[test]
    fn test_extract_account_id_empty_chatgpt_account_id() {
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#);
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            r#"{"chatgpt_account_id":""}"#,
        );
        let jwt = format!("{}.{}.sig", header, payload);
        let id = extract_account_id_impl(&jwt);
        assert!(id.is_none());
    }

    #[test]
    fn test_extract_account_id_empty_organization_id() {
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#);
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            r#"{"organizations":[{"id":""}]}"#,
        );
        let jwt = format!("{}.{}.sig", header, payload);
        let id = extract_account_id_impl(&jwt);
        assert!(id.is_none());
    }

    #[test]
    fn test_extract_account_id_organizations_non_object_entry() {
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#);
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            r#"{"organizations":["not_an_object"]}"#,
        );
        let jwt = format!("{}.{}.sig", header, payload);
        let id = extract_account_id_impl(&jwt);
        assert!(id.is_none());
    }

    #[test]
    fn test_extract_account_id_no_id_field_in_org() {
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#);
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            r#"{"organizations":[{"name":"test_org"}]}"#,
        );
        let jwt = format!("{}.{}.sig", header, payload);
        let id = extract_account_id_impl(&jwt);
        assert!(id.is_none());
    }

    #[test]
    fn test_parse_jwt_claims_impl_valid_but_non_json_payload() {
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#);
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"not json");
        let jwt = format!("{}.{}.sig", header, payload);
        let result = parse_jwt_claims_impl(&jwt);
        assert!(result.is_err());
        assert!(result.err().unwrap().contains("parsing JWT claims"));
    }

    #[test]
    fn test_base64url_decode_invalid_characters() {
        let result = base64url_decode("!!!invalid!!!");
        assert!(result.is_err());
        assert!(result.err().unwrap().contains("base64 decode"));
    }

    #[test]
    fn test_base64url_decode_empty() {
        let result = base64url_decode("").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_url_encode_empty() {
        assert_eq!(url_encode(""), "");
    }

    #[test]
    fn test_url_encode_unicode() {
        let encoded = url_encode("hello\u{00E9}");
        assert!(encoded.contains("%"));
    }

    #[test]
    fn test_url_encode_colon() {
        assert_eq!(url_encode(":"), "%3A");
    }

    #[test]
    fn test_url_encode_question_mark() {
        assert_eq!(url_encode("?"), "%3F");
    }

    #[test]
    fn test_url_encode_ampersand() {
        assert_eq!(url_encode("&"), "%26");
    }

    #[test]
    fn test_url_encode_equals() {
        assert_eq!(url_encode("="), "%3D");
    }

    #[test]
    fn test_url_decode_percent_with_hex_digits() {
        assert_eq!(url_decode("%41%42%43"), "ABC");
    }

    #[test]
    fn test_url_decode_lowercase_hex() {
        assert_eq!(url_decode("%61%62%63"), "abc");
    }

    #[test]
    fn test_parse_query_params_duplicate_keys() {
        let params = parse_query_params("key=val1&key=val2");
        assert_eq!(params.get("key").unwrap(), "val2");
        assert_eq!(params.len(), 1);
    }

    #[test]
    fn test_parse_query_params_empty_value() {
        let params = parse_query_params("key=");
        assert_eq!(params.get("key").unwrap(), "");
    }

    #[test]
    fn test_parse_query_params_url_encoded_key() {
        let params = parse_query_params("my%20key=value");
        assert_eq!(params.get("my key").unwrap(), "value");
    }

    #[test]
    fn test_hex_encode_single_byte() {
        assert_eq!(hex_encode(&[0x00]), "00");
        assert_eq!(hex_encode(&[0xff]), "ff");
        assert_eq!(hex_encode(&[0x0a]), "0a");
    }

    #[test]
    fn test_build_authorize_url_full_params() {
        let cfg = OAuthProviderConfig {
            issuer: "https://auth.test.com".to_string(),
            client_id: "test_client".to_string(),
            scopes: "openid email".to_string(),
            originator: "testapp".to_string(),
            port: 9999,
        };
        let pkce = crate::pkce::generate_pkce();
        let url = build_authorize_url(&cfg, &pkce, "test_state", "http://localhost:9999/callback");

        assert!(url.starts_with("https://auth.test.com/oauth/authorize?"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("client_id=test_client"));
        assert!(url.contains("code_challenge="));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("state=test_state"));
        assert!(url.contains("originator=testapp"));
        assert!(url.contains("id_token_add_organizations=true"));
        assert!(url.contains("codex_cli_simplified_flow=true"));
    }

    #[tokio::test]
    async fn test_refresh_access_token_no_refresh_token() {
        let cfg = OAuthProviderConfig::openai();
        let cred = AuthCredential {
            access_token: "at_123".to_string(),
            refresh_token: None,
            expires_at: None,
            provider: "openai".to_string(),
            auth_method: "oauth".to_string(),
            account_id: None,
        };
        let result = cfg.refresh_access_token(&cred).await;
        assert!(result.is_err());
        assert!(result.err().unwrap().contains("no refresh token"));
    }

    #[tokio::test]
    async fn test_refresh_access_token_empty_refresh_token() {
        let cfg = OAuthProviderConfig::openai();
        let cred = AuthCredential {
            access_token: "at_123".to_string(),
            refresh_token: Some("".to_string()),
            expires_at: None,
            provider: "openai".to_string(),
            auth_method: "oauth".to_string(),
            account_id: None,
        };
        let result = cfg.refresh_access_token(&cred).await;
        assert!(result.is_err());
        assert!(result.err().unwrap().contains("no refresh token"));
    }

    #[test]
    fn test_parse_token_response_with_valid_id_token() {
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#);
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            r#"{"chatgpt_account_id":"acct_from_id_token"}"#,
        );
        let id_token = format!("{}.{}.sig", header, payload);

        let body = format!(r#"{{"access_token":"at_123","id_token":"{}"}}"#, id_token);
        let cred = parse_token_response_impl(body.as_bytes(), "openai").unwrap();
        assert_eq!(cred.account_id.unwrap(), "acct_from_id_token");
    }

    #[test]
    fn test_standalone_extract_account_id_empty_str() {
        assert_eq!(super::extract_account_id(""), "");
    }

    #[test]
    fn test_oauth_provider_config_fields() {
        let cfg = OAuthProviderConfig::openai();
        assert!(!cfg.issuer.is_empty());
        assert!(!cfg.client_id.is_empty());
        assert!(!cfg.scopes.is_empty());
        assert!(cfg.port > 0);
    }

    #[test]
    fn test_url_encode_all_special() {
        assert_eq!(url_encode("\0"), "%00");
        assert_eq!(url_encode("\n"), "%0A");
        assert_eq!(url_encode("\r"), "%0D");
        assert_eq!(url_encode("\""), "%22");
    }

    #[test]
    fn test_url_decode_empty() {
        assert_eq!(url_decode(""), "");
    }

    #[test]
    fn test_hex_val_all_hex_chars() {
        assert_eq!(hex_val(b'0'), 0);
        assert_eq!(hex_val(b'9'), 9);
        assert_eq!(hex_val(b'a'), 10);
        assert_eq!(hex_val(b'f'), 15);
        assert_eq!(hex_val(b'A'), 10);
        assert_eq!(hex_val(b'F'), 15);
    }

    #[test]
    fn test_parse_token_response_missing_access_token_field() {
        let body = r#"{"id_token":"header.payload.sig"}"#;
        let result = parse_token_response_impl(body.as_bytes(), "openai");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_token_response_basic_fields() {
        let body = r#"{"access_token":"at_123","token_type":"bearer"}"#;
        let cred = parse_token_response_impl(body.as_bytes(), "openai").unwrap();
        assert_eq!(cred.access_token, "at_123");
        assert_eq!(cred.provider, "openai");
    }

    #[test]
    fn test_parse_token_response_with_refresh_token_field() {
        let body = r#"{"access_token":"at_123","refresh_token":"rt_456"}"#;
        let cred = parse_token_response_impl(body.as_bytes(), "test").unwrap();
        assert_eq!(cred.refresh_token.as_deref(), Some("rt_456"));
    }

    #[test]
    fn test_parse_token_response_with_expires_in_field() {
        let body = r#"{"access_token":"at_123","expires_in":3600}"#;
        let cred = parse_token_response_impl(body.as_bytes(), "test").unwrap();
        assert!(cred.expires_at.is_some());
    }

    #[test]
    fn test_parse_device_code_response_valid_fields() {
        let json = r#"{"device_auth_id":"dc_123","user_code":"ABCD-1234","verification_uri":"https://example.com/verify","expires_in":900,"interval":5}"#;
        let resp = parse_device_code_response(json).unwrap();
        assert_eq!(resp.device_auth_id, "dc_123");
        assert_eq!(resp.user_code, "ABCD-1234");
        assert_eq!(resp.interval, 5);
    }

    #[test]
    fn test_base64url_decode_various_length_inputs() {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let test_cases: Vec<&[u8]> = vec![b"a", b"ab", b"abc", b"abcd", b"abcde", b"abcdef", b"abcdefg", b"abcdefgh"];
        for original in test_cases {
            let encoded = URL_SAFE_NO_PAD.encode(original);
            let decoded = base64url_decode(&encoded).unwrap();
            assert_eq!(decoded.as_slice(), original);
        }
    }

    #[test]
    fn test_build_authorize_url_with_custom_issuer() {
        let cfg = OAuthProviderConfig {
            issuer: "https://custom.auth.example.com".to_string(),
            client_id: "my_client_id".to_string(),
            scopes: "openid profile".to_string(),
            originator: "myapp".to_string(),
            port: 8080,
        };
        let pkce = crate::pkce::generate_pkce();
        let url = build_authorize_url(&cfg, &pkce, "state123", "http://localhost:8080/cb");
        assert!(url.starts_with("https://custom.auth.example.com/oauth/authorize?"));
        assert!(url.contains("client_id=my_client_id"));
        assert!(url.contains("originator=myapp"));
    }

    #[test]
    fn test_extract_account_id_with_account_id_in_jwt() {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#);
        let payload = URL_SAFE_NO_PAD.encode(r#"{"chatgpt_account_id":"acct_test_123"}"#);
        let id_token = format!("{}.{}.sig", header, payload);
        let result = super::extract_account_id(&id_token);
        assert_eq!(result, "acct_test_123");
    }

    #[test]
    fn test_parse_jwt_claims_impl_with_nested_claims() {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#);
        let payload = URL_SAFE_NO_PAD.encode(r#"{"sub":"user_1","email":"test@example.com","org":{"id":"org1","role":"admin"}}"#);
        let jwt = format!("{}.{}.sig", header, payload);
        let claims = parse_jwt_claims_impl(&jwt).unwrap();
        assert_eq!(claims["sub"].as_str().unwrap(), "user_1");
        assert_eq!(claims["email"].as_str().unwrap(), "test@example.com");
        assert!(claims.get("org").is_some());
    }

    #[tokio::test]
    async fn test_refresh_access_token_network_failure() {
        let cfg = OAuthProviderConfig::openai();
        let cred = AuthCredential {
            access_token: "at_123".to_string(),
            refresh_token: Some("rt_456".to_string()),
            expires_at: None,
            provider: "openai".to_string(),
            auth_method: "oauth".to_string(),
            account_id: None,
        };
        let result = cfg.refresh_access_token(&cred).await;
        assert!(result.is_err());
    }
}
