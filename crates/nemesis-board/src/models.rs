//! 看板数据模型（开发计划 MVP ★ 表对应的 Rust 类型）。
//!
//! 存储约定：枚举以蛇形字符串落库（`in_progress`），serde 同形；时间戳为
//! Unix 秒（i64，与 nemesis-data::RequestLog 一致）。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// 枚举
// ---------------------------------------------------------------------------

/// Issue 状态（状态机；合法转移见 [`crate::state_machine`]）。
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
    #[allow(clippy::should_implement_trait)] // lenient parser with fallback; deliberately NOT std::str:FromStr (no Err semantics)
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
            IssueStatus::Cancelled => "backlog（issue.reopen）",
        }
    }
}

impl std::fmt::Display for IssueStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 评论类型：普通评论 / 状态变更痕迹 / 系统写入（autopilot、回流等）；
/// 扩展：讨论发言 / 提问 / 交付物回流（信封 `board.comment.post` 的
/// `mtype` 与之对应）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommentType {
    Comment,
    StatusChange,
    System,
    Discussion,
    Question,
    Delivery,
}

impl CommentType {
    pub fn as_str(&self) -> &'static str {
        match self {
            CommentType::Comment => "comment",
            CommentType::StatusChange => "status_change",
            CommentType::System => "system",
            CommentType::Discussion => "discussion",
            CommentType::Question => "question",
            CommentType::Delivery => "delivery",
        }
    }

    #[allow(clippy::should_implement_trait)] // lenient parser with fallback; deliberately NOT std::str:FromStr (no Err semantics)
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "comment" => Some(CommentType::Comment),
            "status_change" => Some(CommentType::StatusChange),
            "system" => Some(CommentType::System),
            "discussion" => Some(CommentType::Discussion),
            "question" => Some(CommentType::Question),
            "delivery" => Some(CommentType::Delivery),
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

/// 派发记录状态（表 `issue_dispatch.state`； 超时/取消策略在此基础上扩展）。
pub mod dispatch_state {
    /// 已派发，等 worker 回报（peer_chat_callback）。
    pub const DISPATCHED: &str = "dispatched";
    /// worker 已出队开跑（goal / 重平衡信号：queued vs executing 的
    /// 分界——重平衡只挪 queued 单，running 单不可迁移）。由 worker 出队时
    /// 经 nb_bus `task.started` 上报（信封 ns="task"）。
    pub const RUNNING: &str = "running";
    /// worker 成功回报（写回结果评论 + issue → in_review）。
    pub const DONE: &str = "done";
    /// worker 汇报失败 / RPC 送达失败 / 超时 sweep 兜底。
    pub const FAILED: &str = "failed";
    /// 管理端主动取消（per-task cancel；终结动作见 store 的
    /// cancel_dispatch / fail_dispatch）。
    pub const CANCELLED: &str = "cancelled";
}

/// 派发记录（表 `issue_dispatch`； 派发链路：issue ↔ peer_chat task 绑定）。
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
    /// 派发代际号（a ④）：首派 0、第 N 次重派 N-1（落库时按该
    /// issue 已有派发行数计算）。随派发下行到 worker、写回原样回声——
    /// 写回闸据此拒绝旧代迟到回调覆盖新一代派发状态。
    pub generation: i64,
}

/// 自检取证在途路由持久行（表 `selfcheck_pending`）。
/// selfcheck 派发不写 `issue_dispatch`（写了会被 Route 0 当交付写回），
/// 路由键以本表持久化：gateway 重启后内存注册表清零，worker 取证回调凭
/// 本表兜底路由；`status`/`response` 同时是二段验收状态机——`None` =
/// 回调未到（可被恢复轮询捞回），`Some` = 已路由待二段完成（完成删行）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelfcheckPendingRow {
    pub task_id: String,
    pub issue_id: i64,
    /// 派发目标节点 id（worker；恢复轮询目标）。
    pub worker_id: String,
    /// 回调状态（"ok"/"error"；NULL = 回调未到）。
    pub status: Option<String>,
    /// 回调取证内容（与 status 同批烧入）。
    pub response: Option<String>,
    pub created_at: i64,
}

/// 试玩取证路由行（表 `playtest_pending`，P3b 机器行为闸）——与
/// [`SelfcheckPendingRow`] 同形态：派发不写 issue_dispatch，路由键持久化
/// + 回调状态机（NULL = 回调未到；Some = 已烧入待定案臂消费，完成删行）。
pub struct PlaytestPendingRow {
    pub task_id: String,
    pub issue_id: i64,
    /// 派发目标节点 id（worker；恢复轮询目标）。
    pub worker_id: String,
    /// 回调状态（"ok"/"error"；NULL = 回调未到）。
    pub status: Option<String>,
    /// 回调试玩报告正文（与 status 同批烧入）。
    pub response: Option<String>,
    pub created_at: i64,
}

