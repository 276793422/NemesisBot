//! External process channel (stdin/stdout pipe).
//!
//! Manages communication with external input/output executables.
//! Input EXE: reads stdout and sends to message bus.
//! Output EXE: receives messages via stdin.

use async_trait::async_trait;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

use nemesis_types::channel::{InboundMessage, OutboundMessage};
use nemesis_types::error::{NemesisError, Result};

use crate::base::{BaseChannel, Channel};

use nemesis_utils::line_cap::read_line_capped;

/// 单行字节帽（BUG-20261108-028）：裸 read_line 把整行无界积累进内存才
/// 返回——失控的 input 脚本（打印无换行的死循环输出）打爆宿主内存。
/// 消息行是聊天文本（远小于此），1MB 已远超任何合法行。
const MAX_EXTERNAL_LINE_BYTES: usize = 1024 * 1024;

/// 可取消的退避睡眠（BUG-20261128-032 重启循环用）：返回 false = 等待期间
/// 收到停机 cancel（stop 在途），调用方应立即退出不再重启。
async fn cancelable_sleep(cancel_rx: &mut tokio::sync::oneshot::Receiver<()>, secs: u64) -> bool {
    tokio::select! {
        _ = cancel_rx => false,
        _ = tokio::time::sleep(std::time::Duration::from_secs(secs)) => true,
    }
}

/// input EXE 意外死亡自动重启的退避上限（秒）。指数退避 1→2→4…封顶。
const INPUT_RESTART_BACKOFF_CAP_SECS: u64 = 30;
/// 进程存活超过该秒数后死亡视为「稳定期死亡」（连续快速死亡计数清零）。
const INPUT_STABLE_SECS: u64 = 60;
/// 连续快速死亡（spawn 后 < INPUT_STABLE_SECS 即死）达到该次数 → 放弃重启
/// （防 crash-loop 烧 CPU/日志），诚实翻转 running 旗标。
const INPUT_MAX_CONSECUTIVE_FAST_DEATHS: u32 = 5;
/// 死亡诊断日志携带的 stderr 尾部行数上限。
const INPUT_STDERR_TAIL_LINES: usize = 8;

/// External channel configuration.
#[derive(Debug, Clone)]
pub struct ExternalConfig {
    /// Input executable path.
    pub input_exe: String,
    /// Output executable path.
    pub output_exe: String,
    /// Chat ID for messages.
    pub chat_id: String,
    /// Targets to sync messages to.
    pub sync_to: Vec<String>,
    /// Allowed sender IDs.
    pub allow_from: Vec<String>,
    /// BUG-20261123-037：EXE 子进程 cwd 锚点（gateway 装配注入 workspace
    /// 根）。None = 继承进程 cwd（旧行为）——gateway 部署形态 cwd 不可预测
    /// （终端=启动目录 / 托盘自启=system32 / 任务计划=system32），用户配置
    /// 的相对路径 EXE / 脚本侧相对路径读写随部署形态漂移。
    pub working_dir: Option<std::path::PathBuf>,
}

/// External channel using stdin/stdout pipes.
pub struct ExternalChannel {
    /// Shared handle so the input reader task can access the allow-list and
    /// sync targets (mirrors Go's BaseChannel pointer).
    base: Arc<BaseChannel>,
    config: ExternalConfig,
    running: Arc<AtomicBool>,
    /// Message bus sender for publishing inbound messages from the input EXE.
    bus_sender: broadcast::Sender<InboundMessage>,
    /// Cancellation channel for the input read loop.
    cancel_tx: parking_lot::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

impl ExternalChannel {
    /// Creates a new `ExternalChannel`.
    ///
    /// `bus_sender` is used to publish inbound messages read from the input
    /// EXE's stdout (parity with Go's `NewExternalChannel(cfg, messageBus)`).
    pub fn new(
        config: ExternalConfig,
        bus_sender: broadcast::Sender<InboundMessage>,
    ) -> Result<Self> {
        if config.input_exe.is_empty() || config.output_exe.is_empty() {
            return Err(NemesisError::Channel(
                "both input_exe and output_exe must be specified".to_string(),
            ));
        }

        Ok(Self {
            base: Arc::new(BaseChannel::with_allow_list(
                "external",
                config.allow_from.clone(),
            )),
            config,
            running: Arc::new(AtomicBool::new(false)),
            bus_sender,
            cancel_tx: parking_lot::Mutex::new(None),
        })
    }

    /// Returns the input executable path.
    pub fn input_exe(&self) -> &str {
        &self.config.input_exe
    }

