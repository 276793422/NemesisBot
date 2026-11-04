//! target="self" 单机本地执行器（网关装配面）。
//!
//! 看板派发 `target="self"` = 本机 AgentLoop 直驱（一等公民，不走集群 RPC
//! 回环）：执行器把 dispatch prompt 以 InboundMessage 上总线（channel="web"
//! 既有 proven 路径；`session_key = board_dispatch:{task_id}`），**先订阅
//! 后发布**按 `meta.session_key` 捕获最终回复，写回看板——镜像集群回调
//! Route 0 全链（write_back → spawn_board_review → R-9 重估波）。estop
//! 触发/超时/总线关闭 = 诚实 FAILED 写回，不静默吞。
//!
//! 项目解析（执行根裁决）：issue.project_id → board 项目 directory →
//! L6++ registry canonical 匹配 → `bind_session`（执行根 = 项目目录）；
//! 无匹配 = 主 loop 回落 + WARN（执行根 = 主 workspace）——回落是诚实
//! 降级，不猜不静默换臂。
//!
//! 交付捕获（BUG 2026-11-05 根修）：绑定成功臂发车前钉项目目录 HEAD 为
//! 交付基线，completed 收口时从项目目录就地 diff（复用 exec 捕获内核）+
//! 落账 deliveries 表 `{task_id}:branch`（集群回调链同款 record + 审计
//! shim）——publish 步的查账键此前对 `local-` 任务恒空（唯一落账方是集群
//! 回调，本地链路完全没有捕获管线），publish 恒诚实 failed。任何捕获
//! 失败 = WARN 不阻断写回主链路（交付证据缺席可观测）。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use nemesis_bus::MessageBus;
use nemesis_types::channel::{InboundMessage, OutboundMessage};

use nemesis_web::handlers::board::{LocalDispatchExecutor, LocalDispatchRequest};

/// 本地派发超时缺省（与 board.dispatch_timeout_secs 配置缺省一致）。
const DEFAULT_TIMEOUT_SECS: u64 = 3600;
/// 0=关表时的硬顶（执行器兜底不能跟「不限」走——与集群 stale 安全网
/// max(24h, …) 有界化同纪律；本地单任务 2h 足够宽）。
const HARD_CAP_SECS: u64 = 2 * 60 * 60;

/// 网关侧本地执行器：捕获总线 / 项目 loop 管理器 / 看板 store / 评审
/// 依赖集。全字段 Arc 化可 Clone——submit 同步面快照一份带进 spawn。
#[derive(Clone)]
pub(crate) struct GatewayLocalExecutor {
    bus: Arc<MessageBus>,
    projects_manager: Arc<crate::projects::manager::ProjectLoopManager>,
    board_store: Option<Arc<nemesis_board::BoardStore>>,
    workspace: PathBuf,
    home: PathBuf,
    moderator_loop: Arc<std::sync::OnceLock<Arc<nemesis_agent::r#loop::AgentLoop>>>,
    cluster: Arc<nemesis_cluster::cluster::Cluster>,
    estop: Arc<nemesis_agent::estop::EstopState>,
    /// estop 冻结停车的 (种类, issue_id) 队列（与评审闭包共享同一份）。
    estop_parked: Arc<std::sync::Mutex<Vec<(crate::board_review::ParkedKind, i64)>>>,
    selfcheck: crate::board_review::SelfcheckRegistry,
    /// 交付账本连接（BUG 2026-11-05 根修）：completed 收口捕获落账
    /// `{task_id}:branch`（publish 查账键）。打不开 = 捕获缺席可观测，
    /// 不炸装配。
    task_db: Option<Arc<nemesis_taskstore::TaskDb>>,
    #[cfg(feature = "security")]
    security_slot:
        Option<Arc<std::sync::OnceLock<Option<Arc<nemesis_security::pipeline::SecurityPlugin>>>>>,
}

/// 交付基线（submit 发车前钉住，带进等待任务）：基线 commit 是 diff
/// 基准（agent 合法 commit 移动 HEAD，HEAD 当基线 = diff 恒空失真，
/// delivery_gate T1 同族）；repo_dir 是捕获对象（执行根 = 项目目录）。
#[derive(Clone)]
struct DeliveryBaseline {
    repo_dir: PathBuf,
    baseline_commit: String,
}

