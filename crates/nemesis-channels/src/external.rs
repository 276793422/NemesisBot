//! External process channel (stdin/stdout pipe).
//!
//! Manages communication with external input/output executables.
//! Input EXE: reads stdout and sends to message bus.
//! Output EXE: receives messages via stdin.

use async_trait::async_trait;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
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
    /// Handle to the input process (for killing on stop).
    input_child: parking_lot::Mutex<Option<Child>>,
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
            input_child: parking_lot::Mutex::new(None),
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
    fn spawn_input_reader(&self) {
        let input_exe = self.config.input_exe.clone();
        let chat_id = self.config.chat_id.clone();
        let running = self.running.clone();
        let base = Arc::clone(&self.base);
        let bus_sender = self.bus_sender.clone();

        let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
        *self.cancel_tx.lock() = Some(cancel_tx);

        tokio::spawn(async move {
            // 通道 EXE 多为 console 程序；gateway 托盘/无控制台运行（release
            // windows 子系统）时压掉弹窗（stdout/stderr 走管道）。
            let mut cmd = Command::new(&input_exe);
            cmd.stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
            #[cfg(target_os = "windows")]
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    error!(exe = %input_exe, error = %e, "[ExternalChannel] failed to spawn input EXE");
                    return;
                }
            };

            let stdout = match child.stdout.take() {
                Some(s) => s,
                None => {
                    error!("[ExternalChannel] input EXE has no stdout");
                    return;
                }
            };

            let mut reader = BufReader::new(stdout);
            let mut cancel_rx = cancel_rx;

            loop {
                tokio::select! {
                    _ = &mut cancel_rx => {
                        info!("[ExternalChannel] input reader cancelled, killing process");
                        let _ = child.kill().await;
                        return;
                    }
                    result = read_line_capped(&mut reader, MAX_EXTERNAL_LINE_BYTES) => {
                        match result {
                            Ok(None) => {
                                info!("[ExternalChannel] input EXE closed stdout");
                                break;
                            }
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
                                        let inbound = InboundMessage {
                                            channel: base.name().to_string(),
                                            sender_id: chat_id.clone(),
                                            chat_id: chat_id.clone(),
                                            content: trimmed.to_string(),
                                            media: Vec::new(),
                                            session_key: chat_id.clone(),
                                            correlation_id: String::new(),
                                            metadata: std::collections::HashMap::new(),
                                            voice_playback: None,
                                        };
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
                            Err(e) => {
                                error!(error = %e, "[ExternalChannel] error reading from input EXE");
                                break;
                            }
                        }
                    }
                }
            }

            if !running.load(Ordering::SeqCst) {
                let _ = child.kill().await;
            }
            let _ = child.wait().await;
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
        info!(
            input_exe = %self.config.input_exe,
            output_exe = %self.config.output_exe,
            chat_id = %self.config.chat_id,
            "[ExternalChannel] starting external channel"
        );
        self.running.store(true, Ordering::SeqCst);
        self.base.set_enabled(true);

        // Spawn the input reader
        self.spawn_input_reader();

        info!("[ExternalChannel] channel started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("[ExternalChannel] stopping external channel");
        self.running.store(false, Ordering::SeqCst);
        self.base.set_enabled(false);

        // Cancel the input reader
        if let Some(tx) = self.cancel_tx.lock().take() {
            let _ = tx.send(());
        }

        // Kill the input process (take out of mutex to avoid holding lock across await)
        let mut child_opt = self.input_child.lock().take();
        if let Some(ref mut child) = child_opt {
            let _ = child.kill().await;
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
        let content = self.format_output(&msg.content);

        tokio::spawn(async move {
            // 同 input EXE：压掉 console 子进程弹窗（stdin 走管道）。
            let mut cmd = Command::new(&output_exe);
            cmd.stdin(std::process::Stdio::piped());
            #[cfg(target_os = "windows")]
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
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
