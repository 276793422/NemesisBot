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
//! gateway 装配形态（`log_dir` 注入）另有**落盘 tee**（2026-11-22）：每任务
//! 输出双写 append-only 镜像文件（完整历史），内存窗口 + 盘上全量分工——
//! 环绕后的早期输出从文件兜（read_file/grep 可达），见 [`TeeFile`]。

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
use crate::loop_tools::{env_slot_schema, parse_env_slot};

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

/// `background_output wait_secs` 的钳制上限（秒）——工具调用不该长阻塞
/// dispatch（要任务终态就走本参数显式等待，不做 sleep 长轮询）。
pub const MAX_OUTPUT_WAIT_SECS: u64 = 30;

/// 发起会话锚（2026-11-17 会话资源盘点）：`background_start` 时从
/// [`RequestContext`] 捕获的三元组快照，随任务条目落库——
/// [`BackgroundProcessRegistry::list_by_session_snapshot`] 按会话盘点
/// （资源看板 background 类目）的数据面。完成唤醒已于 2026-11-22 摘除
/// （完成推送定性为流程 bug，产品层不该存在；零通知阵营——与 codex /
/// gemini 缺省 / goose 同侧），本结构只服务盘点面。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobOrigin {
    /// 发起通道（"web" 等——出站回复经同通道回投）。
    pub channel: String,
    /// 发起会话的 chat id。
    pub chat_id: String,
    /// 发起会话的 agent session key（round-trip 保会话归属）。
    pub session_key: String,
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

/// 落盘 tee 句柄（2026-11-22 输出落盘）：每任务一个 append-only 镜像文件
/// `{log_dir}/bg_{millis}_{job_id}.log`（job_id 是进程内 AtomicU64 从 1 起
/// **跨重启必撞名**——毫秒前缀防碰撞，对齐 capture_sink `logs/capture/{ts}_`
/// 先例）。
///
/// 语义纪律：
/// - **原始字节直写**（不过 secrets 掩码）：与老 exec 文件捕获 / terminal
///   审计日志（含回显密码 + 文件头注记）同级既有实践；文件头带敏感注记；
///   `read_file` 读出参照常过凭据扫描层（安全 8 层第④层）；
/// - **文件不设帽**：完整历史镜像（内存 256KB 环帽语义不变——「内存窗口 +
///   盘上全量」分工）；失控进程磁盘风险与老 exec 文件捕获形态同级；
/// - **best-effort 写穿**：IO 失败（盘满等）不阻断内存缓冲与 pump——
///   只警告一次，后续静默（失败进程不该淹日志）；
/// - **不删文件**：条目移除 / kill / 注册表 Drop 均不删——盘上日志是档案
///   （gateway 重启杀全部 job 时文件保留已产出内容，本机制的意义所在）。
pub(crate) struct TeeFile {
    file: std::sync::Mutex<std::fs::File>,
    path: PathBuf,
    warn_once: AtomicBool,
}

impl TeeFile {
    /// 创建镜像文件并写文件头（目录不存在则建）。`job_id` 进文件名，
    /// `cmd`/`cwd` 进头注记（档案自描述）。
    fn create(dir: &Path, job_id: u64, cmd: &str, cwd: &str) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let path = dir.join(format!("bg_{}_{}.log", millis, job_id));
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        use std::io::Write as _;
        // 文件头注记：原始字节直写可能含敏感信息（terminal 审计同款诚实注记）。
        let _ = writeln!(file, "# NemesisBot background job log (raw output mirror)");
        let _ = writeln!(file, "# job_id={} started_at_unix_ms={}", job_id, millis);
        let _ = writeln!(file, "# command: {}", cmd);
        let _ = writeln!(file, "# cwd: {}", cwd);
        let _ = writeln!(
            file,
            "# 注意：本文件是进程原始输出的逐字节镜像（append-only），可能包含敏感信息（凭据/密钥回显）。"
        );
        let _ = writeln!(file);
        Ok(Self {
            file: std::sync::Mutex::new(file),
            path,
            warn_once: AtomicBool::new(false),
        })
    }

    /// 镜像文件路径。
    fn path(&self) -> &Path {
        &self.path
    }

    /// 追加原始字节（best-effort 写穿：IO 失败警告一次后静默）。
    fn append(&self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        use std::io::Write as _;
        if let Ok(mut f) = self.file.lock()
            && f.write_all(data).and_then(|_| f.flush()).is_err()
            && !self.warn_once.swap(true, Ordering::Relaxed)
        {
            debug!(
                path = %self.path.display(),
                "[BackgroundRegistry] tee write failed (disk full? io error) — \
                 further tee errors silent for this job"
            );
        }
    }
}

