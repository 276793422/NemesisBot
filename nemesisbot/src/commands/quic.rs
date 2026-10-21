//! QUIC 中继 CLI（feature=roaming-relay 穿透 nemesis-cluster/roaming-relay）。
//!
//! 独立中继的两个入口同源共用 `run_relay_server`（同一实现，不冲突）：
//! - `quic relay`：分类管理入口（QUIC 命令族内自洽，配 `quic status` 用）。
//! - `gateway --quic-relay`：统一入口（GatewayCtx::assemble 早退调进本函数；
//!   与桥中继 `gateway --relay` 旗标先例同形态）。
//!
//! 两者都读 config.cluster.json `roaming.relay` 段装配（`relay.enabled` 只
//! 约束 gateway 进程内启动闸，独立进程形态不受此闸约束——命令本身即显式
//! 意志）；token 空 = fail-closed 拒绝启动。
//!
//! - `quic status`：只读状态面——配置解析产物、对外 relay URL、证书文件
//!   状态（零副作用，不触发证书生成）。

use std::path::Path;

use nemesis_cluster::roaming::config::{RelayTls, resolved_roaming};

/// `nemesisbot quic <action>` 入口。
pub async fn run(home: &Path, action: QuicAction) -> Result<(), String> {
    match action {
        QuicAction::Relay => run_relay_server(home).await,
        QuicAction::Status => show_status(home),
    }
}

/// `quic relay` 独立子命令动作。
#[derive(clap::Subcommand, Debug)]
pub enum QuicAction {
    /// 启动自建 QUIC 中继服务器（内网穿透；Ctrl-C 停机）
    Relay,
    /// 查看中继配置解析产物与证书状态（只读）
    Status,
}

fn resolved(
    home: &Path,
) -> (
    nemesis_cluster::roaming::config::RoamingOverlayConfig,
    std::path::PathBuf,
) {
    let config_path = crate::common::cluster_config_path(home);
    let cfg = resolved_roaming(&config_path);
    (cfg, config_path)
}

/// 独立中继进程：装配 → 打印监听面 → Ctrl-C 等待 → 优雅停机。
///
/// 两个 CLI 入口同源共用（单一实现）：`nemesisbot quic relay`（分类管理入口）
/// 与 `nemesisbot gateway --quic-relay`（统一入口，与桥中继 `--relay` 旗标
/// 先例同形态——经 GatewayCtx::assemble 早退调进本函数）。与桥中继
/// `run_relay` 同款 fail-closed 纪律：`roaming.relay.token` 空 = 拒绝启动
/// （纯中继面向公网，无鉴权裸奔不可放行——此前的「警告后 AllowAll」偏差已
/// 对齐收口）。
pub(crate) async fn run_relay_server(home: &Path) -> Result<(), String> {
    let (cfg, config_path) = resolved(home);
    let workspace = crate::common::workspace_path(home);
    let relay = &cfg.relay;

    if relay.token.trim().is_empty() {
        eprintln!(
            "[QuicRelay] 启动失败：config.cluster.json 未配置 roaming.relay.token（访问令牌；\
空 = 无鉴权裸奔）。请在 roaming.relay.token 填入预共享令牌后重试（客户端侧 \
roaming.relay_token 需配对同值）。"
        );
        return Err("--quic-relay/quic relay requires roaming.relay.token to be set".to_string());
    }

    println!("QUIC 中继服务器（自建 iroh relay，独立进程）");
    println!("  配置: {}", config_path.display());
    println!(
        "  TLS 档: {}",
        match relay.tls {
            RelayTls::SelfSigned => "self-signed（自签，首启生成 <workspace>/cluster/relay/）",
            RelayTls::Manual => "manual（用户自备证书）",
            RelayTls::None => "none（纯 HTTP relay transport）",
        }
    );
    println!("  访问控制: SharedToken（客户端需配对 roaming.relay_token）");

    let mut handle =
        nemesis_cluster::roaming::relay_server::spawn_relay_server(relay, &workspace).await?;
    if let Some(http) = handle.http_addr {
        println!("  HTTP relay transport: {http}");
    }
    if let Some(quic) = handle.quic_addr {
        println!("  QUIC relay transport: {quic}");
    }
    println!(
        "  对外 relay URL（客户端 roaming.relay_url 填这个）: {}",
        nemesis_cluster::roaming::relay_server::advertised_relay_url(relay)
    );
    if relay.tls == RelayTls::SelfSigned {
        println!(
            "  证书: {}",
            nemesis_cluster::roaming::relay_server::relay_cert_path(&workspace).display()
        );
    }
    println!("就绪。Ctrl-C 停机。");

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| format!("监听 Ctrl-C 失败: {e}"))?;
    println!("停机中…");
    handle.shutdown().await;
    println!("已停机。");
    Ok(())
}

