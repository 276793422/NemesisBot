// 刻意设计：本文件测试用进程级串行锁（GLOBAL_STATE_LOCK 等 env/资源互斥锁）
// 保护环境操作，guard 必须跨 async 测试体的 await 持有；#[tokio::test] 每个
// 测试独立 current_thread runtime，持锁方在自己线程上恢复运行，不会死锁。
// 测试域统一豁免（逐处 allow ~200 个不现实）。
#![allow(clippy::await_holding_lock)]

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
    assert!(!cred.is_expired() || cred.is_expired()); // Just ensure it doesn't panic
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

// ===========================================================================
// r9_zero（R9 补测批零头组，2026-08-27）：openai 真链路 login 子进程双臂。
// 此前豁免理由是「真网络/浏览器流」；两个确定性夹具拆除该豁免：
//   ① 回调端口预占 trick —— oauth.rs 的 login_browser 在打开任何浏览器之前
//     （oauth.rs:57 open_browser_impl）先 bind 127.0.0.1:1455（oauth.rs:53），
//     我们抢先把 1455 占死 → bind 必败 → 立即 Err 走 paste-token 回落，
//     浏览器从头到尾不会被拉起，零外部副作用。
//   ② issuer 接缝 + 死端点 —— NEMESISBOT_OAUTH_ISSUER 指 127.0.0.1:9（discard，
//     本机必拒），device-code 流第一步 POST {issuer}/api/accounts/deviceauth/
//     usercode 立即连接拒绝 → 同样回落；stdin 只喂 "\n" → 空 token 取消臂。
// ===========================================================================

mod r9_zero {
    use test_harness::{resolve_nemesisbot_bin, TestWorkspace};

    /// 单个环境变量的保存/移除/Drop 恢复。必须在持有
    /// crate::GLOBAL_STATE_LOCK 期间使用（子进程在 spawn 时继承 env）。
    struct EnvVarRemoved {
        name: &'static str,
        saved: Option<String>,
    }

    impl EnvVarRemoved {
        fn take(name: &'static str) -> Self {
            let saved = std::env::var(name).ok();
            unsafe {
                std::env::remove_var(name);
            }
            Self { name, saved }
        }
    }

    impl Drop for EnvVarRemoved {
        fn drop(&mut self) {
            unsafe {
                match self.saved.take() {
                    Some(v) => std::env::set_var(self.name, v),
                    None => std::env::remove_var(self.name),
                }
            }
        }
    }

    /// 同上，但方向相反：设置一个值并在 Drop 时恢复旧值。
    struct EnvVarSet {
        name: &'static str,
        saved: Option<String>,
    }

    impl EnvVarSet {
        fn set(name: &'static str, val: &str) -> Self {
            let saved = std::env::var(name).ok();
            unsafe {
                std::env::set_var(name, val);
            }
            Self { name, saved }
        }
    }

    impl Drop for EnvVarSet {
        fn drop(&mut self) {
            unsafe {
                match self.saved.take() {
                    Some(v) => std::env::set_var(self.name, v),
                    None => std::env::remove_var(self.name),
                }
            }
        }
    }

    #[tokio::test]
    async fn openai_browser_flow_prebind_falls_back_to_paste_token_and_saves() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let _issuer = EnvVarRemoved::take("NEMESISBOT_OAUTH_ISSUER");

        let ws = TestWorkspace::new().unwrap();
        std::fs::create_dir_all(ws.home()).unwrap();

        // 抢占回调端口。若环境里已被别的进程占用则无法保证不弹浏览器，软跳过。
        let blocker = std::net::TcpListener::bind("127.0.0.1:1455");
        if blocker.is_err() {
            return; // 环境性占用：跳过（非断言失败）
        }

        let Ok(bin) = resolve_nemesisbot_bin() else {
            return; // 无可用二进制（理论不可达，harness 已兜底）
        };
        let out = ws
            .run_cli_with_stdin(
                &bin,
                &["auth", "login", "--provider", "openai"],
                "sk-r9-paste-token\n",
                30,
            )
            .await;
        drop(blocker); // 立即释放 1455（占用窗口最小化；断言失败也不会拖住端口）

        assert!(
            out.success(),
            "paste-token 成功路径应 rc 0\nstdout={} stderr={}",
            out.stdout,
            out.stderr
        );
        assert!(out.stdout_contains("Using browser-based OAuth flow..."));
        assert!(out.stdout_contains("OAuth flow failed"), "端口被占 → bind 失败必须报错回落");
        assert!(out.stdout_contains("Falling back to paste-token mode."));
        assert!(out.stdout_contains("Enter openai API token"));
        assert!(out.stdout_contains("Token saved to:"));
        assert!(out.stdout_contains("Logged in to openai successfully."));

