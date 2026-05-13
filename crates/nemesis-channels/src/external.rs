//! External process channel (stdin/stdout pipe).
//!
//! Manages communication with external input/output executables.
//! Input EXE: reads stdout and sends to message bus.
//! Output EXE: receives messages via stdin.

use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
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
                    error!(exe = %input_exe, error = %e, "failed to spawn input EXE");
                    return;
                }
            };

            let stdout = match child.stdout.take() {
                Some(s) => s,
                None => {
                    error!("input EXE has no stdout");
                    return;
                }
            };

            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            let mut cancel_rx = cancel_rx;

            loop {
                tokio::select! {
                    _ = &mut cancel_rx => {
                        info!("input reader cancelled, killing process");
                        let _ = child.kill().await;
                        return;
                    }
                    result = reader.read_line(&mut line) => {
                        match result {
                            Ok(0) => {
                                info!("input EXE closed stdout");
                                break;
                            }
                            Ok(_) => {
                                let trimmed = line.trim();
                                if !trimmed.is_empty() {
                                    debug!(line = %trimmed, "received from input EXE");
                                }
                                line.clear();
                            }
                            Err(e) => {
                                error!(error = %e, "error reading from input EXE");
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

    async fn start(&self) -> Result<()> {
        info!(
            input_exe = %self.config.input_exe,
            output_exe = %self.config.output_exe,
            chat_id = %self.config.chat_id,
            "starting external channel"
        );
        self.running.store(true, Ordering::SeqCst);
        self.base.set_enabled(true);

        // Spawn the input reader
        self.spawn_input_reader();

        info!("External channel started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("stopping external channel");
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

        info!("External channel stopped");
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
        debug!(content = %msg.content, "External sending to output EXE");

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
                    error!(exe = %output_exe, error = %e, "failed to spawn output EXE");
                    return;
                }
            };

            if let Some(ref mut stdin) = child.stdin {
                if let Err(e) = stdin.write_all(content.as_bytes()).await {
                    error!(error = %e, "failed to write to output EXE stdin");
                }
            }

            match child.wait().await {
                Ok(status) => {
                    debug!(status = %status, "output EXE exited");
                }
                Err(e) => {
                    warn!(error = %e, "failed to wait for output EXE");
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
mod tests {
    use super::*;

    #[test]
    fn test_external_channel_new_validates() {
        let config = ExternalConfig {
            input_exe: String::new(),
            output_exe: String::new(),
            chat_id: "default".to_string(),
            sync_to: Vec::new(),
            allow_from: Vec::new(),
        };
        assert!(ExternalChannel::new(config).is_err());
    }

    #[tokio::test]
    async fn test_external_channel_lifecycle() {
        let config = ExternalConfig {
            input_exe: "/bin/echo".to_string(),
            output_exe: "/bin/cat".to_string(),
            chat_id: "test-chat".to_string(),
            sync_to: Vec::new(),
            allow_from: Vec::new(),
        };
        let ch = ExternalChannel::new(config).unwrap();
        assert_eq!(ch.name(), "external");

        ch.start().await.unwrap();
        assert!(ch.running.load(Ordering::SeqCst));

        ch.stop().await.unwrap();
        assert!(!ch.running.load(Ordering::SeqCst));
    }

    #[test]
    fn test_process_input_line() {
        let config = ExternalConfig {
            input_exe: "/bin/echo".to_string(),
            output_exe: "/bin/cat".to_string(),
            chat_id: "test-chat".to_string(),
            sync_to: Vec::new(),
            allow_from: Vec::new(),
        };
        let ch = ExternalChannel::new(config).unwrap();

        let (sender, chat, content) = ch.process_input_line("hello world").unwrap();
        assert_eq!(sender, "test-chat");
        assert_eq!(chat, "test-chat");
        assert_eq!(content, "hello world");
    }

    #[test]
    fn test_process_input_line_empty() {
        let config = ExternalConfig {
            input_exe: "/bin/echo".to_string(),
            output_exe: "/bin/cat".to_string(),
            chat_id: "test-chat".to_string(),
            sync_to: Vec::new(),
            allow_from: Vec::new(),
        };
        let ch = ExternalChannel::new(config).unwrap();

        assert!(ch.process_input_line("").is_none());
        assert!(ch.process_input_line("   ").is_none());
    }

    #[test]
    fn test_format_output() {
        let config = ExternalConfig {
            input_exe: "/bin/echo".to_string(),
            output_exe: "/bin/cat".to_string(),
            chat_id: "test-chat".to_string(),
            sync_to: Vec::new(),
            allow_from: Vec::new(),
        };
        let ch = ExternalChannel::new(config).unwrap();

        assert_eq!(ch.format_output("hello"), "hello\n");
    }

    #[tokio::test]
    async fn test_send_validates_chat_id() {
        let config = ExternalConfig {
            input_exe: "/bin/echo".to_string(),
            output_exe: "/bin/cat".to_string(),
            chat_id: "test-chat".to_string(),
            sync_to: Vec::new(),
            allow_from: Vec::new(),
        };
        let ch = ExternalChannel::new(config).unwrap();
        ch.start().await.unwrap();

        let msg = OutboundMessage {
            channel: "external".to_string(),
            chat_id: "wrong-chat".to_string(),
            content: "hello".to_string(),
            message_type: String::new(),
        };
        assert!(ch.send(msg).await.is_err());
    }

    #[test]
    fn test_external_config_accessors() {
        let config = ExternalConfig {
            input_exe: "/path/to/input".to_string(),
            output_exe: "/path/to/output".to_string(),
            chat_id: "my-chat".to_string(),
            sync_to: vec!["web".to_string()],
            allow_from: vec!["user1".to_string()],
        };
        let ch = ExternalChannel::new(config).unwrap();
        assert_eq!(ch.input_exe(), "/path/to/input");
        assert_eq!(ch.output_exe(), "/path/to/output");
        assert_eq!(ch.chat_id(), "my-chat");
    }

    #[test]
    fn test_new_requires_input_exe() {
        let config = ExternalConfig {
            input_exe: String::new(),
            output_exe: "/bin/cat".to_string(),
            chat_id: "test".to_string(),
            sync_to: Vec::new(),
            allow_from: Vec::new(),
        };
        assert!(ExternalChannel::new(config).is_err());
    }

    #[test]
    fn test_new_requires_output_exe() {
        let config = ExternalConfig {
            input_exe: "/bin/echo".to_string(),
            output_exe: String::new(),
            chat_id: "test".to_string(),
            sync_to: Vec::new(),
            allow_from: Vec::new(),
        };
        assert!(ExternalChannel::new(config).is_err());
    }

    #[tokio::test]
    async fn test_send_not_running() {
        let config = ExternalConfig {
            input_exe: "/bin/echo".to_string(),
            output_exe: "/bin/cat".to_string(),
            chat_id: "test-chat".to_string(),
            sync_to: Vec::new(),
            allow_from: Vec::new(),
        };
        let ch = ExternalChannel::new(config).unwrap();
        // Not started - should fail
        let msg = OutboundMessage {
            channel: "external".to_string(),
            chat_id: "test-chat".to_string(),
            content: "hello".to_string(),
            message_type: String::new(),
        };
        assert!(ch.send(msg).await.is_err());
    }

    #[test]
    fn test_process_input_line_whitespace() {
        let config = ExternalConfig {
            input_exe: "/bin/echo".to_string(),
            output_exe: "/bin/cat".to_string(),
            chat_id: "test-chat".to_string(),
            sync_to: Vec::new(),
            allow_from: Vec::new(),
        };
        let ch = ExternalChannel::new(config).unwrap();

        let (_, _, content) = ch.process_input_line("  hello world  ").unwrap();
        assert_eq!(content, "hello world");
    }

    // ---- Additional comprehensive external channel tests ----

    #[test]
    fn test_process_input_line_unicode() {
        let config = ExternalConfig {
            input_exe: "/bin/echo".to_string(),
            output_exe: "/bin/cat".to_string(),
            chat_id: "test-chat".to_string(),
            sync_to: Vec::new(),
            allow_from: Vec::new(),
        };
        let ch = ExternalChannel::new(config).unwrap();

        let (_, _, content) = ch.process_input_line("你好世界 🌍").unwrap();
        assert_eq!(content, "你好世界 🌍");
    }

    #[test]
    fn test_process_input_line_newlines() {
        let config = ExternalConfig {
            input_exe: "/bin/echo".to_string(),
            output_exe: "/bin/cat".to_string(),
            chat_id: "test-chat".to_string(),
            sync_to: Vec::new(),
            allow_from: Vec::new(),
        };
        let ch = ExternalChannel::new(config).unwrap();

        let (_, _, content) = ch.process_input_line("line1\nline2").unwrap();
        assert_eq!(content, "line1\nline2");
    }

    #[test]
    fn test_process_input_line_tabs() {
        let config = ExternalConfig {
            input_exe: "/bin/echo".to_string(),
            output_exe: "/bin/cat".to_string(),
            chat_id: "test-chat".to_string(),
            sync_to: Vec::new(),
            allow_from: Vec::new(),
        };
        let ch = ExternalChannel::new(config).unwrap();

        let (_, _, content) = ch.process_input_line("\thello\t").unwrap();
        assert_eq!(content, "hello");
    }

    #[test]
    fn test_process_input_line_long_line() {
        let config = ExternalConfig {
            input_exe: "/bin/echo".to_string(),
            output_exe: "/bin/cat".to_string(),
            chat_id: "test-chat".to_string(),
            sync_to: Vec::new(),
            allow_from: Vec::new(),
        };
        let ch = ExternalChannel::new(config).unwrap();

        let long = "x".repeat(100_000);
        let (_, _, content) = ch.process_input_line(&long).unwrap();
        assert_eq!(content.len(), 100_000);
    }

    #[test]
    fn test_format_output_empty() {
        let config = ExternalConfig {
            input_exe: "/bin/echo".to_string(),
            output_exe: "/bin/cat".to_string(),
            chat_id: "test-chat".to_string(),
            sync_to: Vec::new(),
            allow_from: Vec::new(),
        };
        let ch = ExternalChannel::new(config).unwrap();

        assert_eq!(ch.format_output(""), "\n");
    }

    #[test]
    fn test_format_output_unicode() {
        let config = ExternalConfig {
            input_exe: "/bin/echo".to_string(),
            output_exe: "/bin/cat".to_string(),
            chat_id: "test-chat".to_string(),
            sync_to: Vec::new(),
            allow_from: Vec::new(),
        };
        let ch = ExternalChannel::new(config).unwrap();

        assert_eq!(ch.format_output("你好"), "你好\n");
    }

    #[test]
    fn test_process_input_line_returns_chat_id_as_sender() {
        let config = ExternalConfig {
            input_exe: "/bin/echo".to_string(),
            output_exe: "/bin/cat".to_string(),
            chat_id: "my-chat".to_string(),
            sync_to: Vec::new(),
            allow_from: Vec::new(),
        };
        let ch = ExternalChannel::new(config).unwrap();

        let (sender, chat, _) = ch.process_input_line("hello").unwrap();
        assert_eq!(sender, "my-chat");
        assert_eq!(chat, "my-chat");
    }

    #[test]
    fn test_new_validates_both_exes() {
        let config = ExternalConfig {
            input_exe: String::new(),
            output_exe: String::new(),
            chat_id: "test".to_string(),
            sync_to: Vec::new(),
            allow_from: Vec::new(),
        };
        // Both empty - should fail
        assert!(ExternalChannel::new(config).is_err());
    }

    #[test]
    fn test_sync_to_config() {
        let config = ExternalConfig {
            input_exe: "/bin/echo".to_string(),
            output_exe: "/bin/cat".to_string(),
            chat_id: "test".to_string(),
            sync_to: vec!["web".to_string(), "discord".to_string()],
            allow_from: vec!["user1".to_string()],
        };
        let ch = ExternalChannel::new(config).unwrap();
        assert_eq!(ch.input_exe(), "/bin/echo");
        assert_eq!(ch.output_exe(), "/bin/cat");
        assert_eq!(ch.chat_id(), "test");
    }

    #[tokio::test]
    async fn test_start_stop_multiple_cycles() {
        let config = ExternalConfig {
            input_exe: "/bin/echo".to_string(),
            output_exe: "/bin/cat".to_string(),
            chat_id: "test-chat".to_string(),
            sync_to: Vec::new(),
            allow_from: Vec::new(),
        };
        let ch = ExternalChannel::new(config).unwrap();

        for _ in 0..3 {
            ch.start().await.unwrap();
            assert!(ch.running.load(Ordering::SeqCst));
            ch.stop().await.unwrap();
            assert!(!ch.running.load(Ordering::SeqCst));
        }
    }

    // ---- Additional coverage tests ----

    #[tokio::test]
    async fn test_send_correct_chat_id() {
        let config = ExternalConfig {
            input_exe: "/bin/echo".to_string(),
            output_exe: "/bin/cat".to_string(),
            chat_id: "test-chat".to_string(),
            sync_to: Vec::new(),
            allow_from: Vec::new(),
        };
        let ch = ExternalChannel::new(config).unwrap();
        ch.start().await.unwrap();

        let msg = OutboundMessage {
            channel: "external".to_string(),
            chat_id: "test-chat".to_string(),
            content: "hello".to_string(),
            message_type: String::new(),
        };
        // Should succeed - correct chat_id, spawns output process
        let result = ch.send(msg).await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_process_input_line_special_chars() {
        let config = ExternalConfig {
            input_exe: "/bin/echo".to_string(),
            output_exe: "/bin/cat".to_string(),
            chat_id: "test-chat".to_string(),
            sync_to: Vec::new(),
            allow_from: Vec::new(),
        };
        let ch = ExternalChannel::new(config).unwrap();

        let (_, _, content) = ch.process_input_line("!@#$%^&*()").unwrap();
        assert_eq!(content, "!@#$%^&*()");
    }

    #[test]
    fn test_format_output_special_chars() {
        let config = ExternalConfig {
            input_exe: "/bin/echo".to_string(),
            output_exe: "/bin/cat".to_string(),
            chat_id: "test-chat".to_string(),
            sync_to: Vec::new(),
            allow_from: Vec::new(),
        };
        let ch = ExternalChannel::new(config).unwrap();

        assert_eq!(ch.format_output("line1\nline2"), "line1\nline2\n");
    }

    #[test]
    fn test_process_input_line_only_spaces() {
        let config = ExternalConfig {
            input_exe: "/bin/echo".to_string(),
            output_exe: "/bin/cat".to_string(),
            chat_id: "test-chat".to_string(),
            sync_to: Vec::new(),
            allow_from: Vec::new(),
        };
        let ch = ExternalChannel::new(config).unwrap();

        assert!(ch.process_input_line("     ").is_none());
    }

    #[test]
    fn test_process_input_line_only_tabs() {
        let config = ExternalConfig {
            input_exe: "/bin/echo".to_string(),
            output_exe: "/bin/cat".to_string(),
            chat_id: "test-chat".to_string(),
            sync_to: Vec::new(),
            allow_from: Vec::new(),
        };
        let ch = ExternalChannel::new(config).unwrap();

        assert!(ch.process_input_line("\t\t\t").is_none());
    }

    // --- Additional coverage tests ---

    #[test]
    fn test_external_config_all_fields() {
        let config = ExternalConfig {
            input_exe: "/usr/bin/input".to_string(),
            output_exe: "/usr/bin/output".to_string(),
            chat_id: "my-chat".to_string(),
            sync_to: vec!["web".to_string(), "discord".to_string()],
            allow_from: vec!["admin".to_string()],
        };
        assert_eq!(config.input_exe, "/usr/bin/input");
        assert_eq!(config.output_exe, "/usr/bin/output");
        assert_eq!(config.chat_id, "my-chat");
        assert_eq!(config.sync_to.len(), 2);
        assert_eq!(config.allow_from.len(), 1);
    }

    #[test]
    fn test_process_input_line_with_spaces_and_text() {
        let config = ExternalConfig {
            input_exe: "/bin/echo".to_string(),
            output_exe: "/bin/cat".to_string(),
            chat_id: "chat".to_string(),
            sync_to: Vec::new(),
            allow_from: Vec::new(),
        };
        let ch = ExternalChannel::new(config).unwrap();

        let (sender, chat, content) = ch.process_input_line("  hello world  ").unwrap();
        assert_eq!(content, "hello world");
        assert_eq!(sender, "chat");
        assert_eq!(chat, "chat");
    }

    #[tokio::test]
    async fn test_start_stop_idempotent() {
        let config = ExternalConfig {
            input_exe: "/bin/echo".to_string(),
            output_exe: "/bin/cat".to_string(),
            chat_id: "test-chat".to_string(),
            sync_to: Vec::new(),
            allow_from: Vec::new(),
        };
        let ch = ExternalChannel::new(config).unwrap();
        ch.start().await.unwrap();
        ch.start().await.unwrap();
        assert!(ch.running.load(Ordering::SeqCst));

        ch.stop().await.unwrap();
        ch.stop().await.unwrap();
        assert!(!ch.running.load(Ordering::SeqCst));
    }

    #[test]
    fn test_format_output_multi_line() {
        let config = ExternalConfig {
            input_exe: "/bin/echo".to_string(),
            output_exe: "/bin/cat".to_string(),
            chat_id: "test-chat".to_string(),
            sync_to: Vec::new(),
            allow_from: Vec::new(),
        };
        let ch = ExternalChannel::new(config).unwrap();

        assert_eq!(ch.format_output("line1\nline2\nline3"), "line1\nline2\nline3\n");
    }

    #[test]
    fn test_process_input_line_carriage_return() {
        let config = ExternalConfig {
            input_exe: "/bin/echo".to_string(),
            output_exe: "/bin/cat".to_string(),
            chat_id: "test-chat".to_string(),
            sync_to: Vec::new(),
            allow_from: Vec::new(),
        };
        let ch = ExternalChannel::new(config).unwrap();

        let (_, _, content) = ch.process_input_line("  hello\r\n  ").unwrap();
        assert_eq!(content, "hello");
    }

    #[tokio::test]
    async fn test_send_with_sync_to_config() {
        let config = ExternalConfig {
            input_exe: "/bin/echo".to_string(),
            output_exe: "/bin/cat".to_string(),
            chat_id: "test-chat".to_string(),
            sync_to: vec!["web".to_string()],
            allow_from: Vec::new(),
        };
        let ch = ExternalChannel::new(config).unwrap();
        ch.start().await.unwrap();

        let msg = OutboundMessage {
            channel: "external".to_string(),
            chat_id: "test-chat".to_string(),
            content: "sync test".to_string(),
            message_type: String::new(),
        };
        // Should succeed - correct chat_id
        let result = ch.send(msg).await;
        assert!(result.is_ok());
    }

    // ============================================================
    // Additional coverage tests for 95%+ target (round 2)
    // ============================================================

    #[test]
    fn test_external_config_default_fields() {
        let config = ExternalConfig {
            input_exe: "a".to_string(),
            output_exe: "b".to_string(),
            chat_id: "c".to_string(),
            sync_to: Vec::new(),
            allow_from: Vec::new(),
        };
        let ch = ExternalChannel::new(config).unwrap();
        assert_eq!(ch.name(), "external");
        assert_eq!(ch.chat_id(), "c");
    }

    #[tokio::test]
    async fn test_send_validates_running_state() {
        let config = ExternalConfig {
            input_exe: "/bin/echo".to_string(),
            output_exe: "/bin/cat".to_string(),
            chat_id: "chat".to_string(),
            sync_to: Vec::new(),
            allow_from: Vec::new(),
        };
        let ch = ExternalChannel::new(config).unwrap();
        // Never started, so send should fail
        let msg = OutboundMessage {
            channel: "external".to_string(),
            chat_id: "chat".to_string(),
            content: "test".to_string(),
            message_type: String::new(),
        };
        let result = ch.send(msg).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not running"));
    }

    #[test]
    fn test_format_output_multiline_content() {
        let config = ExternalConfig {
            input_exe: "/bin/echo".to_string(),
            output_exe: "/bin/cat".to_string(),
            chat_id: "chat".to_string(),
            sync_to: Vec::new(),
            allow_from: Vec::new(),
        };
        let ch = ExternalChannel::new(config).unwrap();
        let output = ch.format_output("line1\nline2\nline3");
        assert!(output.ends_with('\n'));
    }

    #[tokio::test]
    async fn test_send_with_invalid_chat_id_error_message() {
        let config = ExternalConfig {
            input_exe: "/bin/echo".to_string(),
            output_exe: "/bin/cat".to_string(),
            chat_id: "expected-chat".to_string(),
            sync_to: Vec::new(),
            allow_from: Vec::new(),
        };
        let ch = ExternalChannel::new(config).unwrap();
        ch.start().await.unwrap();

        let msg = OutboundMessage {
            channel: "external".to_string(),
            chat_id: "wrong-chat-id".to_string(),
            content: "test".to_string(),
            message_type: String::new(),
        };
        let result = ch.send(msg).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("invalid chat ID"));
        assert!(err.contains("wrong-chat-id"));
        assert!(err.contains("expected-chat"));
    }

    #[test]
    fn test_new_valid_config() {
        let config = ExternalConfig {
            input_exe: "/usr/bin/input".to_string(),
            output_exe: "/usr/bin/output".to_string(),
            chat_id: "chat-1".to_string(),
            sync_to: vec!["web".to_string()],
            allow_from: vec!["user1".to_string()],
        };
        let ch = ExternalChannel::new(config);
        assert!(ch.is_ok());
        let ch = ch.unwrap();
        assert_eq!(ch.input_exe(), "/usr/bin/input");
        assert_eq!(ch.output_exe(), "/usr/bin/output");
        assert_eq!(ch.chat_id(), "chat-1");
    }
}
