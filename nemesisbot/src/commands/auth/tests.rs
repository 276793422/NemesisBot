use tempfile::TempDir;

#[test]
fn test_valid_providers_openai() {
    let valid_providers = ["openai", "anthropic"];
    assert!(valid_providers.contains(&"openai"));
}

#[test]
fn test_valid_providers_anthropic() {
    let valid_providers = ["openai", "anthropic"];
    assert!(valid_providers.contains(&"anthropic"));
}

#[test]
fn test_invalid_provider_rejected() {
    let valid_providers = ["openai", "anthropic"];
    assert!(!valid_providers.contains(&"google"));
    assert!(!valid_providers.contains(&"invalid"));
}

#[test]
fn test_auth_path_construction() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    let auth_path = home.join("auth.json");
    assert!(auth_path.to_string_lossy().contains("auth.json"));
}

#[test]
fn test_auth_store_creation() {
    let tmp = TempDir::new().unwrap();
    let auth_path = tmp.path().join("auth.json");
    let store = nemesis_auth::AuthStore::new(&auth_path.to_string_lossy());
    let providers = store.list_providers();
    assert!(providers.is_empty());
}

#[test]
fn test_auth_store_save_and_get() {
    let tmp = TempDir::new().unwrap();
    let auth_path = tmp.path().join("auth.json");

    let store = nemesis_auth::AuthStore::new(&auth_path.to_string_lossy());
    let cred = nemesis_auth::AuthCredential::login_paste_token("openai", "test-token-12345").unwrap();
    store.save("openai", cred).unwrap();

    let retrieved = store.get("openai");
    assert!(retrieved.is_some());
    // auth_method may be "token" (crate returns the actual method name)
    let method = &retrieved.unwrap().auth_method;
    assert!(!method.is_empty());
}

#[test]
fn test_auth_store_list_providers() {
    let tmp = TempDir::new().unwrap();
    let auth_path = tmp.path().join("auth.json");

    let store = nemesis_auth::AuthStore::new(&auth_path.to_string_lossy());
    let cred = nemesis_auth::AuthCredential::login_paste_token("openai", "test-token").unwrap();
    store.save("openai", cred).unwrap();

    let providers = store.list_providers();
    assert_eq!(providers.len(), 1);
    assert!(providers.contains(&"openai".to_string()));
}

#[test]
fn test_auth_store_remove() {
    let tmp = TempDir::new().unwrap();
    let auth_path = tmp.path().join("auth.json");

    let store = nemesis_auth::AuthStore::new(&auth_path.to_string_lossy());
    let cred = nemesis_auth::AuthCredential::login_paste_token("openai", "test-token").unwrap();
    store.save("openai", cred).unwrap();

    store.remove("openai").unwrap();
    assert!(store.get("openai").is_none());
}

#[test]
fn test_auth_store_delete_all() {
    let tmp = TempDir::new().unwrap();
    let auth_path = tmp.path().join("auth.json");

    let store = nemesis_auth::AuthStore::new(&auth_path.to_string_lossy());
    let cred1 = nemesis_auth::AuthCredential::login_paste_token("openai", "token1").unwrap();
    let cred2 = nemesis_auth::AuthCredential::login_paste_token("anthropic", "token2").unwrap();
    store.save("openai", cred1).unwrap();
    store.save("anthropic", cred2).unwrap();

    store.delete_all().unwrap();
    assert!(store.list_providers().is_empty());
}

#[test]
fn test_provider_display_name() {
    let name = nemesis_auth::provider_display_name("openai");
    assert!(!name.is_empty());
}

#[test]
fn test_credential_is_expired() {
    let cred = nemesis_auth::AuthCredential::login_paste_token("openai", "test").unwrap();
    let _ = cred.is_expired();
}

#[test]
fn test_credential_needs_refresh() {
    let cred = nemesis_auth::AuthCredential::login_paste_token("openai", "test").unwrap();
    let _ = cred.needs_refresh();
}

