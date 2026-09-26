//! WS9/P22：会话租约（WorkspaceLease）——workspace 级写租约（v1 粒度 =
//! 整个工作区的写类操作互斥）。
//!
//! 背景：主 loop / 项目 loop / executor 子进程 / cron sweep 多个执行体
//! 并发写同一工作区时，write-class 工具可能交错落盘（A 的 checkpoint 影
//! 子树与 B 的编辑互相踩）。租约 = **文件锁（OS 级）+ 持有者 sidecar（可
//! 读性）**：
//! - 文件锁用 `fd-lock`（`RwLock<File>` 包装 Windows `LockFileEx` /
//!   POSIX `flock`）——锁按**文件句柄**生效：同进程双句柄与跨进程语义
//!   一致；持有进程崩溃/被杀，OS 自动释放（无死锁遗留）；
//! - `<workspace>/logs/workspace_lease.lock` 是锁载体；旁边的
//!   `workspace_lease.lock.holder.json` 记 `{holder, acquired_at}`——锁
//!   本身不可读名字，sidecar 专供人/前端展示（`probe` 读取；进程死亡后
//!   sidecar 残留 = 陈旧记录，probe 诚实标注）。
//!
//! 持锁形态（架构要点）：**guard 不出借**——`fd-lock` 的
//! `RwLockWriteGuard<'lock, File>` 借用 `RwLock` 本体，「自持锁 + 自持
//! guard」是自引用结构，借用检查容不下；轮询重试循环里 guard 携带
//! `'a` 逃逸更是 E0499 必然。因此锁由**专用 std 线程**持有（线程内
//! try_write 轮询 → 持 guard → 阻塞等释放信号；sync 阻塞在独立线程无
//! 害），调用方只拿 owned 的 [`LeaseAcquisition`]（Drop 发释放信号）。
//!
//! 接线（v1，诚实边界）：
//! - **工具包装层**接线（`register_shared_tools` 尾部把写类工具换成
//!   [`LeaseGuardTool`] 委派包装）——**不动 loop.rs dispatch 流**；
//! - `exec` 不包（v1 边界：exec 的写副作用无参数级声明，包装只能拦到
//!   命令面，拦不到效果面——诚实不做假拦截）；
//! - 开关 `agents.lease_enabled`（默认 true）在**装配期**消费
//!   （agent_factory 构造 `SharedToolConfig.workspace_lease`）——改配置
//!   需重启生效（与 executor 段同语义）；
//! - **F8 豁免（诚实边界）**：rewind/redo 的文件恢复（checkpoint
//!   `git_restore` / `hybrid_restore`，checkout+删除直达工作区）**不走租
//!   约**——它是用户显式发起的独占操作，前置已有 busy 闸（目标会话不在
//!   执行中）+ P20 冲突预检 + force 审计留痕；且「恢复」的本职就是覆盖
//!   工作区现状，包进租约会与其他执行体的写意图对峙排队，语义打架。
//!   并发写互斥的担保范围 = 写类**工具**调用，不含恢复类运维动作。

use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use fd_lock::RwLock;

use crate::context::RequestContext;
use crate::r#loop::{FileChange, Tool};

/// 锁载体文件名（`<workspace>/logs/` 下）。
const LOCK_FILE_NAME: &str = "workspace_lease.lock";

/// 默认宽限期：锁被占时排队等待的上限（超过即诚实拒绝）。行业对齐
/// （编辑器写锁 30s 级）；测试用 `with_grace` 收窄。
pub const LEASE_DEFAULT_GRACE: Duration = Duration::from_secs(30);

/// 排队轮询间隔（锁是纳秒级 syscall，轮询足够；不引入额外 notify 通道）。
const LEASE_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Workspace 级写租约（`SharedToolConfig.workspace_lease` 的载荷）。
/// `Clone`（Arc 共享内部无须额外堆）；`Debug` 手写（holder 可能含用户名
/// 级信息——照实打，无敏感字节）。
#[derive(Clone)]
pub struct WorkspaceLease {
    /// 锁载体文件绝对路径（`<workspace>/logs/workspace_lease.lock`）。
    lock_path: PathBuf,
    /// 持有者 sidecar（`<lock>.holder.json`）——展示用，不参与互斥。
    holder_path: PathBuf,
    /// 本租约的使用方身份（`gateway:pid:{pid}` / `executor:pid:{pid}` /
    /// 测试自定义）。acquire 成功时写进 sidecar。
    holder: String,
    /// 排队宽限（超时诚实拒绝）。
    grace: Duration,
}

