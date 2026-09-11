//! 全自动流转 P3/D1：`board_issue` 工具——主 agent 的看板建单 + AI 拆解入口。
//!
//! 产品语义「对 master 说一句话建单」：用户在对话里说「建个单把登录修了」，
//! LLM 调 `board_issue create`；「把 NB-7 拆解发车」→ `board_issue plan`。
//!
//! 注册面：**仅主 agent**（`agent_factory.rs::build_agent_loop`；store 未装配
//! = 工具不注册）。cluster agent（worker）**不装**——worker 本地 board.db 是
//! dashboard 视图非权威，建单落不到 master 权威库（与 board_discuss 相反：
//! 发言经 RPC 上行即可，建单必须直写权威 store）。
//!
//! 复用（单一真相源）：`create` 走 `nemesis_web::handlers::board::build_new_issue`
//! （WSAPI `issue.create` 同一解析/默认值）；`plan` 走同 crate 的
//! `execute_plan_chain`（WSAPI issue.plan / autopilot auto_plan / 项目
//! auto_start / 本工具四入口同一编排，含 A1 auto_confirm 语义）。
//!
//! tier 策略：不进 `tier_allowed_tools`（与 board_discuss/cluster_rpc 同族，
//! 仅 Big/Unresolved 全量档可见）；execute 内**显式 tier 闸**兜底（样板
//! `nemesis-web/src/handlers/cluster.rs::persona_generate`，拒 Mini）。

use std::sync::Arc;

use nemesis_agent::context::RequestContext;
use nemesis_board::assignment::Actor;

/// 工具注册名。
pub const TOOL_NAME: &str = "board_issue";

/// board_issue 工具：在权威看板上建单 / 触发 AI 拆解链。
pub struct BoardIssueTool {
    store: Arc<nemesis_board::BoardStore>,
    /// 集群引用（单节点 cluster.enabled=false 时 None——create/plan 仍可用，
    /// 派发诚实失败落系统评论，与 WSAPI issue.plan 同语义）。
    cluster: Option<Arc<nemesis_cluster::cluster::Cluster>>,
    /// planner moderator 槽（gateway 在主 agent 建成后填充；先建槽后填模式）。
    moderator_slot: Arc<std::sync::OnceLock<Arc<nemesis_agent::r#loop::AgentLoop>>>,
    /// home 目录（A1 `board.plan.auto_confirm` 旗标现读）。
    home: std::path::PathBuf,
    /// SSE 事件 hub（plan_ready/plan_failed 推送；None = 不推）。
    hub: Option<Arc<nemesis_web::events::EventHub>>,
}

impl BoardIssueTool {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        store: Arc<nemesis_board::BoardStore>,
        cluster: Option<Arc<nemesis_cluster::cluster::Cluster>>,
        moderator_slot: Arc<std::sync::OnceLock<Arc<nemesis_agent::r#loop::AgentLoop>>>,
        home: std::path::PathBuf,
        hub: Option<Arc<nemesis_web::events::EventHub>>,
    ) -> Self {
        Self {
            store,
            cluster,
            moderator_slot,
            home,
            hub,
        }
    }

    /// 解析后的 moderator loop（tier 闸 + plan 链共用）。
    fn moderator(&self) -> Result<Arc<nemesis_agent::r#loop::AgentLoop>, String> {
        self.moderator_slot
            .get()
            .cloned()
            .ok_or_else(|| "board 服务未就绪（moderator agent 未运行），请稍后再试".to_string())
    }

    /// 建单/拆解的操作者身份：集群节点 id（单机 = "master"）。
    fn actor(&self) -> Actor {
        match &self.cluster {
            Some(c) => Actor::agent(c.node_id()),
            None => Actor::agent("master"),
        }
    }

    /// issue 引用 → Issue：纯数字 = DB id；`NB-<n>` = 单号。
    fn resolve_issue(&self, issue_ref: &str) -> Result<nemesis_board::Issue, String> {
        let r = issue_ref.trim();
        if let Some(n) = r.strip_prefix("NB-").or_else(|| r.strip_prefix("nb-")) {
            return self.store.get_issue_by_number(&format!("NB-{n}"));
        }
        r.parse::<i64>()
            .map_err(|_| format!("issue 引用 {r:?} 无法识别（应为数字 id 或 NB-<n> 单号）"))
            .and_then(|id| self.store.get_issue(id))
    }
}

/// 解析后的子命令（execute 内部第一步；独立成纯函数便于单测）。
#[derive(Debug)]
pub(crate) enum BoardIssueCommand {
    Create {
        title: String,
        description: String,
        acceptance_criteria: Option<String>,
        priority: Option<i32>,
        project_id: Option<i64>,
    },
    Plan {
        issue_ref: String,
    },
}

