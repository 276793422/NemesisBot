//! （devtool-upgrade）：网关级后台进程注册表 + 四动词工具
//! （start / output / kill / steer，steer 为 2026-11-03 扩展）。
//!
//! 起一个长跑进程拿到句柄，随后分页读输出、随时终止、（可选）经 stdin
//! 活管道中途注入输入。此前唯一的异步形态
//! `exec_async` 是 fire-and-forget——`wait_seconds` 后 `Child` 直接 drop（无
//! kill_on_drop），管道被丢弃导致句柄与输出全部不可回读。
//!
//! 生命周期 = 网关进程：注册表是 gateway 级单例（`SharedResources` 持有），
//! 跨 AgentLoop 重启存活；注册表 Drop 时给所有存活任务发 kill 旗标（监督
//! 任务 ≤200ms 内收尸），`kill_on_drop(true)` 兜底 runtime 关停路径。
//!
//! **不进 [`MOVE_TOOLS`](crate::remote_executor_tool:MOVE_TOOLS)**：executor
//! 分离是 per-call 子进程，一次调用后退出——在子进程里起后台任务，句柄必然
//! 随子进程孤儿化。因此四动词固定在 gateway 进程内跑（与 `exec_async` 同一
//! 决策，见 remote_executor_tool.rs 顶部注记），`exec_worker` 侧不注入注册表
//! （SharedToolConfig.background_registry = None → 工具不注册）。
//!
//! 安全：`background_start` 语义等同 `exec`（起进程）→ 8 层管线按
//! ProcessExec 审查命令本体；`background_output` / `background_kill` 只操作
//! 本注册表内的自有任务（命令已在 start 时过闸，读缓冲/杀自属子进程不构成
//! 新的攻击面）→ 显式映射 `Internal`（LOW）。`background_steer`（
//! 2026-11-03）：向自有任务 stdin 注入文本 = 操控进程行为，风险与起进程
//! 同级 → 显式映射 `ProcessExec`（exec 类审批口径对齐，清单既定项）；steer
//! 文本作为工具入参照常过注入检测层。 盲点修复后管线未知名 =
//! CRITICAL fail-closed，不再有「未知名放行」分支可搭（types.rs 表内
//! 显式条目，例外语义保留但不再依赖兜底行为）。
//!
//! stdin 活管道（2026-11-03）：`start(.., steerable=true)` 保留 child
//! stdin（此前恒 `Stdio::null()`），`background_steer` 向其写字节（text +
//! 换行 + flush）。registry 层只管字节——协议帧封装（stream-json /
//! app-server 帧协议）是 delegate_registry 的职责，不在本层重复。缺省
//! steerable=false 零行为变化（读 stdin 的命令照旧即刻 EOF，不会因活管道
//! 挂住）。写入队列 = stdin Mutex 的锁队列（tokio Mutex FIFO）——并发
//! steer 串行落盘不交错撕裂。
//!
//! 输出上限：每任务 [`DEFAULT_MAX_OUTPUT_BYTES`] 字节环形语义——超限丢头部
//! 保尾部，`dropped_bytes` 如实上报（offset 分页坐标按 `produced` 单调计）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Mutex;
use tokio::time::sleep;
use tracing::debug;

use nemesis_path::paths::canonicalize_for_compare;

use crate::context::RequestContext;
use crate::r#loop::Tool;

/// 注册表默认任务上限（验收：16 上限拒绝）。满员时优先淘汰最旧的
/// **已完成**任务（输出仍可读期间照常挤占——超出 16 就得腾地方），全部
/// 在跑才拒绝新任务。
pub const DEFAULT_MAX_JOBS: usize = 16;

/// 每任务输出缓冲上限（丢头部保尾部）。
pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 256 * 1024;

/// `background_output` 单次返回的 chunk 上限（模型友好分页粒度）。
pub const MAX_CHUNK_BYTES: usize = 8 * 1024;

/// 监督任务轮询子进程退出的间隔。
const SUPERVISE_INTERVAL: Duration = Duration::from_millis(200);

/// 完成唤醒通告携带的输出尾部窗口（字节）。
pub const COMPLETION_TAIL_BYTES: usize = 4 * 1024;

/// `background_output wait_secs` 的钳制上限（秒）——工具调用不该长阻塞
/// dispatch（完成等待的正常路径是完成唤醒，不是长轮询）。
pub const MAX_OUTPUT_WAIT_SECS: u64 = 30;

/// 完成唤醒的发起会话锚（2026-11-08 子进程实时输出与完成唤醒）：
/// `background_start` 时从 [`RequestContext`] 捕获的三元组快照。任务退出
/// 时 gateway 侧 watcher 据此合成 [`InboundMessage`](nemesis_types::channel::InboundMessage)
/// （sender="system"）经 bus 走标准入站路径回灌发起会话——模型自动收到
/// 完成通告与输出尾部，无需 sleep 轮询（行业对照：openclaw notifyOnExit
/// 同款「完成后自动通知」形态）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobOrigin {
    /// 发起通道（"web" 等——出站回复经同通道回投）。
    pub channel: String,
    /// 发起会话的 chat id。
    pub chat_id: String,
    /// 发起会话的 agent session key（round-trip 保会话归属）。
    pub session_key: String,
}

/// 任务完成事件（completion sink 消费面）。`set_completion_sink` 的
/// 接收端在 gateway 装配期取走并 spawn 常驻回灌任务。
#[derive(Debug, Clone)]
pub struct CompletionEvent {
    pub job_id: u64,
    pub status: JobStatus,
    pub origin: JobOrigin,
    /// 完成时刻的输出尾部（≤[`COMPLETION_TAIL_BYTES`] 字节，lossy utf8；
    /// 全文仍可 `background_output` 分页读）。
    pub tail: String,
}

/// `background_kill` 等待子进程退出的上限（超时如实报 still running）。
const KILL_WAIT: Duration = Duration::from_secs(5);

/// 停止阶梯的宽限窗：优雅停（POSIX=进程组 SIGTERM）后等自然退出的
/// 时长。必须小于 [`KILL_WAIT`]（5s）——阶梯 3s 宽限 + 强杀生效（~0.5s）
/// 要落在 kill() 的等待窗内，否则调用方会在升级到强杀之前误报 stubborn。
pub(crate) const STOP_GRACE: Duration = Duration::from_secs(3);

/// 单个后台任务的终态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobStatus {
    /// 进程退出码；信号杀死（Unix）时为 None。
    pub exit_code: Option<i32>,
    /// 退出码 0（或 Windows 等价成功态）。
    pub success: bool,
    /// 是否经 `background_kill` / 注册表兜底路径终止。
    pub killed: bool,
}

