// WebSocket Client - Single Event Loop Model (Like JavaScript)
use anyhow::{Context, Result};
use chrono::Local;
use colored::Colorize;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

use crate::config::{Config, MessageRulesConfig};
use crate::external_output::ExternalOutput;
use crate::request_lock::RequestLock;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ClientMessage {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type")]
pub enum ServerMessage {
    #[serde(rename = "message")]
    Message { role: String, content: String, #[serde(default)] timestamp: String },
    #[serde(rename = "pong")]
    Pong {},
    #[serde(rename = "error")]
    Error { #[serde(default)] error: String },
}

#[derive(Debug)]
pub struct Statistics {
    pub messages_sent: AtomicU64,
    pub messages_received: AtomicU64,
    pub bytes_sent: AtomicU64,
    pub bytes_received: AtomicU64,
    pub reconnect_count: AtomicU64,
    pub connected_at: AtomicU64,
}

impl Statistics {
    pub fn new() -> Self {
        Self {
            messages_sent: AtomicU64::new(0),
            messages_received: AtomicU64::new(0),
            bytes_sent: AtomicU64::new(0),
            bytes_received: AtomicU64::new(0),
            reconnect_count: AtomicU64::new(0),
            connected_at: AtomicU64::new(0),
        }
    }

    pub fn print(&self, config: &Config) {
        if !config.statistics.enabled { return; }
        let sent = self.messages_sent.load(Ordering::Relaxed);
        let received = self.messages_received.load(Ordering::Relaxed);
        let bytes_sent = self.bytes_sent.load(Ordering::Relaxed);
        let bytes_received = self.bytes_received.load(Ordering::Relaxed);
        let reconnects = self.reconnect_count.load(Ordering::Relaxed);
        println!("\n{}", format!("📊 Sent: {} msgs | Received: {} msgs | Reconnects: {}", sent, received, reconnects).dimmed());
    }
}

pub struct WebSocketClient {
    config: Config,
    stats: Arc<Statistics>,
    running: Arc<AtomicBool>,
    external_rx: Option<mpsc::UnboundedReceiver<String>>,
    output_program: Option<Arc<ExternalOutput>>,
    request_lock: Option<Arc<RequestLock>>,
}

impl WebSocketClient {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            stats: Arc::new(Statistics::new()),
            running: Arc::new(AtomicBool::new(false)),
            external_rx: None,
            output_program: None,
            request_lock: None,
        }
    }

    pub fn with_external_receiver(mut self, rx: mpsc::UnboundedReceiver<String>) -> Self {
        self.external_rx = Some(rx);
        self
    }

    pub fn with_output_program(mut self, output: Arc<ExternalOutput>) -> Self {
        self.output_program = Some(output);
        self
    }

    pub fn with_request_lock(mut self, lock: Arc<RequestLock>) -> Self {
        self.request_lock = Some(lock);
        self
    }

    pub fn get_running_flag(&self) -> Arc<AtomicBool> {
        self.running.clone()
    }

    pub async fn start(&mut self) -> Result<()> {
        self.running.store(true, Ordering::Relaxed);
        let mut reconnect_attempts = 0;
        let mut reconnect_delay = self.config.reconnect.initial_delay;

        // Take external receiver once - it will be reused across reconnections
        let mut external_rx = self.external_rx.take().ok_or_else(|| anyhow::anyhow!("No receiver"))?;

        while self.running.load(Ordering::Relaxed) {
            if self.config.reconnect.max_attempts > 0 && reconnect_attempts >= self.config.reconnect.max_attempts {
                return Err(anyhow::anyhow!("Max reconnect attempts"));
            }

            match self.connect_and_run(&mut external_rx).await {
                Ok(_) => break,
                Err(e) => {
                    eprintln!("{}", format!("⚠️  Connection error: {}", e).yellow().bold());
                    if !self.config.reconnect.enabled || !self.running.load(Ordering::Relaxed) {
                        return Err(e);
                    }
                    reconnect_attempts += 1;
                    self.stats.reconnect_count.fetch_add(1, Ordering::Relaxed);
                    eprintln!("{}", format!("🔄 Reconnecting in {} seconds... (attempt {})", reconnect_delay, reconnect_attempts).yellow());
                    sleep(Duration::from_secs(reconnect_delay)).await;
                    reconnect_delay = std::cmp::min((reconnect_delay as f64 * self.config.reconnect.delay_multiplier) as u64, self.config.reconnect.max_delay);
                }
            }
        }
        Ok(())
    }

    async fn connect_and_run(&mut self, external_rx: &mut mpsc::UnboundedReceiver<String>) -> Result<()> {
        println!("🔄 Connecting...");
        let url = if self.config.server.token.is_empty() {
            self.config.server.url.clone()
        } else {
            format!("{}?token={}", self.config.server.url, self.config.server.token)
        };

        let (ws_stream, _) = connect_async(&url).await.context("Failed to connect")?;
        println!("{}", "✅ Connected!".green());

        let (mut ws_sender, mut ws_receiver) = ws_stream.split();
        self.stats.connected_at.store(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| anyhow::anyhow!("Time error: {}", e))?
                .as_secs(),
            Ordering::Relaxed,
        );

        let running = self.running.clone();
        let config = self.config.clone();
        let stats = self.stats.clone();

        println!("📤 SINGLE EVENT LOOP - Never send + receive simultaneously");
        let mut cli_rx_closed = false;
        let mut last_activity = std::time::Instant::now();
        let idle_timeout = Duration::from_secs(30);

        loop {
            tokio::select! {
                // Receive from WebSocket
                msg = ws_receiver.next() => {
                    match msg {
                        Some(Ok(Message::Text(text))) => {
                            // println!("📥 [RX] {} bytes", text.len());
                            last_activity = std::time::Instant::now();
                            stats.bytes_received.fetch_add(text.len() as u64, Ordering::Relaxed);
                            if let Ok(msg) = serde_json::from_str::<ServerMessage>(&text) {
                                match msg {
                                    ServerMessage::Message { role, content, timestamp } => {
                                        // Apply message rules
                                        let (processed_content, rule_applied, should_skip) = apply_message_rules(&config, &content);

                                        if should_skip {
                                            // Skip this message entirely
                                            stats.messages_received.fetch_add(1, Ordering::Relaxed);
                                            let rule_name = rule_applied.as_ref().map(|s| s.as_str()).unwrap_or("unknown");
                                            log_message(&config, &format!("🚫 Skipped message due to rule: {}", rule_name));
                                            // println!("{}", format!("  🚫 Message skipped (rule: {})", rule_name).dimmed());

                                            // Still release lock even if message is skipped
                                            if let Some(ref lock) = self.request_lock {
                                                lock.release().await;
                                            }
                                        } else {
                                            // Display the message
                                            stats.messages_received.fetch_add(1, Ordering::Relaxed);
                                            log_message(&config, &format!("[{}]: {}", role, processed_content));
                                            print_received_message(&config, &role, &processed_content, &timestamp);

                                            // Log which rule was applied (if any)
                                            if let Some(rule_name) = rule_applied {
                                                // println!("{}", format!("  🔔 Applied rule: {}", rule_name).dimmed());
                                            }

                                            // Send to output program if configured (only for assistant messages)
                                            if role == "assistant" {
                                                if let Some(ref output) = self.output_program {
                                                    if let Err(e) = output.send(&processed_content).await {
                                                        // eprintln!("{}", format!("⚠️  Failed to send to output program: {}", e).yellow());
                                                        log_message(&config, &format!("⚠️  Failed to send to output program: {}", e));
                                                    }
                                                }
                                            }

                                            // Release request lock if configured
                                            if let Some(ref lock) = self.request_lock {
                                                lock.release().await;
                                            }
                                        }
                                    }
                                    ServerMessage::Pong {} => { log_message(&config, "PONG"); }
                                    ServerMessage::Error { error } => {
                                        eprintln!("❌ Error: {}", error);
                                        // Also release lock on error
                                        if let Some(ref lock) = self.request_lock {
                                            lock.release().await;
                                        }
                                    }
                                }
                            }
                        }
                        Some(Ok(Message::Close(close_frame))) => {
                            println!("🔌 Server closed connection");
                            break;
                        }
                        Some(Err(e)) => { eprintln!("⚠️  Error: {}", e); break; }
                        Some(Ok(_)) => {}
                        None => { println!("📥 Stream ended"); break; }
                    }
                }

                // Receive from CLI (external_rx)
                msg = async {
                    if cli_rx_closed {
                        std::future::pending().await
                    } else {
                        external_rx.recv().await
                    }
                } => {
                    match msg {
                        Some(content) => {
                            // Check request lock before sending
                            let can_send = if let Some(ref lock) = self.request_lock {
                                match lock.try_acquire(content.clone()).await {
                                    Ok(_) => true,
                                    Err(_) => false,
                                }
                            } else {
                                true
                            };

                            if !can_send {
                                // Lock is busy, drop this message
                                log_message(&config, "⚠️  Request locked, message dropped");
                                continue;
                            }

                            // println!("📤 [TX] {}", content);
                            last_activity = std::time::Instant::now();
                            let msg = ClientMessage {
                                msg_type: "message".to_string(),
                                content: content.clone(),
                                timestamp: Some(Local::now().to_rfc3339()),
                            };
                            if let Ok(json) = serde_json::to_string(&msg) {
                                let json_len = json.len();
                                match ws_sender.send(Message::Text(json.into())).await {
                                    Ok(_) => {
                                        // println!("✅ Sent");
                                        stats.messages_sent.fetch_add(1, Ordering::Relaxed);
                                        stats.bytes_sent.fetch_add(json_len as u64, Ordering::Relaxed);
                                        log_message(&config, &content);
                                    }
                                    Err(_) => {
                                        eprintln!("❌ Send failed");
                                        // Release lock on send failure
                                        if let Some(ref lock) = self.request_lock {
                                            lock.release().await;
                                        }
                                        break;
                                    }
                                }
                            }
                        }
                        None => {
                            cli_rx_closed = true;
                        }
                    }
                }
            }

            // Exit conditions:
            // 1. If CLI is closed AND running is false AND no activity for timeout period
            if cli_rx_closed && !running.load(Ordering::Relaxed) && last_activity.elapsed() > idle_timeout {
                println!("⏱️  Idle timeout after CLI closed, exiting");
                break;
            }
        }

        running.store(false, Ordering::Relaxed);
        Ok(())
    }

    pub fn stop(&self) { self.running.store(false, Ordering::Relaxed); }
    pub fn get_stats(&self) -> &Statistics { &self.stats }
    pub fn is_connected(&self) -> bool { self.running.load(Ordering::Relaxed) }
}

