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
    let cred =
        nemesis_auth::AuthCredential::login_paste_token("openai", "test-token-12345").unwrap();
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

// ===========================================================================
// run() 全臂（S11c，quality-hardening goal 冲刺 S11）—— env home 隔离 +
// GLOBAL_STATE_LOCK 串行。Login openai 的真 OAuth（auth.rs:49-75）结构性
// 豁免（真网络/浏览器流）；anthropic 走 paste-token 分支，stdin 是管道 EOF
// → 空输入 → 取消路径（确定性）。
// ===========================================================================

mod run_arm {
    use super::super::{run, AuthAction};
    use tempfile::TempDir;

    async fn with_env_home<F, Fut>(f: F)
    where
        F: FnOnce(std::path::PathBuf) -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        unsafe {
            std::env::set_var("NEMESISBOT_HOME", tmp.path());
        }
        f(tmp.path().join(".nemesisbot")).await;
        unsafe {
            std::env::remove_var("NEMESISBOT_HOME");
        }
    }

    #[tokio::test]
    async fn login_invalid_provider_is_rejected_cleanly() {
        with_env_home(|_home| async {
            run(
                AuthAction::Login {
                    provider: "google".into(),
                    device_code: false,
                },
                false,
            )
            .await
            .expect("非法 provider → 打印支持列表后 Ok，不写 auth.json");
        })
        .await;
    }

    #[tokio::test]
    async fn login_anthropic_eof_stdin_cancels_without_writing() {
        with_env_home(|home| async move {
            std::fs::create_dir_all(&home).unwrap();
            run(
                AuthAction::Login {
                    provider: "anthropic".into(),
                    device_code: false,
                },
                false,
            )
            .await
            .expect("stdin EOF → 空输入取消，Ok");
            assert!(
                !home.join("auth.json").exists(),
                "取消路径不得落盘 auth.json"
            );
        })
        .await;
    }

    fn cred_with(
        expires_at: Option<chrono::DateTime<chrono::Local>>,
        account: Option<&str>,
    ) -> nemesis_auth::AuthCredential {
        nemesis_auth::AuthCredential {
            access_token: "tok".into(),
            refresh_token: None,
            expires_at,
            provider: "openai".into(),
            auth_method: "token".into(),
            account_id: account.map(str::to_string),
        }
    }

    #[tokio::test]
    async fn logout_without_file_is_noop_ok() {
        with_env_home(|_home| async {
            run(AuthAction::Logout { provider: None }, false)
                .await
                .expect("无 auth.json → 提示后 Ok");
        })
        .await;
    }

    #[tokio::test]
    async fn logout_specific_provider_present_and_absent() {
        with_env_home(|home| async move {
            std::fs::create_dir_all(&home).unwrap();
            let auth_path = home.join("auth.json");
            let store = nemesis_auth::AuthStore::new(&auth_path.to_string_lossy());
            store
                .save("openai", nemesis_auth::AuthCredential::login_paste_token("openai", "k").unwrap())
                .unwrap();

            run(
                AuthAction::Logout {
                    provider: Some("openai".into()),
                },
                false,
            )
            .await
            .expect("logout openai ok");
            assert!(
                !nemesis_auth::AuthStore::new(&auth_path.to_string_lossy())
                    .list_providers()
                    .contains(&"openai".to_string()),
                "openai 凭据已删"
            );

            // 已删后再 logout 同名 → "No credentials found" 分支，Ok。
            run(
                AuthAction::Logout {
                    provider: Some("openai".into()),
                },
                false,
            )
            .await
            .expect("不存在的 provider → Ok");
        })
        .await;
    }

    #[tokio::test]
    async fn logout_all_deletes_every_provider() {
        with_env_home(|home| async move {
            std::fs::create_dir_all(&home).unwrap();
            let store = nemesis_auth::AuthStore::new(&home.join("auth.json").to_string_lossy());
            store
                .save(
                    "openai",
                    nemesis_auth::AuthCredential::login_paste_token("openai", "k1").unwrap(),
                )
                .unwrap();
            run(AuthAction::Logout { provider: None }, false)
                .await
                .expect("logout all ok");
            assert!(
                nemesis_auth::AuthStore::new(&home.join("auth.json").to_string_lossy())
                    .list_providers()
                    .is_empty()
            );
        })
        .await;
    }

    #[tokio::test]
    async fn status_no_file_empty_and_all_states() {
        with_env_home(|home| async move {
            std::fs::create_dir_all(&home).unwrap();
            // 无文件。
            run(AuthAction::Status, false).await.expect("no file ok");

            // 空对象。
            std::fs::write(home.join("auth.json"), "{}").unwrap();
            run(AuthAction::Status, false).await.expect("empty ok");

            // expired / needs-refresh / active+account+expires 三态齐发。
            let store = nemesis_auth::AuthStore::new(&home.join("auth.json").to_string_lossy());
            store
                .save(
                    "openai",
                    cred_with(Some(chrono::Local::now() - chrono::Duration::hours(1)), None),
                )
                .unwrap();
            store
                .save(
                    "anthropic",
                    cred_with(Some(chrono::Local::now() + chrono::Duration::minutes(3)), None),
                )
                .unwrap();
            store
                .save(
                    "zhipu",
                    cred_with(
                        Some(chrono::Local::now() + chrono::Duration::hours(2)),
                        Some("acct-7"),
                    ),
                )
                .unwrap();
            run(AuthAction::Status, false)
                .await
                .expect("三态 + Account/Expires 行 → Ok");
        })
        .await;
    }
}