/// 注册表中的单个任务条目。
struct JobEntry {
    cmd: String,
    cwd: String,
    started_at: std::time::SystemTime,
    /// stdout+stderr 合流输出（两个 pump task 追加）。
    output: Arc<Mutex<OutputBuffer>>,
    /// tee 文件路径（tee 关 = None）——`background_output` / start 响应
    /// `log_path` 字段来源（模型可发现的「最差读文件」路径）。tee 句柄
    /// 本体不存条目：两个 pump task 持 Arc 克隆写文件，管道 EOF 后随任务
    /// 退场关闭（条目只存路径供响应透出）。
    log_path: Option<PathBuf>,
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
    /// 本 job 掩码池（vault 计划 P2++，2026-11-11）：background_start 调用
    /// 的 dispatch 注入池（`ctx.secrets`）原样捕获——env 槽注入的凭据真值
    /// （含会话占位符展开值）若被任务输出回显，`output()` 与完成通告两个
    /// 读出面据此就地遮蔽（明文不进 session_log / LLM 上下文）。纯内存态：
    /// JobEntry 不序列化，明文不出进程；空池 = 零成本直通。
    secrets: Arc<Vec<String>>,
    /// 发起会话锚快照（2026-11-17 会话资源盘点）：watcher 闭包持原件之外
    /// 在条目上留一份只读投影，供 `list_by_session` 按会话盘点（资源看板
    /// background 类目）。None = 基线 start() 路径（无会话归属——不进任何
    /// 会话的盘点，delegate 等系统面任务不出现在看板）。
    origin: Option<JobOrigin>,
}

impl JobEntry {
    async fn is_running(&self) -> bool {
        self.status.lock().await.is_none()
    }
}

