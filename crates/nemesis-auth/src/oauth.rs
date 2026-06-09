//! OAuth2 provider configuration and authentication flows.
//!
//! Supports browser-based OAuth, device code flow, and token refresh.

use crate::pkce::PkceCodes;
use crate::token::AuthCredential;

use base64::Engine;
use chrono::Local;
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
        Some(Local::now() + chrono::Duration::seconds(expires_in as i64))
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
        use std::os::windows::process::CommandExt;
        std::process::Command::new("cmd")
            .raw_arg(format!("/c start {}", url))
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
mod tests;
