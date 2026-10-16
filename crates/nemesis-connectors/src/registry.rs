//! workspace connector 注册表：扫描 `<workspace>/connectors/<id>/connector.toml`。
//!
//! 目录形态镜像 skills：每 connector 一个目录，损坏的 connector **诚实入列**
//! （`LoadedConnector::error`），不静默跳过——用户能从管理面看到坏的是什么、
//! 坏在哪。缺 `connectors/` 目录 = 空注册表（常态，非错误）。

use std::path::{Path, PathBuf};

use crate::manifest::{ValidatedConnector, ValidatedOp, parse_connector_manifest};
use crate::signing::ConnectorTrust;

pub const CONNECTORS_DIR: &str = "connectors";
pub const MANIFEST_FILE: &str = "connector.toml";

/// 扫描结果条目：connector 或诚实错误。
#[derive(Debug, Clone)]
pub struct LoadedConnector {
    pub dir: PathBuf,
    pub connector: Option<ValidatedConnector>,
    pub error: Option<String>,
    /// 签名徽标（D6.1）：verified / community / unsigned。验签失败的条目
    /// 走 `error` 拒收路径，不落条目。
    pub trust: ConnectorTrust,
}

/// 按 connector id 查找失败原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectorLookupError {
    NotFound { id: String },
    Invalid { id: String, error: String },
    Disabled { id: String },
}

impl std::fmt::Display for ConnectorLookupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectorLookupError::NotFound { id } => {
                write!(
                    f,
                    "connector `{id}` 不存在（检查 <workspace>/connectors/{id}/connector.toml）"
                )
            }
            ConnectorLookupError::Invalid { id, error } => {
                write!(f, "connector `{id}` 清单非法: {error}")
            }
            ConnectorLookupError::Disabled { id } => {
                write!(
                    f,
                    "connector `{id}` 已停用（connector.toml enabled = false）"
                )
            }
        }
    }
}

/// 目录签名评估（scan/find 共用；feature 门在 signing 侧）。
fn assess_dir_trust(dir: &Path, workspace: &Path) -> Result<ConnectorTrust, String> {
    #[cfg(feature = "security")]
    {
        let store = crate::signing::workspace_trust_store(workspace);
        crate::signing::assess_connector_signature(dir, &store).map(|a| a.trust)
    }
    #[cfg(not(feature = "security"))]
    {
        let _ = workspace;
        crate::signing::assess_connector_signature(dir).map(|a| a.trust)
    }
}

/// 扫描 workspace 注册表（目录名升序，确定性）。
pub fn scan_connectors(workspace: &Path) -> Vec<LoadedConnector> {
    let dir = workspace.join(CONNECTORS_DIR);
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return out; // 缺目录 = 空注册表
    };
    // CLAUDE.md 纪律：read_dir 顺序是平台实现细节，必须显式排序。
    let mut entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let path = e.path();
        if !path.is_dir() {
            continue; // 散文件不是 connector
        }
        let manifest_path = path.join(MANIFEST_FILE);
        if !manifest_path.exists() {
            out.push(LoadedConnector {
                dir: path,
                connector: None,
                error: Some(format!("缺少 {MANIFEST_FILE}")),
                trust: ConnectorTrust::Unsigned,
            });
            continue;
        }
        match std::fs::read_to_string(&manifest_path) {
            Ok(text) => match parse_connector_manifest(&text) {
                Ok(c) => match assess_dir_trust(&path, workspace) {
                    // D6.1：带签名且验签失败（篡改/调换/吊销）→ 拒收，
                    // corrupt 形态诚实入列（不静默跳过也不过带）。
                    Err(err) => out.push(LoadedConnector {
                        dir: path,
                        connector: None,
                        error: Some(err),
                        trust: ConnectorTrust::Unsigned,
                    }),
                    Ok(trust) => out.push(LoadedConnector {
                        dir: path,
                        connector: Some(c),
                        error: None,
                        trust,
                    }),
                },
                Err(err) => out.push(LoadedConnector {
                    dir: path,
                    connector: None,
                    error: Some(err),
                    trust: ConnectorTrust::Unsigned,
                }),
            },
            Err(err) => out.push(LoadedConnector {
                dir: path,
                connector: None,
                error: Some(format!("读取失败: {err}")),
                trust: ConnectorTrust::Unsigned,
            }),
        }
    }
    out
}