// ---------------------------------------------------------------------------
// 实体
// ---------------------------------------------------------------------------

/// Issue（看板核心实体；表 `issue`）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Issue {
    pub id: i64,
    /// 人读编号，如 ``（workspace 前缀 + 自增 counter）。
    pub number: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub status: IssueStatus,
    pub priority: i32,
    /// 多态指派：manager_self 或 worker；未指派为 None。
    pub assignee: Option<crate::assignment::AssignmentType>,
    pub assignee_id: Option<String>,
    pub creator: crate::assignment::Actor,
    pub parent_issue_id: Option<i64>,
    pub project_id: Option<i64>,
    /// 截止时间（Unix 秒）。
    pub due_date: Option<i64>,
    /// 看板列内排序位（拖拽用）。
    pub position: i64,
    /// 验收标准（JSON 或自由文本；完成判定：manager agent 自判 done）。
    #[serde(default)]
    pub acceptance_criteria: Option<String>,
    /// 来源（autopilot/cron/channel/...；MVP 仅记录，不做路由）。
    pub origin: Option<TaskOrigin>,
    /// Swarm （planner 拆解）：派发需求角色（匹配器硬条件；None=不限）。
    #[serde(default)]
    pub required_role: Option<String>,
    /// Swarm：派发需求标签（匹配器硬条件；空=不限）。
    #[serde(default)]
    pub required_tags: Vec<String>,
    /// 集群专业职能框架：派发需求职能 slug（`family[:spec]`，精确
    /// 匹配无继承；None/空 = 不限职能）。
    #[serde(default)]
    pub required_profession: Option<String>,
    /// 环境能力硬条件（16）：派发需求节点能力词（与
    /// 节点宣告的 capabilities 同词表；匹配是全量硬闸、不参与松弛，见
    /// matcher 硬条件 5）；空 = 无能力需求。
    #[serde(default)]
    pub required_capabilities: Vec<String>,
    /// 派发需求运行时后端：`process`|`vm`，与节点
    /// 宣告的 runtime_backend 精确匹配（大小写不敏感；节点未宣告 =
    /// fail-closed 排除，不参与松弛，见 matcher 硬条件 6）；None/空 =
    /// 无需求不过闸（旧节点混编零回归）。
    #[serde(default)]
    pub required_backend: Option<String>,
    /// 交付证据契约：AC 含 file: 类锚点 → 建单时
    /// 自动置位（统一定义在 [`crate::store:BoardStore::create_issue`]，
    /// 按 `has_file_anchor` 谓词）；置位后验收 PASS 必须有交付记录背书
    /// （delivered/confirmed），缺失 = 评审门降级 FAIL「缺交付证据」。
    /// 只读字段（无 patch 面）：由 AC 内容决定，不是人工意志位。
    #[serde(default)]
    pub requires_evidence: bool,
    /// 取消单「永久收起」标记（/ 清理出口）：任何前端表面都不再出现，
    /// 即使「显示已取消」开关打开。board.db 仍保留行（审计链完整）。
    #[serde(default)]
    pub hidden: bool,
    /// 质量档（v25 列；看板人工触点体系）：'' = 标准（默认，无触点）/
    /// `experience` = 体验型（规格确认 + 原型里程碑两触点）。空串/未知值
    /// 均按标准档处理（读取宽容，写入收口在 WSAPI issue.create）。
    #[serde(default)]
    pub quality: String,
    /// 任务级无人值守覆盖（v25 列）：None = 继承项目/配置默认；
    /// Some(true/false) = 显式覆盖。沿 conflict_frozen 的 Option 形态——
    /// None 是「未表态」，不是 false。
    #[serde(default)]
    pub unattended: Option<bool>,
    /// 原型里程碑单标记（v26 列；触点体系 P3）：planner 拆解体验型
    /// 任务时把首个可运行构建的子单标为里程碑单——其评审链先过机器
    /// 行为闸再挂原型触点卡。父单非 experience 档时该标记无消费方
    /// （confirm_plan 落库前归一清零）。
    #[serde(default)]
    pub milestone: bool,
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
    /// 来源坐标映射（JSON 落 origin_meta 列；flow 模板 land 时写入）。
    /// 语义由来源方定义（如 `{"owner":..,"repo":..,"number":..}`），board
    /// 只存取不解释；v1 旧单无此字段（DEFAULT 空 map）。
    #[serde(default)]
    pub origin_meta: BTreeMap<String, String>,
}

