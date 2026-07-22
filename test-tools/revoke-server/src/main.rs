//! revoke-server（v3：DLL 验证模块架构的云端签发 + 吊销服务）。
//!
//! 启动：`revoke-server --keys-file keys.json [--init-keys] [--bind 127.0.0.1:7878]`
//!                     `[--db-url revoke.db] [--admin-token ...]`
//!
//! 首次：`--init-keys` 生成密钥体系（root/CA/issuer 私钥 + 证书链）到 `--keys-file`，打印根公钥。
//! 后续：`--keys-file` 加载（root_sk 签云端响应/CRL；issuer_sk 签 exe，envelope 带链）。
//!
//! API：`POST /v1/verify` | `GET /v1/crl` | `GET /v1/trusted-keys` | `POST /v1/sign`（带链签发）
//!      `POST /v1/admin/revoke` | `POST /v1/admin/trusted-key` | `POST /v1/admin/user`
//!      `GET /v1/audit` | `GET /v1/signatures` | `GET /v1/admin/users` | `GET /v1/health`

mod handlers;
mod state;
mod store;

use anyhow::Result;
use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::response::Html;
use axum::routing::{get, post};
use clap::Parser;
use nemesis_verify::hex_util::hex_encode;

/// Web UI（嵌入式单页：登录 + CRL/吊销/trusted-keys/审计）。
const INDEX_HTML: &str = include_str!("web/index.html");
const ADMIN_HTML: &str = include_str!("web/admin.html");

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn admin_page() -> Html<&'static str> {
    Html(ADMIN_HTML)
}

#[derive(Parser)]
#[command(
    name = "revoke-server",
    version,
    about = "签名吊销服务端（v3 DLL 架构 + 证书链签发）"
)]
struct Cli {
    /// 监听地址
    #[arg(long, default_value = "127.0.0.1:7878")]
    bind: String,
    /// 数据库 URL（SQLite 库文件，默认 revoke.db）
    #[arg(long, default_value = "revoke.db")]
    db_url: String,
    /// 密钥体系 JSON 文件（root/CA/issuer 私钥 + 证书链）。
    #[arg(long, default_value = "keys.json")]
    keys_file: String,
    /// 首次生成密钥体系到 --keys-file（已存在则覆盖）。打印根公钥（客户端/DLL 内置用）。
    #[arg(long)]
    init_keys: bool,
    /// admin 接口鉴权 token
    #[arg(long, default_value = "admin-token-change-me")]
    admin_token: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.init_keys {
        let h = nemesis_verify::keygen::generate_hierarchy(0, u64::MAX);
        h.save(&cli.keys_file)?;
        println!("✓ generated key hierarchy → {}", cli.keys_file);
        println!(
            "  root pubkey (内置客户端/DLL): {}",
            hex_encode(&h.root_vk.to_bytes())
        );
        println!("  issuer pubkey: {}", hex_encode(&h.issuer_vk.to_bytes()));
    }

    let state = state::AppState::new(&cli.db_url, &cli.keys_file, cli.admin_token.clone())?;

    let app = Router::new()
        .route("/", get(index))
        .route("/admin", get(admin_page))
        .route("/v1/verify", post(handlers::verify))
        .route("/v1/crl", get(handlers::get_crl))
        .route("/v1/crl/query", post(handlers::crl_query))
        .route("/v1/trusted-keys", get(handlers::get_trusted_keys))
        .route("/v1/sign", post(handlers::sign_upload))
        .route("/v1/admin/revoke", post(handlers::admin_revoke))
        .route("/v1/admin/trusted-key", post(handlers::admin_trusted_key))
        .route("/v1/admin/user", post(handlers::admin_create_user))
        .route("/v1/admin/issuer", post(handlers::admin_create_issuer))
        .route("/v1/admin/issuers", get(handlers::list_issuers))
        .route("/v1/audit", get(handlers::get_audit))
        .route("/v1/signatures", get(handlers::list_signatures))
        .route("/v1/admin/users", get(handlers::list_users))
        .route("/v1/health", get(handlers::health))
        .layer(DefaultBodyLimit::max(200 * 1024 * 1024)) // 200MB（文件上传签发）
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&cli.bind).await?;
    println!(
        "revoke-server listening on http://{} (db: {}, keys: {})",
        cli.bind, cli.db_url, cli.keys_file
    );
    println!("  admin token: {}", cli.admin_token);
    axum::serve(listener, app).await?;
    Ok(())
}
