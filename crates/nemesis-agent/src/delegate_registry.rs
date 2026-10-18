//! A3 委派持续操控（2026-10-18 差距修正计划批 1）— delegate 四工具。
//!
//! 把外部 CLI 编码代理（claude / codex）从「spawn-wait 一次性委派」升级为
//! 「注册-操控」：`delegate_start` 起一个**带 stdin 活管道**的常驻子代理，
//! `delegate_send` 中途注入指令（steer），`delegate_output` 协议感知增量读，
//! `delegate_stop` 随时停止（优雅关 stdin + 树杀兜底）；`resume_session_id`
//! （claude）= 会话恢复。既有一次性 `claude_code`/`codex_delegate` 工具不动
//! （新增不改写）。
//!
//! 结构（对 B4 后台注册表的扩展面，`background_registry` 复用件逐字节同源）：
//! - 输出缓冲/管道排干/监督收尸/树杀/工作区围栏复用 B4 的
//!   `OutputBuffer`/`spawn_pump`/`supervise`/`tree_kill`/`resolve_cwd`——
//!   delegate 条目 = 「B4 条目 + stdin 写口 + 协议抽取 + 墙钟上限」，
//!   offset 分页坐标语义与 `background_output` 一致；
//! - 协议 adapter 是**纯函数面**（[`DelegateProtocolKind`]）：构造 spawn
//!   参数、编码 stdin 帧、从 stdout NDJSON 行抽取 assistant 文本与
//!   session_id——每个 CLI 一个实现，版本漂移只改一处；
//! - estop 联动：manager 持 [`crate::estop::EstopState`] 的弱引用 watcher
//!   （首个 delegate 启动时 lazy spawn；manager 随 loop 重建销毁后 watcher
//!   自行退出，不泄漏）——急停触发 = 全部 delegate 关 stdin + 树杀，与 PTY
//!   终端同语义；estop 已触发时 `delegate_start` 直接拒绝。
//!
//! 安全面（不旁路）：
//! - `delegate_start` = ProcessExec 档（起进程；nemesis-security `types.rs`
//!   静态分类表显式映射，对齐 background_start）；`delegate_send`/
//!   `delegate_output`/`delegate_stop` = Internal 档（操控本注册表内的自有
//!   子代理，对齐 background_output/kill 例外语义）；
//! - `delegate_send` 的 steer 文本随 args 过注入检测层（Layer 1 扫工具入参
//!   ——dispatch 瀑布自动覆盖，无需工具内重复扫描）；
//! - 凭据继承走显式白名单：`env_allowlist: [KEY_NAMES]` → 运行期从
//!   workspace vault 逐名解密注入子进程 env（白名单显式，不全量 dump；
//!   vault 缺席/锁定/别名缺失 → 诚实报错，不静默空注）。
//!
//! 运行闸：config `agents.delegate.enabled`（默认关，U13 claude_code/
//! codex_delegate 同款 opt-in）+ CLI 在场探测（claude/codex 至少一个在
//! PATH，都没有则四工具不注册）。墙钟上限 `agents.delegate.max_lifetime_secs`
//! （默认 3600）= 子代理最长寿命，到点树杀并在输出流留诚实注记。
//!
//! 协议版本边界（诚实声明）：
//! - claude adapter 实测对齐（`claude -p --input-format stream-json
//!   --output-format stream-json --verbose`，本机已装 claude CLI）；
//! - codex app-server（JSON-RPC over stdio：initialize → newConversation →
//!   sendUserMessage / interruptConversation）按上游公开协议形态实现 +
//!   canned-line 单测钉住解析逻辑；**本机无 codex CLI，真机冒烟未跑**——
//!   接入前先 delegate_start 冒烟一次，协议漂移只改 adapter 纯函数面。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

use async_trait::async_trait;
use parking_lot::{Mutex as SyncMutex, RwLock};
use serde_json::json;
use tokio::sync::Mutex;

use crate::background_registry::{
    DEFAULT_MAX_OUTPUT_BYTES, JobStatus, OutputBuffer, resolve_cwd, spawn_pump, supervise,
};
use crate::context::RequestContext;
use crate::estop::EstopState;
use crate::r#loop::Tool;

/// 并发 delegate 上限（防 LLM 疯狂起子代理；满员先淘汰最旧已完成条目，
/// 全在跑才诚实拒绝）。
pub const MAX_DELEGATES: usize = 8;

/// codex app-server JSON-RPC 握手超时（起进程 + initialize +
/// newConversation 两个往返；15s 覆盖冷启动）。
const CODEX_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);

/// stop 后等子进程退出的上限（超过如实报 still terminating）。
const STOP_WAIT: Duration = Duration::from_secs(6);

// ---------------------------------------------------------------------------
// 协议 adapter（纯函数面）
// ---------------------------------------------------------------------------

/// 委派子代理的 wire 协议。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DelegateProtocolKind {
    /// `claude -p --input-format stream-json --output-format stream-json
    /// --verbose`——stdin 持续注入 user 帧 = steer；`--resume <sid>` = 恢复；
    /// EOF on stdin = 优雅收尾。
    ClaudeStreamJson,
    /// `codex app-server`——JSON-RPC over stdio：initialize →
    /// newConversation → sendUserMessage。
    CodexAppServer,
}

