//! 看板数据模型（开发计划 §1.4 MVP ★ 表对应的 Rust 类型）。
//!
//! 存储约定：枚举以蛇形字符串落库（`in_progress`），serde 同形；时间戳为
//! Unix 秒（i64，与 nemesis-data::RequestLog 一致）。

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// 枚举
// ---------------------------------------------------------------------------

/// Issue 状态（§1.1 状态机；合法转移见 [`crate::state_machine`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueStatus {
    Backlog,
    Todo,
    InProgress,
    InReview,
    Done,
    Blocked,
    Cancelled,
}

impl IssueStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            IssueStatus::Backlog => "backlog",
            IssueStatus::Todo => "todo",
            IssueStatus::InProgress => "in_progress",
            IssueStatus::InReview => "in_review",
            IssueStatus::Done => "done",
            IssueStatus::Blocked => "blocked",
            IssueStatus::Cancelled => "cancelled",
        }
    }

    /// 从库里的字符串解析（未知名回退 `None`，由调用方报错）。
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "backlog" => Some(IssueStatus::Backlog),
            "todo" => Some(IssueStatus::Todo),
            "in_progress" => Some(IssueStatus::InProgress),
            "in_review" => Some(IssueStatus::InReview),
            "done" => Some(IssueStatus::Done),
            "blocked" => Some(IssueStatus::Blocked),
            "cancelled" => Some(IssueStatus::Cancelled),
            _ => None,
        }
    }

    /// 终态（done / cancelled）不可再转移。
    pub fn is_terminal(&self) -> bool {
        matches!(self, IssueStatus::Done | IssueStatus::Cancelled)
    }

    /// 人读的合法目标集（错误提示用）。
    pub fn allowed_targets(&self) -> &'static str {
        match self {
            IssueStatus::Backlog => "todo/in_progress/done/blocked/cancelled",
            IssueStatus::Todo => "in_progress/done/blocked/cancelled",
            IssueStatus::InProgress => "in_review/done/blocked/cancelled",
            IssueStatus::InReview => "in_progress/done/blocked/cancelled",
            IssueStatus::Blocked => "todo/in_progress/cancelled",
            IssueStatus::Done => "（终态）",
            IssueStatus::Cancelled => "（终态）",
        }
    }
}

impl std::fmt::Display for IssueStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 评论类型：普通评论 / 状态变更痕迹 / 系统写入（autopilot、回流等）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommentType {
    Comment,
    StatusChange,
    System,
}

impl CommentType {
    pub fn as_str(&self) -> &'static str {
        match self {
            CommentType::Comment => "comment",
            CommentType::StatusChange => "status_change",
            CommentType::System => "system",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "comment" => Some(CommentType::Comment),
            "status_change" => Some(CommentType::StatusChange),
            "system" => Some(CommentType::System),
            _ => None,
        }
    }
}

/// 优先级档位（整数落库，0=低 … 3=紧急；便于排序与过滤）。
pub mod priority {
    pub const LOW: i32 = 0;
    pub const MEDIUM: i32 = 1;
    pub const HIGH: i32 = 2;
    pub const URGENT: i32 = 3;
}

/// 派发记录状态（表 `issue_dispatch.state`；P4 超时/取消策略在此基础上扩展）。
pub mod dispatch_state {
    /// 已派发，等 worker 回报（peer_chat_callback）。
    pub const DISPATCHED: &str = "dispatched";
    /// worker 成功回报（写回结果评论 + issue → in_review）。
    pub const DONE: &str = "done";
    /// worker 汇报失败 / RPC 送达失败 / 超时 sweep 兜底（P4）。
    pub const FAILED: &str = "failed";
    /// 管理端主动取消（P4 per-task cancel；终结动作见 store 的
    /// cancel_dispatch / fail_dispatch）。
    pub const CANCELLED: &str = "cancelled";
}

/// 派发记录（表 `issue_dispatch`；W2 P2 派发链路：issue ↔ peer_chat task 绑定）。
/// task_id 是 peer_chat_callback 的写回路由键。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatchRecord {
    pub task_id: String,
    pub issue_id: i64,
    /// 派发目标节点 id（worker）。
    pub worker_id: String,
    /// [`dispatch_state`] 词表。
    pub state: String,
    pub dispatched_at: i64,
    pub completed_at: Option<i64>,
}

// ---------------------------------------------------------------------------
// 实体
// ---------------------------------------------------------------------------

