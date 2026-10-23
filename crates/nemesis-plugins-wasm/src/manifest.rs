//! 插件 manifest（`plugin.toml`）解析与校验。
//!
//! 形态（TOML；根级 `signature` 表由验签器只剥参与规范字节计算）：
//!
//! ```toml
//! api-version = 1
//! slug = "translate"
//! kind = "tool"              # tool | observer | policy（一 manifest 一 kind 不混装）
//! name = "Translate"
//! version = "0.1.0"
//! description = "..."
//! wasm = "plugin.wasm"       # 缺省
//! wasm-sha256 = "hex64"      # 必填
//! min-tier = "big"           # mini|normal|big，缺省 big（ Invocation 期闸）
//!
//! [permissions]
//! egress = []                # 精确域名 allowlist；空 = 无出站
//! allow-private = false
//! x-secret = ["api_key"]     # 凭据名声明（vault:plugin/<slug>/<name>）
//!
//! [limits]                   # 可选；只允许在宿主全局上限内收紧
//! fuel = 100_000_000
//!
//! [config-schema]            # 可选；实例配置声明（键 → 人读描述）
//! target_lang = "翻译目标语言"
//! ```

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::error::PluginError;

/// manifest 允许的最大字节数（防畸形大文件拖垮解析）。
pub const MANIFEST_MAX_BYTES: usize = 256 * 1024;

/// 插件 kind。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginKind {
    /// 工具插件（world `plugin-tool`）。
    Tool,
    /// 观察者插件（world `plugin-observer`）。
    Observer,
    /// 循环策略插件（world `plugin-loop-policy`；2026-10-22 样板工程批 4）。
    Policy,
}

impl PluginKind {
    /// manifest 字符串形态。
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Tool => "tool",
            Self::Observer => "observer",
            Self::Policy => "policy",
        }
    }
}

/// manifest `permissions` 段。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ManifestPermissions {
    /// 出站域名 allowlist（精确匹配；空 = 无出站能力）。
    #[serde(default)]
    pub egress: Vec<String>,
    /// 允许私网/回环目标（默认 false；测试场景显式开）。
    #[serde(default)]
    pub allow_private: bool,
    /// 凭据名声明（x-secret；进入 vault 别名约定）。
    #[serde(default, rename = "x-secret")]
    pub x_secret: Vec<String>,
}

/// manifest 根结构（`signature` 表不在此——由验签器从原始文档剥离）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct PluginManifest {
    /// 合同代际（必须等于宿主 [`crate::CONTRACT_API_VERSION`]）。
    pub api_version: u32,
    /// 插件 slug（目录名；`[a-z0-9-]{1,64}`）。
    pub slug: String,
    /// kind。
    pub kind: PluginKindSerde,
    /// 人读名称。
    pub name: String,
    /// 语义版本。
    pub version: String,
    /// 描述。
    #[serde(default)]
    pub description: String,
    /// wasm 文件名（相对插件目录；缺省 `plugin.wasm`）。
    #[serde(default = "default_wasm_file")]
    pub wasm: String,
    /// wasm sha256（hex64；安装期重算比对）。
    pub wasm_sha256: String,
    /// 最低调用档（mini/normal/big；缺省 big）。
    #[serde(default = "default_min_tier")]
    pub min_tier: String,
    /// 权限段。
    #[serde(default)]
    pub permissions: ManifestPermissions,
    /// 资源收紧覆盖（只允许收紧）。
    #[serde(default)]
    pub limits: BTreeMap<String, u64>,
    /// 实例配置声明（键 → 人读描述；x-secret 键不在此——凭据面分离）。
    #[serde(default)]
    pub config_schema: BTreeMap<String, String>,
    /// 接管声明（内置工具名清单；2026-10-23 工具接管层）。仅 `kind=tool`
    /// 可声明；deny-list 命中 parse 拒绝。声明 = 安装审批卡 + wasm.list
    /// 徽标双留痕，激活还要过「目标在 loop 注册表在场」闸（agent_factory 8d）。
    #[serde(default)]
    pub takeover: Vec<String>,
}

