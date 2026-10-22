//! QUIC CLI（feature=cluster-roaming 穿透 nemesis-cluster/roaming；2026-10-23
//! 与集群解耦：QUIC 自身配置/身份/状态是 QUIC 层资产，不寄居集群命令族）。
//!
//! - `quic keygen`：生成本机 iroh 漫游身份（自 `cluster roaming keygen` 挪来
//!   ——命令面挪、身份文件物理路径不动，免得平白多一步迁移）。
//! - `quic status`：只读状态面——client 段（客户端连接参数）+ relay 段
//!   （服务端配置解析产物、证书状态；零副作用，不触发证书生成）。
//!
//! 服务端面（`quic relay` 独立进程）随 `quic-relay` feature 编译：
//! 独立中继的两个入口同源共用 `run_relay_server`（同一实现，不冲突）——
//! `quic relay`：分类管理入口；`gateway --quic-relay`：统一入口
//! （GatewayCtx::assemble 早退调进本函数；与桥中继 `gateway --relay`
//! 旗标先例同形态）。两者都读 **config.quic.json** `relay` 段装配
//!（`relay.enabled` 只约束 gateway 进程内启动闸，独立进程形态不受此闸
//! 约束——命令本身即显式意志）；token 空 = fail-closed 拒绝启动。
//! 首读触发旧位置（cluster.json roaming 段）一次性迁移
//!（[`ensure_quic_config`]，幂等）。无 quic-relay 时 `quic status` 服务端
//! 段诚实输出「feature 未编译」。

use std::path::Path;

use nemesis_cluster::roaming::config::RelayTls;
use nemesis_cluster::roaming::quic_config::{QuicConfig, ensure_quic_config, resolved_quic};

/// `nemesisbot quic <action>` 入口。
pub async fn run(home: &Path, action: QuicAction) -> Result<(), String> {
    match action {
        #[cfg(feature = "quic-relay")]
        QuicAction::Relay => run_relay_server(home).await,
        QuicAction::Keygen { force } => run_keygen(home, force),
        QuicAction::Status => show_status(home),
    }
}

/// `quic` 子命令动作。
#[derive(clap::Subcommand, Debug)]
pub enum QuicAction {
    /// 启动自建 QUIC 中继服务器（内网穿透；Ctrl-C 停机；feature=quic-relay）
    #[cfg(feature = "quic-relay")]
    Relay,
    /// 生成本机 iroh 漫游身份密钥（workspace/cluster/roaming.key）
    Keygen {
        /// Regenerate even if a key exists (rotates node ID — peers must re-trust)
        #[arg(long)]
        force: bool,
    },
    /// 查看 QUIC 配置解析产物与证书状态（只读）
    Status,
}

/// QUIC 配置单一读取入口：旧位置首读迁移（幂等）→ config.quic.json 解析。
fn resolved(home: &Path) -> (QuicConfig, std::path::PathBuf) {
    let quic_path = crate::common::quic_config_path(home);
    let cluster_path = crate::common::cluster_config_path(home);
    // 旧位置（cluster.json roaming 段）一次性迁移；quic.json 已在 = no-op。
    let _ = ensure_quic_config(&cluster_path, &quic_path);
    (resolved_quic(&quic_path), quic_path)
}

/// `quic keygen`：生成本机 iroh 漫游身份（2026-10-23 自 `cluster roaming
/// keygen` 挪来——身份是 QUIC 层资产不是集群资产；**物理路径不变**
/// workspace/cluster/roaming.key，对端 peers.toml 静态条目零影响）。
fn run_keygen(home: &Path, force: bool) -> Result<(), String> {
    use nemesis_cluster::roaming::endpoint::{RoamingIdentity, persist, roaming_key_path};

    // roaming_* 收的是 workspace 根（与 gateway setup_roaming、WSAPI 面同
    // 口径），不是 home——错传会把 key 写到 `.nemesisbot/` 而非
    // `.nemesisbot/workspace/`，gateway 读不到。
    let workspace = crate::common::workspace_path(home);
    let key_path = roaming_key_path(&workspace);
    let (id, created) = if force && key_path.exists() {
        let id = RoamingIdentity::generate();
        persist(&id, &key_path).map_err(|e| format!("写身份密钥失败: {e}"))?;
        (id, true)
    } else {
        RoamingIdentity::load_or_create(&workspace).map_err(|e| format!("身份密钥装配失败: {e}"))?
    };
    println!("Roaming identity key: {}", key_path.display());
    println!(
        "  状态: {}",
        if created {
            "新生成"
        } else {
            "已存在（加载）"
        }
    );
    println!("  node_id: {}", id.node_id_hex());
    println!("  完整地址: iroh:{}", id.node_id_hex());
    if !created {
        println!("  （密钥已存在未变更；轮换加 --force——对端须重新信任新 node_id）");
    }
    Ok(())
}