/// 任务行投影的单一真相源（`list()` 与 `list_by_session_snapshot()` 共用
/// ——两读路径字段漂移 = WSAPI 与看板行不一致，抽此处钉死）。`None` 字节
/// 计数（快照遇输出锁忙）序列化为 `null`；`status_json`（终态投影）的
/// 键值平铺进行——exit_code/success/killed 与 [`Self::output`] 的终态
/// 平铺同构。
fn background_job_row(
    id: u64,
    cmd: &str,
    cwd: &str,
    started_at_unix: u64,
    running: bool,
    steerable: bool,
    total_bytes: Option<u64>,
    dropped_bytes: Option<u64>,
    status_json: Option<serde_json::Value>,
) -> serde_json::Value {
    let mut row = serde_json::json!({
        "id": id,
        "command": cmd,
        "cwd": cwd,
        "started_at_unix": started_at_unix,
        "running": running,
        "steerable": steerable,
        "total_bytes": total_bytes,
        "dropped_bytes": dropped_bytes,
    });
    if let Some(obj) = status_json.as_ref().and_then(serde_json::Value::as_object) {
        for (k, v) in obj {
            row[k.as_str()] = v.clone();
        }
    }
    row
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
    /// 输出落盘目录（2026-11-22 输出落盘 tee）：Some = 每个 job 的输出
    /// chunk 双写 `{log_dir}/bg_{millis}_{job_id}.log` append-only 镜像；
    /// None = tee 关（默认——基线/测试/executor 分离形态零磁盘副作用；
    /// gateway 装配期注入 `<workspace>/logs/background`）。
    log_dir: Option<PathBuf>,
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
        // runtime 关停路径由 Child 的 kill_on_drop(true) 兜底。tee 镜像
        // 文件不删——盘上日志是档案（本机制的意义所在）。
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

    /// 测试/定制形态：指定任务数与输出上限（tee 关——零磁盘副作用）。
    pub fn with_limits(max_jobs: usize, max_output: usize) -> Self {
        Self {
            jobs: Arc::new(Mutex::new(HashMap::new())),
            next_id: AtomicU64::new(1),
            max_jobs,
            max_output,
            log_dir: None,
        }
    }

    /// gateway 装配形态：注入输出落盘目录（tee 开——每任务输出双写
    /// append-only 镜像文件，见 [`TeeFile`]）。
    pub fn with_log_dir(mut self, dir: PathBuf) -> Self {
        self.log_dir = Some(dir);
        self
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
            let status = job.status.lock().await;
            rows.push(background_job_row(
                id,
                &job.cmd,
                &job.cwd,
                job.started_at
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
                running,
                job.steerable,
                Some(buf.produced),
                Some(buf.dropped),
                status.as_ref().map(|s| s.to_json()),
            ));
        }
        rows
    }

    /// 按发起会话盘点（2026-11-17 会话资源看板 background 类目）：
    /// **同步 try_lock 快照**——`GlobalResourceProvider::collect` 是 sync
    /// 契约，注册表主锁是 tokio Mutex（async），不能 `blocking_lock`（
    /// runtime 内调用会 panic/deadlock）。全部走 `try_lock` 不等待：注册表
    /// 主锁拿不到（起任务/收割窗口）= `Err` 忙，看板该拍 Unavailable、
    /// 下拍自愈；输出锁拿不到（pump 写窗口）= 字节计数 `null`（诚实缺省，
    /// 不是 0）；status 锁拿不到（完成写窗口，微秒级）= running best-effort
    /// 视作仍在跑，下一拍自愈。try_lock 零等待 = 本方法与任何持锁任务
    /// 结构性无死锁。
    ///
    /// 只返回 `origin.session_key` 精确匹配的条目，行形状与 [`Self::list`]
    /// 一致（[`background_job_row`] 单一真相源）。基线 `start()` 路径
    /// （origin=None，delegate 等系统面任务）不进任何会话的盘点——无归属
    /// 即不归属，诚实口径。
    pub fn list_by_session_snapshot(
        &self,
        session_key: &str,
    ) -> Result<Vec<serde_json::Value>, &'static str> {
        let Ok(jobs) = self.jobs.try_lock() else {
            return Err("后台进程注册表忙，请稍后刷新");
        };
        let mut ids: Vec<u64> = jobs
            .iter()
            .filter(|(_, j)| j.origin.as_ref().map(|o| o.session_key.as_str()) == Some(session_key))
            .map(|(id, _)| *id)
            .collect();
        ids.sort_unstable_by(|a, b| b.cmp(a));
        let mut rows = Vec::with_capacity(ids.len());
        for id in ids {
            let job = &jobs[&id];
            // running 判据与 is_running 同源：status 槽 None = 仍在跑。
            let status_guard = job.status.try_lock().ok();
            let running = status_guard.as_ref().map(|s| s.is_none()).unwrap_or(true);
            let status_json = status_guard
                .as_ref()
                .and_then(|s| s.as_ref())
                .map(|st| st.to_json());
            let (total_bytes, dropped_bytes) = match job.output.try_lock() {
                Ok(buf) => (Some(buf.produced), Some(buf.dropped)),
                Err(_) => (None, None),
            };
            rows.push(background_job_row(
                id,
                &job.cmd,
                &job.cwd,
                job.started_at
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
                running,
                job.steerable,
                total_bytes,
                dropped_bytes,
                status_json,
            ));
        }
        Ok(rows)
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
        self.start_inner(command, cwd, steerable, None, Arc::new(Vec::new()), None)
            .await
    }

    /// [`Self::start`] 的 env 槽形态（vault 计划方案 A，2026-11-11）：
    /// `env` = 显式环境变量（`background_start` 工具解析传入；值已由
    /// dispatch 注入层把 `vault:` 别名改写为真值）。`start()` 原签名保留
    /// ——基线注册（delegate_registry）与既有测试零改动。
    /// `secrets`（vault 计划 P2++，2026-11-11）：本调用 dispatch 注入池
    /// （`ctx.secrets` 原样捕获）——存进 JobEntry，供 output()/完成通告
    /// 两个读出面遮蔽任务回显的真值；空池 = 零成本直通。
    ///
    /// `origin`（2026-11-19 会话资源看板 G1 定性修复）：发起会话锚随
    /// 条目落库——此前 `start_with_origin` 只把 origin 传给完成唤醒
    /// watcher，条目上恒 `origin: None`，`list_by_session_snapshot` 按
    /// 会话盘点永远空（资源看板 background 类目对任何会话都 total=0，
    /// 直起的后台进程在面板不可见；实录 probe-g9：job_id=1/2 在跑、
    /// 面板双零）。修 = origin 下穿进 start_inner，条目出生即带锚；
    /// None（基线 `start()` 路径）行为不变。
    async fn start_inner(
        &self,
        command: &str,
        cwd: &str,
        steerable: bool,
        env: Option<&[(String, String)]>,
        secrets: Arc<Vec<String>>,
        origin: Option<JobOrigin>,
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
            // 显式 env 槽在 sanitize 之后追加（build_platform_shell 内置
            // sanitize——声明意图语义，同 exec 工具族）。
            .envs(env.iter().flat_map(|pairs| pairs.iter().cloned()))
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
        // job_id 先于 pump 装配分配：tee 文件名需要它（bg_{millis}_{id}.log）。
        // spawn 失败路径不回收（id 序列出现空洞）——id 是不透明句柄，无害。
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);

        let output = Arc::new(Mutex::new(OutputBuffer::new(self.max_output)));
        let status: Arc<Mutex<Option<JobStatus>>> = Arc::new(Mutex::new(None));
        let kill_flag = Arc::new(AtomicBool::new(false));
        let stdin = Arc::new(Mutex::new(if steerable {
            child.stdin.take()
        } else {
            None
        }));

        // 输出落盘 tee（2026-11-22）：log_dir 注入形态创建 append-only 镜像
        // 文件；创建失败诚实降级 = 无 tee（内存缓冲语义不变，warn 一次）。
        let (tee, log_path) = match self.log_dir.as_ref() {
            Some(dir) => match TeeFile::create(dir, id, command, cwd) {
                Ok(t) => {
                    let p = t.path().to_path_buf();
                    (Some(Arc::new(t)), Some(p))
                }
                Err(e) => {
                    tracing::warn!(
                        job = id,
                        error = %e,
                        "[BackgroundRegistry] tee file create failed — output stays memory-only"
                    );
                    (None, None)
                }
            },
            None => (None, None),
        };

        // 同 exec 的 管道必须由独立任务并发排干——否则长输出
        // 写满 ~64KB 管道缓冲会把子进程卡死在 write 上。stdout/stderr 合流
        // 追加进同一缓冲（保持产出顺序交错，与终端观感一致）；tee 在场时
        // 双写落盘（append-only 完整镜像）。JoinHandle 直接丢弃——任务随
        // 管道 EOF 自行退出。
        spawn_pump(child.stdout.take(), output.clone(), tee.clone());
        spawn_pump(child.stderr.take(), output.clone(), tee.clone());

        // 监督任务：轮询 try_wait（微秒级持锁）→ 记录终态；见 kill 旗标先
        // 树杀。不持锁跨 await，background_output 的读路径永不被阻塞。
        let child = Arc::new(Mutex::new(child));
        tokio::spawn(supervise(
            pid.expect("freshly spawned child always has a pid"),
            child,
            status.clone(),
            kill_flag.clone(),
        ));

        // payload 用落盘路径的显示串（log_path 本体随 JobEntry 入册）。
        let log_path_display = log_path.as_ref().map(|p| p.to_string_lossy().to_string());

        self.jobs.lock().await.insert(
            id,
            JobEntry {
                cmd: command.to_string(),
                cwd: cwd.to_string(),
                started_at: std::time::SystemTime::now(),
                output,
                status,
                log_path,
                kill_flag,
                stdin,
                steerable,
                secrets,
                origin,
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

        let mut payload = serde_json::json!({
            "job_id": id,
            "pid": pid,
            "command": command,
            "cwd": cwd,
            "running": true,
            "steerable": steerable,
            "note": note,
        });
        // 落盘 tee 在场时透出镜像文件路径（模型可发现的「最差读文件」面
        // ——内存环形窗口环绕后，早期输出从文件兜）。
        if let Some(p) = log_path_display {
            payload["log_path"] = serde_json::json!(p);
        }
        Ok(payload)
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
        // vault 计划 P2++（2026-11-11）：分页读出过本 job 掩码池——env 槽
        // 真值被任务回显时不随工具结果落 session_log / 进 LLM 上下文。
        // next_offset 仍按原始字节坐标推进（掩码改写长度不回写坐标）；
        // 凭据恰被分页边界切开的极端时序尽力而为（同实时尾窗滑窗边界，
        // 权威面 = 无回显即无暴露）。
        let chunk_text = {
            // 解码走单一真相源（BUG-20261115-034）：cmd /C 后台任务的无控制台
            // 管道输出按 OEM 码页产出，lossy UTF-8 会打成 U+FFFD。CJK 码页
            // 机器上 GBK 等 8 位编码文本就地恢复。已知边界（诚实降级）：
            // 码页多字节序列恰在 MAX_CHUNK_BYTES 窗口边界劈开时两半皆损
            // （无增量码页解码；utf8_safe_len 只护 UTF-8 尾，对非 UTF-8
            // 字节不截）——UTF-8 输出（绝大多数后台任务）边界安全不变。
            let raw = nemesis_utils::child_decode::decode_child_output(&chunk);
            if job.secrets.is_empty() {
                raw
            } else {
                crate::r#loop::session_secrets::mask_secrets_in_result(&raw, job.secrets.as_ref())
            }
        };
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
            "chunk": chunk_text,
        });
        // 落盘 tee 在场时透出完整历史镜像路径（2026-11-22 输出落盘）：内存
        // 环形窗口环绕后早期输出从文件兜（read_file/grep 可达）——模型可
        // 发现的「最差读文件」路径。
        if let Some(p) = &job.log_path {
            payload["log_path"] = serde_json::json!(p.to_string_lossy());
        }
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

    /// 起一个带发起会话锚的后台任务（origin 盘点面形态）。
    ///
    /// 与 [`Self::start`] 同一装配路径（`start()` 原样保留、不签名变更——
    /// 基线注册与既有测试继续走它）；origin 在场时随条目落库，供
    /// [`Self::list_by_session_snapshot`] 按会话盘点（资源看板 background
    /// 类目）。`origin = None` → 行为与 `start()` 完全一致。
    ///
    /// **完成唤醒已摘除（2026-11-22 拍板）**：完成推送定性为流程 bug——
    /// 后台化的产品语义就是「去干活、不用告诉我」，要结果由 agent 自己
    /// 选（轮询 `background_output` / `wait_secs` 显式等待）。原 notify
    /// 参数随本体一并退场（「给错误功能加开关」不再有存在意义）。
    ///
    /// `env`（vault 计划方案 A，2026-11-11）：显式环境变量槽（同
    /// [`Self::start_inner`]；工具 execute 解析传入，None = 零行为变化）。
    /// `secrets`（vault 计划 P2++，2026-11-11）：本调用 dispatch 注入池
    /// （`ctx.secrets` 原样捕获）——随 JobEntry 存档，`output()` 读出面
    /// 据此遮蔽回显真值。
    pub async fn start_with_origin(
        &self,
        command: &str,
        cwd: &str,
        steerable: bool,
        origin: Option<JobOrigin>,
        env: Option<&[(String, String)]>,
        secrets: Arc<Vec<String>>,
    ) -> Result<serde_json::Value, String> {
        self.start_inner(command, cwd, steerable, env, secrets, origin)
            .await
    }

    /// exec 超时收编（2026-11-16 B43：exec 超时自动后台化）。
    ///
    /// 把一个**已 spawn、超时未退**的前台 exec 子进程原样收进注册表——
    /// 不杀，进程继续跑；四动词（output / kill / steer 诚实拒绝）照常
    /// 生效。调用方（ExecTool 超时臂）在收编前已把 stdout/stderr
    /// 排水切换为经 `stdout_rx` / `stderr_rx` 通道实时转发；
    /// `seed_output` = 超时前已累积的输出快照（先入缓冲与 tee 镜像，
    /// 保住超时前的可见性——环形 cap 照常裁头、`dropped_bytes` 如实）。
    /// 通道关闭 = 管道 EOF。
    ///
    /// 与 [`Self::start`] 的差异：不 spawn 子进程（child 由调用方移交，
    /// 含其 `kill_on_drop(true)` 兜底——条目被淘汰 / 注册表 Drop 时随
    /// Child drop 收杀，与 start 路径同语义）；stdin 恒 None（exec 以
    /// null stdin spawn，`steer` 走诚实拒绝文案）；pid 由调用方在 spawn
    /// 后即捕获传入（超时时刻 `child.id()` 可能已 None——进程已退未收尸
    /// 的窗口）。收编失败（满员全在跑 / pid 缺席）时 Err 返回，child 随
    /// 失败路径 drop（kill_on_drop 收杀兜底），调用方回落树杀旧路径。
    ///
    /// tee（2026-11-22 输出落盘）：log_dir 注入形态下，收编即建镜像文件
    /// ——seed（超时前输出）先行落盘，后续转发 chunk 双写（收编时刻起
    /// 落盘 = 已知历史的完整镜像）。
    #[allow(clippy::too_many_arguments)]
    pub async fn adopt_exec(
        &self,
        command: &str,
        cwd: &str,
        child: tokio::process::Child,
        pid: Option<u32>,
        seed_output: Vec<u8>,
        stdout_rx: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
        stderr_rx: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
        origin: Option<JobOrigin>,
        secrets: Arc<Vec<String>>,
    ) -> Result<serde_json::Value, String> {
        let Some(pid) = pid else {
            return Err("child pid unavailable (already reaped?)".to_string());
        };
        // 满员处理：与 start_inner 同款——先淘汰最旧的已完成任务；全在
        // 跑才拒绝（收编是尽力而为，拒绝即回落树杀）。
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
                        debug!(
                            evicted = id,
                            "[BackgroundRegistry] evicted finished job (adopt)"
                        );
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

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let output = Arc::new(Mutex::new(OutputBuffer::new(self.max_output)));
        // 超时前快照先入缓冲（保住超时前的输出可见性）。
        output.lock().await.append(&seed_output);
        let status: Arc<Mutex<Option<JobStatus>>> = Arc::new(Mutex::new(None));
        let kill_flag = Arc::new(AtomicBool::new(false));

        // tee（2026-11-22）：收编即建镜像文件；创建失败诚实降级 = 无 tee。
        let (tee, log_path) = match self.log_dir.as_ref() {
            Some(dir) => match TeeFile::create(dir, id, command, cwd) {
                Ok(t) => {
                    t.append(&seed_output);
                    let p = t.path().to_path_buf();
                    (Some(Arc::new(t)), Some(p))
                }
                Err(e) => {
                    tracing::warn!(
                        job = id,
                        error = %e,
                        "[BackgroundRegistry] tee file create failed (adopt) — output stays memory-only"
                    );
                    (None, None)
                }
            },
            None => (None, None),
        };

        // 转发泵：调用方排水任务经通道送 chunk（stdout/stderr 合流追加，
        // 与 start 路径的 pump 语义一致；tee 在场时双写落盘）。JoinHandle
        // 直接丢弃——任务随通道关闭（双管道 EOF）自行退出。
        tokio::spawn(pump_forward(stdout_rx, output.clone(), tee.clone()));
        tokio::spawn(pump_forward(stderr_rx, output.clone(), tee.clone()));

        let child = Arc::new(Mutex::new(child));
        tokio::spawn(supervise(pid, child, status.clone(), kill_flag.clone()));

        // payload 用落盘路径的显示串（log_path 本体随 JobEntry 入册）。
        let log_path_display = log_path.as_ref().map(|p| p.to_string_lossy().to_string());

        self.jobs.lock().await.insert(
            id,
            JobEntry {
                cmd: command.to_string(),
                cwd: cwd.to_string(),
                started_at: std::time::SystemTime::now(),
                output,
                status,
                log_path,
                kill_flag,
                stdin: Arc::new(Mutex::new(None)),
                steerable: false,
                secrets: Arc::clone(&secrets),
                origin,
            },
        );
        debug!(
            job = id,
            pid,
            adopted = true,
            "[BackgroundRegistry] adopted timed-out exec"
        );

        let note = "timed-out exec adopted into the background registry; it is still \
                    running — read output with background_output (wait_secs blocks for \
                    the exit code), stop with background_kill";
        let mut payload = serde_json::json!({
            "job_id": id,
            "pid": pid,
            "command": command,
            "cwd": cwd,
            "running": true,
            "steerable": false,
            "adopted": true,
            "note": note,
        });
        if let Some(p) = log_path_display {
            payload["log_path"] = serde_json::json!(p);
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

    /// 用户面移除单个任务条目（Dashboard「清除」；仅终态行——运行中诚实
    /// 拒绝并指引先 kill）。移除后 agent 工具通道再读 output 报 no such
    /// job_id（前端 tail 的「输出已过期」态兜住）。返回移除确认。
    pub async fn remove(&self, id: u64) -> Result<serde_json::Value, String> {
        let mut jobs = self.jobs.lock().await;
        let running = match jobs.get(&id) {
            Some(job) => job.is_running().await,
            None => {
                return Err(format!("background.remove: no such job_id {}", id));
            }
        };
        if running {
            return Err(format!(
                "background.remove: job {} is still running; stop it with \
                 background_kill first",
                id
            ));
        }
        jobs.remove(&id);
        debug!(job = id, "[BackgroundRegistry] removed by user");
        Ok(serde_json::json!({ "removed": true, "id": id }))
    }

    /// 用户面批量清除全部终态条目（Dashboard「清除已完成」）。运行中条目
    /// 保留（无从确认死活的条目不擅杀）。状态判定用 `try_lock`（持 jobs
    /// 锁期间不 await status——与 start_inner 满员淘汰同一形态）。
    pub async fn clear_finished(&self) -> serde_json::Value {
        let mut jobs = self.jobs.lock().await;
        let victims: Vec<u64> = jobs
            .iter()
            .filter(|(_, j)| j.status.try_lock().map(|s| s.is_some()).unwrap_or(false))
            .map(|(id, _)| *id)
            .collect();
        for id in &victims {
            jobs.remove(id);
        }
        debug!(
            removed = victims.len(),
            remaining = jobs.len(),
            "[BackgroundRegistry] cleared finished jobs"
        );
        serde_json::json!({ "removed": victims.len(), "remaining": jobs.len() })
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

/// 排干管道追加进共享输出缓冲（stdout/stderr 各一个任务）；`tee` 在场时
/// 双写落盘（2026-11-22 输出落盘——append-only 完整镜像）。任务随管道
/// EOF 自行退出（调用方不需要 JoinHandle）。
///
/// `pub(crate)`：delegate_registry 复用（delegate 子进程同样
/// 必须并发排干，否则 64KB 管道缓冲写满卡死子进程—— 教训同源）；
/// delegate 无落盘面，恒传 `None`。
pub(crate) fn spawn_pump(
    pipe: Option<impl tokio::io::AsyncRead + Unpin + Send + 'static>,
    output: Arc<Mutex<OutputBuffer>>,
    tee: Option<Arc<TeeFile>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut pipe = match pipe {
            Some(p) => p,
            None => return,
        };
        let mut chunk = [0u8; 8192];
        loop {
            match tokio::io::AsyncReadExt::read(&mut pipe, &mut chunk).await {
                Ok(0) | Err(_) => return,
                Ok(n) => {
                    output.lock().await.append(&chunk[..n]);
                    if let Some(t) = tee.as_ref() {
                        t.append(&chunk[..n]);
                    }
                }
            }
        }
    })
}

/// 收编路径的转发泵（2026-11-16 B43）：消费 exec 侧排水任务经通道送来的
/// chunk，追加进注册表输出缓冲（stdout/stderr 合流，与 [`spawn_pump`]
/// 语义一致）；`tee` 在场时双写落盘。unbounded 通道 = 发送端（exec 侧
/// 排水任务）永不阻塞；通道关闭 = 对应管道 EOF，任务退出。
async fn pump_forward(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
    output: Arc<Mutex<OutputBuffer>>,
    tee: Option<Arc<TeeFile>>,
) {
    while let Some(chunk) = rx.recv().await {
        output.lock().await.append(&chunk);
        if let Some(t) = tee.as_ref() {
            t.append(&chunk);
        }
    }
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
        // BUG-20261122-017：gateway 以 windows 子系统构建（main.rs
        // `windows_subsystem = "windows"`），控制台子进程缺
        // CREATE_NO_WINDOW 会弹黑窗（本文件 cmd spawn 旗标对齐）。
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
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
/// `gate`：editor 开关（Full Access）围栏闸——界外 cwd 拒绝分支先问一次
/// （exec 族 = 整体放行族，见 `loop_tools::cwd_gate_allows` 注）。None =
/// 旧行为。`pub(crate)`：delegate_registry 的 delegate_start 复用同一
/// 语义（子代理工作目录与后台任务同规约束）。
pub(crate) fn resolve_cwd(
    workspace: &str,
    restrict: bool,
    cwd: Option<&str>,
    gate: Option<&crate::loop_tools::EditorGate>,
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
        // ── exec 族工作区围栏（background_start / delegate_start 共用）──
        // 与 ExecTool 同形态同闸（editor 开关 × exec 围栏两层不认账修复）：
        // 界外 cwd 先问闸再拒。产线形态**恒上膛**（registry/background
        // 注册点硬编码 restrict=true，config 关不掉），editor 开关闸是唯一
        // 运行时解锁。闸口径 = 整体放行族（读族臂，见
        // [`crate::loop_tools::cwd_gate_allows`] 注）；闸缺席 / false =
        // 维持拒绝（旧行为，消息带解锁指引）。
        if !resolved.starts_with(&ws) && !crate::loop_tools::cwd_gate_allows(gate, raw) {
            return Err(crate::loop_tools::cwd_denied_message(raw));
        }
    }
    Ok(target.to_string_lossy().to_string())
}

/// `background_start`：起长跑进程，立刻返回 job_id。
pub struct BackgroundStartTool {
    workspace: String,
    restrict: bool,
    registry: Arc<BackgroundProcessRegistry>,
    /// editor 开关（Full Access）围栏闸（见 [`crate::loop_tools::EditorGate`]）。
    editor_gate: Option<crate::loop_tools::EditorGate>,
}

impl BackgroundStartTool {
    pub fn new(workspace: &str, restrict: bool, registry: Arc<BackgroundProcessRegistry>) -> Self {
        Self {
            workspace: workspace.to_string(),
            restrict,
            registry,
            editor_gate: None,
        }
    }

    /// editor 开关（Full Access）围栏闸链式注入（registry 从
    /// `SharedToolConfig.workspace_boundary.editor_gate` 提取分发）。None = 旧行为。
    pub fn with_editor_gate(mut self, gate: Option<crate::loop_tools::EditorGate>) -> Self {
        self.editor_gate = gate;
        self
    }
}

#[async_trait]
impl Tool for BackgroundStartTool {
    fn description(&self) -> String {
        "Start a long-running shell command in the background and return immediately \
         with a job_id. Use for dev servers, watchers, builds, GUI apps (notepad, \
         calc), or any command that outlives a normal exec timeout. Read output \
         incrementally with background_output (offset paging); stop with \
         background_kill. The job's full output is also teed to a log file on disk \
         (log_path in responses) — the in-memory window is capped at 256KB, so read \
         the log_path file for very early output that has scrolled out. Use \
         background_output wait_secs (max 30) when you deliberately want to block \
         for the exit code; do not loop it with sleeps. Pass steerable=true to keep \
         a live stdin pipe and inject input later with background_steer. \
         Credentials/secrets go via the env parameter as vault:<alias> references \
         — never in the command string (it is echoed back in job listings)."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "description": "Shell command to run in the background"},
                "cwd": {"type": "string", "description": "Working directory (default: workspace root; confined to workspace)"},
                "steerable": {"type": "boolean", "description": "Keep a live stdin pipe so the job can receive input via background_steer (default false)"},
                "env": env_slot_schema()
            },
            "required": ["command"]
        })
    }

    // vault 计划方案 A（2026-11-11）：env 槽承载凭据别名（同 ExecTool——
    // dispatch 最内层对象叶改写真值，job 条目不记 env，list/output 面
    // 天然零真值）。
    fn credential_arg_keys(&self) -> &[&str] {
        &["env"]
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
            self.editor_gate.as_ref(),
        )?;
        let steerable = val
            .get("steerable")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        // 显式 env 槽（vault 计划方案 A）：值已由 dispatch 注入层改写
        // 真值（credential_arg_keys=["env"] 声明槽）。
        let env_pairs = parse_env_slot(&val)?;
        // [SEALED 2026-11-08] 旧模式（轮询形态）：不带 origin 起任务。保留
        // 备查；恢复 = 把下面 start_with_origin 调用换回这行并删掉 origin
        // 捕获段。
        // let payload = self.registry.start(command, &cwd, steerable).await?;
        // ── origin 捕获（2026-11-17 会话资源盘点）：任务条目锚定发起会话
        // （list_by_session_snapshot 盘点面）。**完成唤醒已摘除（2026-11-22
        // 拍板：完成推送定性流程 bug）**——origin 只服务盘点，不再驱动任何
        // 回灌/通告；任务要终态就显式 background_output(wait_secs) 或轮询。
        // channel/chat_id 缺席（无会话上下文的调用形态）→ None。
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
            .start_with_origin(
                command,
                &cwd,
                steerable,
                origin,
                env_pairs.as_deref(),
                // vault 计划 P2++（2026-11-11）：本调用 dispatch 注入池
                // 原样捕获进 JobEntry——任务回显的凭据真值在 output()
                // 读出面就地遮蔽（空池零成本直通）。
                context.secrets.clone(),
            )
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
         when the job has finished. There is no completion notice — jobs never \
         notify on exit; when you want the exit code, call this with wait_secs \
         (blocks up to 30s), otherwise poll with offset paging. If the in-memory \
         window has scrolled past early output, read the job's log_path file \
         (returned in responses) which holds the complete history."
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
