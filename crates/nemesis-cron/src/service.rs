//! Cron service for scheduled job execution.

use chrono::Local;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

/// Cron schedule definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronSchedule {
    pub kind: String,
    pub at_ms: Option<i64>,
    pub every_ms: Option<i64>,
    pub expr: Option<String>,
    pub tz: Option<String>,
}

/// Cron job payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronPayload {
    pub kind: String,
    pub message: String,
    pub command: Option<String>,
    pub deliver: bool,
    pub channel: Option<String>,
    pub to: Option<String>,
    /// Target agent conversation key (e.g. `agent:main:session:<sid>`). When
    /// set, the fired InboundMessage targets that conversation so the exchange
    /// is persisted into it (and, if a live WS tab is bound, pushed in real
    /// time). Empty/None = fall through to routing-derived keying.
    #[serde(default)]
    pub session_key: Option<String>,
    /// T3 (U12): per-fire tool-round budget for continuation turns. When set,
    /// the fired turn's tool-iteration cap uses this value instead of the
    /// global `max_tool_iterations` (a scheduled continuation must not burn
    /// the full 100-round budget). Exhaustion stops gracefully via the
    /// existing grace-round path — the job is NOT deleted; the next fire
    /// re-budgets. Set by the CronTool `continue_session` path (enum 5/10/20)
    /// and consumed by the gateway fire handler as `cron_max_rounds`
    /// metadata. `None` = no per-job budget (global default applies).
    #[serde(default)]
    pub max_rounds: Option<u32>,
}

/// Partial update for a cron job. Each field is optional: `None` = leave
/// unchanged. For the nullable string fields (`channel`/`to`/`session_key`),
/// `Some("")` means "clear". Used by the web `tasks.cron.update` handler.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CronJobPatch {
    pub name: Option<String>,
    pub schedule: Option<CronSchedule>,
    pub message: Option<String>,
    pub channel: Option<String>,
    pub to: Option<String>,
    pub session_key: Option<String>,
    pub enabled: Option<bool>,
    /// P1-2 (2026-08-24 UI entry gap): per-fire tool-round budget, patchable.
    /// Three states — `None` = leave unchanged, `Some(None)` = clear the
    /// budget (global default applies), `Some(Some(n))` = set the budget.
    /// Mirrors the `max_rounds` semantics of `CronPayload` / `add_job_ext`.
    pub max_rounds: Option<Option<u32>>,
}

/// One executed run of a cron job (for the task's run-history view).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronRunRecord {
    pub at_ms: i64,
    /// "ok" | "error" | "executed" (mirrors `last_status`).
    pub status: String,
    pub error: Option<String>,
}

/// Cron job state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJobState {
    pub next_run_at_ms: Option<i64>,
    pub last_run_at_ms: Option<i64>,
    pub last_status: Option<String>,
    pub last_error: Option<String>,
    /// Recent run history (newest last), capped to `MAX_HISTORY`. `#[serde(default)]`
    /// so jobs created before this field existed load with an empty history.
    #[serde(default)]
    pub history: Vec<CronRunRecord>,
}

/// Maximum number of past runs kept per job in `history`.
const MAX_HISTORY: usize = 50;

impl CronJobState {
    /// Append a run record (newest last), trimming to `MAX_HISTORY`.
    pub fn push_history(&mut self, at_ms: i64, status: String, error: Option<String>) {
        self.history.push(CronRunRecord {
            at_ms,
            status,
            error,
        });
        if self.history.len() > MAX_HISTORY {
            let drop_n = self.history.len() - MAX_HISTORY;
            self.history.drain(..drop_n);
        }
    }
}

/// Cron job definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub schedule: CronSchedule,
    pub payload: CronPayload,
    pub state: CronJobState,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub delete_after_run: bool,
}

/// Cron store persistence（单一真相源在 [`crate::store`]——CLI 与 gateway
/// 共用同一 loader/saver，禁止任何一方自行拼存储 JSON）。
use crate::store::CronStoreData;

/// Job handler callback type. Mirrors Go's `JobHandler func(job *CronJob) (string, error)`.
#[allow(dead_code)]
type JobHandlerFn = Box<dyn Fn(&CronJob) -> Result<String, String> + Send + Sync>;

/// 系统内部 cron 作业的保留名字前缀（单一真相源）。这些前缀是 gateway
/// on_job 分派分支（`ctx.rs` 按 `job.name.starts_with` 路由）与 board 启动
/// 孤儿清理（按前缀 + id 判定）的**按名约定**——用户作业撞名会被劫持
/// （message 被忽略、劫进系统分支）或被误删。用户面创建入口
/// （[`CronService::add_job`] / [`CronService::add_job_ext`]）loud 拒绝；
/// 内部装配点走 [`CronService::add_job_internal`]。
pub const RESERVED_NAME_PREFIXES: &[&str] = &["board-ap:", "memory-dreaming:"];

/// 名字命中的保留前缀（None = 不命中）。
pub fn reserved_prefix_hit(name: &str) -> Option<&'static str> {
    RESERVED_NAME_PREFIXES
        .iter()
        .copied()
        .find(|p| name.starts_with(p))
}

/// Cron service.
pub struct CronService {
    store_path: String,
    store: Arc<Mutex<CronStoreData>>,
    running: Arc<Mutex<bool>>,
    stop_handle: Arc<Mutex<Option<JoinHandle<()>>>>,
    on_job: Arc<Mutex<Option<Box<dyn Fn(&CronJob) -> Result<String, String> + Send + Sync>>>>,
    /// REL-002 损坏守卫（BUG-20261025-49，gateway 侧补位）：store 加载失败
    /// （损坏/半截/迁移失败）后置位——此后一切落盘拒绝，防止「空内存 store
    /// 写穿盘上未知数据」。CLI（`commands/cron.rs` 的 REL-002 守卫）早已
    /// 拒绝对损坏 store 做任何改动，gateway 侧此前只 warn + 空转，且
    /// `start()` 无条件 save —— 一次重启即把用户全部定时任务静默顶掉。
    /// `reload()` 成功 = 解毒（文件修复后的恢复入口）。
    load_poisoned: Arc<Mutex<bool>>,
    /// Startup arm gate (H1, U12): a fresh process starts DISARMED —
    /// persisted jobs do not fire until the owning startup path calls
    /// [`CronService::arm`] (the gateway does this once bus+agent are ready).
    /// This is the "restart → paused pending confirmation" stance: a restart
    /// must not silently resume autonomous
    /// scheduling. Jobs created AFTER startup are unaffected (armed latches
    /// for the process lifetime; only `new()` re-arms the gate).
    pending_arm: Arc<Mutex<bool>>,
}