    /// Returns the output executable path.
    pub fn output_exe(&self) -> &str {
        &self.config.output_exe
    }

    /// Returns the chat ID.
    pub fn chat_id(&self) -> &str {
        &self.config.chat_id
    }

    /// Processes a line from input EXE's stdout.
    pub fn process_input_line(&self, line: &str) -> Option<(String, String, String)> {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }
        Some((
            self.config.chat_id.clone(),
            self.config.chat_id.clone(),
            line.to_string(),
        ))
    }

    /// Formats a message for output EXE's stdin.
    pub fn format_output(&self, content: &str) -> String {
        format!("{content}\n")
    }

    /// Spawns the input process and reads stdout in a background task.
    ///
    /// Each non-empty line is published to the message bus as an
    /// `InboundMessage` (parity with Go's `readInputEXEStdout` →
    /// `HandleMessage` + `SyncToTargets`).
    ///
    /// BUG-20261128-032：input EXE 是声明为长寿命的通道进程，其意外死亡
    /// （脚本崩溃 / 编码错误 / 被外部杀死——实证：Windows python 管道 stdout
    /// 默认 locale 编码，`print("你好")` 直接 UnicodeEncodeError 退出）曾被
    /// 当作正常关停处理（INFO「closed stdout」+ break，无重启、无告警、
    /// 无退出码诊断），通道 input 面静默死亡、后续消息全部丢失，而出站面
    /// 照常工作（半死状态无人知晓）。现修复为三段语义：
    /// ① 死亡定性——按退出码分级（非零=error 带退出码+stderr 尾部 / 零=
    ///   info 正常退出 / 进程存活但 stdout 关闭=warn 并 kill 收尸）；
    /// ② 非停机期死亡自动重启（指数退避 1→30s 封顶），进程稳定存活
    ///   （≥INPUT_STABLE_SECS）后死亡清零连续快速死亡计数；
    /// ③ 连续快速死亡达上限 → 放弃重启（防 crash-loop），error 告警 +
    ///   翻转 base running 旗标（manager status 上报诚实反映通道残废；
    ///   `running` AtomicBool 不动——出站 send 面语义独立，仍可用）。
    fn spawn_input_reader(&self) {
        let input_exe = self.config.input_exe.clone();
        let working_dir = self.config.working_dir.clone();
        let chat_id = self.config.chat_id.clone();
        let running = self.running.clone();
        let base = Arc::clone(&self.base);
        let bus_sender = self.bus_sender.clone();

        let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
        *self.cancel_tx.lock() = Some(cancel_tx);

        tokio::spawn(async move {
            let mut cancel_rx = cancel_rx;
            let mut backoff_secs: u64 = 1;
            let mut consecutive_fast_deaths: u32 = 0;

            loop {
                let spawned_at = std::time::Instant::now();
                // 通道 EXE 多为 console 程序；gateway 托盘/无控制台运行（release
                // windows 子系统）时压掉弹窗（stdout/stderr 走管道）。
                let mut cmd = Command::new(&input_exe);
                // cwd 锚点（BUG-20261123-037）：装配注入的 workspace 根，相对
                // 路径不随 gateway 部署形态漂移。None = 继承进程 cwd（旧行为）。
                if let Some(wd) = &working_dir {
                    cmd.current_dir(wd);
                }
                cmd.stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped());
                #[cfg(target_os = "windows")]
                cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
                // env 窄版闸（家族闸纪律：三方进程 spawn 点显式接闸——
                // BUG-20261123-012 横向修齐）：用户脚本不继承 bot 内部命名
                // 空间变量（NEMESISBOT_VAULT_PASSPHRASE 主口令通道等）；
                // 脚本走 stdin/stdout 文本协议，不依赖 bot 内部 env。
                let _stripped_env =
                    nemesis_utils::env_sanitize::sanitize_tokio_command_narrow(&mut cmd);
                let mut child = match cmd.spawn() {
                    Ok(c) => c,
                    Err(e) => {
                        // BUG-20261128-032 同面：spawn 失败（exe 不存在/无执行
                        // 权）此前同样静默 return——通道 input 面无声缺席。按
                        // 快速死亡形态纳入退避重启，达上限后放弃并诚实翻旗标。
                        error!(exe = %input_exe, error = %e, "[ExternalChannel] failed to spawn input EXE");
                        if !running.load(Ordering::SeqCst) {
                            return;
                        }
                        consecutive_fast_deaths += 1;
                        if consecutive_fast_deaths >= INPUT_MAX_CONSECUTIVE_FAST_DEATHS {
                            error!(
                                exe = %input_exe,
                                attempts = consecutive_fast_deaths,
                                "[ExternalChannel] input EXE repeatedly failed to spawn, giving up restarts (input face dead)"
                            );
                            base.set_running(false);
                            return;
                        }
                        if !cancelable_sleep(&mut cancel_rx, backoff_secs).await {
                            return;
                        }
                        backoff_secs = (backoff_secs * 2).min(INPUT_RESTART_BACKOFF_CAP_SECS);
                        continue;
                    }
                };

                let stdout = match child.stdout.take() {
                    Some(s) => s,
                    None => {
                        error!("[ExternalChannel] input EXE has no stdout");
                        return;
                    }
                };

                // stderr 排水（BUG-20261123-002 同族）：input EXE 是长寿命通道
                // 进程，piped 不排水 = 第三方 EXE 话痨 stderr 写满管道缓冲后整体
                // 写阻塞——stdout 读循环再也收不到行，通道静默瘫痪。有帽行读落
                // debug 日志；EOF/取消 kill 后随进程退出自然收尾（句柄 detach）。
                // BUG-20261128-032：尾部若干行同时进环形缓冲，死亡诊断日志携带
                // （本 BUG 实证：python UnicodeEncodeError traceback 被排水任务
                // 吃成 debug 级，死亡根因不可见）。
                let stderr_tail: Arc<parking_lot::Mutex<Vec<String>>> =
                    Arc::new(parking_lot::Mutex::new(Vec::new()));
                if let Some(stderr) = child.stderr.take() {
                    let tail = Arc::clone(&stderr_tail);
                    tokio::spawn(async move {
                        let mut reader = BufReader::new(stderr);
                        loop {
                            match read_line_capped(&mut reader, MAX_EXTERNAL_LINE_BYTES).await {
                                Ok(None) | Err(_) => break,
                                Ok(Some((raw, truncated))) => {
                                    let line = String::from_utf8_lossy(&raw).trim_end().to_string();
                                    let mut t = tail.lock();
                                    if t.len() >= INPUT_STDERR_TAIL_LINES {
                                        t.remove(0);
                                    }
                                    t.push(line.clone());
                                    tracing::debug!(
                                        truncated,
                                        line = %line,
                                        "[ExternalChannel] input EXE stderr"
                                    );
                                }
                            }
                        }
                    });
                }

                let mut reader = BufReader::new(stdout);

                /// 读循环的死亡原因（跨 select 退出路径定性用）。
                enum Death {
                    Cancelled,
                    Eof,
                    ReadError(String),
                }

                let death = loop {
                    tokio::select! {
                        _ = &mut cancel_rx => break Death::Cancelled,
                        result = read_line_capped(&mut reader, MAX_EXTERNAL_LINE_BYTES) => {
                            match result {
                                Ok(None) => break Death::Eof,
                                Ok(Some((raw, truncated))) => {
                                    // 带帽读（BUG-20261108-028）：超长毒丸行丢弃跳过
                                    //（游标已对齐下一行行首），reader 任务保活。
                                    if truncated {
                                        warn!(
                                            cap = MAX_EXTERNAL_LINE_BYTES,
                                            "[ExternalChannel] oversized line dropped"
                                        );
                                        continue;
                                    }
                                    let line = String::from_utf8_lossy(&raw).into_owned();
                                    let trimmed = line.trim();
                                    if !trimmed.is_empty() {
                                        debug!(line = %trimmed, "[ExternalChannel] received from input EXE");

                                        // Allow-list check (mirrors Go HandleMessage → IsAllowed)
                                        if base.is_allowed(&chat_id) {
                                            let inbound = InboundMessage::user_ingress(base.name().to_string(), chat_id.clone(), chat_id.clone(), trimmed.to_string())
                                            .with_session_key(chat_id.clone());
                                            if let Err(e) = bus_sender.send(inbound) {
                                                warn!(
                                                    error = %e,
                                                    "[ExternalChannel] failed to publish inbound message"
                                                );
                                            }

                                            // Sync to configured targets (mirrors Go SyncToTargets)
                                            base.sync_to_targets(trimmed).await;
                                        } else {
                                            warn!(
                                                chat_id = %chat_id,
                                                "[ExternalChannel] message blocked by allow-list"
                                            );
                                        }
                                    }
                                }
                                Err(e) => break Death::ReadError(e.to_string()),
                            }
                        }
                    }
                };

                // 停机路径：stop() 先翻 running=false 再发 cancel——收到 cancel
                // 即停机意愿，kill 收尸后直接退出（不重启）。
                if let Death::Cancelled = death {
                    info!("[ExternalChannel] input reader cancelled, killing process");
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    return;
                }

                // 死亡定性（BUG-20261128-032 ①）：按退出码分级（单次 try_wait
                // 同时产出描述与分类——tokio Child 状态虽幂等，分类判据与描述
                // 同源产出避免双查漂移），stderr 尾部随诊断日志带出。
                let stderr_tail_dump = {
                    let t = stderr_tail.lock();
                    if t.is_empty() {
                        String::new()
                    } else {
                        format!("；stderr 尾部: {}", t.join(" | "))
                    }
                };
                // 读循环 IO 错误描述并入死亡诊断（读失败时分类判据只能给出
                // 退出码/stdout 关闭，IO 根因只在这里可见）。
                let io_err_note = match &death {
                    Death::ReadError(e) => format!("；stdout 读循环 IO 错误: {e}"),
                    _ => String::new(),
                };
                let stderr_tail_dump = format!("{stderr_tail_dump}{io_err_note}");
                enum DeathClass {
                    /// 非零退出码：脚本崩溃/异常退出（error 级）。
                    Abnormal(String),
                    /// 进程存活但关了自己的 stdout：协议面死（warn + kill 收尸）。
                    AliveStdoutClosed(String),
                    /// 退出码 0 / 信号终止 / 探测失败：info 级正常关停语义。
                    Normal(String),
                }
                let class = match child.try_wait() {
                    Ok(Some(status)) => match status.code() {
                        Some(0) => {
                            DeathClass::Normal(format!("exit code 0（正常退出）{stderr_tail_dump}"))
                        }
                        Some(code) => DeathClass::Abnormal(format!(
                            "exit code {code}（异常退出）{stderr_tail_dump}"
                        )),
                        None => DeathClass::Normal(format!("被信号终止{stderr_tail_dump}")),
                    },
                    Ok(None) => {
                        // 进程活着但关了自己的 stdout——协议面已死，留着进程
                        // 无意义（孤儿占资源），kill 收尸。
                        let _ = child.kill().await;
                        let _ = child.wait().await;
                        DeathClass::AliveStdoutClosed(format!(
                            "进程仍存活但关闭了 stdout（已收尸）{stderr_tail_dump}"
                        ))
                    }
                    Err(e) => DeathClass::Normal(format!("wait 探测失败: {e}")),
                };
                match class {
                    DeathClass::Abnormal(detail) => {
                        error!(
                            exe = %input_exe,
                            detail = %detail,
                            "[ExternalChannel] input EXE died abnormally"
                        );
                    }
                    DeathClass::AliveStdoutClosed(detail) => {
                        warn!(
                            exe = %input_exe,
                            detail = %detail,
                            "[ExternalChannel] input EXE closed stdout while still running"
                        );
                    }
                    DeathClass::Normal(detail) => {
                        info!(
                            exe = %input_exe,
                            detail = %detail,
                            "[ExternalChannel] input EXE closed stdout"
                        );
                    }
                }

                // 停机期间的死亡 = 正常关停链路（stop 先翻旗标再 cancel，此处
                // 读到 false 说明 stop 在途），不重启。
                if !running.load(Ordering::SeqCst) {
                    return;
                }

                // 重启决策（BUG-20261128-032 ②③）：稳定存活后死亡清零计数
                // 并重置退避；连续快速死亡达上限放弃（防 crash-loop）。
                let lived_secs = spawned_at.elapsed().as_secs();
                if lived_secs >= INPUT_STABLE_SECS {
                    consecutive_fast_deaths = 0;
                    backoff_secs = 1;
                } else {
                    consecutive_fast_deaths += 1;
                    if consecutive_fast_deaths >= INPUT_MAX_CONSECUTIVE_FAST_DEATHS {
                        error!(
                            exe = %input_exe,
                            attempts = consecutive_fast_deaths,
                            "[ExternalChannel] input EXE died quickly {} times in a row, giving up restarts (input face dead)",
                            consecutive_fast_deaths
                        );
                        base.set_running(false);
                        return;
                    }
                }
                warn!(
                    exe = %input_exe,
                    backoff_secs,
                    "[ExternalChannel] restarting input EXE after death"
                );
                if !cancelable_sleep(&mut cancel_rx, backoff_secs).await {
                    return;
                }
                backoff_secs = (backoff_secs * 2).min(INPUT_RESTART_BACKOFF_CAP_SECS);
            }
        });
    }
}

