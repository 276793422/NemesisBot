//! Swarm：验收 agent 编排层（批作业三态处置）。
//!
//! 触发链：worker 回报 → `write_back_board_dispatch` 写回 in_review →
//! gateway 回调闭包 [`spawn_board_review`] → 本模块 [`review_issue`]：
//! 裸提示词 detached LLM 调用（与 planner 同款，`system_prompt` 换
//! 验收提示词 + 注入防线声明）→ `parse_review` 失败回灌重试 ≤2 次 →
//! 仍失败按 UNSURE 诚实处置（**评审自身失败恒转人工，不重派 worker**——
//! 评审没出结论就没有重派授权，unlimited_mode 只治理真实 verdict，见
//! [`review_issue`] Err 分支注记）→ 三态分派（PASS/FAIL/UNSURE）。
//!
//! 全自动流转 追加：
//! - ** 执行型验收**：最近派发 worker 是本机节点且
//!   `board.review.max_turns > 1` 时，评审 agent 以只读工具白名单最多
//!   N 轮取证再下结论（远端 worker 一律纯文本，防读错工作区）。
//! - ** 自检取证**：`board.review.selfcheck=true` 时评审可向 worker
//!   发一轮取证请求（`need_evidence`），回报后触发二段验收（证据只带
//!   一程，防取证-评审乒乓）。
//! - ** 换节点重派**：同一 worker 连续 ≥2 次派发仍 FAIL → 换历史
//!   未用过的次优匹配节点。
//! - ** 重派决策量化**（nanobot 指纹晋升思想移植）：
//!   worker × 任务类型历史成功率指纹三档（prefer/avoid/neutral，纯逻辑
//!   在 `nemesis_board::fingerprint`，记账落 board.db v16 两表、task_id
//!   幂等）。`board.fingerprint_weighting=true`（默认 false = 决策表现状
//!   行为字节等价）时换节点候选在匹配器序之上稳定加权；换节点评论带档
//!   位留痕（「（档位 prefer，成功率 5/6）」）。记账与开关解耦——评审
//!   定案（PASS/FAIL）照常入账，灰度期攒数据，开闸即有历史。
//! - ** 预算保险丝**：`board.budget` 三维（子单数/累计派发/墙钟）任一
//!   超限即停自动重派转人工；unlimited_mode 下降级为 WARN 继续。
//! - ** 项目收口**：`board.review.auto_close_project=true` 时项目下全部
//!   顶层父单 done → 汇总验收，PASS → completed；FAIL/UNSURE → 缺口
//!   评论 + completed 回滚 in_progress（不自动重开父单）。
//!
//! 纯逻辑（提示词/schema/解析）真相源在 `nemesis-board::review`；本模块
//! 只做装配与副作用（评论/状态转移/重派/配置读取）。
//!
//! 并发验收安全性（风险项）：多 issue 同时 in_review 会并发 spawn
//! 多个 review 任务，但 `run_detached` 每次调用建临时 instance（会话
//! `subagent:board-review:{uuid}` 互不相干），共享件均为 `Arc`，无串行
//! 队列必要。评审失败不回滚、不炸写回——评论留痕 + warn，人工兜底。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use nemesis_board::assignment::Actor;
use nemesis_board::models::{CommentType, IssueStatus, NewComment};
use tracing::{debug, error, info, warn};

/// 收口评审（父单/项目）worker_report 内联字节上限（
/// 三轮误升级根修）：收口 worker_report 携带实物证据段（清单 201 条 +
/// 最终报告全文节选，实测 ~70-120KB），默认 `MAX_INLINE_BYTES` 64KB 上限
/// 把节选段整体截掉——三轮评审 prompt 全部停在文件清单中段，「看不到最终
/// 报告正文」的 UNSURE 全是对残缺输入的诚实反应。证据段自带 60K 字符节选
/// 总预算，此处 4 倍只是不再二次截伤；子单评审仍走默认 64KB。
const CLOSURE_WORKER_REPORT_MAX_BYTES: usize = 256 * 1024;

/// estop 停车种类（释放 watcher 据此选复评入口； 加 Project 维度）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParkedKind {
    /// 子单评审。
    Issue,
    /// 父单收口评审。
    Parent,
    /// 项目收口评审。
    Project,
}

/// 验收依赖集（gateway 回调闭包构造，一次快照传入）。
/// `Clone` 供父单收口钩子每次触发时从 Arc 快照派生。
#[derive(Clone)]
pub struct BoardReviewDeps {
    pub store: Arc<nemesis_board::BoardStore>,
    /// workspace 根（/ 锚点检查的路径安全边界：[CHECK] file: 类锚点
    /// 只在此子树内解析；后续带工具实核/资产读取的评审形态同用此根）。
    pub workspace: PathBuf,
    /// home 根（config.json 读取：auto_review/auto_accept/max_redispatch
    /// 每次评审时从盘上现读，改配置即时生效，不取装配期快照）。
    pub home: PathBuf,
    /// 主 AgentLoop 动态解析槽（gateway 级共享槽 Arc；写者唯一 =
    /// AgentLoopServiceAdapter）。评审任务运行时读——agent stop→start 重建
    /// 后自然跟随（BUG-20261108-012：此前 OnceLock 静态绑定初始 loop）。
    pub moderator_loop: Arc<parking_lot::RwLock<Option<Arc<nemesis_agent::r#loop::AgentLoop>>>>,
    pub cluster: Arc<nemesis_cluster::cluster::Cluster>,
    /// 急停句柄（全自动流转 / 保险丝）：挂起时评审冻结（不跑 LLM、
    /// 不重派、不收口），issue 停车登记；release 后由
    /// [`spawn_estop_resume_watcher`] 复评恢复。与 SharedResources 共享
    /// 同一 Arc——CLI/托盘/Dashboard/WSAPI 四入口全部生效。
    pub estop: Arc<nemesis_agent::estop::EstopState>,
    /// estop 冻结停车的 (种类, issue_id) 队列（gateway 装配一份共享，
    /// 评审闭包与 release watcher 共用）。std Mutex：持锁不跨 await。
    pub estop_parked: Arc<std::sync::Mutex<Vec<(ParkedKind, i64)>>>,
    /// 自检取证路由表（gateway 装配一份，callback 闭包与评审任务
    /// 共享）：task_id → issue_id。
    pub selfcheck: SelfcheckRegistry,
    /// d：评审结论 → 交付账本闭环的审计插件槽（cfg security；None =
    /// headless/测试形态）。槽位惰性 get()——装配期安全插件可能还没 set，
    /// 评审运行时才解析。
    #[cfg(feature = "security")]
    pub security_slot:
        Option<Arc<std::sync::OnceLock<Option<Arc<nemesis_security::pipeline::SecurityPlugin>>>>>,
}

/// 自检在途注册表：task_id → issue_id。selfcheck 派发**不写**
/// `issue_dispatch`（那是正式派发的写回路由键，写了会被 Route 0 当交付
/// 写回推进状态），callback 回来时凭本表识别这是取证任务并路由到二段
/// 验收——peer_chat_callback payload 只有 task_id，没有 chat_id，字面
/// marker 路由不可行。std Mutex：持锁不跨 await。
///
/// 内存表是 gateway 进程态，重启清零后 worker 取证回调
/// 路由 miss → master 仍回 `Ok("received")` → worker 销毁唯一持久副本 →
/// 证据蒸发、单据永久卡死 in_review。修复 = 路由键同步落 board store
/// `selfcheck_pending` 表（[`Self:with_store`]）：`take` 内存 miss 时以
/// 持久行兜底路由；`status`/`response` 烧入后二段完成前再崩由
/// [`selfcheck:sweep_selfcheck_pending`] 恢复重放。store 缺席（无 board
/// feature 的测试形态）= 纯内存旧行为。
#[derive(Clone, Default)]
pub struct SelfcheckRegistry {
    inner: Arc<std::sync::Mutex<HashMap<String, i64>>>,
    /// 路由持久层（None = 纯内存；生产装配带 board store）。
    store: Option<Arc<nemesis_board::BoardStore>>,
    /// 本进程内二段验收在途集合（task_id）：sweep 恢复臂的重放判据——
    /// 在途不重放（评审进行中防双跑）；进程重启后集合清零，残留行自然
    /// 重放。std Mutex：持锁不跨 await。
    inflight_reviews: Arc<std::sync::Mutex<HashSet<String>>>,
}

impl SelfcheckRegistry {
    /// 纯内存形态（后仅测试使用——生产装配走
    /// [`Self:with_store`] 挂持久层）。
    #[cfg(test)]
    pub fn new() -> Self {
        Self::default()
    }

    /// 生产装配形态（gateway 持 board store 时）。
    pub fn with_store(store: Option<Arc<nemesis_board::BoardStore>>) -> Self {
        Self {
            store,
            ..Self::default()
        }
    }

    pub(crate) fn register(&self, task_id: String, issue_id: i64, worker_id: &str) {
        self.inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(task_id.clone(), issue_id);
        // 路由键持久化：写失败 warn 降级（内存表照常 = 旧行为，本进程内
        // 路由不受影响；重启恢复面收窄但不阻断派发）。
        if let Some(store) = self.store.as_ref()
            && let Err(e) = store.register_selfcheck_pending(&task_id, issue_id, worker_id)
        {
            warn!("[BoardReview] selfcheck 路由持久化失败（task {task_id}）：{e}");
        }
    }

    pub(crate) fn take(&self, task_id: &str) -> Option<i64> {
        let memory_hit = self
            .inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(task_id);
        if memory_hit.is_some() {
            return memory_hit;
        }
        // 重启恢复：内存表清零后持久行兜底路由。只认未
        // 路由行（status IS NULL，store 层把关）——已路由行的重复回调不
        // 二次触发二段验收。
        let durable = self
            .store
            .as_ref()
            .and_then(|s| s.get_pending_selfcheck_route(task_id).ok())
            .flatten();
        if durable.is_some() {
            warn!(
                "[BoardReview] selfcheck 路由内存表 miss，持久层兜底命中（task {task_id}，gateway 重启恢复）"
            );
        }
        durable
    }

    /// 同 issue 是否已有在途取证（防重入：重派循环里评审反复挂起会重复
    /// 发取证任务）。内存表 ∪ 持久行（重启后内存表清零，持久行兜住跨
    /// 重启重入——已路由待二段的行同样算在途）。
    pub(crate) fn has_inflight(&self, issue_id: i64) -> bool {
        let memory_hit = self
            .inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .any(|&v| v == issue_id);
        memory_hit
            || self
                .store
                .as_ref()
                .and_then(|s| s.has_selfcheck_pending_for_issue(issue_id).ok())
                .unwrap_or(false)
    }

    /// 二段验收在途标记（本进程；sweep 恢复臂重放判据）。
    pub(crate) fn mark_review_started(&self, task_id: &str) {
        self.inner_mark_review(task_id, true);
    }

    pub(crate) fn mark_review_finished(&self, task_id: &str) {
        self.inner_mark_review(task_id, false);
    }

    fn inner_mark_review(&self, task_id: &str, started: bool) {
        let mut set = self
            .inflight_reviews
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if started {
            set.insert(task_id.to_string());
        } else {
            set.remove(task_id);
        }
    }

    pub(crate) fn has_inflight_review(&self, task_id: &str) -> bool {
        self.inflight_reviews
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains(task_id)
    }
}

/// （机制加固）：锚点主根统一定义。锚点双检（子单验收/
/// 父单收口/项目收口）、评审取证资产取回、档案导出等所有「工作区根」
/// 消费点一律经此取根——根的来源收敛在装配点一处
/// （gateway `post_agent.rs` 用 `SharedResources::workspace_dir()` 填
/// `deps.workspace`，显式覆盖跟随、canonical 布局回退），**禁止**各消费点
/// 再从 `home` 手拼 `join("workspace")`（实证：部署形态漂移下手拼根
/// 指向不存在目录，九锚点全 os error 2 假红）。项目目录 fallback（第二根）
/// 仍由 `run_anchors_dual*` 内部处理，不在本函数职责内。
pub(crate) fn resolve_anchor_root(deps: &BoardReviewDeps) -> &std::path::Path {
    &deps.workspace
}