impl JobStatus {
    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "exit_code": self.exit_code,
            "success": self.success,
            "killed": self.killed,
        })
    }
}

/// 任务输出缓冲：环形语义（超限丢头部保尾部）+ 单调字节坐标。
///
/// `produced` 是进程累计产出的字节数（永不回退），`dropped` 是头部被丢弃
/// 的字节数。offset 分页坐标基于 `produced` 空间：读 offset 若小于 `dropped`
/// 则自动钳到 `dropped`（数据已丢，调用方从 `dropped_bytes` 得知）。
///
/// `pub(crate)`：delegate_registry（委派三动词）复用同一
/// 环形缓冲语义——delegate 条目是「带 stdin 写口的 条目 + 协议抽取」，
/// 缓冲语义必须逐字节一致（offset 坐标跨工具族可迁移）。
#[derive(Debug, Default)]
pub(crate) struct OutputBuffer {
    buf: Vec<u8>,
    produced: u64,
    dropped: u64,
    max: usize,
}

impl OutputBuffer {
    pub(crate) fn new(max: usize) -> Self {
        Self {
            max,
            ..Default::default()
        }
    }

    pub(crate) fn append(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        self.buf.extend_from_slice(data);
        self.produced += data.len() as u64;
        if self.buf.len() > self.max {
            let excess = self.buf.len() - self.max;
            self.buf.drain(..excess);
            self.dropped += excess as u64;
        }
    }

    /// 累计产出坐标（单调，offset 分页空间的「下一读位置」）。
    pub(crate) fn produced(&self) -> u64 {
        self.produced
    }

    /// 当前缓冲内还持有的字节（produced 空间的 `[dropped, produced)` 段）。
    /// delegate 侧做全缓冲协议抽取（assistant 文本）用。
    pub(crate) fn buf_bytes(&self) -> &[u8] {
        &self.buf
    }

    /// 从 `offset`（produced 空间）读至多 [`MAX_CHUNK_BYTES`] 字节。
    /// 返回 (chunk, 是否发生了头部钳制)。
    pub(crate) fn read_chunk(&self, offset: u64) -> (Vec<u8>, bool) {
        let clamped = offset.max(self.dropped);
        let skipped = offset < self.dropped;
        let start = (clamped - self.dropped).min(self.buf.len() as u64) as usize;
        let end = (start + MAX_CHUNK_BYTES).min(self.buf.len());
        (self.buf[start..end].to_vec(), skipped)
    }

    /// 分页 `next_offset` 单一真相源：钳到保留窗口起点后加本 chunk 实际
    /// 消费字节。`produced()` 不能当 next_offset——`read_chunk` 只返回至
    /// 多 [`MAX_CHUNK_BYTES`] 窗，坐标跳 produced 会让按 next_offset 分页
    /// 的调用方永久跳过中段字节（字节洞）。
    pub(crate) fn next_read_offset(&self, offset: u64, consumed: usize) -> u64 {
        offset.max(self.dropped) + consumed as u64
    }
}

/// chunk 尾部的 UTF-8 消费安全长度：[`MAX_CHUNK_BYTES`] 字节窗口可能把
/// 多字节字符劈在窗口边界，尾部不完整序列（`from_utf8` 报 `error_len ==
/// None`）此刻 lossy 转换会渲染成替换符，而它本可与**后续**输出拼成完整
/// 字符。返回「本 chunk 可安全消费的字节数」——截到 `valid_up_to`，剩余
/// 字节留给下一次 `read_chunk`（字节坐标单调，不丢不重）。
/// 中段真非法序列（`error_len == Some`，二进制/非 UTF-8 输出）不截：
/// lossy 诚实降级为替换符，且保证每轮必有推进。
pub(crate) fn utf8_safe_len(chunk: &[u8]) -> usize {
    match std::str::from_utf8(chunk) {
        Ok(_) => chunk.len(),
        // 尾部不完整序列（error_len() == None）：截到 valid_up_to（整段只有
        // 一个不完整序列时 = 0，空消费——等下一轮与后续字节拼齐）。
        Err(e) if e.error_len().is_none() => e.valid_up_to(),
        // 中段真非法序列（error_len() == Some）：不截——lossy 诚实降级为
        // 替换符，且保证每轮必有推进（二进制/非 UTF-8 输出不能卡住 tail）。
        _ => chunk.len(),
    }
}

/// 注册表中的单个任务条目。
struct JobEntry {
    cmd: String,
    cwd: String,
    started_at: std::time::SystemTime,
    /// stdout+stderr 合流输出（两个 pump task 追加）。
    output: Arc<Mutex<OutputBuffer>>,
    /// 终态；None = 还在跑。
    status: Arc<Mutex<Option<JobStatus>>>,
    /// kill 请求旗标（监督任务看到即树杀；注册表 Drop 同步置位）。
    /// pid 由监督任务闭包持有（Drop 路径无法异步树杀，用不上）。
    kill_flag: Arc<AtomicBool>,
    /// stdin 活管道：Some = steerable 启动且未关闭；None = 非可操控
    /// 启动（null stdin）或已关闭。写入队列 = 本 Mutex 的锁队列。
    stdin: Arc<Mutex<Option<tokio::process::ChildStdin>>>,
    /// 是否以 steerable=true 启动（区分「没开过 stdin」与「开过但已断」
    /// 两种 None 形态，steer 拒绝文案诚实分流）。
    steerable: bool,
}

impl JobEntry {
    async fn is_running(&self) -> bool {
        self.status.lock().await.is_none()
    }
}

/// 网关级后台进程注册表。
///
/// 单线程语义由调用方保证（工具调用天然串行穿 dispatch）；内部状态全部
/// `Arc<Mutex<..>>` 共享给监督/pump 任务，注册表本体只存句柄表。
pub struct BackgroundProcessRegistry {
    jobs: Arc<Mutex<HashMap<u64, JobEntry>>>,
    next_id: AtomicU64,
    max_jobs: usize,
    max_output: usize,
    /// 完成事件汇聚通道（2026-11-08）：gateway 装配期
    /// [`Self::set_completion_sink`] 一次；None = 无唤醒面（基线/测试
    /// 形态——`start_with_origin` 仍 spawn watcher，但 watcher 只在事件
    /// 通道在场时才发送）。`OnceLock`：进程级装配一次语义，set 幂等。
    completion_tx: Arc<std::sync::OnceLock<tokio::sync::mpsc::UnboundedSender<CompletionEvent>>>,
}

