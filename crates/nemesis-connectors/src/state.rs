//! B2（2026-10-19 独占接管）：connector 运行态 sidecar——per-tool 例外表。
//!
//! 存放 `<workspace>/config/connector_state/<connector_id>.json`：
//! `{"blocked": [...], "exempt": [...], "enabled": true|null}`（工具名 =
//! 完整 mcp 工具名；`enabled` 是 A3-a（治理补全 P2）扩进来的启停覆盖位：
//! Some = 用户意志（Dashboard/CLI 翻位恒写这里），None = 清单
//! `[connector] enabled` 键（作者安装缺省）生效——签名清单字节从此恒稳定）。
//!
//! **为什么外置而不进 connector.toml**：带分离签名的清单，其字节被签名
//! 覆盖——例外调整若写清单 = 每次微调都要重签，治理动作的摩擦大到没人
//! 用，等于没有例外。sidecar 与清单分离（先例：wasm 插件实例配置），
//! 签名清单恒不可变，例外状态是活的运行态。A3-a（治理补全 P2）落地后，
//! `set_enabled` 对签名条目的「翻位破坏验签」矛盾同样经 sidecar 松绑——
//! 启停不再触碰 connector.toml。
//!
//! 两档语义（方向相反，UI 必须分开呈现）：
//! - `exempt` **豁免** = 该工具恢复原生直调（供给闸放行）——用户显式放弃
//!   对这一个接口的监管，其余照管；
//! - `blocked` **封禁** = 连接器路也不提供镜像 op——该能力对 agent 完全
//!   消失（原生侧独占闸照隐）。
//! - `default`（两表皆无）= 镜像接管基线（receipt 恒写；档位按 server 自述
//!   annotations hints 四档定，B3——未声明 = medium/read 保守基线）。
//!
//! 删 connector 不清 sidecar（重装同名 id 自动恢复策略；孤儿文件无害，
//! 不做隐式清理——「删数据」是显式动作）。每调用现读磁盘：无缓存、无
//! 失效通知问题，与 `scan_connectors` 同哲学。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 单 connector 的运行态 sidecar（per-tool 例外表 + 启停覆盖位）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolPolicyState {
    #[serde(default)]
    pub blocked: Vec<String>,
    #[serde(default)]
    pub exempt: Vec<String>,
    /// 启停覆盖（A3-a）：Some = 用户意志（覆盖清单 `[connector] enabled`），
    /// None = 清单缺省生效。存量 sidecar 无此键 = None，零迁移。
    #[serde(default)]
    pub enabled: Option<bool>,
}

/// per-tool 策略三态（判定产物，消费方拿到的语义形态）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolPolicy {
    /// 镜像接管基线（无例外记录）。
    Default,
    /// 豁免：恢复原生直调（独占闸放行、镜像不提供）。
    Exempt,
    /// 封禁：原生照隐、镜像不提供——能力对 agent 消失。
    Blocked,
}

/// sidecar 目录：`<workspace>/config/connector_state/`。
pub fn state_dir(workspace: &Path) -> PathBuf {
    workspace.join("config").join("connector_state")
}

/// 单 connector 的 sidecar 路径。id 字符集由 [`super::manifest::validate_id`]
/// 钉死（`[a-z0-9_-]`）——路径安全（无分隔符/`..`/大小写歧义）。
pub fn state_path(workspace: &Path, connector_id: &str) -> Result<PathBuf, String> {
    super::manifest::validate_id(connector_id, "connector id")
        .map_err(|e| format!("sidecar 拒绝非法 id: {e}"))?;
    Ok(state_dir(workspace).join(format!("{connector_id}.json")))
}