/// 执行一次完整验收。返回 `Ok(false)` = 未评审即跳过（配置关/状态已变/
/// loop 未就绪/estop 冻结），`Ok(true)` = 评审闭环完成（含 取证挂起）。
async fn review_issue(
    deps: &BoardReviewDeps,
    issue_id: i64,
    ctx: ReviewCtx,
) -> Result<bool, String> {
    // 同因熔断账本（随函数全程使用：estop 停车清账 / 非重派终态清账 /
    // Redispatch 臂记账判定）。D：账本移 nemesis-board 公开（web 层
    // 手动重派清账同源同一 JSON）。
    let mut fail_streaks = nemesis_board::fail_streak::FailStreakLedger::load(&deps.workspace);
    // estop 保险丝检查点 ①：挂起即冻结（不读配置不跑 LLM），停车等释放。
    // 停车清账：急停是世界被外力改变，release 复评时旧失败计数作废。
    if estop_fuse_engaged(deps, issue_id, ParkedKind::Issue) {
        fail_streaks.clear(issue_id);
        return Ok(false);
    }
    let cfg = load_board_flags(&deps.home)?;
    if !cfg.auto_review {
        debug!("[BoardReview] issue {issue_id} 跳过：board.auto_review=false");
        return Ok(false);
    }

    let store = &deps.store;
    let issue = store.get_issue(issue_id)?;
    // 只有 in_review 才评：write_back 与人工操作竞态时（人工已改状态）
    // 让位人工，不覆盖。
    if issue.status != IssueStatus::InReview {
        info!(
            "[BoardReview] issue {issue_id} 状态已是 {}（非 in_review），跳过自动验收",
            issue.status
        );
        return Ok(false);
    }

    // U-B：LLM 端点不可用封存闸——评审挂起（不烧 重评链）。
    // 挂起即写中断档案，恢复探针解封后逐单续评（review pending）。手动
    // review.retrigger 期间撞封存也走此路：诚实挂起 + 自动续做，意图不丢。
    if nemesis_web::handlers::board_unavailable::frozen_now() {
        nemesis_web::handlers::board_unavailable::write_interruption(
            &deps.workspace,
            &deps.store,
            &issue,
            "review",
            "评审挂起：LLM 端点不可用封存中，恢复后探针自动续评".to_string(),
            serde_json::json!({ "attempt": ctx.retry }),
        );
        warn!("[BoardReview] issue {issue_id} 评审挂起：LLM 端点不可用封存中（恢复后自动续评）");
        return Ok(false);
    }

    // 父单分流——review.retrigger 对收口失败转人工
    // （保持 in_review）的父单同样一键重评。review_parent_issue 是收口汇总
    // 评审（输入=父单目标+全子单结论），review_issue 的子单式评审对父单无
    // 意义（父单自身无交付汇报，跑出来的是无依据结论）。分流在状态闸之后：
    // review_parent_issue 内部自带 estop/旗标/状态/子树四重守卫，重复读
    // config 幂等无害；所有 review_issue 调用路径（retrigger/write_back/
    // 重放 sweep/estop 复评）自动获得正确分流。
    if !store.list_children(issue_id)?.is_empty() {
        info!("[BoardReview] issue {issue_id} 是父单（有子单），分流收口汇总评审");
        return review_parent_issue(deps, issue_id).await;
    }

    // （职能→评审预设查表）：评审参数按本单职能解析（职能预设 > 全局
    // board.review.* > 内置默认；无职能单 = 全局值，行为不变）。后续消费点
    // 只读解析结果，不再直读 cfg.review 同名字段。
    let eff_review = cfg
        .review
        .resolve_for_profession(issue.required_profession.as_deref());

    // ---- 组装评审输入：worker 汇报 + 线程争议点 ----
    let comments = store.list_comments(issue_id)?;
    let dispatches = store.list_dispatches(issue_id)?;
    // /evidence-on-demand：评审工具模式（纯函数）。计算提到最前——变更集
    // 注入块的「未附内容」注记按模式分流取证指引（EvidenceOnly/ReadOnly 提
    // 工具通路，NoTools 保持历史文案）；预算块与 run_review_panel 消费同值。
    let tool_mode = pick_review_tool_mode(
        dispatches.last().map(|d| d.worker_id.as_str()),
        deps.cluster.node_id(),
        eff_review.max_turns,
    );
    // worker 汇报 = **最新**一次交付（重派轮次取新汇报，不拿首轮旧账）；
    // 无结构化汇报时退「最新派发执行者」的最近 agent 文本评论（诚实降级
    // 路径），再没有就空串（评审 agent 见「未提供」自判）。
    // 任务卡回显不是交付（UAT 双层实证）：回显模型把派发
    // 任务卡原样抄回当回复，文本里带着验收标准与上轮失败明细引文——锚点
    // 行原文可剥离（anchor.rs 自指防御），但引文散文里的裸词仍会二阶自
    // 命中。凡内容含任务卡头的评论一律不作交付输入（Delivery 与降级
    // agent 评论同滤），回显 worker 诚实走 FAIL → 重派/预算/转人工漏斗。
    let is_card_echo =
        |c: &nemesis_board::Comment| c.content.contains(nemesis_board::TASK_CARD_HEADER);
    let delivery = comments
        .iter()
        .rev()
        .find(|c| c.ctype == CommentType::Delivery && !is_card_echo(c))
        .cloned();
    // 降级交付的作者判定（合并合流：两路独立根修取交集语义）
    // ——验收流程评论（挂起/FAIL/换节点/转人工，author=评审节点）不得冒
    // 充 worker 汇报抢交付槽：
    // 1) 有派发记录：作者必须 = 最新派发行 worker_id（write_back 落库即
    //    Actor::agent(&disp.worker_id)，逐字同源）——评审者暂缓/FAIL 意见
    //    （子单 22 三连败 + /T-MRG in_review 停车，同根）被身份
    //    闸排除；本机单 worker==本节点 id 也天然正确（dispatch 到本节点
    //    是合法形态，重平衡事故面三连全是本机单）。
    // 2) 无派发记录（手工单）：宽松回退但排除评审者自身署名（无派发行
    //    可依时只能靠身份排除；评审者 ⏳/FAIL 意见同样不是 worker 汇报）。
    let last_worker_id = dispatches.last().map(|d| d.worker_id.clone());
    let self_node_id = deps.cluster.node_id();
    let (delivery_id, worker_report) = match &delivery {
        Some(c) => (Some(c.id), c.content.clone()),
        None => {
            let latest = comments.iter().rev().find(|c| {
                c.author.kind == "agent"
                    && c.ctype == CommentType::Comment
                    && !is_card_echo(c)
                    && match last_worker_id.as_deref() {
                        Some(w) => c.author.id == w,
                        None => c.author.id != self_node_id,
                    }
            });
            match latest {
                Some(c) => (Some(c.id), c.content.clone()),
                None => (None, String::new()),
            }
        }
    };
    // 线程 = 除最新交付汇报外的评论，滤掉 status_change/system 机械评论
    // （评审上一轮自己的 FAIL 意见 + 更早轮次的交付汇报保留在场——重派
    // 轮次的完整上下文）。任务卡回显同滤（系统性检查，与交付
    // 槽位过滤同根：回显文本带验收标准原文与上轮失败明细，进线程同样是
    // 二阶自命中污染源——交付槽位挡了，线程侧漏进来等于没挡）。
    let thread: Vec<(String, String)> = comments
        .iter()
        .filter(|c| Some(c.id) != delivery_id)
        .filter(|c| !matches!(c.ctype, CommentType::StatusChange | CommentType::System))
        .filter(|c| !is_card_echo(c))
        .map(|c| {
            (
                format!("{} {}", c.author.kind, c.author.id),
                c.content.clone(),
            )
        })
        .collect();

    // 跨轮缺口清账：最近一份本节点评审落下的缺口清单（❌ 重派评论的
    // 【评审差距】标记段）= 本轮待清账缺口。作者限定评审节点——worker
    // 汇报/讨论线程里出现同形标记不采信（防伪造清单注入「上一轮评审已
    // 判定」语境）。无标记（首评/旧格式评论）= 空清单零注入。
    let prior_gaps: Vec<nemesis_board::LedgerItem> = comments
        .iter()
        .rev()
        .filter(|c| c.author.kind == "agent" && c.author.id == self_node_id)
        .find_map(|c| {
            let ledger = nemesis_board::parse_gap_ledger(&c.content);
            (!ledger.is_empty()).then_some(ledger)
        })
        .unwrap_or_default();

    let round = dispatches.len().saturating_sub(1) as u64;

    // 转人工评论的 @人对象 = 创建者是人（admin）时 @ 创建者（impl-plan
    // 「UNSURE @人 请求裁决」）；agent/system 创建者无人类可 @，退化
    // 为普通评论。
    let human_mention = if issue.creator.kind == "admin" {
        format!("@{} ", issue.creator.id)
    } else {
        String::new()
    };
    // 评审评论作者（解析失败分支与三态分派共用）。
    let reviewer = Actor::agent(deps.cluster.node_id());

    // 子单评审内联上限 = 交付上限 + 8KB 余量（资产引用束根修）：
    // >64KB 交付写回形态是「64KB 头 + 尾部全文引用束注记」，默认 64KB 截断
    // 恰好砍掉尾部注记——文本评审从此看不到「交付附了图像资产」的事实
    // （vision 臂不受影响，取数走原始评论），可能误判「汇报未附证据」。
    // 头部本身就是交付前 64KB，72KB 上限让注记完整在场，评审可见文本与
    // 实际交付对齐。
    let mut prompt = nemesis_board::build_review_user_prompt_with_limits(
        &issue.number,
        &issue.title,
        &issue.description,
        issue.acceptance_criteria.as_deref(),
        &worker_report,
        &thread,
        nemesis_board::report::MAX_INLINE_BYTES + 8 * 1024,
        &prior_gaps,
    );
    // 父单口径注入（误升级根修）：返工单/子单被派的差距
    // 常引用父单验收标准的语义（如某条 AC「以台账+待人工签核清单为口径，
    // 不阻塞自动验收」），但该口径行只存在于父单 acceptance_criteria——
    // 子单评审 prompt 此前看不到，评审 LLM 只能按差距清单的字面（或 worker
    // 自我结论）裁决，把口径已豁免的人工后续事项误判成 FAIL（再被
    // REDISPATCH_LOOP 标记放大成升级人工）。注入父单验收标准作为**待审
    // 参照数据**（非指令），让「按父单口径裁决」有据可依。父单缺失/读取
    // 失败/AC 为空 = 诚实跳过（孤儿子单照常评审）。
    if let Some(parent_id) = issue.parent_issue_id
        && let Ok(parent) = store.get_issue(parent_id)
        && let Some(section) = render_parent_ac_section(&parent)
    {
        prompt.push_str(&section);
    }
    // 二段评审：worker 取证回报作为待审数据拼入（证据是 worker LLM
    // 的输出，同样受数据/指令分离声明约束）。
    if let Some(ev) = ctx
        .evidence
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        prompt.push_str(&format!(
            "\n## 自检证据（执行 worker 取证回报）\n{}\n",
            nemesis_utils::truncate(ev, 8000)
        ));
    }
    // F- 根修（UAT 实证）：变更集合并结果作为**系统客观数据**注入。
    // worker 自报的交付路径是一面之词——远程 worker 经集群 exec 在沙箱目录
    // 产文件时，汇报里的沙箱绝对路径让无工具的评审 LLM 误判「文件未落盘」
    // → 假阳 UNSURE 转人工（实际变更集早已合并进项目工作区）。系统合并
    // commit 的实际 diff 才是「产物是否进入项目工作区」的证据真相源。
    // 无合并记录（非文件型任务/本机 worker）或档案目录缺失 = 诚实跳过。
    if let Some(project_id) = issue.project_id
        && let Ok(project) = store.get_project(project_id)
        && let Ok(root) = project_archive_root(project.directory.as_deref())
    {
        let merged_commit = store.list_activity(issue_id).ok().and_then(|acts| {
            acts.iter()
                .rev()
                .find(|a| a.action == crate::board_archive_ingest::ACTION_MERGED)
                .and_then(|a| a.details.as_deref())
                .and_then(|d| serde_json::from_str::<serde_json::Value>(d).ok())
                .and_then(|v| v.get("commit").and_then(|c| c.as_str()).map(str::to_string))
        });
        if let Some(commit_oid) = merged_commit {
            match nemesis_board::commit_changed_files(&root, &commit_oid) {
                Ok(files) if !files.is_empty() => {
                    // F-（真机）：清单只证实「落盘」，
                    // 评审员核对不了「内容」——变更集明明带全文却看不到，
                    // 对「产物内容是否达标」只能 UNSURE 转人工。注入 commit
                    // 内实际 blob 内容节选（非 worker 自述）：小文本全文
                    // 截断呈现，二进制/超限/条数超预算的诚实注记跳过。
                    // 真机实证加宽：2000 字符节选把 PRD 截
                    // 在「核心玩法循环」开头，评审员以「未展示功能需求/
                    // 非功能需求/逐条验收标准，无法对照验收标准确认」确定性
                    // UNSURE（阈值 2 轮即停车）——节选截断本身就是假证据不足。
                    // evidence-on-demand 重构：注入层降级为
                    // **快路径**（省轮次的优化，不再承担正确性）——单文件
                    // 12000 字符 × 最多 4 个文件（总量上界 ~48K 字符，防多
                    // 文件交付撑爆 prompt）；快路径没覆盖到的内容，评审员用
                    // board_evidence_read 工具按档案围栏自取（模式分流见
                    // render_unattached_note）。正确性不再由任意硬编码预算
                    // 决定（用户）。
                    const BLOB_READ_MAX_BYTES: usize = 64 * 1024;
                    const BLOB_SNIPPET_CHARS: usize = 12_000;
                    const BLOB_CONTENT_MAX_FILES: usize = 4;
                    let mut listing = String::new();
                    let mut content_budget = BLOB_CONTENT_MAX_FILES;
                    let mut with_content = 0usize;
                    for (path, status) in &files {
                        listing.push_str(&format!("- {path}（{status}）\n"));
                        let allow = if content_budget == 0 {
                            0
                        } else {
                            BLOB_SNIPPET_CHARS
                        };
                        if allow > 0
                            && let Ok(Some(text)) = nemesis_board::commit_blob_text(
                                &root,
                                &commit_oid,
                                path,
                                BLOB_READ_MAX_BYTES,
                            )
                        {
                            let snippet = nemesis_utils::truncate(text.trim(), allow);
                            listing.push_str(&format!(
                                "  内容节选（commit 内 blob 原文）：\n```text\n{snippet}\n```\n"
                            ));
                            content_budget -= 1;
                            with_content += 1;
                        }
                    }
                    if files.len() > with_content {
                        listing.push_str(&render_unattached_note(
                            files.len() - with_content,
                            tool_mode != ReviewToolMode::NoTools,
                        ));
                    }
                    prompt.push_str(&format!(
                        "\n## 变更集合并结果（系统客观数据）\n交付变更集已由系统合并进项目工作区（commit {}）。合并实际落盘的文件清单与内容节选：\n{listing}\n以上为系统合并记录的客观证据（清单=落盘证实；内容节选=commit 内 blob 原文，非 worker 自述）。任务验收标准涉及产物内容时，直接对照节选核对{}，不要因「内容未随报提供」而判证据不足。\n",
                        &commit_oid[..12.min(commit_oid.len())],
                        match tool_mode {
                            ReviewToolMode::NoTools => "",
                            _ => "；节选没覆盖到的文件用 board_evidence_read 取证",
                        }
                    ));
                    // 组件5（件4 评审注入，F- 同族）：变更集携带
                    // `.discipline/` 时追加纪律闭环证据段（None=非纪律任务
                    // 诚实跳过，不拿陈旧产物污染无关任务）。
                    if let Some(disc) = render_discipline_evidence(&root, &commit_oid, &files) {
                        prompt.push_str(&disc);
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    warn!("[BoardReview] issue {issue_id} 合并清单读取失败（跳过证据注入）: {e}")
                }
            }
        }
    }

    // ---- 客观锚点双检：先便宜后昂贵，零 LLM 成本零命令执行 ----
    // acceptance_criteria 的 [CHECK] 行系统先核验：全过 → 摘要拼进 prompt
    // （LLM 仍审语义项）；有 fail → 合成 FAIL 短路（跳过 LLM），照常流经
    // 下方三态决策表（语义零改动：锚点 FAIL = ReviewVerdict::Fail，
    // unlimited/max_redispatch 照旧）。路径形态不安全的锚点行（恶意/越界）
    // 解析期即被拒：不产出锚点、回落语义项 + 系统告警评论（标准
    // 自身的毛病不惩罚执行者，也不让验收炸掉）。
    let (anchors, _semantic_fallback, rejected_anchors) =
        nemesis_board::parse_anchors(issue.acceptance_criteria.as_deref().unwrap_or(""));
    if !rejected_anchors.is_empty() {
        let mut warn_body =
            String::from("⚠ 验收标准含不安全/非法锚点行，已忽略（按普通语义标准评审）：\n");
        for r in &rejected_anchors {
            warn_body.push_str(&format!("- `{}`：{}\n", r.raw, r.reason));
        }
        if let Err(e) = store.add_comment(nemesis_board::NewComment {
            issue_id,
            author: nemesis_board::Actor::system("board"),
            content: warn_body,
            parent_id: None,
            ctype: CommentType::System,
        }) {
            warn!("[BoardReview] issue {issue_id} 锚点告警评论落库失败：{e}");
        }
    }
    // 双根核验（锚点全败根修）：档案管线把 worker 变更
    // 集合并在项目目录（board-projects/<项目名>/，不进主 workspace），主根
    // 只能命中非档案管线的首跑件——挂项目的单子以项目目录做 fallback，
    // 双根 OR 兼容两种交付形态。
    let project_fallback = issue
        .project_id
        .and_then(|pid| store.get_project(pid).ok())
        .and_then(|p| project_archive_root(p.directory.as_deref()).ok());
    // 三态分流：主根与 fallback 根都不可达 = 基础设施故障——不产出
    // 逐锚点 FAIL（那会触发重派；机器上没人能修好「根不存在」，重派循环
    // 烧预算还惩罚不到任何人），转人工 + WARN 审计。重试入口 = 人工核查
    // 环境后 `board.review.retrigger`。
    if let Err(root_err) = nemesis_board::validate_anchor_roots(
        &anchors,
        resolve_anchor_root(deps),
        project_fallback.as_deref(),
    ) {
        warn!(
            "[BoardReview] issue {issue_id} 锚点根不可达（基础设施故障）→ 转人工，不产出锚点 FAIL：{root_err}"
        );
        let handoff_note = park_with_handoff(
            deps,
            &issue.number,
            issue_id,
            nemesis_board::handoff::HandoffSpec {
                stop_kind: "infra",
                suggested_action: "retry",
                hint: "锚点主根与项目目录均不可达（基础设施故障，非交付质量问题）：核查工作区/项目档案目录是否被移动或删除，修好后复评".into(),
                resume_entry: "review.retrigger（number=NB-x，环境修复后复评）".into(),
                environment: last_dispatch_env(&dispatches),
                attempted: vec![serde_json::json!({
                    "action": "validate_anchor_roots",
                    "result": root_err.to_string(),
                })],
            },
        );
        let msg = format!(
            "{}🧭 客观锚点检查无法执行：锚点主根与项目目录均不可达（基础设施故障，非交付质量问题）。已停止自动验收，请人工核查工作区环境后用 board.review.retrigger 重试，或直接裁决。\n\n{root_err}{handoff_note}",
            human_mention
        );
        store.add_comment(nemesis_board::NewComment {
            issue_id,
            author: nemesis_board::Actor::system("board-review"),
            content: msg,
            parent_id: None,
            ctype: CommentType::System,
        })?;
        record_auto_decide(
            deps,
            issue_id,
            "escalate_human",
            "indeterminate",
            serde_json::json!({ "reason": "anchor_root_missing", "detail": root_err }),
        );
        return Ok(true);
    }
    // exec 策略闸：默认关闭 = exec 锚点诚实跳过并注记（不进失败集）。
    let exec_policy = if cfg.review.exec_anchors_enabled {
        nemesis_board::ExecAnchorPolicy::Enabled {
            timeout_secs: cfg.review.exec_anchor_timeout_secs,
        }
    } else {
        nemesis_board::ExecAnchorPolicy::Disabled
    };
    let (anchor_results, anchor_skipped) = run_anchors_dual_ext_blocking(
        &anchors,
        resolve_anchor_root(deps),
        project_fallback.as_deref(),
        &worker_report,
        &exec_policy,
    )
    .await;
    // 必需检查契约闸（项 B「必需的必须跑」）：命中规则的
    // 单子若有必需锚点「AC 未声明 / 被跳过 / 未产出结果」→ 强制停车转人工
    //（needs_validation）：不进语义评审、不允许 auto_accept，系统评论列缺
    // 口明细，状态留 in_review。已运行的锚点（过或不过）= 覆盖满足，裁决
    // 归锚点双检既有短路——本闸只封「零证据放行」面。插在双检后、其余评
    // 审动作前：停车时 skip 注记评论 / PASS 评论 / vision / LLM 全不触发。
    let required_gaps = nemesis_board::required_check_gaps(
        &cfg.review.required_checks,
        &issue.required_tags,
        &anchors,
        &anchor_results,
        &anchor_skipped,
    );
    if !required_gaps.is_empty() {
        let gaps_detail = nemesis_board::render_required_check_gaps(&required_gaps);
        warn!(
            "[BoardReview] issue {issue_id} 必需检查契约未满足（{} 项缺口）→ 强制停车转人工，不进语义评审",
            required_gaps.len()
        );
        let handoff_note = park_with_handoff(
            deps,
            &issue.number,
            issue_id,
            nemesis_board::handoff::HandoffSpec {
                stop_kind: "required_checks",
                suggested_action: "manual_check",
                hint: "必需检查未满足（验收标准缺少规则要求的锚点或锚点未执行）：补齐 AC 锚点声明或开启 exec 锚点后复评".into(),
                resume_entry: "review.retrigger（number=NB-x，补齐必需检查后复评）".into(),
                environment: last_dispatch_env(&dispatches),
                attempted: vec![serde_json::json!({
                    "action": "required_check_gaps",
                    "result": gaps_detail,
                })],
            },
        );
        let msg = format!(
            "{}🧭 必需检查契约未满足：本单命中必需检查规则，但以下锚点未覆盖（未声明 / 被跳过 / 未运行）——按规则不得自动验收（不进语义评审、不允许 auto_accept）。请人工补齐后用 board.review.retrigger 复评，或直接裁决。\n\n{gaps_detail}{handoff_note}",
            human_mention
        );
        store.add_comment(nemesis_board::NewComment {
            issue_id,
            author: nemesis_board::Actor::system("board-review"),
            content: msg,
            parent_id: None,
            ctype: CommentType::System,
        })?;
        record_auto_decide(
            deps,
            issue_id,
            "escalate_human",
            "needs_validation",
            serde_json::json!({
                "reason": "required_checks_unmet",
                "gaps": required_gaps
                    .iter()
                    .map(|g| serde_json::json!({
                        "rule": match g {
                            nemesis_board::RequiredCheckGap::NotConfigured { rule_key, .. }
                            | nemesis_board::RequiredCheckGap::Skipped { rule_key, .. }
                            | nemesis_board::RequiredCheckGap::NotRun { rule_key, .. } =>
                                rule_key.clone(),
                        },
                        "check": match g {
                            nemesis_board::RequiredCheckGap::NotConfigured { check_id, .. }
                            | nemesis_board::RequiredCheckGap::Skipped { check_id, .. }
                            | nemesis_board::RequiredCheckGap::NotRun { check_id, .. } =>
                                check_id.clone(),
                        },
                        "kind": match g {
                            nemesis_board::RequiredCheckGap::NotConfigured { .. } =>
                                "not_configured",
                            nemesis_board::RequiredCheckGap::Skipped { .. } => "skipped",
                            nemesis_board::RequiredCheckGap::NotRun { .. } => "not_run",
                        },
                    }))
                    .collect::<Vec<_>>(),
            }),
        );
        return Ok(true);
    }
    // anchor_summary 注记：锚点双检结果合并进最新派
    // 发任务的交付记录（deliveries.payload.anchor_summary）——交付证据自此
    // 自带客观核验摘要，审计/复盘单行可读（无锚点单不注记，payload 不加
    // 空壳键）。best-effort：无交付记录（人工转入/旧流程单）、tasks.db 不
    // 在、合并失败 = 诚实留痕，评审流程不受影响。
    if !anchor_results.is_empty()
        && let (Some(task_id), Some(db)) = (
            dispatches.last().map(|d| d.task_id.clone()),
            deps.cluster.task_db(),
        )
    {
        let summary = serde_json::json!({
            "anchor_summary": {
                "passed": nemesis_board::all_passed(&anchor_results),
                "total": anchor_results.len(),
                "detail": nemesis_board::render_anchor_summary(&anchor_results),
                "checked_at": chrono::Local::now().to_rfc3339(),
            }
        });
        // 键 = `{task_id}:branch`（生产唯一必然在场的交付行——peer_chat 回调
        // 收据 branch_delivery_inner 落账）；裸 task_id = 旧形态兼容回落
        // （人工转入/旧流程单/旧夹具）。裸键直合并 = 根修前
        // 的错法：branch 行命名空间（9e9e06bf）早于 注记（27ba44a1），
        // 注记自上线起恒 miss 恒 WARN。
        let branch_key = format!("{task_id}:branch");
        let merged = db
            .merge_delivery_payload(&branch_key, &summary)
            .or_else(|_| db.merge_delivery_payload(&task_id, &summary));
        match merged {
            Ok(_) => {
                debug!("[BoardReview] issue {issue_id} anchor_summary 已注记进交付 {task_id}")
            }
            Err(e) => warn!(
                "[BoardReview] issue {issue_id} anchor_summary 注记失败（交付 {task_id}）: {e}"
            ),
        }
    }
    // exec 跳过注记：诚实落系统评论 + 注入 prompt（评审员知道命令类验收
    // 未执行，不自行脑补 PASS）。注记自描述原因（开关关闭 / 环境依赖缺失）。
    if !anchor_skipped.is_empty() {
        let note_body = format!(
            "⏭ 以下 exec: 锚点本轮未执行（原因见各条注记）：\n{}",
            anchor_skipped
                .iter()
                .map(|s| format!("- {s}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        if let Err(e) = store.add_comment(nemesis_board::NewComment {
            issue_id,
            author: nemesis_board::Actor::system("board"),
            content: note_body,
            parent_id: None,
            ctype: CommentType::System,
        }) {
            warn!("[BoardReview] issue {issue_id} exec 跳过注记评论落库失败：{e}");
        }
        prompt.push_str(&format!(
            "\n## 客观锚点检查（部分未执行）\n以下 exec: 锚点本轮未执行（开关关闭或环境依赖缺失，见各条注记），其验收项**未经系统核验**，请按语义项人工评判，不得视为已通过：\n{}",
            anchor_skipped
                .iter()
                .map(|s| format!("- {s}"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    // 锚点全过 → 落 PASS 摘要评论（确定性审计痕迹， 断言锚点；语义
    // 评审意见另行落评，两层不混）。失败路径的明细由 FAIL 分支评论承载。
    if !anchor_results.is_empty() && nemesis_board::all_passed(&anchor_results) {
        let pass_body = format!(
            "✅ 客观锚点检查通过（{} 条）：\n{}",
            anchor_results.len(),
            nemesis_board::render_anchor_summary(&anchor_results)
        );
        if let Err(e) = store.add_comment(nemesis_board::NewComment {
            issue_id,
            author: nemesis_board::Actor::system("board"),
            content: pass_body,
            parent_id: None,
            ctype: CommentType::System,
        }) {
            warn!("[BoardReview] issue {issue_id} 锚点 PASS 评论落库失败：{e}");
        }
    }
    // 锚点全过的摘要注入提升到 LLM 评审分支之前：vision 路径与文本
    // 路径共用同一 prompt 基线（此前只在文本分支注入，vision 提前取 prompt
    // 会缺这段上下文）。锚点失败短路分支不消费 prompt，多注入无副作用。
    if !anchor_results.is_empty() && nemesis_board::all_passed(&anchor_results) {
        prompt.push_str(&format!(
            "\n## 客观锚点检查（已通过）\n以下客观验收锚点已由系统核验通过，无需重复核验，请专注评审其余语义项：\n{}",
            nemesis_board::render_anchor_summary(&anchor_results)
        ));
    }
    // 必检计划对账表注入（SKIP≠PASS）：任务文本反引号命令
    // = 必检计划（确定性派生），× delivery.checks 执行证据的**系统对账表**
    // 作为客观数据进 prompt——评审 agent 读表裁决，不再凭 worker 自述
    // 「已测试」放行。开关关闭 / 无计划项 = 诚实跳过（prompt 零注水）。
    if cfg.review.require_check_plan {
        let plan = nemesis_board::derive_check_plan(&[
            &issue.title,
            issue.description.as_str(),
            issue.acceptance_criteria.as_deref().unwrap_or(""),
        ]);
        if !plan.is_empty() {
            let checks = delivery_checks_for(deps, &dispatches);
            let statuses = nemesis_board::reconcile_check_plan(&plan, &checks);
            prompt.push_str(&format!(
                "\n## 必检计划对账（系统客观数据）\n任务文本中的反引号命令 = 本单必检计划；下表是系统把计划与 worker 执行证据（检查类工具运行记录）逐条对账的结果。未执行 ≠ 通过：计划内必检项没跑或失败，交付不完整：\n{}",
                nemesis_board::render_check_plan_reconciliation(&statuses)
            ));
        }
    }
    // 评审分层（gap ②）：config `board.review.tier` → 档位（无法识别安全
    // 回落 Thorough）。提升到 LLM 评审分支之前（vision 与文本两路共用同一
    // system prompt）。先例注入只走 Thorough——fast 档追求精简，参考性
    // 历史数据不进 prompt。
    let review_tier = nemesis_prompts::board::parse_review_tier(&cfg.review.tier);
    // （机制加固）：评审预算感知注入——预算块进 prompt 后
    // vision/文本两路都拿到（锚点失败短路分支不消费 prompt，多注入无副作
    // 用）。tool_mode 已在注入块之前算好（函数前段）。
    prompt.push_str(&render_review_budget_block(tool_mode));
    // 评审分镜（profession-capability-evolution）：按任务职能
    // 选 lens；禁令节文本服务端预取（vision/文本两路共用；NoTools 形态下
    // 评审员没有读文件工具，注入是唯一有牙齿的通路）。多检查员面板（>1）
    // 走 异构三镜，镜 1（constraint）消费同一份预取文本——一并预取。
    // 职能无分镜且单检查员 = 历史形态，零读取。
    let review_lens = nemesis_prompts::board::review_lens_for_profession(
        issue.required_profession.as_deref().unwrap_or(""),
    );
    let lens_reference = if review_lens.is_some() || eff_review.checkers > 1 {
        prefetch_ban_section(resolve_anchor_root(deps), project_fallback.as_deref())
    } else {
        None
    };
    // ---- vision 路径：交付/线程评论带图像资产引用束 → 程序化取回 →
    // base64 image part 进评审裸调用（单跑不走多检查员 panel——panel v1
    // 保持纯文本）。无图 = 零改动走既有文本路径；锚点已 FAIL
    // 时短路优先，vision 不取图不烧 token。无默认模型槽 / 视觉调用硬失败
    // = prompt 注记（图已取回 master 本地，人工可看）按次回落文本评审。
    let anchors_failed = !anchor_results.is_empty() && !nemesis_board::all_passed(&anchor_results);
    let vision_output = if anchors_failed {
        None
    } else {
        let review_images = crate::board_review_assets::collect_review_images(
            resolve_anchor_root(deps),
            &deps.cluster,
            &comments,
        )
        .await;
        if review_images.is_empty() {
            None
        } else {
            let paths_note = review_images
                .iter()
                .map(|i| i.path.display().to_string())
                .collect::<Vec<_>>()
                .join("；");
            match nemesis_providers::default_slot::current() {
                Some((provider, model, _)) => {
                    let mut vision_prompt = prompt.clone();
                    // vision 单跑 = 职能分镜同款（panel v1 保持纯文本）。
                    let vision_system =
                        nemesis_prompts::board::render_review_system_prompt_with_lens(
                            review_tier,
                            review_lens,
                            lens_reference.as_deref(),
                        );
                    match crate::board_review_assets::run_review_llm_vision(
                        provider,
                        &model,
                        &vision_system,
                        &mut vision_prompt,
                        &review_images,
                        &prior_gaps,
                    )
                    .await
                    {
                        Ok(out) => {
                            // 缺口清账机械闸（视觉臂）：视觉 prompt 是基础
                            // prompt 的克隆，清单注入在场——产出却没过对账
                            // 校验 = 视觉评审本次不可靠，按通道不可用处理
                            // 回落文本评审（文本臂有重试闸兜底）。
                            if let Err(e) = nemesis_board::check_gap_closure(&out, &prior_gaps) {
                                warn!(
                                    "[BoardReview] issue {issue_id} vision 评审缺口对账未过 → 回落文本评审：{}",
                                    e.message
                                );
                                prompt.push_str(&format!(
                                    "\n## 交付图像（视觉评审不可用）\n{} 张交付图像已取回 master 本地：{paths_note}。自动视觉评审本次不可用（{}），请按文本证据评审；图像请转人工查看。\n",
                                    review_images.len(),
                                    e.message
                                ));
                                None
                            } else {
                                info!(
                                    "[BoardReview] issue {issue_id} vision 评审出结论（{} 张附图）",
                                    review_images.len()
                                );
                                Some(out)
                            }
                        }
                        Err(e) => {
                            warn!(
                                "[BoardReview] issue {issue_id} vision 评审失败 → 回落文本评审：{e}"
                            );
                            prompt.push_str(&format!(
                                "\n## 交付图像（视觉评审不可用）\n{} 张交付图像已取回 master 本地：{paths_note}。自动视觉评审本次不可用（{e}），请按文本证据评审；图像请转人工查看。\n",
                                review_images.len()
                            ));
                            None
                        }
                    }
                }
                None => {
                    warn!(
                        "[BoardReview] issue {issue_id} 检测到 {} 张交付图像但无默认模型槽 → 文本评审 + 人工看图注记",
                        review_images.len()
                    );
                    prompt.push_str(&format!(
                        "\n## 交付图像（无视觉评审通道）\n{} 张交付图像已取回 master 本地：{paths_note}。当前无可用视觉模型，请按文本证据评审；图像请转人工查看。\n",
                        review_images.len()
                    ));
                    None
                }
            }
        }
    };
    let anchor_short_circuit =
        !anchor_results.is_empty() && !nemesis_board::all_passed(&anchor_results);
    let output = if anchor_short_circuit {
        warn!("[BoardReview] issue {issue_id} 客观锚点检查失败 → 短路 FAIL（跳过 LLM 语义评审）");
        nemesis_board::ReviewOutput {
            verdict: nemesis_board::ReviewVerdict::Fail,
            reasons: anchor_results
                .iter()
                .filter(|r| !r.passed)
                .map(|r| format!("锚点失败: {} — {}", r.anchor.raw, r.detail))
                .collect(),
            // c §③：失败锚点自动成 FAIL 条目（[anchor] 标记），
            // 条目清单渲染/重派反馈消费面与 LLM 评审输出同构。
            criteria: anchor_results
                .iter()
                .filter(|r| !r.passed)
                .map(|r| nemesis_board::CriterionVerdict {
                    criterion: format!("[anchor] {}", r.anchor.raw),
                    verdict: nemesis_board::ReviewVerdict::Fail,
                    evidence: r.detail.clone(),
                })
                .collect(),
            gap: format!(
                "客观锚点检查失败（确定性核验，未进入 AI 语义评审）：\n{}",
                nemesis_board::render_anchor_failures(&anchor_results)
            ),
            experience: None,
            need_evidence: None,
            evidence_request: None,
            unknowns: Vec::new(),
            // 锚点短路无上一轮清单对账面（客观核验，非 LLM 输出）。
            gap_closure: Vec::new(),
        }
    } else if let Some(out) = vision_output {
        out
    } else {
        // 色审议需要评审 agent 在场——只有真正要跑 LLM 的路径才要求 loop
        // 就绪（锚点短路 FAIL 是确定性判定，loop 未就绪也成立，人工兜底
        // 照常能看见明细评论）。
        let Some(agent_loop) = deps.moderator_loop.read().clone() else {
            warn!("[BoardReview] issue {issue_id} 主 agent 未就绪，跳过自动验收（人工兜底）");
            return Ok(false);
        };

        // ---- LLM 评审（首跑 + 回灌重试 ≤2，planner 同款）----
        // 工具模式：本机 worker + max_turns>1 → 只读白名单多轮取证；
        // 其余（远端 worker / 默认配置）纯文本单轮（与历史行为字节等价）。
        // （tool_mode 已在 vision 分支前算好并注入预算块——。）
        // 先例注入只走 Thorough——fast 档追求精简，参考性历史数据不进
        // prompt（review_tier 已提升到评审分支之前，vision/文本共用）。
        if review_tier == nemesis_prompts::board::ReviewTier::Thorough {
            // 判例沉淀（gap ②）：历史「自动决策被人工回滚」记录作**参考
            // 数据**注入（渲染块自带数据非指令护栏）。空记录/查询失败 =
            // 诚实跳过，不注入空节。
            match store.list_rollback_precedents(5) {
                Ok(precedents) if !precedents.is_empty() => {
                    let entries: Vec<nemesis_prompts::board::PrecedentEntry> = precedents
                        .iter()
                        .map(|p| nemesis_prompts::board::PrecedentEntry {
                            issue_ref: format!("{} {}", p.issue_number, p.issue_title),
                            decision: p.decision.clone(),
                            rollback_reason: p.rollback_reason.clone(),
                        })
                        .collect();
                    prompt.push_str(&nemesis_prompts::board::render_precedents_block(&entries));
                }
                Ok(_) => {}
                Err(e) => debug!("[BoardReview] issue {issue_id} 先例查询失败（跳过注入）：{e}"),
            }
        }
        match run_review_panel(
            &agent_loop,
            &prompt,
            tool_mode,
            eff_review.checkers,
            review_tier,
            review_lens,
            lens_reference.as_deref(),
            &prior_gaps,
        )
        .await
        {
            Ok(ok) => {
                // U-B：评审成功 = 端点活证据（连击清零；封存中不可能到这里
                // ——入口闸已拦，此处是成功路径对账）。
                nemesis_web::handlers::board_unavailable::record_llm_success(&cfg.unavailable);
                ok
            }
            Err(last_err) => {
                // U-B：评审失败喂封存判定；恰好触发封存 = 在途工作逐一写
                // 中断档案（本单 + 在途拆解链）。
                let tripped = nemesis_web::handlers::board_unavailable::record_llm_failure(
                    &cfg.unavailable,
                    &last_err,
                );
                if tripped {
                    nemesis_web::handlers::board_unavailable::write_trip_interruptions(
                        Some(deps.workspace.as_path()),
                        &deps.store,
                        Some((
                            &issue,
                            "review",
                            "评审进行中 LLM 端点不可用封存：恢复后探针自动续评".to_string(),
                            serde_json::json!({ "attempt": ctx.retry }),
                        )),
                        &last_err,
                    );
                }
                // S- 迟到评审守卫（评审自身失败路径）：评审期间单据已被
                // 人工改状态 → 不落评论不重派，让位人工（无 verdict 可留痕）。
                if !review_still_relevant(deps, issue_id, "parse_failed", "子单验收") {
                    return Ok(false);
                }
                // 评审自身失败 ≠ verdict——评审侧基础设施
                // 故障（LLM 限流/端点抖动）自愈后重跑同一单据即可，不必打扰
                // 人工。`board.review.failure_retries`（默认 1，0=关回旧行为）
                // 预算内 → 落审计评论 + 延迟自动重评（ctx.retry 记账，重评
                // 再失败预算耗尽才转人工）。重评不重派：与 教训（评审侧
                // 故障不可放大成 worker 重写循环）方向相反，worker 不受影响。
                // estop/状态已变守卫由 review_issue 入口闸天然覆盖（重评走
                // 同一入口）。
                if cfg.review.failure_retries > 0
                    && ctx.retry < cfg.review.failure_retries
                    && !estop_fuse_engaged(deps, issue_id, ParkedKind::Issue)
                {
                    let attempt = ctx.retry + 1;
                    let total = cfg.review.failure_retries;
                    info!(
                        "[BoardReview] issue {issue_id} 评审自身失败（{last_err}），{REVIEW_RETRY_DELAY_SECS}s 后自动重评（第 {attempt}/{total} 次）"
                    );
                    post_review_comment(
                        store,
                        issue_id,
                        &deps.cluster,
                        &format!(
                            "🔁 验收评审自身失败（非交付质量问题），{REVIEW_RETRY_DELAY_SECS}s 后自动重评（第 {attempt}/{total} 次）。\n\n原因：{last_err}"
                        ),
                    )?;
                    spawn_review_retry(deps.clone(), issue_id, attempt);
                    return Ok(true);
                }
                // 评审自身失败（LLM 调用失败 / 取证预算耗尽 / 解析失败）≠
                // 交付质量 FAIL：评审没出结论就没有重派 worker 的授权。
                // unlimited_mode 只治理真实 verdict 的重派（下方决策表），
                // 不放大评审侧基础设施故障——否则评审侧问题（取证预算线
                // 过低、LLM 限流）会变成对 worker 的无限重写循环（
                // web-FPS 实证：评审 2 轮预算耗尽无结论 → 合格交付被
                // 反复重派，worker 已感知循环并尝试升级而通道不可达）。恒按
                // UNSURE 诚实处置转人工，与父单/项目收口的 Err
                // 分支同语义；评审预算不足的正确修复手段是调大
                // board.review.max_turns（0=不限），不是把 worker 送回来
                // 重写。
                // U-A：停车现场落盘（重评预算耗尽 = retry_exhausted 停车）。
                let handoff_note = park_with_handoff(
                    deps,
                    &issue.number,
                    issue_id,
                    nemesis_board::handoff::HandoffSpec {
                        stop_kind: "retry_exhausted",
                        suggested_action: "retry",
                        hint: format!(
                            "验收评审自身失败且重评预算耗尽（failure_retries={}）：评审侧故障不惩罚交付，修好环境（调大 board.review.max_turns（0=不限）/ 检查模型端点）后一键重试",
                            cfg.review.failure_retries
                        ),
                        resume_entry: "review.retrigger（number=NB-x，修好后重试）".into(),
                        environment: last_dispatch_env(&dispatches),
                        attempted: vec![serde_json::json!({
                            "action": "review_retry",
                            "result": format!("重评 {}/{} 次后仍失败：{last_err}", ctx.retry, cfg.review.failure_retries),
                        })],
                    },
                );
                let comment = format!(
                    "{}🤷 验收 agent 无法判定（验收评审自身失败），请人工裁决。评审侧故障不惩罚交付：修好环境（如调大 board.review.max_turns（0=不限）/ 检查模型端点）后，用 review.retrigger（number=NB-N）一键重试验收。\n\n原因：{last_err}{handoff_note}",
                    human_mention
                );
                post_review_comment(store, issue_id, &deps.cluster, &comment)?;
                return Ok(true);
            }
        }
    };

    // S- 迟到评审守卫：LLM 评审期间单据被人工改状态（reopen/取消等）
    // 则结论整体让位——取证挂起与下方任何处置分支（含最危险的迟到重派）
    // 都不再执行。
    if !review_still_relevant(deps, issue_id, output.verdict.as_str(), "子单验收") {
        return Ok(false);
    }

    // ---- 取证挂起：评审需要更多证据 → 向执行 worker 发一轮自检。
    // 挂起在经验落库之前（评审未定案不蒸馏）；锚点短路输出无 need_evidence
    // 槽位，天然不进本分支。二段评审 ctx.allow_selfcheck=false，只带一程。
    // AC 含 re: 交付文本锚点的单**按需强制**自检——re:
    // 锚点核对的是 delivery 文本本体，可被「把标记字符串抄进交付」欺骗
    // （真机：语义二审识破但已烧 2 轮重派预算）。锚点已过 + 语义评审
    // 阶段仍挂一轮取证（要求实际执行并回报证据）是对投机的结构性抑制；
    // 无派发记录（无取证对象）不强制；全局 board.review.selfcheck 只管
    // LLM 主动要证据的常规路径，强制臂绕过它。LLM 未要证据时合成固定请求。
    let anchor_forced = !dispatches.is_empty()
        && nemesis_board::has_content_regex_anchor(
            issue.acceptance_criteria.as_deref().unwrap_or(""),
        );
    if ctx.allow_selfcheck
        && (eff_review.selfcheck || anchor_forced)
        && let Some(request) = nemesis_board::selfcheck_request_text(&output).or_else(|| {
            // 锚点短路输出（确定性 FAIL）不得合成取证请求——1140 行的设计
            // 断言「短路输出无 need_evidence 槽位，天然不进本分支」被下方
            // 强制臂打破：短路 FAIL 被拖进取证悬挂（T30 ET 案例 2026-11-05：
            // ⏳ 验收暂缓取证取代首轮短路 FAIL 处置），重派漏斗被取证轮
            // 挤占，转人工评论丢锚点明细。F6 强制臂语义本就是「锚点已过 +
            // 语义评审仍嫌疑」——短路 = 锚点未过，不属强制臂射程。
            (!anchor_short_circuit && anchor_forced).then(|| {
                format!(
                    "本单验收标准包含 {} 交付文本锚点（存在引用欺骗风险）：请实际执行相关验证并回报可核验证据（真实命令输出、产物文件内容片段），不得引用任务卡原文来满足锚点。",
                    nemesis_board::ANCHOR_PREFIX
                )
            })
        })
    {
        if deps.selfcheck.has_inflight(issue_id) {
            warn!("[BoardReview] issue {issue_id} 已有在途自检取证，跳过重复挂起，按现有结论处置");
        } else if dispatches.last().map(|d| d.worker_id.as_str()) == Some(deps.cluster.node_id()) {
            // 本机 worker：工件就在本机，取证走 执行型评审轮（或人工），
            // 不绕 peer_chat 自发自收。
            info!(
                "[BoardReview] issue {issue_id} worker 为本机节点，取证需求交给执行型验收/人工，不另发自检"
            );
        } else {
            // estop 检查点（发车前）：急停中不发自检，停车等释放。
            if estop_fuse_engaged(deps, issue_id, ParkedKind::Issue) {
                return Ok(false);
            }
            // 取证软件化：执行档案直读优先于向 worker 发 LLM
            // 自检。worker 的执行记录已随交付回灌在项目 records/<编号>/
            // execution/，master 本地直读 = 零 token、零 RPC、零 worker 依赖
            // （上游 LLM 链路故障时评审取证不被卡死）；证据走与 worker 回报
            // 同一条二段评审管线（allow_selfcheck=false，只带一程）。档案
            // 缺失/为空 = 诚实回落 dispatch_selfcheck（旧行为不变）。
            if let Some(evidence) = gather_execution_evidence(deps, &issue) {
                let note = "🔎 执行档案直读取证：评审需要更多证据，已从项目执行档案本地读取 worker 执行记录（免 worker 取证轮），自动进行二段验收。";
                let _ = post_review_comment(store, issue_id, &deps.cluster, note);
                spawn_selfcheck_second_stage(
                    deps.clone(),
                    issue_id,
                    "ok".to_string(),
                    evidence,
                    None,
                );
                info!(
                    "[BoardReview] issue {issue_id} 执行档案直读取证完成，二段验收已发起（免 worker 取证轮）"
                );
                return Ok(true);
            }
            let target_worker = dispatches
                .last()
                .map(|d| d.worker_id.clone())
                .ok_or_else(|| "无历史派发记录，无法确定取证目标".to_string())?;
            match dispatch_selfcheck(deps, &issue, &target_worker, &request).await {
                Ok(task_id) => {
                    deps.selfcheck
                        .register(task_id.clone(), issue_id, &target_worker);
                    let note = format!(
                        "⏳ 验收暂缓（board.review.selfcheck）：评审 agent 需要更多证据，已向执行 worker 发起取证（task {task_id}）。\n\n取证请求：{request}\n\nworker 回报后自动进行二段验收。"
                    );
                    let _ = post_review_comment(store, issue_id, &deps.cluster, &note);
                    info!(
                        "[BoardReview] issue {issue_id} 验收挂起等取证（task {task_id}），二段验收待回报"
                    );
                    return Ok(true);
                }
                Err(e) => {
                    warn!(
                        "[BoardReview] issue {issue_id} 取证派发失败：{e}（按现有结论处置，转人工兜底）"
                    );
                    let _ = post_review_comment(
                        store,
                        issue_id,
                        &deps.cluster,
                        &format!("⛔ 取证派发失败（{e}），验收按现有材料处置或人工介入。"),
                    );
                }
            }
        }
    }

    // ---- 交付证据契约：PASS 必须有交付记录背书。
    // 置位面：issue 级 `requires_evidence`（AC 含 file: 锚点建单时自动置
    // 位，恒生效；checkpoint-git 关的执行体建单时跳过自动置位）∪
    // 全局 `board.review.require_delivery_evidence`（所有的单加码）。
    // 校验对象 = 最新派发任务在 tasks.db deliveries 表的记录。生产真实落
    // 账点 = peer_chat 回调收据的 branch_delivery_inner（幂等键 =
    // `{task_id}:branch`，payload = delivery JSON **直接形态**）——本注释
    // 旧文「A 端 handle_task_complete 落账，幂等键 = task_id」的契约在生产
    // 从未成立：on_complete 回调零接线（`Cluster::with_callback`/
    // `set_callback` 生产零注册点），cluster_result 行及其裸键只存在于旧
    // 测试想象中（根修，查找收编
    // [`delivery_evidence_record`] 单一入口）；**非 rejected 即在
    // 场**（项 C 起 status 初始 recorded、通道接受才 delivered
    // ——通知维度与交付证据正交：recorded/delivered/confirmed 都算在场，
    // rejected 单独为拒绝判据）。
    // （二轮复评）结构完整性升级：空壳记录（完成回调无条件
    // 记账，payload 可为 Null）不再满足闸——payload JSON 必须带 `delivery`
    // 结构（真走交付管线的 capture；普通 cluster_result 空壳行不算），
    // `delivery.commit`/`.tree` 非空（不可变交付点），`patch_truncated=true`
    // ⇒ `patch_path` 非空（截断必有盘上留底， 既有契约复用）。缺口分档
    // 文案：无记录 / rejected / 无 delivery 结构（worker 未走交付管线或
    // 执行体未编译交付管线 checkpoint-git）/ 缺 commit·tree / 截断无留底。
    // typed 证据块加法演进：`delivery.files`
    // / `delivery.checks` **在场时**无论开关都验形态（files path 复用
    // anchor 路径形态闸——拒绝绝对路径/`..`/盘符/UNC，sha256 64-hex；
    // checks 每条 tool/cmd 非空字符串 + ok 布尔 + summary 字符串），验不过
    // = FAIL；缺席块行为不变。严格闸 `board.review.evidence_typed_required`
    // （默认 **false** 烧入期）开启后 `files` 块必须在场（git tree diff
    // 确定性派生，任何 git 交付都应可派生——缺席 = 证据面残缺）；`checks`
    // **恒可选**（没跑检查 = 诚实的事实陈述，强制会诱发造值）。
    // 全部 = 确定性降级 FAIL——结构事实零 token 可判（不劳语义评审，也不
    // 进 自检挂起：自检取证改变不了账本事实），后续重派/熔断/转人工
    // 全走既有 FAIL 决策表，无新分支。
    // 版本绑定 = `dispatches.last().task_id`（最新派发锚）——评估方「产物
    // 属于旧 attempt」已被此覆盖：重派产生新 task_id，旧记录不满足新锚。
    // 本闸在 之后执行：LLM 主动要证据的挂起路径优先走完，二段评审的
    // 结论回到这里同样过闸。
    let mut output = output;
    let evidence_required = cfg.review.require_delivery_evidence || issue.requires_evidence;
    if evidence_required && output.verdict == nemesis_board::ReviewVerdict::Pass {
        let evidence = dispatches.last().map(|d| d.task_id.clone()).and_then(|task_id| {
            let db = deps.cluster.task_db()?;
            let rec = delivery_evidence_record(&db, &task_id)?;
            if rec.status == "rejected" {
                return Some(Err(format!(
                    "缺交付证据：最新派发任务 {task_id} 的交付记录状态为 rejected（拒绝不算在场）——交付已被拒。请走正常交付流程后重交。"
                )));
            }
            let delivery = delivery_evidence_payload(&rec);
            let Some(delivery) = delivery else {
                return Some(Err(
                    "缺交付证据：交付记录存在但 payload 无 delivery 结构——worker 未走交付管线，或执行体未编译交付管线（checkpoint-git 裁剪形态）。请用带交付管线的执行体重跑。".to_string(),
                ));
            };
            let commit = delivery.get("commit").and_then(|v| v.as_str()).unwrap_or("");
            let tree = delivery.get("tree").and_then(|v| v.as_str()).unwrap_or("");
            if commit.is_empty() || tree.is_empty() {
                return Some(Err(
                    "缺交付证据：delivery 结构缺 commit/tree（不可变交付点缺失）——交付分支捕获不完整。请重跑任务以重新捕获交付。".to_string(),
                ));
            }
            let truncated = delivery
                .get("patch_truncated")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let patch_path = delivery
                .get("patch_path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if truncated && patch_path.is_empty() {
                return Some(Err(
                    "缺交付证据：patch 已截断但无 patch_path 盘上留底——截断交付必须留底（G4 契约）。请重跑任务以重新捕获交付。".to_string(),
                ));
            }
            // typed files 块（在场即验形态，加法演进）：path 形态闸复用
            // anchor 路径校验（解析期 FS 无关，集群跨平台拒绝 Windows 形态
            // 绝对路径）。（复评）kind 分臂：capture 侧含全部
            // 变更类型——add/modify 须 sha256 64-hex（modify 另须 old_sha256
            // 64-hex）；delete 无新内容可哈希，须 old_sha256 64-hex 且无
            // sha256（old 摘要承载「删除确实发生」的证据）。kind 缺席 = 旧
            // 形态条目（恒 add/modify、恒有 sha256）→ 有 sha 即过，存量
            // 夹具零改动；kind 在场但值未知 = loud 形态非法（不静默滑过）。
            // 键在场但非数组同样是形态非法。
            if let Some(files_val) = delivery.get("files") {
                let Some(files) = files_val.as_array() else {
                    return Some(Err(
                        "交付证据 typed files 块形态非法：files 必须是数组。请重跑任务以重新捕获交付。".to_string(),
                    ));
                };
                let is_hex64 =
                    |s: &str| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit());
                for f in files {
                    let path = f.get("path").and_then(|v| v.as_str()).unwrap_or("");
                    if path.is_empty() || nemesis_board::anchor::validate_anchor_path_shape(path).is_err() {
                        return Some(Err(format!(
                            "交付证据 typed files 块形态非法：path「{path}」不是合法的仓库相对路径（拒绝绝对路径/`..`/盘符等）。请重跑任务以重新捕获交付。"
                        )));
                    }
                    let sha = f.get("sha256").and_then(|v| v.as_str()).unwrap_or("");
                    let old_sha = f.get("old_sha256").and_then(|v| v.as_str()).unwrap_or("");
                    let shape_bad = match f.get("kind").and_then(|v| v.as_str()) {
                        None => !is_hex64(sha),
                        Some("add") => !is_hex64(sha) || !old_sha.is_empty(),
                        Some("modify") => !is_hex64(sha) || !is_hex64(old_sha),
                        Some("delete") => !sha.is_empty() || !is_hex64(old_sha),
                        Some(_) => true,
                    };
                    if shape_bad {
                        return Some(Err(format!(
                            "交付证据 typed files 块形态非法：path「{path}」的 kind/sha256/old_sha256 组合不合 wire 契约（add/modify 须 64 位 hex sha256；modify 另须 old_sha256；delete 须 64 位 hex old_sha256 且无 sha256）。请重跑任务以重新捕获交付。"
                        )));
                    }
                }
            }
            // typed checks 块（在场即验形态）：每条 = capture 侧
            // CheckRecord wire 契约（tool/cmd 非空字符串 + ok 布尔 +
            // summary 字符串；多余字段不拦——加法演进对齐 JSON Schema 默认）。
            if let Some(checks_val) = delivery.get("checks") {
                let Some(checks) = checks_val.as_array() else {
                    return Some(Err(
                        "交付证据 typed checks 块形态非法：checks 必须是数组。请重跑任务以重新捕获交付。".to_string(),
                    ));
                };
                for c in checks {
                    let tool = c.get("tool").and_then(|v| v.as_str()).unwrap_or("");
                    let cmd = c.get("cmd").and_then(|v| v.as_str()).unwrap_or("");
                    let ok = c.get("ok").and_then(|v| v.as_bool());
                    let summary = c.get("summary").and_then(|v| v.as_str());
                    if tool.trim().is_empty() || cmd.trim().is_empty() || ok.is_none() || summary.is_none() {
                        return Some(Err(
                            "交付证据 typed checks 块形态非法：每条记录须含非空 tool/cmd、布尔 ok、字符串 summary。请重跑任务以重新捕获交付。".to_string(),
                        ));
                    }
                }
            }
            // 版本绑定交叉对账（加法演进：checks_binds 在场
            // 才验）：checks 窗口的基线/关闭 tree 必须与 capture 元数据一致
            // ——「这些检查就是在交付的那棵树上跑的」。base≠base（检查跑在
            // 别的基线上）或 head≠tree（检查跑完后又改了文件）= 证据失真，
            // 按结构事实降级 FAIL（零 token 可判）。键缺席 = 旧形态 payload，
            // 诚实跳过不追溯。
            if let Some(binds) = delivery.get("checks_binds") {
                let Some(binds) = binds.as_object() else {
                    return Some(Err(
                        "交付证据 checks_binds 块形态非法：必须是对象。请重跑任务以重新捕获交付。".to_string(),
                    ));
                };
                let bind_base = binds.get("base_tree").and_then(|v| v.as_str());
                let bind_head = binds.get("head_tree").and_then(|v| v.as_str());
                let cap_base = delivery.get("base_tree").and_then(|v| v.as_str());
                let cap_tree = delivery.get("tree").and_then(|v| v.as_str());
                if let (Some(bb), Some(cb)) = (bind_base, cap_base)
                    && bb != cb
                {
                    return Some(Err(format!(
                        "交付证据版本绑定不一致：checks 基线 tree（{bb}）≠ capture 基线 tree（{cb}）——检查证据跑在别的版本上。请重跑任务以重新捕获交付。"
                    )));
                }
                if let (Some(bh), Some(ct)) = (bind_head, cap_tree)
                    && bh != ct
                {
                    return Some(Err(format!(
                        "交付证据版本绑定不一致：checks 关窗 tree（{bh}）≠ capture 交付 tree（{ct}）——检查跑完后又改了文件，检查证据已失真。请重跑任务以重新捕获交付。"
                    )));
                }
            }
            // 严格闸（默认 false 烧入期，翻闸须用户裁决）：files 必须在场。
            // 后 files 恒序列化（空数组 = 「零文件变更」的真实派生事实，
            // 仍按证据面残缺处置——git 交付的文件清单不应为空）。
            if cfg.review.evidence_typed_required
                && delivery
                    .get("files")
                    .and_then(|v| v.as_array())
                    .is_none_or(|a| a.is_empty())
            {
                return Some(Err(
                    "交付缺类型化证据块：delivery.files 不在场——git tree diff 应可确定性派生（任何 git 交付都有文件清单），缺席 = 证据面残缺。请用带交付管线的执行体重跑。".to_string(),
                ));
            }
            Some(Ok(()))
        });
        if let Some(Err(gap)) = evidence {
            output.verdict = nemesis_board::ReviewVerdict::Fail;
            output.gap = gap.clone();
            output
                .reasons
                .push("交付证据缺失/不完整（评审门强制，结构完整性校验 F5）".to_string());
            let _ = post_review_comment(
                store,
                issue_id,
                &deps.cluster,
                &format!(
                    "🔍 交付证据闸：评审结论 PASS 但交付证据结构校验未通过，按缺证据降级 FAIL 处置（重派/转人工走既有 FAIL 流程）。\n\n缺口：{gap}"
                ),
            );
            warn!("[BoardReview] issue {issue_id} 交付证据缺失：PASS 降级 FAIL（{gap}）");
        } else if evidence.is_none() {
            output.verdict = nemesis_board::ReviewVerdict::Fail;
            output.gap = "缺交付证据：本单要求交付记录背书（AC 含 file: 锚点或全局交付证据开关生效），但最新派发任务在交付账本中无 delivered/confirmed 记录——worker 未走交付管线或交付已被拒。请走正常交付流程后重交。".to_string();
            output.reasons.push(
                "交付证据缺失：最新派发任务无 delivered/confirmed 交付记录（评审门强制）"
                    .to_string(),
            );
            let _ = post_review_comment(
                store,
                issue_id,
                &deps.cluster,
                "🔍 交付证据闸：评审结论 PASS 但最新派发任务在交付账本无 delivered/confirmed 记录，按缺证据降级 FAIL 处置（重派/转人工走既有 FAIL 流程）。",
            );
            warn!("[BoardReview] issue {issue_id} 交付证据缺失：PASS 降级 FAIL（缺交付记录）");
        }
    }

    // ---- 必检计划闸（SKIP≠PASS）：评审 PASS 后确定性对账
    // ——任务文本反引号命令 = 必检计划（derive 纯函数，与 prompt 注入同源
    // 现算），× delivery.checks 执行证据（delivery_checks_for）。计划内没跑
    // （⏭）或跑了失败（❌）= 交付不完整 → PASS 降级 FAIL（结构事实零 token
    // 可判，不劳语义评审，也不进 自检挂起——取证改变不了执行记录）。
    // 后续重派/熔断/转人工全走既有 FAIL 决策表，无新分支。开关 =
    // board.review.require_check_plan（默认 false 烧入期）。无计划项 =
    // 诚实跳过（本闸只管「计划内必检项」，不强制所有单都跑检查）。
    if cfg.review.require_check_plan && output.verdict == nemesis_board::ReviewVerdict::Pass {
        let plan = nemesis_board::derive_check_plan(&[
            &issue.title,
            issue.description.as_str(),
            issue.acceptance_criteria.as_deref().unwrap_or(""),
        ]);
        if !plan.is_empty() {
            let checks = delivery_checks_for(deps, &dispatches);
            let statuses = nemesis_board::reconcile_check_plan(&plan, &checks);
            let unmet: Vec<&nemesis_board::CheckPlanStatus> = statuses
                .iter()
                .filter(|s| !s.ran || s.ok == Some(false))
                .collect();
            if !unmet.is_empty() {
                let gap = format!(
                    "必检计划未满足：计划内 {} 项必检项未跑或失败（SKIP≠PASS）——交付不完整。未满足项：{}",
                    unmet.len(),
                    unmet
                        .iter()
                        .map(|s| format!(
                            "`{}`（{}）",
                            s.check,
                            if s.ran { "执行失败" } else { "未执行" }
                        ))
                        .collect::<Vec<_>>()
                        .join("、")
                );
                output.verdict = nemesis_board::ReviewVerdict::Fail;
                output.gap = gap.clone();
                output
                    .reasons
                    .push("必检计划未满足（评审门强制，SKIP≠PASS）".to_string());
                let _ = post_review_comment(
                    store,
                    issue_id,
                    &deps.cluster,
                    &format!(
                        "🔍 必检计划闸：评审结论 PASS 但计划内必检项未全部执行通过，按交付不完整降级 FAIL 处置（重派/转人工走既有 FAIL 流程）。\n\n{}",
                        nemesis_board::render_check_plan_reconciliation(&statuses)
                    ),
                );
                warn!("[BoardReview] issue {issue_id} 必检计划未满足：PASS 降级 FAIL（{gap}）");
            }
        }
    }

    // c §③ unknowns 落盘：评审存疑事项转 issue 系统评论的
    // `unknown: <事项>` 结构化标记行（b ⑥ FINISH 谓词消费此标记；
    // 人工评论回应即裁决）。落盘时机 = 全部降级闸走完、verdict 终态后。
    persist_unknowns(store, issue_id, &output.unknowns);

    // coding_delivery 整包行：所有闸走完、结论终态后把
    // 整包证据索引 + 终态结论入账（kind="coding_delivery"，幂等键 =
    // `{task_id}:coding_delivery`——复评重放幂等跳过，同一交付只出一行整
    // 包账）。patch 全文不重复入库（branch 行/盘上留底已有）。无派发或无
    // 交付记录（人工转入/旧流程单）= 诚实跳过。任何失败只 warn 不炸评审。
    {
        let task_id = dispatches.last().map(|d| d.task_id.clone());
        let delivery_obj = task_id.as_deref().and_then(|tid| {
            let db = deps.cluster.task_db()?;
            delivery_evidence_record(&db, tid)
                .and_then(|rec| delivery_evidence_payload(&rec).cloned())
        });
        let anchor_summary = if anchor_results.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::json!({
                "passed": nemesis_board::all_passed(&anchor_results),
                "total": anchor_results.len(),
                "detail": nemesis_board::render_anchor_summary(&anchor_results),
            })
        };
        crate::review_ledger::coding_delivery_record(
            deps.cluster.task_db().as_ref(),
            task_id.as_deref(),
            issue_id,
            &issue.number,
            output.verdict.as_str(),
            &output.gap,
            &output.reasons,
            delivery_obj.as_ref(),
            anchor_summary,
        );
    }

    // 经验落库（.5 集体记忆 蒸馏唯一写闸）：评审 agent 的
    // experience 槽位 → team_memory 表。空壳条目（词表外空白 category /
    // 空 scope/content）丢弃；去重合并与计数在 store 内部；失败只 warn
    // 不炸评审流程（经验沉淀是增值动作，验收结论不受影响）。 挂起
    // 路径在上方 return，不蒸馏（评审未定案）。
    store_experience(
        store,
        &issue.number,
        deps.cluster.node_id(),
        &output.experience,
    );

    // 指纹记账：评审已定案（迟到守卫 / 取证挂起 / estop 冻结都在更
    // 早的分支分流了），按 verdict 给本轮执行的 worker（最新派发）记成败。
    // task_id 幂等，评审重放 / estop 复评不重复计数；开关无关（见 fn 注释）。
    record_fingerprint_outcome(deps, &issue, &dispatches, output.verdict);

    let unlimited = cfg.unlimited_mode;
    match decide_review_action(
        output.verdict,
        round,
        cfg.max_redispatch,
        cfg.auto_accept,
        unlimited,
    ) {
        ReviewAction::AutoAccept => {
            // EST-03（横扫加固）：评审 LLM 是长跑——发起时 estop
            // 可能尚未触发、结束时已挂起。收货落 done 前补一道检查点：挂起
            // 即停车入队（release 复评恢复），不越过急停继续自动流转。
            if estop_fuse_engaged(deps, issue_id, ParkedKind::Issue) {
                return Ok(false);
            }
            fail_streaks.clear(issue_id);
            // （第 6 轮审计）：决策审计在收货**生效后**落
            // ——曾先记 auto_accept 再 settle，settle Err（? 传播）时审计已
            // 记「已自动收货」而状态仍 in_review，决策流视图出现从未生效
            // 的决策（审计面与实际状态分叉）。审计记录已发生的事，不预设
            // 将发生的事。
            auto_accept_and_settle(deps, issue_id, &output.reasons)?;
            record_auto_decide(
                deps,
                issue_id,
                "auto_accept",
                output.verdict.as_str(),
                serde_json::json!({
                    "round": round,
                    "reasons": render_reasons(&output.reasons),
                }),
            );
        }
        ReviewAction::SuggestManual => {
            fail_streaks.clear(issue_id);
            record_auto_decide(
                deps,
                issue_id,
                "suggest_manual",
                output.verdict.as_str(),
                serde_json::json!({
                    "round": round,
                    "reasons": render_reasons(&output.reasons),
                }),
            );
            let reasons = render_reasons(&output.reasons);
            store.add_comment(NewComment {
                issue_id,
                author: reviewer.clone(),
                content: format!("✅ agent 验收通过，待人工确认\n\n{reasons}"),
                parent_id: None,
                ctype: CommentType::Comment,
            })?;
            info!("[BoardReview] issue {issue_id} PASS → 待人工确认（保持 in_review）");
        }
        ReviewAction::Redispatch => {
            // 会话 token 预算（集群无人值守硬化；优先于
            // //unlimited——预算耗尽是资源闸不是行为异常：重派会被
            // worker 入口拒接原样退回，换节点也不解（会话键收敛同一
            // worker 是派发语义），只有人工裁决能解开（调大预算 / 新建
            // 会话 / 改派发语义）。unlimited 放大的是重派次数，不是预算。
            if latest_fail_class(&comments) == Some("token_budget") {
                fail_streaks.clear(issue_id);
                // U-A：停车现场落盘（verdict_chain 全程链）。
                let handoff_note = park_with_handoff(
                    deps,
                    &issue.number,
                    issue_id,
                    nemesis_board::handoff::HandoffSpec {
                        stop_kind: "budget",
                        suggested_action: "manual_check",
                        hint: "worker 会话 token 预算耗尽（资源闸，重派会被入口拒接原样退回、换节点也不解）：调大 agents.defaults.session_token_budget 或新建会话后重新派发".into(),
                        resume_entry: "review.retrigger（number=NB-x，修好后复评）/ issue.dispatch（新会话重派）".into(),
                        environment: last_dispatch_env(&dispatches),
                        attempted: vec![serde_json::json!({
                            "action": "redispatch",
                            "result": "worker 入口预算闸拒接（fail_class=token_budget）",
                        })],
                    },
                );
                let comment = format!(
                    "{}💰 会话 token 预算耗尽：worker 会话累计用量已达 \
                     agents.defaults.session_token_budget 上限，已停止自动重派，请人工裁决（调大预算或新建会话后重新派发/复评）。{handoff_note}",
                    human_mention
                );
                store.add_comment(NewComment {
                    issue_id,
                    author: reviewer.clone(),
                    content: comment,
                    parent_id: None,
                    ctype: CommentType::Comment,
                })?;
                record_auto_decide(
                    deps,
                    issue_id,
                    "escalate_human",
                    output.verdict.as_str(),
                    serde_json::json!({
                        "round": round,
                        "reason": "token_budget",
                    }),
                );
                warn!("[BoardReview] issue {issue_id} 会话 token 预算耗尽 → 停止重派转人工");
                return Ok(true);
            }
            // worker 循环升级（优先于 计数与 unlimited_mode）：worker
            // 自己声明同问题反复重派无解 → 不再重派直接转人工。worker 喊停
            // 是比失败签名计数更强的信号；unlimited 放大的是「系统侧重派
            // 次数不设限」，不是「无视执行方喊停」。
            if let Some(marker) = detect_worker_loop_marker(
                store,
                issue_id,
                deps.cluster.node_id(),
                // 轮次绑定：标记只认最新派发（含）之后写入的评论——旧轮喊停
                // 已由重派动作答复，历史标记不得阻塞换条件后的新轮（
                // 死循环根修，语义见 fn 文档）。
                dispatches.last().map(|d| d.dispatched_at),
            )? {
                fail_streaks.clear(issue_id);
                // U-A：停车现场落盘。
                let handoff_note = park_with_handoff(
                    deps,
                    &issue.number,
                    issue_id,
                    nemesis_board::handoff::HandoffSpec {
                        stop_kind: "worker_loop",
                        suggested_action: "manual_check",
                        hint: format!(
                            "worker 自报同一问题反复重派无解（卡点：{marker}）——执行方喊停是比失败计数更强的信号：换方法/换人/修需求书后再续"
                        ),
                        resume_entry: "issue.dispatch（换条件重派）/ review.retrigger（复评）"
                            .into(),
                        environment: last_dispatch_env(&dispatches),
                        attempted: vec![serde_json::json!({
                            "action": "redispatch",
                            "result": format!("worker 循环升级标记（卡点：{marker}）"),
                        })],
                    },
                );
                let comment = format!(
                    "{}🛑 worker 循环升级：执行方在交付中声明同一问题反复重派无解（卡点：{marker}），已停止自动重派，请人工裁决（换方法/换人/修需求书后重新派发或复评）。{handoff_note}",
                    human_mention
                );
                store.add_comment(NewComment {
                    issue_id,
                    author: reviewer.clone(),
                    content: comment,
                    parent_id: None,
                    ctype: CommentType::Comment,
                })?;
                record_auto_decide(
                    deps,
                    issue_id,
                    "escalate_human",
                    output.verdict.as_str(),
                    serde_json::json!({
                        "round": round,
                        "reason": "worker_loop_marker",
                        "note": marker,
                    }),
                );
                warn!("[BoardReview] issue {issue_id} worker 循环升级标记 → 停止重派转人工");
                return Ok(true);
            }
            // 同因熔断（机制加固；A 签名带 verdict；优先于
            // unlimited_mode 与 P2A——unlimited 放大的是「重派次数不设限」，
            // 不是「无收敛重派授权」）：同一失败签名（{verdict}|{排序失败
            // 锚点集合}；无失败锚点 = {verdict}|no-anchors）连续 ≥3 次 →
            // 停止自动重派转人工。签名翻转已在账本内清零重计。
            let streak_signature =
                nemesis_board::fail_streak::failure_signature(&anchor_results, &output.verdict);
            let streak_outcome = fail_streaks.record_and_check(issue_id, &streak_signature);
            if let nemesis_board::fail_streak::StreakOutcome::Tripped(n) = streak_outcome {
                // （第 6 轮审计）：熔断转人工是**非重派终态**
                // ——账本清账（与 worker 循环升级/token 预算/UNSURE 闸三臂
                // 同纪律，fail_streak.rs 不变量「转人工清账」）。曾漏清：
                // 熔断停车后人按 resume_entry 指引 review.retrigger 复评
                // （retrigger 不清账），存量 count≥3 仍在账——人工修好后
                // 首轮同签名失败即刻再熔断，「人工处理后清零复评」的承诺
                // 在此路径落空，人工修复零重派预算。
                fail_streaks.clear(issue_id);
                // C：熔断评论 + 审计 extra 带最后一轮评审理由（停车即可
                // 裁决，对齐 EscalateHuman 先例）。U-A：停车现场落盘（本
                // 分支已并入 park_with_handoff 单一收口）。
                let handoff_note = park_with_handoff(
                    deps,
                    &issue.number,
                    issue_id,
                    nemesis_board::handoff::HandoffSpec {
                        stop_kind: "fail_streak",
                        suggested_action: "manual_check",
                        hint: format!(
                            "同一失败签名「{streak_signature}」连续 {n} 轮未通过（无收敛重派）：人工核查交付/环境后复评或手动重派（清零计数）"
                        ),
                        resume_entry: "review.retrigger（number=NB-x，人工处理后清零复评）".into(),
                        environment: last_dispatch_env(&dispatches),
                        attempted: vec![serde_json::json!({
                            "action": "redispatch",
                            "result": format!("同因熔断：签名 {streak_signature} 连续 {n} 轮"),
                        })],
                    },
                );
                let mut comment = format!(
                    "{}⛔ 同因熔断：同一失败签名已连续 {n} 轮未通过（签名：{streak_signature}），已停止重派，请人工裁决（人工处理后自动复评/手动重派会清零计数）。\n{handoff_note}",
                    human_mention
                );
                append_reasons_unless_gapped(&mut comment, &output);
                store.add_comment(NewComment {
                    issue_id,
                    author: reviewer.clone(),
                    content: comment,
                    parent_id: None,
                    ctype: CommentType::Comment,
                })?;
                record_auto_decide(
                    deps,
                    issue_id,
                    "escalate_human",
                    output.verdict.as_str(),
                    serde_json::json!({
                        "round": round,
                        "reason": "fail_signature_streak",
                        "signature": streak_signature,
                        "streak": n,
                        "reasons": render_reasons(&output.reasons),
                    }),
                );
                warn!(
                    "[BoardReview] issue {issue_id} 同因熔断：签名「{streak_signature}」连续 {n} 轮 → 转人工不重派"
                );
                return Ok(true);
            }
            // B UNSURE 独立收敛闸（默认 2，0=不限）：UNSURE 恒签名
            // 「unsure|no-anchors」，2 < 阈值 3 → 本闸先停。语义：给一次
            // worker 自愈机会，超出即认定「只有人能解」（第 4 轮实证
            // ——已补齐全部证据仍 UNSURE，卡点超出无人环境能力）。复用
            // EscalateHuman 语义：清账 + 审计 + reasons 评论。
            if nemesis_board::fail_streak::unsure_gate_trips(
                &output.verdict,
                &streak_outcome,
                cfg.review.max_unsure_streak,
            ) {
                let n = match streak_outcome {
                    nemesis_board::fail_streak::StreakOutcome::Continue(n) => n,
                    nemesis_board::fail_streak::StreakOutcome::Tripped(_) => {
                        unreachable!("Tripped 已由 M3 熔断臂先行 return，闸判定只收 Continue")
                    }
                };
                let max_unsure = cfg.review.max_unsure_streak;
                fail_streaks.clear(issue_id);
                // U-A：停车现场落盘。
                let handoff_note = park_with_handoff(
                    deps,
                    &issue.number,
                    issue_id,
                    nemesis_board::handoff::HandoffSpec {
                        stop_kind: "unsure_streak",
                        suggested_action: "manual_check",
                        hint: format!(
                            "连续 {n} 轮验收无法定案（UNSURE 达闸 {max_unsure}）——已补齐全部证据仍不能客观判定，卡点超出无人环境能力，只有人能解"
                        ),
                        resume_entry: "review.retrigger（number=NB-x，人工处理后清零复评）".into(),
                        environment: last_dispatch_env(&dispatches),
                        attempted: vec![serde_json::json!({
                            "action": "redispatch",
                            "result": format!("UNSURE 连续 {n} 轮（阈值 {max_unsure}）"),
                        })],
                    },
                );
                record_auto_decide(
                    deps,
                    issue_id,
                    "escalate_human",
                    output.verdict.as_str(),
                    serde_json::json!({
                        "round": round,
                        "reason": "unsure_streak",
                        "signature": streak_signature,
                        "streak": n,
                        "threshold": max_unsure,
                        "reasons": render_reasons(&output.reasons),
                    }),
                );
                let mut comment = format!(
                    "{}🤷 连续 {n} 轮验收无法定案（UNSURE，达到 board.review.max_unsure_streak={max_unsure}），已停止自动重派，请人工裁决（人工处理后自动复评/手动重派会清零计数）。\n{handoff_note}",
                    human_mention
                );
                append_reasons_unless_gapped(&mut comment, &output);
                store.add_comment(NewComment {
                    issue_id,
                    author: reviewer.clone(),
                    content: comment,
                    parent_id: None,
                    ctype: CommentType::Comment,
                })?;
                warn!(
                    "[BoardReview] issue {issue_id} UNSURE 连续 {n} 轮（阈值 {max_unsure}）→ 转人工不重派"
                );
                return Ok(true);
            }
            // P2A 能力类失败保护（根修；② b 泛化
            // 为分类→重派策略映射）：分类存在时映射表优先于纯预算逻辑。
            // Freeze（副作用不明）无论目标是否已换、unlimited 与否一律冻结
            // 转人工（unlimited 放大的是重派次数，不是副作用知情权——
            // token_budget 专用闸同语义先例）；CapabilitySwitch /
            // SwitchNode 仅在目标未换时加闸（换节点 = 新环境/新 mandate，
            // 重放可能成）；TransientRetry 与无分类走现状预算逻辑（兼容
            // 不变）。目标预判放在 ❌ 评论/审计之前（pick 纯读无副作用）；
            // 通过闸后走正常重派（启发式护栏，非正确性不变量）。
            let preview_target = pick_redispatch_target(deps, &issue, &dispatches).ok();
            let target_switched = preview_target.as_ref().is_some_and(|c| c.is_switch());
            let latest_fc = latest_fail_class(&comments);
            let frozen = latest_fc.filter(|fc| redispatch_policy(fc) == FailClassPolicy::Freeze);
            if let Some(fc) = frozen {
                fail_streaks.clear(issue_id);
                // U-A：停车现场落盘。
                let handoff_note = park_with_handoff(
                    deps,
                    &issue.number,
                    issue_id,
                    nemesis_board::handoff::HandoffSpec {
                        stop_kind: "fail_class_freeze",
                        suggested_action: "manual_check",
                        hint: format!(
                            "失败分类（fail_class: {fc}）语义为副作用后果不明——自动重派无法评估影响范围，已冻结：人工检查处置后重派"
                        ),
                        resume_entry: "issue.dispatch（人工处置后重派）/ review.retrigger（复评）"
                            .into(),
                        environment: last_dispatch_env(&dispatches),
                        attempted: vec![serde_json::json!({
                            "action": "redispatch",
                            "result": format!("冻结类失败 {fc}（副作用不明不自动重派）"),
                        })],
                    },
                );
                let comment = format!(
                    "{}🧯 失败冻结保护（fail_class: {fc}）：该失败分类的副作用后果不明，自动重派可能扩大影响，已停止自动重派，请人工检查处置。\n\n{}{handoff_note}",
                    human_mention,
                    capability_fail_hint(fc)
                );
                store.add_comment(NewComment {
                    issue_id,
                    author: reviewer.clone(),
                    content: comment,
                    parent_id: None,
                    ctype: CommentType::Comment,
                })?;
                record_auto_decide(
                    deps,
                    issue_id,
                    "escalate_human",
                    output.verdict.as_str(),
                    serde_json::json!({
                        "round": round,
                        "fail_class": fc,
                        "reason": "fail_class_freeze",
                        "reasons": render_reasons(&output.reasons),
                    }),
                );
                warn!(
                    "[BoardReview] issue {issue_id} 冻结类失败（{fc}）→ 转人工不重派（副作用不明）"
                );
                // 触发点①：重派无意义停车 → 重规划起草
                // （建议≠授权；fire-and-forget，失败不影响本臂结论）。
                crate::board_replan::spawn_replan_draft(
                    deps.clone(),
                    issue_id,
                    "fail_class_freeze",
                );
                return Ok(true);
            }
            let capability_fail = (!target_switched)
                .then_some(latest_fc)
                .flatten()
                .filter(|fc| {
                    matches!(
                        redispatch_policy(fc),
                        FailClassPolicy::CapabilitySwitch | FailClassPolicy::SwitchNode
                    )
                });
            if let Some(fc) = capability_fail {
                if unlimited {
                    let note = format!(
                        "⚠ 能力类失败（fail_class: {fc}）且重派目标未换——同节点重放大概率原样复现（unlimited_mode 仅告警继续，estop 可随时止血）"
                    );
                    let _ = post_review_comment(store, issue_id, &deps.cluster, &note);
                    warn!(
                        "[BoardReview] issue {issue_id} 能力类失败（{fc}）+ 同目标重派（unlimited_mode 告警继续）"
                    );
                } else {
                    // U-A：停车现场落盘。
                    let handoff_note = park_with_handoff(
                        deps,
                        &issue.number,
                        issue_id,
                        nemesis_board::handoff::HandoffSpec {
                            stop_kind: "capability_fail",
                            suggested_action: "redispatch",
                            hint: format!(
                                "能力类失败（fail_class: {fc}）且重派目标未换——同模型同环境大概率原样复现：换节点/换模型档位后重派"
                            ),
                            resume_entry: "issue.dispatch（换节点）/ 调整模型后 review.retrigger"
                                .into(),
                            environment: last_dispatch_env(&dispatches),
                            attempted: vec![serde_json::json!({
                                "action": "redispatch",
                                "result": format!("能力类失败 {fc} + 同目标预判"),
                            })],
                        },
                    );
                    let comment = format!(
                        "{}🤖 能力类失败保护（fail_class: {fc}）：worker 在同一节点反复以相同方式失败，自动重派同节点大概率原样复现，已停止自动重派，请人工裁决。\n\n{}{handoff_note}",
                        human_mention,
                        capability_fail_hint(fc)
                    );
                    store.add_comment(NewComment {
                        issue_id,
                        author: reviewer.clone(),
                        content: comment,
                        parent_id: None,
                        ctype: CommentType::Comment,
                    })?;
                    record_auto_decide(
                        deps,
                        issue_id,
                        "escalate_human",
                        output.verdict.as_str(),
                        serde_json::json!({
                            "round": round,
                            "fail_class": fc,
                            "reason": "capability_fail_same_target",
                            "reasons": render_reasons(&output.reasons),
                        }),
                    );
                    info!(
                        "[BoardReview] issue {issue_id} 能力类失败（{fc}）+ 同目标 → 转人工（保持 in_review）"
                    );
                    // 触发点①：重派无意义停车 → 重规划起草。
                    crate::board_replan::spawn_replan_draft(
                        deps.clone(),
                        issue_id,
                        "capability_fail",
                    );
                    return Ok(true);
                }
            }
            // UNSURE（无限模式）无 gap——给诚实占位意见，重派 prompt 不空转。
            // c §③：带逐条判定时渲染条目清单（worker 知道具体哪条
            // 没过、缺什么证据）；旧格式/无条目回落 gap 原文（行为不变）。
            // 缺口清账：上一轮清单里未声明闭合的条目结转 + 本轮新增条目
            // 一起重新编号（parse/render 互逆，下轮评审从评论解析回清单）。
            let gap = nemesis_board::render_gap_with_ledger(&output, &prior_gaps);
            let gap = if gap.is_empty() {
                "验收 agent 无法定案（UNSURE）。请对照验收标准逐条自查，补充客观证据后重新汇报。"
                    .to_string()
            } else {
                gap
            };
            let next_round = round + 1;
            let budget_display = if unlimited {
                "∞".to_string()
            } else {
                cfg.max_redispatch.to_string()
            };
            store.add_comment(NewComment {
                issue_id,
                author: reviewer.clone(),
                content: format!(
                    "❌ agent 验收未通过（第 {next_round}/{budget_display} 次重派）\n\n## 差距\n{gap}\n{}",
                    render_reasons(&output.reasons)
                ),
                parent_id: None,
                ctype: CommentType::Comment,
            })?;
            record_auto_decide(
                deps,
                issue_id,
                "redispatch",
                output.verdict.as_str(),
                serde_json::json!({
                    "round": next_round,
                    "unlimited": unlimited,
                    "reasons": render_reasons(&output.reasons),
                }),
            );
            // 停滞可观测（配套，复审补充）：连续重派且差距文本与上一轮
            // 完全相同 → WARN 告警（只告警不停流程，人可观测不干预）。
            // （第 6 轮审计）：comments 是评审入口快照（761
            // 行），本轮刚落的差距评论**不在**里面——曾 skip(1) 想跳「刚落
            // 的本轮评论」，实际跳掉的是上一轮的差距评论（本该比对的对象）
            // 停滞告警晚一轮触发（同差距要连续 3 轮才响）甚至 2 轮即收敛
            // 时永不触发。c §③：比对文本 = 渲染后差距（条目清单轮次间
            // 同构可比；旧格式一轮过渡后回落 gap 原文，语义不变）。
            if unlimited && round >= 2 && !gap.is_empty() {
                let stale = comments
                    .iter()
                    .rev()
                    .find(|c| c.ctype == CommentType::Comment && c.content.contains("## 差距"))
                    .is_some_and(|c| c.content.contains(gap.as_str()));
                if stale {
                    warn!(
                        "[BoardReview] issue {issue_id} 停滞嫌疑：第 {next_round} 轮重派差距与上轮相同（unlimited_mode 仅告警不停，estop 可随时止血）"
                    );
                }
            }
            // 换节点重派：同一 worker 连续 ≥2 次派发仍 FAIL → 换历史
            // 未用过的次优匹配节点；否则维持同 worker（会话上下文延续）。
            // estop 保险丝检查点 ③：发车前最后一闸（评审 LLM 期间触发急停
            // 的在途轮次在此截断），停车等释放。
            if estop_fuse_engaged(deps, issue_id, ParkedKind::Issue) {
                return Ok(false);
            }
            // 预算保险丝（发车前）：三维任一超限 → 停自动重派转人工；
            // unlimited_mode 降级为 WARN 继续（保险丝非护栏的边界一致）。
            if let Some(breach) = budget_breach(deps, &issue, &cfg) {
                if unlimited {
                    warn!(
                        "[BoardReview] issue {issue_id} 预算超限（unlimited_mode 仅告警继续）：{breach}"
                    );
                } else {
                    // U-A：停车现场落盘。
                    let handoff_note = park_with_handoff(
                        deps,
                        &issue.number,
                        issue_id,
                        nemesis_board::handoff::HandoffSpec {
                            stop_kind: "budget",
                            suggested_action: "manual_check",
                            hint: format!(
                                "自动重派预算超限（{breach}）：调大 board.budget.*（0=关该维）后复评"
                            ),
                            resume_entry: "review.retrigger（number=NB-x，调预算后复评）".into(),
                            environment: last_dispatch_env(&dispatches),
                            attempted: vec![serde_json::json!({
                                "action": "redispatch",
                                "result": format!("预算超限：{breach}"),
                            })],
                        },
                    );
                    let msg = format!(
                        "{}🛑 自动重派预算超限，停止自动重派，请人工裁决。\n\n超限项：{breach}{handoff_note}",
                        human_mention
                    );
                    post_review_comment(store, issue_id, &deps.cluster, &msg)?;
                    // 决策审计（重构）：预算超限转人工是终局决策，
                    // 落 escalate_human 记录——重放守卫（review_already_
                    // concluded）据此判「已定案」不再重放（否则每次重启都把
                    // 本单重评审一遍再停一次车，🛑 评论跨重启刷屏）。
                    record_auto_decide(
                        deps,
                        issue_id,
                        "escalate_human",
                        output.verdict.as_str(),
                        serde_json::json!({
                            "round": next_round,
                            "stop_kind": "budget",
                            "breach": breach,
                        }),
                    );
                    info!("[BoardReview] issue {issue_id} 预算超限 → 停止重派转人工（{breach}）");
                    // 触发点①：重派无意义停车 → 重规划起草。
                    crate::board_replan::spawn_replan_draft(
                        deps.clone(),
                        issue_id,
                        "budget_breach",
                    );
                    return Ok(true);
                }
            }
            let target = match pick_redispatch_target(deps, &issue, &dispatches) {
                Ok(c) => {
                    let switched = c.is_switch();
                    let target = c.into_target();
                    if switched {
                        // 开关开时评论带档位注记（「（档位 prefer，
                        // 成功率 5/6）」）——重派决策可审计；关时与现状字节一致。
                        let note = fingerprint_note_text(deps, &issue, &target)
                            .map(|n| format!("（{n}）"))
                            .unwrap_or_default();
                        let _ = post_review_comment(
                            store,
                            issue_id,
                            &deps.cluster,
                            &format!("🔁 连续多轮未通过，本次重派换节点执行 → {target}{note}"),
                        );
                    }
                    target
                }
                Err(e) => {
                    let msg = format!("⛔ 验收未通过，确定重派目标失败：{e}（请人工派发或处置）");
                    store.add_comment(NewComment {
                        issue_id,
                        author: Actor::system("board-review"),
                        content: msg,
                        parent_id: None,
                        ctype: CommentType::System,
                    })?;
                    warn!("[BoardReview] issue {issue_id} 重派目标决策失败：{e}");
                    // 决策审计（重构）：转人工终局决策落记录
                    // （重放守卫据此判已定案，不跨重启反复重评审）。
                    record_auto_decide(
                        deps,
                        issue_id,
                        "escalate_human",
                        output.verdict.as_str(),
                        serde_json::json!({
                            "round": next_round,
                            "stop_kind": "target_pick_failed",
                            "error": e,
                        }),
                    );
                    return Ok(true);
                }
            };
            // 并发让位预检（闭环-13）：评审 LLM 期间其他触发源
            //（重试臂 / 补派波 / 人工）已抢到 claim（在途派发）→ 本轮
            // 重派让位——继续派会撞在途任务，且 Err 臂的「请人工派发」文案
            // 对这种瞬时竞态是误导（数秒后并发触发源自己完成派发）。
            if store.has_active_dispatch(issue_id).unwrap_or(false) {
                info!("[BoardReview] issue {issue_id} FAIL 复核期间已有在途派发接管，重派让位");
                return Ok(true);
            }
            // 发车分臂：target="self" 走单机本地执行面（一等公民，与集群
            // 是否启用无关——"self" 不是集群概念，canonical_peer_id("self")
            // 必然无解，走 core 会以「未知节点：self」⛔ 停车，本地单重派
            // 全断。语义与 WSAPI issue.dispatch / flow 自动
            // 派发的 self 分臂同源；其余目标照旧走集群 core。
            let dispatch_result = if target.eq_ignore_ascii_case("self") {
                nemesis_web::handlers::board::dispatch_issue_local(
                    store,
                    issue_id,
                    &reviewer,
                    Some(&gap),
                )
            } else {
                nemesis_web::handlers::board::dispatch_issue_core(
                    store,
                    Some(&deps.cluster),
                    issue_id,
                    &target,
                    &reviewer,
                    Some(&gap),
                )
            };
            match dispatch_result {
                Ok(_) => {
                    info!("[BoardReview] issue {issue_id} FAIL → 重派 #{next_round} → {target}");
                    // 决策审计挪到实际派发成功后（重构）：此前在
                    // 发车前的检查点之前落「redispatch」记录，estop 停车/
                    // 预算停车/目标决策失败/派发失败的轮次都被记成已重派——
                    // 审计假象，且重启后重放守卫（review_already_concluded）
                    // 误判已定案跳过重放，单据卡死 in_review 无自愈。
                    record_auto_decide(
                        deps,
                        issue_id,
                        "redispatch",
                        output.verdict.as_str(),
                        serde_json::json!({
                            "round": next_round,
                            "unlimited": unlimited,
                            "reasons": render_reasons(&output.reasons),
                            "target": target,
                        }),
                    );
                }
                Err(e) => {
                    // Err 三分诊：estop 中途拉闸 → 尊重冻结（release 后
                    // 复评恢复），不落评论；并发触发源已抢到 claim（预检后
                    // 窗口内）→ 让位，不落评论；其余（无在线节点等）→ ⛔
                    // 留痕请人工。前两臂写 system 评论 = 对瞬时态撒谎，
                    // 且会打断 ⛔/转人工标记链（/停车去重都锚 system 评论）。
                    if deps.estop.is_engaged() {
                        debug!(
                            "[BoardReview] issue {issue_id} 重派受阻（estop 生效，留待 release 后复评）：{e}"
                        );
                        return Ok(false);
                    }
                    if store.has_active_dispatch(issue_id).unwrap_or(false) {
                        info!("[BoardReview] issue {issue_id} 重派竞态让位（在途派发已接管）：{e}");
                        return Ok(true);
                    }
                    let msg = format!("⛔ 验收未通过，自动重派失败：{e}（请人工派发或处置）");
                    store.add_comment(NewComment {
                        issue_id,
                        author: Actor::system("board-review"),
                        content: msg,
                        parent_id: None,
                        ctype: CommentType::System,
                    })?;
                    warn!("[BoardReview] issue {issue_id} 重派失败：{e}");
                    // 决策审计（重构）：⛔ 转人工终局决策落记录
                    // （重放守卫据此判已定案，不跨重启反复重评审）。
                    record_auto_decide(
                        deps,
                        issue_id,
                        "escalate_human",
                        output.verdict.as_str(),
                        serde_json::json!({
                            "round": next_round,
                            "stop_kind": "dispatch_failed",
                            "error": e,
                        }),
                    );
                }
            }
        }
        ReviewAction::EscalateHuman => {
            // 转人工 = 人即将介入，失败账清零（人工处理后计数作废）。
            fail_streaks.clear(issue_id);
            // U-A：停车现场落盘——本臂也是停车路径（零预算 FAIL / 单轮
            // UNSURE 等直接转人工形态），「全部停车分支收口 park_with_
            // handoff 单一真相源」的最后一臂。stop_kind 按 verdict 诚实
            // 分型：UNSURE 单轮≠ streak 闸（unsure），FAIL 预算耗尽≠
            // token 预算（redispatch_budget）。
            let stop_kind = match output.verdict {
                nemesis_board::ReviewVerdict::Unsure => "unsure",
                _ => "redispatch_budget",
            };
            let handoff_note = park_with_handoff(
                deps,
                &issue.number,
                issue_id,
                nemesis_board::handoff::HandoffSpec {
                    stop_kind,
                    suggested_action: "manual_check",
                    hint: format!(
                        "验收无法定案（verdict={}，重派预算 {}/已耗尽状态：round={round}）：人工核查交付与验收意见后复评或重派",
                        output.verdict.as_str(),
                        cfg.max_redispatch
                    ),
                    resume_entry: "review.retrigger（number=NB-x，人工处理后复评）/ issue.dispatch（人工裁决后重派）".into(),
                    environment: last_dispatch_env(&dispatches),
                    attempted: vec![serde_json::json!({
                        "action": "review",
                        "result": format!("verdict={}，按决策表直接转人工", output.verdict.as_str()),
                    })],
                },
            );
            record_auto_decide(
                deps,
                issue_id,
                "escalate_human",
                output.verdict.as_str(),
                serde_json::json!({
                    "round": round,
                    "reasons": render_reasons(&output.reasons),
                }),
            );
            let mut comment = format!("{}🤷 验收 agent 无法定案，请人工裁决\n", human_mention);
            if output.verdict == nemesis_board::ReviewVerdict::Fail {
                // c §③：渲染逐条条目清单（有非 PASS 条目时）；旧格式回落 gap 原文。
                comment.push_str(&format!(
                    "\n重派预算已耗尽（{} 次），最新差距仍在：\n{}\n",
                    cfg.max_redispatch,
                    nemesis_board::render_gap_with_criteria(&output)
                ));
            }
            append_reasons_unless_gapped(&mut comment, &output);
            comment.push_str(&handoff_note);
            store.add_comment(NewComment {
                issue_id,
                author: reviewer.clone(),
                content: comment,
                parent_id: None,
                ctype: CommentType::Comment,
            })?;
            info!(
                "[BoardReview] issue {issue_id} verdict={} → 转人工（保持 in_review）",
                output.verdict.as_str()
            );
            // 触发点①：重派无意义停车（预算耗尽/单轮 UNSURE）
            // → 重规划起草。
            crate::board_replan::spawn_replan_draft(deps.clone(), issue_id, "escalate_human");
        }
    }
    Ok(true)
}

/// 决策审计：`auto_decide` 活动落库（决策流视图 `board.audit.list` 的
/// 数据源；`rollback_decision` 只认本 action）。details = JSON：
/// `{decision, verdict,..extra}` 合并 + B33 授权血统（单子带 origin 时
/// 附 `origin_type`/`origin_id`；autopilot 单附 `rule`=`{id}@{updated_at}`
/// 血统戳，从单子行读、不随规则现状态漂移，D1）。失败只 warn（审计是
/// 增值动作，不炸验收）。
/// 冲突四审计（conflict/conflict_auto_resolve/conflict_redispatch/
/// conflict_switch_worker）同走本漏斗——board 冲突防线与验收共用单一审计。
/// d：同一漏斗同步落交付账本（review 行 + issue scope 分支闭环，
/// [`crate::review_ledger`]）——审计漏斗单一，证据链同步不漂移。
pub(crate) fn record_auto_decide(
    deps: &BoardReviewDeps,
    issue_id: i64,
    decision: &str,
    verdict: &str,
    extra: serde_json::Value,
) {
    let store = deps.store.as_ref();
    let node_id = deps.cluster.node_id();
    let mut details = serde_json::json!({ "decision": decision, "verdict": verdict });
    if let (Some(obj), Some(ex)) = (details.as_object_mut(), extra.as_object()) {
        for (k, v) in ex {
            obj.insert(k.clone(), v.clone());
        }
    }
    // B33 授权血统（D1 拍板：血统从单子 origin 读，非规则现状态）——
    // 单子出生时 origin_meta 已烧入 `rule_version={id}@{updated_at}` 与
    // `mode`（见 plan.rs autopilot_new_issue），本漏斗统一把它带上审计行：
    // 重派/验收/转人工沿用旧授权延续语义，血统戳取自单子行（不可变），
    // 规则后续改名/停用/编辑都不追溯在途。autopilot 单额外带 `rule`
    // 血统字段（`{id}@{updated_at}` 形态）；其余来源带 origin_type/
    // origin_id；无 origin 的手工单/旧单不写字段（诚实缺席，不伪造）。
    let issue_fetched = store.get_issue(issue_id).ok();
    if let (Some(obj), Some(origin)) = (
        details.as_object_mut(),
        issue_fetched.as_ref().and_then(|i| i.origin.as_ref()),
    ) {
        obj.insert(
            "origin_type".to_string(),
            serde_json::Value::String(origin.origin_type.clone()),
        );
        obj.insert(
            "origin_id".to_string(),
            serde_json::Value::String(origin.origin_id.clone()),
        );
        if origin.origin_type == "autopilot"
            && let Some(v) = origin.origin_meta.get("rule_version")
        {
            obj.insert("rule".to_string(), serde_json::Value::String(v.clone()));
        }
    }
    if let Err(e) = store.add_activity(
        issue_id,
        &Actor::agent(node_id),
        "auto_decide",
        Some(&details.to_string()),
    ) {
        warn!("[BoardReview] issue {issue_id} auto_decide 活动落库失败：{e}");
    }
    // C 里程碑 4（看板项目档案 goal）：评审决策 → docs/review/NB-xx.md +
    // timeline。写失败不阻塞评审（writer 内部 WARN+审计）；存量项目静默跳过。
    if let Some(issue) = issue_fetched.as_ref() {
        nemesis_board::archive_writer::write_review_milestone(
            store, issue, node_id, decision, verdict, &details,
        );
    }
    // d：交付账本闭环（review 行 + auto_accept/redispatch 分支裁决）。
    // 失败容错——证据面缺席不停评审。
    crate::review_ledger::review_outcome_delivery(
        deps,
        deps.cluster.task_db().as_ref(),
        issue_id,
        decision,
        verdict,
        &details,
    );
}

#[cfg(test)]
mod tests;

// ---------------------------------------------------------------------------
// 机械拆分：按域分文件。纯代码搬家，行为零变更；
// 各子模块项经 pub use 保持原 board_review:路径不变。
// ---------------------------------------------------------------------------
mod anchors;
mod decision;
mod evidence;
mod llm_review;
mod parent_project;
mod selfcheck;

// 各子模块项经 pub(crate) use 保持原 board_review:crate 内路径不变
// （子模块均无 pub 项，pub use 会报重导出可见性告警）。
pub(crate) use anchors::*;
pub(crate) use decision::*;
pub(crate) use evidence::*;
pub(crate) use llm_review::*;
pub(crate) use parent_project::*;
pub(crate) use selfcheck::*;