fn print_received_message(config: &Config, role: &str, content: &str, _timestamp: &str) {
    let ts = if config.ui.show_timestamp { format!("[{}] ", Local::now().format("%H:%M:%S")) } else { String::new() };
    let (role_str, color) = match role {
        "assistant" => ("🤖 Assistant", "bright cyan"),
        "user" => ("👤 User", "bright green"),
        "system" => ("⚙️  System", "bright yellow"),
        _ => ("📨 Unknown", "white"),
    };
    let msg = if config.ui.color {
        format!("{}{}{}: {}", ts.dimmed(), role_str.color(color), ":".color(color), content)
    } else {
        format!("{}{}: {}", ts, role_str, content)
    };
    println!("{}", msg);
}

fn log_message(config: &Config, message: &str) {
    if !config.logging.enabled || config.logging.file.is_empty() { return; }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&config.logging.file) {
        use std::io::Write;
        let _ = writeln!(f, "[{}] {}", Local::now().to_rfc3339(), message);
    }
}

/// Apply message rules to content
/// Returns (processed_content, rule_name_if_applied, should_skip)
fn apply_message_rules(config: &Config, content: &str) -> (String, Option<String>, bool) {
    if !config.message_rules.enabled {
        return (content.to_string(), None, false);
    }

    for rule in &config.message_rules.rules {
        if !rule.enabled {
            continue;
        }

        let matches = if rule.case_sensitive {
            content.contains(&rule.pattern)
        } else {
            content.to_lowercase().contains(&rule.pattern.to_lowercase())
        };

        if matches {
            log_message(config, &format!("🔄 Rule '{}' applied: {}", rule.name, rule.description));

            if rule.skip {
                // Skip this message entirely
                return (String::new(), Some(rule.name.clone()), true);
            } else {
                // Replace message content
                return (rule.replacement.clone(), Some(rule.name.clone()), false);
            }
        }
    }

    (content.to_string(), None, false)
}