/// Issue（看板核心实体；表 `issue`）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Issue {
    pub id: i64,
    /// 人读编号，如 `NB-42`（workspace 前缀 + 自增 counter）。
    pub number: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub status: IssueStatus,
    pub priority: i32,
    /// 多态指派（§1.3）：manager_self 或 worker；未指派为 None。
    pub assignee: Option<crate::assignment::AssignmentType>,
    pub assignee_id: Option<String>,
    pub creator: crate::assignment::Actor,
    pub parent_issue_id: Option<i64>,
    pub project_id: Option<i64>,
    /// 截止时间（Unix 秒）。
    pub due_date: Option<i64>,
    /// 看板列内排序位（P3 拖拽用）。
    pub position: i64,
    /// 验收标准（JSON 或自由文本；完成判定 §8：manager agent 自判 done）。
    #[serde(default)]
    pub acceptance_criteria: Option<String>,
    /// 来源（autopilot/cron/channel/...；MVP 仅记录，不做路由）。
    pub origin: Option<TaskOrigin>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 任务来源（issue.origin_type + origin_id 的组合形状）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskOrigin {
    /// 来源种类，如 "autopilot" / "cron" / "channel" / "cli"。
    pub origin_type: String,
    /// 来源 id（cron job id、通道消息 id 等）。
    pub origin_id: String,
}

/// 评论（表 `comment`；`parent_id` 支持一层线程，P3 UI 展开）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Comment {
    pub id: i64,
    pub issue_id: i64,
    pub author: crate::assignment::Actor,
    pub content: String,
    pub parent_id: Option<i64>,
    pub ctype: CommentType,
    pub created_at: i64,
}

/// 时间线条目（表 `activity_log`；details 为 JSON 字符串）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityLog {
    pub id: i64,
    pub issue_id: i64,
    pub actor: crate::assignment::Actor,
    pub action: String,
    #[serde(default)]
    pub details: Option<String>,
    pub created_at: i64,
}

/// 订阅者（表 `issue_subscriber`；通知投递 P4 接通道）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subscriber {
    pub issue_id: i64,
    pub subscriber: crate::assignment::Actor,
    #[serde(default)]
    pub reason: String,
}

/// 项目（表 `project`；MVP 仅分组聚合用）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// 自由字符串：MVP 约定 "active" / "archived"。
    pub status: String,
    pub priority: i32,
    pub lead: Option<crate::assignment::Actor>,
    #[serde(default)]
    pub icon: String,
    pub created_at: i64,
}

/// 附件元数据（表 `attachment`；P1 只记录元数据，文件上传 P3）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub id: i64,
    pub issue_id: i64,
    pub filename: String,
    pub storage_path: String,
    pub size: i64,
    pub created_at: i64,
}

/// 通知种类词表（表 `notification.kind`；P3 store + dashboard 收件箱，
/// 经 21 通道的站外投递留 P4）。
pub mod notification_kind {
    /// 被指派（收件人 = 新指派对象）。
    pub const ASSIGNED: &str = "assigned";
    /// 订阅的 issue 有新评论（收件人 = 订阅者/指派 − 评论作者）。
    pub const COMMENTED: &str = "commented";
    /// 评论中被 @（收件人 = 被 @ 且非作者）。
    pub const MENTIONED: &str = "mentioned";
    /// 指派的 issue 状态变化（收件人 = 指派对象 − 操作者；状态在看板列
    /// 可见，订阅者不推——避免 worker 回报写回时对创建者双份轰炸）。
    pub const STATUS_CHANGED: &str = "status_changed";
    /// 派发失败（P4 超时 sweep / worker 离线兜底；收件人 = 创建者 ∪ 指派
    /// 对象 ∪ 订阅者，由 sweep 调用方显式 [`crate::BoardStore::notify`]，
    /// 不走 store 内部钩子）。
    pub const DISPATCH_FAILED: &str = "dispatch_failed";
}

/// 站内通知（表 `notification`；收件人是多态 Actor）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub id: i64,
    pub recipient: crate::assignment::Actor,
    /// [`notification_kind`] 词表。
    pub kind: String,
    /// 单行人读摘要（如 `NB-3 修复登录`）。
    pub title: String,
    /// 正文片段（评论原文 / 变更说明）。
    #[serde(default)]
    pub content: String,
    /// 关联 issue（可空——保留扩展位）。
    pub issue_id: Option<i64>,
    pub read: bool,
    pub created_at: i64,
}

/// 发通知的输入（[`crate::BoardStore::notify`]）。
#[derive(Debug, Clone)]
pub struct NewNotification {
    pub recipient: crate::assignment::Actor,
    pub kind: String,
    pub title: String,
    pub content: String,
    pub issue_id: Option<i64>,
}

// ---------------------------------------------------------------------------
// 输入形状
// ---------------------------------------------------------------------------