impl GatewayLocalExecutor {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        bus: Arc<MessageBus>,
        projects_manager: Arc<crate::projects::manager::ProjectLoopManager>,
        board_store: Option<Arc<nemesis_board::BoardStore>>,
        workspace: PathBuf,
        home: PathBuf,
        moderator_loop: Arc<std::sync::OnceLock<Arc<nemesis_agent::r#loop::AgentLoop>>>,
        cluster: Arc<nemesis_cluster::cluster::Cluster>,
        estop: Arc<nemesis_agent::estop::EstopState>,
        estop_parked: Arc<std::sync::Mutex<Vec<(crate::board_review::ParkedKind, i64)>>>,
        selfcheck: crate::board_review::SelfcheckRegistry,
        task_db: Option<Arc<nemesis_taskstore::TaskDb>>,
        #[cfg(feature = "security")] security_slot: Option<
            Arc<std::sync::OnceLock<Option<Arc<nemesis_security::pipeline::SecurityPlugin>>>>,
        >,
    ) -> Self {
        Self {
            bus,
            projects_manager,
            board_store,
            workspace,
            home,
            moderator_loop,
            cluster,
            estop,
            estop_parked,
            selfcheck,
            task_db,
            #[cfg(feature = "security")]
            security_slot,
        }
    }

    /// issue.project_id → board 项目 directory → L6++ registry canonical
    /// 匹配。Ok(Some((id, name, dir))) = 可绑定（dir = 项目目录，交付
    /// 捕获对象）；Ok(None) = 无对应项目（回落主 loop）；Err = 解析 IO
    /// 失败（同样回落，但留 WARN 细节）。
    fn resolve_project_session(
        &self,
        issue_id: i64,
    ) -> Result<Option<(String, String, PathBuf)>, String> {
        let Some(store) = self.board_store.as_ref() else {
            return Ok(None);
        };
        let issue = store.get_issue(issue_id)?;
        let Some(pid) = issue.project_id else {
            return Ok(None);
        };
        let project = store.get_project(pid)?;
        let Some(directory) = project
            .directory
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
        else {
            return Ok(None);
        };
        // 与注册链同一 canonical 表示（剥 verbatim），逐字节比较。
        let target = nemesis_path::paths::canonicalize_for_compare(std::path::Path::new(directory));
        let entry = crate::projects::registry::list_projects(self.projects_manager.registry_path())
            .into_iter()
            .find(|p| p.path == target);
        Ok(entry.map(|e| (e.id, e.name, target)))
    }

    /// 交付捕获 + 落账（BUG 2026-11-05 根修：镜像集群回调记账语义——本地的
    /// 「回调」对应物就是 completed 写回）。基线透传 → exec 捕获内核就地
    /// diff（patch 留底 + wire 组装与集群同一函数）→ deliveries 表
    /// `{task_id}:branch`（publish 查账键）。无基线（绑定回落主 loop /
    /// 非 git 目录）= 无捕获：执行根不在项目目录时从项目目录 diff 是
    /// 错位证据，publish 诚实报无可发布物。
    fn capture_delivery(&self, task_id: &str, baseline: Option<&DeliveryBaseline>) {
        let Some(b) = baseline else {
            tracing::warn!(
                "[LocalDispatch] task {task_id} 无交付基线（项目解析失败/非 git 目录/绑定回落主 loop）——交付捕获缺席，publish 将诚实报无可发布物"
            );
            return;
        };
        let node_id = self.cluster.node_id().to_string();
        #[cfg(feature = "security")]
        let security_plugin = self
            .security_slot
            .as_ref()
            .and_then(|slot| slot.get())
            .and_then(|opt| opt.as_ref());
        let recorded = capture_and_record_local_delivery(
            self.task_db.as_ref(),
            &b.repo_dir,
            &self.workspace,
            task_id,
            &node_id,
            &b.baseline_commit,
            #[cfg(feature = "security")]
            security_plugin,
        );
        if recorded {
            tracing::info!("[LocalDispatch] task {task_id} 交付已捕获入账（{task_id}:branch）");
        }
    }

    /// 写回 + 评审触发 + R-9 重估波（镜像 cluster_init.rs 回调 Route 0）。
    /// `generation` = 派发代际号回声（AOR 批二a ④ 写回闸拒绝旧代迟到回调）。
    fn finish(
        &self,
        task_id: &str,
        status: &str,
        response: &str,
        fail_class: &str,
        generation: Option<i64>,
    ) {
        let outcome = crate::commands::gateway::write_back_board_dispatch(
            &self.board_store,
            &self.workspace,
            task_id,
            status,
            response,
            fail_class,
            generation,
        );
        tracing::info!(
            "[LocalDispatch] task {task_id} 写回：is_board_task={} settled={} status={status}",
            outcome.is_board_task,
            outcome.settled
        );
        if let Some(review_issue_id) = outcome.issue_for_review
            && let Some(store) = self.board_store.as_ref()
        {
            crate::board_review::spawn_board_review(
                crate::board_review::BoardReviewDeps {
                    store: store.clone(),
                    workspace: self.workspace.clone(),
                    home: self.home.clone(),
                    moderator_loop: self.moderator_loop.clone(),
                    cluster: self.cluster.clone(),
                    estop: self.estop.clone(),
                    estop_parked: self.estop_parked.clone(),
                    selfcheck: self.selfcheck.clone(),
                    #[cfg(feature = "security")]
                    security_slot: self.security_slot.clone(),
                },
                review_issue_id,
            );
        }
        if outcome.settled
            && !self.estop.is_engaged()
            && let Some(store) = self.board_store.as_ref()
        {
            let store = store.clone();
            let actor = nemesis_board::Actor::system("board");
            let cluster = self.cluster.clone();
            tokio::spawn(async move {
                let (_cands, dispatched, failed) =
                    nemesis_web::handlers::board::sweep_parked_dispatches(&store, &cluster, &actor);
                if dispatched > 0 || failed > 0 {
                    tracing::info!("[LocalDispatch] R-9 重估波：重派 {dispatched} / 失败 {failed}");
                }
            });
        }
    }

    /// 等最终回复 → 写回终态。submit 返回后本任务独立运行；写回有终态
    /// 幂等守卫（write_back 内部），重复触发无害。
    async fn wait_and_write_back(
        self,
        task_id: String,
        session_key: String,
        baseline: Option<DeliveryBaseline>,
        generation: Option<i64>,
        mut rx: tokio::sync::broadcast::Receiver<OutboundMessage>,
        mut estop_rx: tokio::sync::watch::Receiver<bool>,
    ) {
        let timeout_secs = local_dispatch_timeout_secs();
        let deadline = if timeout_secs == 0 {
            Duration::from_secs(HARD_CAP_SECS)
        } else {
            Duration::from_secs(timeout_secs)
        };

        let reply = tokio::select! {
            r = capture_reply(&mut rx, &session_key) => r,
            _ = wait_estop(&mut estop_rx) => {
                // estop 触发：诚实 FAILED 写回（不静默吞）——estop_parked
                // 停车机制自然接管后续复评恢复。
                self.finish(
                    &task_id,
                    "error",
                    "本地执行中止：estop 急停触发",
                    "estop",
                    generation,
                );
                return;
            }
            _ = tokio::time::sleep(deadline) => {
                let shown = if timeout_secs == 0 { HARD_CAP_SECS } else { timeout_secs };
                self.finish(
                    &task_id,
                    "error",
                    &format!("本地执行超时（{shown}s）无最终回复"),
                    "timeout",
                    generation,
                );
                return;
            }
        };

        match reply {
            Some(content) => {
                // 捕获落账先行于写回（写回触发评审，账本先 settled）。
                // 镜像集群纪律：只在成功收口捕获，任何失败 WARN 不阻断写回。
                self.capture_delivery(&task_id, baseline.as_ref());
                self.finish(&task_id, "completed", &content, "", generation);
            }
            // 总线关闭 = 网关正在停机：诚实报错（重启路径无本执行器内存
            // 态，看板单留 FAILED 由人工/复评接管）。
            None => self.finish(
                &task_id,
                "error",
                "出站总线已关闭（网关停机），任务无最终回复",
                "error",
                generation,
            ),
        }
    }
}