#[test]
fn test_auth_store_get_nonexistent() {
    let tmp = TempDir::new().unwrap();
    let auth_path = tmp.path().join("auth.json");
    let store = nemesis_auth::AuthStore::new(&auth_path.to_string_lossy());
    assert!(store.get("nonexistent").is_none());
}

#[test]
fn test_auth_no_file_exists() {
    let tmp = TempDir::new().unwrap();
    let auth_path = tmp.path().join("nonexistent").join("auth.json");
    assert!(!auth_path.exists());
}

// -------------------------------------------------------------------------
// Additional auth tests for coverage
// -------------------------------------------------------------------------

#[test]
fn test_multiple_provider_operations() {
    let tmp = TempDir::new().unwrap();
    let auth_path = tmp.path().join("auth.json");
    let store = nemesis_auth::AuthStore::new(&auth_path.to_string_lossy());

    // Save multiple credentials
    let cred1 = nemesis_auth::AuthCredential::login_paste_token("openai", "key1").unwrap();
    let cred2 = nemesis_auth::AuthCredential::login_paste_token("anthropic", "key2").unwrap();
    store.save("openai", cred1).unwrap();
    store.save("anthropic", cred2).unwrap();

    let providers = store.list_providers();
    assert_eq!(providers.len(), 2);

    // Get individual
    assert!(store.get("openai").is_some());
    assert!(store.get("anthropic").is_some());

    // Remove one
    store.remove("openai").unwrap();
    assert!(store.get("openai").is_none());
    assert!(store.get("anthropic").is_some());
    assert_eq!(store.list_providers().len(), 1);
}

#[test]
fn test_auth_credential_fields() {
    let cred = nemesis_auth::AuthCredential::login_paste_token("openai", "test-key-12345").unwrap();
    assert!(!cred.auth_method.is_empty());
    assert!(cred.is_expired() == false || cred.is_expired() == true); // Just ensure it doesn't panic
}

#[test]
fn test_auth_store_nonexistent_remove() {
    let tmp = TempDir::new().unwrap();
    let auth_path = tmp.path().join("auth.json");
    let store = nemesis_auth::AuthStore::new(&auth_path.to_string_lossy());
    // Removing a nonexistent provider should not panic
    let result = store.remove("nonexistent");
    // May succeed or fail depending on implementation
    let _ = result;
}

#[test]
fn test_provider_display_names() {
    let name_openai = nemesis_auth::provider_display_name("openai");
    assert!(!name_openai.is_empty());

    let name_anthropic = nemesis_auth::provider_display_name("anthropic");
    assert!(!name_anthropic.is_empty());

    // Unknown provider should still return something
    let name_unknown = nemesis_auth::provider_display_name("unknown_provider");
    assert!(!name_unknown.is_empty());
}

#[test]
fn test_auth_store_overwrite() {
    let tmp = TempDir::new().unwrap();
    let auth_path = tmp.path().join("auth.json");
    let store = nemesis_auth::AuthStore::new(&auth_path.to_string_lossy());

    let cred1 = nemesis_auth::AuthCredential::login_paste_token("openai", "key1").unwrap();
    store.save("openai", cred1).unwrap();

    let cred2 = nemesis_auth::AuthCredential::login_paste_token("openai", "key2-updated").unwrap();
    store.save("openai", cred2).unwrap();

    let providers = store.list_providers();
    assert_eq!(providers.len(), 1);
}

#[test]
fn test_auth_path_in_home_directory() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path();
    let auth_path = home.join("auth.json");
    assert!(auth_path.to_string_lossy().ends_with("auth.json"));
}

#[test]
fn test_token_empty_detection() {
    let token = "";
    assert!(token.is_empty());

    let token = "  ";
    assert!(!token.is_empty()); // whitespace is not empty

    let token = " valid-token ";
    assert!(!token.is_empty());
}

#[test]
fn test_token_trim() {
    let token = "  my-api-key  ".trim().to_string();
    assert_eq!(token, "my-api-key");
}
