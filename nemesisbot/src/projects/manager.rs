//! 项目 loop 生命周期管理器（L6++ M2，2026-09-08）。
//!
//! 职责：按 projects registry 启动每个项目的常驻 AgentLoop（per-call mpsc
//! 入站通道，与主 loop 的 adapter 同构），维护「会话 → 项目」归属索引，提供
//! 优雅停机。bus 路由消费（主桥 skip 谓词 + 项目调度转发）在 G3 接线；本模块
//! 只提供路由纯函数与索引数据。
//!
//! 归属真相源 = meta sidecar（`{session_stem}.meta.json` 的 project_id/
//! project_path，G1 落地）；owner_index 是启动期从 sidecar 扫出的内存缓存，
//! M4 的 sessions.create / fork 增量维护。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tracing::{info, warn};

use nemesis_agent::session::SessionStore;
use nemesis_types::channel::{InboundMessage, OutboundMessage};

use super::registry::{self, ProjectEntry};
use crate::agent_factory::SharedResources;

#[cfg(test)]
mod tests;

// ---------------------------------------------------------------------------
// 路由决策纯函数（G2 门测试对象；G3 主桥 skip 谓词与项目调度共用）
// ---------------------------------------------------------------------------

/// 一条入站消息的去向。三值与 impl plan M2 的桥过滤语义一一对应：
/// - `System`：`channel=="system"` 的系统消息（cluster_continuation /
///   subagent_continuation / heartbeat 等内部回灌）——项目调度丢弃，主桥
///   照常消费（系统语义只属于主 loop）。
/// - `ToProject(pid)`：归属索引命中——项目调度转发到对应项目 loop，主桥 skip。
/// - `ToMain`：无归属（对话组会话 / 纯新会话）——主桥放行，项目调度丢弃。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteDecision {
    ToProject(String),
    ToMain,
    System,
}

/// 桥过滤纯函数：`owner` 是归属索引对该消息 session_key 的查询结果
/// （None = 无绑定）。可穷举测试（G2 门）。
pub fn route_decision(msg: &InboundMessage, owner: Option<&str>) -> RouteDecision {
    if msg.channel == "system" {
        return RouteDecision::System;
    }
    // history 只读查询豁免（BUG 2026-09-23 第二道防线）：历史数据本就全在
    // 主 workspace `logs/session_logs/`，与项目 loop 的存亡/忙闲无关——即使
    // 会话带项目归属，history 也必须回主 loop 路径（主 loop 的
    // handle_history_request 就地读 chat_log 应答），绝不能被项目路由劫持
    // 成「项目不可用」错误或排进项目 loop 串行队列。web 咽喉点的入站过滤
    // 链（HistoryFilter）是第一道（消息根本不进 bus）；本臂保证任何绕过
    // 链到达 bus 的 history（遗留调用方/其他发布方）同样走主 loop。
    if msg.metadata.get("request_type").map(String::as_str) == Some("history") {
        return RouteDecision::ToMain;
    }
    match owner {
        Some(pid) => RouteDecision::ToProject(pid.to_string()),
        None => RouteDecision::ToMain,
    }
}

/// meta 文件 stem → 完整 session_key。镜像 `nemesis-web` handlers/logs.rs
/// 扫描端的还原约定（单一语义，两处实现必须同步改——web 固定前缀精确还原
/// `agent_main_session_` → `agent:main:session:{rest}`，其余 naive 全量
/// `_`→`:`；chat_log 存盘时 `:` 落盘成 `_`，带下划线的合法 sid 靠前缀区分）。
pub fn session_key_from_stem(stem: &str) -> String {
    if let Some(rest) = stem.strip_prefix("agent_main_session_") {
        format!("agent:main:session:{rest}")
    } else {
        stem.replace('_', ":")
    }
}

/// session_key → chat_log/meta 文件 stem（[`session_key_from_stem`] 的
/// 正向逆变换）：白名单消毒（SAN-05 单一真相源）。web 形态
/// `agent:main:session:{sid}` 自然还原成带 `agent_main_session_` 前缀的
/// stem；IM 会话 naive 还原（与 chat_log 落盘约定一致）；B 端含 `/` 复合
/// 键映射为平面 stem（SAN-01——旧 `replace(':')` 会嵌套出目录）。
pub fn stem_from_session_key(session_key: &str) -> String {
    nemesis_utils::sanitize::sanitize_path_segment(session_key)
}

// ---------------------------------------------------------------------------
// ProjectLoopManager
// ---------------------------------------------------------------------------

struct ProjectLoopHandle {
    agent_loop: Arc<nemesis_agent::r#loop::AgentLoop>,
    agent_task: tokio::task::JoinHandle<()>,
}

/// 项目常驻 loop 的生命周期 + 归属索引 + 路由管理。gateway 持一个实例
/// （主 loop 构建后创建），装配期 `start_routing()`（全进程唯一调度订阅）
/// + teardown 时 `stop_all`。
pub struct ProjectLoopManager {
    shared: Arc<SharedResources>,
    registry_path: PathBuf,
    main_workspace: PathBuf,
    session_store: Arc<SessionStore>,
    /// 消息总线（调度订阅 + 不可用错误出站）。
    bus: Arc<nemesis_bus::MessageBus>,
    rt: tokio::runtime::Handle,
    state: Mutex<HashMap<String, ProjectLoopHandle>>,
    /// pid → 项目 loop 的 mpsc 发送端（调度转发的目标）。与 `state` 分离：
    /// 路由只依赖通道，不依赖 AgentLoop 实例（测试可注入裸通道）。spawn
    /// 成对插入、stop 成对摘除，两表无跨表不变量。
    channels: Mutex<HashMap<String, tokio::sync::mpsc::Sender<InboundMessage>>>,
    /// start_routing 幂等闸（全进程唯一 1 个项目调度订阅）。
    routing_started: AtomicBool,
    /// session_key → project_id（启动期从 meta sidecar 扫出 + 运行期回填/
    /// 增量维护；miss 时回读 sidecar 兜底——见 `owner_of`）。
    owner_index: parking_lot::RwLock<HashMap<String, String>>,
}

impl ProjectLoopManager {
    pub fn new(
        shared: Arc<SharedResources>,
        session_store: Arc<SessionStore>,
        bus: Arc<nemesis_bus::MessageBus>,
    ) -> Self {
        let main_workspace = shared.workspace_dir();
        let registry_path = registry::registry_path(&main_workspace);
        Self {
            shared,
            registry_path,
            main_workspace,
            session_store,
            bus,
            rt: tokio::runtime::Handle::current(),
            state: Mutex::new(HashMap::new()),
            channels: Mutex::new(HashMap::new()),
            routing_started: AtomicBool::new(false),
            owner_index: parking_lot::RwLock::new(HashMap::new()),
        }
    }

    /// 启动全部项目 loop（gateway 装配期调用一次）。注册表缺失/损坏走
    /// lenient 读（空表），目录消失的项目 warn + skip 不炸启动。同时从
    /// meta sidecar 扫出归属索引。
    pub fn start_all(&self) {
        self.reload_owner_index();
        let entries = registry::list_projects(&self.registry_path);
        if entries.is_empty() {
            return;
        }
        info!("[ProjectLoops] starting {} project loop(s)", entries.len());
        for entry in &entries {
            if let Err(e) = self.spawn_project(entry) {
                warn!(
                    id = %entry.id,
                    name = %entry.name,
                    path = %entry.path.display(),
                    "[ProjectLoops] 项目 loop 启动失败: {e}"
                );
            }
        }
    }