/// 按 id 查找单个 connector（区分 不存在 / 损坏 / 停用 三态）。
pub fn find_connector(
    workspace: &Path,
    id: &str,
) -> Result<ValidatedConnector, ConnectorLookupError> {
    let dir = workspace.join(CONNECTORS_DIR).join(id);
    let manifest_path = dir.join(MANIFEST_FILE);
    if !manifest_path.exists() {
        // 目录存在但无 manifest 也是 NotFound 之外的损坏态
        if dir.is_dir() {
            return Err(ConnectorLookupError::Invalid {
                id: id.to_string(),
                error: format!("缺少 {MANIFEST_FILE}"),
            });
        }
        return Err(ConnectorLookupError::NotFound { id: id.to_string() });
    }
    let text =
        std::fs::read_to_string(&manifest_path).map_err(|e| ConnectorLookupError::Invalid {
            id: id.to_string(),
            error: format!("读取失败: {e}"),
        })?;
    let c = parse_connector_manifest(&text).map_err(|error| ConnectorLookupError::Invalid {
        id: id.to_string(),
        error,
    })?;
    // D6.1：带签名且验签失败（篡改/调换/吊销）→ 执行面拒用——即使清单
    // parse 仍然合法（改动后的清单可能语义完好，但签名不再对得上）。
    assess_dir_trust(&dir, workspace).map_err(|error| ConnectorLookupError::Invalid {
        id: id.to_string(),
        error,
    })?;
    if !c.enabled {
        return Err(ConnectorLookupError::Disabled { id: id.to_string() });
    }
    Ok(c)
}

/// 在 connector 内按 id 找 op。
pub fn find_op<'a>(c: &'a ValidatedConnector, op_id: &str) -> Option<&'a ValidatedOp> {
    c.ops.iter().find(|o| o.id == op_id)
}