impl Default for BackgroundProcessRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for BackgroundProcessRegistry {
    fn drop(&mut self) {
        // 同步 best-effort：给所有存活任务置 kill 旗标，监督任务（持全部
        // Arc 克隆、生命周期独立于注册表）≤SUPERVISE_INTERVAL 内收尸。
        // runtime 关停路径由 Child 的 kill_on_drop(true) 兜底。
        if let Ok(jobs) = self.jobs.try_lock() {
            for job in jobs.values() {
                job.kill_flag.store(true, Ordering::Relaxed);
            }
            debug!(
                count = jobs.len(),
                "[BackgroundRegistry] dropped — kill flags raised for all jobs"
            );
        }
    }
}

impl BackgroundProcessRegistry {
    pub fn new() -> Self {
        Self::with_limits(DEFAULT_MAX_JOBS, DEFAULT_MAX_OUTPUT_BYTES)
    }

    /// 测试/定制形态：指定任务数与输出上限。
    pub fn with_limits(max_jobs: usize, max_output: usize) -> Self {
        Self {
            jobs: Arc::new(Mutex::new(HashMap::new())),
            next_id: AtomicU64::new(1),
            max_jobs,
            max_output,
            completion_tx: Arc::new(std::sync::OnceLock::new()),
        }
    }

    /// 装配完成事件汇聚通道（gateway 装配期调用一次；幂等，重复 set
    /// 以首次为准并如实返回）。接收端由调用方持 `UnboundedReceiver`
    /// spawn 常驻回灌任务。
    pub fn set_completion_sink(
        &self,
        tx: tokio::sync::mpsc::UnboundedSender<CompletionEvent>,
    ) -> bool {
        self.completion_tx.set(tx).is_ok()
    }

    /// 任务状态槽句柄（完成唤醒 watcher 消费；仅新路径使用）。
    fn job_status_handle(&self, id: u64) -> Option<Arc<Mutex<Option<JobStatus>>>> {
        self.jobs
            .try_lock()
            .ok()
            .and_then(|jobs| jobs.get(&id).map(|j| j.status.clone()))
    }

    /// 任务输出缓冲句柄（完成唤醒 watcher 取尾部用；仅新路径使用）。
    fn job_output_handle(&self, id: u64) -> Option<Arc<Mutex<OutputBuffer>>> {
        self.jobs
            .try_lock()
            .ok()
            .and_then(|jobs| jobs.get(&id).map(|j| j.output.clone()))
    }

    /// 当前任务数（含已完成——完成后条目保留至被淘汰/注册表销毁）。
    pub async fn job_count(&self) -> usize {
        self.jobs.lock().await.len()
    }

    /// 全量任务投影（WSAPI `background.list` 消费；按 id 倒序 = 新任务在前）。
    /// 只投影元数据与产出计数，不含输出内容（内容走 [`Self::output`] 按单调
    /// 坐标分页拉）。终态任务把 exit_code/success/killed 平铺进行（与
    /// [`Self::output`] 的终态平铺同构）。
    pub async fn list(&self) -> Vec<serde_json::Value> {
        let jobs = self.jobs.lock().await;
        let mut ids: Vec<u64> = jobs.keys().copied().collect();
        ids.sort_unstable_by(|a, b| b.cmp(a));
        let mut rows = Vec::with_capacity(ids.len());
        for id in ids {
            let job = &jobs[&id];
            let running = job.is_running().await;
            let buf = job.output.lock().await;
            let mut row = serde_json::json!({
                "id": id,
                "command": job.cmd,
                "cwd": job.cwd,
                "started_at_unix": job
                    .started_at
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
                "running": running,
                "steerable": job.steerable,
                "total_bytes": buf.produced,
                "dropped_bytes": buf.dropped,
            });
            let status = job.status.lock().await;
            if !running
                && let Some(s) = status.as_ref()
                && let Some(obj) = s.to_json().as_object()
            {
                for (k, v) in obj {
                    row[k.as_str()] = v.clone();
                }
            }
            rows.push(row);
        }
        rows
    }

    /// 起一个后台任务。`command` 经平台 shell 解释（cmd /C 或 sh -c）。
    /// `steerable=false` → stdin 恒 null（旧行为，读 stdin 的命令即刻 EOF）；
    /// `steerable=true` → stdin 活管道（[`Self::steer`] 可注入，任务不读
    /// stdin 也无害——管道缓冲写满前子进程照常跑）。
    pub async fn start(
        &self,
        command: &str,
        cwd: &str,
        steerable: bool,
    ) -> Result<serde_json::Value, String> {
        // 满员处理：先淘汰最旧的已完成任务；全在跑才拒绝。
        {
            let mut jobs = self.jobs.lock().await;
            if jobs.len() >= self.max_jobs {
                let oldest_finished = jobs
                    .iter()
                    .filter(|(_, j)| j.status.try_lock().map(|s| s.is_some()).unwrap_or(false))
                    .map(|(id, _)| *id)
                    .min();
                match oldest_finished {
                    Some(id) => {
                        jobs.remove(&id);
                        debug!(evicted = id, "[BackgroundRegistry] evicted finished job");
                    }
                    None => {
                        return Err(format!(
                            "background job limit reached ({} running); \
                             kill one with background_kill first",
                            self.max_jobs
                        ));
                    }
                }
            }
        }

        let mut child = build_platform_shell(command)
            .current_dir(cwd)
            .stdin(if steerable {
                // 活管道——保留写半端供 background_steer 注入。
                std::process::Stdio::piped()
            } else {
                std::process::Stdio::null()
            })
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            // 任务条目/监督任务都销毁时强杀子进程——孤儿进程兜底。
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("Failed to start background command: {}", e))?;
        let pid = child.id();

        let output = Arc::new(Mutex::new(OutputBuffer::new(self.max_output)));
        let status: Arc<Mutex<Option<JobStatus>>> = Arc::new(Mutex::new(None));
        let kill_flag = Arc::new(AtomicBool::new(false));
        let stdin = Arc::new(Mutex::new(if steerable {
            child.stdin.take()
        } else {
            None
        }));

        // 同 exec 的 管道必须由独立任务并发排干——否则长输出
        // 写满 ~64KB 管道缓冲会把子进程卡死在 write 上。stdout/stderr 合流
        // 追加进同一缓冲（保持产出顺序交错，与终端观感一致）。
        spawn_pump(child.stdout.take(), output.clone());
        spawn_pump(child.stderr.take(), output.clone());

