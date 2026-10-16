//! P3 T4：常驻执行体监督器（AgentRuntimeSupervisor）。
//!
//! per-agent 长驻子进程：每个 agent id 一个 executor 角色的 `nemesisbot`
//! 子进程（[`NEMESISBOT_EXECUTOR_AGENT_ID`] 标识），跨工具调用存活；home
//! 即状态（journal / 配置 / 浏览器 profile 全落 agent home），sleep = 进程
//! 退出（状态已落盘），wake / 下次 execute = 重新 spawn。
//!
//! 隔离纪律（P3 ①）：spawn 用 `env_clear()` + OS 运行时基线白名单
//! （[`nemesis_agent::remote_executor_tool::filter_env_allowlist`]，剥离
//! 宿主凭据 / token / NEMESISBOT_*）+ home 族重定向（HOME/USERPROFILE/
//! APPDATA/… 全指 agent home）——子进程内 `$HOME` 即 agent home，宿主
//! 凭据不可见。
//!
//! 盒分支（P3 ②，`sandbox` feature + Windows）：Sandboxie 引擎就绪时子进程
//! 经 `Start.exe /box:NemesisAgent_<id>` 进 per-agent 常驻盒（transport 换
//! 具名管道，journal / 浏览器 profile 走 OpenFilePath 写穿透）；引擎未就绪
//! 退化纯 stdio（⑥ 兼容回退）。非 Windows：`executor.sandbox=true` 时挂
//! `NEMESISBOT_EXECUTOR_SANDBOX` 标记，子进程 exec_worker 启动自装
//! landlock/bwrap（U11 与 per-call 同源语义，stdio 传输不变）。
//!
//! 幂等纪律（P3 ⑤，T2 journal 的消费方）：每次调用生成新 call_id（v2
//! 协议行）；子进程死亡（Eof）时**一次**重建会话并**原样重发**同 call_id
//! 请求——子进程侧 journal replay 保证已完成的调用回缓存结果不重执行
//! （at-most-once），未完成的调用重跑一次（at-least-once）。
//!
//! 槽位看护（T9）：watchdog（15s tick）发现死槽 → 清理 → 按同 exec_root
//! 自动重拉（home 即状态，journal/profile 从盘回载）；连续失败超
//! [`MAX_AUTO_RELAUNCH`] 熔断，一次成功 execute/wake 复位预算。idle 到点
//! 自动睡眠**不重拉**（有意的停）；sleep（显式）同理——睡着的槽已离开
//! map，watchdog 看不见它，不会误拉。
//!
//! 失败分型（T7 回落判据）：[`SupervisorError::Unavailable`] = 常驻路径
//! 没跑起来（config 关闭 / 满员 / spawn 失败）——此时**本调用尚未发生**，
//! 调用方按兼容语义回落 per-call（T9）；[`SupervisorError::ToolFailed`]
//! = 调用已尝试（可能已有副作用）——原样上抛，**绝不回落**（per-call 是
//! 另一个 journal 禁用的进程，回落 = 有副作用工具的重复执行）。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use nemesis_agent::context::RequestContext;
use nemesis_agent::executor_transport::{
    ExecutorResponseWire, PersistentExecutorSession, TransportError, new_call_id,
};
use nemesis_agent::remote_executor_tool::{filter_env_allowlist, home_redirect_env_pairs};
use nemesis_config::ExecutorPersistentConfig;
use nemesis_path::resolve_agent_home_in_workspace;

/// call 超时的兜底下限（config 显式 0 = 用缺省档，不做「0=不限」——
/// supervision 面没有不限：卡死的执行体必须被 watchdog/超时回收）。
const FALLBACK_CALL_TIMEOUT_SECS: u64 = 600;