/// `quic status` 只读状态面。
fn show_status(home: &Path) -> Result<(), String> {
    let (cfg, config_path) = resolved(home);
    let workspace = crate::common::workspace_path(home);
    let relay = &cfg.relay;

    println!("roaming 段解析产物（{}）:", config_path.display());
    println!("  叠加层总开关 (roaming.enabled): {}", cfg.enabled);
    println!(
        "  客户端自建中继 (roaming.relay_url): {}",
        if cfg.relay_url.is_empty() {
            "（空 = n0 公共中继）"
        } else {
            &cfg.relay_url
        }
    );
    println!(
        "  客户端中继令牌 (roaming.relay_token): {}",
        if cfg.relay_token.is_empty() {
            "（未设）"
        } else {
            "（已设）"
        }
    );
    // 本机 iroh 身份（组网接线关键项：对端 peers.toml 静态条目的
    // `iroh:<hex>@<relay_url>` 要写这个 hex——gateway 启动日志同值，
    // [node].iroh_id 声明可对账）。
    let key_path = nemesis_cluster::roaming::endpoint::roaming_key_path(&workspace);
    if key_path.exists() {
        match nemesis_cluster::roaming::endpoint::RoamingIdentity::load(&key_path) {
            Ok(id) => {
                let hex = id.node_id_hex();
                println!("本机 iroh 身份: iroh:{hex}");
                println!("  对端静态条目写法: address = \"iroh:{hex}@<本机对外 relay_url>\"");
            }
            Err(e) => println!("本机 iroh 身份: 读取失败（{e}）"),
        }
    } else {
        println!(
            "本机 iroh 身份: 未生成（`cluster roaming keygen` 或首次启用 roaming 的 gateway 自动生成）"
        );
    }
    println!("服务端自建中继 (roaming.relay):");
    println!("  gateway 进程内启动 (enabled): {}", relay.enabled);
    println!("  HTTP 端口: {}", relay.http_port);
    println!("  QUIC 端口: {}", relay.quic_port);
    println!(
        "  对外主机 (external_host): {}",
        if relay.external_host.is_empty() {
            "（空 = 自动探测）"
        } else {
            &relay.external_host
        }
    );
    println!(
        "  TLS 档: {}",
        match relay.tls {
            RelayTls::SelfSigned => "self-signed",
            RelayTls::Manual => "manual",
            RelayTls::None => "none",
        }
    );
    println!(
        "  访问令牌 (token): {}",
        if relay.token.is_empty() {
            "（未设 = 拒绝启动，fail-closed：无鉴权跳板不允许裸奔）"
        } else {
            "（已设 = SharedToken）"
        }
    );
    match relay.tls {
        RelayTls::Manual => {
            println!("  证书: {}（用户自备）", relay.cert_path);
            println!("  私钥: {}（用户自备）", relay.key_path);
        }
        _ => {
            let cert = nemesis_cluster::roaming::relay_server::relay_cert_path(&workspace);
            let key = nemesis_cluster::roaming::relay_server::relay_key_path(&workspace);
            println!(
                "  证书: {}（{}）",
                cert.display(),
                if cert.exists() {
                    "已生成"
                } else {
                    "未生成（首次启动自签）"
                }
            );
            println!(
                "  私钥: {}（{}）",
                key.display(),
                if key.exists() {
                    "已生成"
                } else {
                    "未生成（首次启动自签）"
                }
            );
        }
    }
    println!(
        "  对外 relay URL: {}",
        nemesis_cluster::roaming::relay_server::advertised_relay_url(relay)
    );
    Ok(())
}