/// 接管 deny-list（宿主特权语义工具——沙盒内 guest 无法诚实兑现，接管
/// 一律拒绝）。与 `nemesis_agent::MOVE_TOOLS`（remote_executor_tool.rs）+
/// 进程/注册表/系统面同源；两清单相等性由 nemesisbot plugin_bridge tests
/// 的 drift 断言钉住（本 crate 依赖方向无 nemesis-agent，只能字面复制）。
pub const TAKEOVER_DENIED_TOOLS: &[&str] = &[
    // nemesis-agent::MOVE_TOOLS（执行体分离高危族——exec/脚本/文件写面/搜索）
    "exec",
    "run_script",
    "run_checks",
    "read_file",
    "write_file",
    "list_dir",
    "edit_file",
    "multiedit",
    "append_file",
    "delete_file",
    "move_file",
    "copy_file",
    "create_dir",
    "delete_dir",
    "grep",
    "git",
    // 进程/注册表/系统面（CRITICAL/宿主特权）
    "process_spawn",
    "process_kill",
    "process_exec",
    "process_list",
    "registry_read",
    "registry_write",
    "system_shutdown",
    "system_info",
    "hardware_i2c",
];

/// `builtin` 是否在接管 deny-list（宿主特权语义，沙盒内不可诚实兑现）。
#[must_use]
pub fn takeover_denied(builtin: &str) -> bool {
    TAKEOVER_DENIED_TOOLS.contains(&builtin)
}

/// kind 的 serde 中介（字符串 ↔ 枚举；错误信息可读）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PluginKindSerde(pub PluginKind);

impl std::ops::Deref for PluginKindSerde {
    type Target = PluginKind;
    fn deref(&self) -> &PluginKind {
        &self.0
    }
}

impl<'de> Deserialize<'de> for PluginKindSerde {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        match s.as_str() {
            "tool" => Ok(Self(PluginKind::Tool)),
            "observer" => Ok(Self(PluginKind::Observer)),
            "policy" => Ok(Self(PluginKind::Policy)),
            other => Err(serde::de::Error::custom(format!(
                "invalid kind '{other}' (expected tool | observer | policy)"
            ))),
        }
    }
}

fn default_wasm_file() -> String {
    "plugin.wasm".to_string()
}

fn default_min_tier() -> String {
    "big".to_string()
}

/// slug 合法性（`[a-z0-9-]{1,64}`，不以 - 开头结尾）。
#[must_use]
pub fn valid_slug(slug: &str) -> bool {
    (1..=64).contains(&slug.len())
        && slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !slug.starts_with('-')
        && !slug.ends_with('-')
        && slug != "all"
}

/// sha256 hex64 形态校验。
#[must_use]
pub fn valid_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

impl PluginManifest {
    /// 从目录里的 `plugin.toml` 解析 + 结构校验（不含签名/wasm 校验）。
    pub fn load_from_dir(dir: &std::path::Path) -> Result<(Self, String), PluginError> {
        let path = dir.join("plugin.toml");
        let raw = std::fs::read_to_string(&path)
            .map_err(|e| PluginError::Manifest(format!("read {}: {e}", path.display())))?;
        if raw.len() > MANIFEST_MAX_BYTES {
            return Err(PluginError::Manifest(format!(
                "manifest exceeds {MANIFEST_MAX_BYTES} bytes"
            )));
        }
        Self::parse(&raw)
    }

    /// 解析 + 结构校验（返回 manifest 与原始文本；签名校验见 `trust`）。
    pub fn parse(raw: &str) -> Result<(Self, String), PluginError> {
        let m: PluginManifest = toml_edit::de::from_str(raw)
            .map_err(|e| PluginError::Manifest(format!("parse: {e}")))?;
        m.validate()?;
        Ok((m, raw.to_string()))
    }

