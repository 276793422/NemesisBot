// 机械拆分：本文件内容由 lib.rs 原样搬入（纯代码搬家，行为零变更）。

use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillsConfig {
    #[serde(default)]
    pub enabled: bool,
    /// Whether `skill_manage` writes require interactive approval (default false).
    #[serde(default)]
    pub manage_approval: bool,
    /// 验签：无签名技能是否放行（默认 true 兼容存量；false = strict，拒绝无签名）。
    #[serde(default = "default_skills_allow_unsigned")]
    pub allow_unsigned: bool,
    /// 装前审批：安装走审批卡（默认 true；CLI 可 --yes 跳过，WSAPI 走审批基建）。
    #[serde(default = "default_skills_install_approval")]
    pub install_approval: bool,
    /// 版本龄下限（天；默认 0 = 关）。
    #[serde(default)]
    pub min_age_days: i64,
    /// 版本龄策略："warn"（默认）| "block"。
    #[serde(default = "default_skills_min_age_policy")]
    pub min_age_policy: String,
}

// Default 手工实现与 serde 缺省函数同源（derive 会把 allow_unsigned 落成
// false，与「默认 true 兼容存量」语义相反）。
impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            manage_approval: false,
            allow_unsigned: default_skills_allow_unsigned(),
            install_approval: default_skills_install_approval(),
            min_age_days: 0,
            min_age_policy: default_skills_min_age_policy(),
        }
    }
}

fn default_skills_allow_unsigned() -> bool {
    true
}

fn default_skills_install_approval() -> bool {
    true
}

fn default_skills_min_age_policy() -> String {
    "warn".to_string()
}

/// MCP 主配置（config.mcp.json 顶层）。**单一真相源**（收敛）：
/// 此前 nemesis-config 与 nemesis-mcp 各持一份服务器配置结构（`env:
/// Vec<String>` vs `Option<Vec<String>>`、`timeout: i64` vs
/// `timeout_secs: u64`），同一份 config.mcp.json 出现两种解析结果——用户
/// 实盘文件里的 `"env": null` 让 Dashboard 侧直接解析崩溃（"invalid type:
/// null, expected a sequence"），MCP runtime 却一切正常。现在本结构是唯一
/// 定义，nemesis-mcp 直接复用（`pub type ServerConfig =
/// McpServerConfig`）；反序列化全量宽容（见 [`mcp_config_from_value`]）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct McpConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub servers: Vec<McpServerConfig>,
    #[serde(default = "default_mcp_timeout")]
    pub timeout: i64,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            servers: Vec::new(),
            timeout: default_mcp_timeout(),
        }
    }
}

impl<'de> serde::Deserialize<'de> for McpConfig {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let v = serde_json::Value::deserialize(deserializer)?;
        mcp_config_from_value(v).map_err(serde::de::Error::custom)
    }
}

