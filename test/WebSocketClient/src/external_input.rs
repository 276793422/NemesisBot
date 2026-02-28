// External Input Program Management - Simplified version
use anyhow::{Context, Result};
use chrono::Local;
use colored::Colorize;
use std::io::BufRead;
use std::process::{Command, Stdio};
use tokio::sync::mpsc;

use crate::config::Config;

/// Manages external input program
pub struct ExternalInput {
    config: Config,
    program_path: String,
    max_retries: u32,
    retry_delay: u64,
}

impl ExternalInput {
    /// Create a new external input manager
    pub fn new(config: Config, program_path: String) -> Self {
        Self {
            config,
            program_path,
            max_retries: 6,
            retry_delay: 10,
        }
    }

    /// Start the external input program
    /// Returns a handle to the spawned task
    pub async fn start(
        &self,
        tx: mpsc::UnboundedSender<String>,
    ) -> Result<tokio::task::JoinHandle<()>> {
        let program_path = self.program_path.clone();
        let config = self.config.clone();
        let max_retries = self.max_retries;
        let retry_delay = self.retry_delay;

        let handle = tokio::spawn(async move {
            let mut retries = 0;

            loop {
                match Self::spawn_and_read(&program_path, &config, tx.clone()).await {
                    Ok(_) => {
                        // Program exited normally, don't retry
                        break;
                    }
                    Err(e) => {
                        retries += 1;

                        if retries >= max_retries {
                            log_message(
                                &config,
                                &format!("❌ Input program failed after {} retries: {}, switching to CLI input", max_retries, e),
                            );
                            // eprintln!(
                            //     "{}",
                            //     format!("❌ Input program failed after {} retries, switching to CLI input", max_retries).red()
                            // );
                            break;
                        }

                        log_message(
                            &config,
                            &format!("⚠️  Input program failed (attempt {}/{}): {}, retrying in {} seconds...", retries, max_retries, e, retry_delay),
                        );
                        // eprintln!(
                        //     "{}",
                        //     format!("⚠️  Input program failed (attempt {}/{}), retrying in {} seconds...", retries, max_retries, retry_delay).yellow()
                        // );

                        tokio::time::sleep(tokio::time::Duration::from_secs(retry_delay)).await;
                    }
                }
            }
        });

        Ok(handle)
    }

    /// Spawn the input program and read its stdout
    async fn spawn_and_read(
        program_path: &str,
        config: &Config,
        tx: mpsc::UnboundedSender<String>,
    ) -> Result<()> {
        log_message(config, &format!("🚀 Starting input program: {}", program_path));

        let mut child = Command::new(program_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("Failed to spawn input program")?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("Failed to capture stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow::anyhow!("Failed to capture stderr"))?;

        // Wait for child to exit (in background)
        let _ = std::thread::spawn(move || {
            let _ = child.wait();
        });

        // Read stderr in background thread
        let config_stderr = config.clone();
        std::thread::spawn(move || {
            let reader = std::io::BufReader::new(stderr);
            for line in reader.lines().flatten() {
                log_message(&config_stderr, &format!("[Input stderr] {}", line));
            }
        });

        // Read stdout line by line
        let reader = std::io::BufReader::new(stdout);

        for line in reader.lines() {
            match line {
                Ok(text) => {
                    let text = text.trim();
                    if !text.is_empty() {
                        log_message(&config, &format!("[Input] {}", text));

                        if tx.send(text.to_string()).is_err() {
                            log_message(&config, "⚠️  Failed to send input to main channel");
                            break;
                        }
                    }
                }
                Err(e) => {
                    log_message(&config, &format!("Error reading from input program: {}", e));
                    break;
                }
            }
        }

        Ok(())
    }
}

fn log_message(config: &Config, message: &str) {
    if !config.logging.enabled || config.logging.file.is_empty() {
        return;
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&config.logging.file) {
        use std::io::Write;
        let _ = writeln!(f, "[{}] {}", Local::now().to_rfc3339(), message);
    }
}