    /// 启动单个项目 loop。幂等（已在运行 = Ok）；目录消失诚实报错。
    pub fn spawn_project(&self, entry: &ProjectEntry) -> Result<(), String> {
        if !entry.path.is_dir() {
            return Err(format!("项目目录不存在: {}", entry.path.display()));
        }
        {
            let state = self.state.lock().unwrap();
            if state.contains_key(&entry.id) {
                return Ok(());
            }
        }
        let agent_loop = crate::agent_factory::build_project_agent_loop(
            &self.shared,
            entry,
            self.session_store.clone(),
        )
        .map_err(|e| format!("构建项目 loop 失败: {e}"))?;
        // B9 commit 闸（turn 末 VM→宿主回传 + 基线刷新）：全项目 loop 无差别
        // 注册（触发期自检——VM 档是 agents 表运行期属性，非 VM 项目零开销
        // 穿越；supervisor 未装配时同样零开销穿越）。
        #[cfg(feature = "vm")]
        agent_loop.add_lifecycle_hook(std::sync::Arc::new(crate::vm_sync::VmCommitHook {
            project_id: entry.id.clone(),
            project_path: entry.path.clone(),
            main_workspace: self.main_workspace.clone(),
        }));
        // 与主 loop 的 AgentLoopServiceAdapter 同构：per-loop mpsc 入站通道
        // + reinject tx（queue-drain 回灌走同一条路）+ 后台任务消费。
        let (inbound_tx, inbound_rx) = tokio::sync::mpsc::channel(1024);
        agent_loop.set_reinject_tx(inbound_tx.clone());
        let loop_for_task = agent_loop.clone();
        let agent_task = self.rt.spawn(async move {
            loop_for_task.run_bus_arc(inbound_rx).await;
        });
        let mut state = self.state.lock().unwrap();
        if state.contains_key(&entry.id) {
            // 极小窗口的双 spawn（Phase 1 单调用方不触发）；输者清理自己。
            drop(state);
            agent_loop.stop();
            agent_loop.clear_session_busy();
            agent_task.abort();
            return Ok(());
        }
        state.insert(
            entry.id.clone(),
            ProjectLoopHandle {
                agent_loop,
                agent_task,
            },
        );
        drop(state);
        self.channels
            .lock()
            .unwrap()
            .insert(entry.id.clone(), inbound_tx.clone());
        // 四轮复查收尾（与 agents/loop_manager 同款）：stop_project 可在
        // 本函数「state 插入之后、channels 插入之前」整体穿过，此处插入即
        // 成孤儿 tx——`route_one` 经 `project_inbound_tx` 直读 channels 表
        // 不查 state，孤儿 tx 会收到发送，但其接收端已随 stop 的 abort 关
        // 闭 → `tx.send` 失败走 `send_unroutable_error` 诚实报错（功能面
        // 不受影响），但条目会残留到 teardown。复查收拾：state 已无本项目
        // = 撞上了 stop，把刚插入的 tx 摘回。`same_channel` 收窄为只摘自
        // 己插入的那条——项目 id 是稳定 id（remove 后重注册同 id 并发
        // spawn 合法形态），裸 remove 可能误摘新 loop 的通道。
        let orphaned = !self.state.lock().unwrap().contains_key(&entry.id);
        if orphaned {
            let mut chans = self.channels.lock().unwrap();
            if chans
                .get(&entry.id)
                .is_some_and(|cur| cur.same_channel(&inbound_tx))
            {
                chans.remove(&entry.id);
            }
        }
        info!(
            id = %entry.id,
            name = %entry.name,
            path = %entry.path.display(),
            tools = 0, // 真实工具数在工厂日志里；此处避免再借 loop
            "[ProjectLoops] 项目 loop 已启动"
        );
        Ok(())
    }

    /// 停止单个项目 loop（先摘表再停——路由摘除语义，G3/G4 复用）。
    /// 返回是否确有对应实例被停止。
    pub fn stop_project(&self, project_id: &str) -> bool {
        let handle = self.state.lock().unwrap().remove(project_id);
        // 调度转发端同步摘除：此后 route_one 对该项目走「不可用」诚实报错。
        self.channels.lock().unwrap().remove(project_id);
        let Some(handle) = handle else {
            return false;
        };
        handle.agent_loop.stop();
        handle.agent_loop.clear_session_busy();
        // 镜像 AgentLoopServiceAdapter::stop：用户显式停止，abort 掉在途
        // 轮次（未完成的回复可接受）；session busy 已清，不产生死锁会话。
        handle.agent_task.abort();
        info!(id = %project_id, "[ProjectLoops] 项目 loop 已停止");
        true
    }

    /// 停止全部项目 loop（gateway teardown）。
    pub fn stop_all(&self) {
        let ids: Vec<String> = self.state.lock().unwrap().keys().cloned().collect();
        for id in ids {
            self.stop_project(&id);
        }
    }

    /// 在运行的项目 loop 的 project_id 列表。
    pub fn running_pids(&self) -> Vec<String> {
        self.state.lock().unwrap().keys().cloned().collect()
    }

    /// 项目 loop 是否在运行（bridge list 的 running 字段用）。
    pub fn is_running(&self, project_id: &str) -> bool {
        self.state.lock().unwrap().contains_key(project_id)
    }

    /// 项目目录（注册表查询；未注册 = None）。fs.tree / todo 根切换用。
    pub fn project_path(&self, project_id: &str) -> Option<PathBuf> {
        registry::find_project(&self.registry_path, project_id).map(|e| e.path)
    }

    /// 项目数上限（`config.json` 的 `projects.max`，create 时实时读——
    /// 与 tier 热重载同一哲学：改配置即生效，无需重启）。读取失败回退
    /// 默认 4 + warn（注册表校验层才是 loud 层）。
    fn max_projects(&self) -> usize {
        let path = self.shared.home.join("config.json");
        match nemesis_config::load_config(&path) {
            Ok(cfg) => cfg
                .projects
                .map(|p| p.max)
                .unwrap_or_else(|| nemesis_config::ProjectsConfig::default().max),
            Err(e) => {
                warn!("[ProjectLoops] 读取 config projects.max 失败，回退默认 4: {e}");
                nemesis_config::ProjectsConfig::default().max
            }
        }
    }

    /// 项目会话绑定（G4 sessions.create 带 project_id）：校验项目存在 +
    /// 目录在 → 烧 sidecar project 字段（真相源 chat_log，与 G1 读取端
    /// 同一格式）+ 登记内存索引。返回绑定到的条目。
    pub fn bind_session(
        &self,
        session_key: &str,
        project_id: &str,
    ) -> Result<ProjectEntry, String> {
        let entry = registry::find_project(&self.registry_path, project_id)
            .ok_or_else(|| format!("项目不存在: {project_id}"))?;
        if !entry.path.is_dir() {
            return Err(format!(
                "项目「{}」目录不存在: {}（先恢复目录或移除项目再试）",
                entry.name,
                entry.path.display()
            ));
        }
        nemesis_agent::chat_log::write_session_project(
            session_key,
            &entry.id,
            &entry.path.to_string_lossy(),
        );
        self.remember_session(session_key, &entry.id);
        Ok(entry)
    }

