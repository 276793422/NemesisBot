// External Output Program Management - Simplified version
use anyhow::{Context, Result};
use chrono::Local;
use std::io::{BufRead, Write};
use std::process::{Command, Stdio};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::config::Config;

/// Manages external output program
pub struct ExternalOutput {
    config: Config,
    program_path: String,
    stdin: Arc<Mutex<Option<std::process::ChildStdin>>>,
}

impl ExternalOutput {
    /// Create a new external output manager
    pub fn new(config: Config, program_path: String) -> Self {
        Self {
            config,
            program_path,
            stdin: Arc::new(Mutex::new(None)),
        }
    }

    /// Start the external output program
    pub async fn start(&self) -> Result<()> {
        let program_path = self.program_path.clone();

        log_message(&self.config, &format!("🚀 Starting output program: {}", program_path));

        let mut child = Command::new(&program_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("Failed to spawn output program")?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("Failed to capture stdin"))?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        // Spawn tasks to read stdout and stderr
        let config = self.config.clone();
        if let Some(stdout) = stdout {
            std::thread::spawn(move || {
                let reader = std::io::BufReader::new(stdout);
                for line in reader.lines() {
                    if let Ok(l) = line {
                        log_message(&config, &format!("[Output stdout] {}", l));
                    }
                }
            });
        }

        if let Some(stderr) = stderr {
            let config = self.config.clone();
            std::thread::spawn(move || {
                let reader = std::io::BufReader::new(stderr);
                for line in reader.lines() {
                    if let Ok(l) = line {
                        log_message(&config, &format!("[Output stderr] {}", l));
                    }
                }
            });
        }

        // Monitor child process
        let config_clone = self.config.clone();
        std::thread::spawn(move || {
            let _ = child.wait();
            log_message(&config_clone, "Output program exited");
        });

        // Store stdin
        *self.stdin.lock().await = Some(stdin);

        Ok(())
    }

    /// Send output to the external program
    pub async fn send(&self, content: &str) -> Result<()> {
        let mut stdin_opt = self.stdin.lock().await;

        if let Some(ref mut stdin) = *stdin_opt {
            log_message(&self.config, &format!("[Output] {}", content));

            writeln!(stdin, "{}", content).context("Failed to write to output program")?;
            stdin.flush().context("Failed to flush output program")?;

            Ok(())
        } else {
            log_message(&self.config, "⚠️  Output program not available, skipping");
            Ok(())
        }
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