impl std::fmt::Debug for WorkspaceLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkspaceLease")
            .field("lock_path", &self.lock_path)
            .field("holder", &self.holder)
            .field("grace_secs", &self.grace.as_secs())
            .finish()
    }
}

impl WorkspaceLease {
    /// 构造（agent_factory / exec_worker / 测试共用）。`workspace` 是工作
    /// 区根目录。
    pub fn new(workspace: &std::path::Path, holder: &str) -> Self {
        let dir = workspace.join("logs");
        Self {
            lock_path: dir.join(LOCK_FILE_NAME),
            holder_path: dir.join(format!("{LOCK_FILE_NAME}.holder.json")),
            holder: holder.to_string(),
            grace: LEASE_DEFAULT_GRACE,
        }
    }

    /// 测试/特殊形态：收窄宽限（默认 30s 不适合单测）。
    pub fn with_grace(mut self, grace: Duration) -> Self {
        self.grace = grace;
        self
    }

    /// 获取租约。成功 = 返回 [`LeaseAcquisition`]（持有至 Drop；Drop 即
    /// 发释放信号，锁线程落 guard + 清 sidecar 后退出）。失败（宽限超时
    /// /基础设施故障）= `Err(io::Error)`：
    /// - `WouldBlock` + 中文文本（含 sidecar 里的当前持有者名）= 争用超
    ///   时（调用方当「诚实拒绝」处理）；
    /// - 其他 kind = 锁文件打开失败 / 线程故障（调用方按基础设施故障
    ///   自行决定降级语义——见 [`LeaseGuardTool`]）。
    pub async fn acquire(self: &Arc<Self>) -> io::Result<LeaseAcquisition> {
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<io::Result<()>>();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let lease = Arc::clone(self);
        std::thread::Builder::new()
            .name(format!("ws-lease:{}", lease.holder))
            .spawn(move || {
                // Err 在这里补发 ready（成功路径由 hold_lease 自己发 Ok）。
                if let Err(e) = lease.hold_lease(&release_rx, &ready_tx) {
                    let _ = ready_tx.send(Err(e));
                }
            })
            .map_err(|e| io::Error::other(format!("租约线程启动失败: {e}")))?;
        // 锁线程自带 grace 界；外层再兜 grace+5s（防 recv 通道意外悬挂）。
        // Err 路径给线程补发 release，防「线程已持锁却无人释放」悬挂。
        let wait = async {
            // 两层 `?` 剥掉 JoinError / RecvError 后，剩锁线程的**诚实判
            // 决**（`io::Result<()>`：Ok=获取成功 / Err=争用超时或故障），
            // 原样作为本 async 块的结果——不可 `let _` 吞掉（否则拒绝会
            // 被伪装成成功，2026-09-26 scratch 实证过的真 bug）。
            tokio::task::spawn_blocking(move || ready_rx.recv())
                .await
                .map_err(|e| io::Error::other(format!("租约等待任务失败: {e}")))?
                .map_err(|e| io::Error::other(format!("租约线程提前退出: {e}")))?
        };
        match tokio::time::timeout(self.grace + Duration::from_secs(5), wait).await {
            Ok(Ok(())) => Ok(LeaseAcquisition {
                release_tx: Some(release_tx),
                holder_path: self.holder_path.clone(),
                holder: self.holder.clone(),
            }),
            Ok(Err(e)) => {
                let _ = release_tx.send(());
                Err(e)
            }
            Err(_elapsed) => {
                let _ = release_tx.send(());
                Err(io::Error::other("租约等待超时（锁线程未按时应答）"))
            }
        }
    }

