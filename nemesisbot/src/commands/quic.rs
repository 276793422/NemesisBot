//! QUIC CLI（M4 起恒编译：cluster-roaming/quic-relay feature 已删，代理层
//! 恒在；2026-10-23 与集群解耦：QUIC 自身配置/身份/状态是 QUIC 层资产，
//! 不寄居集群命令族）。
//!
//! - `quic keygen`：生成本机 iroh 漫游身份（自 `cluster roaming keygen` 挪来
//!   ——命令面挪、身份文件物理路径不动，免得平白多一步迁移）。
//! - `quic status`：只读状态面——client 段（客户端连接参数）+ relay 段
//!   （服务端配置解析产物、证书状态；零副作用，不触发证书生成）。
//!
//! 服务端面（`quic relay` 独立进程）运行面在 **plugin_roaming C-ABI 插件**
//! （iroh-relay/rustls/axum 栈随插件迁出主二进制）：宿主只剩 token
//! fail-closed 闸 + 装载 + 调插件 start/stop。两个入口同源共用
//! `run_relay_server`（同一实现，不冲突）——`quic relay`：分类管理入口；
//! `gateway --quic-relay`：统一入口。两者都读 **config.quic.json** `relay`
//! 段装配（`relay.enabled` 只约束 gateway 进程内启动闸，独立进程形态不受
//! 此闸约束——命令本身即显式意志）；token 空 = fail-closed 拒绝启动（插件
//! 内同值兜底拒启，宿主前置检查只为了更清楚的错误文案）。首读触发旧位置
//!（cluster.json roaming 段）一次性迁移（[`ensure_quic_config`]，幂等）。
//! 插件缺席 = 诚实报错（CLI 是显式意志，没有降级可言）。
//!
//! BUG-20261025-20：「恒编译」撞上 cluster 本身可裁——本模块引用的
//! `nemesis_cluster` 依赖是 optional（随 cluster feature），minimal-IoT 等
//! 关 cluster 的裁剪组合此前直接 E0433 编译红。修法 = 解析/桥接面整体
//! cfg 门控（voice「未编译诚实报」同款惯例）：命令面（clap 枚举）恒注册、
//! L1 清单稳定；cluster 关 = 三个动作执行面诚实报「未编译」。

use std::path::Path;

#[cfg(feature = "cluster")]
use nemesis_cluster::roaming::config::RelayTls;
#[cfg(feature = "cluster")]
use nemesis_cluster::roaming::quic_config::{QuicConfig, ensure_quic_config, resolved_quic};

/// `nemesisbot quic <action>` 入口。
pub async fn run(home: &Path, action: QuicAction) -> Result<(), String> {
    #[cfg(not(feature = "cluster"))]
    {
        let _ = (home, action);
        return Err(
            "cluster feature 未编译：QUIC 配置解析/身份桥面在 nemesis-cluster（\
minimal-IoT 等裁剪形态缺席）——如需 quic 命令请启用 cluster feature 重新编译"
                .to_string(),
        );
    }

    #[cfg(feature = "cluster")]
    {
        match action {
            QuicAction::Relay => run_relay_server(home).await,
            QuicAction::Keygen { force } => run_keygen(home, force).await,
            QuicAction::Status => show_status(home).await,
        }
    }
}