/// 评论（表 `comment`；`parent_id` 支持一层线程， UI 展开）。
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

/// 审计决策行（全自动流转 / 决策流视图）：`auto_decide` 活动 + 关联
/// 单据编号/标题（JOIN 补齐，决策列表直接可读）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditDecisionRow {
    #[serde(flatten)]
    pub activity: ActivityLog,
    pub issue_number: String,
    pub issue_title: String,
}

/// 回滚先例行（判例沉淀）：一条「自动决策被人工回滚」的活动记录 + 关联
/// 单据编号/标题 + 原决策内容。数据面产物，供评审提示词参考注入
/// （渲染侧带数据非指令护栏，见 nemesis-prompts `board::render_precedents_block`）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollbackPrecedent {
    /// 回滚活动 id（activity_log.id）。
    pub rollback_activity_id: i64,
    /// 关联单据编号（如）。
    pub issue_number: String,
    /// 单据标题。
    pub issue_title: String,
    /// 原自动决策内容（audit_rollback → 原 auto_decide details JSON；
    /// done_rollback → 「done（自动收货）」占位——终态回滚无独立决策记录可考）。
    pub decision: String,
    /// 回滚原因（done_rollback 内嵌 reason；audit_rollback 无 → 空）。
    pub rollback_reason: String,
}

/// 订阅者（表 `issue_subscriber`；通知投递 接通道）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subscriber {
    pub issue_id: i64,
    pub subscriber: crate::assignment::Actor,
    #[serde(default)]
    pub reason: String,
}

/// 项目状态（全自动流转）：主链 `active → in_progress → completed`，
/// 归档旁路 `archived`。合法转移表见 [`crate::project_state`]；存量库中的
/// 未知字符串在读取时宽容映射 `active`（[`crate::store`] row 映射统一收口）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectStatus {
    Active,
    InProgress,
    Completed,
    Archived,
}

impl ProjectStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProjectStatus::Active => "active",
            ProjectStatus::InProgress => "in_progress",
            ProjectStatus::Completed => "completed",
            ProjectStatus::Archived => "archived",
        }
    }

    /// 从库里的字符串解析（未知名回退 `None`，由调用方决定宽容/报错）。
    #[allow(clippy::should_implement_trait)] // lenient parser with fallback; deliberately NOT std::str:FromStr (no Err semantics)
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "active" => Some(ProjectStatus::Active),
            "in_progress" => Some(ProjectStatus::InProgress),
            "completed" => Some(ProjectStatus::Completed),
            "archived" => Some(ProjectStatus::Archived),
            _ => None,
        }
    }

    /// 人读的合法目标集（错误提示用）。
    pub fn allowed_targets(&self) -> &'static str {
        match self {
            ProjectStatus::Active => "in_progress/archived",
            ProjectStatus::InProgress => "completed/archived",
            // completed → in_progress 为 项目级验收 FAIL 自动回退预留。
            ProjectStatus::Completed => "archived/in_progress",
            ProjectStatus::Archived => "active（重开）",
        }
    }
}

impl std::fmt::Display for ProjectStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 项目（表 `project`；MVP 仅分组聚合用）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// 状态词表：`active / in_progress / completed / archived`（状态机，
    /// 合法转移见 [`crate::project_state`]）。落库为自由字符串——前端兼容
    /// 旧值；写入校验收口在 [`crate::store:BoardStore::update_project`]。
    pub status: String,
    pub priority: i32,
    pub lead: Option<crate::assignment::Actor>,
    #[serde(default)]
    pub icon: String,
    /// 项目级验收标准（v10 列；空 = 无）。 收口验收聚合父单 AC 时本字段
    /// 排首位——project.create/patch 写入，完成判定真实消费（此前只存父单，
    /// 项目级标准无处安放）。
    #[serde(default)]
    pub acceptance_criteria: String,
    /// 项目档案目录绝对路径（v12 列；看板项目档案 goal）。None =
    /// 存量项目/未绑定——不参与档案管线（投影/里程碑写入全部跳过，不回填）。
    /// **绑定不可变**：仅 create_project 写入一次，此后改名不改目录；
    /// ProjectPatch 刻意不含本字段（无任何修改入口）。
    #[serde(default)]
    pub directory: Option<String>,
    /// 冲突冻结标志（v14 列；看板项目档案 goal）。true = 项目域冻结：
    /// 新派发被闸拒绝、合并只登记 pending_merges 不合入。只限本项目（不株连
    /// 其他项目），不动 ProjectStatus 状态机。系统管理（冲突停车置位 /
    /// `project.resume` 解除），ProjectPatch 刻意不含。
    #[serde(default)]
    pub conflict_frozen: bool,
    /// 待补合并队列（v14 列 JSON；看板项目档案 goal）。冻结期在途
    /// 交付「收到→入档案→不合入」，未合入变更集按完成顺序登记于此；
    /// 解冻后串行补合并（补合并再冲突→再走漏斗）。系统管理，ProjectPatch
    /// 刻意不含。
    #[serde(default)]
    pub pending_merges: Vec<PendingMerge>,
    /// 项目级无人值守默认（v25 列；看板人工触点体系）：None = 按 config
    /// `board.attendance.mode` 全局默认；Some(true) = 本项目一路走到底
    /// （触点只记档不等待）；Some(false) = 本项目强制人工检查点。就地翻转
    /// （规格确认卡「别再问我」）走系统管理 setter（`set_project_unattended`），
    /// ProjectPatch 刻意不含（无人值守是任务声明，不是随手 patch 面）。
    #[serde(default)]
    pub unattended: Option<bool>,
    pub created_at: i64,
}

