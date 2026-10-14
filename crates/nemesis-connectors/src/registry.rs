//! workspace connector 注册表：扫描 `<workspace>/connectors/<id>/connector.toml`。
//!
//! 目录形态镜像 skills：每 connector 一个目录，损坏的 connector **诚实入列**
//! （`LoadedConnector::error`），不静默跳过——用户能从管理面看到坏的是什么、
//! 坏在哪。缺 `connectors/` 目录 = 空注册表（常态，非错误）。

use std::path::{Path, PathBuf};

use crate::manifest::{ValidatedConnector, ValidatedOp, parse_connector_manifest};

pub const CONNECTORS_DIR: &str = "connectors";
pub const MANIFEST_FILE: &str = "connector.toml";

/// 扫描结果条目：connector 或诚实错误。
#[derive(Debug, Clone)]
pub struct LoadedConnector {
    pub dir: PathBuf,
    pub connector: Option<ValidatedConnector>,
    pub error: Option<String>,
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
            });
            continue;
        }
        match std::fs::read_to_string(&manifest_path) {
            Ok(text) => match parse_connector_manifest(&text) {
                Ok(c) => out.push(LoadedConnector {
                    dir: path,
                    connector: Some(c),
                    error: None,
                }),
                Err(err) => out.push(LoadedConnector {
                    dir: path,
                    connector: None,
                    error: Some(err),
                }),
            },
            Err(err) => out.push(LoadedConnector {
                dir: path,
                connector: None,
                error: Some(format!("读取失败: {err}")),
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
    if !c.enabled {
        return Err(ConnectorLookupError::Disabled { id: id.to_string() });
    }
    Ok(c)
}

/// 在 connector 内按 id 找 op。
pub fn find_op<'a>(c: &'a ValidatedConnector, op_id: &str) -> Option<&'a ValidatedOp> {
    c.ops.iter().find(|o| o.id == op_id)
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