#[async_trait]
impl Channel for ExternalChannel {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn is_running(&self) -> bool {
        self.base.is_running()
    }

    // BUG-20261112-118：trait 默认 add_sync_target 是 Ok(()) 静默存根——
    // 模板默认 `external.sync_to=["web"]` 在本通道上通道级 map 恒空，
    // 读循环的入站镜像（sync_to_targets）永远空转。委托 BaseChannel 固有
    // 实现接通（出站面已由 manager dispatch_outbound 集中触发兜住，通道
    // 级 map 只服务入站镜像扇出）。
    fn add_sync_target(&self, name: &str, channel: Arc<dyn Channel>) -> Result<()> {
        self.base.add_sync_target(name, channel)
    }

    fn remove_sync_target(&self, name: &str) {
        self.base.remove_sync_target(name)
    }

    async fn start(&self) -> Result<()> {
        // BUG-20261125-019（BUG-20261121-050 同构家族横向扫漏网补齐）：
        // 幂等双启守卫 + base running 旗标接线——无守卫的重复 start 会
        // spawn 多个并行 receive loop 每条消息 N 倍喂 agent；trait
        // is_running()（读 base 旗标）恒 false 使 manager 运行中守卫
        // （start_all / any_channel_running / status 上报三消费点）失真。
        if self.base.is_running() {
            info!("[ExternalChannel] already running, skipping duplicate start");
            return Ok(());
        }
        info!(
            input_exe = %self.config.input_exe,
            output_exe = %self.config.output_exe,
            chat_id = %self.config.chat_id,
            "[ExternalChannel] starting external channel"
        );
        self.running.store(true, Ordering::SeqCst);
        self.base.set_running(true);
        self.base.set_enabled(true);

        // Spawn the input reader
        self.spawn_input_reader();

        info!("[ExternalChannel] channel started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("[ExternalChannel] stopping external channel");
        // 顺序纪律（重启循环的停机判定依赖）：先翻 running 再发 cancel——
        // 重启循环在死亡/退避路径上读 running=false 即识别停机意愿。
        self.running.store(false, Ordering::SeqCst);
        // BUG-20261125-019：对称清 base 旗标（契约面见 start 内注释）。
        self.base.set_running(false);
        self.base.set_enabled(false);

        // Cancel the input reader（重启循环在任意 select 点收到后 kill 当前
        // 子进程并退出；input_child 字段已删——spawn 后 child 生命周期全程
        // 在 reader task 内，字段从未被写入过，stop 里的 kill 死路径由
        // cancel 臂承担）。
        if let Some(tx) = self.cancel_tx.lock().take() {
            let _ = tx.send(());
        }

        info!("[ExternalChannel] channel stopped");
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        if !self.running.load(Ordering::SeqCst) {
            return Err(NemesisError::Channel(
                "external channel not running".to_string(),
            ));
        }

        if msg.chat_id != self.config.chat_id {
            return Err(NemesisError::Channel(format!(
                "invalid chat ID: {} (expected: {})",
                msg.chat_id, self.config.chat_id
            )));
        }

        self.base.record_sent();
        debug!(content = %msg.content, "[ExternalChannel] sending to output EXE");

        // Spawn the output process and write to stdin
        let output_exe = self.config.output_exe.clone();
        let working_dir = self.config.working_dir.clone();
        let content = self.format_output(&msg.content);

        tokio::spawn(async move {
            // 同 input EXE：压掉 console 子进程弹窗（stdin 走管道）。
            let mut cmd = Command::new(&output_exe);
            // cwd 锚点（BUG-20261123-037）：同 input EXE——装配注入的
            // workspace 根，相对路径不随 gateway 部署形态漂移。
            // None = 继承进程 cwd（旧行为）。
            if let Some(wd) = &working_dir {
                cmd.current_dir(wd);
            }
            cmd.stdin(std::process::Stdio::piped());
            #[cfg(target_os = "windows")]
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
            // env 窄版闸（家族闸纪律——BUG-20261123-012 横向修齐，
            // 同 input EXE：脚本不继承 bot 内部命名空间变量）。
            let _stripped_env =
                nemesis_utils::env_sanitize::sanitize_tokio_command_narrow(&mut cmd);
            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    error!(exe = %output_exe, error = %e, "[ExternalChannel] failed to spawn output EXE");
                    return;
                }
            };

            if let Some(ref mut stdin) = child.stdin
                && let Err(e) = stdin.write_all(content.as_bytes()).await
            {
                error!(error = %e, "[ExternalChannel] failed to write to output EXE stdin");
            }

            match child.wait().await {
                Ok(status) => {
                    debug!(status = %status, "[ExternalChannel] output EXE exited");
                }
                Err(e) => {
                    warn!(error = %e, "[ExternalChannel] failed to wait for output EXE");
                }
            }
        });

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