        // 监督任务：轮询 try_wait（微秒级持锁）→ 记录终态；见 kill 旗标先
        // 树杀。不持锁跨 await，background_output 的读路径永不被阻塞。
        let child = Arc::new(Mutex::new(child));
        tokio::spawn(supervise(
            pid.expect("freshly spawned child always has a pid"),
            child,
            status.clone(),
            kill_flag.clone(),
        ));

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.jobs.lock().await.insert(
            id,
            JobEntry {
                cmd: command.to_string(),
                cwd: cwd.to_string(),
                started_at: std::time::SystemTime::now(),
                output,
                status,
                kill_flag,
                stdin,
                steerable,
            },
        );
        debug!(
            job = id,
            pid = pid.map(|p| p as u64),
            steerable,
            "[BackgroundRegistry] started"
        );

        let mut note = "poll background_output with job_id (+offset) to read output; \
                         background_kill to stop"
            .to_string();
        if steerable {
            note.push_str("; background_steer to inject input via stdin");
        }

        Ok(serde_json::json!({
            "job_id": id,
            "pid": pid,
            "command": command,
            "cwd": cwd,
            "running": true,
            "steerable": steerable,
            "note": note,
        }))
    }

    /// 分页读任务输出。`offset` 是 produced 字节空间坐标（上一轮返回的
    /// `next_offset`）。任务不存在（从未有/已被淘汰）→ Err。
    pub async fn output(&self, id: u64, offset: u64) -> Result<serde_json::Value, String> {
        let jobs = self.jobs.lock().await;
        let job = jobs.get(&id).ok_or_else(|| {
            format!(
                "background_output: no such job_id {} (never started, \
                                    already killed-and-evicted, or evicted to make room)",
                id
            )
        })?;
        let running = job.is_running().await;
        let buf = job.output.lock().await;
        let (mut chunk, skipped) = buf.read_chunk(offset);
        // BUG-20261108-027：运行中把尾部不完整 UTF-8 序列截到字符边界——
        // MAX_CHUNK_BYTES 字节窗口劈开多字节字符时，lossy 转换会把「本可
        // 与后续输出拼完整」的尾巴渲染成替换符（CJK 输出高频命中）。截掉
        // 的字节留在缓冲区由下一轮 offset 重读拼接（next_offset 只推进
        // 已消费字节，坐标单调不丢不重）。终态不截：没有后续输出可拼接，
        // 且「追平即停」的轮询语义要求终态推进到底（next_offset ==
        // total_bytes），未完成序列按既有语义诚实渲染替换符。
        if running {
            let safe = utf8_safe_len(&chunk);
            chunk.truncate(safe);
        }
        let mut payload = serde_json::json!({
            "job_id": id,
            "running": running,
            "command": job.cmd,
            "cwd": job.cwd,
            // steerable 随 output 面透出：UI 详情态据此决定是否渲染 stdin
            // 注入框（与 start()/list() 同源，不另设查询命令）。
            "steerable": job.steerable,
            "started_at_unix": job
                .started_at
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            "total_bytes": buf.produced,
            "dropped_bytes": buf.dropped,
            "offset": offset,
            "next_offset": buf.next_read_offset(offset, chunk.len()),
            "chunk": String::from_utf8_lossy(&chunk),
        });
        if skipped {
            payload["note"] = serde_json::json!(
                "requested offset precedes the retained window (head dropped by the \
                 output cap); chunk starts at the oldest retained byte"
            );
        }
        // 终态平铺进响应（exit_code/success/killed 与 kill() 返回同构）。
        let flat = if !running {
            let status = job.status.lock().await;
            status.as_ref().map(|s| s.to_json())
        } else {
            None
        };
        if let Some(obj) = flat.as_ref().and_then(|j| j.as_object()) {
            for (k, v) in obj {
                payload[k.as_str()] = v.clone();
            }
        }
        Ok(payload)
    }

    /// 起一个带完成唤醒的后台任务（2026-11-08 子进程实时输出与完成唤醒）。
    ///
    /// 与 [`Self::start`] 同一装配路径（`start()` 原样保留、不签名变更——
    /// 基线注册与既有测试继续走它）；origin 在场时：
    /// 1. 覆盖返回 payload 的 `note`（旧 note 指引轮询，会重新诱发 sleep
    ///    -poll 反模式；origin 路径的指引是「完成后自动通知，不要轮询」，
    ///    openclaw notifyOnExit 同款话术）；
    /// 2. spawn 完成唤醒 watcher：轮询任务终态（与 supervise 同拍 200ms，
    ///    无 Notify 注册窗口竞态面），退出时取输出尾部（≤
    ///    [`COMPLETION_TAIL_BYTES`] 字节）发 [`CompletionEvent`] 进
    ///    completion sink——gateway 侧把它合成为 InboundMessage 经 bus
    ///    走标准入站路径回灌发起会话（AgentLoop 零改动）。
    ///
    /// `origin = None` → 行为与 `start()` 完全一致（无 note 覆盖、无
    /// watcher），调用方语义不变。
    ///
    /// `notify = false`（2026-11-09 完成唤醒可选化）：查看型 GUI（notepad
    /// 等**用户自开自关**的进程）退出对 agent 无信息利益——唤醒一次 LLM
    /// 报告「你关的东西关了」是纯噪声。受管但静默：不 spawn 完成唤醒
    /// watcher（完成唤醒的唯一机制面就是 watcher，不 spawn = 零唤醒），
    /// job 状态由 supervise 照常落账，`background_output`/`background_kill`
    /// 管理能力完整保留；note/payload 显式标注静默语义。仅 origin 在场时
    /// 生效（origin 缺席的基线路径本就无唤醒面，notify 被忽略、字节不变）。
    /// 缺省 true = 现行为（开发服务器/长构建退出 = 崩溃信号，必须唤醒）。
    pub async fn start_with_origin(
        &self,
        command: &str,
        cwd: &str,
        steerable: bool,
        origin: Option<JobOrigin>,
        notify: bool,
    ) -> Result<serde_json::Value, String> {
        let mut payload = self.start(command, cwd, steerable).await?;
        let Some(origin) = origin else {
            return Ok(payload);
        };
        if !notify {
            payload["notify"] = serde_json::json!(false);
            payload["note"] = serde_json::json!(
                "job started with notify=false — no completion notice will be \
                 delivered when it exits (the user manages its lifecycle); use \
                 background_output to read output or check state, background_kill \
                 to stop"
            );
            return Ok(payload);
        }
        payload["note"] = serde_json::json!(
            "job started; when it exits a completion notice (with the recent output) \
             is delivered back into this conversation automatically — do NOT \
             sleep-poll background_output waiting for it to finish; read output \
             with background_output only when you need it"
        );
        let job_id = payload.get("job_id").and_then(|v| v.as_u64());
        if let Some(id) = job_id {
            match (self.job_status_handle(id), self.job_output_handle(id)) {
                (Some(status), Some(output)) => {
                    tokio::spawn(completion_watcher(
                        id,
                        status,
                        output,
                        origin,
                        Arc::clone(&self.completion_tx),
                    ));
                }
                _ => {
                    // 条目句柄取不到（理论竞态：jobs 锁正被别的调用方长持）
                    // ——诚实降级：无 watcher = 无唤醒，不静默装没事。
                    debug!(
                        job = id,
                        "[BackgroundRegistry] completion watcher not spawned (entry handles unavailable)"
                    );
                }
            }
        }
        Ok(payload)
    }

    /// 分页读任务输出，可选阻塞等待任务退出（2026-11-08）。`wait_secs = 0`
    /// → 行为与 [`Self::output`] 逐字节一致（即取即返）；>0 → 每
    /// [`SUPERVISE_INTERVAL`] 复查一次任务状态，等**退出**（不是等新输出
    /// ——退出才是模型要的状态翻转信号，见 PLAN 决策记录）或超窗，返回
    /// 停止等待时刻的 output payload（含期间到达的新字节与终态平铺）。
    /// 上限 [`MAX_OUTPUT_WAIT_SECS`]（30s）——工具调用不该长阻塞 dispatch。
    pub async fn output_with_wait(
        &self,
        id: u64,
        offset: u64,
        wait_secs: u64,
    ) -> Result<serde_json::Value, String> {
        let clamped = wait_secs.min(MAX_OUTPUT_WAIT_SECS);
        if clamped == 0 {
            return self.output(id, offset).await;
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(clamped);
        loop {
            let payload = self.output(id, offset).await?;
            let running = payload
                .get("running")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if !running || tokio::time::Instant::now() >= deadline {
                return Ok(payload);
            }
            sleep(SUPERVISE_INTERVAL).await;
        }
    }

    /// 终止任务：置旗标（监督任务 ≤[`SUPERVISE_INTERVAL`] 内树杀），随后
    /// 等终态（≤[`KILL_WAIT`]）。已完成的任务照常返回其终态（幂等，不报
    /// 错）。
    pub async fn kill(&self, id: u64) -> Result<serde_json::Value, String> {
        let kill_flag = {
            let jobs = self.jobs.lock().await;
            let job = jobs
                .get(&id)
                .ok_or_else(|| format!("background_kill: no such job_id {}", id))?;
            job.kill_flag.clone()
        };
        kill_flag.store(true, Ordering::Relaxed);

        // 等监督任务记录终态（它持有 Child 锁做 start_kill；这里只等
        // status 槽，无锁竞争）。已在 kill 前 natural-exit 的也会落到这里。
        let deadline = tokio::time::Instant::now() + KILL_WAIT;
        loop {
            {
                let jobs = self.jobs.lock().await;
                if let Some(job) = jobs.get(&id) {
                    let status = job.status.lock().await;
                    if let Some(s) = status.as_ref() {
                        let mut payload = s.to_json();
                        payload["job_id"] = serde_json::json!(id);
                        return Ok(payload);
                    }
                } else {
                    return Err(format!(
                        "background_kill: job {} vanished while killing (registry evicted it)",
                        id
                    ));
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(format!(
                    "background_kill: kill signal sent to job {} but it has not exited \
                     within {}s (stubborn process; it WILL be reaped by the supervisor)",
                    id,
                    KILL_WAIT.as_secs()
                ));
            }
            sleep(Duration::from_millis(100)).await;
        }
    }

    /// steer：向运行中任务的 stdin 写入 `text`（字节级
    /// ——registry 层不做协议帧封装；text 后补换行 + flush，行协议 CLI 直读）。
    /// 并发 steer 经 stdin Mutex 的锁队列串行落盘，不交错撕裂。
    ///
    /// 诚实拒绝面：任务不存在 / 已结束 / 非 steerable 启动（stdin 从未开）
    /// / stdin 已断（进程死了写半端随之收走）——各自独立文案。
    pub async fn steer(&self, id: u64, text: &str) -> Result<serde_json::Value, String> {
        let (stdin, steerable, running) = {
            let jobs = self.jobs.lock().await;
            let job = jobs
                .get(&id)
                .ok_or_else(|| format!("background_steer: no such job_id {}", id))?;
            let running = job.is_running().await;
            (Arc::clone(&job.stdin), job.steerable, running)
        };
        if !running {
            return Err(format!("background_steer: job {} is not running", id));
        }
        if !steerable {
            return Err(format!(
                "background_steer: job {} was started without steerable=true \
                 (stdin is closed; restart with steerable=true to steer)",
                id
            ));
        }
        {
            use tokio::io::AsyncWriteExt;
            let mut guard = stdin.lock().await;
            let Some(s) = guard.as_mut() else {
                return Err(format!(
                    "background_steer: job {} stdin is closed (process exited?)",
                    id
                ));
            };
            s.write_all(text.as_bytes())
                .await
                .map_err(|e| format!("failed to write to job {} stdin: {}", id, e))?;
            s.write_all(b"\n")
                .await
                .map_err(|e| format!("failed to write to job {} stdin: {}", id, e))?;
            s.flush()
                .await
                .map_err(|e| format!("failed to flush job {} stdin: {}", id, e))?;
        }
        Ok(serde_json::json!({
            "job_id": id,
            "running": true,
            "note": "text delivered to the job's stdin (read its reaction via background_output)",
        }))
    }
}

/// 监督任务：轮询 try_wait → 记录终态；见 kill 旗标跑一次停止阶梯
/// （优雅停→宽限窗→强杀升级），再以 start_kill 兜底直杀 shell
/// （阶梯指令丢失时至少不漏直接子进程）。
///
/// `pub(crate)`：delegate_registry 复用同一监督语义（kill
/// 旗标 + 自然退出收尸）；delegate 侧另有独立的墙钟上限 watch 任务只置
/// 旗标，不重复实现树杀循环。
pub(crate) async fn supervise(
    pid: u32,
    child: Arc<Mutex<tokio::process::Child>>,
    status: Arc<Mutex<Option<JobStatus>>>,
    kill_flag: Arc<AtomicBool>,
) {
    let mut kill_sent = false;
    loop {
        if kill_flag.load(Ordering::Relaxed) {
            if !kill_sent {
                kill_sent = true;
                // 停止阶梯：优雅停 → 宽限窗 → 强杀升级（内含等待，
                // 只跑一次；此后每轮仅剩收尸轮询 + start_kill 兜底）。
                stop_ladder(pid, &child, STOP_GRACE).await;
            }
            // 兜底直杀：阶梯已生效时这里是已退出进程上的 Err——忽略。
            let _ = child.lock().await.start_kill();
        }
        let exited = {
            let mut c = child.lock().await;
            c.try_wait().ok().flatten()
        };
        if let Some(st) = exited {
            *status.lock().await = Some(JobStatus {
                exit_code: st.code(),
                success: st.success(),
                killed: kill_flag.load(Ordering::Relaxed),
            });
            return;
        }
        sleep(SUPERVISE_INTERVAL).await;
    }
}

/// 完成唤醒 watcher（2026-11-08）：轮询任务终态（与 supervise 同拍；
/// 状态槽写入者 = supervise，读侧 clone 快照不跨锁 await）→ 退出时取
/// 输出尾部发 [`CompletionEvent`]。completion sink 未装配（基线/测试）
/// → 只空等终态后返回，零副作用。
async fn completion_watcher(
    job_id: u64,
    status: Arc<Mutex<Option<JobStatus>>>,
    output: Arc<Mutex<OutputBuffer>>,
    origin: JobOrigin,
    completion_tx: Arc<std::sync::OnceLock<tokio::sync::mpsc::UnboundedSender<CompletionEvent>>>,
) {
    let final_status = loop {
        let snap = status.lock().await.clone();
        if let Some(s) = snap {
            break s;
        }
        sleep(SUPERVISE_INTERVAL).await;
    };
    let tail = {
        let buf = output.lock().await;
        let start = buf.produced().saturating_sub(COMPLETION_TAIL_BYTES as u64);
        let (chunk, _skipped) = buf.read_chunk(start);
        String::from_utf8_lossy(&chunk).into_owned()
    };
    if let Some(tx) = completion_tx.get() {
        let _ = tx.send(CompletionEvent {
            job_id,
            status: final_status,
            origin,
            tail,
        });
    }
}

/// 排干管道追加进共享输出缓冲（stdout/stderr 各一个任务）。
///
/// `pub(crate)`：delegate_registry 复用（delegate 子进程同样
/// 必须并发排干，否则 64KB 管道缓冲写满卡死子进程—— 教训同源）。
pub(crate) fn spawn_pump(
    pipe: Option<impl tokio::io::AsyncRead + Unpin + Send + 'static>,
    output: Arc<Mutex<OutputBuffer>>,
) {
    tokio::spawn(async move {
        let mut pipe = match pipe {
            Some(p) => p,
            None => return,
        };
        let mut chunk = [0u8; 8192];
        loop {
            match tokio::io::AsyncReadExt::read(&mut pipe, &mut chunk).await {
                Ok(0) | Err(_) => return,
                Ok(n) => output.lock().await.append(&chunk[..n]),
            }
        }
    });
}

/// 平台 shell 命令构造——与 `ExecTool` / `AsyncExecTool` 同语义：
/// Windows 用 `cmd /C` + `raw_arg`（`.arg()` 的自动加引号会打碎 cmd.exe
/// 自己的引号处理），Unix 用 `sh -c`。stdin 由调用方决定。
///
/// 额外两条（树杀前提）：
/// - Unix `process_group(0)`：子进程自成一进程组（pgid == pid），树杀用
///   `kill -- -PGID` 一锅端（shell 的孙进程不会孤儿化）。
/// - Windows `CREATE_NO_WINDOW`：后台任务不该弹控制台窗口（网关以托盘/
///   服务形态运行时尤其如此；cc_hooks 同款旗标）。
pub(crate) fn build_platform_shell(command: &str) -> tokio::process::Command {
    #[cfg(target_os = "windows")]
    let mut c = {
        let mut c = tokio::process::Command::new("cmd");
        c.raw_arg(format!("/C {}", command));
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        c
    };
    #[cfg(not(target_os = "windows"))]
    let mut c = {
        let mut c = tokio::process::Command::new("sh");
        c.arg("-c").arg(command);
        // tokio Command 自带 process_group（镜像 std API），无需 CommandExt。
        c.process_group(0);
        c
    };
    // 环境凭据剥离：本函数自注释「与 ExecTool /
    // AsyncExecTool 同语义」——语义含 剥离，此前漏接（后台命令与前台
    // exec 同为 LLM 驱动 spawn，echo 环境即可读凭据）。
    let stripped = nemesis_utils::env_sanitize::sanitize_tokio_command(&mut c);
    if stripped > 0 {
        debug!("[Background] env sanitized: {stripped} vars stripped");
    }
    c
}

/// 停止阶梯：优雅停。
///
/// - POSIX：向进程组发 SIGTERM（shell 捕获后优雅收尾、子树随之退出；
///   默认处理同样直接终止）。
/// - Windows：控制台子进程没有真优雅终止通道（`taskkill` 无 `/F` 靠
///   窗口消息，控制台程序基本无效）——即树杀强杀，宽限窗实际
///   演变为「强杀后的收尸等待」（诚实边界，不虚报优雅语义）。
///
/// `pub(crate)`：delegate_registry 经 supervise 共用同一阶梯。
pub(crate) async fn graceful_stop(pid: u32) {
    #[cfg(not(target_os = "windows"))]
    {
        let mut c = tokio::process::Command::new("kill");
        c.args(["-TERM", "--", &format!("-{}", pid)]);
        // 2s 上限：指令本身不该卡住阶梯（与 tree_kill 同纪律）。
        let _ = tokio::time::timeout(Duration::from_secs(2), c.output()).await;
    }
    #[cfg(target_os = "windows")]
    tree_kill(pid).await;
}

/// 停止阶梯：强杀升级（优雅段超窗后的兜底）。
///
/// - POSIX：向进程组发 SIGKILL——此前缺失（[`tree_kill`] 对 POSIX 只发
///   TERM，trap/僵死进程永不升级，`start_kill` 只杀 shell 本体会漏孙
///   进程）。
/// - Windows：复用树杀（幂等；已是强杀，走到这里说明进程连 `/F`
///   都还没死透——多补一轮，`stop_ladder` 末尾另有 `start_kill` 兜底）。
pub(crate) async fn force_kill(pid: u32) {
    #[cfg(not(target_os = "windows"))]
    {
        let mut c = tokio::process::Command::new("kill");
        c.args(["-KILL", "--", &format!("-{}", pid)]);
        let _ = tokio::time::timeout(Duration::from_secs(2), c.output()).await;
    }
    #[cfg(target_os = "windows")]
    tree_kill(pid).await;
}

/// 子进程停止阶梯（**长命进程路径专用**：background 任务与 delegate
/// 经 [`supervise`] 共用；exec 等 per-call 短命子进程保持 [`tree_kill`]
/// 直杀语义，避免过度工程）：优雅停 → 宽限窗内轮询等自然退出 → 超窗
/// 强杀升级 → `start_kill` 兜底直杀 shell 本体。
///
/// 调用方随后照常 try_wait 收尸记录终态（真相源不变）。指令丢失一律
/// 忽略错误——收尸才为权威依据（与 tree_kill 同纪律）；PID 复用窗口同
/// tree_kill（毫秒级固有竞态，不做缓解）。
pub(crate) async fn stop_ladder(
    pid: u32,
    child: &Arc<Mutex<tokio::process::Child>>,
    grace: Duration,
) {
    graceful_stop(pid).await;
    let deadline = tokio::time::Instant::now() + grace;
    loop {
        let exited = {
            let mut c = child.lock().await;
            c.try_wait().ok().flatten()
        };
        if exited.is_some() {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        sleep(Duration::from_millis(100)).await;
    }
    force_kill(pid).await;
    // 进程组指令丢失时至少不漏直接子进程（已退出进程上调用无害）。
    let _ = child.lock().await.start_kill();
}

/// 树杀：干掉 pid 及其整棵子树。
///
/// - Windows：`taskkill /T /F /PID`（OS 级树遍历）。
/// - Unix：`kill -TERM -- -PGID`（进程组杀；pgid == pid，见
///   `build_platform_shell` 的 process_group(0)）。
///
/// 指令丢失（进程已退/PID 复用/kill 不存在）一律忽略错误——调用方随后
/// 的 try_wait 收尸才为权威依据。PID 复用窗口（进程退出→树杀之间）是平台
/// 固有竞态，量级为毫秒且后果止于误杀同名树，不做缓解。
///
/// `pub(crate)`：exec/run_checks/run_script 的超时收尸共用
/// 同一树杀（此前 start_kill 只杀 shell 本体，cargo/rustc 等整棵子树存活
/// ——产品验证实验 #1 实证：run_checks 声明 600s 超时后 cargo 树继续跑了
/// 25 分钟直到手工杀树）。Unix 侧前提是 spawn 时 `process_group(0)`
/// （`make_piped_shell_command` / run_script 同款）。
pub(crate) async fn tree_kill(pid: u32) {
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = tokio::process::Command::new("taskkill");
        c.args(["/T", "/F", "/PID", &pid.to_string()]);
        c
    };
    #[cfg(not(target_os = "windows"))]
    let mut cmd = {
        let mut c = tokio::process::Command::new("kill");
        c.args(["-TERM", "--", &format!("-{}", pid)]);
        c
    };
    // 2s 上限：树杀指令本身不该卡住监督循环；超时按失败处理（监督循环
    // 里 kill 旗标只发一次，兜底还有 start_kill 直杀 shell）。
    let _ = tokio::time::timeout(Duration::from_secs(2), cmd.output()).await;
}

// ---------------------------------------------------------------------------
// 四动词工具（start / output / kill / steer）
// ---------------------------------------------------------------------------

/// cwd 解析 + 工作区边界（与 `ExecTool` 同形态：相对 cwd join workspace，
/// 双侧 canonicalize_for_compare 归一化防 8.3 短名/大小写失配）。
/// 返回**解析后的绝对路径**——`Command::current_dir` 对相对路径按父进程
/// cwd 解释，与工作区基准脱钩（exec 侧传原始串是历史行为，这里不复制）。
///
/// `pub(crate)`：delegate_registry 的 delegate_start 复用同一
/// 语义（子代理工作目录与后台任务同规约束）。
pub(crate) fn resolve_cwd(
    workspace: &str,
    restrict: bool,
    cwd: Option<&str>,
) -> Result<String, String> {
    let raw = cwd.unwrap_or(workspace);
    let target = if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        Path::new(workspace).join(raw)
    };
    if restrict {
        let resolved = canonicalize_for_compare(&target);
        let ws = canonicalize_for_compare(Path::new(workspace));
        if !resolved.starts_with(&ws) {
            return Err(format!(
                "Access denied: path '{}' is outside workspace",
                raw
            ));
        }
    }
    Ok(target.to_string_lossy().to_string())
}