/// 读 sidecar（缺文件/坏 JSON/坏条目 = 空表——**读路径永不报错**：例外
/// 状态缺失按 default 基线走，治理不因状态文件损坏而放宽或崩溃；坏文件
/// 在 set 路径写入时被整体覆盖）。
pub fn load_tool_state(workspace: &Path, connector_id: &str) -> ToolPolicyState {
    let Ok(path) = state_path(workspace, connector_id) else {
        return ToolPolicyState::default();
    };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// 单工具的当下策略（exempt 优先于 blocked——同名双录是配置矛盾，豁免
/// 是更强的显式意志：用户亲手把这一个工具放回原生直调，封禁残留视为
/// 历史遗迹）。
pub fn tool_policy(state: &ToolPolicyState, tool: &str) -> ToolPolicy {
    if state.exempt.iter().any(|t| t == tool) {
        ToolPolicy::Exempt
    } else if state.blocked.iter().any(|t| t == tool) {
        ToolPolicy::Blocked
    } else {
        ToolPolicy::Default
    }
}

/// 写单工具策略（`default` = 从两表移除；两表皆空时删除整个 sidecar 文件
/// ——不留空壳）。原子写（tmp+rename，与注册表/预算同纪律）。坏 JSON 的
/// 旧文件被整体覆盖（读路径已按空表降级，这里覆盖 = 修复）。
pub fn set_tool_policy(
    workspace: &Path,
    connector_id: &str,
    tool: &str,
    policy: ToolPolicy,
) -> Result<(), String> {
    let tool = tool.trim();
    if tool.is_empty() {
        return Err("工具名不能为空".to_string());
    }
    let path = state_path(workspace, connector_id)?;
    let mut state = load_tool_state(workspace, connector_id);
    state.blocked.retain(|t| t != tool);
    state.exempt.retain(|t| t != tool);
    match policy {
        ToolPolicy::Default => {}
        ToolPolicy::Exempt => state.exempt.push(tool.to_string()),
        ToolPolicy::Blocked => state.blocked.push(tool.to_string()),
    }
    if state.blocked.is_empty() && state.exempt.is_empty() && state.enabled.is_none() {
        // 全空（三字段）= 回到基线：删文件（存在才删；不存在幂等成功）。
        // enabled 有值时文件必须保留——启停位是活状态，不能被策略清理顺手抹掉。
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("sidecar 清理失败: {e}")),
        }
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("sidecar 目录创建失败: {e}"))?;
    }
    let text =
        serde_json::to_string_pretty(&state).map_err(|e| format!("sidecar 序列化失败: {e}"))?;
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), text.as_bytes(), 0o600)
        .map_err(|e| format!("sidecar 写入失败: {e}"))
}

/// effective enabled 单点合并（A3-a）：sidecar 有值 = 用户意志胜；缺省
/// （None）= 清单 `[connector] enabled` 键（作者安装缺省）。合并落点在
/// [`crate::registry::scan_connectors`] / [`crate::registry::find_connector`]
/// 两处出口，全部消费面（list / find / invoke 闸 / 独占隐藏表）自动同源。
pub fn effective_enabled(state: &ToolPolicyState, manifest_enabled: bool) -> bool {
    state.enabled.unwrap_or(manifest_enabled)
}

/// 翻启停位（A3-a）：恒写 sidecar，清单字节不动——签名条目可正常启停。
/// 没有「清除回清单缺省」的入口（两态开关；真要恢复作者缺省 = 删 sidecar
/// 文件或改清单，都是显式动作）。
pub fn set_enabled(workspace: &Path, connector_id: &str, enabled: bool) -> Result<(), String> {
    let path = state_path(workspace, connector_id)?;
    let mut state = load_tool_state(workspace, connector_id);
    state.enabled = Some(enabled);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("sidecar 目录创建失败: {e}"))?;
    }
    let text =
        serde_json::to_string_pretty(&state).map_err(|e| format!("sidecar 序列化失败: {e}"))?;
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), text.as_bytes(), 0o600)
        .map_err(|e| format!("sidecar 写入失败: {e}"))
}

#[cfg(test)]
mod tests;