/// 待补合并队列条目：冻结期收到但未合入的变更集登记。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PendingMerge {
    /// 派发任务 id（变更集归属键）。
    pub task_id: String,
    /// 变更集所属 issue。
    pub issue_id: i64,
    /// 变更集安置目录（执行档案落地处，`changeset/` 子目录可再读）。
    pub placement_dir: String,
    /// 登记原因（冻结中 / 冲突停车等，人读）。
    pub reason: String,
    /// 登记时刻（epoch ms）。
    pub parked_at_ms: i64,
}

// ---------------------------------------------------------------------------
// 人工触点（看板人工触点体系，v25；表 `board_touchpoints`）
// ---------------------------------------------------------------------------

/// 触点种类（`board_touchpoints.kind`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TouchpointKind {
    /// 规格点火：规划入口的规格蒸馏产物确认（确认前不拆解）。
    Spec,
    /// 原型里程碑：首个可运行构建，过机器行为闸后人工方向校验。
    Prototype,
}

impl TouchpointKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            TouchpointKind::Spec => "spec",
            TouchpointKind::Prototype => "prototype",
        }
    }
    /// 数据库字符串 → 枚举（命名避开 `FromStr::from_str` 歧义，clippy 纪律）。
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "spec" => Some(TouchpointKind::Spec),
            "prototype" => Some(TouchpointKind::Prototype),
            _ => None,
        }
    }
}

impl std::fmt::Display for TouchpointKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 触点状态（`board_touchpoints.status`；触点自己的状态机，见
/// [`crate::touchpoint`]——不污染 issue 通用状态机，对齐审计回滚先例）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TouchpointStatus {
    /// 等人（deadline 前的唯一活态）。
    Pending,
    /// 人工确认。
    Confirmed,
    /// 超时按机器默认动作收口（沉默 = 显式审计事件）。
    TimeoutDefaulted,
    /// 局部否决（返工）——返工完成后机器闸复验，复验通过回 pending
    /// （勾「改完再看」）或直接继续。
    VetoedLocal,
    /// 改规格否决——按依赖图裁剪未开工受影响子单后重拆重派。
    VetoedSpec,
    /// 人工取消（整单取消）。
    Cancelled,
    /// 震荡熔断停车（同一范围连续 veto_max_rounds 次否决且无机械进展
    /// → 整单停车标「需同步沟通」）。
    ParkedSync,
}

impl TouchpointStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TouchpointStatus::Pending => "pending",
            TouchpointStatus::Confirmed => "confirmed",
            TouchpointStatus::TimeoutDefaulted => "timeout_defaulted",
            TouchpointStatus::VetoedLocal => "vetoed_local",
            TouchpointStatus::VetoedSpec => "vetoed_spec",
            TouchpointStatus::Cancelled => "cancelled",
            TouchpointStatus::ParkedSync => "parked_sync",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(TouchpointStatus::Pending),
            "confirmed" => Some(TouchpointStatus::Confirmed),
            "timeout_defaulted" => Some(TouchpointStatus::TimeoutDefaulted),
            "vetoed_local" => Some(TouchpointStatus::VetoedLocal),
            "vetoed_spec" => Some(TouchpointStatus::VetoedSpec),
            "cancelled" => Some(TouchpointStatus::Cancelled),
            "parked_sync" => Some(TouchpointStatus::ParkedSync),
            _ => None,
        }
    }
    /// 终态（no self-transition；恢复只经显式回滚路径，不在状态机内；
    /// timeout_defaulted 的迟到输入走评论进变更管理，不重开触点）。
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            TouchpointStatus::Confirmed
                | TouchpointStatus::TimeoutDefaulted
                | TouchpointStatus::Cancelled
                | TouchpointStatus::ParkedSync
        )
    }
}

