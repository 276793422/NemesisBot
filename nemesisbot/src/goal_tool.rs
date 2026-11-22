//! 目标系统 agent 工具面（2026-11-17 计划）：`goal` 单工具三 action——
//! `add`（自建目标绑定当前会话）/ `delete`（仅删自建）/ `disable`
//! （停用当前会话目标；人建目标受 `goals.allow_agent_disable` 全局开关）。
//!
//! 防自锁（计划钦定）：agent 工具面**不含** enable / done——enable 归人
//! （paused/blocked 恢复），done 唯一出口是评估闸判定（agent 自称完成不
//! 生效）。agent 只能新建、删自有、停用；「目标何时算完成」永远由评估闸
//! 说了算。
//!
//! 绑定模型：goal 注册表条目带 `session_key`，工具执行时按
//! `context.session_key` 反查——agent 无 id 参数，只动**本会话**的目标
//!（不能跨会话操作，add 撞既有未完成目标诚实拒）。
//!
//! 人建 goal.md 文件保护由 WorkspaceBoundary.goals_guard 承担
//!（创建者二分：人建对 agent 只读）；本工具绕过文件工具直写 goals/
//! 目录，是注册目标内容的合法通道（等价于 Dashboard create——人的
//!意志经 LLM 转达，文件保护只防文件工具的越权写，不防本工具）。
//!
//! 注册面：仅主 agent（`agent_factory` 8g；项目 loop / cluster worker
//! 不装——目标绑定主会话，worker 会话无 goals 语义）。
//!
//! tier 策略：不进 `tier_allowed_tools`（mini/normal 白名单天然不含，仅
//! big/Unresolved 全量档可见）——目标自建是重决策面，小模型不供给。

use nemesis_agent::context::RequestContext;
use nemesis_goals::{GoalCreatedBy, GoalStatus, load_registry, update_registry, write_goal_md};
use std::path::PathBuf;

pub const GOAL_TOOL_NAME: &str = "goal";

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `goals.allow_agent_disable` 读取（config.json 单一真相源；缺省/读取
/// 失败 = true 允许——fail-open，与 GoalsConfig serde default 同值）。
pub fn allow_agent_disable(config_path: Option<&std::path::Path>) -> bool {
    let Some(path) = config_path else {
        return true;
    };
    match nemesis_config::load_config(path) {
        Ok(cfg) => cfg.goals.map(|g| g.allow_agent_disable).unwrap_or(true),
        Err(_) => true,
    }
}

/// 当前会话绑定的 goal（同键多条时取 updated_at 最新——理论不可达，
/// 防御性收敛；done 条目跳过——done 是历史，不再占用会话）。
pub fn find_bound_goal(
    workspace: &std::path::Path,
    session_key: &str,
) -> Result<Option<nemesis_goals::GoalEntry>, String> {
    let entries = load_registry(workspace)?;
    let mut best: Option<nemesis_goals::GoalEntry> = None;
    for e in entries {
        if e.session_key == session_key && e.status != GoalStatus::Done {
            let newer = best
                .as_ref()
                .map(|b| e.updated_at_ms > b.updated_at_ms)
                .unwrap_or(true);
            if newer {
                best = Some(e);
            }
        }
    }
    Ok(best)
}

/// goal 工具（单工具 action 制）。
pub struct GoalTool {
    workspace: PathBuf,
    config_path: Option<PathBuf>,
}

impl GoalTool {
    pub fn new(workspace: PathBuf, config_path: Option<PathBuf>) -> Self {
        Self {
            workspace,
            config_path,
        }
    }
}

#[async_trait::async_trait]
impl nemesis_agent::r#loop::Tool for GoalTool {
    fn description(&self) -> String {
        "Manage a self-supervision goal bound to this session. Actions: \
         `add` (create a goal file for this session — the evaluation gate \
         independently re-checks every turn against it and decides DONE or \
         CONTINUE, so write concrete acceptance criteria), `delete` (remove \
         your own goal; human-created goals cannot be deleted by agent), \
         `disable` (pause the session goal until a human re-enables it; \
         human-created goals may be blocked by config \
         `goals.allow_agent_disable`). You cannot mark a goal done or \
         re-enable it — only the evaluation gate decides completion and \
         only a human unlocks. For long tasks (likely dozens of tool \
         rounds), call `add` at the start with verifiable acceptance \
         criteria — the gate, not self-reporting, decides when the task \
         is done."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["add", "delete", "disable"], "description": "Which goal operation to perform." },
                "content": { "type": "string", "description": "add: goal text with concrete acceptance criteria (the gate checks every turn against this)." },
                "max_rounds": { "type": "integer", "minimum": 0, "description": "add: CONTINUE-round budget (0 = unlimited, default 256)." },
                "eval_model": { "type": "string", "description": "add: evaluation model slot; \"small\" uses the chore model, omit for the main model." }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String> {
        let parsed: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("args 不是合法 JSON: {e}"))?;
        let action = parsed
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or("缺少 action（add/delete/disable）")?;
        if context.session_key.is_empty() {
            return Err("本会话无会话键，无法绑定目标".to_string());
        }
        match action {
            "add" => self.add(&parsed, context).await,
            "delete" => self.delete(context).await,
            "disable" => self.disable(context).await,
            other => Err(format!("未知 action: {other}（add/delete/disable）")),
        }
    }
}

