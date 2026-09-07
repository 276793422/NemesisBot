//! B4（devtool-upgrade 阶段 3）：网关级后台进程注册表 + 三件套工具。
//!
//! 起一个长跑进程拿到句柄，随后分页读输出、随时终止。此前唯一的异步形态
//! `exec_async` 是 fire-and-forget——`wait_seconds` 后 `Child` 直接 drop（无
//! kill_on_drop），管道被丢弃导致句柄与输出全部不可回读。
//!
//! 生命周期 = 网关进程：注册表是 gateway 级单例（`SharedResources` 持有），
//! 跨 AgentLoop 重启存活；注册表 Drop 时给所有存活任务发 kill 旗标（监督
//! 任务 ≤200ms 内收尸），`kill_on_drop(true)` 兜底 runtime 关停路径。
//!
//! **不进 [`MOVE_TOOLS`](crate::remote_executor_tool::MOVE_TOOLS)**：executor
//! 分离是 per-call 子进程，一次调用后退出——在子进程里起后台任务，句柄必然
//! 随子进程孤儿化。因此三件套固定在 gateway 进程内跑（与 `exec_async` 同一
//! 决策，见 remote_executor_tool.rs 顶部注记），`exec_worker` 侧不注入注册表
//! （SharedToolConfig.background_registry = None → 工具不注册）。
//!
//! 安全：`background_start` 语义等同 `exec`（起进程）→ 8 层管线按
//! ProcessExec 审查命令本体；`background_output` / `background_kill` 只操作
//! 本注册表内的自有任务（命令已在 start 时过闸，读缓冲/杀自属子进程不构成
//! 新的攻击面）→ 不映射 operation（走未知名放行分支）。
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

/// 注册表默认任务上限（B4 验收：16 上限拒绝）。满员时优先淘汰最旧的
/// **已完成**任务（输出仍可读期间照常挤占——超出 16 就得腾地方），全部
/// 在跑才拒绝新任务。
pub const DEFAULT_MAX_JOBS: usize = 16;

/// 每任务输出缓冲上限（丢头部保尾部）。
pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 256 * 1024;

/// `background_output` 单次返回的 chunk 上限（模型友好分页粒度）。
pub const MAX_CHUNK_BYTES: usize = 8 * 1024;

/// 监督任务轮询子进程退出的间隔。
const SUPERVISE_INTERVAL: Duration = Duration::from_millis(200);

/// `background_kill` 等待子进程退出的上限（超时如实报 still running）。
const KILL_WAIT: Duration = Duration::from_secs(5);

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
#[derive(Debug, Default)]
struct OutputBuffer {
    buf: Vec<u8>,
    produced: u64,
    dropped: u64,
    max: usize,
}

impl OutputBuffer {
    fn new(max: usize) -> Self {
        Self {
            max,
            ..Default::default()
        }
    }

    fn append(&mut self, data: &[u8]) {
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

    /// 从 `offset`（produced 空间）读至多 [`MAX_CHUNK_BYTES`] 字节。
    /// 返回 (chunk, 是否发生了头部钳制)。
    fn read_chunk(&self, offset: u64) -> (Vec<u8>, bool) {
        let clamped = offset.max(self.dropped);
        let skipped = offset < self.dropped;
        let start = (clamped - self.dropped).min(self.buf.len() as u64) as usize;
        let end = (start + MAX_CHUNK_BYTES).min(self.buf.len());
        (self.buf[start..end].to_vec(), skipped)
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
        }
    }

    /// 当前任务数（含已完成——完成后条目保留至被淘汰/注册表销毁）。
    pub async fn job_count(&self) -> usize {
        self.jobs.lock().await.len()
    }