    /// 取运行中项目 loop 的 Arc（G3 项目调度 / G4 WSAPI 消费）。
    pub fn project_loop(&self, project_id: &str) -> Option<Arc<nemesis_agent::r#loop::AgentLoop>> {
        self.state
            .lock()
            .unwrap()
            .get(project_id)
            .map(|h| h.agent_loop.clone())
    }

    pub fn registry_path(&self) -> &std::path::Path {
        &self.registry_path
    }

    // ------------------------------------------------------------------
    // 路由（M3，2026-09-08）
    // ------------------------------------------------------------------

    /// 启动项目调度订阅（M3）。**全进程只允许 1 个**（幂等闸，重复调用
    /// 直接返回）。订阅后 bus inbound 的 fan-out WARN 属预期——主桥 +
    /// 本调度是故意并存的多订阅者，隔离语义靠「主桥 skip 谓词 + 本调度
    /// 丢弃非项目消息」这一对互补过滤实现。
    ///
    /// 桥内语义（`route_one`）：`channel=="system"` 与无归属消息丢弃
    /// （主 loop 已消费）；归属命中转发进对应项目 mpsc；归属命中但目标
    /// 不可用（inactive/已移除/loop 刚停）→ 回诚实出站错误，不静默丢。
    pub fn start_routing(self: &Arc<Self>) {
        if self.routing_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let rx = self.bus.subscribe_inbound();
        let mgr = Arc::clone(self);
        self.rt.spawn(async move {
            let mut rx = rx;
            loop {
                match rx.recv().await {
                    Ok(msg) => mgr.route_one(&msg).await,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        // P2-1：接收侧真实丢失计入 bus 指标（丢的是项目
                        // 会话消息，用户侧表现为项目 loop 未响应）。
                        mgr.bus.record_inbound_lagged(n);
                        warn!("[ProjectLoops] 项目调度 lagged by {n} message(s)");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        info!("[ProjectLoops] 项目调度器已启动（全进程唯一订阅）");
    }

    /// 路由单条消息（`start_routing` 的循环体；单一裁决点 =
    /// [`route_decision`] 纯函数）。
    pub(crate) async fn route_one(&self, msg: &InboundMessage) {
        // system 永不进项目 loop（热路径短路，与 route_decision 首臂一致
        // ——cluster_continuation 等系统回灌即使 session_key 带项目归属也
        // 只归主 loop 消费）。
        if msg.channel == "system" {
            return;
        }
        let owner = self.owner_of(&msg.session_key);
        let RouteDecision::ToProject(pid) = route_decision(msg, owner.as_deref()) else {
            return; // ToMain：主桥放行，这里丢弃
        };
        // B9 开工闸（VM 档项目）：投递前确保 VM 内工作副本与宿主一致。
        // 拦截/冲突都扣住消息（不出半就绪的工作副本给 agent 撞，不出半
        // 裁决的冲突面给 agent 撞）。非 VM 档项目零开销直通。
        #[cfg(feature = "vm")]
        match self.vm_sync_gate(&pid).await {
            GateVerdict::Blocked(e) => {
                self.send_sync_blocked_error(msg, &pid, &e);
                return;
            }
            GateVerdict::Conflicts(files) if !files.is_empty() => {
                self.spawn_conflict_resolution(msg, &pid, files);
                return;
            }
            GateVerdict::Conflicts(_) | GateVerdict::Pass => {}
        }
        match self.project_inbound_tx(&pid) {
            Some(tx) => {
                if tx.send(msg.clone()).await.is_err() {
                    // 接收端关闭（loop 刚被 stop 的窗口）→ 按不可用处理。
                    self.send_unroutable_error(msg, &pid);
                }
            }
            None => self.send_unroutable_error(msg, &pid),
        }
    }

    /// B9 开工闸：项目档位为 vm 时，消息投递前做「宿主变更清单 + 嫌疑
    /// 文件三方对比 + 批量推进」（[`crate::vm_sync::open_work_gate`]——
    /// 基线在册且宿主无变更 = 零传输直通；首次 = 开工全量同步，分钟级
    /// 一次性成本；§4.2 ④ 的开工闸语义——**轮边界批量对齐**，不做透明
    /// 按需 fill）。双改冲突（宿主≠基线 且 VM≠基线）原样上抛，由
    /// [`Self::spawn_conflict_resolution`] 接管（审批卡三选项，文件粒度
    /// 隔离）。
    ///
    /// 直通臂（零开销）：agents 表无该项目记录 / 档位非 vm——绝大多数
    /// 项目走这里。VM supervisor 未装配 = 诚实拦（用户给项目开了 vm 档
    /// 但环境没跟上）；注册表无条目 = 放行（交路由层原「项目不可用」
    /// 语义，那里能从会话 sidecar 恢复展示名，比这里的拦截文案更准）。
    #[cfg(feature = "vm")]
    async fn vm_sync_gate(&self, pid: &str) -> GateVerdict {
        let Some(mgr) = crate::agents::manager() else {
            return GateVerdict::Pass;
        };
        let Some(rec) = mgr.agent_by_project(pid) else {
            return GateVerdict::Pass;
        };
        if rec.runtime_backend != nemesis_taskstore::agents::runtime_backend::VM {
            return GateVerdict::Pass;
        }
        let Some(sup) = mgr.vm_supervisor() else {
            return GateVerdict::Blocked(
                "VM 执行环境未装配（vm feature 关闭或 gateway 未启用 VM），无法开工".to_string(),
            );
        };
        let Some(entry) = registry::list_projects(&self.registry_path)
            .into_iter()
            .find(|p| p.id == pid)
        else {
            return GateVerdict::Pass; // 见函数文档：交「项目不可用」原语义
        };
        match crate::vm_sync::open_work_gate(
            &self.main_workspace,
            pid,
            &entry.path,
            sup.as_ref(),
            &rec.agent_id,
            false,
        )
        .await
        {
            Ok(gate) => match gate {
                crate::vm_sync::GateSync::Fresh(report) => {
                    info!(
                        project = %pid,
                        files = report.files,
                        bytes = report.bytes,
                        selection = report.selection,
                        "[ProjectLoops] VM 工作副本首次开工全量同步完成"
                    );
                    GateVerdict::Pass
                }
                crate::vm_sync::GateSync::Ready { synced_at_ms } => {
                    tracing::debug!(
                        project = %pid,
                        synced_at_ms,
                        "[ProjectLoops] VM 工作副本基线在册且宿主无变更，零传输直通"
                    );
                    GateVerdict::Pass
                }
                crate::vm_sync::GateSync::Aligned { pushed, deleted } => {
                    info!(
                        project = %pid,
                        pushed = pushed.len(),
                        deleted = deleted.len(),
                        "[ProjectLoops] VM 工作副本开工增量对齐完成（{} 推 / {} 删）",
                        pushed.len(),
                        deleted.len()
                    );
                    GateVerdict::Pass
                }
                crate::vm_sync::GateSync::Conflicts(files) => GateVerdict::Conflicts(files),
            },
            Err(e) => GateVerdict::Blocked(e),
        }
    }

    /// B9 开工闸拦截消息的诚实出站报错（`send_unroutable_error` 同款
    /// 形态；刻意不写 chat_log——消息未进任何 loop，路由层保持纯转发
    /// 语义）。
    #[cfg(feature = "vm")]
    fn send_sync_blocked_error(&self, msg: &InboundMessage, pid: &str, reason: &str) {
        let name = self.display_label(pid, &msg.session_key);
        let content = format!("⚠ 项目「{name}」VM 工作副本未就绪，消息已拦截：{reason}");
        warn!(
            project = %pid,
            channel = %msg.channel,
            chat_id = %msg.chat_id,
            "[ProjectLoops] {content}"
        );
        self.bus
            .publish_outbound(OutboundMessage::new(&msg.channel, &msg.chat_id, &content));
    }

    /// B9 冲突裁决任务派发（`route_one` 冲突臂）：扣留消息，后台逐文件
    /// 发审批卡；全部裁决收场后回注消息（任何一卡取消/超时 = 整轮取消，
    /// 诚实出站告知）。任务只捕获 'static 克隆，不借 `&self`。
    #[cfg(feature = "vm")]
    fn spawn_conflict_resolution(
        &self,
        msg: &InboundMessage,
        pid: &str,
        files: Vec<crate::vm_sync::ConflictFile>,
    ) {
        let name = self.display_label(pid, &msg.session_key);
        let payload = ConflictResolutionTask {
            msg: msg.clone(),
            pid: pid.to_string(),
            name,
            files,
            main_workspace: self.main_workspace.clone(),
            registry_path: self.registry_path.clone(),
            question_slot: self.shared.question_slot.clone(),
            tx: self.project_inbound_tx(pid),
            bus: self.bus.clone(),
        };
        self.rt.spawn(run_conflict_resolution(payload));
    }

    /// 归属命中但目标项目 loop 不可用（目录消失/inactive/已移除）→ 不
    /// 静默丢：向来源会话回诚实出站错误（经 bus 出站链路 → 前端按现状
    /// 错误消息显示）。
    ///
    /// 刻意**不写 chat_log**：chat_log 追加走全局 path manager，而调度器
    /// 是纯路由层（消息从未进过任何 loop，用户消息行也尚未落盘——那由
    /// loop 的 turn 路径负责）。错误只做出站投递。
    /// 不可用项目的展示名三级回落：① 注册表名（目录消失但条目还在）→
    /// ② 会话 sidecar 的 project_path 尾段（项目已移除、注册表名字已
    /// 不可得时，从烧入会话的绑定路径恢复可辨识名）→ ③ pid 兜底。
    /// bus 路由（send_unroutable_error）与 WSAPI（ProjectsBridge
    /// display_label 覆写）共用，单一真相源。
    pub fn display_label(&self, pid: &str, session_key: &str) -> String {
        if let Some(p) = registry::list_projects(&self.registry_path)
            .into_iter()
            .find(|p| p.id == pid)
        {
            return p.name;
        }
        if let Some(path) = self.session_project_path(session_key)
            && let Some(tail) = std::path::Path::new(&path)
                .file_name()
                .and_then(|n| n.to_str())
        {
            return tail.to_string();
        }
        pid.to_string()
    }

    /// 读会话 sidecar meta 的 project_path（`owner_of` 同款磁盘形态；
    /// 只读不回填——展示名是冷路径，不值得进内存索引）。
    fn session_project_path(&self, session_key: &str) -> Option<String> {
        if session_key.is_empty() {
            return None;
        }
        let stem = stem_from_session_key(session_key);
        let meta_path = nemesis_path::resolve_session_logs_dir_in_workspace(&self.main_workspace)
            .join(format!("{stem}.meta.json"));
        let data = std::fs::read_to_string(&meta_path).ok()?;
        serde_json::from_str::<serde_json::Value>(&data)
            .ok()?
            .get("project_path")?
            .as_str()
            .map(|s| s.to_string())
    }

    fn send_unroutable_error(&self, msg: &InboundMessage, pid: &str) {
        let name = self.display_label(pid, &msg.session_key);
        let content = format!("⚠ 项目「{name}」当前不可用（目录缺失或已移除），消息未能投递。");
        warn!(
            project = %pid,
            channel = %msg.channel,
            chat_id = %msg.chat_id,
            "[ProjectLoops] {content}"
        );
        self.bus
            .publish_outbound(OutboundMessage::new(&msg.channel, &msg.chat_id, &content));
    }

    /// 取项目 loop 的调度转发端（`channels` 表；与 AgentLoop 实例解耦，
    /// 测试可注入裸通道）。
    pub(crate) fn project_inbound_tx(
        &self,
        project_id: &str,
    ) -> Option<tokio::sync::mpsc::Sender<InboundMessage>> {
        self.channels.lock().unwrap().get(project_id).cloned()
    }

    /// 主桥 skip 谓词（装配进 `AgentLoopServiceAdapter`）：只有
    /// `route_decision` 判 ToProject 的消息才跳过主 loop——system 永不
    /// 跳过（系统回灌语义只属于主 loop），无归属消息照常进主 loop。
    pub(crate) fn bridge_should_skip(&self, msg: &InboundMessage) -> bool {
        // 热路径短路（与 route_decision 首臂一致，省一次 owner 查询/兜底读）。
        if msg.channel == "system" {
            return false;
        }
        matches!(
            route_decision(msg, self.owner_of(&msg.session_key).as_deref()),
            RouteDecision::ToProject(_)
        )
    }

    // ------------------------------------------------------------------
    // 归属索引
    // ------------------------------------------------------------------

    /// 从 meta sidecar 全量重建归属索引（启动期）。扫描端语义与
    /// `nemesis-web` scan_session_logs 一致：遍历 session_logs/*.jsonl，
    /// 读同名 `.meta.json` sidecar 的 project_id。损坏/半写文件静默跳过
    /// （lenient——启动不因个别坏文件失败）。
    pub fn reload_owner_index(&self) {
        let dir = nemesis_path::resolve_session_logs_dir_in_workspace(&self.main_workspace);
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => return, // 目录不存在 = 零会话，索引为空
        };
        let mut idx = self.owner_index.write();
        idx.clear();
        for ent in entries.flatten() {
            let path = ent.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let meta_path = path.with_extension("meta.json");
            let Ok(data) = std::fs::read_to_string(&meta_path) else {
                continue; // 无 sidecar = 无绑定
            };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) else {
                warn!(
                    path = %meta_path.display(),
                    "[ProjectLoops] meta sidecar 解析失败，跳过"
                );
                continue;
            };
            if let Some(pid) = v.get("project_id").and_then(|p| p.as_str()) {
                idx.insert(session_key_from_stem(stem), pid.to_string());
            }
        }
        let bound = idx.len();
        if bound > 0 {
            info!("[ProjectLoops] owner index rebuilt: {bound} session(s) bound to projects");
        }
    }

    /// 增量登记（M4 sessions.create / fork 接线时调用）。
    pub fn remember_session(&self, session_key: &str, project_id: &str) {
        self.owner_index
            .write()
            .insert(session_key.to_string(), project_id.to_string());
    }

    /// 摘除会话归属（M3 方法就绪；G4 WSAPI sessions.delete/clear 经
    /// ProjectsBridge 联动接线）。返回是否确有**内存索引**绑定被移除。
    ///
    /// 两级动作（G4 起完整语义）：
    /// ① 摘内存索引；
    /// ② 同步清 sidecar 的 project 字段（title/血缘/manual 保留）——否则
    ///    [`Self::owner_of`] 的 sidecar 兜底会把绑定「复活」。delete 路径
    ///    的 meta 已随 delete_chat_log 一并删除，②为 no-op；clear 路径
    ///    meta 保留 title，②真正生效。
    pub fn forget_session(&self, session_key: &str) -> bool {
        let removed = self.owner_index.write().remove(session_key).is_some();
        nemesis_agent::chat_log::clear_session_project(session_key);
        removed
    }

    /// 查归属（主桥谓词 + 项目调度共用）。两级：
    /// ① 内存索引（启动扫描 + remember/回填维护）；
    /// ② **miss 兜底回读 meta sidecar**（M3：防 fork 等未通知路径漏索引
    /// ——fork 继承绑定但绕过 sessions.create）；命中后回填索引，后续
    /// 消息走纯内存查询。sidecar 缺失/损坏/无 project_id = None（落对话组）。
    pub fn owner_of(&self, session_key: &str) -> Option<String> {
        if let Some(pid) = self.owner_index.read().get(session_key).cloned() {
            return Some(pid);
        }
        if session_key.is_empty() {
            return None;
        }
        let stem = stem_from_session_key(session_key);
        let meta_path = nemesis_path::resolve_session_logs_dir_in_workspace(&self.main_workspace)
            .join(format!("{stem}.meta.json"));
        let data = match std::fs::read_to_string(&meta_path) {
            Ok(d) => d,
            Err(_) => return None, // 无 sidecar = 无绑定（大多数会话走这里）
        };
        let pid = serde_json::from_str::<serde_json::Value>(&data)
            .ok()?
            .get("project_id")?
            .as_str()?
            .to_string();
        self.owner_index
            .write()
            .insert(session_key.to_string(), pid.clone());
        Some(pid)
    }

    /// 测试专用：向 `channels` 表注入裸通道（不构建 AgentLoop——六矩阵
    /// 路由测试只验证转发/丢弃语义，不需要真 loop 消费）。
    #[cfg(test)]
    pub(crate) fn insert_test_channel(
        &self,
        project_id: &str,
        tx: tokio::sync::mpsc::Sender<InboundMessage>,
    ) {
        self.channels
            .lock()
            .unwrap()
            .insert(project_id.to_string(), tx);
    }
}

// ---------------------------------------------------------------------------
// ProjectsBridge 实现（G4）：trait 定义在 nemesis-web（依赖方向：web 不
// 依赖 nemesisbot），gateway 装配期把本 manager 装进 `PROJECTS_BRIDGE` 槽，
// projects.* WSAPI 与 resolve_session_loop 全部经此 trait 触达。
// ---------------------------------------------------------------------------

impl ProjectLoopManager {
    /// 模型配置热切联动（BUG 2026-09-21）：项目 loop 在 gateway 装配期经
    /// `start_all` 一次性 spawn，`spawn_project` 幂等永不重建——主 loop 的
    /// `models.set_default` 热切从未触达项目 loop。典型症状：先起网关后配
    /// LLM（对话模式热切即好），项目模式永远 NullProvider「未配置模型」。
    ///
    /// 本方法用**盘上 config** 重走与 `build_project_agent_loop` 同源的解析
    /// （`get_effective_llm` → `resolve_model_config` → `create_provider`），
    /// 对每个在跑项目 loop `set_provider_and_model` 热换（与主 loop 的
    /// runtime swap 同构：不重建 loop、不打断会话，in-flight 走旧 provider）。
    /// 解析/构造失败 = warn 保持现状——绝不把能用的 loop 换成 NullProvider；
    /// 无在跑 loop = 静默 no-op（新建项目 spawn 时自会读新 config）。
    pub fn reload_providers(&self) {
        let config_path = self.shared.home.join("config.json");
        let cfg = match nemesis_config::load_config(&config_path) {
            Ok(c) => c,
            Err(e) => {
                warn!(
                    "[ProjectLoops] reload_providers: config.json 读取失败，项目 loop 保持现状: {e}"
                );
                return;
            }
        };
        let llm_ref = nemesis_config::get_effective_llm(Some(&cfg));
        let resolution = match nemesis_config::resolve_model_config(&cfg, &llm_ref) {
            Ok(r) => r,
            Err(e) => {
                warn!(
                    "[ProjectLoops] reload_providers: 模型 '{llm_ref}' 解析失败，项目 loop 保持现状: {e}"
                );
                return;
            }
        };
        let factory_cfg = nemesis_providers::factory::FactoryConfig {
            proxy: resolution.proxy.clone(),
            stream_mode: resolution.stream_mode.clone(),
            cache_breakpoints: resolution.cache_breakpoints,
            llm_ref: format!("{}/{}", resolution.provider_name, resolution.model_name),
            api_key: resolution.api_key.clone(),
            api_base: resolution.api_base.clone(),
            // workspace 留空：provider 不消费该字段（消费方是工具层，项目
            // loop 的工具 workspace 在装配期已锚定项目目录，与 provider 无关）。
            workspace: String::new(),
            connect_mode: resolution.connect_mode.clone(),
            protocol: resolution.protocol.clone(),
            timeout_secs: resolution.timeout_secs,
            account_id: String::new(),
            headers: std::collections::HashMap::new(),
        };
        let provider = match nemesis_providers::factory::create_provider(&factory_cfg) {
            Ok(p) => p,
            Err(e) => {
                warn!(
                    "[ProjectLoops] reload_providers: provider 构造失败，项目 loop 保持现状: {e}"
                );
                return;
            }
        };
        let adapter = Arc::new(nemesis_web::llm_bridge::ProviderAdapter::new(
            provider,
            resolution.model_name.clone(),
        ));
        let mut swapped = 0usize;
        for handle in self.state.lock().unwrap().values() {
            handle
                .agent_loop
                .set_provider_and_model(adapter.clone(), resolution.model_name.clone());
            swapped += 1;
        }
        if swapped > 0 {
            info!(
                model = %resolution.model_name,
                loops = swapped,
                "[ProjectLoops] 项目 loop provider 已随模型热切同步"
            );
        }
    }
}

impl nemesis_web::handlers::projects::ProjectsBridge for ProjectLoopManager {
    fn display_label(&self, project_id: &str, session_key: &str) -> String {
        // 三级回落真相源（注册表名 → sidecar project_path 尾段 → pid）。
        ProjectLoopManager::display_label(self, project_id, session_key)
    }

    fn list(&self) -> Vec<nemesis_web::handlers::projects::ProjectInfo> {
        registry::list_projects(&self.registry_path)
            .into_iter()
            .map(|e| nemesis_web::handlers::projects::ProjectInfo {
                running: self.is_running(&e.id),
                id: e.id,
                name: e.name,
                path: e.path.to_string_lossy().to_string(),
                created_at: e.created_at,
                instructions_trusted: e.instructions_trusted,
            })
            .collect()
    }

    fn create(
        &self,
        name: &str,
        path: &str,
    ) -> Result<nemesis_web::handlers::projects::ProjectInfo, String> {
        // 校验链（路径存在→canonicalize→重叠→上限）+ 写 registry 在
        // registry::create_project；成功后立即 spawn loop（失败时注册表
        // 条目保留——重启 start_all 会重试，错误文案照实回显）。
        let entry = registry::create_project(
            &self.registry_path,
            &self.main_workspace,
            name,
            path,
            self.max_projects(),
        )
        .map_err(|e| format!("{e:#}"))?;
        self.spawn_project(&entry)?;
        Ok(nemesis_web::handlers::projects::ProjectInfo {
            running: true,
            id: entry.id,
            name: entry.name,
            path: entry.path.to_string_lossy().to_string(),
            created_at: entry.created_at,
            instructions_trusted: entry.instructions_trusted,
        })
    }

    fn remove(
        &self,
        project_id: &str,
    ) -> Result<nemesis_web::handlers::projects::ProjectInfo, String> {
        // 只解除分组（registry 摘条目）+ stop loop。会话 sidecar 的归属
        // **保留**——前端据此把会话落「已移除」灰组（§3.4-F4），发送走
        // 路由层「不可用」诚实报错。
        let entry = registry::remove_project(&self.registry_path, project_id)
            .map_err(|e| format!("{e:#}"))?;
        self.stop_project(project_id);
        Ok(nemesis_web::handlers::projects::ProjectInfo {
            running: false,
            id: entry.id,
            name: entry.name,
            path: entry.path.to_string_lossy().to_string(),
            created_at: entry.created_at,
            instructions_trusted: entry.instructions_trusted,
        })
    }

    fn rename(
        &self,
        project_id: &str,
        new_name: &str,
    ) -> Result<nemesis_web::handlers::projects::ProjectInfo, String> {
        let entry = registry::rename_project(&self.registry_path, project_id, new_name)
            .map_err(|e| format!("{e:#}"))?;
        Ok(nemesis_web::handlers::projects::ProjectInfo {
            running: self.is_running(project_id),
            id: entry.id,
            name: entry.name,
            path: entry.path.to_string_lossy().to_string(),
            created_at: entry.created_at,
            instructions_trusted: entry.instructions_trusted,
        })
    }

    fn set_trust(
        &self,
        project_id: &str,
        trusted: bool,
    ) -> Result<nemesis_web::handlers::projects::ProjectInfo, String> {
        // 注册表先写（真相源；损坏 loud 拒绝）。
        let entry = registry::set_project_trust(&self.registry_path, project_id, trusted)
            .map_err(|e| format!("{e:#}"))?;
        // 热更新在跑 loop（区块逐 build 重渲染，下一轮即生效；无残留）。
        // 未在跑 = 仅写盘（spawn 时 build_project_agent_loop 按注册表解析
        // 起点）。config 读取失败 → warn + annotate 兜底（注册表已写成功
        // 不回滚——lenient 与装配期 from_policy 未知值同语义）。
        if let Some(loop_arc) = self.project_loop(project_id) {
            let trust = if trusted {
                nemesis_agent::workspace_instructions::InstructionTrust::Trusted
            } else {
                let policy = nemesis_config::load_config(&self.shared.home.join("config.json"))
                    .map(|c| {
                        c.projects
                            .as_ref()
                            .map(|p| p.instruction_trust.clone())
                            .unwrap_or_default()
                    })
                    .unwrap_or_else(|e| {
                        warn!(
                            "[ProjectLoops] set_trust: config.json 读取失败，未受信项目按 annotate 兜底: {e}"
                        );
                        String::new()
                    });
                nemesis_agent::workspace_instructions::InstructionTrust::from_policy(&policy)
            };
            loop_arc.set_instruction_trust(trust);
        }
        Ok(nemesis_web::handlers::projects::ProjectInfo {
            running: self.is_running(project_id),
            id: entry.id,
            name: entry.name,
            path: entry.path.to_string_lossy().to_string(),
            created_at: entry.created_at,
            instructions_trusted: entry.instructions_trusted,
        })
    }

    fn owner_of(&self, session_key: &str) -> Option<String> {
        ProjectLoopManager::owner_of(self, session_key)
    }

    fn loop_for_session(&self, project_id: &str) -> Option<Arc<nemesis_agent::r#loop::AgentLoop>> {
        self.project_loop(project_id)
    }

    fn project_path(&self, project_id: &str) -> Option<std::path::PathBuf> {
        ProjectLoopManager::project_path(self, project_id)
    }

    fn bind_session(&self, session_key: &str, project_id: &str) -> Result<(), String> {
        ProjectLoopManager::bind_session(self, session_key, project_id).map(|_| ())
    }

    fn forget_session(&self, session_key: &str) {
        ProjectLoopManager::forget_session(self, session_key);
    }

    fn reload_provider_all(&self) {
        ProjectLoopManager::reload_providers(self);
    }
}