        // auth.json 落盘且内容正确（AuthStore = HashMap<provider, AuthCredential>）。
        let raw = std::fs::read_to_string(ws.home().join("auth.json"))
            .expect("auth.json 必须落盘");
        let v: serde_json::Value = serde_json::from_str(&raw).expect("合法 JSON");
        let cred = v.get("openai").expect("openai 条目存在");
        assert_eq!(
            cred.get("access_token").and_then(|t| t.as_str()),
            Some("sk-r9-paste-token")
        );
        assert_eq!(cred.get("provider").and_then(|t| t.as_str()), Some("openai"));
        assert_eq!(cred.get("auth_method").and_then(|t| t.as_str()), Some("token"));
    }

    #[tokio::test]
    async fn device_code_dead_issuer_then_empty_input_cancels_login() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();

        let ws = TestWorkspace::new().unwrap();
        std::fs::create_dir_all(ws.home()).unwrap();
        let Ok(bin) = resolve_nemesisbot_bin() else {
            return;
        };

        // issuer 接缝指向本机 discard 端口 9：device-code 第一步 HTTP 请求立即
        // 连接拒绝（零外网）。这同时实证了接缝本身生效——若接缝失效会去打真
        // auth.openai.com（网络依赖，无法作为断言）。
        let _issuer = EnvVarSet::set("NEMESISBOT_OAUTH_ISSUER", "http://127.0.0.1:9");

        let out = ws
            .run_cli_with_stdin(
                &bin,
                &["auth", "login", "--provider", "openai", "--device-code"],
                "\n",
                30,
            )
            .await;

        assert!(out.success(), "取消路径是正常收尾（rc 0）\n{:?}", out);
        assert!(out.stdout_contains("Using device code flow..."));
        assert!(out.stdout_contains("OAuth flow failed"));
        assert!(out.stdout_contains("Enter openai API token"));
        assert!(out.stdout_contains("No token entered. Login cancelled."));
        assert!(
            !ws.home().join("auth.json").exists(),
            "取消不得落盘 auth.json"
        );
    }
}

// ===========================================================================
// r10（覆盖率 goal R10 批，2026-08-27）：login_device_code 成功段点亮。
//
// auth.rs:78-84 的 Ok 臂此前从未被走到：r9_zero 只覆盖了「device-code 死端点
// → 回落 paste-token → 取消」的失败一半。本测用生产已有的 NEMESISBOT_
// OAUTH_ISSUER 注入点把 issuer 指向本地 std TcpListener mock，按 oauth.rs
// 真实契约喂全三段链：
//   ① POST {issuer}/api/accounts/deviceauth/usercode
//        → {"device_auth_id","user_code","interval":1}
//      interval 必须 ≥1：oauth.rs 对 <1 会 clamp 到 5；填 1 使轮询首个
//      tick 立即触发，整链秒级完成。
//   ② POST {issuer}/api/accounts/deviceauth/token
//        → {"authorization_code","code_verifier"}（poll 层拿的是 code）
//   ③ POST {issuer}/oauth/token → {"access_token":...}（空 access_token 被拒）
// 链路终点 = store.save + 双 println + return Ok(())——auth.rs 成功臂全行点亮。
//
// 进程内直调 run()；持 crate::GLOBAL_STATE_LOCK 改 NEMESISBOT_HOME /
// NEMESISBOT_OAUTH_ISSUER 并摘掉代理变量（reqwest 默认读环境代理，不摘会
// 把 127.0.0.1 请求发去系统代理）。
// ===========================================================================

mod r10_device_code_flow {
    use super::super::{run, AuthAction};
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use tempfile::TempDir;

    /// env 快照 RAII：构造时记录全部相关变量旧值并施加新值，Drop 一律恢复。
    /// 必须在持有 GLOBAL_STATE_LOCK 期间使用。
    struct EnvSnapshot(Vec<(String, Option<String>)>);

    impl EnvSnapshot {
        const NAMES: [&'static str; 8] = [
            "NEMESISBOT_HOME",
            "NEMESISBOT_OAUTH_ISSUER",
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ];

        fn take_and_apply(home_parent: &std::path::Path, issuer_url: String) -> Self {
            let saved: Vec<(String, Option<String>)> =
                Self::NAMES.iter().map(|k| ((*k).to_string(), std::env::var(k).ok())).collect();
            unsafe {
                for k in [
                    "HTTP_PROXY",
                    "HTTPS_PROXY",
                    "ALL_PROXY",
                    "http_proxy",
                    "https_proxy",
                    "all_proxy",
                ] {
                    std::env::remove_var(k);
                }
                std::env::set_var("NEMESISBOT_HOME", home_parent);
                std::env::set_var("NEMESISBOT_OAUTH_ISSUER", issuer_url);
            }
            Self(saved)
        }
    }