    /// 起一个后台任务。`command` 经平台 shell 解释（cmd /C 或 sh -c），
    /// stdin 恒 null（后台任务无交互输入通道，同 exec 决策）。
    pub async fn start(&self, command: &str, cwd: &str) -> Result<serde_json::Value, String> {
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
            .stdin(std::process::Stdio::null())
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

        // B4 同 exec 的 B2 教训：管道必须由独立任务并发排干——否则长输出
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
            },
        );
        debug!(
            job = id,
            pid = pid.map(|p| p as u64),
            "[BackgroundRegistry] started"
        );

        Ok(serde_json::json!({
            "job_id": id,
            "pid": pid,
            "command": command,
            "cwd": cwd,
            "running": true,
            "note": "poll background_output with job_id (+offset) to read output; \
                     background_kill to stop",
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
        let buf = job.output.lock().await;
        let (chunk, skipped) = buf.read_chunk(offset);
        let running = job.is_running().await;
        let mut payload = serde_json::json!({
            "job_id": id,
            "running": running,
            "command": job.cmd,
            "cwd": job.cwd,
            "started_at_unix": job
                .started_at
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            "total_bytes": buf.produced,
            "dropped_bytes": buf.dropped,
            "offset": offset,
            "next_offset": offset.max(buf.dropped) + chunk.len() as u64,
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
}

/// 监督任务：轮询 try_wait → 记录终态；见 kill 旗标先树杀（一次），再
/// 以 start_kill 兜底直杀 shell（树杀指令丢失时至少不漏直接子进程）。
async fn supervise(
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
                tree_kill(pid).await;
            }
            // 兜底直杀：树杀已生效时这里是已退出进程上的 Err——忽略。
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

/// 排干管道追加进共享输出缓冲（stdout/stderr 各一个任务）。
fn spawn_pump(
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
/// 额外两条（树杀前提，B4）：
/// - Unix `process_group(0)`：子进程自成一进程组（pgid == pid），树杀用
///   `kill -- -PGID` 一锅端（shell 的孙进程不会孤儿化）。
/// - Windows `CREATE_NO_WINDOW`：后台任务不该弹控制台窗口（网关以托盘/
///   服务形态运行时尤其如此；cc_hooks 同款旗标）。
pub(crate) fn build_platform_shell(command: &str) -> tokio::process::Command {
    #[cfg(target_os = "windows")]
    {
        let mut c = tokio::process::Command::new("cmd");
        c.raw_arg(format!("/C {}", command));
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        c
    }
    #[cfg(not(target_os = "windows"))]
    {
        let mut c = tokio::process::Command::new("sh");
        c.arg("-c").arg(command);
        // tokio Command 自带 process_group（镜像 std API），无需 CommandExt。
        c.process_group(0);
        c
    }
}

/// 树杀：干掉 pid 及其整棵子树。
///
/// - Windows：`taskkill /T /F /PID`（OS 级树遍历）。
/// - Unix：`kill -TERM -- -PGID`（进程组杀；pgid == pid，见
///   `build_platform_shell` 的 process_group(0)）。
///
/// 指令丢失（进程已退/PID 复用/kill 不存在）一律忽略错误——调用方随后
/// 的 try_wait 收尸才是真相源。PID 复用窗口（进程退出→树杀之间）是平台
/// 固有竞态，量级为毫秒且后果止于误杀同名树，不做缓解。
async fn tree_kill(pid: u32) {
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
// 三件套工具
// ---------------------------------------------------------------------------

/// cwd 解析 + 工作区边界（与 `ExecTool` 同形态：相对 cwd join workspace，
/// 双侧 canonicalize_for_compare 归一化防 8.3 短名/大小写失配）。
/// 返回**解析后的绝对路径**——`Command::current_dir` 对相对路径按父进程
/// cwd 解释，与工作区基准脱钩（exec 侧传原始串是历史行为，这里不复制）。
fn resolve_cwd(workspace: &str, restrict: bool, cwd: Option<&str>) -> Result<String, String> {
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
         with a job_id. Use for dev servers, watchers, builds, or any command that \
         outlives a normal exec timeout. Read output incrementally with \
         background_output; stop with background_kill."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "description": "Shell command to run in the background"},
                "cwd": {"type": "string", "description": "Working directory (default: workspace root; confined to workspace)"}
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
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
        let payload = self.registry.start(command, &cwd).await?;
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
         when the job has finished."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "job_id": {"type": "integer", "description": "Job id returned by background_start"},
                "offset": {"type": "integer", "description": "Byte offset to read from (default 0; use next_offset from the previous call)"}
            },
            "required": ["job_id"]
        })
    }

    /// 纯读自有缓冲（U5 并行读池安全）。
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
        let payload = self.registry.output(job_id, offset).await?;
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

#[cfg(test)]
mod tests;
