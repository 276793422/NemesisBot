//! 统一 WorkPool 设施（2026-11-03）。
//!
//! 池化组件的四条统一手法（统一定义：`2026-11-01_改进项落地执行清单`
//! 节；未来任何池化组件设计时对照，不各造各的）：
//!
//! 1. **CPU 准入上限** [`cpu_admission_permits`] = `max(1, available_parallelism()-1)`
//!    ——给宿主进程（gateway）留一核，不把调度器饿死。
//! 2. **分池而非单一池**：[`PoolLimits::reader`]（≤2，多核不加——读队列短，
//!    加 isolate 只复制代码缓存无益）/ [`PoolLimits::cpu`]（compute，CPU 准入）/
//!    [`PoolLimits::writer`]（串行保序——提交序=执行序=完成序）。
//! 3. **双上限背压**：pending 任务数 + 生产者自报的 retained input 字节
//!    （`try_submit` / [`WorkPool::try_reserve`] 的 `input_bytes` 形参）；
//!    超限返回**类型化** [`PoolOverloaded`]（立即拒绝，不挂起排队）。
//! 4. **取消三事实分离**：结果拒绝（[`WorkTask::join`] → `Err(TaskCancelled)`）、
//!    执行结果（cancel 落在完成后 join 仍 `Ok`——不追溯）、资源释放
//!    （许可与字节预留保留到 worker 停止、清理落定，[`WorkTask::settle`]
//!    之后账本才回落）是**独立事实**，互不蕴含。
//!
//! 两种执行形态：
//! - **spawn 形态**（[`WorkPool::try_submit`]）：future 必须 `'static`，
//!   许可+账本预留由 worker future 自持；[`WorkTask`] 提供 join/cancel/settle。
//! - **借用形态**（[`WorkPool::try_reserve`] + [`WorkPool::permits`]）：
//!   执行体是借用局部状态的 future（无法 `'static` spawn，如 并行批）时，
//!   只共享全局准入信号量与双上限账本；预留守卫（[`PoolReservation`]）由
//!   调用方 future 自持到执行结束。
//!
//! writer 池是独立驱动（mpsc 队列 + 单消费者逐个 spawn 串行 await）：
//! tokio 多线程调度下 spawn 顺序≠信号量获取顺序，靠信号量做不出严格 FIFO，
//! 保序必须队列化。worker panic 在 writer 池映射为取消（结果通道随 panic
//! future 一起 drop）；spawn 形态则经 [`WorkTask::join`] 原样重放 panic。

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use tokio::sync::{Semaphore, mpsc, oneshot};

/// [`PoolLimits::cpu`] / [`PoolLimits::reader`] / [`PoolLimits::writer`]
/// 共用的默认 retained input 字节上限（64 MiB）。
pub const DEFAULT_MAX_INPUT_BYTES: usize = 64 * 1024 * 1024;

/// 手法1 统一定义：CPU 准入 = `max(1, available_parallelism() - 1)`
/// ——至少 1 个许可，至多 N-1（给宿主进程留一核）。
pub fn cpu_admission_permits() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .saturating_sub(1)
        .max(1)
}

/// 池容量三元组（手法2/3 的参数面；账本在单一锁内 check-and-reserve）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PoolLimits {
    /// 并发执行许可数。
    pub permits: usize,
    /// 在途（已预留未释放）任务数上限。
    pub max_pending: usize,
    /// retained input 字节上限（生产者自报口径的总账）。
    pub max_input_bytes: usize,
}

impl PoolLimits {
    /// compute 池默认：许可 = CPU 准入（手法1），pending = 许可 ×4（≥8，
    /// 批量提交留排队余量），字节 = [`DEFAULT_MAX_INPUT_BYTES`]。
    pub fn cpu() -> Self {
        let permits = cpu_admission_permits();
        Self {
            permits,
            max_pending: permits.saturating_mul(4).max(8),
            max_input_bytes: DEFAULT_MAX_INPUT_BYTES,
        }
    }