/// 宽容解析 config.mcp.json（根因修复："invalid type: null,
/// expected a sequence" 一类解析崩溃的统一出口）。接受三种顶层形状：
/// 1. 本项目标准形状 `{enabled, servers: [...], timeout}`；
/// 2. Claude Desktop 生态形状 `{"mcpServers": {name: {...}}}`（map，name
///    缺省时从 key 取；数组亦可）——LLM 抄外部配置进盘也能吃；
/// 3. 仅顶层字段（无 servers/mcpServers）——视为空配置，不再报错。
///    env/args/headers/tags 逐字段宽容（`null`/map/数组皆收，见
///    [`flexible_string_list`]）；全局与 per-server timeout 皆宽容
///    （`null`/字符串数字 → 默认值）。
fn mcp_config_from_value(v: serde_json::Value) -> std::result::Result<McpConfig, String> {
    let obj = match v {
        serde_json::Value::Object(o) => o,
        _ => return Err("MCP config must be a JSON object".to_string()),
    };

    if obj.contains_key("servers") {
        #[derive(serde::Deserialize)]
        struct StandardShape {
            #[serde(default)]
            enabled: bool,
            #[serde(default)]
            servers: Vec<McpServerConfig>,
            #[serde(default = "default_mcp_timeout", deserialize_with = "flexible_i64")]
            timeout: i64,
        }
        let shape: StandardShape =
            serde_json::from_value(serde_json::Value::Object(obj)).map_err(|e| e.to_string())?;
        return Ok(McpConfig {
            enabled: shape.enabled,
            servers: shape.servers,
            timeout: shape.timeout,
        });
    }

    if let Some(ms) = obj.get("mcpServers") {
        let mut servers = Vec::new();
        match ms {
            serde_json::Value::Object(map) => {
                for (key, sv) in map {
                    let mut entry = sv.clone();
                    if entry.get("name").is_none() {
                        entry["name"] = serde_json::Value::String(key.clone());
                    }
                    servers.push(
                        serde_json::from_value::<McpServerConfig>(entry)
                            .map_err(|e| format!("mcpServers[{key}]: {e}"))?,
                    );
                }
            }
            serde_json::Value::Array(arr) => {
                for (i, sv) in arr.iter().enumerate() {
                    servers.push(
                        serde_json::from_value::<McpServerConfig>(sv.clone())
                            .map_err(|e| format!("mcpServers[{i}]: {e}"))?,
                    );
                }
            }
            _ => return Err("`mcpServers` must be an object or array".to_string()),
        }
        // 外部生态配置文件的语义是「配置存在即启用」；顶层 enabled 显式声明则优先。
        let enabled = obj.get("enabled").and_then(|b| b.as_bool()).unwrap_or(true);
        return Ok(McpConfig {
            enabled,
            servers,
            timeout: default_mcp_timeout(),
        });
    }

    // 无 servers/mcpServers：仅开关等顶层字段（容忍空/残缺配置）。
    let enabled = obj
        .get("enabled")
        .and_then(|b| b.as_bool())
        .unwrap_or(false);
    Ok(McpConfig {
        enabled,
        servers: Vec::new(),
        timeout: default_mcp_timeout(),
    })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpServerConfig {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub transport_type: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, deserialize_with = "flexible_string_list")]
    pub headers: Vec<String>,
    #[serde(default, deserialize_with = "flexible_string_list")]
    pub args: Vec<String>,
    /// `"KEY=VALUE"` 列表。宽容反序列化：`null` → 空表；`{"K":"V"}` →
    /// `["K=V"]`——用户实盘 config.mcp.json 里 `"env": null` 触发
    /// Dashboard "invalid type: null" 崩溃的根因出口。
    #[serde(default, deserialize_with = "flexible_string_list")]
    pub env: Vec<String>,
    /// 请求超时（秒）。线上键名保持 `timeout`（Dashboard/CLI/存盘早已用
    /// 该键），`timeout_secs` 作为别名同时接受；`null`/字符串数字宽容。
    #[serde(
        rename = "timeout",
        alias = "timeout_secs",
        default = "default_server_timeout",
        deserialize_with = "flexible_timeout_secs"
    )]
    pub timeout_secs: u64,
    #[serde(default)]
    pub provider_name: String,
    #[serde(default)]
    pub provider_url: String,
    #[serde(default, deserialize_with = "flexible_string_list")]
    pub tags: Vec<String>,
    #[serde(default)]
    pub command: String,
    /// 浏览器 profile 注入 opt-in 闸。
    /// true = 本 server 允许宿主按上下文注入 per-agent Chromium
    /// `--user-data-dir`（gateway agent loop 路径 = agent home
    /// browser-profile/；执行体子进程路径 = `NEMESISBOT_BROWSER_PROFILE`
    /// env 回落）。默认 false = 普通 MCP 零变化（共享 server 不被塞身份）；
    /// server args 已显式带 `--user-data-dir`（空格或 `=` 形态）= 用户配置
    /// 优先，不重复注入。false 序列化跳过（存量 config.mcp.json 字节稳定）。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub browser_profile: bool,
    /// 未类型化键兜底（二次回归 同族根修，对齐 `ModelConfig.extra`
    /// 惯例）：typed `save_config` round-trip 时编译器保证保留未知键，不再静默
    /// 抹掉（如未来新增的 per-server 键）。空 map 序列化跳过 → 无 unknown 键的
    /// 条目输出形态与既有字节一致。注意：PartialEq 参与相等比较（round-trip
    /// 两侧携带同一 extra 时语义不变）。
    /// BTreeMap 键序确定（同 `ModelConfig.extra`，字节稳定）。
    #[serde(
        flatten,
        default,
        skip_serializing_if = "std::collections::BTreeMap::is_empty"
    )]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
}