impl LocalDispatchExecutor for GatewayLocalExecutor {
    fn submit(&self, req: LocalDispatchRequest) -> Result<(), String> {
        let session_key = format!("board_dispatch:{}", req.task_id);

        // 项目解析（执行根裁决）。绑定失败/无匹配都回落主 loop——执行
        // 照常，只留 WARN 注明执行根实际位置。绑定成功臂钉交付基线
        // （执行根 = 项目目录，改动落项目目录，捕获才有意义；回落臂无
        // 基线 = 无交付捕获，publish 诚实报无可发布物）。
        let mut baseline: Option<DeliveryBaseline> = None;
        match self.resolve_project_session(req.issue_id) {
            Ok(Some((pid, name, dir))) => {
                match self.projects_manager.bind_session(&session_key, &pid) {
                    Ok(_) => {
                        tracing::info!(
                            "[LocalDispatch] task {} → 项目 loop（{pid} / {name}），执行根 = 项目目录",
                            req.task_id
                        );
                        baseline = pin_local_baseline(&dir, &req.task_id);
                    }
                    Err(e) => tracing::warn!(
                        "[LocalDispatch] task {} 项目绑定失败（{pid}）：{e} —— 回落主 loop",
                        req.task_id
                    ),
                }
            }
            Ok(None) => tracing::warn!(
                "[LocalDispatch] task {} 无对应 L6++ 项目（issue 无项目 / 项目目录未注册）——回落主 loop",
                req.task_id
            ),
            Err(e) => tracing::warn!(
                "[LocalDispatch] task {} 项目解析失败：{e} —— 回落主 loop",
                req.task_id
            ),
        }

        // 先订阅后发布：出站订阅先于入站消息建立，最终回复帧（meta
        // .session_key = session_key，loop/bus.rs 最终回复盖章）不丢首帧。
        let rx = self.bus.subscribe_outbound();

        self.bus.publish_inbound(InboundMessage {
            channel: "web".to_string(),
            sender_id: "board".to_string(),
            chat_id: req.task_id.clone(),
            content: req.prompt.clone(),
            media: Vec::new(),
            session_key: session_key.clone(),
            correlation_id: String::new(),
            metadata: HashMap::new(),
            voice_playback: None,
        });
        tracing::info!(
            "[LocalDispatch] task {}（issue {}）已入总线（session {session_key}），等待最终回复",
            req.task_id,
            req.issue_number
        );

        // 异步等待 + 写回 + 评审触发。submit 立即返回——派发链语义是
        // 「发车成功」，执行结果经写回链落看板（镜像 RPC ACK 后异步回报）。
        let estop_rx = self.estop.subscribe();
        let me = self.clone();
        tokio::spawn(async move {
            me.wait_and_write_back(
                req.task_id,
                session_key,
                baseline,
                req.generation,
                rx,
                estop_rx,
            )
            .await
        });
        Ok(())
    }
}