impl GoalTool {
    async fn add(
        &self,
        parsed: &serde_json::Value,
        context: &RequestContext,
    ) -> Result<String, String> {
        let content = parsed
            .get("content")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or("缺少 content（目标全文，含具体验收标准）")?;
        let max_rounds = match parsed.get("max_rounds") {
            None | Some(serde_json::Value::Null) => None,
            Some(v) => Some(v.as_u64().ok_or("max_rounds 必须是非负整数")?),
        };
        let eval_model = parsed
            .get("eval_model")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from);

        let ts = now_ms();
        let id = update_registry(&self.workspace, |entries| {
            if let Some(existing) = entries
                .iter()
                .find(|e| e.session_key == context.session_key && e.status != GoalStatus::Done)
            {
                return Err(format!(
                    "本会话已有未完成目标 {}（{}）——先 disable 或 delete 后再新建；目标详情见 goals/{}.goal.md",
                    existing.id,
                    existing.status.as_str(),
                    existing.id
                ));
            }
            let id = nemesis_goals::alloc_id(&self.workspace);
            entries.push(nemesis_goals::GoalEntry::new(
                id.clone(),
                context.session_key.clone(),
                GoalCreatedBy::Agent,
                max_rounds,
                eval_model.clone(),
                ts,
            ));
            Ok(id)
        })?;
        write_goal_md(&self.workspace, &id, content)?;
        Ok(format!(
            "目标 {id} 已创建并绑定本会话（created_by=agent）。从下一个回合结束起，评估闸会独立核验目标达成情况：\
             未达成会注入缺口让你继续，达成才判 done——请勿在回复中自行宣告完成。目标全文：goals/{id}.goal.md。"
        ))
    }

    async fn delete(&self, context: &RequestContext) -> Result<String, String> {
        let goal = find_bound_goal(&self.workspace, &context.session_key)?
            .ok_or_else(|| "本会话未绑定任何未完成目标".to_string())?;
        if goal.created_by != GoalCreatedBy::Agent {
            return Err(format!(
                "目标 {} 由人工创建，agent 不可删除——请与用户确认后由用户经 Dashboard 或 WSAPI goals.delete 删除",
                goal.id
            ));
        }
        nemesis_goals::remove_goal(&self.workspace, &goal.id)?;
        Ok(format!(
            "目标 {} 已删除（goal.md + 评估台账 + 注册表行三清）。",
            goal.id
        ))
    }

    async fn disable(&self, context: &RequestContext) -> Result<String, String> {
        let goal = find_bound_goal(&self.workspace, &context.session_key)?
            .ok_or_else(|| "本会话未绑定任何未完成目标".to_string())?;
        if goal.created_by != GoalCreatedBy::Agent
            && !allow_agent_disable(self.config_path.as_deref())
        {
            return Err(format!(
                "目标 {} 由人工创建，且配置 goals.allow_agent_disable=false 禁止 agent 停用人工目标——请与用户确认后由用户经 Dashboard goals.disable 操作",
                goal.id
            ));
        }
        let status = update_registry(&self.workspace, |entries| {
            let e = entries
                .iter_mut()
                .find(|e| e.id == goal.id)
                .ok_or_else(|| format!("goal {} 不存在", goal.id))?;
            if !nemesis_goals::transition_allowed(e.status, GoalStatus::Paused) {
                return Err(format!(
                    "goal {} 当前状态 {}，不允许转移到 paused（评估闸判定/人工操作优先）",
                    e.id,
                    e.status.as_str()
                ));
            }
            e.status = GoalStatus::Paused;
            e.updated_at_ms = now_ms();
            Ok(e.status.as_str())
        })?;
        Ok(format!(
            "目标 {} 已停用（{}）。评估闸不再触发；恢复需人工经 Dashboard goals.enable。",
            goal.id, status
        ))
    }
}

#[cfg(all(test, feature = "goals"))]
mod tests;