impl Default for McpServerConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            transport_type: String::new(),
            url: String::new(),
            description: String::new(),
            headers: Vec::new(),
            args: Vec::new(),
            env: Vec::new(),
            timeout_secs: default_server_timeout(),
            provider_name: String::new(),
            provider_url: String::new(),
            tags: Vec::new(),
            command: String::new(),
            browser_profile: false,
            extra: std::collections::BTreeMap::new(),
        }
    }
}

impl McpServerConfig {
    /// Normalize legacy fields: map old `command` to `url` and set default `transport_type`.
    pub fn normalize(&mut self) {
        if self.url.is_empty() && !self.command.is_empty() {
            self.url = self.command.clone();
        }
        if self.transport_type.is_empty() {
            self.transport_type = "stdio".to_string();
        }
    }

    /// Builder: stdio 便捷构造（nemesis-mcp 侧测试与 ad-hoc 连接用）。
    pub fn new(name: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            command: command.into(),
            ..Default::default()
        }
    }

    /// Builder: 追加一个命令行参数。
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Builder: 追加一个环境变量（`"KEY=VALUE"`）。
    pub fn env(mut self, kv: impl Into<String>) -> Self {
        self.env.push(kv.into());
        self
    }

    /// Builder: 设置请求超时（秒）。
    pub fn timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    /// Builder: 打开浏览器 profile 注入 opt-in。
    pub fn browser_profile(mut self, yes: bool) -> Self {
        self.browser_profile = yes;
        self
    }
}

/// MCP HTTP 头行语法校验（统一定义， 复查）：CLI
/// `mcp add`/`mcp discover`、Dashboard `server.add`/`server.update`、运行期
/// `resolved_header_pairs` 三方同源。只验语法，绝不解引用、绝不回显值段
/// （值段可能夹带真凭据；错误文案只接头名/引用名——两者非秘密）。
///
/// 拒绝集：
/// - 缺冒号（应为 `"KEY: VALUE"`）；
/// - 头名为空 / 含空白或控制字符；
/// - `Mcp-Session-Id` 协议保留头（server 指派、transport 自管，配置行
///   只有伪造会话身份一种效果）；
/// - 整值 `env:`/`yaml:`/`vault:` 引用名为空或带首尾空格（运行期按原样
///   解析不 trim，必失败 → WARN 跳过头 → 401——写入面挡住省一轮学费；
///   语义与 [`crate::resolve_secret_field`] 的整值前缀识别一致，内嵌引用
///   如 `Bearer vault:x` 按字面量发送、放行）。
pub fn validate_mcp_header_line(line: &str) -> std::result::Result<(), String> {
    let Some((raw_name, raw_value)) = line.split_once(':') else {
        return Err("header 行缺冒号（应为 \"KEY: VALUE\"）".to_string());
    };
    let name = raw_name.trim();
    if name.is_empty() || name.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(format!("header 名非法（空 / 含空白或控制字符）：{name}"));
    }
    if name.eq_ignore_ascii_case("mcp-session-id") {
        return Err("Mcp-Session-Id 是协议保留头（server 指派），不能配置".to_string());
    }
    let value = raw_value.trim();
    for (prefix, what) in [
        ("env:", "环境变量名"),
        ("yaml:", "凭据别名"),
        ("vault:", "vault 别名"),
    ] {
        if let Some(rest) = value.strip_prefix(prefix) {
            if rest.is_empty() {
                return Err(format!(
                    "header 值 {what} 引用名为空——写作 {prefix}<name>（如 vault:pet-token）"
                ));
            }
            if rest.trim() != rest {
                return Err(format!(
                    "header 值 {what} 引用名带首尾空格（运行期按原样解析，必失败）——\
                     写作 {prefix}{rest}（冒号后不带空格）"
                ));
            }
        }
    }
    Ok(())
}