/// W3（2026-10-25 加固）：job 暂停句柄——供
/// `on_job` handler 在触发期 pause 自身 job（父会话已删除 → 诚实停车）。
///
/// 为什么绕开 `Arc<Mutex<CronService>>`：handler 可能在调用方已持有外层
/// 互斥时执行（web `tasks.cron.run` 即时运行的 `svc.lock().execute_job()`），
/// handler 内再锁外层 = 自死锁；本句柄只携带 store + 存储路径克隆，直接走
/// store 互斥（触发循环调 handler 在「outside lock」段，store 锁空闲）。
#[derive(Clone)]
pub struct JobPauseHandle {
    store: Arc<Mutex<CronStoreData>>,
    store_path: String,
}

impl JobPauseHandle {
    /// 置 `enabled=false` 并落盘（幂等；不删 job——用户可在任务管理手动
    /// 重新启用，恢复后若父会话仍未重建，下次触发会再次诚实暂停）。
    /// 返回是否发生了状态变化（job 不存在或已停用 = false）。
    pub fn pause_job(&self, job_id: &str) -> bool {
        let mut s = self.store.lock();
        let Some(job) = s.jobs.iter_mut().find(|j| j.id == job_id) else {
            return false;
        };
        if !job.enabled {
            return false;
        }
        job.enabled = false;
        // W1b 不变量（对齐 toggle_job / enable_job / patch_job；BUG-20261025-49
        // 补位）：停用作业不排程——残留旧 next_run 会让禁用作业带上幻影
        // 「下次触发」，且 recompute_next_runs 只碰 enabled 作业，幻影跨重启
        // 存留。生产 W3 触发期路径此刻 next_run 已被触发循环置 None（本行
        // no-op），此行封的是非触发期调用面的不变量缺口。
        job.state.next_run_at_ms = None;
        job.updated_at_ms = Local::now().timestamp_millis();
        let _ = save_store_to_path(&self.store_path, &s);
        info!(
            "[Cron] Job paused via pause handle: id={} (disabled; manual re-enable allowed)",
            job_id
        );
        true
    }
}

impl CronService {
    /// Create a new cron service. Starts DISARMED (see [`Self::arm`]).
    pub fn new(store_path: &str) -> Self {
        info!(
            "[Cron] Service created, store_path={} (disarmed until arm())",
            store_path
        );
        let svc = Self {
            store_path: store_path.to_string(),
            store: Arc::new(Mutex::new(CronStoreData {
                version: 1,
                jobs: vec![],
            })),
            running: Arc::new(Mutex::new(false)),
            stop_handle: Arc::new(Mutex::new(None)),
            on_job: Arc::new(Mutex::new(None)),
            load_poisoned: Arc::new(Mutex::new(false)),
            pending_arm: Arc::new(Mutex::new(true)),
        };
        if let Err(e) = svc.load_store() {
            warn!("[Cron] Failed to load store on init: {}", e);
        }
        svc
    }

    /// Arm the service: allow persisted jobs to fire. Called once by the
    /// gateway after startup completes (bus + agent ready). Idempotent.
    pub fn arm(&self) {
        let mut p = self.pending_arm.lock();
        if *p {
            info!("[Cron] Service armed: persisted jobs may now fire");
            *p = false;
        }
    }

    /// Whether the startup gate is still closed.
    pub fn is_disarmed(&self) -> bool {
        *self.pending_arm.lock()
    }