// ---------------------------------------------------------------------------
// B9 开工闸裁决值 + 冲突裁决任务（'static 后台编排；route_one 冲突臂派发）
// ---------------------------------------------------------------------------

/// B9（VM 项目工作副本）开工闸裁决值：`Pass` = 直通/对齐完成；
/// `Blocked` = 诚实拦截（消息不投递）；`Conflicts` = 双改冲突，扣留
/// 消息转人工逐文件裁决。
#[cfg(feature = "vm")]
enum GateVerdict {
    Pass,
    Blocked(String),
    Conflicts(Vec<crate::vm_sync::ConflictFile>),
}

/// 冲突审批卡超时（秒）。逐文件一卡，用户不答 = 整轮取消（不悄悄
/// 选边——plan §4.3）。
#[cfg(feature = "vm")]
const CONFLICT_ASK_TIMEOUT_SECS: u64 = 300;

/// 冲突审批卡 question_id 发号器（进程内自增 + 毫秒戳，跨重启唯一）。
#[cfg(feature = "vm")]
fn next_conflict_question_id() -> String {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    format!(
        "vm-conflict-{}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0),
        N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    )
}

/// B9 冲突裁决三选一（每文件一张卡；多选/非候选/超时 = 取消）。
#[cfg(feature = "vm")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConflictChoice {
    /// ① 宿主版覆盖 VM 版（VM 内改动丢弃）。
    HostWins,
    /// ② 保留 VM 版：宿主改动**挂起**（park）——turn 末 commit 不把
    /// VM 版回传覆盖宿主，宿主改动也不进 VM；此后在宿主重新修改该文件
    /// = 手动合并完成，自动解挂并推进 VM。
    KeepVm,
    /// ③/超时/答非所列：取消本轮开工（消息不投递，什么都不改）。
    Cancel,
}