impl std::fmt::Display for TouchpointStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 触点行（表 `board_touchpoints`）。触点单本体 = 普通 issue（本表只存
/// 触点语义字段，不重复单据已有的状态机/评论/任务树）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Touchpoint {
    pub id: i64,
    /// 承载触点单的 issue id（触点卡/决定史评论全在这张单上）。
    pub issue_id: i64,
    /// 归属项目（冗余自 issue.project_id，超时 sweep 按项目过滤用）。
    pub project_id: Option<i64>,
    pub kind: TouchpointKind,
    pub status: TouchpointStatus,
    /// 绝对 deadline（epoch ms）：estop/停机不白等（release 后按真实墙钟
    /// 判定，已过线立即按默认动作收口）。
    pub deadline_ms: i64,
    /// 触点载荷 JSON（spec=规格正文/假设清单/体验验收条款/版本号；
    /// prototype=红队报告/playtest 摘要/资产清单）。结构由挂卡方定义，
    /// board 只存取不解释。
    #[serde(default)]
    pub payload: String,
    /// 人工输入 JSON（确认=空/修订=结构化 delta/否决=范围+原因+改完再看）。
    #[serde(default)]
    pub human_input: String,
    /// 同范围否决轮数（veto_max_rounds 熔断计数器）。
    #[serde(default)]
    pub veto_rounds: i64,
    /// 审计事件数组 JSON（沉默/迟到输入都是显式事件）。
    #[serde(default)]
    pub audit: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 建触点的输入（[`crate::store:BoardStore::create_touchpoint`]）。
#[derive(Debug, Clone)]
pub struct NewTouchpoint {
    pub issue_id: i64,
    pub project_id: Option<i64>,
    pub kind: TouchpointKind,
    /// 绝对 deadline（epoch ms）；调用方按 config 超时换算。
    pub deadline_ms: i64,
    /// 初始载荷 JSON（空 = 超时后由默认动作兜底）。
    pub payload: String,
}

/// 附件元数据（表 `attachment`； 只记录元数据，文件上传）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub id: i64,
    pub issue_id: i64,
    pub filename: String,
    pub storage_path: String,
    pub size: i64,
    pub created_at: i64,
}

/// 通知种类词表（表 `notification.kind`； store + dashboard 收件箱，
/// 经 21 通道的站外投递留）。
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
    /// 派发失败（超时 sweep / worker 离线兜底；收件人 = 创建者 ∪ 指派
    /// 对象 ∪ 订阅者，由 sweep 调用方显式 [`crate::BoardStore:notify`]，
    /// 不走 store 内部钩子）。
    pub const DISPATCH_FAILED: &str = "dispatch_failed";
    /// 重规划建议被策略闸拒绝（autonomous 模式下建议动作
    /// 超约束被拒 → 冻结 + 审计评论 + 本通知显形——宁可停不可闯）。
    pub const REPLAN_BLOCKED: &str = "replan_blocked";
    /// 人工触点挂卡（规格/原型等待确认；收件人 = admin，挂卡即回执——
    /// 系统评论不触发评论通知，触点三拍显式 notify）。
    pub const TOUCHPOINT_PENDING: &str = "touchpoint_pending";
    /// 触点等待提醒（T+10min 一拍；audit `remind` 幂等去重）。
    pub const TOUCHPOINT_REMIND: &str = "touchpoint_remind";
    /// 触点超时通报（按默认继续 / 转同步沟通，已落系统评论）。
    pub const TOUCHPOINT_TIMEOUT: &str = "touchpoint_timeout";
    /// 项目汇报包就绪（收口自动产出；收件人 = admin，正文含报告摘要）。
    pub const REPORT_READY: &str = "report_ready";
}

/// 站内通知（表 `notification`；收件人是多态 Actor）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub id: i64,
    pub recipient: crate::assignment::Actor,
    /// [`notification_kind`] 词表。
    pub kind: String,
    /// 单行人读摘要（如 ` 修复登录`）。
    pub title: String,
    /// 正文片段（评论原文 / 变更说明）。
    #[serde(default)]
    pub content: String,
    /// 关联 issue（可空——保留扩展位）。
    pub issue_id: Option<i64>,
    pub read: bool,
    pub created_at: i64,
}