    /// Start the cron service.
    pub async fn start(&self) -> Result<(), String> {
        info!("[Cron] Starting cron service");
        let mut running = self.running.lock();
        if *running {
            info!("[Cron] Cron service already running, skipping");
            return Ok(());
        }
        // REL-002 损坏守卫（BUG-20261025-49）：毒态下 start 的无条件
        // recompute+save 正是把空内存 store 写穿损坏文件的元凶——先拒绝，
        // 不落盘、不 spawn（毒态下循环内的 save_store_to_path 不可达：
        // 解毒唯一入口是 load_store 成功，即 reload()/重建服务）。
        if *self.load_poisoned.lock() {
            return Err(format!(
                "cron store 加载失败（损坏/不可解析），调度器拒绝启动以防空内存 \
                 store 落盘顶掉盘上任务；请修复或删除后重启/reload: {}",
                self.store_path
            ));
        }
        self.recompute_next_runs();
        self.save_store()?;

        *running = true;
        let running_flag = self.running.clone();
        let store = self.store.clone();
        let on_job = self.on_job.clone();
        let store_path = self.store_path.clone();
        let pending_arm_flag = self.pending_arm.clone();

        let handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            loop {
                interval.tick().await;
                if !*running_flag.lock() {
                    break;
                }
                // Check jobs — mirrors Go's checkJobs()
                let now_ms = Local::now().timestamp_millis();
                // H1 arm gate: while disarmed (fresh process, gateway startup
                // not yet complete), due jobs are NOT fired and their
                // next_run stays intact — they resume once arm() latches.
                if *pending_arm_flag.lock() {
                    continue;
                }
                let due: Vec<String> = {
                    let s = store.lock();
                    s.jobs
                        .iter()
                        .filter(|j| {
                            j.enabled && j.state.next_run_at_ms.is_some_and(|t| t <= now_ms)
                        })
                        .map(|j| j.id.clone())
                        .collect()
                };
                if due.is_empty() {
                    continue;
                }
                debug!("[Cron] Found {} due job(s)", due.len());

                // Reset next_run for due jobs and save (under lock), matching Go's
                // "reset before unlock to avoid duplicate execution" pattern.
                {
                    let mut s = store.lock();
                    for job in &mut s.jobs {
                        if due.contains(&job.id) {
                            job.state.next_run_at_ms = None;
                        }
                    }
                    let _ = save_store_to_path(&store_path, &s);
                }

                // Execute jobs outside lock — mirrors Go's checkJobs -> executeJobByID
                for job_id in &due {
                    // Read-lock: copy job for callback
                    let callback_job = {
                        let s = store.lock();
                        s.jobs.iter().find(|j| j.id == *job_id).cloned()
                    };
                    let Some(callback_job) = callback_job else {
                        continue;
                    };

                    info!(
                        "[Cron] Executing scheduled job: name={}, id={}",
                        callback_job.name, callback_job.id
                    );

                    // G3：触发账本开账（确定性 id=cron_job:{job}:{tick_ms}；
                    // 账本未安装=零变化，best-effort 不阻断触发）。单发
                    // 语义：进程死亡留给启动 sweep 诚实标 failed。
                    let ledger_run_id =
                        nemesis_taskstore::ledger::cron_job_run_id(&callback_job.id, now_ms);
                    nemesis_taskstore::ledger::begin_run(
                        &ledger_run_id,
                        "cron_job",
                        &callback_job.name,
                        "cron",
                        serde_json::json!({ "trigger": "schedule" }),
                    );

                    // Call on_job handler (outside lock)
                    let handler_result = {
                        let on_job = on_job.lock();
                        on_job.as_ref().map(|h| h(&callback_job))
                    };

                    // G3：触发账本收口（无 handler 也算触发完成——与 cron
                    // 自身 last_status="ok" 同口径）。
                    match &handler_result {
                        Some(Ok(_)) | None => {
                            nemesis_taskstore::ledger::settle_run(&ledger_run_id, "succeeded", None)
                        }
                        Some(Err(e)) => {
                            nemesis_taskstore::ledger::settle_run(&ledger_run_id, "failed", Some(e))
                        }
                    }

                    // Write-lock: update job state after execution
                    let start_time = now_ms;
                    {
                        let mut s = store.lock();
                        if let Some(job) = s.jobs.iter_mut().find(|j| j.id == *job_id) {
                            job.state.last_run_at_ms = Some(start_time);
                            job.updated_at_ms = Local::now().timestamp_millis();

                            match &handler_result {
                                Some(Ok(_)) => {
                                    job.state.last_status = Some("ok".to_string());
                                    job.state.last_error = None;
                                    info!(
                                        "[Cron] Scheduled job completed: name={}, id={}, status=ok",
                                        job.name, job.id
                                    );
                                }
                                Some(Err(e)) => {
                                    job.state.last_status = Some("error".to_string());
                                    job.state.last_error = Some(e.clone());
                                    error!(
                                        "[Cron] Scheduled job failed: name={}, id={}, error={}",
                                        job.name, job.id, e
                                    );
                                }
                                None => {
                                    // No handler configured
                                    job.state.last_status = Some("ok".to_string());
                                    job.state.last_error = None;
                                    debug!(
                                        "[Cron] Scheduled job executed (no handler): name={}, id={}",
                                        job.name, job.id
                                    );
                                }
                            }

                            // Append to run history (capped).
                            job.state.push_history(
                                start_time,
                                job.state.last_status.clone().unwrap_or_default(),
                                job.state.last_error.clone(),
                            );

                            // Compute next run time
                            if job.schedule.kind == "at" {
                                if job.delete_after_run
                                    && job.state.last_status.as_deref() == Some("ok")
                                {
                                    // 成功单发：下方 retain 删除，无需收尾。
                                } else {
                                    // 非删除单发（成败皆然）/ 失败的删除单发：
                                    // 停摆保留。失败的 delete_after_run 任务被
                                    // retain 留作证据，但必须一并 disable——
                                    // 留着 enabled=true + 过期 at_ms 的僵尸会在
                                    // 下次启动被 recompute_next_runs 的 H1 子句
                                    // 复活成 stale due time，每次网关重启都
                                    // 重放一次已失败的单发（副作用重复：建单/
                                    // LLM turn）。（BUG-HUNT R34）
                                    job.enabled = false;
                                    job.state.next_run_at_ms = None;
                                }
                            } else if job.enabled {
                                job.state.next_run_at_ms = compute_next_run(
                                    &job.schedule,
                                    Local::now().timestamp_millis(),
                                );
                            } else {
                                // BUG-20261025-41：作业在 handler 执行期内被并发
                                // 停用（W3 pause_job 生产路径——触发期父会话已删
                                // 除自动暂停，pause 落盘先于本段）不得回填
                                // next_run。裸重算会让停用作业带幻影「下次触发」
                                // 持久化且跨重启存留（recompute_next_runs 只碰
                                // enabled 作业）——违反 W1b 不变量（disabled 作业
                                // 不排程，与 toggle/enable/patch 同一约束）。
                                job.state.next_run_at_ms = None;
                            }
                        }
                        // Remove delete_after_run jobs that have completed
                        s.jobs.retain(|j| {
                            j.id != *job_id
                                || !j.delete_after_run
                                || j.state.last_status.as_deref() != Some("ok")
                        });
                        let _ = save_store_to_path(&store_path, &s);
                    }
                }
            }
        });

        *self.stop_handle.lock() = Some(handle);
        info!("[Cron] Cron service started successfully");
        Ok(())
    }

    /// Stop the cron service.
    pub fn stop(&self) {
        info!("[Cron] Stopping cron service");
        *self.running.lock() = false;
        if let Some(h) = self.stop_handle.lock().take() {
            h.abort();
        }
        info!("[Cron] Cron service stopped");
    }

    /// Add a new job (legacy 6-param signature — kept for existing callers and
    /// tests). Delegates to [`add_job_ext`] with
    /// `session_key=None, max_rounds=None, enabled=true`.
    pub fn add_job(
        &self,
        name: &str,
        schedule: CronSchedule,
        message: &str,
        deliver: bool,
        channel: Option<&str>,
        to: Option<&str>,
    ) -> Result<CronJob, String> {
        self.add_job_ext(
            name, schedule, message, deliver, channel, to, None, None, true,
        )
    }

    /// Add a new job with full control over `session_key`, `max_rounds` and
    /// `enabled`. This is the path used by the web UI (`tasks.cron.add`) and
    /// the agent's CronTool (`continue_session` + `max_rounds`).
    ///
    /// 保留命名空间（[`RESERVED_NAME_PREFIXES`]）loud 拒绝——`board-ap:`/
    /// `memory-dreaming:` 前缀是系统内部作业的按名路由命名空间（on_job
    /// 分派分支与启动孤儿清理都按名字前缀判定），用户作业撞名会在触发时
    /// 被劫持进系统分支（message 被忽略）、在 gateway 启动 sync 被当孤儿
    /// 误删。内部注册走 [`Self::add_job_internal`]。
    ///
    /// 死排程闸（BUG-20261024-11）：`enabled=true` 时若在创建时刻就算不出
    /// 任何下一次触发（过期 at / 非正或溢出的 every / 永不匹配的 cron /
    /// 未知 kind），loud 拒绝——否则任务会以「已排程」的假象永不触发。
    pub fn add_job_ext(
        &self,
        name: &str,
        schedule: CronSchedule,
        message: &str,
        deliver: bool,
        channel: Option<&str>,
        to: Option<&str>,
        session_key: Option<&str>,
        max_rounds: Option<u32>,
        enabled: bool,
    ) -> Result<CronJob, String> {
        if let Some(prefix) = reserved_prefix_hit(name) {
            return Err(format!(
                "cron job 名字以保留前缀 {prefix:?} 开头（系统内部作业命名空间），\
                 触发时会被劫持、启动同步时会被误删，请换名字: {name:?}"
            ));
        }
        // BUG-20261024-11：死排程闸——enabled 作业在创建时刻就算不出任何
        // 下一次触发（过期 at / 非正 every / 间隔溢出 / 永不匹配的 cron
        // 表达式 / 未知 kind）＝ 永远不会 fire 的静默僵尸任务，loud 拒绝
        // 而非假装已排程。disabled 作业放行（W1b 语义：不排程不触发，
        // enable_job 重算时才落 next_run）。
        if enabled && compute_next_run(&schedule, Local::now().timestamp_millis()).is_none() {
            return Err(format!(
                "调度永远不会有下一次触发，拒绝创建死排程任务: {name:?} \
                 （可能原因：at 已过期 / every 间隔非正或溢出 / cron 表达式永不匹配 / 调度类型未知）"
            ));
        }
        self.add_job_internal(
            name,
            schedule,
            message,
            deliver,
            channel,
            to,
            session_key,
            max_rounds,
            enabled,
        )
    }

    /// 内部注册旁路：跳过保留命名空间检查（autopilot `board-ap:{id}`、
    /// dreaming `memory-dreaming:sweep` 等系统作业的装配点专用）。用户面
    /// 入口一律走 [`Self::add_job_ext`] / [`Self::add_job`]。
    pub fn add_job_internal(
        &self,
        name: &str,
        schedule: CronSchedule,
        message: &str,
        deliver: bool,
        channel: Option<&str>,
        to: Option<&str>,
        session_key: Option<&str>,
        max_rounds: Option<u32>,
        enabled: bool,
    ) -> Result<CronJob, String> {
        let cron_expr = schedule.expr.as_deref().unwrap_or(schedule.kind.as_str());
        info!(
            "[Cron] Job added: name={}, schedule_kind={}, cron={}, enabled={}",
            name, schedule.kind, cron_expr, enabled
        );
        let now_ms = Local::now().timestamp_millis();
        let delete_after_run = schedule.kind == "at";
        let job = CronJob {
            id: generate_id(),
            name: name.to_string(),
            enabled,
            schedule: schedule.clone(),
            payload: CronPayload {
                kind: "agent_turn".to_string(),
                message: message.to_string(),
                command: None,
                deliver,
                channel: channel.map(|s| s.to_string()),
                to: to.map(|s| s.to_string()),
                session_key: session_key.map(|s| s.to_string()),
                max_rounds,
            },
            state: CronJobState {
                next_run_at_ms: if enabled {
                    compute_next_run(&schedule, now_ms)
                } else {
                    None
                },
                last_run_at_ms: None,
                last_status: None,
                last_error: None,
                history: Vec::new(),
            },
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
            delete_after_run,
        };
        {
            let mut s = self.store.lock();
            s.jobs.push(job.clone());
        }
        // BUG #16 (W1b): roll back the in-memory push when persistence fails.
        // Previously the job stayed in the store after an Err return — the
        // caller was told "create failed" while the job silently remained
        // scheduled (memory/disk divergence + duplicate-create hazard on
        // caller retry). Err must mean "not created".
        if let Err(e) = self.save_store() {
            self.store.lock().jobs.retain(|j| j.id != job.id);
            return Err(e);
        }
        Ok(job)
    }

    /// Remove a job. `Ok(false)` = 不存在；`Err` = 落盘失败（内存已回滚，
    /// 与 [`Self::add_job_ext`] / [`Self::enable_job`] 同一 BUG #16 纪律：
    /// Err 必须意味着"没删成"——吞掉保存错误会造成内存/磁盘分叉，调用方
    /// 以为删了而盘上任务照常触发）。
    pub fn remove_job(&self, job_id: &str) -> Result<bool, String> {
        info!("[Cron] Job remove requested: id={}", job_id);
        enum Outcome {
            Missing,
            Removed { idx: usize, job: CronJob },
        }
        let outcome = {
            let mut store = self.store.lock();
            match store.jobs.iter().position(|j| j.id == job_id) {
                Some(idx) => Outcome::Removed {
                    idx,
                    job: store.jobs.remove(idx),
                },
                None => Outcome::Missing,
            }
        };
        match outcome {
            Outcome::Missing => Ok(false),
            Outcome::Removed { idx, job } => {
                let save_err = {
                    let store = self.store.lock();
                    crate::store::save(std::path::Path::new(&self.store_path), &store).err()
                };
                if let Some(e) = save_err {
                    // 回滚：按原下标插回（save 失败 ⇒ 盘上内容未动，内存必须一致）。
                    let mut store = self.store.lock();
                    let idx = idx.min(store.jobs.len());
                    store.jobs.insert(idx, job);
                    return Err(e);
                }
                Ok(true)
            }
        }
    }

    /// List jobs.
    pub fn list_jobs(&self, include_disabled: bool) -> Vec<CronJob> {
        let store = self.store.lock();
        let jobs: Vec<CronJob> = if include_disabled {
            store.jobs.clone()
        } else {
            store.jobs.iter().filter(|j| j.enabled).cloned().collect()
        };
        debug!(
            "[Cron] Listing jobs, count={}, include_disabled={}",
            jobs.len(),
            include_disabled
        );
        jobs
    }

    /// Get status. Matches Go's `Status()` return:
    /// `{"enabled": bool, "jobs": int, "nextWakeAtMS": Option<i64>}`
    pub fn status(&self) -> serde_json::Value {
        let store = self.store.lock();
        let next_wake = store
            .jobs
            .iter()
            .filter(|j| j.enabled)
            .filter_map(|j| j.state.next_run_at_ms)
            .min();
        serde_json::json!({
            "enabled": *self.running.lock(),
            "jobs": store.jobs.len(),
            "nextWakeAtMS": next_wake,
        })
    }

    /// Get a job by ID.
    pub fn get_job(&self, job_id: &str) -> Option<CronJob> {
        self.store
            .lock()
            .jobs
            .iter()
            .find(|j| j.id == job_id)
            .cloned()
    }

    /// Update a job's name and/or schedule.
    pub fn update_job(
        &self,
        job_id: &str,
        name: Option<&str>,
        schedule: Option<CronSchedule>,
    ) -> Result<(), String> {
        info!("[Cron] Job updated: id={}", job_id);
        // 改名同受保留命名空间约束（与 add_job_ext 同一单一真相源）：改名
        // 绕过创建闸 = 用户作业可被改成 `board-ap:{i64}` 形态——启动孤儿
        // 清理按形态+未注册 id 静默删除、on_job 分派按形态劫持（message
        // 被忽略），创建时防线防的正是这两个危害。
        if let Some(n) = name
            && let Some(prefix) = reserved_prefix_hit(n)
        {
            return Err(format!(
                "新名字以保留前缀 {prefix:?} 开头（系统内部作业命名空间），\
                 改名后会被触发劫持、启动同步时会被误删，请换名字: {n:?}"
            ));
        }
        let now_ms = Local::now().timestamp_millis();
        let mut store = self.store.lock();
        let job = store
            .jobs
            .iter_mut()
            .find(|j| j.id == job_id)
            .ok_or_else(|| format!("job not found: {}", job_id))?;
        // BUG-20261025-30：死排程闸延伸到更新路径（BUG-20261024-11 的闸只设
        // 在创建漏斗 add_job_ext）——给启用中的作业换入算不出任何下一次触发
        // 的调度 = 以「已排程」假象成为永不触发的静默僵尸，loud 拒绝。校验
        // 在任何变更应用前，Err = 无副作用。
        if let Some(s) = &schedule
            && job.enabled
            && compute_next_run(s, now_ms).is_none()
        {
            return Err(format!(
                "新调度永远不会有下一次触发，拒绝将启用中任务更新为死排程: {job_id} \
                 （可能原因：at 已过期 / every 间隔非正或溢出 / cron 表达式永不匹配 / 调度类型未知）"
            ));
        }
        // BUG #16 sweep: snapshot for rollback on save failure (see add_job_ext).
        let before = job.clone();
        if let Some(n) = name {
            job.name = n.to_string();
        }
        if let Some(s) = schedule {
            job.schedule = s;
            // BUG-20261025-41：delete_after_run 是 kind=="at" 的派生旗标（与
            // add_job_internal 同一派生式），换调度必须重算——at→周期 的改期
            // 若保留旧 true，任务首次成功触发即被 retain 静默删除（周期任务
            // 无声消失、零日志）。
            job.delete_after_run = job.schedule.kind == "at";
            // W1b 不变量（对齐 patch_job）：disabled 作业不排程——裸写
            // next_run 会让禁用作业带上幻影「下次触发」。
            if job.enabled {
                job.state.next_run_at_ms = compute_next_run(&job.schedule, now_ms);
            }
        }
        job.updated_at_ms = now_ms;
        drop(store);
        if let Err(e) = self.save_store() {
            let mut store = self.store.lock();
            if let Some(job) = store.jobs.iter_mut().find(|j| j.id == job_id) {
                *job = before;
            }
            return Err(e);
        }
        Ok(())
    }

    /// Patch a job's fields (name/schedule/message/channel/to/session_key/enabled).
    /// Each `None` field is left unchanged; for `channel`/`to`/`session_key`,
    /// `Some("")` clears the value. Returns the updated job. Used by the web
    /// `tasks.cron.update` handler.
    pub fn patch_job(&self, job_id: &str, patch: &CronJobPatch) -> Result<CronJob, String> {
        info!("[Cron] Job patched: id={}", job_id);
        // 改名同受保留命名空间约束（单一真相源 [`reserved_prefix_hit`]，
        // 与 add_job_ext / update_job 同闸）：改名绕过创建闸 = 用户作业可被
        // 改成 `board-ap:{i64}` 形态——启动孤儿清理按形态+未注册 id 静默
        // 删除、on_job 分派按形态劫持。校验在任何变更应用前，Err = 无副作用。
        if let Some(n) = &patch.name
            && let Some(prefix) = reserved_prefix_hit(n)
        {
            return Err(format!(
                "新名字以保留前缀 {prefix:?} 开头（系统内部作业命名空间），\
                 改名后会被触发劫持、启动同步时会被误删，请换名字: {n:?}"
            ));
        }
        let now_ms = Local::now().timestamp_millis();
        let mut store = self.store.lock();
        let job = store
            .jobs
            .iter_mut()
            .find(|j| j.id == job_id)
            .ok_or_else(|| format!("job not found: {}", job_id))?;
        // BUG-20261025-30：死排程闸延伸到 patch 路径——按 patch 后的最终
        // (schedule, enabled) 组合判定：enabled 且算不出任何下一次触发 =
        // 永不触发的静默僵尸，loud 拒绝。校验在任何变更应用前，Err = 无副
        // 作用（与上方保留前缀检查同位）。禁用终态放行（W1b：不排程不触发，
        // 与 add_job_ext 对 disabled 死排程放行同语义；board 自动驾驶同步
        // 另有 BUG-20261025-18 挂载防线前置，dreaming 更新失败走 warn 保留
        // 旧有效排程）。
        let final_schedule = patch
            .schedule
            .clone()
            .unwrap_or_else(|| job.schedule.clone());
        let final_enabled = patch.enabled.unwrap_or(job.enabled);
        if final_enabled && compute_next_run(&final_schedule, now_ms).is_none() {
            return Err(format!(
                "patch 后的调度永远不会有下一次触发，拒绝死排程更新: {job_id} \
                 （可能原因：at 已过期 / every 间隔非正或溢出 / cron 表达式永不匹配 / 调度类型未知）"
            ));
        }
        // BUG #16 sweep: snapshot for rollback on save failure (see add_job_ext).
        let before = job.clone();
        if let Some(n) = &patch.name {
            job.name = n.clone();
        }
        if let Some(sched) = &patch.schedule {
            job.schedule = sched.clone();
            // BUG-20261025-41：delete_after_run 派生旗标随 kind 重算（同
            // update_job——at→周期改期后不得保留单发删除语义）。
            job.delete_after_run = job.schedule.kind == "at";
            if job.enabled {
                job.state.next_run_at_ms = compute_next_run(&job.schedule, now_ms);
            }
        }
        if let Some(m) = &patch.message {
            job.payload.message = m.clone();
        }
        if let Some(c) = patch.channel.as_ref() {
            job.payload.channel = if c.is_empty() { None } else { Some(c.clone()) };
        }
        if let Some(t) = patch.to.as_ref() {
            job.payload.to = if t.is_empty() { None } else { Some(t.clone()) };
        }
        if let Some(sk) = patch.session_key.as_ref() {
            job.payload.session_key = if sk.is_empty() {
                None
            } else {
                Some(sk.clone())
            };
        }
        // P1-2: three-state budget patch — Some(None) clears (global default
        // applies), Some(Some(n)) sets; None leaves unchanged.
        if let Some(new_budget) = patch.max_rounds {
            job.payload.max_rounds = new_budget;
        }
        if let Some(en) = patch.enabled {
            job.enabled = en;
            job.state.next_run_at_ms = if en {
                compute_next_run(&job.schedule, now_ms)
            } else {
                None
            };
        }
        job.updated_at_ms = now_ms;
        let updated = job.clone();
        drop(store);
        if let Err(e) = self.save_store() {
            let mut store = self.store.lock();
            if let Some(job) = store.jobs.iter_mut().find(|j| j.id == job_id) {
                *job = before;
            }
            return Err(e);
        }
        Ok(updated)
    }

    /// Toggle a job's enabled state. Returns the new state.
    pub fn toggle_job(&self, job_id: &str) -> Result<bool, String> {
        let now_ms = Local::now().timestamp_millis();
        let mut store = self.store.lock();
        let job = store
            .jobs
            .iter_mut()
            .find(|j| j.id == job_id)
            .ok_or_else(|| format!("job not found: {}", job_id))?;
        // BUG #16 sweep: snapshot for rollback on save failure (see add_job_ext).
        // toggle is NOT idempotent — without rollback, a caller retrying after
        // a save failure would flip the job back.
        let before = job.clone();
        // BUG-20261025-30：死排程闸延伸到 toggle 翻正方向（同 enable_job）
        // ——休眠死排程翻正 = 永不触发的静默僵尸，loud 拒绝。校验在任何变
        // 更应用前，Err = 无副作用（toggle 非幂等，拒绝后保持原状态）。
        let enabling = !job.enabled;
        let next = if enabling {
            compute_next_run(&job.schedule, now_ms)
        } else {
            None
        };
        if enabling && next.is_none() {
            return Err(format!(
                "调度永远不会有下一次触发，拒绝启用死排程任务: {job_id} \
                 （可能原因：at 已过期 / every 间隔非正或溢出 / cron 表达式永不匹配 / 调度类型未知）"
            ));
        }
        job.enabled = enabling;
        // 翻正 = 闸已保证 Some；停用 = W1b 不变量（对齐 enable_job /
        // patch_job）：disabled 作业不排程——残留旧 next_run 会让禁用作业
        // 带上幻影「下次触发」。
        job.state.next_run_at_ms = next;
        let new_state = job.enabled;
        info!("[Cron] Job toggled: id={}, enabled={}", job_id, new_state);
        job.updated_at_ms = now_ms;
        drop(store);
        if let Err(e) = self.save_store() {
            let mut store = self.store.lock();
            if let Some(job) = store.jobs.iter_mut().find(|j| j.id == job_id) {
                *job = before;
            }
            return Err(e);
        }
        Ok(new_state)
    }

    /// Enable or disable a specific job. Mirrors Go's `EnableJob(jobID, enabled)`.
    /// Returns the updated job if found.
    pub fn enable_job(&self, job_id: &str, enabled: bool) -> Result<CronJob, String> {
        info!(
            "[Cron] Job enable/disable: id={}, enabled={}",
            job_id, enabled
        );
        let now_ms = Local::now().timestamp_millis();
        let mut store = self.store.lock();
        let job = store
            .jobs
            .iter_mut()
            .find(|j| j.id == job_id)
            .ok_or_else(|| format!("job not found: {}", job_id))?;
        // BUG-20261025-30：死排程闸延伸到启用路径——休眠的死排程作业（如
        // 过期 at）启用 = 以「已启用」假象成为永不触发的静默僵尸，loud 拒
        // 绝。校验在任何变更应用前，Err = 无副作用。禁用方向不设闸。
        let next = if enabled {
            compute_next_run(&job.schedule, now_ms)
        } else {
            None
        };
        if enabled && next.is_none() {
            return Err(format!(
                "调度永远不会有下一次触发，拒绝启用死排程任务: {job_id} \
                 （可能原因：at 已过期 / every 间隔非正或溢出 / cron 表达式永不匹配 / 调度类型未知）"
            ));
        }
        // BUG #16 sweep: snapshot for rollback on save failure (see add_job_ext).
        let before = job.clone();
        job.enabled = enabled;
        job.state.next_run_at_ms = next;
        job.updated_at_ms = now_ms;
        let updated = job.clone();
        drop(store);
        if let Err(e) = self.save_store() {
            let mut store = self.store.lock();
            if let Some(job) = store.jobs.iter_mut().find(|j| j.id == job_id) {
                *job = before;
            }
            return Err(e);
        }
        Ok(updated)
    }

    /// Reload jobs from disk. Mirrors Go's `CronService.Load()`.
    /// 加载成功 = 清除 REL-002 毒态（文件修复后的恢复入口）；失败 = 置毒。
    pub fn reload(&self) -> Result<(), String> {
        self.load_store()
    }

    /// Set the job handler callback. Mirrors Go's `SetOnJob(handler)`.
    /// When set, the handler is called when a cron job fires.
    pub fn set_on_job(
        &self,
        handler: impl Fn(&CronJob) -> Result<String, String> + Send + Sync + 'static,
    ) {
        *self.on_job.lock() = Some(Box::new(handler));
    }

    /// W3（2026-10-25 加固）：取 job 暂停句柄（见 [`JobPauseHandle`]）。
    /// 在 `set_on_job` 前调用，闭包捕获句柄，触发期可安全 pause 自身 job。
    pub fn job_pause_handle(&self) -> JobPauseHandle {
        JobPauseHandle {
            store: Arc::clone(&self.store),
            store_path: self.store_path.clone(),
        }
    }

    /// Execute a job immediately by ID. Unlike the scheduled fire loop this
    /// does NOT advance `next_run_at_ms` (the schedule still fires normally) —
    /// it's a manual "run now". Invokes the `on_job` handler so the agent
    /// actually runs (the previous implementation only updated state and never
    /// fired the handler, so "run now" was a no-op for delivery). State is
    /// marked `"executed"` on success / `"error"` on handler failure.
    pub fn execute_job(&self, job_id: &str) -> Result<(), String> {
        let start = std::time::Instant::now();
        info!("[Cron] Executing job: id={}", job_id);
        // REL-002 损坏守卫（BUG-20261025-49）：毒态下拒绝「先触发后记账」
        // ——handler 副作用已发生而状态落不了盘 = 记账与事实分叉。先拒绝。
        if *self.load_poisoned.lock() {
            return Err(format!(
                "cron store 加载失败（损坏/不可解析），拒绝手动触发（状态无法落盘）;\
                 请修复或删除后重载: {}",
                self.store_path
            ));
        }
        let now_ms = Local::now().timestamp_millis();

        // Read-lock: copy job for callback (mirror the fire loop pattern).
        let callback_job = {
            let s = self.store.lock();
            s.jobs.iter().find(|j| j.id == job_id).cloned()
        };
        let Some(callback_job) = callback_job else {
            error!("[Cron] Job not found for execution: id={}", job_id);
            return Err(format!("job not found: {}", job_id));
        };

        // G3：手动触发同款账本（trigger=manual 与计划触发区分；账本未
        // 安装=零变化）。
        let ledger_run_id = nemesis_taskstore::ledger::cron_job_run_id(job_id, now_ms);
        nemesis_taskstore::ledger::begin_run(
            &ledger_run_id,
            "cron_job",
            &callback_job.name,
            "cron",
            serde_json::json!({ "trigger": "manual" }),
        );

        // Call on_job handler (outside lock).
        let handler_result = {
            let on_job = self.on_job.lock();
            on_job.as_ref().map(|h| h(&callback_job))
        };

        // G3：手动触发收口（None=无 handler，同 execute 语义视为完成）。
        match &handler_result {
            Some(Ok(_)) | None => {
                nemesis_taskstore::ledger::settle_run(&ledger_run_id, "succeeded", None)
            }
            Some(Err(e)) => {
                nemesis_taskstore::ledger::settle_run(&ledger_run_id, "failed", Some(e))
            }
        }

        // Write-lock: update state after execution.
        {
            let mut store = self.store.lock();
            if let Some(job) = store.jobs.iter_mut().find(|j| j.id == job_id) {
                job.state.last_run_at_ms = Some(now_ms);
                job.updated_at_ms = Local::now().timestamp_millis();
                match &handler_result {
                    Some(Ok(_)) | None => {
                        job.state.last_status = Some("executed".to_string());
                        job.state.last_error = None;
                    }
                    Some(Err(e)) => {
                        job.state.last_status = Some("error".to_string());
                        job.state.last_error = Some(e.clone());
                        error!(
                            "[Cron] Manual execute failed: name={}, id={}, error={}",
                            job.name, job.id, e
                        );
                    }
                }
                // Append to run history (capped).
                job.state.push_history(
                    now_ms,
                    job.state.last_status.clone().unwrap_or_default(),
                    job.state.last_error.clone(),
                );
            }
            drop(store);
            self.save_store()?;
        }
        let elapsed = start.elapsed().as_millis();
        info!(
            "[Cron] Job executed: name={}, id={}, duration_ms={}",
            callback_job.name, job_id, elapsed
        );
        Ok(())
    }

    /// Validate a cron expression. Supports both 5-field (min hour day month weekday)
    /// and 6-field (sec min hour day month weekday) expressions.
    /// Also supports L, W, #, JAN-DEC, SUN-SAT names — matching Go's gronx.
    pub fn validate_schedule(expr: &str) -> Result<(), String> {
        if expr.trim().is_empty() {
            return Err("cron expression is empty".to_string());
        }
        croner::Cron::from_str(expr)
            .map_err(|e| format!("invalid cron expression '{}': {}", expr, e))?;
        Ok(())
    }

    /// Parse a cron expression and return a human-readable description.
    pub fn describe_schedule(expr: &str) -> String {
        // Accept both 5-field and 6-field
        let parts: Vec<&str> = expr.split_whitespace().collect();
        if parts.len() < 5 || parts.len() > 6 {
            return format!("Invalid: {}", expr);
        }

        // Determine offset: 6-field has seconds at index 0
        let has_seconds = parts.len() == 6;
        let off = if has_seconds { 1 } else { 0 };

        let minute = parts[off];
        let hour = parts[off + 1];
        let day = parts[off + 2];
        let month = parts[off + 3];
        let weekday = parts[off + 4];

        // Every minute: all wildcard
        if minute == "*" && hour == "*" && day == "*" && month == "*" && weekday == "*" {
            return "Every minute".to_string();
        }
        // Every N minutes
        if let Some(step) = minute.strip_prefix("*/")
            && hour == "*"
            && day == "*"
            && month == "*"
            && weekday == "*"
        {
            return format!("Every {} minutes", step);
        }
        // Daily at specific time
        if minute != "*" && hour != "*" && day == "*" && month == "*" && weekday == "*" {
            return format!("Daily at {}:{}", hour, minute);
        }
        // Weekday schedule
        if weekday != "*" && day == "*" && month == "*" {
            return format!("At {}:{} on weekdays {}", hour, minute, weekday);
        }

        expr.to_string()
    }

    fn load_store(&self) -> Result<(), String> {
        match crate::store::load(std::path::Path::new(&self.store_path)) {
            Ok(store) => {
                *self.store.lock() = store;
                // 加载成功 = 解毒（文件修复后 reload/restart 的恢复入口）。
                *self.load_poisoned.lock() = false;
                Ok(())
            }
            Err(e) => {
                // 加载失败 = 内存停在空 store，此态下任何落盘都会顶掉盘上
                // 未知数据——置毒，save/start/execute 全面拒绝（见 save_store）。
                *self.load_poisoned.lock() = true;
                Err(e)
            }
        }
    }

    fn save_store(&self) -> Result<(), String> {
        if *self.load_poisoned.lock() {
            return Err(format!(
                "cron store 加载失败（损坏/不可解析），拒绝落盘以防空内存 store \
                 顶掉盘上任务；请修复或删除后重载: {}",
                self.store_path
            ));
        }
        let s = self.store.lock();
        crate::store::save(std::path::Path::new(&self.store_path), &s)
    }

    fn recompute_next_runs(&self) {
        let now_ms = Local::now().timestamp_millis();
        let disarmed = self.is_disarmed();
        let mut store = self.store.lock();
        for job in &mut store.jobs {
            if job.enabled {
                job.state.next_run_at_ms = compute_next_run(&job.schedule, now_ms);
                // H1: while DISARMED, a past-due one-shot "at" job keeps its
                // stale due time instead of being recomputed to None — an
                // "at" job whose time passed during downtime must fire once
                // when the service arms (compute_next_run drops it because
                // at_ms <= now; that drop is correct only for a fully
                // restarted schedule, not a gated one). BUG-HUNT R34: 只限
                // 「从未执行过」的单发（last_run=None）——已执行失败的僵尸
                // （error 被 retain 留证）不得每次重启重放；last_run=Some =
                // 单发语义已消耗，重试走用户显式 update_job 换新 at_ms。
                if disarmed
                    && job.state.next_run_at_ms.is_none()
                    && job.schedule.kind == "at"
                    && job.state.last_run_at_ms.is_none()
                {
                    job.state.next_run_at_ms = job.schedule.at_ms;
                }
            }
        }
    }

    /// Get the next wake time in milliseconds (mirrors Go's getNextWakeMS).
    #[allow(dead_code)]
    fn get_next_wake_ms(&self) -> Option<i64> {
        let s = self.store.lock();
        s.jobs
            .iter()
            .filter(|j| j.enabled)
            .filter_map(|j| j.state.next_run_at_ms)
            .min()
    }
}