/// 建 issue 的输入（[`crate::BoardStore::create_issue`]）。
#[derive(Debug, Clone)]
pub struct NewIssue {
    pub title: String,
    pub description: String,
    pub priority: i32,
    pub assignee: Option<crate::assignment::AssignmentType>,
    pub assignee_id: Option<String>,
    pub creator: crate::assignment::Actor,
    pub parent_issue_id: Option<i64>,
    pub project_id: Option<i64>,
    pub due_date: Option<i64>,
    pub acceptance_criteria: Option<String>,
    pub origin: Option<TaskOrigin>,
}

impl Default for NewIssue {
    fn default() -> Self {
        Self {
            title: String::new(),
            description: String::new(),
            priority: priority::MEDIUM,
            assignee: None,
            assignee_id: None,
            creator: crate::assignment::Actor::admin("admin"),
            parent_issue_id: None,
            project_id: None,
            due_date: None,
            acceptance_criteria: None,
            origin: None,
        }
    }
}

/// 加评论的输入。
#[derive(Debug, Clone)]
pub struct NewComment {
    pub issue_id: i64,
    pub author: crate::assignment::Actor,
    pub content: String,
    pub parent_id: Option<i64>,
    pub ctype: CommentType,
}

/// 列表过滤条件（None 字段不参与过滤；动态 WHERE）。
#[derive(Debug, Clone, Default)]
pub struct IssueFilter {
    pub status: Option<IssueStatus>,
    /// (assignee_type, assignee_id) 精确匹配。
    pub assignee: Option<(crate::assignment::AssignmentType, String)>,
    pub project_id: Option<i64>,
    pub priority: Option<i32>,
    /// 编号/标题子串（大小写不敏感；空串不过滤）。
    pub query: Option<String>,
}

/// 部分更新 issue 字段的 patch（None = 不改；本 patch 不含 status/assignee，
/// 二者分别走状态机转移与指派接口，保证审计痕迹完整）。
#[derive(Debug, Clone, Default)]
pub struct IssuePatch {
    pub title: Option<String>,
    pub description: Option<String>,
    pub priority: Option<i32>,
    pub project_id: Option<i64>,
    pub due_date: Option<i64>,
    pub position: Option<i64>,
    pub acceptance_criteria: Option<String>,
    pub parent_issue_id: Option<i64>,
}

/// 部分更新项目字段的 patch（None = 不改；status 约定 "active"/"archived"，
/// 归档即软删除——列表仍可见但默认折叠）。
#[derive(Debug, Clone, Default)]
pub struct ProjectPatch {
    pub name: Option<String>,
    pub description: Option<String>,
    pub status: Option<String>,
    pub icon: Option<String>,
}

/// 自动化规则（W2 P4 autopilot；表 `autopilot`）：到点建 issue（标题支持
/// `{date}` 占位符）并可向指定节点派活。run 历史不单独建表——每次触发的
/// issue 带 `origin = autopilot/{id}`，按 origin 反查即历史。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Autopilot {
    pub id: i64,
    pub name: String,
    /// cron 表达式（5 段；经 `nemesis-cron` 的 validate_schedule 校验）。
    pub cron: String,
    /// 建 issue 的标题模板（`{date}` → 当地日期 YYYY-MM-DD）。
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub priority: i32,
    pub project_id: Option<i64>,
    /// 派活目标节点 id（空 = 仅建单不派活，MVP 语义）。
    #[serde(default)]
    pub target: String,
    pub enabled: bool,
    /// live CronService 对应 job 的 id（`board-ap:{id}` 名字约定）。
    /// CronService 不支持指定 job id（add_job_ext 返回随机 id），只能注册
    /// 后回存映射——启动同步据此判「规则有没有挂上 cron」。
    #[serde(default)]
    pub cron_job_id: Option<String>,
    /// 上次触发时间（Unix 秒；触发落账走 [`crate::BoardStore::mark_autopilot_run`]）。
    #[serde(default)]
    pub last_run_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 建 autopilot 的输入（[`crate::BoardStore::create_autopilot`]）。
#[derive(Debug, Clone)]
pub struct NewAutopilot {
    pub name: String,
    pub cron: String,
    pub title: String,
    pub description: String,
    pub priority: i32,
    pub project_id: Option<i64>,
    pub target: String,
    pub enabled: bool,
}

/// 部分更新 autopilot 字段的 patch（None = 不改；cron_job_id/last_run_at
/// 是运行时簿记字段，不进 patch——前者走 `set_autopilot_cron_job`）。
#[derive(Debug, Clone, Default)]
pub struct AutopilotPatch {
    pub name: Option<String>,
    pub cron: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub priority: Option<i32>,
    pub project_id: Option<i64>,
    pub target: Option<String>,
    pub enabled: Option<bool>,
}

#[cfg(test)]
mod tests;