    /// 锁线程主体：try_write 轮询排队至 `grace` → 写 sidecar → 发 ready →
    /// 阻塞等释放信号 → 落 guard + 清**自己的** sidecar。guard 终生不出
    /// 本函数（不出借原则，见模块文档）。
    fn hold_lease(
        &self,
        release_rx: &std::sync::mpsc::Receiver<()>,
        ready_tx: &std::sync::mpsc::Sender<io::Result<()>>,
    ) -> io::Result<()> {
        let file = self.open_lock_file()?;
        let mut lock = RwLock::new(file);
        let deadline = Instant::now() + self.grace;
        let guard = loop {
            match lock.try_write() {
                Ok(g) => break g,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        let holder = read_holder_name(&self.holder_path)
                            .unwrap_or_else(|| "其他会话/进程".to_string());
                        return Err(io::Error::new(
                            io::ErrorKind::WouldBlock,
                            format!(
                                "工作区正被 {holder} 占用（等待 {}s 后拒绝；持有进程退出后锁自动释放）",
                                self.grace.as_secs()
                            ),
                        ));
                    }
                    std::thread::sleep(LEASE_POLL_INTERVAL);
                }
                Err(e) => return Err(e),
            }
        };
        self.write_holder();
        let _ = ready_tx.send(Ok(()));
        // 持有至释放信号（LeaseAcquisition::Drop 触发；进程死亡 = OS 释放）。
        let _ = release_rx.recv();
        drop(guard);
        remove_own_holder(&self.holder_path, &self.holder);
        Ok(())
    }

    /// 打开（必要时创建）锁载体文件（锁线程 / probe 共用）。
    fn open_lock_file(&self) -> io::Result<std::fs::File> {
        if let Some(parent) = self.lock_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&self.lock_path)
    }

    /// 持有者 sidecar 落盘（best-effort；sidecar 失败不影响锁的互斥性）。
    fn write_holder(&self) {
        let v = serde_json::json!({
            "holder": self.holder,
            "acquired_at": chrono::Local::now().to_rfc3339(),
        });
        if let Err(e) = std::fs::write(&self.holder_path, v.to_string()) {
            tracing::warn!(
                "[lease] holder sidecar 写入失败 {}: {}",
                self.holder_path.display(),
                e
            );
        }
    }

    /// WSAPI `system.lease_status` 探针（无副作用）：对给定 workspace 试
    /// 取锁立即释放，读 sidecar 汇报现状。`workspace = None`（未装配形态
    /// ——headless/测试 ctx）→ `supported: false`。
    ///
    /// 诚实注记：held 且无 sidecar = 旧形态持有者；free 且有 sidecar = 上
    /// 一持有者的陈旧记录（进程死亡 OS 释放锁，sidecar 无墓碑机制）。
    pub fn probe(workspace: Option<&str>) -> serde_json::Value {
        let Some(ws) = workspace else {
            return serde_json::json!({
                "supported": false,
                "held": false,
                "note": "workspace 未知（headless/未装配形态）",
            });
        };
        let lock_path = std::path::Path::new(ws).join("logs").join(LOCK_FILE_NAME);
        let holder_path = lock_path
            .parent()
            .map(|p| p.join(format!("{LOCK_FILE_NAME}.holder.json")));
        let mut held = false;
        let mut note = String::new();
        // F7（2026-09-26 复查）：探针只读化——锁文件不存在 = 从未有租约，
        // 直接诚实汇报返回，不创建 logs/ 与锁载体文件（状态查询不得有副
        // 作用；此前 create(true) 让首次探测凭空造出锁文件）。
        if !lock_path.exists() {
            return serde_json::json!({
                "supported": true,
                "held": false,
                "holder": serde_json::Value::Null,
                "acquired_at": serde_json::Value::Null,
                "lock_path": lock_path.display().to_string(),
                "note": "尚无锁文件（此工作区从未有租约记录）",
            });
        }
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
        {
            Ok(file) => {
                let mut lock = RwLock::new(file);
                match lock.try_write() {
                    Ok(g) => drop(g),
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => held = true,
                    Err(e) => note = format!("锁探测失败: {e}"),
                }
            }
            Err(e) => note = format!("锁文件打开失败: {e}"),
        }
        let holder = holder_path.as_deref().and_then(read_holder_json);
        if held && holder.is_none() {
            note = "持有者信息缺失（旧形态持有者或 sidecar 丢失）".to_string();
        }
        if !held && holder.is_some() {
            note = "sidecar 为上一持有者的陈旧记录（持有进程已退出，锁已由 OS 释放）".to_string();
        }
        serde_json::json!({
            "supported": true,
            "held": held,
            "holder": holder.as_ref().map(|h| h["holder"].clone()),
            "acquired_at": holder.as_ref().and_then(|h| h.get("acquired_at")).cloned(),
            "lock_path": lock_path.display().to_string(),
            "note": note,
        })
    }
}

/// 读 sidecar 的 holder 名（缺文件/解析失败 = None）。
fn read_holder_name(holder_path: &std::path::Path) -> Option<String> {
    read_holder_json(holder_path)?
        .get("holder")
        .and_then(|h| h.as_str())
        .map(String::from)
}

/// 读 sidecar 全量 JSON。
fn read_holder_json(holder_path: &std::path::Path) -> Option<serde_json::Value> {
    let data = std::fs::read_to_string(holder_path).ok()?;
    serde_json::from_str(&data).ok()
}