/// `quic` 子命令动作。
#[derive(clap::Subcommand, Debug)]
pub enum QuicAction {
    /// 启动自建 QUIC 中继服务器（内网穿透；Ctrl-C 停机；运行面在 plugin_roaming）
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
#[cfg(feature = "cluster")]
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
/// QUIC 插件化 M2：身份面（seed→node_id 派生是 ed25519 运算）在
/// plugin_roaming——CLI 先装载插件（幂等）再走桥；插件缺席 = 诚实报错
/// （CLI 是显式意志，没有降级可言）。
#[cfg(feature = "cluster")]
async fn run_keygen(home: &Path, force: bool) -> Result<(), String> {
    use crate::roaming_plugin_loader::{RoamingLoadOutcome, load_roaming_plugin, relay_init_json};
    use nemesis_cluster::roaming::plugin_bridge::{self, BridgeOutcome};
    use nemesis_cluster::roaming::roaming_key_path;

    // resolved() 顺带完成旧位置迁移（幂等）——cfg 不用但迁移副作用要。
    let (_cfg, config_path) = resolved(home);
    // roaming_* 收的是 workspace 根（与 gateway setup_roaming、WSAPI 面同
    // 口径），不是 home——错传会把 key 写到 `.nemesisbot/` 而非
    // `.nemesisbot/workspace/`，gateway 读不到。
    let workspace = crate::common::workspace_path(home);
    let key_path = roaming_key_path(&workspace);

    // 装载（幂等——gateway 形态已装载时直接 Loaded）。身份文件解析根 =
    // workspace（init 时经 get_workspace_dir 注入插件，与 gateway 同口径）。
    match load_roaming_plugin(
        &workspace,
        &relay_init_json(&config_path),
        &nemesis_path::workspace_config_dir(&workspace),
    ) {
        RoamingLoadOutcome::Loaded => {}
        RoamingLoadOutcome::DisabledBySwitch => {
            return Err(
                "plugin_roaming 已停用（本地插件库开关 config.plugin.native.json）\
——身份面在插件内，请启用后重试"
                    .to_string(),
            );
        }
        RoamingLoadOutcome::Absent => {
            return Err(
                "plugin_roaming 未找到（exe 旁 plugins/plugin_roaming 缺席）——\
身份面在插件内，请随发行包安装 plugins/plugin_roaming/plugin_roaming.dll"
                    .to_string(),
            );
        }
        RoamingLoadOutcome::Rejected(reason) => {
            return Err(format!("plugin_roaming 拒载: {reason}"));
        }
    }

    let info = match if force {
        plugin_bridge::identity_keygen_force().await
    } else {
        plugin_bridge::identity_ensure().await
    } {
        BridgeOutcome::Ok(info) => info,
        BridgeOutcome::Absent => {
            return Err("plugin_roaming 未装载（桥槽空）——身份面在插件内".to_string());
        }
        BridgeOutcome::Failed(e) => return Err(e),
    };
    println!("Roaming identity key: {}", key_path.display());
    println!(
        "  状态: {}",
        if info.created {
            "新生成"
        } else {
            "已存在（加载）"
        }
    );
    println!("  node_id: {}", info.iroh_id);
    println!("  完整地址: iroh:{}", info.iroh_id);
    if !info.created {
        println!("  （密钥已存在未变更；轮换加 --force——对端须重新信任新 node_id）");
    }
    Ok(())
}

/// 独立中继进程：token fail-closed → 装载 plugin_roaming → 插件 start →
/// 打印监听面 → Ctrl-C → 插件 stop。
///
/// 两个 CLI 入口同源共用（单一实现）：`nemesisbot quic relay`（分类管理入口）
/// 与 `nemesisbot gateway --quic-relay`（统一入口，与桥中继 `--relay` 旗标
/// 先例同形态——经 GatewayCtx::assemble 早退调进本函数）。QUIC 插件化 M1：
/// iroh-relay 服务器栈不在本进程——插件内自持 tokio runtime 承载 HTTP/QUIC
/// 监听与成员注册表；宿主经 C-ABI 调 start/stop（同步 FFI 内投插件自持
/// runtime，宿主侧 loader 走 spawn_blocking）。与桥中继 `run_relay` 同款
/// fail-closed 纪律：`relay.token` 空 = 拒绝启动（纯中继面向公网，无鉴权
/// 裸奔不可放行）。插件缺席/拒载 = 诚实报错退出（显式意志不降级）。
#[cfg(feature = "cluster")]
pub(crate) async fn run_relay_server(home: &Path) -> Result<(), String> {
    use crate::roaming_plugin_loader::{
        RoamingLoadOutcome, load_roaming_plugin, roaming_start, roaming_stop,
    };

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

    println!("QUIC 中继服务器（自建 iroh relay，plugin_roaming 承载）");
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

    // 插件装载（幂等——gateway 已装载时直接 Loaded）。init 配置 = relay 段
    // raw 直传（loader 纪律：不走 typed 往返）。
    let init_json = crate::roaming_plugin_loader::relay_init_json(&config_path);
    match load_roaming_plugin(
        &workspace,
        &init_json,
        &nemesis_path::workspace_config_dir(&workspace),
    ) {
        RoamingLoadOutcome::Loaded => {}
        RoamingLoadOutcome::DisabledBySwitch => {
            return Err(
                "plugin_roaming 已停用（本地插件库开关 config.plugin.native.json）\
——中继运行面在插件内，请启用后重试"
                    .to_string(),
            );
        }
        RoamingLoadOutcome::Absent => {
            return Err(
                "plugin_roaming 未找到（exe 旁 plugins/plugin_roaming 缺席）——\
中继运行面在插件内，请随发行包安装 plugins/plugin_roaming/plugin_roaming.dll"
                    .to_string(),
            );
        }
        RoamingLoadOutcome::Rejected(reason) => {
            return Err(format!("plugin_roaming 拒载: {reason}"));
        }
    }

    // 插件内 token 兜底 fail-closed（token_missing）+ 实际拉起。回执 =
    // status 快照形状（running/http_addr/quic_addr/registry_addr）。
    let status = roaming_start()
        .await
        .map_err(|e| format!("中继启动失败: {e}"))?;
    if let Some(http) = status.get("http_addr").and_then(|v| v.as_str()) {
        println!("  HTTP relay transport: {http}");
    }
    if let Some(quic) = status.get("quic_addr").and_then(|v| v.as_str()) {
        println!("  QUIC relay transport: {quic}");
    }
    match status.get("registry_addr").and_then(|v| v.as_str()) {
        Some(reg) => {
            println!("  成员注册表 (rendezvous): {reg}（自动发现面；设备接入即报到、周期拉名册）")
        }
        None => println!("  成员注册表 (rendezvous): 关（registry_port=0，纯传输形态）"),
    }
    println!(
        "  对外 relay URL（客户端 client.relay_url 填这个）: {}",
        nemesis_cluster::roaming::relay_info::advertised_relay_url(relay)
    );
    if relay.tls == RelayTls::SelfSigned {
        println!(
            "  证书: {}",
            nemesis_cluster::roaming::relay_info::relay_cert_path(&workspace).display()
        );
    }
    println!("就绪。Ctrl-C 停机。");

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| format!("监听 Ctrl-C 失败: {e}"))?;
    println!("停机中…");
    match roaming_stop().await {
        Ok(_) => println!("已停机。"),
        Err(e) => println!("停机异常（中继可能仍在运行）: {e}"),
    }
    Ok(())
}

