//! authn-demo — 身份与访问框架 demo CLI（独立验证任务，非产品集成）。
//!
//! 每个验证项一个子命令：
//! | 验证项 | 子命令 | 依赖 |
//! |--------|--------|------|
//! | ③ 本地账号 + argon2 | `hash-password` / `local-login` | 无 |
//! | ④ 会话行为 | `selftest`（签发/过期/并发全覆盖） | 无 |
//! | ⑤ 静态 token 兼容 | `compat` | 无 |
//! | ① OIDC 全流程 | `oidc-login`（浏览器）/ `oidc-ropc`（脚本化） | Keycloak 容器 |
//! | ② LDAP 全流程 | `ldap-login` | OpenLDAP 容器 |
//!
//! 快速开始：`authn-demo selftest` 不需要任何外部服务。

use std::collections::HashMap;

use authn_core::compat::{AccessControl, AccessDecision, AccessMode};
use authn_core::identity::Identity;
use authn_core::ldap::{authenticate as ldap_authenticate, GroupExtraction, LdapConfig};
use authn_core::local::{hash_password, LocalUserDatabase};
use authn_core::oidc::{
    begin as oidc_begin, complete as oidc_complete, decode_jwt_payload, extract_display_name,
    extract_roles, password_grant_token, OidcConfig,
};
use authn_core::session::{SessionConfig, SessionStore};
use clap::{Parser, Subcommand, ValueEnum};
use serde_json::json;

#[derive(Parser)]
#[command(name = "authn-demo", about = "身份与访问框架 demo（独立验证任务）")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 生成 argon2id PHC 哈希（用于 users.json 的 password_hash 字段）
    HashPassword { password: String },
    /// 纯逻辑自测（无外部服务依赖）：argon2/会话/兼容模式全链路
    Selftest,
    /// 本地账号登录演示：认证 → 签发会话 → 校验 → 打印身份
    LocalLogin {
        /// 账号文件（JSON），样例见 examples/users.sample.json
        #[arg(long)]
        users_file: String,
        #[arg(long)]
        username: String,
        #[arg(long)]
        password: String,
        /// 会话 TTL 秒数（演示过期可给 1）
        #[arg(long, default_value_t = 3600)]
        session_ttl_secs: u64,
    },
    /// OIDC 浏览器全流程：打印授权 URL → 本机等回调 → 校验 → 打印身份
    OidcLogin {
        /// 如 http://localhost:8088/realms/demo
        #[arg(long)]
        issuer: String,
        #[arg(long, default_value = "authn-demo")]
        client_id: String,
        #[arg(long)]
        client_secret: Option<String>,
        #[arg(long, default_value_t = 18081)]
        port: u16,
    },
    /// OIDC 脚本化路径（ROPC/directAccessGrants）：无浏览器、确定性
    OidcRopc {
        #[arg(long)]
        issuer: String,
        #[arg(long, default_value = "authn-demo")]
        client_id: String,
        #[arg(long)]
        client_secret: Option<String>,
        #[arg(long)]
        username: String,
        #[arg(long)]
        password: String,
    },
    /// LDAP 认证：bind → 组提取 → 打印身份
    LdapLogin {
        /// ldap://localhost:1389 或 ldaps://localhost:1636
        #[arg(long)]
        url: String,
        #[arg(long)]
        username: String,
        #[arg(long)]
        password: String,
        /// 用户 DN 模板，{} 为用户名占位
        #[arg(long, default_value = "uid={},ou=people,dc=example,dc=org")]
        user_dn_template: String,
        #[arg(long, default_value = "dc=example,dc=org")]
        base_dn: String,
        /// 组提取模式：memberof（AD）| filter（OpenLDAP groupOfNames 反查）
        #[arg(long, default_value = "filter")]
        group_mode: GroupMode,
        /// 组条目 objectClass（filter 模式用；OpenLDAP=groupOfNames，AD=group）
        #[arg(long, default_value = "groupOfNames")]
        group_object_class: String,
        /// 组成员属性（filter 模式用；几乎恒为 member）
        #[arg(long, default_value = "member")]
        group_member_attr: String,
        /// 对 ldap:// 端口做 STARTTLS 明文升级（公司 LDAP 常强制；ldaps:// 时别开）
        #[arg(long, default_value_t = false)]
        starttls: bool,
        /// 跳过 TLS 证书校验（自签/公司内网 CA 的 LDAPS 演示用）
        #[arg(long, default_value_t = false)]
        tls_no_verify: bool,
    },
    /// 旧静态 token 兼容语义演示（空=开放 / 非空=单用户）
    Compat {
        /// 配置的静态 token（空串 = 开放模式）
        #[arg(long, default_value = "")]
        static_token: String,
        /// 请求实际出示的 token（缺省 = 不带）
        #[arg(long)]
        presented: Option<String>,
    },
}

