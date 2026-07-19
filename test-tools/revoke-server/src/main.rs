//! revoke-server（store 集成版：SQLite 默认 + 审计）。
//!
//! 启动：revoke-server --crkey <hex> [--bind 127.0.0.1:7878] [--db-url revoke.db]
//!                     [--admin-token ...]
//!
//! API：POST /v1/verify | GET /v1/crl | GET /v1/trusted-keys
//!      POST /v1/admin/revoke | POST /v1/admin/trusted-key | GET /v1/audit | GET /v1/health
//!
//! 公开响应用吊销根密钥（--crkey）签；admin 操作写审计。

mod handlers;
mod state;
mod store;

use anyhow::Result;
use axum::response::Html;
use axum::routing::{get, post};
use axum::Router;
use clap::Parser;

/// Web UI（嵌入式单页：登录 + CRL/吊销/trusted-keys/审计）。
const INDEX_HTML: &str = include_str!("web/index.html");

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

#[derive(Parser)]
#[command(name = "revoke-server", version, about = "签名吊销服务端（SQLite + 审计）")]
struct Cli {
    /// 监听地址
    #[arg(long, default_value = "127.0.0.1:7878")]
    bind: String,
    /// 数据库 URL（SQLite 库文件，默认 revoke.db；后续可扩 mysql:/postgres:）
    #[arg(long, default_value = "revoke.db")]
    db_url: String,
    /// 吊销根私钥（hex，签响应；与客户端 crpub 配对）
    #[arg(long)]
    crkey: String,
    /// admin 接口鉴权 token
    #[arg(long, default_value = "admin-token-change-me")]
    admin_token: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let state = state::AppState::new(&cli.db_url, &cli.crkey, cli.admin_token.clone())?;

    let app = Router::new()
        .route("/", get(index))
        .route("/v1/verify", post(handlers::verify))
        .route("/v1/crl", get(handlers::get_crl))
        .route("/v1/trusted-keys", get(handlers::get_trusted_keys))
        .route("/v1/admin/revoke", post(handlers::admin_revoke))
        .route("/v1/admin/trusted-key", post(handlers::admin_trusted_key))
        .route("/v1/audit", get(handlers::get_audit))
        .route("/v1/health", get(handlers::health))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&cli.bind).await?;
    println!("revoke-server listening on http://{} (db: {})", cli.bind, cli.db_url);
    println!("  admin token: {}", cli.admin_token);
    axum::serve(listener, app).await?;
    Ok(())
}