impl DelegateProtocolKind {
    /// 协议显示名（工具响应/日志用）。
    pub fn label(&self) -> &'static str {
        match self {
            Self::ClaudeStreamJson => "claude-stream-json",
            Self::CodexAppServer => "codex-app-server",
        }
    }

    /// 从 CLI 名（"claude"/"codex"）解析协议。
    pub fn from_cli(cli: &str) -> Option<Self> {
        match cli.trim().to_ascii_lowercase().as_str() {
            "claude" => Some(Self::ClaudeStreamJson),
            "codex" => Some(Self::CodexAppServer),
            _ => None,
        }
    }

    /// 构造 spawn 参数（CLI 路径之后的部分）。
    ///
    /// claude：`-p` print 模式 + stream-json 双向；`--verbose` 是 stream-json
    /// 输出的历史要求（缺了 CLI 直接报错）；resume/权限档按需追加。
    /// **不传** `--dangerously-skip-permissions`——子代理的权限面由 CLI 自己
    /// 的 permission_mode 管（print 模式下未授权操作诚实失败，不弹交互），
    /// 绝不自动放行。
    pub fn build_args(
        &self,
        resume_session_id: Option<&str>,
        permission_mode: &str,
    ) -> Result<Vec<String>, String> {
        match self {
            Self::ClaudeStreamJson => {
                let mut args = vec![
                    "-p".to_string(),
                    "--input-format".to_string(),
                    "stream-json".to_string(),
                    "--output-format".to_string(),
                    "stream-json".to_string(),
                    "--verbose".to_string(),
                ];
                if let Some(sid) = resume_session_id {
                    args.push("--resume".to_string());
                    args.push(sid.to_string());
                }
                if !permission_mode.is_empty() {
                    args.push("--permission-mode".to_string());
                    args.push(permission_mode.to_string());
                }
                Ok(args)
            }
            Self::CodexAppServer => {
                // codex 子代理 v1 不支持 resume（app-server 的会话恢复面与
                // CLI `codex resume` 语义不同，待真机验证后补）。
                if resume_session_id.is_some() {
                    return Err(
                        "codex adapter does not support resume_session_id yet (claude only)"
                            .to_string(),
                    );
                }
                Ok(vec!["app-server".to_string()])
            }
        }
    }

    /// 编码一条 steer 用户帧（写入 stdin，调用方补换行）。
    ///
    /// codex 的 `"id":0` 是占位——实际发送时 manager 覆写为递增 id。
    pub fn encode_user_frame(&self, text: &str, conversation_id: Option<&str>) -> String {
        match self {
            Self::ClaudeStreamJson => json!({
                "type": "user",
                "message": {
                    "role": "user",
                    "content": [{"type": "text", "text": text}]
                }
            })
            .to_string(),
            Self::CodexAppServer => json!({
                "jsonrpc": "2.0",
                "id": 0,
                "method": "sendUserMessage",
                "params": {
                    "conversationId": conversation_id.unwrap_or(""),
                    "items": [{"type": "text", "text": text}]
                }
            })
            .to_string(),
        }
    }

    /// 从一行 stdout NDJSON 抽取 assistant 文本（可多段）。
    ///
    /// claude：只取 `type == "assistant"` 事件里的 text 块——`result` 事件
    /// 的收尾文本与 assistant 事件内容重复（同一轮既流又结），重复抽取会让
    /// assistant 数组出现双份。codex：`codex/event/agent_message` 通知的
    /// `params.msg.message`。
    pub fn extract_assistant_texts(&self, line: &str) -> Vec<String> {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            return Vec::new();
        };
        match self {
            Self::ClaudeStreamJson => {
                if v["type"] == "assistant" {
                    v["message"]["content"]
                        .as_array()
                        .map(|items| {
                            items
                                .iter()
                                .filter(|i| i["type"] == "text")
                                .filter_map(|i| i["text"].as_str())
                                .map(|s| s.to_string())
                                .collect()
                        })
                        .unwrap_or_default()
                } else {
                    Vec::new()
                }
            }
            Self::CodexAppServer => {
                if v["method"] == "codex/event/agent_message" {
                    v["params"]["msg"]["message"]
                        .as_str()
                        .map(|s| vec![s.to_string()])
                        .unwrap_or_default()
                } else {
                    Vec::new()
                }
            }
        }
    }

    /// 从一行 stdout NDJSON 抽取 session_id（claude：事件顶层 `session_id`
    /// 字段；codex：None——conversation id 在握手响应里，由 manager 缓存）。
    pub fn extract_session_id(&self, line: &str) -> Option<String> {
        let v: serde_json::Value = serde_json::from_str(line).ok()?;
        match self {
            Self::ClaudeStreamJson => v["session_id"].as_str().map(|s| s.to_string()),
            Self::CodexAppServer => None,
        }
    }
}

// ---------------------------------------------------------------------------
// 注册表
// ---------------------------------------------------------------------------

/// 一条 delegate 条目的共享句柄束（Arc 存进注册表，clone 出 guard 后用）。
struct DelegateEntry {
    protocol: DelegateProtocolKind,
    cli_display: String,
    #[allow(dead_code)] // 诊断用元数据（日志/未来 delegate_info 面）
    cwd: String,
    #[allow(dead_code)] // 诊断用元数据
    started_at: SystemTime,
    #[allow(dead_code)] // 诊断用元数据
    max_lifetime: Duration,
    pid: u32,
    /// 进程句柄必须持有到收尸：kill_on_drop(true) 语义下提前 drop = 误杀。
    /// 读路径走 status 槽（supervise 填终态），此字段是保命引用不是数据。
    #[allow(dead_code)]
    child: Arc<Mutex<tokio::process::Child>>,
    /// stdin 写半端；None = 已优雅关闭（stop 过 / estop 路径）。
    stdin: Arc<Mutex<Option<tokio::process::ChildStdin>>>,
    output: Arc<Mutex<OutputBuffer>>,
    status: Arc<Mutex<Option<JobStatus>>>,
    kill_flag: Arc<AtomicBool>,
    /// claude session_id（输出流抽取，resume 用）/ codex conversation id
    /// （握手获得，sendUserMessage 用）。小临界区，同步锁（读快照不跨 await）。
    peer_session: SyncMutex<Option<String>>,
    /// codex JSON-RPC 请求 id 计数（1/2 被握手占用）。
    next_rpc_id: AtomicU64,
}