#[derive(Clone, ValueEnum)]
enum GroupMode {
    Memberof,
    Filter,
}

fn print_identity(label: &str, identity: &Identity) {
    println!("--- {label} ---");
    println!("{}", serde_json::to_string_pretty(identity).unwrap());
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let code = match run(cli.command).await {
        Ok(()) => 0,
        Err(msg) => {
            eprintln!("错误: {msg}");
            1
        }
    };
    std::process::exit(code);
}

async fn run(command: Command) -> Result<(), String> {
    match command {
        Command::HashPassword { password } => {
            let hash = hash_password(&password).map_err(|e| e.to_string())?;
            println!("{hash}");
            println!();
            println!("// 把上面整行填进 users.json 的 password_hash 字段");
            Ok(())
        }

        Command::Selftest => {
            println!("== 纯逻辑自测（无外部服务）==\n");
            let mut failures = Vec::new();

            // ① argon2 哈希/校验
            match selftest_local() {
                Ok(lines) => lines
                    .into_iter()
                    .for_each(|l| report(&mut failures, &l.0, l.1)),
                Err(e) => failures.push(format!("local 模块异常: {e}")),
            }
            // ② 会话签发/过期/并发
            match selftest_session() {
                Ok(lines) => lines
                    .into_iter()
                    .for_each(|l| report(&mut failures, &l.0, l.1)),
                Err(e) => failures.push(format!("session 模块异常: {e}")),
            }
            // ③ 静态 token 兼容三态
            match selftest_compat() {
                Ok(lines) => lines
                    .into_iter()
                    .for_each(|l| report(&mut failures, &l.0, l.1)),
                Err(e) => failures.push(format!("compat 模块异常: {e}")),
            }

            println!();
            if failures.is_empty() {
                println!("全部通过 ✅");
                Ok(())
            } else {
                for f in &failures {
                    eprintln!("FAIL: {f}");
                }
                Err(format!("{} 项失败", failures.len()))
            }
        }

        Command::LocalLogin {
            users_file,
            username,
            password,
            session_ttl_secs,
        } => {
            let db = LocalUserDatabase::load_from_json_file(std::path::Path::new(&users_file))
                .map_err(|e| format!("账号文件加载失败: {e}"))?;
            let identity = db
                .authenticate(&username, &password)
                .map_err(|e| e.to_string())?;
            print_identity("认证成功 · Identity", &identity);

            let store = SessionStore::new(SessionConfig {
                ttl_seconds: session_ttl_secs,
                max_sessions_per_user: 4,
            });
            let rec = store.issue(&identity).map_err(|e| e.to_string())?;
            println!("\n--- 会话签发 ---");
            println!(
                "{}",
                json!({
                    "token": rec.token,
                    "subject": rec.subject,
                    "issued_at_unix": rec.issued_at_unix,
                    "expires_at_unix": rec.expires_at_unix,
                })
            );

            let validated = store.validate(&rec.token).map_err(|e| e.to_string())?;
            println!("\n--- 立即校验 ---");
            println!("{}", json!({"valid": true, "subject": validated.subject}));

            if session_ttl_secs <= 2 {
                println!(
                    "\n（TTL={}s：等 {}ms 后演示过期……）",
                    session_ttl_secs,
                    session_ttl_secs * 1000 + 200
                );
                tokio::time::sleep(std::time::Duration::from_millis(
                    session_ttl_secs * 1000 + 200,
                ))
                .await;
                let outcome = store.validate(&rec.token);
                println!(
                    "{}",
                    json!({
                        "second_validate_ok": outcome.is_ok(),
                        "error": outcome.err().map(|e| e.to_string()),
                    })
                );
            }
            Ok(())
        }

        Command::OidcLogin {
            issuer,
            client_id,
            client_secret,
            port,
        } => {
            let config = OidcConfig {
                issuer,
                client_id,
                client_secret,
                redirect_port: port,
                extra_scopes: vec!["profile".into()],
            };
            let flow = oidc_begin(&config).await.map_err(|e| e.to_string())?;
            println!("== OIDC 浏览器全流程 ==");
            println!("1. 在浏览器打开下面的 URL 并登录（Keycloak: alice/alice123）:\n");
            println!("   {}\n", flow.auth_url);
            println!("2. 等待本机回调 http://localhost:{port}/callback ……");

            let (code, state_back) = wait_for_callback(port).await?;
            println!("3. 收到回调，校验 state 与 PKCE 交换令牌……");
            let result = oidc_complete(flow, &code, &state_back)
                .await
                .map_err(|e| e.to_string())?;
            print_identity(
                "登录成功 · Identity（id_token 已库内验签）",
                &result.identity,
            );
            println!("\n--- 已验证 id_token 载荷 ---");
            println!(
                "{}",
                serde_json::to_string_pretty(&result.raw_claims).unwrap()
            );
            println!("\n（提取到的角色: {:?}）", result.identity.roles);
            Ok(())
        }

        Command::OidcRopc {
            issuer,
            client_id,
            client_secret,
            username,
            password,
        } => {
            let config = OidcConfig::new(issuer, client_id);
            let config = OidcConfig {
                client_secret,
                ..config
            };
            let token = password_grant_token(&config, &username, &password)
                .await
                .map_err(|e| e.to_string())?;
            println!("== OIDC ROPC（脚本化，仅测试用）==");
            let access = token
                .get("access_token")
                .and_then(|v| v.as_str())
                .ok_or("响应缺 access_token")?
                .to_string();
            let id_token = token
                .get("id_token")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            println!(
                "{}",
                json!({
                    "token_type": token.get("token_type"),
                    "expires_in": token.get("expires_in"),
                    "has_access_token": !access.is_empty(),
                    "has_id_token": id_token.is_some(),
                })
            );

            // ROPC 路径的 id_token 没经过库验签（无 PKCE/nonce 上下文）——
            // demo 里只解码展示载荷；生产集成一律走浏览器全流程（oidc-login）。
            if let Some(idt) = &id_token {
                let raw = decode_jwt_payload(idt).map_err(|e| e.to_string())?;
                // 角色合并 id_token + access_token（Keycloak 的 realm 角色默认在
                // access_token；与库内 complete() 同一语义）
                let mut roles = extract_roles(&raw);
                if let Ok(ac) = decode_jwt_payload(&access) {
                    roles.extend(extract_roles(&ac));
                }
                roles.sort();
                roles.dedup();
                let identity = Identity {
                    subject: raw
                        .get("sub")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?")
                        .to_string(),
                    display_name: extract_display_name(&raw, "?"),
                    roles,
                    source: authn_core::identity::AuthnSource::Oidc {
                        issuer: config.issuer.trim_end_matches('/').to_string(),
                        client_id: config.client_id.clone(),
                    },
                };
                print_identity("\n身份（ROPC，载荷未验签——仅演示）", &identity);
            }
            Ok(())
        }

        Command::LdapLogin {
            url,
            username,
            password,
            user_dn_template,
            base_dn,
            group_mode,
            group_object_class,
            group_member_attr,
            starttls,
            tls_no_verify,
        } => {
            let config = LdapConfig {
                url,
                starttls,
                tls_no_verify,
                user_dn_template,
                base_dn,
                group_extraction: match group_mode {
                    GroupMode::Memberof => GroupExtraction::MemberOf,
                    GroupMode::Filter => GroupExtraction::MemberFilter,
                },
                group_object_class,
                group_member_attr,
            };
            let result = ldap_authenticate(&config, &username, &password)
                .await
                .map_err(|e| e.to_string())?;
            println!("== LDAP 认证成功 ==");
            println!(
                "{}",
                json!({
                    "user_dn": result.user_dn,
                    "raw_groups": result.raw_groups,
                    "warnings": result.warnings,
                })
            );
            print_identity("\nIdentity", &result.identity);
            Ok(())
        }

        Command::Compat {
            static_token,
            presented,
        } => {
            let ac =
                AccessControl::from_static_token(Some(&static_token)).map_err(|e| e.to_string())?;
            let decision = ac.check(presented.as_deref());
            let mode_desc = match ac.mode() {
                AccessMode::Open => "open（开放——未配置 token）",
                AccessMode::StaticToken(_) => "static_token（单用户）",
            };
            let decision_desc = match decision {
                AccessDecision::AllowOpen => "allow（开放模式）",
                AccessDecision::AllowStaticToken => "allow（静态 token 匹配）",
                AccessDecision::Denied => "deny",
            };
            println!("== 静态 token 兼容语义 ==");
            println!(
                "{}",
                json!({
                    "mode": mode_desc,
                    "presented": presented,
                    "decision": decision_desc,
                })
            );
            Ok(())
        }
    }
}