/// Save store data to a file path. Used by both the service method and the spawn loop.
fn save_store_to_path(path: &str, data: &CronStoreData) -> Result<(), String> {
    crate::store::save(std::path::Path::new(path), data)
}

/// u64 秒 → i64 毫秒的唯一受检换算（CLI `cron add --every` 与 agent
/// CronTool 共用的单一真相源）。溢出 = 诚实 Err，绝不回绕：回绕成负值
/// 会产生永不触发的静默僵尸任务（`compute_next_run` 按 ms > 0 过滤），
/// 正值错拍则产生错误间隔。
pub fn secs_to_ms_checked(secs: u64) -> Result<i64, String> {
    i64::try_from(secs)
        .ok()
        .and_then(|s| s.checked_mul(1000))
        .ok_or_else(|| "seconds value too large".to_string())
}

/// cron 表达式「能否触发」组合判定（语法 + 语义，BUG-20261025-18 的单一
/// 真相源）：解析失败 = false；解析通过但从 `now_ms` 起算不出任何下一次
/// 触发（永不匹配，如 `0 0 30 2 *`——2 月没有 30 日）= false。挂载面用
/// 它在 enabled 作业上拒绝死排程（BUG-20261024-11 不变量覆盖
/// add_job_internal 内部装配路径——autopilot/dreaming 挂载都走旁路）。
pub fn cron_expr_can_fire(expr: &str, now_ms: i64) -> bool {
    if CronService::validate_schedule(expr).is_err() {
        return false;
    }
    let schedule = CronSchedule {
        kind: "cron".to_string(),
        at_ms: None,
        every_ms: None,
        expr: Some(expr.to_string()),
        tz: None,
    };
    compute_next_run(&schedule, now_ms).is_some()
}

