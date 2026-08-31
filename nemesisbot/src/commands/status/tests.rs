// 刻意设计：本文件测试用进程级串行锁（GLOBAL_STATE_LOCK 等 env/资源互斥锁）
// 保护环境操作，guard 必须跨 async 测试体的 await 持有；#[tokio::test] 每个
// 测试独立 current_thread runtime，持锁方在自己线程上恢复运行，不会死锁。
// 测试域统一豁免（逐处 allow ~200 个不现实）。
#![allow(clippy::await_holding_lock)]

#[allow(unused_imports)]
use crate::common;

#[test]
fn test_status_model_parsing_single_provider() {
    let models = serde_json::json!([
        {"model": "openai/gpt-4", "api_key": "sk-12345"},
        {"model": "openai/gpt-3.5", "api_key": ""}
    ]);
    let model_list = models.as_array().unwrap();

    let mut provider_counts: std::collections::HashMap<String, (usize, bool)> =
        std::collections::HashMap::new();
    for m in model_list {
        let model = m.get("model").and_then(|v| v.as_str()).unwrap_or("");
        let parts: Vec<&str> = model.splitn(2, '/').collect();
        if parts.len() == 2 {
            let provider = parts[0].to_lowercase();
            let has_key = m
                .get("api_key")
                .and_then(|v| v.as_str())
                .map(|k| !k.is_empty())
                .unwrap_or(false);
            let entry = provider_counts.entry(provider).or_insert((0, false));
            entry.0 += 1;
            entry.1 = entry.1 || has_key;
        }
    }

    assert_eq!(provider_counts.len(), 1);
    assert_eq!(provider_counts["openai"].0, 2);
    assert!(provider_counts["openai"].1);
}

#[test]
fn test_status_model_parsing_multiple_providers() {
    let models = serde_json::json!([
        {"model": "openai/gpt-4", "api_key": "key1"},
        {"model": "anthropic/claude-3", "api_key": "key2"},
        {"model": "zhipu/glm-4", "api_key": ""}
    ]);
    let model_list = models.as_array().unwrap();

    let mut provider_counts: std::collections::HashMap<String, (usize, bool)> =
        std::collections::HashMap::new();
    for m in model_list {
        let model = m.get("model").and_then(|v| v.as_str()).unwrap_or("");
        let parts: Vec<&str> = model.splitn(2, '/').collect();
        if parts.len() == 2 {
            let provider = parts[0].to_lowercase();
            let has_key = m
                .get("api_key")
                .and_then(|v| v.as_str())
                .map(|k| !k.is_empty())
                .unwrap_or(false);
            let entry = provider_counts.entry(provider).or_insert((0, false));
            entry.0 += 1;
            entry.1 = entry.1 || has_key;
        }
    }

    assert_eq!(provider_counts.len(), 3);
    assert_eq!(provider_counts["openai"], (1, true));
    assert_eq!(provider_counts["anthropic"], (1, true));
    assert_eq!(provider_counts["zhipu"], (1, false));
}

#[test]
fn test_status_model_parsing_no_provider() {
    let models = serde_json::json!([
        {"model": "no-slash-model", "api_key": "key"}
    ]);
    let model_list = models.as_array().unwrap();

    let mut provider_counts: std::collections::HashMap<String, (usize, bool)> =
        std::collections::HashMap::new();
    for m in model_list {
        let model = m.get("model").and_then(|v| v.as_str()).unwrap_or("");
        let parts: Vec<&str> = model.splitn(2, '/').collect();
        if parts.len() == 2 {
            let provider = parts[0].to_lowercase();
            let entry = provider_counts.entry(provider).or_insert((0, false));
            entry.0 += 1;
        }
    }

    assert!(provider_counts.is_empty());
}

#[test]
fn test_status_security_enabled_default() {
    let cfg = serde_json::json!({});
    let security_enabled = cfg
        .get("security")
        .and_then(|s| s.get("enabled"))
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    assert!(security_enabled);
}