/// MCP stdio env 行语法校验（"KEY=VALUE"； 单一权威定义——
/// CLI `mcp add` 写入面 / Dashboard server.add·update / 运行期
/// `resolved_env` WARN 跳过同源）：缺 `=` / 键名空 / 键名含空白 / 整值
/// `vault:`/`env:`/`yaml:` 引用名为空或带首尾空格。与
/// [`validate_mcp_header_line`] 同款 trim-后检查（值部首尾空格在校验与
/// 运行期解析两侧都剥掉——字面量值带首尾空格的场景让位于两侧一致性）。
/// 只验语法，值原样落盘，绝不解引用。
pub fn validate_mcp_env_line(line: &str) -> std::result::Result<(), String> {
    let Some((raw_name, raw_value)) = line.split_once('=') else {
        return Err("env 行缺 '='（应为 \"KEY=VALUE\"）".to_string());
    };
    let name = raw_name.trim();
    if name.is_empty() {
        return Err("env 键名为空".to_string());
    }
    if name.chars().any(|c| c.is_whitespace()) {
        return Err(format!("env 键名含空白：{name}"));
    }
    let value = raw_value.trim();
    for (prefix, what) in [
        ("env:", "环境变量名"),
        ("yaml:", "凭据别名"),
        ("vault:", "vault 别名"),
    ] {
        if let Some(rest) = value.strip_prefix(prefix) {
            if rest.is_empty() {
                return Err(format!(
                    "env 值 {what} 引用名为空——写作 {prefix}<name>（如 vault:pet-token）"
                ));
            }
            if rest.trim() != rest {
                return Err(format!(
                    "env 值 {what} 引用名带首尾空格（运行期按原样解析，必失败）——\
                     写作 {prefix}{rest}（'=' 后不带空格）"
                ));
            }
        }
    }
    Ok(())
}

/// 宽容字符串列表反序列化（统一定义收敛配套）：`null` → 空表；
/// `{"K":"V"}` → `["K=V"]`（LLM/外部生态最常见的 env 写法）；`["a","b"]`
/// → 原样；标量 → 单元素表。元素非字符串时尽力转字符串。
fn flexible_string_list<'de, D>(d: D) -> std::result::Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = serde_json::Value::deserialize(d)?;
    Ok(string_list_from_value(v))
}

fn string_list_from_value(v: serde_json::Value) -> Vec<String> {
    match v {
        serde_json::Value::Null => Vec::new(),
        serde_json::Value::Array(items) => items
            .into_iter()
            .filter_map(|item| match item {
                serde_json::Value::String(s) => Some(s),
                serde_json::Value::Null => None,
                other => Some(other.to_string()),
            })
            .collect(),
        serde_json::Value::Object(map) => map
            .into_iter()
            .map(|(k, v)| match v {
                serde_json::Value::String(s) => format!("{k}={s}"),
                serde_json::Value::Null => k,
                other => format!("{k}={other}"),
            })
            .collect(),
        // 标量字符串取原值（Value::to_string 是 JSON 表示，会带引号）
        serde_json::Value::String(s) => vec![s],
        other => vec![other.to_string()],
    }
}