/// 按 session_key 捕获最终回复（首条匹配且非回执即返回）。receipt=true
/// 的出站是系统回执（busy 排队 ⏳ / slash 确认 / 压缩·限流进度等，生产者
/// 打标单一真相源）——NB-1 实录：queue-ack 被当终局回复 → 57ms 空交付
/// completed，真回复随后无人认领成孤儿。跳过回执继续等真回复。Lagged
/// 继续等（丢帧只影响中间流量——最终回复是最后一条，等下一条匹配即可）；
/// Closed = 网关停机。
async fn capture_reply(
    rx: &mut tokio::sync::broadcast::Receiver<OutboundMessage>,
    session_key: &str,
) -> Option<String> {
    loop {
        match rx.recv().await {
            Ok(msg) => {
                if msg.meta.receipt {
                    continue;
                }
                if msg.meta.session_key.as_deref() == Some(session_key) {
                    return Some(msg.content);
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!("[LocalDispatch] 出站流 lagged（丢 {n} 帧），继续等待");
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
        }
    }
}

/// estop 触发等待：只在 true 态返回（release 边沿不误报）；发送端丢弃
/// （网关停机）挂起等待——外层 capture/sleep 分支兜底。
async fn wait_estop(estop_rx: &mut tokio::sync::watch::Receiver<bool>) {
    loop {
        if estop_rx.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
        if *estop_rx.borrow() {
            return;
        }
    }
}

/// 本地派发超时（config.json board.dispatch_timeout_secs；0=关表 → 调用
/// 方按硬顶处理）。每次 submit 现读——改配置即时生效（与评审配置同纪律）。
fn local_dispatch_timeout_secs() -> u64 {
    nemesis_config::load_live()
        .and_then(|c| c.board)
        .map(|b| b.dispatch_timeout_secs)
        .unwrap_or(DEFAULT_TIMEOUT_SECS)
}

/// 发车前钉交付基线（项目目录 HEAD commit，纯读——用户项目目录零副作用；
/// exec 的 `.git/nemesis_baseline` 持久锚是 bot 自有副本语义，不搬过来）。
/// 非 git 目录 / 文件形态 .git（worktree 链接）/ 空仓（无 HEAD commit）=
/// None + WARN（交付捕获缺席可观测，不阻断发车；镜像 ensure_exec_repo
/// 对文件形态 .git 的裁决）。
fn pin_local_baseline(project_dir: &std::path::Path, task_id: &str) -> Option<DeliveryBaseline> {
    let dot_git = project_dir.join(".git");
    if !dot_git.is_dir() {
        tracing::warn!(
            "[LocalDispatch] task {task_id} 项目目录 {} 非 git 仓库（无 .git 目录）——交付捕获缺席",
            project_dir.display()
        );
        return None;
    }
    let repo = match git2::Repository::open_ext(
        project_dir,
        git2::RepositoryOpenFlags::NO_SEARCH,
        std::path::Path::new("/"),
    ) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("[LocalDispatch] task {task_id} 项目仓库打开失败：{e}——交付捕获缺席");
            return None;
        }
    };
    match repo.head().ok().and_then(|head| head.peel_to_commit().ok()) {
        Some(commit) => Some(DeliveryBaseline {
            repo_dir: project_dir.to_path_buf(),
            baseline_commit: commit.id().to_string(),
        }),
        None => {
            tracing::warn!(
                "[LocalDispatch] task {task_id} 项目仓库无 HEAD commit（空仓/未初始）——交付捕获缺席"
            );
            None
        }
    }
}