    /// reader 池默认（手法2）：≤2 固定，多核不加。
    pub fn reader() -> Self {
        Self {
            permits: 2,
            max_pending: 8,
            max_input_bytes: DEFAULT_MAX_INPUT_BYTES,
        }
    }

    /// writer 池默认（手法2）：串行（1 许可）+ 保序（队列驱动）。
    pub fn writer() -> Self {
        Self {
            permits: 1,
            max_pending: 16,
            max_input_bytes: DEFAULT_MAX_INPUT_BYTES,
        }
    }

    fn normalized(self) -> Self {
        Self {
            permits: self.permits.max(1),
            max_pending: self.max_pending.max(1),
            max_input_bytes: self.max_input_bytes.max(1),
        }
    }
}

/// 手法3：类型化 overloaded（立即拒绝，不挂起排队）。携带四个账本事实，
/// 便于上游拼进面向模型/用户的诚实拒绝文案。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PoolOverloaded {
    pub pending: usize,
    pub max_pending: usize,
    pub retained_input_bytes: usize,
    pub max_input_bytes: usize,
}

impl fmt::Display for PoolOverloaded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "pool overloaded (rejected, not queued): pending {}/{} tasks, retained input {}/{} bytes",
            self.pending, self.max_pending, self.retained_input_bytes, self.max_input_bytes
        )
    }
}

/// 手法4 第一事实面：join 被取消（结果拒绝）。
#[derive(Debug, PartialEq, Eq)]
pub struct TaskCancelled;

impl fmt::Display for TaskCancelled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "task cancelled (result rejected; execution and resource release are independent facts)"
        )
    }
}

impl std::error::Error for TaskCancelled {}

/// 双上限账本（pending + retained input 字节）。增减全部在单一锁下
/// check-and-reserve；释放经 [`GaugeGuard`] Drop（worker 停止/清理落定
/// 时刻）。Arc 化使 spawn 池与 writer 队列池共用同一守卫逻辑。
#[derive(Default)]
struct Gauge {
    pending: usize,
    retained_input_bytes: usize,
}

type SharedGauge = Arc<parking_lot::Mutex<Gauge>>;

/// Drop 释放账本预留（guard 在 worker future 内自持 → 释放时刻 =
/// worker 停止/清理落定时刻，手法4 第三事实的锚点）。
struct GaugeGuard {
    gauge: SharedGauge,
    input_bytes: usize,
}

impl Drop for GaugeGuard {
    fn drop(&mut self) {
        let mut g = self.gauge.lock();
        g.pending = g.pending.saturating_sub(1);
        g.retained_input_bytes = g.retained_input_bytes.saturating_sub(self.input_bytes);
    }
}

/// 双上限 check-and-reserve（调用方持锁面已收拢于此）。超限 = `Err`
///（类型化，立即拒绝不排队）；成功 = 账本已预留。
fn reserve(
    gauge: &SharedGauge,
    limits: &PoolLimits,
    input_bytes: usize,
) -> Result<GaugeGuard, PoolOverloaded> {
    let mut g = gauge.lock();
    let over = g.pending >= limits.max_pending
        || g.retained_input_bytes.saturating_add(input_bytes) > limits.max_input_bytes;
    if over {
        return Err(PoolOverloaded {
            pending: g.pending,
            max_pending: limits.max_pending,
            retained_input_bytes: g.retained_input_bytes,
            max_input_bytes: limits.max_input_bytes,
        });
    }
    g.pending += 1;
    g.retained_input_bytes += input_bytes;
    Ok(GaugeGuard {
        gauge: Arc::clone(gauge),
        input_bytes,
    })
}

/// 借用形态的账本预留守卫（[`WorkPool::try_reserve`] 产物）。
/// Drop 释放；执行体自己经 [`WorkPool::permits`] 拿准入许可。
pub struct PoolReservation {
    /// Drop 释放账本预留（借形执行体的持有期 = 预留期）。
    _guard: GaugeGuard,
    input_bytes: usize,
}