/// 清**自己的** sidecar（读-比对-删，防误删后继持有者的记录——理论上锁
/// 先行传递的时序不容许，但诚实防御）。
fn remove_own_holder(holder_path: &std::path::Path, holder: &str) {
    let Ok(data) = std::fs::read_to_string(holder_path) else {
        return;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) else {
        return;
    };
    if v.get("holder").and_then(|h| h.as_str()) == Some(holder) {
        let _ = std::fs::remove_file(holder_path);
    }
}

/// 租约持有期句柄（owned，`'static`——guard 留在锁线程，见模块文档）。
/// Drop = 发释放信号（锁线程落 guard + 清 sidecar）；持有进程整体死亡则
/// OS 直接释放文件锁，sidecar 残留由 probe 诚实标注为陈旧记录。
#[derive(Debug)]
pub struct LeaseAcquisition {
    release_tx: Option<std::sync::mpsc::Sender<()>>,
    #[allow(dead_code)]
    holder_path: PathBuf,
    #[allow(dead_code)]
    holder: String,
}

impl Drop for LeaseAcquisition {
    fn drop(&mut self) {
        if let Some(tx) = self.release_tx.take() {
            let _ = tx.send(());
        }
    }
}

/// WS9/P22：需要租约的写类工具清单（v1）。`exec` **不在账**——见模块
/// 文档「接线」小节的诚实边界（命令面包装拦不到效果面，不做假拦截）。
pub const LEASE_WRITE_TOOLS: [&str; 7] = [
    "write_file",
    "edit_file",
    "multiedit",
    "append_file",
    "create_dir",
    "delete_file",
    "delete_dir",
];

/// 写类工具的租约守卫委派包装（`SharedToolConfig.workspace_lease = Some`
/// 时 `register_shared_tools` 把 LEASE_WRITE_TOOLS 逐个换成此形态）。实现
/// agent 内部 `crate::r#loop::Tool` trait（与被包装工具同一 trait 对象）；
/// 协议面（description/parameters）与行为面（preview/preview_all/只读声
/// 明/凭据槽位/限额类别等）**全量透传 inner**——checkpoint 安全网（依赖
/// preview_all）、P20 冲突预检、安全 8 层、prompt cache 前缀全部不受包
/// 装影响；唯一被改写的是 execute：acquire → inner.execute → drop 释放。
pub struct LeaseGuardTool {
    inner: Arc<dyn Tool>,
    lease: Arc<WorkspaceLease>,
}

impl LeaseGuardTool {
    /// 包装（register_shared_tools / 测试共用）。
    pub fn new(inner: Arc<dyn Tool>, lease: Arc<WorkspaceLease>) -> Self {
        Self { inner, lease }
    }
}

#[async_trait::async_trait]
impl Tool for LeaseGuardTool {
    async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String> {
        // acquire → 执行 → drop 同帧（LeaseAcquisition 是 owned 句柄，
        // guard 在锁线程里，随执行帧结束 Drop 释放）。
        let _acq = match self.lease.acquire().await {
            Ok(acq) => acq,
            // 争用超时：诚实拒绝（错误文本已含持有者名）。
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                return Err(format!(
                    "{}（另一执行体正在写同一工作区；稍后重试，或等对方回合结束）",
                    e
                ));
            }
            // 基础设施故障（锁文件打不开/线程故障）：不阻断工具，降级为
            // 无租约直跑——租约是协调机制不是安全闸（安全 8 层照常），
            // 因基础设施故障拒绝写会让单点故障放大成全面停摆。诚实 WARN。
            Err(e) => {
                tracing::warn!("[lease] 租约获取失败（{}），工具降级无租约执行", e);
                return self.inner.execute(args, context).await;
            }
        };
        self.inner.execute(args, context).await
    }

    fn set_context(&self, channel: &str, chat_id: &str) {
        self.inner.set_context(channel, chat_id);
    }

    fn set_invocation_depth(&self, depth: usize) {
        self.inner.set_invocation_depth(depth);
    }

    fn description(&self) -> String {
        self.inner.description()
    }

    fn parameters(&self) -> serde_json::Value {
        self.inner.parameters()
    }

    fn preview(&self, args: &str) -> Option<FileChange> {
        self.inner.preview(args)
    }

    fn preview_all(&self, args: &str) -> Vec<FileChange> {
        self.inner.preview_all(args)
    }

    fn is_read_only(&self) -> bool {
        self.inner.is_read_only()
    }

    fn is_parallel_safe(&self) -> bool {
        self.inner.is_parallel_safe()
    }

    fn credential_arg_keys(&self) -> &[&str] {
        self.inner.credential_arg_keys()
    }

    fn limit_categories(&self) -> &[&str] {
        self.inner.limit_categories()
    }
}