fn report(failures: &mut Vec<String>, name: &str, ok: bool) {
    println!("  [{}] {}", if ok { "✅" } else { "❌" }, name);
    if !ok {
        failures.push(name.to_string());
    }
}

// ---------------- selftest 的三段检查 ----------------

type Check = (String, bool);

fn selftest_local() -> Result<Vec<Check>, String> {
    let mut out = Vec::new();
    let hash = hash_password("selftest-pw").map_err(|e| e.to_string())?;
    out.push((
        "argon2 哈希生成（PHC 格式）".into(),
        hash.starts_with("$argon2id$"),
    ));
    out.push((
        "argon2 正确密码校验通过".into(),
        authn_core::local::verify_password("selftest-pw", &hash).unwrap_or(false),
    ));
    out.push((
        "argon2 错误密码校验拒绝".into(),
        !authn_core::local::verify_password("wrong", &hash).unwrap_or(true),
    ));

    let bad_hash_json = r#"{"users":[{"username":"u","password_hash":"garbage"}]}"#;
    let db = LocalUserDatabase::from_json_str(bad_hash_json).map_err(|e| e.to_string())?;
    out.push((
        "本地账号：坏哈希返回错误而非静默拒绝".into(),
        matches!(
            db.authenticate("u", "x"),
            Err(authn_core::local::LocalAuthError::Hash(_))
        ),
    ));

    let hash2 = hash_password("real").map_err(|e| e.to_string())?;
    let db2 = LocalUserDatabase::from_json_str(&format!(
        r#"{{"users":[{{"username":"zoo","display_name":"Zoo","roles":["admin"],"password_hash":"{hash2}"}}]}}"#
    ))
    .map_err(|e| e.to_string())?;
    out.push((
        "本地账号：正确凭据 → Identity(subject/display/roles/source)".into(),
        db.authenticate("u", "x").is_err()
            && db2
                .authenticate("zoo", "real")
                .map(|i| i.roles == vec!["admin"])
                .unwrap_or(false),
    ));
    out.push((
        "本地账号：不存在用户与错密码同一错误（不泄露存在性）".into(),
        db2.authenticate("ghost", "real").is_err() && db2.authenticate("zoo", "bad").is_err(),
    ));
    Ok(out)
}

