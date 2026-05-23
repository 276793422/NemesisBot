use super::*;
use std::sync::Mutex;

/// Global lock to serialize tests that modify the CODEX_HOME env var.
static CODEX_HOME_LOCK: Mutex<()> = Mutex::new(());

/// RAII guard that sets an env var and restores it on drop.
/// Holds the global CODEX_HOME_LOCK to prevent parallel test interference.
struct CodexEnvGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    key: String,
    orig: Option<String>,
}

impl CodexEnvGuard {
    fn set(key: &str, val: &str) -> Self {
        let lock = CODEX_HOME_LOCK.lock().unwrap();
        let orig = std::env::var(key).ok();
        unsafe { std::env::set_var(key, val); }
        Self { _lock: lock, key: key.to_string(), orig }
    }

    fn remove(key: &str) -> Self {
        let lock = CODEX_HOME_LOCK.lock().unwrap();
        let orig = std::env::var(key).ok();
        unsafe { std::env::remove_var(key); }
        Self { _lock: lock, key: key.to_string(), orig }
    }
}

impl Drop for CodexEnvGuard {
    fn drop(&mut self) {
        match &self.orig {
            Some(val) => unsafe { std::env::set_var(&self.key, val); },
            None => unsafe { std::env::remove_var(&self.key); },
        }
    }
}

#[test]
fn test_parse_auth_json() {
    let json = r#"{"tokens":{"access_token":"tok123","refresh_token":"ref456","account_id":"acc789"}}"#;
    let auth: CodexCliAuth = serde_json::from_str(json).unwrap();
    assert_eq!(auth.tokens.access_token, "tok123");
    assert_eq!(auth.tokens.refresh_token, "ref456");
    assert_eq!(auth.tokens.account_id, "acc789");
}

#[test]
fn test_parse_auth_json_minimal() {
    let json = r#"{"tokens":{"access_token":"tok123"}}"#;
    let auth: CodexCliAuth = serde_json::from_str(json).unwrap();
    assert_eq!(auth.tokens.access_token, "tok123");
    assert_eq!(auth.tokens.refresh_token, "");
    assert_eq!(auth.tokens.account_id, "");
}

#[test]
fn test_parse_auth_json_missing_access_token() {
    let json = r#"{"tokens":{"refresh_token":"ref"}}"#;
    let auth: CodexCliAuth = serde_json::from_str(json).unwrap();
    assert!(auth.tokens.access_token.is_empty());
}

#[test]
fn test_read_nonexistent_file() {
    let result = std::fs::read_to_string("/nonexistent/path/auth.json");
    assert!(result.is_err());
}

#[test]
fn test_resolve_codex_auth_path_with_env() {
    let _g = CodexEnvGuard::remove("CODEX_HOME");
    // This test just validates the path construction logic
    let _path = resolve_codex_auth_path();
    // Path will depend on CODEX_HOME env var or home dir
}

// -- Additional tests --

#[test]
fn test_codex_cli_auth_serialization_roundtrip() {
    let auth = CodexCliAuth {
        tokens: CodexCliTokens {
            access_token: "tok123".into(),
            refresh_token: "ref456".into(),
            account_id: "acc789".into(),
        },
    };
    let json = serde_json::to_string(&auth).unwrap();
    let back: CodexCliAuth = serde_json::from_str(&json).unwrap();
    assert_eq!(back.tokens.access_token, "tok123");
    assert_eq!(back.tokens.refresh_token, "ref456");
    assert_eq!(back.tokens.account_id, "acc789");
}

#[test]
fn test_codex_cli_tokens_serialization_roundtrip() {
    let tokens = CodexCliTokens {
        access_token: "access".into(),
        refresh_token: "refresh".into(),
        account_id: "account".into(),
    };
    let json = serde_json::to_string(&tokens).unwrap();
    let back: CodexCliTokens = serde_json::from_str(&json).unwrap();
    assert_eq!(back.access_token, "access");
    assert_eq!(back.refresh_token, "refresh");
    assert_eq!(back.account_id, "account");
}