impl DelegateEntry {
    fn is_running_sync(&self) -> bool {
        self.status.try_lock().map(|s| s.is_none()).unwrap_or(true)
    }

    async fn is_running(&self) -> bool {
        self.status.lock().await.is_none()
    }
}

/// F7：启动在途预留（spawn 已发生、entry 未插表的窗口凭证）。estop
/// watcher / stop_all / 容量闸经它看见并处置「启动中」的子进程；cancel
/// = kill 旗 + start_kill（进程杀，树杀交 supervise 收尸）。
struct StartReservation {
    #[allow(dead_code)] // 诊断用元数据（错误信息/未来观测面）
    pid: u32,
    /// 与 entry/supervise 共享同一 Child Arc（start 里先建 Arc 再分流）。
    child: Arc<Mutex<tokio::process::Child>>,
    kill_flag: Arc<AtomicBool>,
}

impl StartReservation {
    fn cancel(&self) {
        self.kill_flag.store(true, Ordering::Relaxed);
        if let Ok(mut c) = self.child.try_lock() {
            // start_kill 是同步信号面（stop_all/Drop 的同步临界区可用）；
            // 已退出进程上调用无害（错误忽略）。
            let _ = c.start_kill();
        }
    }
}

/// delegate 注册表（每 AgentLoop 一个；loop 重建 → Drop → 全部树杀，
/// 与 background 注册表同生命周期语义）。
pub struct DelegateManager {
    entries: Mutex<HashMap<u64, Arc<DelegateEntry>>>,
    /// F7：启动在途预留（spawn 已发生、entry 未插表）。容量口径 =
    /// entries + starting；estop watcher / stop_all / Drop 同时排空本表，
    /// 封「estop 摸不到启动中子进程」与「并发 start 借未插表窗口破
    /// MAX_DELEGATES」两个窗口。短临界区同步锁（永不跨 await）。
    starting: SyncMutex<Vec<Arc<StartReservation>>>,
    next_id: AtomicU64,
    estop: RwLock<Option<Arc<EstopState>>>,
    /// estop watcher lazy-spawn 一次性守卫。
    estop_watcher_spawned: AtomicBool,
}

impl Drop for DelegateManager {
    fn drop(&mut self) {
        // 同步 best-effort 置旗；监督任务持全部 Arc 克隆，≤200ms 收尸。
        // runtime 关停路径由 Child 的 kill_on_drop(true) 兜底。
        if let Ok(entries) = self.entries.try_lock() {
            let n = entries.len();
            for e in entries.values() {
                e.kill_flag.store(true, Ordering::Relaxed);
            }
            if n > 0 {
                tracing::debug!(count = n, "[DelegateRegistry] dropped — kill flags raised");
            }
        }
        // F7：启动在途同款 best-effort 排空。
        if let Some(mut starting) = self.starting.try_lock() {
            for r in starting.drain(..) {
                r.cancel();
            }
        }
    }
}

