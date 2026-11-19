//! Charter handler — 用途登记（W1 #1，2026-11-19 安全图谱批）。
//!
//! `agent_charter.json`（`<workspace>/config/`）= 用途/场景/边界声明的
//! 单一真相源：`purpose`（一句话用途）、`scenarios`（场景列表）、
//! `read_only`（只读用途声明）、`boundaries`（边界红线列表）。
//!
//! - **WSAPI**：`charter.get`（缺席 = `registered:false` + 全默认字段，
//!   诚实空态不编造）/ `charter.set`（字段校验 + 原子写 + updated_at）。
//! - **联动建议**：`chat.get_mode` 投影 `charter_read_only` +
//!   `charter_hint`（声明只读用途而当前是 build 模式 → 建议切 plan；
//!   建议不是强制——只读声明是登记意图，闸门仍是 plan 模式本体）。
//! - **onboard**：种子落盘（见 `nemesisbot/src/commands/onboard.rs`）。
//!
//! 独立工作区文件（与 approval_rules.json / projects.json 同族），
//! 不进 config.json——无 typed Config 字段，文件自身走 serde 结构校验。

use crate::ws_router::{ModuleHandler, RequestContext};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// charter 文件结构（serde 结构校验 = set 写面前的单一真相源）。
/// Default 派生：全空/全 false/0 = 未登记缺省态。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AgentCharter {
    /// 一句话用途声明（空 = 未登记）。
    pub purpose: String,
    /// 预期使用场景列表。
    pub scenarios: Vec<String>,
    /// 只读用途声明（true = 声明本实例只做读取/分析类工作；
    /// get_mode 联动建议 plan 模式）。
    pub read_only: bool,
    /// 边界红线声明（明确不许做的事，逐条文本）。
    pub boundaries: Vec<String>,
    /// 最后更新时间（unix 秒；0 = 从未显式登记过）。
    pub updated_at: i64,
}

/// charter 文件路径（`<workspace>/config/agent_charter.json`）。
pub(crate) fn charter_path(workspace: &str) -> PathBuf {
    PathBuf::from(workspace)
        .join("config")
        .join("agent_charter.json")
}

/// 读 charter（注册表读路径）。文件缺席 = 全默认 + `registered:false`
/// （诚实空态）；文件存在但损坏 = 诚实报错（不静默回落默认——登记
/// 面沉默丢声明比报错更危险）。
pub(crate) fn read_charter(workspace: &str) -> Result<(AgentCharter, bool), String> {
    let path = charter_path(workspace);
    if !path.exists() {
        return Ok((AgentCharter::default(), false));
    }
    let raw = nemesis_utils::fsutil::read_config_string(&path)
        .map_err(|e| format!("charter 读取失败（{}）：{}", path.display(), e))?;
    let charter: AgentCharter = serde_json::from_str(&raw)
        .map_err(|e| format!("charter 解析失败（{}）：{}", path.display(), e))?;
    Ok((charter, true))
}

/// 写 charter（原子写 + updated_at 盖章）。返回落盘路径。
pub(crate) fn write_charter(workspace: &str, charter: &AgentCharter) -> Result<String, String> {
    let mut c = charter.clone();
    c.updated_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let path = charter_path(workspace);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("charter 目录创建失败（{}）：{}", parent.display(), e))?;
    }
    let data =
        serde_json::to_string_pretty(&c).map_err(|e| format!("charter 序列化失败：{}", e))?;
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), data.as_bytes(), 0o644)
        .map_err(|e| format!("charter 写入失败（{}）：{}", path.display(), e))?;
    Ok(path.to_string_lossy().to_string())
}

/// onboard 种子内容（`updated_at:0` = 未显式登记过；purpose 留空引导
/// 用户填写，不预填编造）。
pub fn seed_json() -> &'static str {
    r#"{
  "purpose": "",
  "scenarios": [],
  "read_only": false,
  "boundaries": [],
  "updated_at": 0
}
"#
}

pub struct CharterHandler;

#[async_trait::async_trait]
impl ModuleHandler for CharterHandler {
    fn module_name(&self) -> &str {
        "charter"
    }

    fn commands(&self) -> &'static [&'static str] {
        &["get", "set"]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let workspace = ctx
            .workspace
            .clone()
            .ok_or("workspace 未知，无法定位 charter 文件")?;
        match cmd {
            "get" => {
                let (charter, registered) = read_charter(&workspace)?;
                let mut v = serde_json::to_value(&charter)
                    .map_err(|e| format!("charter 序列化失败：{}", e))?;
                v["registered"] = serde_json::Value::Bool(registered);
                Ok(Some(v))
            }
            "set" => {
                let data = data.ok_or("missing data (purpose/scenarios/read_only/boundaries)")?;
                // 全量替换语义（PATCH 式逐字段可选更新）：给出字段覆盖，
                // 缺省字段保留现值——避免「只想改 read_only 把 purpose 冲空」。
                let (mut charter, _) = read_charter(&workspace)?;
                if let Some(p) = data.get("purpose") {
                    charter.purpose = p.as_str().ok_or("purpose 必须是字符串")?.trim().to_string();
                }
                if let Some(s) = data.get("scenarios") {
                    charter.scenarios = string_vec(s, "scenarios")?;
                }
                if let Some(r) = data.get("read_only") {
                    charter.read_only = r.as_bool().ok_or("read_only 必须是布尔")?;
                }
                if let Some(b) = data.get("boundaries") {
                    charter.boundaries = string_vec(b, "boundaries")?;
                }
                let path = write_charter(&workspace, &charter)?;
                let mut v = serde_json::to_value(&charter)
                    .map_err(|e| format!("charter 序列化失败：{}", e))?;
                v["registered"] = serde_json::Value::Bool(true);
                v["path"] = serde_json::Value::String(path);
                Ok(Some(v))
            }
            _ => Err(format!("unknown charter command: {cmd}")),
        }
    }
}

/// 字符串数组字段解析（逐项 trim + 丢空串——登记面不接受空白条目）。
fn string_vec(v: &serde_json::Value, field: &str) -> Result<Vec<String>, String> {
    let arr = v
        .as_array()
        .ok_or_else(|| format!("{field} 必须是字符串数组"))?;
    Ok(arr
        .iter()
        .map(|x| x.as_str().unwrap_or("").trim().to_string())
        .filter(|s| !s.is_empty())
        .collect())
}

/// get_mode 联动建议投影（chat handler 消费）：只读声明 × build 模式
/// → 建议切 plan。workspace 未知/文件缺席 → None（不投影不撒谎）。
pub(crate) fn plan_mode_hint(workspace: Option<&str>, mode: &str) -> Option<String> {
    let ws = workspace?;
    let (charter, registered) = read_charter(ws).ok()?;
    if registered && charter.read_only && mode == "build" {
        Some(format!(
            "agent_charter 声明只读用途（purpose: {}），当前是 build 模式——建议切换 plan 模式（/plan 或 chat.set_mode）。",
            if charter.purpose.is_empty() {
                "未填写"
            } else {
                &charter.purpose
            }
        ))
    } else {
        None
    }
}

#[cfg(test)]
mod tests;
