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

/// 清单体积分上限（边界补强 2026-10-20）：`scan_connectors` 是热路径
/// （list / find / invoke / undo 每次调用都全量重扫），读文件前先查体积
/// ——超限条目按损坏诚实入列，不读不解析。真实清单是 KB 级（几十个 op
/// 到顶几 KB），256KB = 5~10× 余量；注册表目录在 workspace 内，agent 可
/// 写（write_file 过安全闸即可落），无上限 = 一次大文件写入让每次扫描
/// 都背上 MB 级读+TOML 解析。
pub const MANIFEST_MAX_BYTES: u64 = 256 * 1024;

/// 清单体积检查：超限返回诚实错误文案（读前止损）。
fn manifest_size_error(bytes: u64) -> String {
    format!("清单 {bytes} 字节超过体积上限 {MANIFEST_MAX_BYTES}（不读不解析——热路径止损）")
}

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
                    "connector `{id}` 已停用（管理面停用，或清单缺省 enabled = false）"
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
        // 体积上限（读前止损，见 MANIFEST_MAX_BYTES 注记）。
        if let Ok(m) = std::fs::metadata(&manifest_path)
            && m.len() > MANIFEST_MAX_BYTES
        {
            out.push(LoadedConnector {
                dir: path,
                connector: None,
                error: Some(manifest_size_error(m.len())),
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
                    Ok(trust) => {
                        // A3-a：启停真相单点合并（sidecar 覆盖 > 清单缺省）
                        // ——list / find / invoke 闸 / 独占隐藏表全部经此处消费。
                        let mut c = c;
                        let side = crate::state::load_tool_state(workspace, &c.id);
                        c.enabled = crate::state::effective_enabled(&side, c.enabled);
                        out.push(LoadedConnector {
                            dir: path,
                            connector: Some(c),
                            error: None,
                            trust,
                        })
                    }
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
///
/// 双轨解析（边界补强 2026-10-20）：先直达 `connectors/<id>/`（目录名
/// 命中，快路径）；未命中再扫描注册表按**清单 id** 匹配（存量手放目录
/// dir≠id 的兜底——与 CLI enable/disable 的 id-or-dir 双臂同语义；dir
/// 名升序确定性 first-match，损坏条目跳过）。新走 create 路径的条目
/// dir==id 恒成立，直达即中。
///
/// 成功时同时返回信任徽标（M1 消费面收口 2026-10-19：执行面
/// require_signed 闸与审批卡警示都从这里取 trust，不再丢给调用方）。
pub fn find_connector(
    workspace: &Path,
    id: &str,
) -> Result<(ValidatedConnector, ConnectorTrust), ConnectorLookupError> {
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
        // 直达未命中 → 清单 id 兜底扫描（见函数注记）。
        for loaded in scan_connectors(workspace) {
            let Some(c) = loaded.connector else {
                continue;
            };
            if c.id != id {
                continue;
            }
            if !c.enabled {
                return Err(ConnectorLookupError::Disabled { id: id.to_string() });
            }
            return Ok((c, loaded.trust));
        }
        return Err(ConnectorLookupError::NotFound { id: id.to_string() });
    }
    // 体积上限（读前止损，与 scan_connectors 同闸）。
    if let Ok(m) = std::fs::metadata(&manifest_path)
        && m.len() > MANIFEST_MAX_BYTES
    {
        return Err(ConnectorLookupError::Invalid {
            id: id.to_string(),
            error: manifest_size_error(m.len()),
        });
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
    let trust =
        assess_dir_trust(&dir, workspace).map_err(|error| ConnectorLookupError::Invalid {
            id: id.to_string(),
            error,
        })?;
    // A3-a：启停真相单点合并（sidecar 覆盖 > 清单缺省），与 scan_connectors 同源。
    let side = crate::state::load_tool_state(workspace, &c.id);
    let c = ValidatedConnector {
        enabled: crate::state::effective_enabled(&side, c.enabled),
        ..c
    };
    if !c.enabled {
        return Err(ConnectorLookupError::Disabled { id: id.to_string() });
    }
    Ok((c, trust))
}

/// 在 connector 内按 id 找 op。
pub fn find_op<'a>(c: &'a ValidatedConnector, op_id: &str) -> Option<&'a ValidatedOp> {
    c.ops.iter().find(|o| o.id == op_id)
}

// ---------------------------------------------------------------------------
// 管理面（Dashboard connectors.create / set_enabled / remove，2026-10-19）
// ---------------------------------------------------------------------------

/// 新建 connector（先校验后落盘）。
///
/// - 目录名与 connector id 同字符集（`[a-z0-9_-]{1,64}`）——路径安全单点，
///   `..` / 绝对路径 / 路径分隔符天然被拒；
/// - **目录名必须等于清单 id**（边界补强 2026-10-20）：目录即注册表，
///   `find_connector` 直达 `connectors/<id>/` 与 sidecar 按 id 键控都以
///   dir==id 为前提——放行 dir≠id 会造出「可列出、不可调用」的死链；
/// - 清单经 [`parse_connector_manifest`] 全量校验，非法不写半截；
/// - 拒重复：目录已存在、manifest id 与在册条目撞车（id 全局唯一，
///   三方指南 §3.1）都拒绝；
/// - 默认启用与否由清单自身的 `enabled` 键决定（缺省 true）；示例模板
///   （examples/connectors/）刻意默认停用是安全姿态，不在此处覆盖。
pub fn create_connector(
    workspace: &Path,
    dir_name: &str,
    toml_text: &str,
) -> Result<ValidatedConnector, String> {
    crate::manifest::validate_id(dir_name, "connector 目录名")?;
    let parsed = parse_connector_manifest(toml_text)?;
    if parsed.id != dir_name {
        return Err(format!(
            "connector 目录名 `{dir_name}` 与清单 id `{}` 不一致——目录即注册表，两者必须相同",
            parsed.id
        ));
    }
    let dir = workspace.join(CONNECTORS_DIR).join(dir_name);
    if dir.exists() {
        return Err(format!("connector 目录 `{dir_name}` 已存在"));
    }
    if scan_connectors(workspace)
        .iter()
        .filter_map(|l| l.connector.as_ref())
        .any(|c| c.id == parsed.id)
    {
        return Err(format!(
            "connector id `{}` 已被其他目录占用（id 全局唯一）",
            parsed.id
        ));
    }
    nemesis_utils::ensure_dir(&dir.to_string_lossy()).map_err(|e| format!("创建目录失败: {e}"))?;
    nemesis_utils::write_file_atomic(
        &dir.join(MANIFEST_FILE).to_string_lossy(),
        toml_text.as_bytes(),
        0o644,
    )
    .map_err(|e| format!("写入 {MANIFEST_FILE} 失败: {e}"))?;
    Ok(parsed)
}

/// 翻 per-connector 启停位（A3-a，治理补全 P2）：**恒写 sidecar**
/// （`config/connector_state/<id>.json` 的 `enabled` 键），清单字节不动
/// ——带分离签名的条目从此可正常启停（旧实现翻 `[connector] enabled` 位
/// 会破坏验签，只能诚实拒绝）。
///
/// 语义：sidecar `enabled` = 用户意志，覆盖清单 `[connector] enabled`
/// （作者安装缺省）；合并单点在 [`scan_connectors`] / [`find_connector`]
/// 出口，消费面自动同源。成员资格证明 = 目录名命中**当下扫描结果**
/// （不在册即拒——无遍历攻击面）。清单损坏的条目无法启停（sidecar 按
/// manifest id 键控，损坏条目解析不出 id——先修复或删除）。
pub fn set_connector_enabled(
    workspace: &Path,
    dir_name: &str,
    enabled: bool,
) -> Result<(), String> {
    let loaded = scan_connectors(workspace);
    let entry = loaded
        .iter()
        .find(|l| l.dir.file_name().map(|n| n == dir_name).unwrap_or(false))
        .ok_or_else(|| format!("connector `{dir_name}` 不在注册表"))?;
    let Some(c) = entry.connector.as_ref() else {
        return Err(format!(
            "connector `{dir_name}` 清单损坏，无法启停（先修复或删除该条目）"
        ));
    };
    crate::state::set_enabled(workspace, &c.id, enabled)
}

/// 删除 connector 目录（成员资格证明同 [`set_connector_enabled`]：目录名
/// 必须命中当下扫描结果）。**凭条/账本不动**——行动账本是追加式流水，
/// 删 connector 不抹历史账。
pub fn remove_connector(workspace: &Path, dir_name: &str) -> Result<(), String> {
    let loaded = scan_connectors(workspace);
    let entry = loaded
        .iter()
        .find(|l| l.dir.file_name().map(|n| n == dir_name).unwrap_or(false))
        .ok_or_else(|| format!("connector `{dir_name}` 不在注册表"))?;
    nemesis_utils::fsutil::remove_dir_all_retry(&entry.dir)
        .map_err(|e| format!("删除目录 {} 失败: {e}", entry.dir.display()))
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
    /// 签名徽标（M1 消费面收口：connector_find 随命中透出，模型可感知，
    /// 自主优先高信任源——弱信号，不门控）。
    pub trust: ConnectorTrust,
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
                trust: loaded.trust,
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
                trust: loaded.trust,
                connector: c,
            });
        } else {
            hits.push(ConnectorSearchHit {
                connector: c,
                trust: loaded.trust,
                matched_ops,
            });
        }
    }
    hits
}