/// 冲突卡三选项（选项原文即应答原文——broker respond 按候选原文校验，
/// [`classify_conflict_answer`] 按序号前缀分类）。
#[cfg(feature = "vm")]
pub(crate) fn conflict_options() -> Vec<String> {
    vec![
        "① 用宿主版覆盖 VM 版（VM 内改动丢弃）".to_string(),
        "② 保留 VM 版（宿主改动挂起不覆盖；之后在宿主重新修改该文件即视为合并完成，会自动推进 VM）"
            .to_string(),
        "③ 取消本轮（消息不投递）".to_string(),
    ]
}

/// 冲突卡问题正文（自含：项目名 + 文件 + 双改事实）。
#[cfg(feature = "vm")]
pub(crate) fn conflict_question_text(name: &str, f: &crate::vm_sync::ConflictFile) -> String {
    format!(
        "项目「{name}」双改冲突：文件 {} 在宿主与 VM 内都被修改（相对上次同步基线）。\
         选择如何处理这一个文件？",
        f.rel
    )
}

/// 应答 → 裁决（`None` = 超时）。选项原文按序号前缀分类；空/多项/
/// 非候选一律 Cancel（不猜）。第二值是取消时的诚实注记（HostWins/
/// KeepVm 恒空串）。
#[cfg(feature = "vm")]
pub(crate) fn classify_conflict_answer(outcome: Option<&[String]>) -> (ConflictChoice, String) {
    let Some(sel) = outcome else {
        return (ConflictChoice::Cancel, "未作答，本轮已取消".to_string());
    };
    if sel.len() != 1 {
        return (
            ConflictChoice::Cancel,
            "应答形态无效，本轮已取消".to_string(),
        );
    }
    let a = sel[0].trim();
    if a.starts_with('①') {
        (ConflictChoice::HostWins, String::new())
    } else if a.starts_with('②') {
        (ConflictChoice::KeepVm, String::new())
    } else {
        (
            ConflictChoice::Cancel,
            "未识别的选择，本轮已取消".to_string(),
        )
    }
}