/// 执行前 args 校验（R4）：按 op 声明的 `args_schema` 检查
/// required / type / enum / minimum / maximum。schema 缺省（None）= 不设限；
/// 多余字段忽略（JSON Schema 缺省语义，对齐 agent 侧 args_validator 惯例）。
/// 返回 Err = 人读拒绝原因（回执/回灌用），**必须在出网（本地 dispatch /
/// 远程 RPC）之前拒绝**。
pub fn validate_op_args(
    schema: Option<&serde_json::Value>,
    args: &serde_json::Value,
) -> Result<(), String> {
    let Some(schema) = schema else {
        return Ok(());
    };
    let Some(obj) = schema.as_object() else {
        return Ok(()); // 加载期 shape 扫描已保证顶层是 object；防御性放行
    };
    // required：缺失即拒
    if let Some(required) = obj.get("required").and_then(|v| v.as_array()) {
        for field in required {
            if let Some(name) = field.as_str()
                && args.get(name).map(|v| v.is_null()).unwrap_or(true)
            {
                return Err(format!("缺少必填参数 `{name}`"));
            }
        }
    }
    // properties：逐字段 type/enum/minimum/maximum
    let Some(properties) = obj.get("properties").and_then(|v| v.as_object()) else {
        return Ok(());
    };
    for (name, spec) in properties {
        let Some(value) = args.get(name) else {
            continue;
        };
        if value.is_null() {
            continue; // null 已由 required 闸裁决；非必填 null 视为缺席
        }
        if let Some(expected) = spec.get("type").and_then(|v| v.as_str())
            && !value_matches_type(value, expected)
        {
            return Err(format!(
                "参数 `{name}` 类型错误：应为 {expected}，实际为 {}",
                type_name_of(value)
            ));
        }
        // enum：类型无关的成员判定（数值统一按 f64 比较——`5` 与 `5.0`
        // 同值；其余类型 serde 深等值）。
        if let Some(allowed) = spec.get("enum").and_then(|v| v.as_array())
            && !allowed.iter().any(|a| enum_member_eq(a, value))
        {
            return Err(format!(
                "参数 `{name}` 取值 `{value}` 不在允许范围 [{}] 内",
                allowed
                    .iter()
                    .map(|a| a.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        // 数值边界（number / integer 共用；JSON 数值统一按 f64 比较）
        if matches!(
            spec.get("type").and_then(|v| v.as_str()),
            Some("number") | Some("integer")
        ) {
            let Some(num) = value_as_number(value) else {
                continue; // 类型闸已拒
            };
            if let Some(min) = spec.get("minimum").and_then(|v| v.as_f64())
                && num < min
            {
                return Err(format!("参数 `{name}` = {num} 低于下限 {min}"));
            }
            if let Some(max) = spec.get("maximum").and_then(|v| v.as_f64())
                && num > max
            {
                return Err(format!("参数 `{name}` = {num} 超过上限 {max}"));
            }
        }
    }
    Ok(())
}

/// JSON Schema type 判定。integer 接受整值浮点（`2.0` 合法 integer）；
/// serde `is_number` 不含 bool，boolean 单独判。
fn value_matches_type(value: &serde_json::Value, expected: &str) -> bool {
    match expected {
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => {
            value.is_i64()
                || value.is_u64()
                || value.as_f64().map(|f| f.fract() == 0.0).unwrap_or(false)
        }
        "boolean" => value.is_boolean(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        _ => true, // 未知 type 声明不设限（加载期 shape 扫描已拦非法声明）
    }
}

fn value_as_number(value: &serde_json::Value) -> Option<f64> {
    value.as_f64()
}

/// enum 成员判定：数值双方向 f64 归一（`5` ≡ `5.0`），其余 serde 深等值。
fn enum_member_eq(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    if let (Some(x), Some(y)) = (a.as_f64(), b.as_f64()) {
        return x == y;
    }
    a == b
}

fn type_name_of(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

/// 检索命中条目。
#[derive(Debug, Clone)]
pub struct ConnectorSearchHit {
    pub connector: ValidatedConnector,
    /// op 级命中的 op id；connector 级命中（名称/描述）= 全部 op id。
    pub matched_ops: Vec<String>,
}

/// 意图检索：query 按空白切词元，任一词元小写包含匹配 connector 的
/// id/name/description 或某 op 的 id/intent 即命中。空 query = 列出全部
/// （列表语义，供 LLM 浏览注册表）。
pub fn search_connectors(workspace: &Path, query: &str) -> Vec<ConnectorSearchHit> {
    let tokens: Vec<String> = query.split_whitespace().map(|t| t.to_lowercase()).collect();
    let mut hits = Vec::new();
    for loaded in scan_connectors(workspace) {
        let Some(c) = loaded.connector else { continue };
        if !c.enabled {
            continue; // 检索面只暴露可用 connector
        }
        let op_hit = |op: &ValidatedOp| -> bool {
            tokens.iter().any(|t| {
                op.id.to_lowercase().contains(t)
                    || op.intent.iter().any(|i| i.to_lowercase().contains(t))
            })
        };
        let matched_ops: Vec<String> = c
            .ops
            .iter()
            .filter(|o| op_hit(o))
            .map(|o| o.id.clone())
            .collect();
        if tokens.is_empty() {
            // 列表语义（空 query）：展示全部 op 供 LLM 浏览
            hits.push(ConnectorSearchHit {
                matched_ops: c.ops.iter().map(|o| o.id.clone()).collect(),
                connector: c,
            });
        } else if matched_ops.is_empty() {
            // op 级未命中 → 尝试 connector 级（id/name/description）
            let conn_hit = tokens.iter().any(|t| {
                c.id.to_lowercase().contains(t)
                    || c.name.to_lowercase().contains(t)
                    || c.description
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(t)
            });
            if !conn_hit {
                continue;
            }
            hits.push(ConnectorSearchHit {
                matched_ops: c.ops.iter().map(|o| o.id.clone()).collect(),
                connector: c,
            });
        } else {
            hits.push(ConnectorSearchHit {
                connector: c,
                matched_ops,
            });
        }
    }
    hits
}

#[cfg(test)]
mod tests;