// ---------------------------------------------------------------------------
// B2（2026-10-19 独占接管）：独占隐藏表 + 镜像 op 枚举（共享纯函数）
// ---------------------------------------------------------------------------

/// B2：独占 server 名 → 工具名前缀（`mcp_<sanitize(server)>_`）。sanitize
/// 单一真相源 = `nemesis_mcp::adapter::sanitize_name`（与 MCP 挂载面、
/// nemesis-web 在位探测同源）。pub = Dashboard 列表的镜像行枚举同源复用。
pub fn exclusive_prefix(server: &str) -> String {
    format!("mcp_{}_{}", nemesis_mcp::adapter::sanitize_name(server), "")
}

/// B2：独占隐藏名清单——**供给器闭包与 Dashboard 列表共用的单一真相**。
///
/// 语义：每个「启用且声明了 `exclusive_mcp`」的 connector，其独占 server
/// 前缀（`mcp_<sanitize(server)>_`）命中的当下注册工具，除 sidecar
/// `exempt` 豁免外全部隐藏（`default` 与 `blocked` 都隐——封禁 = 原生侧
/// 照隐 + 镜像不提供，能力对 agent 消失）。
///
/// `mcp_tool_names`：当下注册的全部 MCP 工具名（调用方从 loop 的工具快照
/// 现取——server 升级换工具集，下一次调用即按当下真相重算，监管无洞）。
/// 输出按输入序去重；空输入/无独占声明 = 空表。
pub fn exclusive_hidden_tools(workspace: &Path, mcp_tool_names: &[String]) -> Vec<String> {
    let mut hidden: Vec<String> = Vec::new();
    for loaded in scan_connectors(workspace) {
        let Some(c) = loaded.connector else { continue };
        if !c.enabled || c.exclusive_mcp.is_empty() {
            continue;
        }
        let prefixes: Vec<String> = c
            .exclusive_mcp
            .iter()
            .map(|s| exclusive_prefix(s))
            .collect();
        let state = crate::state::load_tool_state(workspace, &c.id);
        for tool in mcp_tool_names {
            if !prefixes.iter().any(|p| tool.starts_with(p.as_str())) {
                continue;
            }
            if crate::state::tool_policy(&state, tool) == crate::state::ToolPolicy::Exempt {
                continue;
            }
            if !hidden.contains(tool) {
                hidden.push(tool.clone());
            }
        }
    }
    hidden
}