/// args JSON → 结构化子命令（缺字段 / 空 title / 非法 priority 一律诚实报错
/// ——args_validator 兜的是 schema 层，语义层这里自己守）。
pub(crate) fn parse_board_issue_args(args: &str) -> Result<BoardIssueCommand, String> {
    let v: serde_json::Value =
        serde_json::from_str(args).map_err(|e| format!("invalid JSON args: {e}"))?;
    let sub = v
        .get("subcommand")
        .and_then(|x| x.as_str())
        .ok_or("missing field: subcommand (expected \"create\" or \"plan\")")?;
    match sub {
        "create" => {
            let title = v
                .get("title")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if title.is_empty() {
                return Err("create requires a non-empty title".to_string());
            }
            let description = v
                .get("description")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            let acceptance_criteria = v
                .get("acceptance_criteria")
                .and_then(|x| x.as_str())
                .map(str::to_string);
            let priority = match v.get("priority") {
                None | Some(serde_json::Value::Null) => None,
                Some(p) => {
                    let n = p.as_i64().ok_or("priority must be an integer (0-3)")?;
                    if !(0..=3).contains(&n) {
                        return Err(format!(
                            "priority {n} out of range (0=low 1=medium 2=high 3=urgent)"
                        ));
                    }
                    Some(n as i32)
                }
            };
            let project_id = v.get("project_id").and_then(|x| x.as_i64());
            Ok(BoardIssueCommand::Create {
                title,
                description,
                acceptance_criteria,
                priority,
                project_id,
            })
        }
        "plan" => {
            let issue_ref = v
                .get("issue")
                .and_then(|x| x.as_str())
                .or_else(|| v.get("issue_id").and_then(|x| x.as_str()))
                .unwrap_or("")
                .trim()
                .to_string();
            if issue_ref.is_empty() {
                return Err("plan requires \"issue\" (numeric id or NB-<n>)".to_string());
            }
            Ok(BoardIssueCommand::Plan { issue_ref })
        }
        other => Err(format!(
            "unknown subcommand {other:?} (expected \"create\" or \"plan\")"
        )),
    }
}

impl BoardIssueTool {
    /// 显式 tier 闸（样板 persona_generate）：mini 档拒。供给侧已把本工具从
    /// mini/normal 的工具表滤掉，这里是运行时兜底（tier 热切 / 直接调用）。
    fn check_tier(tier: nemesis_types::capability::ModelTier) -> Result<(), String> {
        if matches!(tier, nemesis_types::capability::ModelTier::Mini) {
            return Err(
                "当前模型能力档为 mini，看板建单/拆解需要 normal/big 档模型，请切换或升级模型后再试"
                    .to_string(),
            );
        }
        Ok(())
    }

    /// A1 旗标现读的 home 入参（未装配 home → None，链内 fail-closed）。
    fn home_ref(&self) -> Option<&std::path::Path> {
        if self.home.as_os_str().is_empty() {
            None
        } else {
            Some(self.home.as_path())
        }
    }
}

#[async_trait::async_trait]
impl nemesis_agent::r#loop::Tool for BoardIssueTool {
    fn description(&self) -> String {
        "Create a board issue or run AI planning on an existing one. \
         Use `create` when the user asks to file a task/issue on the board \
         (\"帮我在看板建个单…\"); use `plan` to decompose a parent issue into \
         sub-issues with the planner and (if board.plan.auto_confirm is on) \
         auto-dispatch them. Returns the created issue number, or the plan \
         result (plan_id / sub-issue count / dispatch status)."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "oneOf": [
                {
                    "type": "object",
                    "properties": {
                        "subcommand": { "const": "create" },
                        "title": { "type": "string", "description": "Issue title (concise imperative)." },
                        "description": { "type": "string", "description": "Background / context for the task." },
                        "acceptance_criteria": { "type": "string", "description": "Optional acceptance criteria (one per line)." },
                        "priority": { "type": "integer", "enum": [0, 1, 2, 3], "description": "0=low 1=medium (default) 2=high 3=urgent." },
                        "project_id": { "type": "integer", "description": "Optional project to attach the issue to." }
                    },
                    "required": ["subcommand", "title"]
                },
                {
                    "type": "object",
                    "properties": {
                        "subcommand": { "const": "plan" },
                        "issue": { "type": "string", "description": "Parent issue reference: numeric id or NB-<n> number." }
                    },
                    "required": ["subcommand", "issue"]
                }
            ]
        })
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let cmd = parse_board_issue_args(args)?;
        match cmd {
            BoardIssueCommand::Create {
                title,
                description,
                acceptance_criteria,
                priority,
                project_id,
            } => {
                let moderator = self.moderator()?;
                Self::check_tier(moderator.tier())?;
                let payload = serde_json::json!({
                    "title": title,
                    "description": description,
                    "acceptance_criteria": acceptance_criteria,
                    "priority": priority,
                    "project_id": project_id,
                });
                let new_issue =
                    nemesis_web::handlers::board::build_new_issue(&payload, self.actor())?;
                let issue = self.store.create_issue(new_issue)?;
                tracing::info!(
                    "[BoardIssueTool] created issue {} (id {}) via agent tool",
                    issue.number,
                    issue.id
                );
                Ok(format!(
                    "已建单 {}：{}\n状态 {} · 优先级 {} · 项目 {:?}\n可继续用 board_issue plan 对它做 AI 拆解。",
                    issue.number, issue.title, issue.status, issue.priority, issue.project_id,
                ))
            }
            BoardIssueCommand::Plan { issue_ref } => {
                let moderator = self.moderator()?;
                Self::check_tier(moderator.tier())?;
                let issue = self.resolve_issue(&issue_ref)?;
                let plan_id = format!("plan-{}", uuid::Uuid::new_v4());
                // 同步 await（调用方决定阻塞）：agent 拿完整链结果向用户汇报，
                // SSE 事件照发（hub 在手）。
                let out = nemesis_web::handlers::board::execute_plan_chain(
                    &self.store,
                    self.cluster.clone(),
                    moderator,
                    issue,
                    self.actor(),
                    &plan_id,
                    self.home_ref(),
                    self.hub.as_deref(),
                )
                .await?;
                Ok(serde_json::to_string_pretty(&out).unwrap_or_else(|_| out.to_string()))
            }
        }
    }
}

#[cfg(test)]
mod tests;
