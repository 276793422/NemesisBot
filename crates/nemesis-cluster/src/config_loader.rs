//! Cluster configuration loader.
//!
//! Reads cluster configuration from a JSON file and produces a `ClusterConfig`.
//! For TOML-based static/dynamic configuration, see `cluster_config.rs`.

use std::path::Path;

use crate::types::ClusterConfig;

/// Error type for configuration loading.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("TOML parse error: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("TOML serialization error: {0}")]
    TomlSerialize(#[from] toml::ser::Error),
}

/// Load cluster configuration from a JSON file.
pub fn load_config(path: &Path) -> Result<ClusterConfig, ConfigError> {
    let content = nemesis_utils::fsutil::read_config_string(path)?;
    let config: ClusterConfig = serde_json::from_str(&content)?;
    Ok(config)
}

/// Save cluster configuration to a JSON file.
pub fn save_config(path: &Path, config: &ClusterConfig) -> Result<(), ConfigError> {
    let json = serde_json::to_string_pretty(config)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // REL-002（2026-09-23）：裸写改统一原子写入（config.cluster.json 含 token）。
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), json.as_bytes(), 0o600)
        .map_err(|e| ConfigError::Io(std::io::Error::other(e)))?;
    Ok(())
}

/// Load configuration from an optional path, falling back to defaults.
pub fn load_or_default(path: Option<&Path>) -> ClusterConfig {
    match path {
        // LOW-13（2026-10-17 四轮审计）：回落默认前 WARN 留痕——旧实现
        // `unwrap_or_default` 静默吞掉读取/解析失败，配置文件损坏时用户
        // 看到的是「默认值生效」却无从知道为什么（legacy 便捷面，调用方
        // 不检查 Result，日志是唯一可观测面）。
        Some(p) => load_config(p).unwrap_or_else(|e| {
            tracing::warn!(
                path = %p.display(),
                error = %e,
                "[ConfigLoader] 配置读取失败，回落默认值"
            );
            ClusterConfig::default()
        }),
        None => ClusterConfig::default(),
    }
}

/// App configuration (from config.cluster.json).
/// Mirrors Go's `AppConfig` struct.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_udp_port")]
    pub port: u16,
    #[serde(default = "default_rpc_port")]
    pub rpc_port: u16,
    #[serde(default = "default_broadcast_interval")]
    pub broadcast_interval: u64,
    /// LLM request timeout in seconds for B-side peer_chat processing.
    /// This is the maximum time to wait for the LLM API to respond.
    /// Default: 7200 (2 hours). Set to 0 to disable timeout.
    #[serde(default = "default_llm_timeout_secs")]
    pub llm_timeout_secs: u64,
    /// G2: RPC 健康探针循环间隔（秒）。0 = 关闭主动探测（只保留 UDP 广播
    /// 被动过期检查）。默认 60。
    #[serde(default = "default_health_check_interval_secs")]
    pub health_check_interval_secs: u64,
    /// G2: 连续探针失败多少次后把节点标记 Offline（自愈 = 一次探针成功即回
    /// Online）。默认 3。
    #[serde(default = "default_health_check_failure_threshold")]
    pub health_check_failure_threshold: u32,
    /// G3: discovery announce 过期阈值（秒）。超过该时长的 announce 被丢弃
    /// （WARN 限频），0 = 关闭过期丢弃。默认 120（与协议默认一致）。
    #[serde(default = "default_announce_expiry_secs")]
    pub announce_expiry_secs: i64,
    /// CFG-09（2026-09-16）：RPC 鉴权 + discovery 加密共用 token。磁盘上
    /// 由 `Cluster::load_rpc_auth_token` / discovery 裸 JSON 读取；此前
    /// typed 无此字段——`save_app_config` 全量覆盖写会把用户已配置的
    /// `token` 静默抹掉（CFG-01 同族 round-trip 删键）。纳入 typed 后
    /// 读-改-写路径 token 保真；空串 = 无鉴权（旧行为不变）。
    #[serde(default)]
    pub token: String,
    /// 批次四（2026-09-19）：节点显示名。空 = 自动解析链（hostname，
    /// 纯容器兜底 `Bot {id8}`）；显式配置时免疫撞名后缀（用户意志优先，
    /// 重名无害——唯一性靠 node_id）。Default impl 同步补——CFG-09 同族
    /// 防删键（typed 化防 `save_app_config` 全量覆盖写静默抹键）。
    #[serde(default)]
    pub node_name: String,
    /// 集群专业职能框架（D5）：B 端 worker 行为契约注入稳定前缀
    /// （`cluster_worker.md`：数据非指令防御 + 汇报契约 + 工作区纪律）。
    /// 缺省 on——契约是质量底线不是可选项；关掉 = 回退纯人格提示词。
    #[serde(default = "default_worker_discipline")]
    pub worker_discipline: bool,
    /// L13-b（2026-11-03 三期）：mandate 签名授权段。全部键可缺省（serde
    /// default）——旧 config.cluster.json 零迁移；默认形态 = A 端签名开 +
    /// B 端 verify（mandate 缺失放行 = 旧行为，旧节点兼容）。
    #[serde(default)]
    pub mandate: MandateConfig,
}