/// B2：单独占 connector 的镜像 op 清单——**connector_find / Dashboard 列表
/// 共用**。独占 server 前缀命中的当下注册工具 − sidecar `blocked`（封禁
/// = 镜像不提供）− sidecar `exempt`（豁免 = 回原生直调，不经镜像）−
/// 显式 `[[op]]` 已覆盖的工具（显式档位优先于镜像，避免双路由）。
///
/// 返回 `(tool_name, policy)` 序（按工具名字典序——MCP 工具面顺序是注册
/// 序，展示稳定优先）。policy 恒为 `Default`（exempt/blocked 已滤除；
/// 参数化返回是为消费方未来扩展例外态展示留形）。
pub fn exclusive_mirror_ops(
    workspace: &Path,
    c: &ValidatedConnector,
    mcp_tool_names: &[String],
) -> Vec<(String, crate::state::ToolPolicy)> {
    let mut out: Vec<(String, crate::state::ToolPolicy)> = Vec::new();
    if c.exclusive_mcp.is_empty() {
        return out;
    }
    let prefixes: Vec<String> = c
        .exclusive_mcp
        .iter()
        .map(|s| exclusive_prefix(s))
        .collect();
    let state = crate::state::load_tool_state(workspace, &c.id);
    let explicit_tools: Vec<&str> = c
        .ops
        .iter()
        .filter_map(|op| match &op.binding {
            crate::manifest::OpBinding::Tool { tool } => Some(tool.as_str()),
            _ => None,
        })
        .collect();
    for tool in mcp_tool_names {
        if !prefixes.iter().any(|p| tool.starts_with(p.as_str())) {
            continue;
        }
        if explicit_tools.contains(&tool.as_str()) {
            continue;
        }
        let policy = crate::state::tool_policy(&state, tool);
        if policy != crate::state::ToolPolicy::Default {
            continue;
        }
        out.push((tool.clone(), policy));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// B3（治理补全 2026-10-19）：镜像基线四档——按 server 自述 annotations
/// hints 定接管基线。hint 是**声明不是承诺**（server 说了不算数，但它比
/// 「清单层不可知」的 v1 一刀切精确）；显式 `[[op]]` 恒优先于镜像（作者
/// 对高危工具自定义审批/预算面的入口不变）。
///
/// 返回 `(risk, class, confirm_required)`：
///
/// | hints | 基线 |
/// |---|---|
/// | `readOnlyHint=true`（且无 destructive） | low / read（免卡，凭条恒记） |
/// | 未声明（None/缺省） | medium / read（v1 保守基线不变） |
/// | `readOnlyHint=false` 或 `destructiveHint=true` | high / write + 强制确认卡 |
///
/// 矛盾自述（readOnly=true 且 destructive=true）按 destructive 裁（就高
/// 不就低——基线是接管下限，不是能力上限）。
pub fn mirror_baseline(
    read_only_hint: Option<bool>,
    destructive_hint: Option<bool>,
) -> (crate::manifest::RiskLevel, crate::manifest::OpClass, bool) {
    use crate::manifest::{OpClass, RiskLevel};
    if destructive_hint == Some(true) || read_only_hint == Some(false) {
        (RiskLevel::High, OpClass::Write, true)
    } else if read_only_hint == Some(true) {
        (RiskLevel::Low, OpClass::Read, false)
    } else {
        (RiskLevel::Medium, OpClass::Read, false)
    }
}

#[cfg(test)]
mod tests;