/// 发通知的输入（[`crate::BoardStore:notify`]）。
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
    /// Swarm （planner 拆解）：派发需求角色/标签（None/空 = 不限）。
    pub required_role: Option<String>,
    pub required_tags: Vec<String>,
    /// 集群专业职能框架：派发需求职能 slug（None/空 = 不限）。
    pub required_profession: Option<String>,
    /// 环境能力硬条件（16）：派发需求节点能力词
    /// （匹配硬闸、不参与松弛）；空 = 无能力需求。
    pub required_capabilities: Vec<String>,
    /// 派发需求运行时后端：`process`|`vm`，精确匹配 + fail-closed
    /// （不参与松弛）；None/空 = 无需求不过闸。
    pub required_backend: Option<String>,
    /// 质量档（'' = 标准 / `experience` = 体验型；触点体系挂卡依据）。
    pub quality: String,
    /// 任务级无人值守覆盖（None = 继承项目/配置默认）。
    pub unattended: Option<bool>,
    /// 原型里程碑单标记（planner 拆解产出；confirm_plan 按父单质量档
    /// 归一落库——父单非 experience 档时清零）。
    pub milestone: bool,
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
            required_role: None,
            required_tags: Vec::new(),
            required_profession: None,
            required_capabilities: Vec::new(),
            required_backend: None,
            quality: String::new(),
            unattended: None,
            milestone: false,
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
    /// 排除归档项目（status=archived）名下的子单。无项目单不受
    /// 影响。Default false = 全量（既有内部调用行为不变）；WSAPI issue.list
    /// 按 `include_archived_projects` 入参映射（默认排除——用户面两个页签
    /// 同享一份默认，不留「列表隐藏看板还在」的半吊子）。
    pub exclude_archived_projects: bool,
    /// 排除已取消单。Default false = 全量；WSAPI issue.list 按
    /// `include_cancelled` 入参映射（默认排除，可开关显示）。
    pub exclude_cancelled: bool,
    /// 排除 hidden 单（/ 清理出口——永久收起，没有放行口）。Default
    /// false = 全量（内部调用/详情可）；WSAPI issue.list 恒置 true。
    pub exclude_hidden: bool,
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
    /// 项目级验收标准（v10 列； 收口验收聚合输入首位）。None = 不改。
    pub acceptance_criteria: Option<String>,
}

/// 自动化规则（autopilot；表 `autopilot`）：到点建 issue（标题支持
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
    /// 全自动流转：触发建单后自动跑 planner 拆解（仅当 target 为空=
    /// 未预指派 worker 时）。与 `board.plan.auto_confirm`（拆解完是否自动
    /// 发车）正交：auto_plan 管「是否拆解」，auto_confirm 管「拆解完是否
    /// 发车」。DB 列 v9 迁移，serde default false（存量规则行为不变）。
    #[serde(default)]
    pub auto_plan: bool,
    /// UAT F-：验收标准（触发建单时透传给 issue）。空/缺省 = 不填，
    /// 评审保持「验收标准（未提供）」保守转人工的既有语义。DB 列 v15 迁移。
    #[serde(default)]
    pub acceptance_criteria: Option<String>,
    /// 三期：可选绑定发起 session_key（父会话）。绑定的
    /// 规则触发时先验父会话存活（jsonl 在档且非空）：存活 = 建单继承父会话
    /// 近段上下文（description 注入）+ 单据可溯源父会话；消失（清空/删除）
    /// = 规则自动暂停 + 留痕评论，不孤儿运行。None = 未绑定 = 现行行为
    /// 零变化。DB 列 v22 迁移（空串归一 None）。
    #[serde(default)]
    pub session_key: Option<String>,
    /// 触发模式（DB 列 v25 迁移，serde default live = 存量规则行为零变化）：
    /// - [`AUTOPILOT_MODE_LIVE`]：全真执行（建单 + 派发/拆解）；
    /// - [`AUTOPILOT_MODE_SHADOW`]：影子（record-only）——只建单 + 决策意图
    ///   评论 + 本地留痕，不派发不拆解（新规则起步坡道；promote = 改回
    ///   live）。
    ///
    /// 未知值 store 层 loud 拒绝（create/update 双闸），不用静默纠偏。
    #[serde(default = "default_autopilot_mode")]
    pub mode: String,
    pub enabled: bool,
    /// live CronService 对应 job 的 id（`board-ap:{id}` 名字约定）。
    /// CronService 不支持指定 job id（add_job_ext 返回随机 id），只能注册
    /// 后回存映射——启动同步据此判「规则有没有挂上 cron」。
    #[serde(default)]
    pub cron_job_id: Option<String>,
    /// 上次触发时间（Unix 秒；触发落账走 [`crate::BoardStore:mark_autopilot_run`]）。
    #[serde(default)]
    pub last_run_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 建 autopilot 的输入（[`crate::BoardStore:create_autopilot`]）。
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
    /// 全自动流转：建单后自动 planner 拆解（同 [`Autopilot::auto_plan`]）。
    pub auto_plan: bool,
    /// UAT F-：验收标准（建单透传，同 [`Autopilot::acceptance_criteria`]）。
    pub acceptance_criteria: Option<String>,
    /// 三期：父会话绑定（同 [`Autopilot::session_key`]）。None = 不绑定。
    pub session_key: Option<String>,
    /// 触发模式（同 [`Autopilot::mode`]；空串 store 归一为 live）。
    pub mode: String,
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
    /// 全自动流转：`Some(v)` = 改 auto_plan（None = 不改）。
    pub auto_plan: Option<bool>,
    /// UAT F-：`Some(v)` = 改验收标准（`Some("")` = 清空，语义同
    /// 不填；None = 不改）。
    pub acceptance_criteria: Option<String>,
    /// 三期：`Some(v)` = 改父会话绑定（`Some("")` = 清空绑定；None =
    /// 不改）。
    pub session_key: Option<String>,
    /// 触发模式：`Some(v)` = 改 mode（promote 路径 shadow→live 走这里；
    /// None = 不改）。非法值 store 层 loud 拒绝。
    pub mode: Option<String>,
}