pub fn compute_next_run(schedule: &CronSchedule, now_ms: i64) -> Option<i64> {
    match schedule.kind.as_str() {
        "at" => schedule.at_ms.filter(|&t| t > now_ms),
        "every" => schedule
            .every_ms
            .filter(|&ms| ms > 0)
            // BUG-20261024-11：受检加法——超大间隔把 next_run 推出 i64
            // 毫秒时间戳时 honest None，绝不回绕成负值（裸加法回绕负值
            // 会让 next_run <= now 恒真 → 每 tick 自旋触发 = 触发风暴）。
            .and_then(|ms| now_ms.checked_add(ms)),
        "cron" => {
            let expr = match &schedule.expr {
                Some(e) if !e.is_empty() => e,
                _ => return None,
            };

            let now = chrono::DateTime::from_timestamp_millis(now_ms)
                .map(|dt| dt.with_timezone(&chrono::Local))
                .unwrap_or_else(chrono::Local::now);

            let cron = match croner::Cron::from_str(expr) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("[cron] failed to parse expr '{}': {}", expr, e);
                    return None;
                }
            };

            // Interpret the cron fields in the configured timezone, or LOCAL
            // when none is specified. The web UI never sets tz, so an expression
            // like "0 9 * * *" must mean 09:00 local — the previous default of
            // UTC made "0 9" fire at 09:00 UTC (e.g. 17:00 in UTC+8). `now` is
            // already DateTime<Local>, so passing it to croner directly makes it
            // evaluate the fields against local time.
            let tz_to_use: Option<chrono_tz::Tz> = match schedule.tz.as_deref() {
                Some(tz_str) => match tz_str.parse() {
                    Ok(tz) => Some(tz),
                    Err(_) => {
                        tracing::warn!("[cron] invalid timezone '{}', using local", tz_str);
                        None
                    }
                },
                None => None,
            };
            let next_ms = match tz_to_use {
                Some(tz) => cron
                    .find_next_occurrence(&now.with_timezone(&tz), false)
                    .map(|dt| dt.with_timezone(&chrono::Local).timestamp_millis()),
                None => cron
                    .find_next_occurrence(&now, false)
                    .map(|dt| dt.with_timezone(&chrono::Local).timestamp_millis()),
            };
            match next_ms {
                Ok(ms) => Some(ms),
                Err(e) => {
                    tracing::warn!(
                        "[cron] failed to compute next run for expr '{}': {}",
                        expr,
                        e
                    );
                    None
                }
            }
        }
        _ => None,
    }
}

fn generate_id() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let bytes: [u8; 8] = rng.r#gen();
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod w1b_tests;