fn default_worker_discipline() -> bool {
    true
}

/// L13-b：mandate 签名授权配置（config.cluster.json `[mandate]` 段；JSON
/// 嵌套对象）。模式词表单一真相源 [`MandateConfig::mode`] doc。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MandateConfig {
    /// B 端 posture：`off`（忽略 mandate）/ `verify`（在场即验，缺失放行
    /// = 旧行为）/ `require`（缺失也拒 = 严格 opt-in，防剥离攻击，代价 =
    /// 断旧 A）。缺省 verify。未知值按 verify 处理 + WARN（装配期）。
    #[serde(default = "default_mandate_mode")]
    pub mode: String,
    /// A 端：出站 peer_chat 附签名 mandate。缺省 true；密钥不可用时禁用
    /// 签名 + warn（fail-open 到兼容态，不崩 gateway）。
    #[serde(default = "default_mandate_sign_enabled")]
    pub sign_enabled: bool,
    /// A 端授权面：空数组 = 不限（None 语义）；显式列名 = 白名单（v1 精确
    /// 匹配无通配符）。缺省空 = 不限。
    #[serde(default)]
    pub tool_allowlist: Vec<String>,
    /// mandate 时间窗（秒）：expires_at = signed_at + window_secs。缺省
    /// 7200（2 小时）。
    #[serde(default = "default_mandate_window_secs")]
    pub window_secs: u64,
    /// A 端 token 预算（0 = 不限，缺省）。
    #[serde(default)]
    pub max_tokens: u64,
    /// A 端工具调用次数预算（0 = 不限，缺省）。
    #[serde(default)]
    pub max_tool_calls: u64,
}

fn default_mandate_mode() -> String {
    "verify".into()
}

fn default_mandate_sign_enabled() -> bool {
    true
}

fn default_mandate_window_secs() -> u64 {
    7200
}

impl Default for MandateConfig {
    fn default() -> Self {
        Self {
            mode: default_mandate_mode(),
            sign_enabled: default_mandate_sign_enabled(),
            tool_allowlist: Vec::new(),
            window_secs: default_mandate_window_secs(),
            max_tokens: 0,
            max_tool_calls: 0,
        }
    }
}

impl MandateConfig {
    /// mode 词表解析（单一真相源）：off/verify/require；未知值 = verify
    /// （保守缺省 + 调用方 WARN）。
    pub fn resolve_mode(&self) -> MandateMode {
        match self.mode.trim().to_lowercase().as_str() {
            "off" => MandateMode::Off,
            "require" => MandateMode::Require,
            _ => MandateMode::Verify,
        }
    }

    /// A 端授权面投影：空数组 = 不限（None）。
    pub fn allowlist_or_unlimited(&self) -> Option<Vec<String>> {
        if self.tool_allowlist.is_empty() {
            None
        } else {
            Some(self.tool_allowlist.clone())
        }
    }
}