/// `background_start`：起长跑进程，立刻返回 job_id。
pub struct BackgroundStartTool {
    workspace: String,
    restrict: bool,
    registry: Arc<BackgroundProcessRegistry>,
}

impl BackgroundStartTool {
    pub fn new(workspace: &str, restrict: bool, registry: Arc<BackgroundProcessRegistry>) -> Self {
        Self {
            workspace: workspace.to_string(),
            restrict,
            registry,
        }
    }
}

#[async_trait]
impl Tool for BackgroundStartTool {
    fn description(&self) -> String {
        "Start a long-running shell command in the background and return immediately \
         with a job_id. Use for dev servers, watchers, builds, GUI apps (notepad, \
         calc), or any command that outlives a normal exec timeout. When the job \
         exits, a completion notice (with the recent output) is automatically \
         delivered back into this conversation — do NOT sleep-poll background_output \
         waiting for it to finish; read output incrementally with background_output \
         only when you need it, and stop with background_kill. Pass steerable=true \
         to keep a live stdin pipe and inject input later with background_steer. \
         Pass notify=false for user-facing GUI apps the user opens and closes \
         themselves (notepad, calc): no completion notice is delivered when they \
         exit (the job stays manageable via background_output/background_kill)."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "description": "Shell command to run in the background"},
                "cwd": {"type": "string", "description": "Working directory (default: workspace root; confined to workspace)"},
                "steerable": {"type": "boolean", "description": "Keep a live stdin pipe so the job can receive input via background_steer (default false)"},
                "notify": {"type": "boolean", "description": "Deliver a completion notice into this conversation when the job exits (default true). Set false for user-facing GUI apps the user opens and closes themselves (notepad, calc) — their exit carries no information for you and notifying the chat is noise"}
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String> {
        let val: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid arguments: {}", e))?;
        let command = val
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or("Missing 'command' argument")?;
        let cwd = resolve_cwd(
            &self.workspace,
            self.restrict,
            val.get("cwd").and_then(|v| v.as_str()),
        )?;
        let steerable = val
            .get("steerable")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        // notify 缺省 true（开发服务器/长构建退出 = 崩溃信号）；仅显式
        // boolean false 关闭。非法值（如字符串 "false"）→ 缺省 true，
        // 不静默纠偏——schema 声明 boolean，模型发非布尔即按缺省走。
        let notify = val.get("notify").and_then(|v| v.as_bool()).unwrap_or(true);
        // [SEALED 2026-11-08] 旧模式（轮询形态）：不带 origin 起任务，返回
        // payload 的 note 指引「poll background_output」——诱发模型
        // background_output→sleep→再读 的空转循环（2026-11-08 用户实测
        // notepad 场景）。保留备查；恢复 = 把下面 start_with_origin 调用
        // 换回这行并删掉 origin 捕获段。
        // let payload = self.registry.start(command, &cwd, steerable).await?;
        // ── 新模式（完成自动唤醒）：捕获发起会话锚，任务退出后 gateway 经
        // bus 走标准入站路径回灌完成通告（模型零轮询）。channel/chat_id
        // 缺席（无会话上下文的调用形态）→ None = 旧模式行为。
        let origin = if context.channel.is_empty() || context.chat_id.is_empty() {
            None
        } else {
            Some(JobOrigin {
                channel: context.channel.clone(),
                chat_id: context.chat_id.clone(),
                session_key: context.session_key.clone(),
            })
        };
        let payload = self
            .registry
            .start_with_origin(command, &cwd, steerable, origin, notify)
            .await?;
        Ok(payload.to_string())
    }
}