/// 触发模式值集（单一真相源；store create/update 双闸校验）。
pub const AUTOPILOT_MODE_LIVE: &str = "live";
/// 影子模式值（record-only 起步坡道；见 [`Autopilot::mode`]）。
pub const AUTOPILOT_MODE_SHADOW: &str = "shadow";

/// mode serde default（存量规则零变化）。
fn default_autopilot_mode() -> String {
    AUTOPILOT_MODE_LIVE.to_string()
}

/// mode 值校验（create/update 双闸共用；空串 = live 归一，未知值拒绝）。
pub fn validate_autopilot_mode(mode: &str) -> Result<String, String> {
    match mode.trim() {
        "" => Ok(AUTOPILOT_MODE_LIVE.to_string()),
        m if m == AUTOPILOT_MODE_LIVE => Ok(AUTOPILOT_MODE_LIVE.to_string()),
        m if m == AUTOPILOT_MODE_SHADOW => Ok(AUTOPILOT_MODE_SHADOW.to_string()),
        other => Err(format!(
            "autopilot mode 需要 \"{AUTOPILOT_MODE_LIVE}\"|\"{AUTOPILOT_MODE_SHADOW}\"，收到 \"{other}\""
        )),
    }
}

// ---------------------------------------------------------------------------
// 讨论频道 + 任务资产（Swarm 本地保存；impl-plan）
// ---------------------------------------------------------------------------

/// 频道消息种类词表（`channel_message.mtype`）。 先立两态； 信封落地
/// 后按需扩展（如 delivery 回流卡）。
pub mod channel_message_type {
    /// 普通发言。
    pub const TEXT: &str = "text";
    /// 系统消息（入频道/离队/派发播报等，不参与发言配额）。
    pub const SYSTEM: &str = "system";
}

/// 线程种类词表（`seq_ledger.thread_kind` + wake 包 `thread.kind` 共用）。
pub mod thread_kind {
    /// issue 评论线程。
    pub const ISSUE: &str = "issue";
    /// 频道讨论线程。
    pub const CHANNEL: &str = "channel";
}

/// 讨论频道（表 `channel`；`name` 含 `#` 前缀存储，如 `#dev`）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub topic: String,
    pub created_at: i64,
}

/// 建频道的输入（[`crate::BoardStore:create_channel`]）。
#[derive(Debug, Clone)]
pub struct NewChannel {
    pub name: String,
    pub topic: String,
}

/// 频道成员（表 `channel_member`；成员是多态 Actor——agent 节点 / admin 人）。
/// `last_seen_message_id` 是未读游标：`list_messages(after_id)` 补拉与前端
/// 翻页共用同一语义。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelMember {
    pub channel_id: i64,
    pub member: crate::assignment::Actor,
    pub last_seen_message_id: i64,
}

