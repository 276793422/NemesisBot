//! External process channel (stdin/stdout pipe).
//!
//! Manages communication with external input/output executables.
//! Input EXE: reads stdout and sends to message bus.
//! Output EXE: receives messages via stdin.

use async_trait::async_trait;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tracing::{debug, error, info, warn};

use nemesis_types::channel::OutboundMessage;
use nemesis_types::error::{NemesisError, Result};

use crate::base::{BaseChannel, Channel};

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
    base: BaseChannel,
    config: ExternalConfig,
    running: Arc<AtomicBool>,
    /// Handle to the input process (for killing on stop).
    input_child: parking_lot::Mutex<Option<Child>>,
    /// Cancellation channel for the input read loop.
    cancel_tx: parking_lot::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

impl ExternalChannel {
    /// Creates a new `ExternalChannel`.
    pub fn new(config: ExternalConfig) -> Result<Self> {
        if config.input_exe.is_empty() || config.output_exe.is_empty() {
            return Err(NemesisError::Channel(
                "both input_exe and output_exe must be specified".to_string(),
            ));
        }

        Ok(Self {
            base: BaseChannel::new("external"),
            config,
            running: Arc::new(AtomicBool::new(false)),
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
    fn spawn_input_reader(&self) {
        let input_exe = self.config.input_exe.clone();
        let running = self.running.clone();

        let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
        *self.cancel_tx.lock() = Some(cancel_tx);

        tokio::spawn(async move {
            let mut child = match Command::new(&input_exe)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
            {
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
            let mut line = String::new();
            let mut cancel_rx = cancel_rx;

            loop {
                tokio::select! {
                    _ = &mut cancel_rx => {
                        info!("[ExternalChannel] input reader cancelled, killing process");
                        let _ = child.kill().await;
                        return;
                    }
                    result = reader.read_line(&mut line) => {
                        match result {
                            Ok(0) => {
                                info!("[ExternalChannel] input EXE closed stdout");
                                break;
                            }
                            Ok(_) => {
                                let trimmed = line.trim();
                                if !trimmed.is_empty() {
                                    debug!(line = %trimmed, "[ExternalChannel] received from input EXE");
                                }
                                line.clear();
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
            let mut child = match Command::new(&output_exe)
                .stdin(std::process::Stdio::piped())
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    error!(exe = %output_exe, error = %e, "[ExternalChannel] failed to spawn output EXE");
                    return;
                }
            };

            if let Some(ref mut stdin) = child.stdin {
                if let Err(e) = stdin.write_all(content.as_bytes()).await {
                    error!(error = %e, "[ExternalChannel] failed to write to output EXE stdin");
                }
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