/// `background_output`：分页读后台任务输出（≤8KB/次，offset 坐标）。
pub struct BackgroundOutputTool {
    registry: Arc<BackgroundProcessRegistry>,
}

impl BackgroundOutputTool {
    pub fn new(registry: Arc<BackgroundProcessRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl Tool for BackgroundOutputTool {
    fn description(&self) -> String {
        "Read accumulated output from a background job started with background_start. \
         Pass the job_id; on repeated calls pass the previous response's next_offset \
         to get only new bytes. Response includes running/done status and exit code \
         when the job has finished. Note: when a job exits you are notified \
         automatically in this conversation — use wait_secs only if you deliberately \
         want to block waiting for exit (capped at 30s); do not loop this call with \
         sleeps."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "job_id": {"type": "integer", "description": "Job id returned by background_start"},
                "offset": {"type": "integer", "description": "Byte offset to read from (default 0; use next_offset from the previous call)"},
                "wait_secs": {"type": "integer", "description": "Optional: block up to N seconds (max 30) waiting for the job to exit before returning (default 0 = return immediately)"}
            },
            "required": ["job_id"]
        })
    }

    /// 纯读自有缓冲（并行读池安全）；wait_secs>0 的等待也只轮询自有
    /// 状态槽，不产生新的攻击面。
    fn is_read_only(&self) -> bool {
        true
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let val: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid arguments: {}", e))?;
        let job_id = val
            .get("job_id")
            .and_then(|v| v.as_u64())
            .ok_or("Missing 'job_id' argument")?;
        let offset = val.get("offset").and_then(|v| v.as_u64()).unwrap_or(0);
        // [SEALED 2026-11-08] 旧模式：即取即返，等待语义缺失——模型想等
        // 任务结束只能本工具与 sleep 交替空转（2026-11-08 用户实测）。保留
        // 备查；恢复 = 把下面 output_with_wait 调用换回这行并删掉
        // wait_secs 解析段（parameters 里的 wait_secs 属性一并还原）。
        // let payload = self.registry.output(job_id, offset).await?;
        // ── 新模式：wait_secs>0 阻塞等退出（≤30s 钳制），0 = 旧行为。
        let wait_secs = val
            .get("wait_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            .min(crate::background_registry::MAX_OUTPUT_WAIT_SECS);
        let payload = self
            .registry
            .output_with_wait(job_id, offset, wait_secs)
            .await?;
        Ok(payload.to_string())
    }
}