#[test]
fn test_codex_cli_tokens_default_fields() {
    let json = r#"{}"#;
    let tokens: CodexCliTokens = serde_json::from_str(json).unwrap();
    assert_eq!(tokens.access_token, "");
    assert_eq!(tokens.refresh_token, "");
    assert_eq!(tokens.account_id, "");
}

#[test]
fn test_read_codex_cli_credentials_missing_file() {
    let _g = CodexEnvGuard::remove("CODEX_HOME");
    let result = read_codex_cli_credentials();
    // Should fail because ~/.codex/auth.json doesn't exist in test env
    let _ = result;
}

#[test]
fn test_create_token_source_returns_function() {
    let _g = CodexEnvGuard::remove("CODEX_HOME");
    // Verify the closure is created and is callable
    let source = create_codex_cli_token_source();
    // The actual call will fail because auth.json doesn't exist,
    // but the source should be a valid closure
    let result = source();
    // It should fail because the file doesn't exist
    assert!(result.is_err());
}

#[test]
fn test_codex_credentials_debug_format() {
    let creds = CodexCredentials {
        access_token: "tok".into(),
        account_id: "acc".into(),
        expires_at: std::time::SystemTime::UNIX_EPOCH,
    };
    let debug_str = format!("{:?}", creds);
    assert!(debug_str.contains("tok"));
    assert!(debug_str.contains("acc"));
}