impl DelegateManager {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            starting: SyncMutex::new(Vec::new()),
            next_id: AtomicU64::new(1),
            estop: RwLock::new(None),
            estop_watcher_spawned: AtomicBool::new(false),
        }
    }

    /// 挂 estop（工厂装配点调用；首个 delegate 启动时 lazy spawn watcher）。
    pub fn attach_estop(&self, estop: Arc<EstopState>) {
        *self.estop.write() = Some(estop);
    }

    /// 当前条目数（含已结束——条目保留到被淘汰/注册表销毁）。
    pub async fn delegate_count(&self) -> usize {
        self.entries.lock().await.len()
    }

    /// estop watcher：weak 引用订阅 estop；急停触发 = 全部 delegate 停止。
    /// manager drop 后 upgrade 失败 → watcher 自行退出（不随 loop 重建泄漏）。
    fn ensure_estop_watcher(self: &Arc<Self>) {
        if self.estop_watcher_spawned.swap(true, Ordering::Relaxed) {
            return;
        }
        let Some(estop) = self.estop.read().clone() else {
            // 未挂 estop（headless/测试形态）——重置守卫，允许后续 attach。
            self.estop_watcher_spawned.store(false, Ordering::Relaxed);
            return;
        };
        let weak = Arc::downgrade(self);
        let mut rx = estop.subscribe();
        tokio::spawn(async move {
            loop {
                if rx.changed().await.is_err() {
                    return; // estop channel 关闭（进程关停）
                }
                if !*rx.borrow() {
                    continue; // release：delegate 已被杀，不复活
                }
                match weak.upgrade() {
                    Some(m) => m.stop_all().await,
                    None => return, // manager 已销毁（loop 重建）→ watcher 退出
                }
            }
        });
    }

    /// 起一个 delegate 子代理，立刻返回 delegate_id（codex 例外：握手同步
    /// 完成——拿不到 conversation id 的 delegate 是废条目，起步即失败诚实
    /// 报错比返回一个发不出消息的 id 好）。
    ///
    /// `env` 是调用方（工具层）解析好的白名单凭据注入面；manager 不碰
    /// vault（凭据解密判断在装配层，子进程只拿解密后的值——「判断在
    /// gateway，子进程是哑执行」同哲学）。
    #[allow(clippy::too_many_arguments)]
    pub async fn start(
        self: &Arc<Self>,
        protocol: DelegateProtocolKind,
        cli_path: &str,
        prompt: Option<&str>,
        cwd: &str,
        resume_session_id: Option<&str>,
        permission_mode: &str,
        env: HashMap<String, String>,
        max_lifetime: Duration,
    ) -> Result<serde_json::Value, String> {
        // estop 已触发 → 拒绝（PTY 终端同款前置闸）。
        if self.estop.read().as_ref().is_some_and(|e| e.is_engaged()) {
            return Err("estop engaged: delegate start rejected (release estop first)".to_string());
        }
        self.ensure_estop_watcher();

        // 满员闸：口径 = 已插表 + 启动在途（F7：并发 start 不再能借
        // 「还没插表」窗口破 MAX_DELEGATES）。先淘汰最旧的已完成条目；
        // 仍满才拒绝。锁序恒定 entries → starting（starting 守卫永不跨
        // await、永不反序拿）。
        {
            let mut entries = self.entries.lock().await;
            let starting_n = self.starting.lock().len();
            if entries.len() + starting_n >= MAX_DELEGATES {
                let mut finished: Vec<u64> = entries
                    .iter()
                    .filter(|(_, e)| !e.is_running_sync())
                    .map(|(id, _)| *id)
                    .collect();
                finished.sort_unstable();
                for id in finished {
                    if entries.len() + starting_n < MAX_DELEGATES {
                        break;
                    }
                    entries.remove(&id);
                    tracing::debug!(
                        evicted = id,
                        "[DelegateRegistry] evicted finished delegate for capacity"
                    );
                }
                if entries.len() + starting_n >= MAX_DELEGATES {
                    return Err(format!(
                        "delegate limit reached ({MAX_DELEGATES} active/starting); \
                         stop one with delegate_stop first"
                    ));
                }
            }
        }

        let args = protocol.build_args(resume_session_id, permission_mode)?;
        let mut cmd = tokio::process::Command::new(cli_path);
        cmd.args(&args)
            .current_dir(cwd)
            .envs(&env)
            // 三条管道全开：stdin = steer 通道（本模块存在的理由），输出双
            // 管道并发排干（B2 教训：不排干 64KB 管道缓冲卡死子进程）。
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            // 条目/监督任务全销毁时强杀——孤儿子代理兜底。
            .kill_on_drop(true);
        // Windows 形态：子代理不弹控制台窗口（同 background CREATE_NO_WINDOW）。
        #[cfg(target_os = "windows")]
        cmd.creation_flags(0x0800_0000);
        // Unix 形态：子代理自成一进程组，树杀一锅端（同 background）。
        #[cfg(not(target_os = "windows"))]
        cmd.process_group(0);

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to start delegate CLI '{cli_path}': {e}"))?;
        let pid = child.id().expect("freshly spawned child always has a pid");

        let output = Arc::new(Mutex::new(OutputBuffer::new(DEFAULT_MAX_OUTPUT_BYTES)));
        let status: Arc<Mutex<Option<JobStatus>>> = Arc::new(Mutex::new(None));
        let kill_flag = Arc::new(AtomicBool::new(false));
        let stdin = Arc::new(Mutex::new(child.stdin.take()));

        spawn_pump(child.stdout.take(), output.clone());
        spawn_pump(child.stderr.take(), output.clone());

        let child = Arc::new(Mutex::new(child));
        tokio::spawn(supervise(
            pid,
            child.clone(),
            status.clone(),
            kill_flag.clone(),
        ));

        // F7：spawn 已发生 → 先落启动预留，再进握手/首帧/插表窗口。estop
        // 复检、stop_all、Drop、容量闸都经预留看见并处置「启动中」的子
        // 进程；所有失败路径经 helper 杀 + 摘（见 finish_start_failed）。
        let reservation = Arc::new(StartReservation {
            pid,
            child: child.clone(),
            kill_flag: kill_flag.clone(),
        });
        self.starting.lock().push(reservation.clone());

        // F7：estop 复检——入口闸到 spawn 之间（容量锁 await 点）触发的
        // 急停，estop watcher 摸不到未入表进程，由本复检兜住。
        if self.estop.read().as_ref().is_some_and(|e| e.is_engaged()) {
            self.finish_start_failed(&reservation);
            return Err(format!(
                "estop engaged: delegate start aborted (pid {pid} killed)"
            ));
        }

        // 墙钟上限 watchdog：到点置 kill 旗标（树杀交给 supervise）+ 在输出
        // 流留诚实注记。0 → Duration::MAX = 不限（不挂任务）。
        if max_lifetime != Duration::MAX {
            let status = status.clone();
            let kill_flag = kill_flag.clone();
            let output = output.clone();
            tokio::spawn(async move {
                tokio::time::sleep(max_lifetime).await;
                if status.lock().await.is_none() {
                    output
                        .lock()
                        .await
                        .append(b"\n[delegate_registry] lifetime budget exhausted - tree-killed\n");
                    kill_flag.store(true, Ordering::Relaxed);
                }
            });
        }

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let entry = Arc::new(DelegateEntry {
            protocol,
            cli_display: cli_display_name(cli_path),
            cwd: cwd.to_string(),
            started_at: SystemTime::now(),
            max_lifetime,
            pid,
            child,
            stdin,
            output,
            status,
            kill_flag,
            peer_session: SyncMutex::new(None),
            next_rpc_id: AtomicU64::new(10),
        });

        // 协议握手：codex 需 initialize → newConversation 拿 conversation id；
        // claude 无握手（首帧直接写）。握手失败 = 起步即坏，树杀回收后诚实
        // 报错（条目仍入册供诊断）。
        if protocol == DelegateProtocolKind::CodexAppServer {
            let mut read_mark: u64 = 0;
            match codex_handshake(&entry, &mut read_mark).await {
                Ok(convo) => *entry.peer_session.lock() = Some(convo),
                Err(e) => {
                    self.fail_start_with_entry(&reservation, &entry, id).await;
                    return Err(format!("codex handshake failed: {e}"));
                }
            }
        }
        if let Some(p) = prompt {
            // F7：不再裸 `?` 丢子进程——首帧失败（stdin 已断/进程已死）与
            // 握手失败同收尾形态：杀 + 入册供诊断 + 摘预留 + 诚实报错。
            if let Err(e) = write_user_frame(&entry, p).await {
                self.fail_start_with_entry(&reservation, &entry, id).await;
                return Err(format!("failed to write initial prompt: {e}"));
            }
        }

        // F7 终检（entries 锁内同步裁决）：estop 或容量被并发 start 打破
        // → 拒绝（杀子进程、不插表——策略性拒绝不产诊断条目）。座位守恒：
        // 每个在途 start 占一个「座位」（starting 或 entries 二其一），
        // 终检通过后摘预留与插表同锁完成，总数不超闸。
        {
            let mut entries = self.entries.lock().await;
            let estop_now = self.estop.read().as_ref().is_some_and(|e| e.is_engaged());
            let over_capacity = entries.len() + self.starting.lock().len() > MAX_DELEGATES;
            if estop_now || over_capacity {
                drop(entries);
                self.finish_start_failed(&reservation);
                return Err(if estop_now {
                    format!("estop engaged: delegate start aborted (pid {pid} killed)")
                } else {
                    format!(
                        "delegate limit reached ({MAX_DELEGATES} active/starting); \
                         concurrent starts raced past the gate"
                    )
                });
            }
            self.starting
                .lock()
                .retain(|r| !Arc::ptr_eq(r, &reservation));
            entries.insert(id, entry);
        }
        tracing::info!(
            delegate = id,
            pid,
            protocol = protocol.label(),
            "[DelegateRegistry] started"
        );

        Ok(json!({
            "delegate_id": id,
            "pid": pid,
            "cli": cli_display_name(cli_path),
            "protocol": protocol.label(),
            "cwd": cwd,
            "resumed": resume_session_id.is_some(),
            "note": "steer with delegate_send; read with delegate_output; stop with delegate_stop",
        }))
    }

    /// F7：启动失败统一收尾（estop 复检/终检拒绝）——经预留杀子进程 +
    /// 摘预留。封掉「supervise 任务持 Child Arc → kill_on_drop 永不触发
    /// → 孤儿进程」窗口。
    fn finish_start_failed(&self, reservation: &Arc<StartReservation>) {
        reservation.cancel();
        self.starting
            .lock()
            .retain(|r| !Arc::ptr_eq(r, reservation));
    }

    /// F7：起步即坏（握手/首帧失败）收尾——杀子进程 + 条目入册供诊断 +
    /// 摘预留。杀与既有语义同源（kill 旗 → supervise 树杀）；差异是失败
    /// 路径不再可能丢下无旗的活进程。
    async fn fail_start_with_entry(
        &self,
        reservation: &Arc<StartReservation>,
        entry: &Arc<DelegateEntry>,
        id: u64,
    ) {
        reservation.cancel();
        self.entries.lock().await.insert(id, entry.clone());
        self.starting
            .lock()
            .retain(|r| !Arc::ptr_eq(r, reservation));
    }

    /// steer：向子代理 stdin 写一条用户帧。
    pub async fn send(&self, id: u64, text: &str) -> Result<serde_json::Value, String> {
        let entry = self.running_entry(id).await?;
        write_user_frame(&entry, text).await?;
        Ok(json!({
            "delegate_id": id,
            "running": true,
            "note": "steer delivered to the delegate's stdin (the CLI applies it per its own protocol)",
        }))
    }

    /// 协议感知增量读：raw chunk（offset 分页，与 background_output 同坐标
    /// 语义）+ 全缓冲 assistant 文本抽取 + 已知 session_id。
    pub async fn output(&self, id: u64, offset: u64) -> Result<serde_json::Value, String> {
        let entry = self.entry(id).await?;
        let output = entry.output.lock().await;
        let (chunk, clamped) = output.read_chunk(offset);
        let next_offset = output.produced();
        // 全缓冲协议抽取（缓冲上限 256KB，逐行 parse 成本可忽略；guard 持有
        // 期间无 await）。
        let mut assistant: Vec<String> = Vec::new();
        let mut session_from_stream: Option<String> = None;
        if entry.protocol == DelegateProtocolKind::ClaudeStreamJson {
            let text = String::from_utf8_lossy(output.buf_bytes());
            for line in text.lines() {
                if line.trim().is_empty() {
                    continue;
                }
                assistant.extend(entry.protocol.extract_assistant_texts(line));
                if session_from_stream.is_none() {
                    session_from_stream = entry.protocol.extract_session_id(line);
                }
            }
        }
        drop(output);
        let session = session_from_stream.or_else(|| entry.peer_session.lock().clone());
        Ok(json!({
            "delegate_id": id,
            "cli": entry.cli_display,
            "pid": entry.pid,
            "protocol": entry.protocol.label(),
            "running": entry.is_running_sync(),
            "session_id": session,
            "assistant": assistant,
            "raw": {
                "chunk": String::from_utf8_lossy(&chunk),
                "bytes_returned": chunk.len(),
                "next_offset": next_offset,
                "clamped": clamped,
            },
        }))
    }

    /// 停止：优雅关 stdin（EOF = claude 完成当前轮后自退）+ 树杀兜底。
    ///
    /// claude print 模式没有 interrupt 帧（stream-json 输入只有 user 帧），
    /// 「现在就停」的唯一语义 = EOF + 树杀——硬停诚实文档化；codex 的
    /// interruptConversation 面待真机验证，v1 同样以树杀兜底，先保证
    /// 「stop 即停」不落空。
    pub async fn stop(&self, id: u64) -> Result<serde_json::Value, String> {
        let entry = self.entry(id).await?;
        // 先关 stdin：防树杀竞态窗口里又注入新帧。
        if let Some(mut s) = entry.stdin.lock().await.take() {
            use tokio::io::AsyncWriteExt;
            let _ = s.shutdown().await;
        }
        if entry.is_running().await {
            entry.kill_flag.store(true, Ordering::Relaxed);
            // 等收尸（supervise 200ms 轮询 + 树杀 2s 上限）。
            let deadline = Instant::now() + STOP_WAIT;
            while Instant::now() < deadline {
                if !entry.is_running().await {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        let st = entry.status.lock().await.clone();
        let mut out = json!({ "delegate_id": id, "stopped": true });
        if let Some(s) = &st {
            out["exit"] = json!({
                "exit_code": s.exit_code,
                "success": s.success,
                "killed": s.killed,
            });
        } else {
            out["note"] = json!("still terminating (kill signal sent; poll delegate_output)");
        }
        Ok(out)
    }

    /// 全部停止（estop / Drop 路径；不等待收尸）。
    pub async fn stop_all(&self) {
        // 先 clone 出条目列表再放开 map 锁（异步工作不持 map guard）。
        let items: Vec<Arc<DelegateEntry>> = self.entries.lock().await.values().cloned().collect();
        for e in items {
            if let Ok(mut guard) = e.stdin.try_lock()
                && let Some(mut s) = guard.take()
            {
                use tokio::io::AsyncWriteExt;
                let _ = s.shutdown().await;
            }
            if e.is_running_sync() {
                e.kill_flag.store(true, Ordering::Relaxed);
            }
        }
        // F7：启动在途一并排空（未入表进程——estop watcher / 关停路径
        // 由此覆盖 spawn～插表窗口）。
        let pending: Vec<Arc<StartReservation>> = {
            let mut s = self.starting.lock();
            std::mem::take(&mut *s)
        };
        for r in &pending {
            r.cancel();
        }
        tracing::info!("[DelegateRegistry] stop_all (estop or shutdown)");
    }

    async fn entry(&self, id: u64) -> Result<Arc<DelegateEntry>, String> {
        self.entries
            .lock()
            .await
            .get(&id)
            .cloned()
            .ok_or_else(|| format!("delegate {id} not found (never started or evicted)"))
    }

    async fn running_entry(&self, id: u64) -> Result<Arc<DelegateEntry>, String> {
        let e = self.entry(id).await?;
        if !e.is_running().await {
            return Err(format!("delegate {id} is not running"));
        }
        // stdin 已被 stop 关掉（进程还没退完）→ 同样不能写。
        if e.stdin.lock().await.is_none() {
            return Err(format!("delegate {id} stdin is closed (stop was called)"));
        }
        Ok(e)
    }
}

impl Default for DelegateManager {
    fn default() -> Self {
        Self::new()
    }
}

fn cli_display_name(cli_path: &str) -> String {
    std::path::Path::new(cli_path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| cli_path.to_string())
}

/// 向 delegate stdin 写一条协议用户帧 + 换行 + flush。
async fn write_user_frame(entry: &DelegateEntry, text: &str) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    let mut guard = entry.stdin.lock().await;
    let Some(stdin) = guard.as_mut() else {
        return Err("delegate stdin is closed (stopped?)".to_string());
    };
    let convo = entry.peer_session.lock().clone();
    let frame = entry.protocol.encode_user_frame(text, convo.as_deref());
    // codex 帧里的 id 占位 0 → 实际发送时覆写为递增 id。
    let frame = if entry.protocol == DelegateProtocolKind::CodexAppServer {
        rewrite_rpc_id(&frame, entry.next_rpc_id.fetch_add(1, Ordering::Relaxed))
    } else {
        frame
    };
    stdin
        .write_all(frame.as_bytes())
        .await
        .map_err(|e| format!("failed to write to delegate stdin: {e}"))?;
    stdin
        .write_all(b"\n")
        .await
        .map_err(|e| format!("failed to write to delegate stdin: {e}"))?;
    stdin
        .flush()
        .await
        .map_err(|e| format!("failed to flush delegate stdin: {e}"))?;
    Ok(())
}

/// 把 codex JSON-RPC 帧的 `"id":0` 覆写为实际 id（encode 占位 0；字符串级
/// 替换对 `"id":0` 精确形态安全——帧由本模块编码，结构可控，serde_json
/// compact 输出无空格）。
fn rewrite_rpc_id(frame: &str, id: u64) -> String {
    frame.replacen("\"id\":0", &format!("\"id\":{id}"), 1)
}

/// codex 握手：initialize → newConversation，从输出流等 JSON-RPC 响应。
async fn codex_handshake(entry: &DelegateEntry, read_mark: &mut u64) -> Result<String, String> {
    rpc_round(
        entry,
        read_mark,
        1,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientInfo":{"name":"nemesisbot","title":"NemesisBot","version":"0.1.0"}}}"#,
        CODEX_HANDSHAKE_TIMEOUT,
    )
    .await?;
    let resp = rpc_round(
        entry,
        read_mark,
        2,
        r#"{"jsonrpc":"2.0","id":2,"method":"newConversation","params":{}}"#,
        CODEX_HANDSHAKE_TIMEOUT,
    )
    .await?;
    // conversationId 形态按上游版本两种键名都收（camelCase 新 / snake 旧）。
    let v: serde_json::Value =
        serde_json::from_str(&resp).map_err(|e| format!("handshake response is not JSON: {e}"))?;
    v["result"]["conversationId"]
        .as_str()
        .or_else(|| v["result"]["conversation_id"].as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| {
            format!(
                "newConversation response missing conversationId: {}",
                truncate_diag(&resp)
            )
        })
}

/// 单个 JSON-RPC 往返：写请求帧 → 轮询输出缓冲等 `"id"` 匹配的响应行。
async fn rpc_round(
    entry: &DelegateEntry,
    read_mark: &mut u64,
    id: u64,
    request: &str,
    timeout: Duration,
) -> Result<String, String> {
    {
        use tokio::io::AsyncWriteExt;
        let mut guard = entry.stdin.lock().await;
        let Some(stdin) = guard.as_mut() else {
            return Err("delegate stdin closed during handshake".to_string());
        };
        stdin
            .write_all(request.as_bytes())
            .await
            .map_err(|e| format!("handshake write failed: {e}"))?;
        stdin
            .write_all(b"\n")
            .await
            .map_err(|e| format!("handshake write failed: {e}"))?;
        stdin
            .flush()
            .await
            .map_err(|e| format!("handshake flush failed: {e}"))?;
    }
    let deadline = Instant::now() + timeout;
    let needle = format!("\"id\":{id}");
    loop {
        if !entry.is_running().await {
            return Err("delegate exited during handshake".to_string());
        }
        let output = entry.output.lock().await;
        if output.produced() > *read_mark {
            let (chunk, _) = output.read_chunk(*read_mark);
            *read_mark += chunk.len() as u64;
            for line in String::from_utf8_lossy(&chunk).lines() {
                let line = line.trim();
                if line.is_empty() || !line.contains(&needle) {
                    continue;
                }
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                    let id_match = v["id"].as_u64() == Some(id);
                    if id_match && (v.get("result").is_some() || v.get("error").is_some()) {
                        if v.get("error").is_some() {
                            return Err(format!(
                                "handshake rpc error: {}",
                                truncate_diag(&v["error"].to_string())
                            ));
                        }
                        return Ok(line.to_string());
                    }
                }
            }
        }
        drop(output);
        if Instant::now() >= deadline {
            return Err(format!(
                "handshake timed out waiting for response id={id} ({timeout:?})"
            ));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn truncate_diag(s: &str) -> String {
    if s.len() <= 400 {
        s.to_string()
    } else {
        format!("{}…", &s[..400])
    }
}

// ---------------------------------------------------------------------------
// env 白名单解析（vault 凭据继承）
// ---------------------------------------------------------------------------

/// 解析 `env_allowlist`：逐名经 **nemesis-config 全局 vault 解析器槽位**
/// （`global_vault_resolver`）解析——与 provider api_key / channel token /
/// MCP env 的 `vault:<alias>` 是**同一条链**（nemesisbot 启动路径注入，
/// 含 argon2 `NEMESISBOT_VAULT_PASSPHRASE` 解锁臂与每次现开现解语义）。
/// 这里不直开 vault 文件：那会绕开注入点且复制解锁逻辑（单一真相源）。
///
/// 任一名失败（解析器未安装/vault 缺席/锁定/别名不存在）→ Err 逐项列明
/// （诚实失败，不静默空注——子代理拿不到预期凭据会在远端以更隐蔽的方式
/// 失败）。
#[cfg(feature = "security")]
pub fn resolve_env_allowlist(
    _workspace: &str,
    aliases: &[String],
) -> Result<HashMap<String, String>, String> {
    if aliases.is_empty() {
        return Ok(HashMap::new());
    }
    let resolver = nemesis_config::global_vault_resolver().ok_or_else(|| {
        format!(
            "env_allowlist requested {} secret(s) but no vault resolver is installed \
             (vault is only wired in full nemesisbot builds with the security feature)",
            aliases.len()
        )
    })?;
    let mut out = HashMap::new();
    let mut failed: Vec<(String, String)> = Vec::new();
    for name in aliases {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        match resolver(name) {
            Ok(v) => {
                out.insert(name.to_string(), v);
            }
            Err(e) => failed.push((name.to_string(), e)),
        }
    }
    if !failed.is_empty() {
        let detail = failed
            .iter()
            .map(|(n, e)| format!("{n}: {e}"))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(format!("env_allowlist resolution failed — {detail}"));
    }
    Ok(out)
}

#[cfg(not(feature = "security"))]
pub fn resolve_env_allowlist(
    _workspace: &str,
    aliases: &[String],
) -> Result<HashMap<String, String>, String> {
    if aliases.is_empty() {
        return Ok(HashMap::new());
    }
    Err("env_allowlist requires the security feature (vault unavailable)".to_string())
}

// ---------------------------------------------------------------------------
// 四工具
// ---------------------------------------------------------------------------

/// `delegate_start`：起一个可操控的 CLI 子代理。
pub struct DelegateStartTool {
    workspace: String,
    restrict: bool,
    max_lifetime: Duration,
    claude_path: Option<String>,
    codex_path: Option<String>,
    /// F8：config `agents.delegate.permission_mode` 缺省值——args 缺省/空
    /// 时落地（语义 = 默认值非强制：args 显式值恒优先，对齐字段文档
    /// 「passthrough」）。
    default_permission_mode: String,
    manager: Arc<DelegateManager>,
}

impl DelegateStartTool {
    pub fn new(
        workspace: &str,
        restrict: bool,
        max_lifetime_secs: u64,
        claude_path: Option<String>,
        codex_path: Option<String>,
        default_permission_mode: String,
        manager: Arc<DelegateManager>,
    ) -> Self {
        Self {
            workspace: workspace.to_string(),
            restrict,
            max_lifetime: if max_lifetime_secs == 0 {
                Duration::MAX
            } else {
                Duration::from_secs(max_lifetime_secs)
            },
            claude_path,
            codex_path,
            default_permission_mode,
            manager,
        }
    }
}

#[async_trait]
impl Tool for DelegateStartTool {
    fn description(&self) -> String {
        "Start a persistent, steerable external CLI coding agent (claude or codex) \
         as a background sub-delegate and return a delegate_id immediately. Unlike \
         one-shot delegation, this keeps a live stdin pipe: steer it mid-run with \
         delegate_send, read incremental output with delegate_output (includes \
         extracted assistant messages and session_id), stop with delegate_stop. \
         Pass resume_session_id (claude) to continue a previous session. Optional \
         env_allowlist injects named secrets from the workspace vault into the \
         child environment. Lifetime is capped; estop kills all delegates."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "cli": {"type": "string", "enum": ["claude", "codex"],
                        "description": "Which CLI agent to delegate to"},
                "prompt": {"type": "string",
                           "description": "Initial task text (optional; can steer later instead)"},
                "cwd": {"type": "string",
                        "description": "Working directory (default: workspace root; confined to workspace)"},
                "resume_session_id": {"type": "string",
                                      "description": "claude only: session id from a previous delegate_output to continue"},
                "permission_mode": {"type": "string",
                                    "description": "claude only: CLI permission mode passthrough (empty = configured default)"},
                "env_allowlist": {"type": "array", "items": {"type": "string"},
                                  "description": "Vault secret names to inject into the child env (explicit whitelist)"}
            },
            "required": ["cli"]
        })
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let v: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid arguments: {e}"))?;
        let cli = v["cli"]
            .as_str()
            .ok_or("missing required argument: cli")?
            .trim();
        let protocol = DelegateProtocolKind::from_cli(cli)
            .ok_or_else(|| format!("unknown cli '{cli}' (expected claude or codex)"))?;
        let cli_path = match protocol {
            DelegateProtocolKind::ClaudeStreamJson => self
                .claude_path
                .as_ref()
                .ok_or("claude CLI not found on PATH (install claude-code first)")?,
            DelegateProtocolKind::CodexAppServer => self
                .codex_path
                .as_ref()
                .ok_or("codex CLI not found on PATH (install codex first)")?,
        }
        .clone();

        let cwd = resolve_cwd(&self.workspace, self.restrict, v["cwd"].as_str())?;
        let prompt = v["prompt"].as_str().map(|s| s.to_string());
        let resume = v["resume_session_id"]
            .as_str()
            .map(|s| s.to_string())
            .filter(|s| !s.trim().is_empty());
        let mut permission_mode = v["permission_mode"]
            .as_str()
            .unwrap_or("")
            .trim()
            .to_string();
        // F8：args 缺省/空 → config 缺省值（默认值语义，显式 args 恒优先）。
        if permission_mode.is_empty() {
            permission_mode = self.default_permission_mode.clone();
        }
        let aliases: Vec<String> = v["env_allowlist"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let env = resolve_env_allowlist(&self.workspace, &aliases)?;

        let out = self
            .manager
            .start(
                protocol,
                &cli_path,
                prompt.as_deref(),
                &cwd,
                resume.as_deref(),
                &permission_mode,
                env,
                self.max_lifetime,
            )
            .await?;
        serde_json::to_string(&out).map_err(|e| format!("serialize: {e}"))
    }
}

/// `delegate_send`：中途注入指令（steer）。
pub struct DelegateSendTool {
    manager: Arc<DelegateManager>,
}

impl DelegateSendTool {
    pub fn new(manager: Arc<DelegateManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl Tool for DelegateSendTool {
    fn description(&self) -> String {
        "Send additional instructions to a running delegate (steer). The text is \
         written to the delegate's stdin in its own protocol, so the CLI agent \
         adjusts course without restarting. Use delegate_start's delegate_id."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "delegate_id": {"type": "integer", "description": "delegate_id from delegate_start"},
                "text": {"type": "string", "description": "Instruction text to inject (steer)"}
            },
            "required": ["delegate_id", "text"]
        })
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let v: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid arguments: {e}"))?;
        let id = v["delegate_id"]
            .as_u64()
            .ok_or("missing required argument: delegate_id")?;
        let text = v["text"]
            .as_str()
            .ok_or("missing required argument: text")?;
        if text.trim().is_empty() {
            return Err("text is empty".to_string());
        }
        let out = self.manager.send(id, text).await?;
        serde_json::to_string(&out).map_err(|e| format!("serialize: {e}"))
    }
}