impl PoolReservation {
    /// 生产者自报的 retained input 字节（预留口径回读）。
    pub fn input_bytes(&self) -> usize {
        self.input_bytes
    }
}

impl fmt::Debug for PoolReservation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PoolReservation")
            .field("input_bytes", &self.input_bytes)
            .finish_non_exhaustive()
    }
}

// ---------- writer 池（串行保序驱动） ----------

type BoxedUnitFuture = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

struct OrderedCore {
    /// 正在执行的任务槽 `(job_id, abort_handle)`——cancel 的身份核验面
    /// （排队中的任务没有槽，cancel 只置 flag）。
    current: parking_lot::Mutex<Option<(u64, tokio::task::AbortHandle)>>,
}

/// 队列元素：结果/settle 通道与账本守卫全部内嵌在 `task` future 里；
/// skip/drop 路径靠整体 Drop 让所有 sender 消亡（接收方映到取消/settle），
/// 无需显式发送。
struct OrderedJob {
    id: u64,
    cancelled: Arc<AtomicBool>,
    task: BoxedUnitFuture,
}

static NEXT_JOB_ID: AtomicU64 = AtomicU64::new(1);

/// writer 池共享核：懒启动的驱动 sender + 运行中任务槽。
struct OrderedPool {
    limits: PoolLimits,
    gauge: SharedGauge,
    core: Arc<OrderedCore>,
    tx: std::sync::OnceLock<mpsc::Sender<OrderedJob>>,
}

// ---------- WorkTask ----------

enum TaskSource<T> {
    Spawned(tokio::task::JoinHandle<T>),
    Ordered {
        id: u64,
        cancelled: Arc<AtomicBool>,
        reply: oneshot::Receiver<Result<T, TaskCancelled>>,
        core: Arc<OrderedCore>,
    },
}

/// `try_submit` / `submit_ordered` 的任务柄。Drop = 放弃结果 = 请求取消
/// （资源释放在 worker 停止时异步落定，不阻塞 drop 方）。
pub struct WorkTask<T> {
    source: Option<TaskSource<T>>,
}

impl<T> WorkTask<T> {
    fn spawned(handle: tokio::task::JoinHandle<T>) -> Self {
        Self {
            source: Some(TaskSource::Spawned(handle)),
        }
    }

    fn take_source(&mut self) -> TaskSource<T> {
        self.source.take().expect("WorkTask source consumed twice")
    }

    /// 手法4 第一事实：请求取消（结果层面拒绝）。worker 在下一悬挂点
    /// 停止；许可与输入预留保留到 worker 停止、清理落定
    /// （[`WorkTask::settle`] 等资源释放）。取消是尽力而为：落在完成后
    /// = 完成照常返回（事实分离，不追溯）。
    pub fn cancel(&mut self) {
        match self.source.as_ref() {
            Some(TaskSource::Spawned(handle)) => handle.abort(),
            Some(TaskSource::Ordered {
                id,
                cancelled,
                core,
                ..
            }) => {
                cancelled.store(true, Ordering::Relaxed);
                // 正在执行且确属本任务才中止（排队中由 driver 出队时查
                // flag 跳过；槽里是别人 = 本任务还在排队，不能误杀）。
                let slot = core.current.lock();
                if let Some((running_id, handle)) = slot.as_ref()
                    && *running_id == *id
                {
                    handle.abort();
                }
            }
            None => {}
        }
    }
}