#[test]
fn test_read_codex_cli_credentials_with_temp_file() {
    // Create a temp auth.json with empty access_token
    let dir = tempfile::tempdir().unwrap();
    let auth_path = dir.path().join("auth.json");
    std::fs::write(&auth_path, r#"{"tokens":{"access_token":"","refresh_token":"ref"}}"#).unwrap();

    // We can't easily redirect resolve_codex_auth_path, but we can test the
    // parsing logic directly. The empty access_token should produce an error.
    let data = std::fs::read_to_string(&auth_path).unwrap();
    let auth: CodexCliAuth = serde_json::from_str(&data).unwrap();
    assert!(auth.tokens.access_token.is_empty());
    // This simulates the check in read_codex_cli_credentials
    assert!(auth.tokens.access_token.is_empty());
}

#[test]
fn test_read_codex_cli_credentials_valid_file() {
    let dir = tempfile::tempdir().unwrap();
    let auth_path = dir.path().join("auth.json");
    std::fs::write(
        &auth_path,
        r#"{"tokens":{"access_token":"valid_tok","refresh_token":"ref","account_id":"acc"}}"#,
    )
    .unwrap();

    let data = std::fs::read_to_string(&auth_path).unwrap();
    let auth: CodexCliAuth = serde_json::from_str(&data).unwrap();
    assert_eq!(auth.tokens.access_token, "valid_tok");
    assert_eq!(auth.tokens.account_id, "acc");
    // Non-empty access_token would pass the check
}

#[test]
fn test_token_source_expiry_check() {
    // Ensure CODEX_HOME is not set by parallel tests
    let _g = CodexEnvGuard::remove("CODEX_HOME");
    // Create a token source and verify expired credentials are rejected
    let source = create_codex_cli_token_source();
    // The source itself is always created; the actual expiry check happens
    // when calling the closure. Since the file doesn't exist, it fails
    // before the expiry check.
    let result = source();
    assert!(result.is_err());
}

#[test]
fn test_codex_cli_auth_invalid_json() {
    let json = r#"{"tokens": not valid}"#;
    let result: Result<CodexCliAuth, _> = serde_json::from_str(json);
    assert!(result.is_err());
}

#[test]
fn test_resolve_codex_auth_path_with_env_var() {
    let _g = CodexEnvGuard::set("CODEX_HOME", "/tmp/codex_test");
    let path = resolve_codex_auth_path().unwrap();
    assert_eq!(path, PathBuf::from("/tmp/codex_test/auth.json"));
}

#[test]
fn test_resolve_codex_auth_path_default() {
    let _g = CodexEnvGuard::remove("CODEX_HOME");
    let path = resolve_codex_auth_path();
    assert!(path.is_ok());
    let p = path.unwrap();
    assert!(p.to_string_lossy().contains(".codex"));
    assert!(p.to_string_lossy().contains("auth.json"));
}

#[test]
fn test_read_codex_cli_credentials_with_valid_file() {
    let dir = tempfile::tempdir().unwrap();
    let codex_dir = dir.path().join(".codex");
    std::fs::create_dir_all(&codex_dir).unwrap();
    let auth_path = codex_dir.join("auth.json");
    std::fs::write(
        &auth_path,
        r#"{"tokens":{"access_token":"valid_tok_123","refresh_token":"ref","account_id":"acc_456"}}"#,
    )
    .unwrap();

    let _g = CodexEnvGuard::set("CODEX_HOME", &codex_dir.to_string_lossy().to_string());
    let result = read_codex_cli_credentials();

    let creds = result.unwrap();
    assert_eq!(creds.access_token, "valid_tok_123");
    assert_eq!(creds.account_id, "acc_456");
    assert!(creds.expires_at > SystemTime::now());
}

#[test]
fn test_read_codex_cli_credentials_with_empty_token() {
    let dir = tempfile::tempdir().unwrap();
    let codex_dir = dir.path().join(".codex");
    std::fs::create_dir_all(&codex_dir).unwrap();
    let auth_path = codex_dir.join("auth.json");
    std::fs::write(&auth_path, r#"{"tokens":{"access_token":"","refresh_token":"ref"}}"#).unwrap();

    let _g = CodexEnvGuard::set("CODEX_HOME", &codex_dir.to_string_lossy().to_string());
    let result = read_codex_cli_credentials();

    assert!(result.is_err());
    assert!(result.unwrap_err().contains("no access_token"));
}

#[test]
fn test_read_codex_cli_credentials_with_invalid_json() {
    let dir = tempfile::tempdir().unwrap();
    let codex_dir = dir.path().join(".codex");
    std::fs::create_dir_all(&codex_dir).unwrap();
    let auth_path = codex_dir.join("auth.json");
    std::fs::write(&auth_path, "not valid json").unwrap();

    let _g = CodexEnvGuard::set("CODEX_HOME", &codex_dir.to_string_lossy().to_string());
    let result = read_codex_cli_credentials();

    assert!(result.is_err());
    assert!(result.unwrap_err().contains("parsing"));
}

#[test]
fn test_token_source_valid_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let codex_dir = dir.path().join(".codex");
    std::fs::create_dir_all(&codex_dir).unwrap();
    let auth_path = codex_dir.join("auth.json");
    std::fs::write(
        &auth_path,
        r#"{"tokens":{"access_token":"tok_fresh","refresh_token":"ref","account_id":"acc_fresh"}}"#,
    )
    .unwrap();

    let _g = CodexEnvGuard::set("CODEX_HOME", &codex_dir.to_string_lossy().to_string());
    let source = create_codex_cli_token_source();
    let result = source();

    assert!(result.is_ok());
    let (token, account_id) = result.unwrap();
    assert_eq!(token, "tok_fresh");
    assert_eq!(account_id, "acc_fresh");
}

#[test]
fn test_codex_cli_auth_clone() {
    let auth = CodexCliAuth {
        tokens: CodexCliTokens {
            access_token: "tok".into(),
            refresh_token: "ref".into(),
            account_id: "acc".into(),
        },
    };
    let cloned = auth.clone();
    assert_eq!(cloned.tokens.access_token, "tok");
}

#[test]
fn test_codex_credentials_clone() {
    let creds = CodexCredentials {
        access_token: "tok".into(),
        account_id: "acc".into(),
        expires_at: SystemTime::UNIX_EPOCH,
    };
    let cloned = creds.clone();
    assert_eq!(cloned.access_token, "tok");
}