/// `background_kill`：终止后台任务并等待退出。
pub struct BackgroundKillTool {
    registry: Arc<BackgroundProcessRegistry>,
}

impl BackgroundKillTool {
    pub fn new(registry: Arc<BackgroundProcessRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl Tool for BackgroundKillTool {
    fn description(&self) -> String {
        "Stop a background job started with background_start. Sends a kill signal and \
         waits up to 5 seconds for the process to exit; returns the final status. \
         Killing an already-finished job reports its exit status (idempotent)."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "job_id": {"type": "integer", "description": "Job id returned by background_start"}
            },
            "required": ["job_id"]
        })
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let val: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid arguments: {}", e))?;
        let job_id = val
            .get("job_id")
            .and_then(|v| v.as_u64())
            .ok_or("Missing 'job_id' argument")?;
        let payload = self.registry.kill(job_id).await?;
        Ok(payload.to_string())
    }
}

/// `background_steer`：向运行中后台任务的 stdin 注入文本。
///
/// 安全档 = ProcessExec（exec 类审批口径对齐——注入指令 = 操控进程，风险
/// 同级，见 nemesis-security types.rs 注记）；steer 文本作为工具入参
/// 照常过注入检测层（dispatch 瀑布自动覆盖，工具内不重复扫描）。
pub struct BackgroundSteerTool {
    registry: Arc<BackgroundProcessRegistry>,
}