impl<T: Send + 'static> WorkTask<T> {
    /// 等待结果（手法4：第一、二事实在这里交汇）。
    /// - `Ok(v)`：完成（cancel 请求落在完成后 = 不追溯，照样返回结果）。
    /// - `Err(TaskCancelled)`：结果拒绝（worker 在下一悬挂点停止）。
    ///
    /// worker panic 原样重放到调用方（spawn 形态；writer 形态的 panic
    /// 映射为取消——见模块注释）。
    pub async fn join(mut self) -> Result<T, TaskCancelled> {
        match self.take_source() {
            TaskSource::Spawned(handle) => match handle.await {
                Ok(v) => Ok(v),
                Err(e) if e.is_cancelled() => Err(TaskCancelled),
                Err(e) => std::panic::resume_unwind(e.into_panic()),
            },
            TaskSource::Ordered { reply, .. } => match reply.await {
                Ok(Ok(v)) => Ok(v),
                _ => Err(TaskCancelled),
            },
        }
    }

    /// 手法4 第三事实：等待资源释放落定（worker 停止 + 账本回落）。
    /// 不携带执行结论——资源释放是独立事实。
    pub async fn settle(mut self) {
        match self.take_source() {
            TaskSource::Spawned(handle) => {
                let _ = handle.await;
            }
            TaskSource::Ordered { reply, .. } => {
                let _ = reply.await;
            }
        }
    }
}

impl<T> fmt::Debug for WorkTask<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkTask").finish_non_exhaustive()
    }
}

impl<T> Drop for WorkTask<T> {
    fn drop(&mut self) {
        // 丢弃 = 放弃结果 = 请求取消；资源释放在 worker 停止时异步落定。
        self.cancel();
    }
}

// ---------- WorkPool ----------

/// 统一 WorkPool（手法1-4 的实施体）。Clone 共享同一准入信号量与账本
/// （跨调用方/跨 agent loop 的全局准入纪律）。
///
/// 提交（spawn 形态 / writer 首次提交懒启动驱动）必须在 tokio runtime
/// 上下文内调用；构造无此要求。
#[derive(Clone)]
pub struct WorkPool {
    inner: Arc<PoolInner>,
    ordered: Arc<OrderedPool>,
}

struct PoolInner {
    limits: PoolLimits,
    permits: Arc<Semaphore>,
    gauge: SharedGauge,
}

impl WorkPool {
    /// 按给定容量构造（容量下界钳 1，防手滑 0 配置）。
    pub fn new(limits: PoolLimits) -> Self {
        let limits = limits.normalized();
        Self {
            inner: Arc::new(PoolInner {
                permits: Arc::new(Semaphore::new(limits.permits)),
                gauge: Arc::new(parking_lot::Mutex::new(Gauge::default())),
                limits,
            }),
            ordered: Arc::new(OrderedPool {
                limits,
                gauge: Arc::new(parking_lot::Mutex::new(Gauge::default())),
                core: Arc::new(OrderedCore {
                    current: parking_lot::Mutex::new(None),
                }),
                tx: std::sync::OnceLock::new(),
            }),
        }
    }

    /// compute 池默认容量（手法1 + 手法2），供 OnceLock 静态共享。
    pub fn compute_shared() -> Arc<Self> {
        Arc::new(Self::new(PoolLimits::cpu()))
    }

    /// 准入信号量（借用形态；[`WorkPool::try_reserve`] 拿账本预留后由
    /// 执行体自行 acquire）。永远不 close——acquire 只会 Pending 或 Ok。
    pub fn permits(&self) -> Arc<Semaphore> {
        Arc::clone(&self.inner.permits)
    }

    /// 借用形态提交：只做账本预留（手法3），执行体借用局部状态自行跑
    /// （[`WorkPool::permits`] 拿准入）。预留守卫由调用方持有到执行结束。
    pub fn try_reserve(&self, input_bytes: usize) -> Result<PoolReservation, PoolOverloaded> {
        reserve(&self.inner.gauge, &self.inner.limits, input_bytes).map(|_guard| PoolReservation {
            _guard,
            input_bytes,
        })
    }