/// 常驻执行体 spawn 的基础 env 对（stdio / 盒两条路径共用的单一装配点，
/// 防 drift）：执行体角色 + 双根（T7：`WORKSPACE` = **exec_root**——工具
/// cwd 锚，main=workspace / project=项目目录 / selfbuilt=home，与
/// `AgentLoopSpec.exec_root` 同一语义；`HOME` = agent home——隔离根，home
/// 族重定向都锚它。两根只在 selfbuilt 形态重合）+ agent id + 常驻标记 +
/// 浏览器身份目录（P3 ④/T6：`NEMESISBOT_BROWSER_PROFILE` 是 MCP 启动链
/// 注入 `--user-data-dir` 的上下文标记——gateway 进程不设它，共享 MCP
/// server 行为零变化；仅执行体上下文内 spawn 的浏览器类 server 拿到
/// per-agent profile）。运输层专用键由调用方按路径补（PIPE /
/// SANDBOX_BACKEND）。
fn base_executor_env(
    exec_root: &std::path::Path,
    home: &std::path::Path,
    agent_id: &str,
) -> Vec<(String, String)> {
    vec![
        ("NEMESISBOT_ROLE".to_string(), "executor".to_string()),
        (
            "NEMESISBOT_EXECUTOR_WORKSPACE".to_string(),
            exec_root.to_string_lossy().into_owned(),
        ),
        (
            "NEMESISBOT_EXECUTOR_HOME".to_string(),
            home.to_string_lossy().into_owned(),
        ),
        (
            "NEMESISBOT_EXECUTOR_AGENT_ID".to_string(),
            agent_id.to_string(),
        ),
        (
            "NEMESISBOT_EXECUTOR_PERSISTENT".to_string(),
            "1".to_string(),
        ),
        (
            "NEMESISBOT_BROWSER_PROFILE".to_string(),
            home.join("browser-profile").to_string_lossy().into_owned(),
        ),
    ]
}

/// 优雅关停等待（sleep/watchdog：EOF 信号 → 进程自行退出 → 超时 kill）。
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// 崩溃自动重拉连续预算（T9 槽位看护）：执行体「起来又立刻死」的循环
/// 不放大多烧 CPU——超预算后停手，等下一次显式调用（execute/wake 不受限，
/// lazy spawn 路径与看护无关）。
const MAX_AUTO_RELAUNCH: u32 = 3;

/// 一个 agent 的常驻槽（会话 + 生命周期时间戳）。
struct AgentSlot {
    /// `None` 只出现在 shutdown 取走的瞬间（取走后槽随即离开 map）。
    session: Option<PersistentExecutorSession>,
    /// 生命周期观测（watchdog/诊断日志用）；idle 判定以 last_used 为准。
    spawned_at: Instant,
    last_used: Instant,
    /// 本槽的 exec_root（T9 槽位看护：崩溃自动重拉用——槽没了记录还在，
    /// 重拉无需回查 agents 表；与 spawn 时传给 [`base_executor_env`] 的
    /// 同一值）。
    exec_root: std::path::PathBuf,
}

impl AgentSlot {
    fn session(&mut self) -> &mut PersistentExecutorSession {
        self.session
            .as_mut()
            .expect("session taken only transiently by shutdown")
    }

    /// shutdown(self) 语义经 take 落地（MutexGuard 里移不出值）。
    async fn shutdown(&mut self, grace: Duration) {
        if let Some(s) = self.session.take() {
            s.shutdown(grace).await;
        }
    }
}

/// supervisor 执行结果错误（分型见模块文档）。
#[derive(Debug)]
pub enum SupervisorError {
    /// 常驻路径不可用（config 关闭 / 满员 / spawn 失败）——调用未发生，
    /// 调用方可安全回落 per-call。
    Unavailable(String),
    /// 调用已尝试且失败（工具报错 / 传输失败）——可能已有副作用，绝不回落。
    ToolFailed(String),
}

impl std::fmt::Display for SupervisorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(m) => write!(f, "persistent executor unavailable: {m}"),
            Self::ToolFailed(m) => write!(f, "persistent executor call failed: {m}"),
        }
    }
}

impl std::error::Error for SupervisorError {}

/// 槽位快照（T8 节点宣告观测面：AgentAnnouncement.state 的数据源）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct SlotState {
    pub agent_id: String,
    pub alive: bool,
    pub idle_secs: u64,
}