impl BackgroundSteerTool {
    pub fn new(registry: Arc<BackgroundProcessRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl Tool for BackgroundSteerTool {
    fn description(&self) -> String {
        "Inject a line of input into a running background job's stdin (steer). The \
         job must have been started with background_start using steerable=true. \
         Use this to drive interactive CLI processes mid-run (answer prompts, send \
         new commands) without restarting them; read the reaction via \
         background_output."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "job_id": {"type": "integer", "description": "Job id returned by background_start"},
                "text": {"type": "string", "description": "Input line to write to the job's stdin (a newline is appended)"}
            },
            "required": ["job_id", "text"]
        })
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let val: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid arguments: {}", e))?;
        let job_id = val
            .get("job_id")
            .and_then(|v| v.as_u64())
            .ok_or("Missing 'job_id' argument")?;
        let text = val
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or("Missing 'text' argument")?;
        if text.trim().is_empty() {
            return Err("text is empty".to_string());
        }
        let payload = self.registry.steer(job_id, text).await?;
        Ok(payload.to_string())
    }
}

#[cfg(test)]
mod tests;

// 覆盖率补充批次：OutputBuffer 钳制 / Drop·Default / 头部丢弃注记 /
// 工具参数校验错误面 / kill 幂等真停。
#[cfg(test)]
mod cov_tests;