/// 频道消息（表 `channel_message`；`parent_id` 引用同频道内消息构成线程）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelMessage {
    pub id: i64,
    pub channel_id: i64,
    pub sender: crate::assignment::Actor,
    pub content: String,
    pub parent_id: Option<i64>,
    /// [`channel_message_type`] 词表。
    pub mtype: String,
    pub created_at: i64,
}

/// 发频道消息的输入（[`crate::BoardStore:append_channel_message`]）。
#[derive(Debug, Clone)]
pub struct NewChannelMessage {
    pub channel_id: i64,
    pub sender: crate::assignment::Actor,
    pub content: String,
    pub parent_id: Option<i64>,
    /// 空串 = [`channel_message_type::TEXT`]。
    pub mtype: String,
}

/// 任务资产索引（表 `asset`；实体落 `workspace/board/assets/<ref>`，库只存
/// 索引——引用与内容分离的本地侧，impl-plan）。不设 TTL：随所属 issue
/// 删除级联清理（多 issue 引用同一 ref 时最后一个解绑才删盘）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoardAsset {
    pub id: i64,
    /// 全局唯一引用名（如 `a3f9…-report.md`； 信封里以 asset_ref 流转）。
    pub ref_name: String,
    pub origin_issue: Option<i64>,
    pub sha256: String,
    pub size: i64,
    /// 相对 `workspace/board/assets/` 的落盘文件名（= ref）。
    pub path: String,
    pub created_at: i64,
}

/// 登记资产的输入（[`crate::BoardStore:register_asset`]；实体文件由调用方
/// 先落盘，store 只记索引）。
#[derive(Debug, Clone)]
pub struct NewAsset {
    pub ref_name: String,
    pub origin_issue: Option<i64>,
    pub sha256: String,
    pub size: i64,
}

/// seq 台账行（表 `seq_ledger`；`board.sync` 补拉返回的统一形状——issue
/// 评论与频道消息混排，`seq` 是 master 单调序号，worker 的补拉游标）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerEntry {
    pub seq: i64,
    /// [`thread_kind::ISSUE`] 或 [`thread_kind::CHANNEL`]。
    pub thread_kind: String,
    /// issue id 或 channel id。
    pub thread_id: i64,
    /// 原表行 id（comment.id / channel_message.id）。
    pub message_id: i64,
    pub sender_type: String,
    pub sender_id: String,
    pub content: String,
    pub parent_id: Option<i64>,
    /// comment.ctype / channel_message.mtype。
    pub kind_tag: String,
    pub created_at: i64,
}

/// `post_discussion_envelope` 的结果：首次落库（is_new=true，携带新行 id
/// 与 seq）或重复请求（is_new=false，response 为缓存的首响 JSON）。
#[derive(Debug, Clone)]
pub struct PostedMessage {
    pub is_new: bool,
    pub message_id: i64,
    pub seq: i64,
    pub response: serde_json::Value,
}

/// 经验类别词表（team_memory.category 约定值；.5）。落库不强制
/// 枚举校验（评审 agent 提示词引导四类，词表外值诚实原样存）。
pub mod team_memory_category {
    /// 坑——踩过的雷/避免的做法。
    pub const PITFALL: &str = "pitfall";
    /// 模式——可复用的做法。
    pub const PATTERN: &str = "pattern";
    /// 约定——团队约定/规范。
    pub const CONVENTION: &str = "convention";
    /// 偏好——工具/风格偏好。
    pub const PREFERENCE: &str = "preference";
}

/// 团队经验条目（表 `team_memory`；.5 schema）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TeamMemoryEntry {
    pub id: i64,
    /// [`team_memory_category`] 词表。
    pub category: String,
    /// 检索键：模块/技术栈标签（注入匹配按 scope 与任务文本比对）。
    pub scope: String,
    pub content: String,
    /// 来源任务编号（如）。
    pub source: String,
    /// 蒸馏者（验收 agent 节点 id）。
    pub author: String,
    /// 被注入次数（衰减依据；重复经验并进旧条目时 +1）。
    pub use_count: i64,
    /// 软删标记（旧经验与实际冲突 → 标记不覆盖）。
    pub deprecated: bool,
    /// Unix 秒。
    pub created_at: i64,
}

/// 新经验条目（验收蒸馏路径的唯一写入口参数）。
#[derive(Debug, Clone)]
pub struct NewTeamMemory {
    pub category: String,
    pub scope: String,
    pub content: String,
    pub source: String,
    pub author: String,
}

#[cfg(test)]
mod cov_tests;
#[cfg(test)]
mod tests;