/// `delegate_output`：协议感知增量读。
pub struct DelegateOutputTool {
    manager: Arc<DelegateManager>,
}

impl DelegateOutputTool {
    pub fn new(manager: Arc<DelegateManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl Tool for DelegateOutputTool {
    fn description(&self) -> String {
        "Read a delegate's incremental output. Returns the raw byte chunk since \
         `offset` (paginated; use raw.next_offset from the previous call) plus \
         the protocol-extracted assistant messages seen so far and the \
         delegate's session_id (needed for resume)."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "delegate_id": {"type": "integer", "description": "delegate_id from delegate_start"},
                "offset": {"type": "integer",
                           "description": "Byte offset from the previous call's raw.next_offset (default 0)"}
            },
            "required": ["delegate_id"]
        })
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let v: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid arguments: {e}"))?;
        let id = v["delegate_id"]
            .as_u64()
            .ok_or("missing required argument: delegate_id")?;
        let offset = v["offset"].as_u64().unwrap_or(0);
        let out = self.manager.output(id, offset).await?;
        serde_json::to_string(&out).map_err(|e| format!("serialize: {e}"))
    }
}

/// `delegate_stop`：停止（关 stdin + 树杀兜底）。
pub struct DelegateStopTool {
    manager: Arc<DelegateManager>,
}

impl DelegateStopTool {
    pub fn new(manager: Arc<DelegateManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl Tool for DelegateStopTool {
    fn description(&self) -> String {
        "Stop a delegate: closes its stdin (no new turns can start) and tree-kills \
         the process tree as a backstop. The exit status is returned when the \
         process terminates within the wait window."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "delegate_id": {"type": "integer", "description": "delegate_id from delegate_start"}
            },
            "required": ["delegate_id"]
        })
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let v: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid arguments: {e}"))?;
        let id = v["delegate_id"]
            .as_u64()
            .ok_or("missing required argument: delegate_id")?;
        let out = self.manager.stop(id).await?;
        serde_json::to_string(&out).map_err(|e| format!("serialize: {e}"))
    }
}

#[cfg(test)]
mod tests;
