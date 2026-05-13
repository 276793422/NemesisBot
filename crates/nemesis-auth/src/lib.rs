//! NemesisBot Auth Module
//!
//! OAuth2 authentication with PKCE, device code flow, token management,
//! and credential storage.

pub mod oauth;
pub mod pkce;
pub mod store;
pub mod token;

pub use oauth::{
    OAuthProviderConfig, DeviceCodeResponse,
    open_ai_oauth_config, login_browser, login_device_code,
    poll_device_code, refresh_access_token, build_authorize_url,
    exchange_code_for_tokens, parse_token_response, extract_account_id,
    parse_jwt_claims, open_browser,
};
pub use pkce::PkceCodes;
pub use store::AuthStore;
pub use token::{provider_display_name, AuthCredential};