    impl Drop for EnvSnapshot {
        fn drop(&mut self) {
            for (k, v) in &self.0 {
                unsafe {
                    match v {
                        Some(s) => std::env::set_var(k, s),
                        None => std::env::remove_var(k),
                    }
                }
            }
        }
    }

    fn mock_body_for(request_head: &str) -> &'static str {
        // 请求行按子串路由（issuer 根为 "/"，路径原样出现在首行）。
        if request_head.contains("/api/accounts/deviceauth/usercode") {
            r#"{"device_auth_id":"dai-r10","user_code":"R10-CODE","interval":1}"#
        } else if request_head.contains("/api/accounts/deviceauth/token") {
            r#"{"authorization_code":"ac-r10","code_verifier":"cv-r10"}"#
        } else if request_head.contains("/oauth/token") {
            r#"{"access_token":"at-r10","refresh_token":"rt-r10","expires_in":3600,"id_token":""}"#
        } else {
            "{}"
        }
    }

    /// 读满一个 HTTP 请求（头到 \r\n\r\n + 按 Content-Length 补 body）后回固定 JSON。
    fn serve_conn(stream: &mut TcpStream) {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 2048];
        loop {
            match stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 64 * 1024 {
                        break;
                    }
                }
                Err(_) => return,
            }
        }

        // 头解析在自有作用域内完成：产出（路由键、Content-Length）并立即
        // 释放对 buf 的不可变借用，后面才能继续往 buf 补读 body 字节。
        let (route_key, content_length) = {
            let text = String::from_utf8_lossy(&buf);
            let head = text.split("\r\n\r\n").next().unwrap_or("");
            let cl = head
                .lines()
                .find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    k.trim()
                        .eq_ignore_ascii_case("content-length")
                        .then(|| v.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            (head.to_string(), cl)
        };
        let head_len = buf
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .map(|p| p + 4)
            .unwrap_or(buf.len());

        while buf.len() < head_len + content_length {
            match stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(_) => return,
            }
        }

        let body_json = mock_body_for(&route_key);
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body_json.len(),
            body_json
        );
        let _ = stream.write_all(resp.as_bytes());
        let _ = stream.flush();
    }

    /// 本地 mock OAuth issuer：nonblocking accept + 5ms 轮询 + AtomicBool 停机。
    struct MockIssuer {
        addr: std::net::SocketAddr,
        shutdown: Arc<AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl Drop for MockIssuer {
        fn drop(&mut self) {
            self.shutdown.store(true, Ordering::Relaxed);
            if let Some(h) = self.thread.take() {
                let _ = h.join();
            }
        }
    }

    impl MockIssuer {
        fn start() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock issuer");
            let addr = listener.local_addr().unwrap();
            let shutdown = Arc::new(AtomicBool::new(false));
            let sd = shutdown.clone();
            let thread = std::thread::spawn(move || {
                use std::io::ErrorKind;
                let _ = listener.set_nonblocking(true);
                while !sd.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((mut s, _)) => {
                            let _ = s.set_nonblocking(false);
                            let _ =
                                s.set_read_timeout(Some(std::time::Duration::from_secs(3)));
                            let _ =
                                s.set_write_timeout(Some(std::time::Duration::from_secs(3)));
                            serve_conn(&mut s);
                        }
                        Err(ref e) if e.kind() == ErrorKind::WouldBlock => {
                            std::thread::sleep(std::time::Duration::from_millis(5));
                        }
                        Err(_) => break,
                    }
                }
            });
            Self { addr, shutdown, thread: Some(thread) }
        }
    }

    #[tokio::test]
    async fn r10_device_code_full_flow_success_saves_credential_via_mock_issuer() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();

        let tmp = TempDir::new().unwrap();
        let issuer = MockIssuer::start();
        let _env = EnvSnapshot::take_and_apply(tmp.path(), format!("http://{}", issuer.addr));

        let home = tmp.path().join(".nemesisbot");
        std::fs::create_dir_all(&home).unwrap();

        run(
            AuthAction::Login {
                provider: "openai".into(),
                device_code: true,
            },
            false,
        )
        .await
        .expect("mock issuer 三段契约下 device-code 必须成功");
        drop(_env);
        drop(issuer);

        let raw = std::fs::read_to_string(home.join("auth.json"))
            .expect("device-code 成功臂必须落盘 auth.json");
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let cred = v.get("openai").expect("openai 条目存在");
        assert_eq!(
            cred.get("access_token").and_then(|t| t.as_str()),
            Some("at-r10"),
            "/oauth/token 兑换的 access_token 应原样入库: {raw}"
        );
        assert_eq!(cred.get("provider").and_then(|t| t.as_str()), Some("openai"));
        // oauth 凭据的 auth_method 恒为 "oauth"（区别于 paste-token 的 "token"）。
        assert_eq!(cred.get("auth_method").and_then(|t| t.as_str()), Some("oauth"));
    }
}