    /// 结构校验（与签名无关的字段级约束）。
    pub fn validate(&self) -> Result<(), PluginError> {
        if self.api_version != crate::CONTRACT_API_VERSION {
            return Err(PluginError::ContractVersion {
                manifest: self.api_version,
                supported: crate::CONTRACT_API_VERSION,
            });
        }
        if !valid_slug(&self.slug) {
            return Err(PluginError::Manifest(format!(
                "invalid slug: {}",
                self.slug
            )));
        }
        if !valid_sha256_hex(&self.wasm_sha256) {
            return Err(PluginError::Manifest(
                "wasm-sha256 must be 64 hex chars".into(),
            ));
        }
        if !matches!(self.min_tier.as_str(), "mini" | "normal" | "big") {
            return Err(PluginError::Manifest(format!(
                "invalid min-tier: {} (mini|normal|big)",
                self.min_tier
            )));
        }
        if self.name.len() > 128 {
            return Err(PluginError::Manifest("name exceeds 128 chars".into()));
        }
        if self.version.len() > 64 {
            return Err(PluginError::Manifest("version exceeds 64 chars".into()));
        }
        // egress allowlist 形态：host 或 host:port。
        for entry in &self.permissions.egress {
            let e = entry.to_lowercase();
            if e.is_empty()
                || e.starts_with('.')
                || e.contains('/')
                || e.contains('*')
                || e.split(':').count() > 2
            {
                return Err(PluginError::Manifest(format!(
                    "invalid egress entry (exact host or host:port only): {entry}"
                )));
            }
        }
        // x-secret 名形态：标识符样。
        for s in &self.permissions.x_secret {
            if s.is_empty()
                || s.len() > 64
                || !s
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                return Err(PluginError::Manifest(format!("invalid x-secret name: {s}")));
            }
        }
        // config-schema 键不与 x-secret 冲突（同名键语义分裂）。
        for key in self.config_schema.keys() {
            if self.permissions.x_secret.iter().any(|s| s == key) {
                return Err(PluginError::Manifest(format!(
                    "config-schema key '{key}' collides with x-secret declaration"
                )));
            }
            if key.is_empty() || key.len() > 64 {
                return Err(PluginError::Manifest(format!(
                    "invalid config-schema key (1-64 chars): {key}"
                )));
            }
        }
        // policy kind 权限收紧（2026-10-22 批 4）：循环策略组件决策纯计算面
        // ——无凭据（x-secret）、无直网（egress/allow-private）。数据来源只
        // 有宿主投影入参 + config-get + workspace-read + llm-complete +
        // tool-invoke（借道原生工具过安全 8 层全闸）。
        if *self.kind == PluginKind::Policy {
            if !self.permissions.egress.is_empty() {
                return Err(PluginError::Manifest(
                    "kind=policy forbids permissions.egress (policy components have no direct network access)".into(),
                ));
            }
            if self.permissions.allow_private {
                return Err(PluginError::Manifest(
                    "kind=policy forbids permissions.allow-private".into(),
                ));
            }
            if !self.permissions.x_secret.is_empty() {
                return Err(PluginError::Manifest(
                    "kind=policy forbids permissions.x-secret (policy components hold no credentials)".into(),
                ));
            }
        }
        // 接管声明校验（2026-10-23 工具接管层）：仅 tool kind；目标名形态 +
        // deny-list（宿主特权语义——沙盒内 guest 无法诚实兑现，接管一律拒）。
        if !self.takeover.is_empty() {
            if *self.kind != PluginKind::Tool {
                return Err(PluginError::Manifest(
                    "takeover requires kind=tool (only tool components can take over built-in interfaces)".into(),
                ));
            }
            for t in &self.takeover {
                if t.is_empty() || t.len() > 64 {
                    return Err(PluginError::Manifest(format!(
                        "invalid takeover target (1-64 chars): {t:?}"
                    )));
                }
                if t.starts_with(crate::PLUGIN_TOOL_PREFIX) {
                    return Err(PluginError::Manifest(format!(
                        "invalid takeover target '{t}': plugin namespace is isolated by design (builtin names only)"
                    )));
                }
                if takeover_denied(t) {
                    return Err(PluginError::Manifest(format!(
                        "takeover of '{t}' is denied: host-privileged semantics cannot be honestly honored in-sandbox"
                    )));
                }
            }
        }
        Ok(())
    }

    /// config-schema 声明键清单（实例配置读取的白名单）。
    #[must_use]
    pub fn config_schema_keys(&self) -> Vec<String> {
        self.config_schema.keys().cloned().collect()
    }
}