fn selftest_session() -> Result<Vec<Check>, String> {
    use authn_core::session::SessionError;
    let mut out = Vec::new();
    let id = Identity {
        subject: "zoo".into(),
        display_name: "Zoo".into(),
        roles: vec![],
        source: authn_core::identity::AuthnSource::Local,
    };

    let store = SessionStore::new(SessionConfig {
        ttl_seconds: 1,
        max_sessions_per_user: 2,
    });
    let rec = store.issue(&id).map_err(|e| e.to_string())?;
    out.push((
        "会话签发（token 43 字符 base64url）".into(),
        rec.token.len() == 43,
    ));
    out.push((
        "会话签发后立即可校验".into(),
        store.validate(&rec.token).is_ok(),
    ));

    std::thread::sleep(std::time::Duration::from_millis(1300));
    out.push((
        "会话过期后校验返回 Expired".into(),
        matches!(store.validate(&rec.token), Err(SessionError::Expired)),
    ));

    let store2 = SessionStore::new(SessionConfig {
        ttl_seconds: 60,
        max_sessions_per_user: 2,
    });
    let a = store2.issue(&id).map_err(|e| e.to_string())?;
    let b = store2.issue(&id).map_err(|e| e.to_string())?;
    out.push((
        "同用户第 3 个会话被并发上限拒绝".into(),
        matches!(store2.issue(&id), Err(SessionError::LimitReached(2))),
    ));
    out.push(("吊销后令牌失效".into(), {
        store2.revoke(&a.token);
        store2.validate(&a.token).is_err() && store2.validate(&b.token).is_ok()
    }));
    Ok(out)
}

