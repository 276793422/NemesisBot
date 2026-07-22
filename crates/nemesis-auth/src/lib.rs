//! NemesisBot Auth Module
//!
//! OAuth2 authentication with PKCE, device code flow, token management,
//! and credential storage.

pub mod oauth;
pub mod pkce;
pub mod store;
pub mod token;

pub use oauth::{
    DeviceCodeResponse, OAuthProviderConfig, build_authorize_url, exchange_code_for_tokens,
    extract_account_id, login_browser, login_device_code, open_ai_oauth_config, open_browser,
    parse_jwt_claims, parse_token_response, poll_device_code, refresh_access_token,
};
pub use pkce::PkceCodes;
pub use store::AuthStore;
pub use token::{AuthCredential, provider_display_name};