/// 常驻执行体监督器。所有方法 `&self`（内部锁），跨 agent 并发安全；
/// 同 agent 的调用被槽位互斥串行（协议是严格逐请求应答）。
pub struct AgentRuntimeSupervisor {
    exe_path: PathBuf,
    /// 宿主 APP home（`<home>/workspace/tools/sandboxie/` 的定位锚——P3 ②
    /// 常驻盒的 SandboxPaths 从这里派生，与 exec_world 通道装配同源）。
    home: PathBuf,
    workspace: PathBuf,
    config_path: PathBuf,
    slots: Mutex<HashMap<String, std::sync::Arc<tokio::sync::Mutex<AgentSlot>>>>,
    /// 崩溃自动重拉计数（T9 槽位看护熔断）：按 agent 连续计数，一次真实
    /// execute 成功或显式 wake 复位。idle/sleep 不计数（那是有意的停）。
    respawn_attempts: Mutex<HashMap<String, u32>>,
}

impl AgentRuntimeSupervisor {
    pub fn new(exe_path: PathBuf, home: PathBuf, workspace: PathBuf, config_path: PathBuf) -> Self {
        Self {
            exe_path,
            home,
            workspace,
            config_path,
            slots: Mutex::new(HashMap::new()),
            respawn_attempts: Mutex::new(HashMap::new()),
        }
    }

    /// config.json **现读**（与 tier `check_config_reload` 同纪律：磁盘是
    /// 唯一真相源，dashboard/CLI 改键即时生效，无快照表）。None = 持久化
    /// 档关闭（enabled=false / 键缺失 / 文件缺失 / 解析失败——全部归「不可
    /// 用」，不猜不兜底）。
    fn persistent_settings(&self) -> Option<ExecutorPersistentConfig> {
        let raw = std::fs::read_to_string(&self.config_path).ok()?;
        let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
        let cfg: ExecutorPersistentConfig =
            serde_json::from_value(v.get("executor")?.get("persistent")?.clone()).ok()?;
        cfg.enabled.then_some(cfg)
    }

    /// T7 接线判断面：持久化档当前是否开启。
    pub fn persistent_enabled(&self) -> bool {
        self.persistent_settings().is_some()
    }

    /// agent home（物理布局单一真相源 = nemesis-path）。
    pub fn agent_home(&self, agent_id: &str) -> PathBuf {
        resolve_agent_home_in_workspace(&self.workspace, agent_id)
    }

    /// 在常驻执行体内执行一次工具调用。失败分型见模块文档。
    ///
    /// `exec_root` 是本次调用的工作区锚（T7：来自 agents 表记录的
    /// `exec_root` 字段——main=workspace / project=项目目录 /
    /// selfbuilt=home，调用方经常驻路由闭包现查 manager 传入；spawn 时
    /// 写进子进程 `NEMESISBOT_EXECUTOR_WORKSPACE`）。同 agent 的并发调用
    /// 共用同一 exec_root（记录不可变字段），槽位互斥串行不受影响。
    pub async fn execute(
        &self,
        agent_id: &str,
        exec_root: &std::path::Path,
        tool: &str,
        args: &str,
        ctx: &RequestContext,
    ) -> Result<String, SupervisorError> {
        let settings = self
            .persistent_settings()
            .ok_or_else(|| SupervisorError::Unavailable("persistent disabled in config".into()))?;
        let context_value = serde_json::to_value(ctx)
            .map_err(|e| SupervisorError::Unavailable(format!("serialize context: {e}")))?;
        let call_id = new_call_id();
        let request_line =
            PersistentExecutorSession::build_v2_request_line(tool, args, &context_value, &call_id)
                .map_err(SupervisorError::Unavailable)?;

        let slot = self.get_or_spawn(agent_id, exec_root, &settings).await?;
        let mut guard = slot.lock().await;

        // 上一次调用留下的死进程 → 先重建再用（journal replay 使任何未决
        // 重发安全；此处无本调用方的在途副作用）。
        if !guard.session().is_alive() {
            tracing::info!(agent_id, "persistent executor dead before call; rebuilding");
            *guard = self
                .spawn_session(agent_id, exec_root)
                .await
                .map_err(|e| SupervisorError::Unavailable(format!("rebuild dead session: {e}")))?;
        }

        let timeout = Duration::from_secs(if settings.call_timeout_secs == 0 {
            FALLBACK_CALL_TIMEOUT_SECS
        } else {
            settings.call_timeout_secs
        });
        match guard
            .session()
            .call_raw(&request_line, Some(&call_id), timeout)
            .await
        {
            Ok(resp) => {
                guard.last_used = Instant::now();
                let out = Self::map_response(resp);
                if out.is_ok() {
                    // 一次真实往返成功 = 执行体健康：重拉预算复位。
                    self.clear_relaunch_counter(agent_id);
                }
                out
            }
            // 子进程死在调用中途：一次重建 + 原样重发同 call_id。已完成
            // 的调用回缓存结果（at-most-once）；未完成的此刻真跑一次
            // （at-least-once）。重建失败 = ToolFailed（不回落——调用已发生）。
            Err(TransportError::Eof) => {
                tracing::warn!(
                    agent_id,
                    tool,
                    call_id,
                    "persistent executor died mid-call; rebuilding and resending \
                     (journal replay dedupes completed calls)"
                );
                *guard = self.spawn_session(agent_id, exec_root).await.map_err(|e| {
                    SupervisorError::ToolFailed(format!("rebuild after crash: {e}"))
                })?;
                match guard
                    .session()
                    .call_raw(&request_line, Some(&call_id), timeout)
                    .await
                {
                    Ok(resp) => {
                        guard.last_used = Instant::now();
                        Self::map_response(resp)
                    }
                    Err(e) => Err(SupervisorError::ToolFailed(format!(
                        "{e}; stderr: {}",
                        guard.session().stderr_snapshot()
                    ))),
                }
            }
            // 活着但没回话（Timeout）/ 线损坏（Io/Protocol）：报错不重试
            // （重试 = 副作用重复；协议级串话更不该在同一会话上继续）。
            Err(e) => Err(SupervisorError::ToolFailed(format!(
                "{e}; stderr: {}",
                guard.session().stderr_snapshot()
            ))),
        }
    }