/// 冲突裁决任务的全部捕获（route_one 派发时一次性克隆；任务不借管理器）。
#[cfg(feature = "vm")]
struct ConflictResolutionTask {
    msg: InboundMessage,
    pid: String,
    name: String,
    files: Vec<crate::vm_sync::ConflictFile>,
    main_workspace: PathBuf,
    registry_path: PathBuf,
    question_slot: nemesis_agent::loop_tools::QuestionBrokerSlot,
    tx: Option<tokio::sync::mpsc::Sender<InboundMessage>>,
    bus: Arc<nemesis_bus::MessageBus>,
}

/// 冲突裁决编排（plan §4.3 + H3 停车语义）：**挂起表先于裁决落**（全部
/// 冲突文件的前置挂起——裁决卡挂着的三百秒窗口内，turn 末回传/手动
/// commit 的 H3 过滤靠它保护宿主改动不被 VM 版覆盖；落盘失败 = 整轮取
/// 消，连卡都不发）→ 逐文件一张审批卡（三选项，spawn_blocking 阻塞等
/// 答）→ ① 记入 host-wins 清单（`apply_conflict_host_wins` 应用时解挂）
/// / ② 前置条目保留（不进 VM；此后宿主重新改动该文件 = 手动合并完成，
/// 开工闸自动解挂推进）/ ③·超时 = 整轮取消并**恢复前置前的挂起表快
/// 照**（撤销本轮前置——回到「下轮开工闸重新检出重新问」的旧语义，也
/// 不吞掉更早轮次遗留的旧条目）。① 逐条落盘（独立 internal_call + 基
/// 线增量刷新；单条失败不拦其余，诚实汇总——失败条的挂起条目留在表
/// 里，H3 过滤继续保护该文件）→ 回注消息。取消臂诚实出站告知且不改任
/// 何工作区文件（挂起表恢复快照 = 状态无痕）。
///
/// broker 缺席（headless / 无人值守未接线）= 取消语义：不能悄悄选边。
/// supervisor / agent_id / 注册表条目在**应用期**重新解析（卡片挂着时
/// 项目可能被移除/切档——消失即诚实取消，不带旧身份落盘）。
#[cfg(feature = "vm")]
async fn run_conflict_resolution(t: ConflictResolutionTask) {
    let broker = t.question_slot.read().clone();
    let Some(broker) = broker else {
        conflict_cancel_outbound(
            &t.bus,
            &t.msg,
            &t.name,
            "无可用交互界面接收冲突裁决卡，本轮已取消（消息未投递，文件未改动）",
        );
        return;
    };

    // 前置挂起（停车凭据先于裁决存在）+ 前置前快照（取消臂恢复用；
    // 过闸快照——不漏并发轮次条目）。
    let pre_existing = crate::vm_sync::snapshot_diverged(&t.main_workspace, &t.pid).await;
    let entries: Vec<(String, String, String)> = t
        .files
        .iter()
        .map(|f| (f.rel.clone(), f.host_sha.clone(), f.vm_sha.clone()))
        .collect();
    if let Err(e) = crate::vm_sync::park_diverged_files(&t.main_workspace, &t.pid, entries).await {
        conflict_cancel_outbound(
            &t.bus,
            &t.msg,
            &t.name,
            &format!("挂起记录落盘失败（{e}），本轮已取消；文件未做任何改动"),
        );
        return;
    }

    let mut host_wins: Vec<String> = Vec::new();
    let mut keep_vm: Vec<String> = Vec::new();
    for f in &t.files {
        let req = nemesis_types::agent::QuestionRequest {
            question_id: next_conflict_question_id(),
            question: conflict_question_text(&t.name, f),
            options: conflict_options(),
            multi: false,
            fields: Vec::new(),
            chat_id: t.msg.chat_id.clone(),
            session_key: t.msg.session_key.clone(),
            timeout_secs: CONFLICT_ASK_TIMEOUT_SECS,
            bypass_auto_approve: false,
        };
        let ask_broker = broker.clone();
        let outcome = tokio::task::spawn_blocking(move || ask_broker.ask(req)).await;
        let (choice, note) = match outcome {
            Ok(Ok(nemesis_types::agent::QuestionOutcome::Answered(sel))) => {
                classify_conflict_answer(Some(&sel))
            }
            // 冲突卡是单选选项形态（fields 空），表单作答不该出现——诚实取消。
            Ok(Ok(nemesis_types::agent::QuestionOutcome::AnsweredForm(_))) => (
                ConflictChoice::Cancel,
                "冲突裁决卡为单选形态，收到表单作答（不合法），本轮已取消".to_string(),
            ),
            Ok(Ok(nemesis_types::agent::QuestionOutcome::Timeout)) => {
                classify_conflict_answer(None)
            }
            // 急停强制了结（循环干预阶梯，2026-10-24）：无裁决 = 保守取消
            // 本轮同步（文件挂起状态保持原样，下轮恢复后重裁）。
            Ok(Ok(nemesis_types::agent::QuestionOutcome::Cancelled)) => (
                ConflictChoice::Cancel,
                "急停触发，冲突裁决中断，本轮已取消（文件挂起状态不变）".to_string(),
            ),
            Ok(Err(e)) => (
                ConflictChoice::Cancel,
                format!("裁决卡故障（{e}），本轮已取消"),
            ),
            Err(e) => (
                ConflictChoice::Cancel,
                format!("裁决任务失败（{e}），本轮已取消"),
            ),
        };
        match choice {
            ConflictChoice::HostWins => {
                host_wins.push(f.rel.clone());
                // 与 ② 臂对等的逐文件裁决留痕（可观测性对齐：②有「保留 VM
                // 版」行，①此前只落 applied 计数，单文件裁决无迹可查）。
                tracing::info!(
                    project = %t.pid,
                    file = %f.rel,
                    "[ProjectLoops] 冲突裁决：用宿主版覆盖 VM 版（VM 内改动丢弃）"
                );
            }
            ConflictChoice::KeepVm => {
                keep_vm.push(f.rel.clone());
                // 幂等重申挂起条目（正常路径前置已在，同值覆盖无变化；
                // 双轮并发裁决同一文件的竞态下——前轮①已解挂该条目——
                // 后轮②重申 = 「后裁决者赢」语义一致）。失败仅 warn：正
                // 常路径前置条目仍在，重申只是竞态补丁。
                let entry = vec![(f.rel.clone(), f.host_sha.clone(), f.vm_sha.clone())];
                if let Err(e) =
                    crate::vm_sync::park_diverged_files(&t.main_workspace, &t.pid, entry).await
                {
                    tracing::warn!(
                        project = %t.pid,
                        file = %f.rel,
                        error = %e,
                        "[ProjectLoops] ② 挂起条目重申失败（前置条目仍在，保守继续）"
                    );
                }
                tracing::info!(
                    project = %t.pid,
                    file = %f.rel,
                    "[ProjectLoops] 冲突裁决：保留 VM 版（宿主改动挂起，turn 末不回传覆盖）"
                );
            }
            ConflictChoice::Cancel => {
                // 撤销前置挂起（恢复前置前快照——状态无痕：下轮开工闸
                // 重新检出重新问；更早轮次的遗留条目不受本轮影响）。
                // 过在飞闸恢复（防与 commit 过滤读 / 并发 park 丢更新）。
                if let Err(e) = crate::vm_sync::restore_diverged_snapshot(
                    &t.main_workspace,
                    &t.pid,
                    &pre_existing,
                )
                .await
                {
                    tracing::warn!(
                        project = %t.pid,
                        error = %e,
                        "[ProjectLoops] 冲突取消臂恢复挂起表快照失败：保留前置条目（保守停车，不改工作区文件）"
                    );
                }
                conflict_cancel_outbound(&t.bus, &t.msg, &t.name, &note);
                return;
            }
        }
    }

    // ② 的挂起条目已在**前置挂起**落定（先于裁决存在，保护裁决窗口）；
    // 这里无需再落表。① 的条目由 apply_conflict_host_wins 应用时解挂。

    // 应用期重新解析身份（卡片挂着时项目可能被移除/切档/环境被拆）。
    let mgr = crate::agents::manager();
    let resolved = mgr.as_ref().and_then(|m| {
        let sup = m.vm_supervisor()?;
        let rec = m.agent_by_project(&t.pid)?;
        let entry = registry::list_projects(&t.registry_path)
            .into_iter()
            .find(|p| p.id == t.pid)?;
        if rec.runtime_backend != nemesis_taskstore::agents::runtime_backend::VM {
            return None;
        }
        Some((sup, rec.agent_id, entry.path))
    });
    let Some((sup, agent_id, project_path)) = resolved else {
        // 裁决期间环境变化：恢复前置前快照（同取消臂——不留本轮半裁决
        // 状态），诚实取消。过在飞闸恢复（同取消臂理由）。
        if let Err(e) =
            crate::vm_sync::restore_diverged_snapshot(&t.main_workspace, &t.pid, &pre_existing)
                .await
        {
            tracing::warn!(
                project = %t.pid,
                error = %e,
                "[ProjectLoops] 裁决环境变化臂恢复挂起表快照失败：保留前置条目（保守停车）"
            );
        }
        conflict_cancel_outbound(
            &t.bus,
            &t.msg,
            &t.name,
            "裁决期间项目/VM 环境状态已变化，本轮已取消（消息未投递）",
        );
        return;
    };

    let mut applied: Vec<String> = Vec::new();
    let mut failed: Vec<(String, String)> = Vec::new();
    for rel in &host_wins {
        match crate::vm_sync::apply_conflict_host_wins(
            &t.main_workspace,
            &t.pid,
            &project_path,
            sup.as_ref(),
            &agent_id,
            rel,
        )
        .await
        {
            Ok(()) => applied.push(rel.clone()),
            Err(e) => failed.push((rel.clone(), e)),
        }
    }
    if !failed.is_empty() {
        let detail = failed
            .iter()
            .map(|(r, e)| format!("- {r}: {e}"))
            .collect::<Vec<_>>()
            .join("\n");
        warn!(
            project = %t.pid,
            failed = failed.len(),
            "[ProjectLoops] 冲突宿主覆盖部分失败:\n{detail}"
        );
    }
    info!(
        project = %t.pid,
        applied = applied.len(),
        kept_vm = keep_vm.len(),
        "[ProjectLoops] 冲突裁决收场，消息回注项目 loop"
    );

    // 回注消息（裁决完成 = 开工闸语义满足：非冲突文件已对齐、冲突文件
    // 按裁决落定——①已覆盖 / ②挂起受停车语义保护，turn 末 commit 不回
    // 传覆盖宿主）。
    let (channel, chat_id) = (t.msg.channel.clone(), t.msg.chat_id.clone());
    match t.tx {
        Some(tx) => {
            if tx.send(t.msg).await.is_err() {
                // loop 在裁决窗口内被停掉：按不可用诚实报（send_unroutable
                // 是 &self 方法，这里用同文案内联——任务不借管理器）。
                let content = format!(
                    "⚠ 项目「{}」的会话在冲突裁决期间被停止，原消息未能投递（冲突处理结果已保留）。",
                    t.name
                );
                t.bus
                    .publish_outbound(OutboundMessage::new(&channel, &chat_id, &content));
            }
        }
        None => {
            conflict_cancel_outbound(
                &t.bus,
                &t.msg,
                &t.name,
                "项目通道已不可用，原消息未能投递（冲突处理结果已保留）",
            );
        }
    }
}

/// 冲突整轮取消的诚实出站告知（`send_sync_blocked_error` 的任务形态；
/// 刻意不写 chat_log——消息未进任何 loop，路由层保持纯转发语义）。
#[cfg(feature = "vm")]
fn conflict_cancel_outbound(
    bus: &nemesis_bus::MessageBus,
    msg: &InboundMessage,
    name: &str,
    note: &str,
) {
    tracing::warn!(
        channel = %msg.channel,
        chat_id = %msg.chat_id,
        "[ProjectLoops] 项目「{name}」双改冲突裁决未收场：{note}"
    );
    bus.publish_outbound(OutboundMessage::new(
        &msg.channel,
        &msg.chat_id,
        &format!(
            "⚠ 项目「{name}」存在宿主与 VM 双改冲突，{note}。重新发送消息可再次触发开工对齐。"
        ),
    ));
}