/// `quic status` 只读状态面。
#[cfg(feature = "cluster")]
async fn show_status(home: &Path) -> Result<(), String> {
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
    // [node].iroh_id 声明可对账）。QUIC 插件化 M2：身份派生（seed→node_id
    // 是 ed25519 运算）在插件内——装载走桥读；缺席/失败 = 诚实注记
    // （状态面不因身份读不到而失败，配置段照常展示）。
    match crate::roaming_plugin_loader::load_roaming_plugin(
        &workspace,
        &crate::roaming_plugin_loader::relay_init_json(&config_path),
        &nemesis_path::workspace_config_dir(&workspace),
    ) {
        crate::roaming_plugin_loader::RoamingLoadOutcome::Loaded => {
            match nemesis_cluster::roaming::plugin_bridge::get_iroh_id().await {
                nemesis_cluster::roaming::plugin_bridge::BridgeOutcome::Ok(hex) => {
                    println!("本机 iroh 身份: iroh:{hex}");
                    println!("  对端静态条目写法: address = \"iroh:{hex}@<本机对外 relay_url>\"");
                }
                nemesis_cluster::roaming::plugin_bridge::BridgeOutcome::Failed(e) => {
                    println!("本机 iroh 身份: 读取失败（{e}）");
                }
                nemesis_cluster::roaming::plugin_bridge::BridgeOutcome::Absent => {
                    println!("本机 iroh 身份: 未装载（plugin_roaming 桥槽空）");
                }
            }
        }
        crate::roaming_plugin_loader::RoamingLoadOutcome::DisabledBySwitch => {
            println!("本机 iroh 身份: plugin_roaming 已停用（本地插件库开关）——身份面不装配");
        }
        crate::roaming_plugin_loader::RoamingLoadOutcome::Absent => {
            println!(
                "本机 iroh 身份: plugin_roaming 未安装（exe 旁 plugins/plugin_roaming 缺席——身份派生面在插件内）"
            );
        }
        crate::roaming_plugin_loader::RoamingLoadOutcome::Rejected(reason) => {
            println!("本机 iroh 身份: plugin_roaming 拒载（{reason}）");
        }
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

/// 服务端证书/对外 URL 段：完整证书状态 + 对外 URL。路径/URL 计算是纯函数
///（[`nemesis_cluster::roaming::relay_info`]），不触插件——状态面零副作用、
/// 不装载插件。
#[cfg(feature = "cluster")]
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
            let cert = nemesis_cluster::roaming::relay_info::relay_cert_path(workspace);
            let key = nemesis_cluster::roaming::relay_info::relay_key_path(workspace);
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
        nemesis_cluster::roaming::relay_info::advertised_relay_url(relay)
    );
}
