//! QUIC 中继显示面（纯函数，零 iroh 依赖；QUIC 插件化 M1 后宿主侧仅存的
//! relay 相关代码——运行面整体迁入 plugin_roaming）。
//!
//! 三个消费者：
//! - CLI `quic status`（[`crate::roaming`] 经 nemesisbot 穿透）；
//! - WSAPI `quic.relay.*`（nemesis-web handlers/quic.rs）；
//! - 插件侧证书路径契约的对账基准（plugin-roaming relay_server.rs 文档
//!   「与宿主 relay_info.rs 同源」——落盘路径不许两边漂移）。
//!
//! `advertised_relay_url` 依赖宿主本机 IP 探测（[`crate::network::get_local_ip`]），
//! 故留宿主不进插件（插件侧无此函数——对外 URL 由宿主显示面计算）。

use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};

use crate::roaming::config::RelaySettings;

/// 中继证书目录（`<workspace>/cluster/relay/`）。
pub fn relay_dir(workspace: &Path) -> PathBuf {
    workspace.join("cluster").join("relay")
}

/// 自签证书 PEM 路径（`relay_cert.pem`）。
pub fn relay_cert_path(workspace: &Path) -> PathBuf {
    relay_dir(workspace).join("relay_cert.pem")
}

/// 自签私钥 PEM 路径（`relay_key.pem`；Unix 0600）。
pub fn relay_key_path(workspace: &Path) -> PathBuf {
    relay_dir(workspace).join("relay_key.pem")
}

/// 对外交付的 relay URL（客户端填进自己 `roaming.relay_url` 的形态）。
///
/// `external_host` 空 = 自动探测本机主 IP（`Cluster::start` 同款枚举；
/// 全回环环境兜底 127.0.0.1——仅 loopback 测试可达，HTTP transport 形态）。
pub fn advertised_relay_url(settings: &RelaySettings) -> String {
    let host = if settings.external_host.trim().is_empty() {
        let ip = crate::network::get_local_ip();
        if ip.is_loopback() || ip.is_unspecified() {
            Ipv4Addr::LOCALHOST.to_string()
        } else {
            ip.to_string()
        }
    } else {
        settings.external_host.trim().to_string()
    };
    format!("http://{host}:{}", settings.http_port)
}