/// 独立中继进程：装配 → 打印监听面 → Ctrl-C 等待 → 优雅停机。
///
/// 两个 CLI 入口同源共用（单一实现）：`nemesisbot quic relay`（分类管理入口）
/// 与 `nemesisbot gateway --quic-relay`（统一入口，与桥中继 `--relay` 旗标
/// 先例同形态——经 GatewayCtx::assemble 早退调进本函数）。与桥中继
/// `run_relay` 同款 fail-closed 纪律：`relay.token` 空 = 拒绝启动
/// （纯中继面向公网，无鉴权裸奔不可放行——此前的「警告后 AllowAll」偏差已
/// 对齐收口）。feature=quic-relay（中继服务器面：iroh-relax/server +
/// rustls + axum 随 feature 进出）。
#[cfg(feature = "quic-relay")]
pub(crate) async fn run_relay_server(home: &Path) -> Result<(), String> {
    let (cfg, config_path) = resolved(home);
    let workspace = crate::common::workspace_path(home);
    let relay = &cfg.relay;

    if relay.token.trim().is_empty() {
        eprintln!(
            "[QuicRelay] 启动失败：config.quic.json 未配置 relay.token（访问令牌；\
空 = 无鉴权裸奔）。请在 config.quic.json relay.token 填入预共享令牌后重试\
（客户端侧 client.relay_token 需配对同值）。"
        );
        return Err("--quic-relay/quic relay requires relay.token to be set".to_string());
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
    println!("  访问控制: SharedToken（客户端需配对 client.relay_token）");

    let mut handle =
        nemesis_cluster::roaming::relay_server::spawn_relay_server(relay, &workspace).await?;
    if let Some(http) = handle.http_addr {
        println!("  HTTP relay transport: {http}");
    }
    if let Some(quic) = handle.quic_addr {
        println!("  QUIC relay transport: {quic}");
    }
    if let Some(reg) = handle.registry.as_ref() {
        println!(
            "  成员注册表 (rendezvous): {}（自动发现面；设备接入即报到、周期拉名册）",
            reg.addr
        );
    } else if relay.registry_port == 0 {
        println!("  成员注册表 (rendezvous): 关（registry_port=0，纯传输形态）");
    }
    println!(
        "  对外 relay URL（客户端 client.relay_url 填这个）: {}",
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
    let client = &cfg.client;

    println!("client 段解析产物（{}）:", config_path.display());
    println!(
        "  客户端自建中继 (client.relay_url): {}",
        if client.relay_url.is_empty() {
            "（空 = n0 公共中继）"
        } else {
            &client.relay_url
        }
    );
    println!(
        "  客户端中继令牌 (client.relay_token): {}",
        if client.relay_token.is_empty() {
            "（未设）"
        } else {
            "（已设）"
        }
    );
    println!(
        "  客户端名册地址 (client.rendezvous_url): {}",
        if client.rendezvous_url.is_empty() {
            "（空 = relay_url host + :3341 推导）"
        } else {
            &client.rendezvous_url
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
        println!("本机 iroh 身份: 未生成（`quic keygen` 或首次启用 roaming 的 gateway 自动生成）");
    }
    println!("relay 段解析产物（服务端自建中继）:");
    println!("  gateway 进程内启动 (enabled): {}", relay.enabled);
    println!("  HTTP 端口: {}", relay.http_port);
    println!("  QUIC 端口: {}", relay.quic_port);
    println!(
        "  成员注册表端口 (registry_port): {}（{}；0=关）",
        relay.registry_port,
        if relay.registry_port == 0 {
            "自动发现关"
        } else if relay.token.is_empty() {
            "token 未设 = 不拉起（无门禁名册不设防）"
        } else {
            "随中继拉起"
        }
    );
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
    relay_server_status_lines(&workspace, relay);
    Ok(())
}

/// 服务端证书/对外 URL 段（quic-relay 双形态）：feature 在 = 完整证书状态 +
/// 对外 URL；feature 关 = 诚实行（配置键解析照常展示——server 段是纯数据）。
#[cfg(feature = "quic-relay")]
fn relay_server_status_lines(
    workspace: &Path,
    relay: &nemesis_cluster::roaming::config::RelaySettings,
) {
    match relay.tls {
        RelayTls::Manual => {
            println!("  证书: {}（用户自备）", relay.cert_path);
            println!("  私钥: {}（用户自备）", relay.key_path);
        }
        _ => {
            let cert = nemesis_cluster::roaming::relay_server::relay_cert_path(workspace);
            let key = nemesis_cluster::roaming::relay_server::relay_key_path(workspace);
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
}

#[cfg(not(feature = "quic-relay"))]
fn relay_server_status_lines(
    _workspace: &Path,
    _relay: &nemesis_cluster::roaming::config::RelaySettings,
) {
    println!("  （quic-relay feature 未编译：中继服务器面不可用，以上为配置解析产物）");
}