/// 本地交付捕获 + 落账（capture → record 两步合一，`pub(crate)` 供回归锁
/// 直测）。基线透传锚定（submit 发车前钉住的 HEAD——agent 合法 commit
/// 移动 HEAD 后 HEAD 当基线会失真，delivery_gate T1 同族）→ exec 捕获
/// 内核 [`crate::delivery_gate::capture_task_delivery_exec`]（就地 diff +
/// patch 留底 + wire 组装单一真相源；本地无 checks 捕获接线，诚实缺席）
/// → 落账走集群回调链同款 record（deliveries 表 `{task_id}:branch` =
/// publish 查账键）+ 审计 shim 同源。true = 已入账（含 capture 后落库；
/// capture 缺席 = false + WARN）。
#[allow(clippy::too_many_arguments)]
pub(crate) fn capture_and_record_local_delivery(
    task_db: Option<&std::sync::Arc<nemesis_taskstore::TaskDb>>,
    repo_dir: &std::path::Path,
    workspace_root: &std::path::Path,
    task_id: &str,
    node_id: &str,
    baseline_commit: &str,
    #[cfg(feature = "security")] security_plugin: Option<
        &std::sync::Arc<nemesis_security::pipeline::SecurityPlugin>,
    >,
) -> bool {
    let Some(delivery) = crate::delivery_gate::capture_task_delivery_exec(
        repo_dir,
        workspace_root,
        task_id,
        node_id,
        Vec::new(),
        Some(baseline_commit),
    ) else {
        tracing::warn!(
            "[LocalDispatch] task {task_id} 交付捕获失败（capture 层缺席）——publish 将诚实报无可发布物"
        );
        return false;
    };
    #[cfg(feature = "security")]
    crate::commands::gateway::record_branch_delivery_audited(
        task_db,
        security_plugin,
        node_id,
        task_id,
        Some(&delivery),
    );
    #[cfg(not(feature = "security"))]
    crate::commands::gateway::record_branch_delivery(task_db, node_id, task_id, Some(&delivery));
    true
}

#[cfg(test)]
mod tests;
