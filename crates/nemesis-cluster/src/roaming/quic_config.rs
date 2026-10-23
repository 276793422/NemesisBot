//! QUIC 中继配置解析（`<workspace>/config/config.quic.json`）。
//!
//! 解耦语义（2026-10-23，QUIC 中继与集群解耦 goal）：QUIC 层 = 传输设施 +
//! 自己的全部配置——服务端 `relay` 段（自建中继，字段与旧 `roaming.relay`
//! 一一对应）+ 客户端 `client` 段（连接三键，旧 `roaming` 段直挂键随迁）；
//! 集群层 = 消费者，只留「要不要使用 QUIC 打洞」开关
//! （config.cluster.json `roaming.enabled`，见 [`super::config`]）。
//!
//! 单一真相源：[`resolved_quic`] 纯函数——缺文件 / 坏 JSON / 缺段 /
//! 字段类型不符一律逐字段落默认值（与 [`super::config::resolved_roaming`]
//! 同一解析纪律；relay 段字段解析经 [`super::config::parse_relay_settings`]
//! 同源复用）。
//!
//! 旧位置首读迁移：[`ensure_quic_config`]——quic.json 不存在且
//! config.cluster.json 携带旧形态（`roaming.relay` 段非空或客户端三键任一
//! 非空）时一次性搬运落盘；旧键保留不删（避免回写冲突），解析只认新位置；
//! 幂等（quic.json 已在 = 无操作）。

use std::path::Path;

use super::config::{RelaySettings, str_field};

/// QUIC 客户端连接参数（config.quic.json `client` 段）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QuicClientConfig {
    /// 自建中继地址（如 `http://vps.example.com:3340`）；空 = n0 公共中继。
    pub relay_url: String,
    /// 中继访问令牌（与服务端 `relay.token` 配对；relay_url 空（n0 公共
    /// 中继）时无意义）。
    pub relay_token: String,
    /// rendezvous 注册表地址（如 `http://vps.example.com:3341`）；空 = 从
    /// relay_url host + 默认端口推导（自建中继同机部署零配置）。
    pub rendezvous_url: String,
}

/// config.quic.json 解析产物（服务端 `relay` 段 + 客户端 `client` 段）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QuicConfig {
    /// 服务端自建中继设置（gateway 独立启动闸 = `relay.enabled`，与集群
    /// 零关联——集群关了中继照跑）。
    pub relay: RelaySettings,
    /// 客户端连接参数（配置归 QUIC 层，消费在集群层——集群 setup_roaming
    /// 改读本段，`roaming` 段不再承载连接参数）。
    pub client: QuicClientConfig,
}

/// 解析 config.quic.json（纯函数，零副作用）。
///
/// 任何解析失败（缺文件 / 坏 JSON / 缺段 / 字段类型不符）逐字段落默认。
pub fn resolved_quic(quic_config_path: &Path) -> QuicConfig {
    let raw = std::fs::read_to_string(quic_config_path).ok();
    let cfg = raw
        .as_deref()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());
    parse_quic_value(cfg.as_ref())
}

/// 从（可选的）quic.json JSON 树解析两段——测试友好拆分。
pub(crate) fn parse_quic_value(cfg: Option<&serde_json::Value>) -> QuicConfig {
    let mut out = QuicConfig::default();
    let Some(root) = cfg else {
        return out;
    };
    if let Some(relay) = root.get("relay") {
        out.relay = super::config::parse_relay_settings(relay);
    }
    if let Some(client) = root.get("client") {
        out.client.relay_url = str_field(client.get("relay_url"));
        out.client.relay_token = str_field(client.get("relay_token"));
        out.client.rendezvous_url = str_field(client.get("rendezvous_url"));
    }
    out
}

/// 旧位置首读迁移：config.cluster.json 旧形态 → config.quic.json。
///
/// 触发条件（三者同时成立）：quic.json 不存在；cluster.json 可解析；
/// `roaming.relay` 段非空 **或** 客户端三键（relay_url/relay_token/
/// rendezvous_url）任一非空。迁移 = 旧键原样搬运（`relay` 段 Value 级
/// 拷贝，未知键保留；客户端三键按解析同款 trim 后写入）——不走枚举再
/// 序列化，字节保真。cluster.json 不动（旧键保留不删，解析只认新位置）。
///
/// 返回是否发生迁移写盘。幂等（quic.json 已在 = false）；写盘失败 =
/// warn + false（调用方下次再触发，不影响本次读取的解析默认值）。
pub fn ensure_quic_config(cluster_config_path: &Path, quic_config_path: &Path) -> bool {
    if quic_config_path.exists() {
        return false;
    }
    let raw = std::fs::read_to_string(cluster_config_path).ok();
    let Some(cfg) = raw
        .as_deref()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
    else {
        return false;
    };
    let Some(roaming) = cfg.get("roaming") else {
        return false;
    };
    let relay_obj = roaming
        .get("relay")
        .and_then(|v| v.as_object())
        .filter(|o| !o.is_empty());
    let client_url = str_field(roaming.get("relay_url"));
    let client_token = str_field(roaming.get("relay_token"));
    let client_rdv = str_field(roaming.get("rendezvous_url"));
    let has_client_keys =
        !client_url.is_empty() || !client_token.is_empty() || !client_rdv.is_empty();
    if relay_obj.is_none() && !has_client_keys {
        // 无旧形态（新装默认模板不含 roaming/relay 段）——不生成文件。
        return false;
    }

    // 与其他 config 写方（WSAPI relay 保存 / onboard）同锁串行，防并发
    // 迁移与保存交错。
    let _cfg_serial = nemesis_config::config_write_lock();
    let quic = serde_json::json!({
        "relay": relay_obj
            .map(|o| serde_json::Value::Object(o.clone()))
            .unwrap_or_else(|| serde_json::json!({})),
        "client": {
            "relay_url": client_url,
            "relay_token": client_token,
            "rendezvous_url": client_rdv,
        },
    });
    if let Some(parent) = quic_config_path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        tracing::warn!("quic 配置迁移失败（建目录 {}）: {}", parent.display(), e);
        return false;
    }
    let Ok(json) = serde_json::to_string_pretty(&quic) else {
        return false;
    };
    if let Err(e) = nemesis_utils::write_file_atomic(
        &quic_config_path.to_string_lossy(),
        json.as_bytes(),
        0o600,
    ) {
        tracing::warn!(
            "quic 配置迁移写盘失败（{}）: {}",
            quic_config_path.display(),
            e
        );
        return false;
    }
    tracing::info!(
        "检测到旧版集群配置中的 QUIC 中继设置，已迁移到 {}（旧键保留不删，解析只认新位置）",
        quic_config_path.display()
    );
    true
}

#[cfg(test)]
mod tests;