    /// spawn 形态提交（compute/reader 池）。future 必须 `'static`；许可
    /// 与账本预留由 worker future 自持到停止（含清理析构）。
    /// 超双上限 = `Err(PoolOverloaded)`（立即拒绝，不排队）。
    pub fn try_submit<F, T>(
        &self,
        input_bytes: usize,
        task: F,
    ) -> Result<WorkTask<T>, PoolOverloaded>
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        let guard = reserve(&self.inner.gauge, &self.inner.limits, input_bytes)?;
        let permits = Arc::clone(&self.inner.permits);
        let handle = tokio::spawn(async move {
            // 许可从 worker 启动持有到 future 整体 drop（abort 时刻的
            // 析构含 guard）——手法4 第三事实。信号量永不 close，恒 Ok。
            let _permit = permits.acquire_owned().await;
            let _guard = guard;
            task.await
        });
        Ok(WorkTask::spawned(handle))
    }

    /// writer 池提交（串行保序：提交序 = 执行序 = 完成序）。容量里的
    /// `permits` 字段被忽略（恒按 1 语义由驱动串行执行）。
    pub fn submit_ordered<F, T>(
        &self,
        input_bytes: usize,
        task: F,
    ) -> Result<WorkTask<T>, PoolOverloaded>
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        let o = &*self.ordered;
        let guard = reserve(&o.gauge, &o.limits, input_bytes)?;
        let id = NEXT_JOB_ID.fetch_add(1, Ordering::Relaxed);
        let cancelled = Arc::new(AtomicBool::new(false));
        let (reply_tx, reply_rx) = oneshot::channel::<Result<T, TaskCancelled>>();

        // 完成路径：显式 drop(guard) 先于结果投递（释放先于接收方可观测，
        // settle 顺序保证）；abort/skip 路径：guard 与全部 sender 随
        // future drop——接收方映到取消/settle。
        let task_boxed: BoxedUnitFuture = Box::pin(async move {
            let v = task.await;
            drop(guard);
            let _ = reply_tx.send(Ok(v));
        });

        // 驱动懒启动（首次提交）；驱动随存活 sender 数退出（池全 drop 后
        // recv None 收尾，队列残留整体 Drop → 调用方映到取消）。
        let tx =
            o.tx.get_or_init(|| {
                let (tx, rx) = mpsc::channel(o.limits.max_pending);
                let core = Arc::clone(&o.core);
                tokio::spawn(run_ordered_driver(rx, core));
                tx
            })
            .clone();

        // 队列深度账本已在 reserve 预留；通道容量（max_pending）是同一
        // 数字的硬背板。满 = 立即拒绝（归还的 job 整体 drop → guard 随
        // future drop 回滚预留，不排队）。
        let job = OrderedJob {
            id,
            cancelled: Arc::clone(&cancelled),
            task: task_boxed,
        };
        if let Err(e) = tx.try_send(job) {
            // 通道满（防御臂：reserve 已把 pending 钳在 max_pending =
            // 通道容量，正常到不了）。归还的 job 整体 drop → guard 随
            // future drop 回滚预留（先 drop 再持锁防死锁）。
            drop(match e {
                tokio::sync::mpsc::error::TrySendError::Full(j) => j,
                tokio::sync::mpsc::error::TrySendError::Closed(j) => j,
            });
            let g = o.gauge.lock();
            return Err(PoolOverloaded {
                pending: g.pending,
                max_pending: o.limits.max_pending,
                retained_input_bytes: g.retained_input_bytes,
                max_input_bytes: o.limits.max_input_bytes,
            });
        }

        Ok(WorkTask {
            source: Some(TaskSource::Ordered {
                id,
                cancelled,
                reply: reply_rx,
                core: Arc::clone(&o.core),
            }),
        })
    }
}

/// writer 池驱动：逐个出队 → spawn → 串行 await。保序+串行由「上一个
/// 完成才收下一个」构造性保证（提交序=出队序=spawn 序=完成序）。
async fn run_ordered_driver(mut rx: mpsc::Receiver<OrderedJob>, core: Arc<OrderedCore>) {
    while let Some(job) = rx.recv().await {
        if job.cancelled.load(Ordering::Relaxed) {
            continue; // 排队中已取消：整体 Drop → 接收方映到取消/settle
        }
        let handle = tokio::spawn(job.task);
        *core.current.lock() = Some((job.id, handle.abort_handle()));
        let _ = handle.await;
        *core.current.lock() = None;
    }
}

#[cfg(test)]
mod tests;