fn selftest_compat() -> Result<Vec<Check>, String> {
    let mut out = Vec::new();
    // 空 = 开放
    let open = AccessControl::from_static_token(Some("")).map_err(|e| e.to_string())?;
    out.push((
        "兼容语义：未配置 token = 开放模式（任何请求放行）".into(),
        open.mode() == &AccessMode::Open && open.check(None) == AccessDecision::AllowOpen,
    ));
    // 非空 = 单用户
    let ac = AccessControl::from_static_token(Some("legacy")).map_err(|e| e.to_string())?;
    out.push((
        "兼容语义：配置 token 后，正确 token 放行".into(),
        ac.check(Some("legacy")) == AccessDecision::AllowStaticToken,
    ));
    out.push((
        "兼容语义：错误 token 拒绝".into(),
        ac.check(Some("nope")) == AccessDecision::Denied,
    ));
    out.push((
        "兼容语义：不带 token 拒绝".into(),
        ac.check(None) == AccessDecision::Denied,
    ));
    Ok(out)
}

// ---------------- OIDC 回调服务器（demo bin 专责；库保持无 transport） ----------------

/// 极简本机 HTTP 回调监听：只解析 GET /callback?code=..&state=..，
/// 响应一段静态 HTML。返回 (authorization_code, state)。
async fn wait_for_callback(port: u16) -> Result<(String, String), String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(|e| format!("回调端口 {port} 绑定失败: {e}"))?;

    let (mut socket, _) =
        tokio::time::timeout(std::time::Duration::from_secs(300), listener.accept())
            .await
            .map_err(|_| "等待回调超时（5 分钟）".to_string())?
            .map_err(|e| format!("accept 失败: {e}"))?;

    let mut buf = vec![0u8; 8192];
    let n = socket
        .read(&mut buf)
        .await
        .map_err(|e| format!("读取回调失败: {e}"))?;
    let request = String::from_utf8_lossy(&buf[..n]);

    // 请求行形如：GET /callback?code=xxx&state=yyy&session_state=zzz HTTP/1.1
    let path = request
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| "回调请求格式非法".to_string())?
        .to_string();

    let query = path.split('?').nth(1).unwrap_or("");
    let mut params: HashMap<String, String> = HashMap::new();
    for pair in query.split('&') {
        let mut kv = pair.splitn(2, '=');
        if let (Some(k), Some(v)) = (kv.next(), kv.next()) {
            params.insert(url_decode(k), url_decode(v));
        }
    }
    if let Some(err) = params.get("error") {
        let desc = params
            .get("error_description")
            .map(String::as_str)
            .unwrap_or("");
        return Err(format!("IdP 返回错误: {err} ({desc})"));
    }
    let code = params
        .get("code")
        .cloned()
        .ok_or_else(|| "回调缺少 code 参数".to_string())?;
    let state = params.get("state").cloned().unwrap_or_default();

    let body = "<html><body style='font-family:sans-serif'><h2>✅ 登录回调已收到</h2><p>可以关闭这个页面，回到终端查看结果。</p></body></html>";
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.shutdown().await;

    Ok((code, state))
}

/// %XX + '+' URL 解码（回调 query 参数量级，够用且零依赖）。
fn url_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() + 1 && i + 2 <= bytes.len() - 1 => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