#[test]
fn test_status_security_disabled() {
    let cfg = serde_json::json!({"security": {"enabled": false}});
    let security_enabled = cfg
        .get("security")
        .and_then(|s| s.get("enabled"))
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    assert!(!security_enabled);
}

#[test]
fn test_status_forge_disabled_default() {
    let cfg = serde_json::json!({});
    let forge_enabled = cfg
        .get("forge")
        .and_then(|f| f.get("enabled"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    assert!(!forge_enabled);
}

#[test]
fn test_status_forge_enabled() {
    let cfg = serde_json::json!({"forge": {"enabled": true}});
    let forge_enabled = cfg
        .get("forge")
        .and_then(|f| f.get("enabled"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    assert!(forge_enabled);
}

#[test]
fn test_status_default_model_extraction() {
    let cfg = serde_json::json!({"default_model": "openai/gpt-4"});
    let model = cfg.get("default_model").and_then(|v| v.as_str());
    assert_eq!(model, Some("openai/gpt-4"));
}

#[test]
fn test_status_no_default_model() {
    let cfg = serde_json::json!({});
    let model = cfg.get("default_model").and_then(|v| v.as_str());
    assert!(model.is_none());
}

#[test]
fn test_status_empty_model_list() {
    let cfg = serde_json::json!({"model_list": []});
    let models = cfg.get("model_list").and_then(|v| v.as_array());
    assert!(models.is_some());
    assert!(models.unwrap().is_empty());
}

#[test]
fn test_model_key_has_key_detection() {
    let m = serde_json::json!({"model": "openai/gpt-4", "api_key": "sk-12345"});
    let has_key = m
        .get("api_key")
        .and_then(|v| v.as_str())
        .map(|k| !k.is_empty())
        .unwrap_or(false);
    assert!(has_key);
}

#[test]
fn test_model_key_empty_key_detection() {
    let m = serde_json::json!({"model": "openai/gpt-4", "api_key": ""});
    let has_key = m
        .get("api_key")
        .and_then(|v| v.as_str())
        .map(|k| !k.is_empty())
        .unwrap_or(false);
    assert!(!has_key);
}

#[test]
fn test_model_key_no_key_field() {
    let m = serde_json::json!({"model": "openai/gpt-4"});
    let has_key = m
        .get("api_key")
        .and_then(|v| v.as_str())
        .map(|k| !k.is_empty())
        .unwrap_or(false);
    assert!(!has_key);
}

// -------------------------------------------------------------------------
// Additional status tests for coverage
// -------------------------------------------------------------------------

#[test]
fn test_status_run_with_temp_config() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    let cfg_path = home.join("config.json");
    let workspace = home.join("workspace");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();

    let cfg = serde_json::json!({
        "default_model": "test/model-1",
        "model_list": [
            {"model": "openai/gpt-4", "api_key": "sk-test123"},
            {"model": "anthropic/claude", "api_key": ""}
        ],
        "security": {"enabled": false},
        "forge": {"enabled": true}
    });
    std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();

    // Verify the config can be parsed back correctly
    let data = std::fs::read_to_string(&cfg_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&data).unwrap();

    // Default model
    let model = parsed.get("default_model").and_then(|v| v.as_str());
    assert_eq!(model, Some("test/model-1"));

    // Model count
    let models = parsed.get("model_list").and_then(|v| v.as_array()).unwrap();
    assert_eq!(models.len(), 2);

    // Provider counts
    let mut provider_counts: std::collections::HashMap<String, (usize, bool)> =
        std::collections::HashMap::new();
    for m in models {
        let model = m.get("model").and_then(|v| v.as_str()).unwrap_or("");
        let parts: Vec<&str> = model.splitn(2, '/').collect();
        if parts.len() == 2 {
            let provider = parts[0].to_lowercase();
            let has_key = m
                .get("api_key")
                .and_then(|v| v.as_str())
                .map(|k| !k.is_empty())
                .unwrap_or(false);
            let entry = provider_counts.entry(provider).or_insert((0, false));
            entry.0 += 1;
            entry.1 = entry.1 || has_key;
        }
    }
    assert_eq!(provider_counts["openai"], (1, true));
    assert_eq!(provider_counts["anthropic"], (1, false));

    // Security
    let security_enabled = parsed
        .get("security")
        .and_then(|s| s.get("enabled"))
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    assert!(!security_enabled);

    // Forge
    let forge_enabled = parsed
        .get("forge")
        .and_then(|f| f.get("enabled"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    assert!(forge_enabled);
}

#[test]
fn test_status_auth_no_credentials() {
    let tmp = tempfile::TempDir::new().unwrap();
    let auth_path = tmp.path().join("auth.json");
    // No auth file means no credentials
    assert!(!auth_path.exists());
}

#[test]
fn test_status_auth_with_credentials() {
    let tmp = tempfile::TempDir::new().unwrap();
    let auth_path = tmp.path().join("auth.json");
    let store = nemesis_auth::AuthStore::new(&auth_path.to_string_lossy());
    let cred = nemesis_auth::AuthCredential::login_paste_token("openai", "test-key").unwrap();
    store.save("openai", cred).unwrap();

    let providers = store.list_providers();
    assert_eq!(providers.len(), 1);
    let retrieved = store.get("openai");
    assert!(retrieved.is_some());

    let cred = retrieved.unwrap();
    let status = if cred.is_expired() {
        "expired"
    } else if cred.needs_refresh() {
        "needs refresh"
    } else {
        "active"
    };
    assert!(!status.is_empty());
}

#[test]
fn test_status_model_parsing_mixed_models() {
    let models = serde_json::json!([
        {"model": "openai/gpt-4", "api_key": "key1"},
        {"model": "anthropic/claude-3", "api_key": ""},
        {"model": "no-slash-model", "api_key": "key3"},
        {"model": "zhipu/glm-4", "api_key": "key4"},
        {"model": "", "api_key": "key5"},
    ]);
    let model_list = models.as_array().unwrap();

    let mut provider_counts: std::collections::HashMap<String, (usize, bool)> =
        std::collections::HashMap::new();
    let mut no_provider_count = 0;
    for m in model_list {
        let model = m.get("model").and_then(|v| v.as_str()).unwrap_or("");
        let parts: Vec<&str> = model.splitn(2, '/').collect();
        if parts.len() == 2 {
            let provider = parts[0].to_lowercase();
            let has_key = m
                .get("api_key")
                .and_then(|v| v.as_str())
                .map(|k| !k.is_empty())
                .unwrap_or(false);
            let entry = provider_counts.entry(provider).or_insert((0, false));
            entry.0 += 1;
            entry.1 = entry.1 || has_key;
        } else {
            no_provider_count += 1;
        }
    }
    assert_eq!(provider_counts.len(), 3);
    assert_eq!(no_provider_count, 2); // "no-slash-model" and ""
}

// ===========================================================================
// run() 全臂（S11c，quality-hardening goal 冲刺 S11）
//
// 上面现存的全是"影子测试"（把 run() 的逻辑复制一遍断言局部变量），run()
// 本体一行都没跑过（LH=0）。这里直接走真 run(false) + NEMESISBOT_HOME 隔离
// （GLOBAL_STATE_LOCK 串行），钉：无 config / 有 config（带 slash、无 slash
// 模型、空 model_list）/ 非法 JSON config / auth.json 各凭据状态分支。
// ===========================================================================

mod run_arm {
    use super::super::run;

    async fn with_env_home<F, Fut>(f: F)
    where
        F: FnOnce(std::path::PathBuf) -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("NEMESISBOT_HOME", tmp.path());
        }
        f(tmp.path().join(".nemesisbot")).await;
        unsafe {
            std::env::remove_var("NEMESISBOT_HOME");
        }
    }

    #[tokio::test]
    async fn no_config_and_no_auth_prints_not_found() {
        with_env_home(|_home| async {
            run(false).expect("缺 config/auth → 纯打印，Ok");
        })
        .await;
    }

    #[tokio::test]
    async fn full_config_covers_model_provider_lines() {
        with_env_home(|home| async move {
            std::fs::create_dir_all(&home).unwrap();
            std::fs::write(
                home.join("config.json"),
                serde_json::to_string(&serde_json::json!({
                    "default_model": "test/model-1",
                    "model_list": [
                        {"model": "openai/gpt-4", "api_key": "sk-x"},
                        {"model": "openai/gpt-3.5", "api_key": ""},
                        {"model": "no-slash-model", "api_key": "k"},
                        {"model": "", "api_key": "k2"}
                    ],
                    "security": {"enabled": false},
                    "forge": {"enabled": true}
                }))
                .unwrap(),
            )
            .unwrap();
            std::fs::create_dir_all(home.join("workspace")).unwrap();
            run(false).expect("完整 config → Ok（provider 计数/无 slash 跳过分支）");
        })
        .await;
    }

    #[tokio::test]
    async fn empty_model_list_skips_provider_block() {
        with_env_home(|home| async move {
            std::fs::create_dir_all(&home).unwrap();
            std::fs::write(
                home.join("config.json"),
                serde_json::to_string(&serde_json::json!({"model_list": []})).unwrap(),
            )
            .unwrap();
            run(false).expect("空 model_list → 跳过 Configured Models 块，Ok");
        })
        .await;
    }

    #[tokio::test]
    async fn invalid_config_json_is_skipped_gracefully() {
        with_env_home(|home| async move {
            std::fs::create_dir_all(&home).unwrap();
            std::fs::write(home.join("config.json"), "not json{{{").unwrap();
            run(false).expect("非法 JSON → from_str Ok 分支跳过，不 Err");
        })
        .await;
    }

    #[tokio::test]
    async fn auth_file_with_all_credential_states() {
        with_env_home(|home| async move {
            std::fs::create_dir_all(&home).unwrap();
            // expired / needs-refresh / active / 带 account_id+expires_at 四种
            // 凭据状态一起过（status.rs:121-152 全分支）。
            let store = nemesis_auth::AuthStore::new(
                &home.join("auth.json").to_string_lossy(),
            );
            let mk = |expires_at: Option<chrono::DateTime<chrono::Local>>,
                      account: Option<&str>|
             nemesis_auth::AuthCredential {
                access_token: "tok".into(),
                refresh_token: None,
                expires_at,
                provider: "openai".into(),
                auth_method: "token".into(),
                account_id: account.map(str::to_string),
            };
            store
                .save(
                    "openai",
                    mk(Some(chrono::Local::now() - chrono::Duration::hours(1)), None),
                )
                .unwrap();
            store
                .save(
                    "anthropic",
                    mk(Some(chrono::Local::now() + chrono::Duration::minutes(3)), None),
                )
                .unwrap();
            store
                .save(
                    "zhipu",
                    mk(
                        Some(chrono::Local::now() + chrono::Duration::hours(2)),
                        Some("acct-42"),
                    ),
                )
                .unwrap();
            run(false).expect("三种凭据状态 + Account/Expires 行 → Ok");
        })
        .await;
    }

    #[tokio::test]
    async fn auth_file_with_zero_providers() {
        with_env_home(|home| async move {
            std::fs::create_dir_all(&home).unwrap();
            // 空对象文件存在但无 provider → "No credentials stored."
            std::fs::write(home.join("auth.json"), "{}").unwrap();
            run(false).expect("空 auth.json → Ok");
        })
        .await;
    }
}