/// 宽容 u64（per-server timeout）：`null`/字符串数字/解析失败 → fallback。
fn flexible_timeout_secs<'de, D>(d: D) -> std::result::Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = serde_json::Value::deserialize(d)?;
    Ok(u64_from_value(v, default_server_timeout()))
}

fn u64_from_value(v: serde_json::Value, fallback: u64) -> u64 {
    match v {
        serde_json::Value::Null => fallback,
        serde_json::Value::Number(n) => n.as_u64().unwrap_or(fallback),
        serde_json::Value::String(s) => s.trim().parse().unwrap_or(fallback),
        _ => fallback,
    }
}

/// 宽容 i64（全局 timeout）：`null`/字符串数字/解析失败 → 默认值。
fn flexible_i64<'de, D>(d: D) -> std::result::Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = serde_json::Value::deserialize(d)?;
    Ok(match v {
        serde_json::Value::Null => default_mcp_timeout(),
        serde_json::Value::Number(n) => n.as_i64().unwrap_or_else(default_mcp_timeout),
        serde_json::Value::String(s) => s.trim().parse().unwrap_or(default_mcp_timeout()),
        _ => default_mcp_timeout(),
    })
}

fn default_server_timeout() -> u64 {
    30
}

// ============================================================================
// Full sub-configurations (loaded from separate files)
// ============================================================================

/// Full skills configuration loaded from config.skills.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillsFullConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub search_cache: SkillsSearchCacheConfig,
    #[serde(default = "default_max_concurrent_searches")]
    pub max_concurrent_searches: i64,
    #[serde(default = "default_search_limit")]
    pub search_limit: i64,
    #[serde(default)]
    pub github_sources: Vec<GitHubSourceConfig>,
    #[serde(default)]
    pub clawhub: SkillsClawHubConfig,
    #[serde(default)]
    pub modelscope: SkillsModelScopeConfig,
    /// Passthrough for untyped user-defined keys（CFG-08：没有这个 flatten
    /// 时，任何 typed save 都会静默丢掉用户手加的自定义键——与 ModelConfig
    /// `extra` 同款先例）。BTreeMap 保 round-trip 字节稳定；空表序列化时
    /// 跳过（输出与既有形态字节一致）。
    #[serde(
        flatten,
        default,
        skip_serializing_if = "std::collections::BTreeMap::is_empty"
    )]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
}

impl Default for SkillsFullConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            search_cache: SkillsSearchCacheConfig::default(),
            max_concurrent_searches: 2,
            search_limit: 50,
            github_sources: vec![],
            clawhub: SkillsClawHubConfig::default(),
            modelscope: SkillsModelScopeConfig::default(),
            extra: std::collections::BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillsSearchCacheConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_cache_max_size")]
    pub max_size: i64,
    #[serde(default = "default_cache_ttl_seconds")]
    pub ttl_seconds: i64,
}

impl Default for SkillsSearchCacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_size: 50,
            ttl_seconds: 300,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GitHubSourceConfig {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub repo: String,
    /// BUG-20261114-034：缺省收敛 true——与 nemesis-skills::types::GitHubSourceConfig
    /// （运行时面 `default_true`）及出厂模板显式值一致。旧 derive 零值使手写极简
    /// 源条目（缺 enabled 键）Dashboard 显示停用而运行时注册；且 typed save 会把
    /// 解析出的 false 物化回盘，下次 gateway 重启翻转运行时行为（源被关）。
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub branch: String,
    #[serde(default)]
    pub index_type: String,
    #[serde(default)]
    pub index_path: String,
    #[serde(default)]
    pub skill_path_pattern: String,
    #[serde(default)]
    pub timeout: i64,
    #[serde(default)]
    pub max_size: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillsClawHubConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub convex_url: String,
    #[serde(default)]
    pub timeout: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillsModelScopeConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub timeout: i64,
}

impl Default for SkillsModelScopeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            timeout: 30,
        }
    }
}