    /// sleep（P3 ⑤）：进程优雅退出，状态全在 home 落盘（home 即状态）。
    /// 返回 false = 槽本来就不在。
    pub async fn sleep_agent(&self, agent_id: &str) -> bool {
        let slot = {
            self.slots
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(agent_id)
        };
        let Some(slot) = slot else {
            return false;
        };
        let mut guard = slot.lock().await;
        tracing::info!(
            agent_id,
            "persistent executor sleeping (state lives in home)"
        );
        guard.shutdown(SHUTDOWN_GRACE).await;
        true
    }

    /// wake（P3 ⑤）：只 spawn 不调用（journal/ home 从盘上回载即状态）。
    /// `exec_root` 语义同 [`Self::execute`]。
    pub async fn wake_agent(
        &self,
        agent_id: &str,
        exec_root: &std::path::Path,
    ) -> Result<(), SupervisorError> {
        let settings = self
            .persistent_settings()
            .ok_or_else(|| SupervisorError::Unavailable("persistent disabled in config".into()))?;
        self.get_or_spawn(agent_id, exec_root, &settings)
            .await
            .map(|_| ())?;
        // 显式 wake = 操作员在场：重拉预算复位。
        self.clear_relaunch_counter(agent_id);
        Ok(())
    }

    /// 槽位快照（T8 宣告面）。调用中的槽取不到锁 = 活跃，按 alive 计。
    pub fn slot_states(&self) -> Vec<SlotState> {
        let map = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        map.iter()
            .map(|(id, slot)| match slot.try_lock() {
                Ok(mut guard) => SlotState {
                    agent_id: id.clone(),
                    alive: guard.session().is_alive(),
                    idle_secs: guard.last_used.elapsed().as_secs(),
                },
                Err(_) => SlotState {
                    agent_id: id.clone(),
                    alive: true,
                    idle_secs: 0,
                },
            })
            .collect()
    }

