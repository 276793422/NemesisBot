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
/// 正向逆变换）：统一 `:`→`_`。web 形态 `agent:main:session:{sid}` 自然
/// 还原成带 `agent_main_session_` 前缀的 stem；IM 会话 naive 还原（与
/// chat_log 落盘约定一致）。
pub fn stem_from_session_key(session_key: &str) -> String {
    session_key.replace(':', "_")
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
        info!(
            "[ProjectLoops] starting {} project loop(s)",
            entries.len()
        );
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
            .insert(entry.id.clone(), inbound_tx);
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
    pub fn project_loop(
        &self,
        project_id: &str,
    ) -> Option<Arc<nemesis_agent::r#loop::AgentLoop>> {
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
            && let Some(tail) =
                std::path::Path::new(&path).file_name().and_then(|n| n.to_str())
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
        let content =
            format!("⚠ 项目「{name}」当前不可用（目录缺失或已移除），消息未能投递。");
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
        let dir =
            nemesis_path::resolve_session_logs_dir_in_workspace(&self.main_workspace);
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
        })
    }

    fn rename(
        &self,
        project_id: &str,
        new_name: &str,
    ) -> Result<nemesis_web::handlers::projects::ProjectInfo, String> {
        let entry =
            registry::rename_project(&self.registry_path, project_id, new_name)
                .map_err(|e| format!("{e:#}"))?;
        Ok(nemesis_web::handlers::projects::ProjectInfo {
            running: self.is_running(project_id),
            id: entry.id,
            name: entry.name,
            path: entry.path.to_string_lossy().to_string(),
            created_at: entry.created_at,
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
}