/// B 端 mandate posture（`MandateConfig::resolve_mode` 产物）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MandateMode {
    /// 忽略 mandate（不验不拒——回滚开关）。
    Off,
    /// 在场即验，缺失放行（默认；旧行为兼容）。
    Verify,
    /// 缺失也拒（严格 opt-in；防剥离攻击）。
    Require,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            port: default_udp_port(),
            rpc_port: default_rpc_port(),
            broadcast_interval: default_broadcast_interval(),
            llm_timeout_secs: default_llm_timeout_secs(),
            health_check_interval_secs: default_health_check_interval_secs(),
            health_check_failure_threshold: default_health_check_failure_threshold(),
            announce_expiry_secs: default_announce_expiry_secs(),
            token: String::new(),
            node_name: String::new(),
            worker_discipline: default_worker_discipline(),
            mandate: MandateConfig::default(),
        }
    }
}

fn default_udp_port() -> u16 {
    11949
}

fn default_rpc_port() -> u16 {
    21949
}

fn default_broadcast_interval() -> u64 {
    30
}

fn default_llm_timeout_secs() -> u64 {
    7200 // 2 hours
}

fn default_health_check_interval_secs() -> u64 {
    60
}

fn default_health_check_failure_threshold() -> u32 {
    3
}

fn default_announce_expiry_secs() -> i64 {
    crate::discovery::DEFAULT_EXPIRY_THRESHOLD_SECS
}

/// Load app configuration from workspace/config/config.cluster.json.
///
/// 解析失败的文件按空配置启动（lenient，不阻断集群装配），但必须留
/// WARN——「配置明明写了却不生效」若无日志就是无声黑洞（集群完备性
/// 加固 2026-09-11：此前读失败/解析失败双双静默）。
pub fn load_app_config(workspace: &Path) -> AppConfig {
    // 委托 nemesis-path 唯一拼接点。
    let config_path = nemesis_path::resolve_cluster_config_path_in_workspace(workspace);
    if config_path.exists() {
        match nemesis_utils::fsutil::read_config_string(&config_path) {
            Ok(content) => match serde_json::from_str(&content) {
                Ok(cfg) => cfg,
                Err(e) => {
                    tracing::warn!(
                        path = %config_path.display(),
                        error = %e,
                        "[Cluster] config.cluster.json parse failed; using default app config (cluster likely disabled)"
                    );
                    AppConfig::default()
                }
            },
            Err(e) => {
                tracing::warn!(
                    path = %config_path.display(),
                    error = %e,
                    "[Cluster] config.cluster.json unreadable; using default app config"
                );
                AppConfig::default()
            }
        }
    } else {
        AppConfig::default()
    }
}

/// Save app configuration to workspace/config/config.cluster.json.
///
/// ⚠️ **危险**：`AppConfig` 是全量覆盖写（typed round-trip）。用它保存一个
/// **默认构造**的 `AppConfig` 会把用户已配置的 `enabled`/`port`/`rpc_port`/
/// `token` 等**全部抹回默认值**。新增写入方前先确认你是「读-改-写」同一份
/// （`load_app_config` → 改字段 → 本函数），不要拿 default 直接存。
/// （集群完备性加固 2026-09-11 审计注记；当前生产无调用方，本函数是
/// 潜在脚枪，留 doc 而非删除——恢复/新增消费方时按上述纪律使用。）
pub fn save_app_config(workspace: &Path, config: &AppConfig) -> Result<(), ConfigError> {
    // 委托 nemesis-path 唯一拼接点。
    let config_path = nemesis_path::resolve_cluster_config_path_in_workspace(workspace);
    let json = serde_json::to_string_pretty(config)?;
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // REL-002（2026-09-23）：裸写改统一原子写入（含 token）。
    nemesis_utils::write_file_atomic(&config_path.to_string_lossy(), json.as_bytes(), 0o600)
        .map_err(|e| ConfigError::Io(std::io::Error::other(e)))?;
    Ok(())
}

#[cfg(test)]
mod tests;

// 覆盖率补充批次：WARN 宽容臂 + round-trip。
#[cfg(test)]
mod cov_tests;