    /// 启动看护循环：死槽清理 + idle 自动睡眠（[`ExecutorPersistentConfig::
    /// idle_shutdown_secs`]，0=关）。持弱引用：supervisor 全体 drop 后循环
    /// 自行退出。
    pub fn start_watchdog(self: &std::sync::Arc<Self>) -> tokio::task::JoinHandle<()> {
        let this = std::sync::Arc::downgrade(self);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(15));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                let Some(sup) = this.upgrade() else { break };
                sup.watchdog_tick().await;
            }
        })
    }

    async fn watchdog_tick(&self) {
        let settings = self.persistent_settings();
        let mut dead = Vec::new();
        let mut idle = Vec::new();
        {
            let map = self.slots.lock().unwrap_or_else(|e| e.into_inner());
            for (id, slot) in map.iter() {
                // try_lock：拿不到 = 调用中 = 视为活跃，本 tick 跳过。
                let Ok(mut guard) = slot.try_lock() else {
                    continue;
                };
                if !guard.session().is_alive() {
                    // 崩溃自动重拉要 exec_root（槽清走后记录随槽消失，
                    // 先抄出来）。
                    dead.push((id.clone(), guard.exec_root.clone()));
                } else if let Some(s) = settings.as_ref()
                    && s.idle_shutdown_secs > 0
                    && guard.last_used.elapsed() >= Duration::from_secs(s.idle_shutdown_secs)
                {
                    idle.push(id.clone());
                }
            }
        }
        for (id, exec_root) in dead {
            self.remove_and_shutdown(&id, "crashed").await;
            self.auto_relaunch(&id, &exec_root).await;
        }
        for id in idle {
            self.remove_and_shutdown(&id, "idle").await;
        }
    }

    /// 崩溃槽自动重拉（T9 槽位看护）：watchdog 清走死槽后按同一 exec_root
    /// 重新 spawn（home 即状态——journal/浏览器 profile 从盘上回载）。连续
    /// 失败超 [`MAX_AUTO_RELAUNCH`] 熔断停手（等下一次显式调用复位预算）；
    /// 常驻档已关（运行中改键/配置损坏）不拉——槽位就此回收，节点宣告态
    /// 随 roster_state 自然降级。idle 睡眠不走这里（那是有意的停）。
    async fn auto_relaunch(&self, agent_id: &str, exec_root: &std::path::Path) {
        let Some(settings) = self.persistent_settings() else {
            tracing::info!(agent_id, "persistent disabled; crashed slot not relaunched");
            return;
        };
        // 预算闸在计数之前：没真正尝试就不记账（计数 = 实际重拉次数）。
        let attempts = {
            let mut map = self
                .respawn_attempts
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let prev = map.get(agent_id).copied().unwrap_or(0);
            if prev >= MAX_AUTO_RELAUNCH {
                None
            } else {
                let n = prev + 1;
                map.insert(agent_id.to_string(), n);
                Some(n)
            }
        };
        let Some(attempts) = attempts else {
            tracing::warn!(
                agent_id,
                "persistent executor auto-relaunch budget exhausted; \
                 waiting for an explicit execute/wake to retry"
            );
            return;
        };
        match self.get_or_spawn(agent_id, exec_root, &settings).await {
            Ok(_) => {
                tracing::info!(agent_id, attempts, "persistent executor auto-relaunched")
            }
            Err(e) => tracing::warn!(
                agent_id,
                attempts,
                error = %e,
                "persistent executor auto-relaunch failed"
            ),
        }
    }

    /// 重拉预算复位（execute 成功往返 / 显式 wake——证明执行体健康）。
    /// 不能挂到 `get_or_spawn` 上：auto_relaunch 自己也走它，挂那儿
    /// crash-loop 永远熔断不了。
    fn clear_relaunch_counter(&self, agent_id: &str) {
        self.respawn_attempts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(agent_id);
    }

    /// 测试观测面：当前重拉计数（熔断断言用）。
    #[cfg(test)]
    fn respawn_attempt_count(&self, agent_id: &str) -> u32 {
        self.respawn_attempts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(agent_id)
            .copied()
            .unwrap_or(0)
    }

    async fn remove_and_shutdown(&self, agent_id: &str, reason: &str) {
        let slot = {
            self.slots
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(agent_id)
        };
        let Some(slot) = slot else { return };
        let mut guard = slot.lock().await;
        tracing::info!(
            agent_id,
            reason,
            age_secs = guard.spawned_at.elapsed().as_secs(),
            idle_secs = guard.last_used.elapsed().as_secs(),
            "persistent executor shutting down"
        );
        guard.shutdown(SHUTDOWN_GRACE).await;
    }

    /// 取槽或 spawn 新槽（容量闸：满员 = Unavailable，并发插入在 map 锁内
    /// 复核防超卖；输掉的竞态 spawn 会被优雅关停）。
    async fn get_or_spawn(
        &self,
        agent_id: &str,
        exec_root: &std::path::Path,
        settings: &ExecutorPersistentConfig,
    ) -> Result<std::sync::Arc<tokio::sync::Mutex<AgentSlot>>, SupervisorError> {
        let existing = {
            let map = self.slots.lock().unwrap_or_else(|e| e.into_inner());
            map.get(agent_id).cloned()
        };
        if let Some(s) = existing {
            return Ok(s);
        }
        let fresh = self
            .spawn_session(agent_id, exec_root)
            .await
            .map_err(|e| SupervisorError::Unavailable(format!("spawn: {e}")))?;
        let arc = std::sync::Arc::new(tokio::sync::Mutex::new(fresh));
        let arc_ref = arc.clone();
        let winner = {
            let mut map = self.slots.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(s) = map.get(agent_id) {
                s.clone()
            } else if map.len() >= settings.max_slots as usize {
                return Err(SupervisorError::Unavailable(format!(
                    "persistent slots full ({})",
                    settings.max_slots
                )));
            } else {
                map.insert(agent_id.to_string(), arc.clone());
                arc
            }
        };
        if !std::sync::Arc::ptr_eq(&winner, &arc_ref) {
            // 输掉插入竞态：自己刚 spawn 的会话没人引用，优雅关停防孤儿进程。
            let mut g = arc_ref.lock().await;
            g.shutdown(Duration::from_secs(2)).await;
        }
        Ok(winner)
    }

    /// spawn 一个 persistent executor 子进程（隔离纪律见模块文档）。
    /// `exec_root` 写进子进程 `NEMESISBOT_EXECUTOR_WORKSPACE`（工具 cwd 锚，
    /// T7 双根拆分）；agent home 照旧从 agent_id 派生（HOME 重定向根）。
    async fn spawn_session(
        &self,
        agent_id: &str,
        exec_root: &std::path::Path,
    ) -> Result<AgentSlot, String> {
        let home = self.agent_home(agent_id);
        // P3 ②：Sandboxie 就绪 → per-agent 常驻盒；未就绪 → stdio 退化
        // （⑥ 兼容回退——无 Sandboxie 的机器常驻档照常可用）。
        #[cfg(all(feature = "sandbox", windows))]
        match self
            .try_spawn_boxed_session(agent_id, exec_root, &home)
            .await?
        {
            Some(session) => {
                let now = Instant::now();
                return Ok(AgentSlot {
                    session: Some(session),
                    spawned_at: now,
                    last_used: now,
                    exec_root: exec_root.to_path_buf(),
                });
            }
            None => {
                tracing::info!(
                    agent_id,
                    "sandboxie engine not ready; persistent executor falling back to stdio"
                );
            }
        }
        // 浏览器身份目录在真盘预建（与盒路径同纪律：目录在场，Chromium
        // 首写不依赖 mkdir 语义）。
        std::fs::create_dir_all(home.join("browser-profile"))
            .map_err(|e| format!("create agent browser-profile dir: {e}"))?;
        let mut cmd = tokio::process::Command::new(&self.exe_path);
        cmd.env_clear();
        cmd.envs(filter_env_allowlist(std::env::vars()));
        cmd.envs(home_redirect_env_pairs(&home));
        cmd.envs(base_executor_env(exec_root, &home, agent_id));
        // PLAN P3 Linux 项：`executor.sandbox=true` 时挂 U11 标记——子进程
        // exec_worker 启动自装 landlock/bwrap（与 per-call 非 Windows 臂同源
        // 语义，stdio 传输不变）。Windows 的盒臂在上面处理隔离；引擎未就绪
        // 退化 stdio 不挂标记（P24 ACL 档的 persistent 接入是后续项）。
        #[cfg(not(windows))]
        if self.executor_sandbox_requested() {
            cmd.env("NEMESISBOT_EXECUTOR_SANDBOX", "1");
        }
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let child = cmd
            .spawn()
            .map_err(|e| format!("spawn persistent executor (agent={agent_id}): {e}"))?;
        let session = PersistentExecutorSession::from_stdio_child(child)
            .map_err(|e| format!("wire persistent executor stdio (agent={agent_id}): {e}"))?;
        let now = Instant::now();
        Ok(AgentSlot {
            session: Some(session),
            spawned_at: now,
            last_used: now,
            exec_root: exec_root.to_path_buf(),
        })
    }

    /// executor.allow_network 现读（盒段 `AllowNetworkAccess=y|n` 的来源；
    /// 与 typed config 的 `#[serde(default)]` 同缺省 false）。
    #[cfg(all(feature = "sandbox", windows))]
    fn executor_allow_network(&self) -> bool {
        std::fs::read_to_string(&self.config_path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| v.get("executor")?.get("allow_network")?.as_bool())
            .unwrap_or(false)
    }

    /// executor.sandbox 现读（非 Windows persistent stdio 臂是否挂
    /// `NEMESISBOT_EXECUTOR_SANDBOX` 标记——子进程 exec_worker 启动时自装
    /// landlock/bwrap，U11 与 per-call 同源语义；缺键/缺段/文件缺失 =
    /// false，与 `ExecutorSeparationConfig` 的 serde default 一致）。
    #[cfg(not(windows))]
    fn executor_sandbox_requested(&self) -> bool {
        std::fs::read_to_string(&self.config_path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| v.get("executor")?.get("sandbox")?.as_bool())
            .unwrap_or(false)
    }

    /// P3 ②：Sandboxie 就绪时把常驻执行体 spawn 进 per-agent 盒
    /// （`NemesisAgent_<agent_id>`，经 Start.exe wrap，stdio 换具名管道——
    /// Start.exe 不转发 stdio，与 per-call 盒路径同约束）。
    ///
    /// - `Ok(None)` = 引擎未就绪（未安装 / 未启动 / 非本方安装）→ 调用方
    ///   退化 stdio 路径（⑥）；
    /// - `Err` = 引擎在但 spawn 链路失败（真盒起不来是硬故障——显式上抛，
    ///   不静默无盒：操作员明确装了 Sandboxie，静默降级违反预期）。
    #[cfg(all(feature = "sandbox", windows))]
    async fn try_spawn_boxed_session(
        &self,
        agent_id: &str,
        exec_root: &std::path::Path,
        home: &std::path::Path,
    ) -> Result<Option<PersistentExecutorSession>, String> {
        use nemesis_agent::executor_transport::StderrTail;
        use nemesis_agent::executor_transport::split_duplex;

        let paths = nemesis_sandbox::SandboxPaths::new(&self.home);
        if !paths.engine_ready() {
            return Ok(None);
        }
        let box_name = nemesis_sandbox::ini::agent_box_name(agent_id);
        let box_root = paths.base_dir.join("box").join(&box_name);

        // 盒内基础设施目录先在真盘建好（journal / 浏览器 profile 走
        // OpenFilePath 写穿透；目录在场消除首写 mkdir 的虚拟化歧义）。
        std::fs::create_dir_all(home.join(".runtime"))
            .map_err(|e| format!("create agent runtime dir: {e}"))?;
        std::fs::create_dir_all(home.join("browser-profile"))
            .map_err(|e| format!("create agent browser-profile dir: {e}"))?;

        // 盒段 upsert（每次 spawn 幂等——install/start 的整文件重写可能
        // 抹掉过 agent 盒段，spawn 侧自愈）。磁盘 RMW 放 blocking 线程。
        let ini_path = paths.ini_path.clone();
        let box_root_c = box_root.clone();
        let home_c = home.to_path_buf();
        let agent_id_c = agent_id.to_string();
        let allow_network = self.executor_allow_network();
        tokio::task::spawn_blocking(move || {
            nemesis_sandbox::ini::upsert_agent_box_section(
                &ini_path,
                &agent_id_c,
                &box_root_c,
                &home_c,
                allow_network,
            )
        })
        .await
        .map_err(|e| format!("agent box ini upsert join: {e}"))?
        .map_err(|e| format!("agent box ini upsert: {e}"))?;

        // 管道 server 先于 spawn（first_pipe_instance——子进程连接前必须
        // 已建好，与 per-call 管线路径同序列）。
        let pipe = nemesis_agent::executor_pipe::agent_pipe_name(
            agent_id,
            &nemesis_agent::executor_pipe::unique_pipe_id(),
        );
        let server = nemesis_agent::executor_pipe::create_server(&pipe)
            .map_err(|e| format!("create agent executor pipe: {e}"))?;

        let mut cmd = tokio::process::Command::new(paths.start_exe());
        cmd.arg(format!("/box:{box_name}"));
        // /hide_window → Start.exe 传 SW_HIDE 给盒内子进程（防黑窗闪现，
        // 与 per-call build_command 同款）。
        cmd.arg("/hide_window");
        cmd.arg(&self.exe_path);
        cmd.env_clear();
        cmd.envs(filter_env_allowlist(std::env::vars()));
        cmd.envs(home_redirect_env_pairs(home));
        cmd.envs(base_executor_env(exec_root, home, agent_id));
        cmd.env("NEMESISBOT_EXECUTOR_PIPE", &pipe);
        // 台账标签证据化：真盒才标 sandboxie 后端名（与 per-call 管线路径
        // 同纪律——无盒 PIPE 传输不冒标）。
        cmd.env("NEMESISBOT_SANDBOX_BACKEND", "sandboxie");
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::piped());
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = cmd.spawn().map_err(|e| {
            format!("spawn boxed persistent executor (agent={agent_id}, box={box_name}): {e}")
        })?;
        let stderr = StderrTail::spawn(child.stderr.take());

        // 等盒内子进程连上来（子进程要过 Start.exe + 盒启动，30s 与
        // per-call 管线路径同预算）；失败杀掉子进程不留孤儿。
        match tokio::time::timeout(Duration::from_secs(30), server.connect()).await {
            Err(_) => {
                let _ = child.start_kill();
                Err(format!(
                    "boxed persistent executor did not connect to pipe within 30s \
                     (agent={agent_id}, box={box_name})"
                ))
            }
            Ok(Err(e)) => {
                let _ = child.start_kill();
                Err(format!("agent executor pipe connect failed: {e}"))
            }
            Ok(Ok(())) => {
                let (read, write) = split_duplex(server);
                Ok(Some(PersistentExecutorSession::from_pipe_parts(
                    child, stderr, read, write,
                )))
            }
        }
    }

    fn map_response(resp: ExecutorResponseWire) -> Result<String, SupervisorError> {
        if resp.ok {
            Ok(resp.result)
        } else if resp.error.is_empty() {
            Err(SupervisorError::ToolFailed(
                "executor returned an error".into(),
            ))
        } else {
            Err(SupervisorError::ToolFailed(resp.error))
        }
    }

    /// 测试观测面：槽内子进程 pid（无槽 = None）。
    #[cfg(test)]
    pub async fn slot_pid(&self, agent_id: &str) -> Option<u32> {
        let slot = {
            let map = self.slots.lock().unwrap_or_else(|e| e.into_inner());
            map.get(agent_id).cloned()
        }?;
        let mut guard = slot.lock().await;
        guard.session().child.id()
    }
}

// ---------------------------------------------------------------------------
// T7 依赖倒置桥：supervisor 实现 agent crate 侧的 PersistentExecutor trait
// （nemesis-agent 不依赖 nemesisbot——trait 在 agent 侧定义，bot 侧实现；
// RemoteExecutorTool 经 Arc<dyn PersistentExecutor> 调进来）。分型转译是
// 纯映射：Unavailable 保持（回落判据），ToolFailed 保持（绝不回落）。
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
impl nemesis_agent::remote_executor_tool::PersistentExecutor for AgentRuntimeSupervisor {
    async fn execute_persistent(
        &self,
        agent_id: &str,
        exec_root: &std::path::Path,
        tool: &str,
        args: &str,
        context: &RequestContext,
    ) -> Result<String, nemesis_agent::remote_executor_tool::PersistentCallError> {
        use nemesis_agent::remote_executor_tool::PersistentCallError;
        self.execute(agent_id, exec_root, tool, args, context)
            .await
            .map_err(|e| match e {
                SupervisorError::Unavailable(m) => PersistentCallError::Unavailable(m),
                SupervisorError::ToolFailed(m) => PersistentCallError::ToolFailed(m),
            })
    }
}

#[cfg(test)]
mod tests;
