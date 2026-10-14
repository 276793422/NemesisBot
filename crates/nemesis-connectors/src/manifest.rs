//! connector.toml 清单 schema 与校验（v1）。
//!
//! 结构形态（单文件）：
//!
//! ```toml
//! [connector]
//! id = "paper-broker"
//! name = "纸面券商"
//! version = "0.1.0"
//! kind = "rest"                       # rest | local | script
//! enabled = true
//! description = "..."
//! [connector.http]                    # rest 型必填
//! base = "https://example.com"
//! [connector.credentials]             # 凭据槽位 → vault 别名 connector/<id>/<key>
//! api_key = { required = false, into = "header:X-API-Key" }
//!
//! [[op]]
//! id = "get_quote"
//! intent = ["查询行情"]
//! risk = "low"
//! class = "read"
//! http = { method = "GET", path = "/quote?symbol={symbol}" }
//!
//! [[op]]
//! id = "place_order"
//! risk = "critical"
//! class = "write"
//! confirm = "required"                # 强授权卡
//! budget = "finance"                  # 资金预算维
//! receipt = "order_id"                # 回执主键字段
//! tool = "exec"                       # local/script 型用 tool / action 绑定
//! ```
//!
//! 全结构 `deny_unknown_fields`（拼错字段 loud 报错，不静默丢）。原始反序
//! 列化形态经 [`parse_connector_manifest`] 校验为 [`ValidatedConnector`]：
//! id/op 字符集、op 唯一、恰一绑定（kind 仅描述性，混合绑定合法）、
//! confirm/budget 值域、http path 占位符语法。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// 原始反序列化形态
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectorManifestRaw {
    #[serde(rename = "connector")]
    pub connector: ConnectorMetaRaw,
    #[serde(rename = "op", default)]
    pub ops: Vec<OpDefRaw>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectorMetaRaw {
    pub id: String,
    pub name: String,
    pub version: String,
    pub kind: ConnectorKind,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub http: Option<HttpBaseRaw>,
    #[serde(default)]
    pub credentials: Option<BTreeMap<String, CredentialSlotRaw>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectorKind {
    /// HTTP API 声明型（op 用 http 绑定）。
    Rest,
    /// 本地能力映射（op 用 tool/action 绑定）。
    Local,
    /// 脚本型（op 映射 exec 类工具）。
    Script,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpBaseRaw {
    pub base: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialSlotRaw {
    #[serde(default)]
    pub required: bool,
    /// 注入形态：`"header:<头名>"` 或 `"query:<参数名>"`；缺省 `header:<槽位名>`。
    #[serde(default)]
    pub into: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpDefRaw {
    pub id: String,
    /// LLM 层意图匹配素材（中文短语列表）。
    #[serde(default)]
    pub intent: Vec<String>,
    pub risk: RiskLevel,
    pub class: OpClass,
    /// `"required"` = 强授权卡。
    #[serde(default)]
    pub confirm: Option<String>,
    /// `"finance"` = 资金预算维。
    #[serde(default)]
    pub budget: Option<String>,
    /// 回执主键字段名（如 `order_id`）。
    #[serde(default)]
    pub receipt: Option<String>,
    /// rest 型绑定：`{ method, path }`。
    #[serde(default)]
    pub http: Option<HttpOpRaw>,
    /// local/script 型绑定：映射到现有工具名。
    #[serde(default)]
    pub tool: Option<String>,
    /// 内置动作绑定（demo 域专用，如 `builtin.demo`）。
    #[serde(default)]
    pub action: Option<String>,
    #[serde(default)]
    pub args_schema: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpOpRaw {
    pub method: HttpMethod,
    pub path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

impl RiskLevel {
    /// 排序权重（Low=0 … Critical=3）。
    pub fn rank(self) -> u8 {
        match self {
            RiskLevel::Low => 0,
            RiskLevel::Medium => 1,
            RiskLevel::High => 2,
            RiskLevel::Critical => 3,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            RiskLevel::Low => "low",
            RiskLevel::Medium => "medium",
            RiskLevel::High => "high",
            RiskLevel::Critical => "critical",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OpClass {
    Read,
    Write,
    Exec,
    Network,
}

impl OpClass {
    pub fn as_str(self) -> &'static str {
        match self {
            OpClass::Read => "read",
            OpClass::Write => "write",
            OpClass::Exec => "exec",
            OpClass::Network => "network",
        }
    }
}

fn default_true() -> bool {
    true
}

// ---------------------------------------------------------------------------
// 校验后形态
// ---------------------------------------------------------------------------

/// 校验通过的 connector（解析产物，消费方只接触这个形态）。
#[derive(Debug, Clone)]
pub struct ValidatedConnector {
    pub id: String,
    pub name: String,
    pub version: String,
    pub kind: ConnectorKind,
    pub enabled: bool,
    pub description: Option<String>,
    /// `[connector.http].base`（rest 型必有）。
    pub base_url: Option<String>,
    pub credentials: BTreeMap<String, CredentialBinding>,
    /// ops（保清单书写序）。
    pub ops: Vec<ValidatedOp>,
}

#[derive(Debug, Clone)]
pub struct CredentialBinding {
    pub slot: String,
    pub required: bool,
    pub into: CredentialInto,
}

#[derive(Debug, Clone)]
pub enum CredentialInto {
    Header(String),
    Query(String),
}

#[derive(Debug, Clone)]
pub struct ValidatedOp {
    pub id: String,
    pub intent: Vec<String>,
    pub risk: RiskLevel,
    pub class: OpClass,
    pub confirm_required: bool,
    pub finance_budget: bool,
    pub receipt_key: Option<String>,
    pub binding: OpBinding,
    pub args_schema: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub enum OpBinding {
    Http {
        method: HttpMethod,
        path: String,
        /// path 中的占位符名（按出现序，`{symbol}` → `symbol`）。
        placeholders: Vec<String>,
    },
    Tool {
        tool: String,
    },
    Action {
        action: String,
    },
}

// ---------------------------------------------------------------------------
// 解析与校验
// ---------------------------------------------------------------------------

/// 解析 + 校验 connector.toml 文本（单一入口）。
pub fn parse_connector_manifest(text: &str) -> Result<ValidatedConnector, String> {
    let raw: ConnectorManifestRaw =
        toml::from_str(text).map_err(|e| format!("TOML 解析失败: {e}"))?;
    validate(raw)
}

fn validate(raw: ConnectorManifestRaw) -> Result<ValidatedConnector, String> {
    let meta = raw.connector;

    validate_id(&meta.id, "connector id")?;

    // http base
    let base_url = meta.http.as_ref().map(|h| h.base.trim().to_string());
    if base_url.as_deref().is_some_and(str::is_empty) {
        return Err(format!(
            "connector `{}`: [connector.http].base 不能为空",
            meta.id
        ));
    }

    // credentials `into` 解析
    let mut credentials = BTreeMap::new();
    if let Some(slots) = meta.credentials {
        for (slot, raw_slot) in slots {
            if slot.is_empty() {
                return Err(format!("connector `{}`: 凭据槽位名不能为空", meta.id));
            }
            let into = match raw_slot.into.as_deref() {
                None => CredentialInto::Header(slot.clone()),
                Some(s) => parse_credential_into(s)
                    .ok_or_else(|| {
                        format!(
                            "connector `{}`: 凭据槽位 `{slot}` 的 into `{s}` 非法（应为 \"header:<名>\" 或 \"query:<名>\"）",
                            meta.id
                        )
                    })?,
            };
            credentials.insert(
                slot.clone(),
                CredentialBinding {
                    slot,
                    required: raw_slot.required,
                    into,
                },
            );
        }
    }

    if raw.ops.is_empty() {
        return Err(format!("connector `{}`: 至少要声明一个 [[op]]", meta.id));
    }

    let mut ops = Vec::with_capacity(raw.ops.len());
    let mut seen = std::collections::BTreeSet::new();
    for op in raw.ops {
        validate_id(&op.id, "op id")?;
        if !seen.insert(op.id.clone()) {
            return Err(format!("connector `{}`: op id `{}` 重复", meta.id, op.id));
        }

        // kind↔绑定一致性 + 恰一绑定
        let http = op.http.is_some();
        let tool = op.tool.is_some();
        let action = op.action.is_some();
        let binding_count = usize::from(http) + usize::from(tool) + usize::from(action);
        if binding_count == 0 {
            return Err(format!(
                "connector `{}` op `{}`: 缺执行绑定（http / tool / action 三选一）",
                meta.id, op.id
            ));
        }
        if binding_count > 1 {
            return Err(format!(
                "connector `{}` op `{}`: 执行绑定只能声明一个（http / tool / action）",
                meta.id, op.id
            ));
        }
        let binding = if let Some(h) = op.http {
            // kind 只是描述性分组（list 视图用）；执行形态按 op 绑定逐个决定
            // ——混合型 connector（如 paper-broker：行情走 http、下单走本地
            // 账本）是合法形态。唯一硬约束：http 绑定必须有 base。
            if base_url.is_none() {
                return Err(format!(
                    "connector `{}` op `{}`: rest 型缺 [connector.http].base",
                    meta.id, op.id
                ));
            }
            let placeholders = parse_path_placeholders(&h.path).map_err(|e| {
                format!(
                    "connector `{}` op `{}`: http path 非法: {e}",
                    meta.id, op.id
                )
            })?;
            OpBinding::Http {
                method: h.method,
                path: h.path,
                placeholders,
            }
        } else if let Some(t) = op.tool {
            if t.trim().is_empty() {
                return Err(format!(
                    "connector `{}` op `{}`: tool 绑定不能为空",
                    meta.id, op.id
                ));
            }
            OpBinding::Tool {
                tool: t.trim().to_string(),
            }
        } else {
            let a = op.action.expect("恰一绑定已保证");
            if a.trim().is_empty() {
                return Err(format!(
                    "connector `{}` op `{}`: action 绑定不能为空",
                    meta.id, op.id
                ));
            }
            OpBinding::Action {
                action: a.trim().to_string(),
            }
        };

        // confirm / budget 值域
        let confirm_required = match op.confirm.as_deref() {
            None => false,
            Some("required") => true,
            Some(other) => {
                return Err(format!(
                    "connector `{}` op `{}`: confirm 值 `{other}` 非法（只支持 \"required\"）",
                    meta.id, op.id
                ));
            }
        };
        let finance_budget = match op.budget.as_deref() {
            None => false,
            Some("finance") => true,
            Some(other) => {
                return Err(format!(
                    "connector `{}` op `{}`: budget 值 `{other}` 非法（只支持 \"finance\"）",
                    meta.id, op.id
                ));
            }
        };

        ops.push(ValidatedOp {
            id: op.id,
            intent: op.intent,
            risk: op.risk,
            class: op.class,
            confirm_required,
            finance_budget,
            receipt_key: op.receipt,
            binding,
            args_schema: op.args_schema,
        });
    }

    Ok(ValidatedConnector {
        id: meta.id,
        name: meta.name,
        version: meta.version,
        kind: meta.kind,
        enabled: meta.enabled,
        description: meta.description,
        base_url,
        credentials,
        ops,
    })
}

/// id / op id 字符集：`[a-z0-9_-]`，1..=64。
fn validate_id(id: &str, what: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > 64 {
        return Err(format!("{what} `{id}` 长度非法（1..=64）"));
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    {
        return Err(format!(
            "{what} `{id}` 含非法字符（只允许小写字母/数字/-/_）"
        ));
    }
    Ok(())
}

/// 解析 `header:<名>` / `query:<名>`。
fn parse_credential_into(s: &str) -> Option<CredentialInto> {
    let (scheme, name) = s.split_once(':')?;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    match scheme {
        "header" => Some(CredentialInto::Header(name.to_string())),
        "query" => Some(CredentialInto::Query(name.to_string())),
        _ => None,
    }
}

/// 校验 http path 占位符语法，返回占位符名列表。
/// 合法形态：`{name}`（name 为 `[a-z0-9_]`，非空）；孤立 `}` 非法；孤立 `{`
/// 或空名/非法字符 loud 报错。
fn parse_path_placeholders(path: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let bytes = path.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'}' => return Err("孤立的 `}`".into()),
            b'{' => {
                let start = i + 1;
                let mut j = start;
                while j < bytes.len() && bytes[j] != b'}' {
                    j += 1;
                }
                if j >= bytes.len() {
                    return Err("占位符缺少闭合 `}`".into());
                }
                let name = &path[start..j];
                if name.is_empty()
                    || !name
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
                {
                    return Err(format!("占位符名 `{name}` 非法（只允许小写字母/数字/_）"));
                }
                out.push(name.to_string());
                i = j + 1;
            }
            _ => i += 1,
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
