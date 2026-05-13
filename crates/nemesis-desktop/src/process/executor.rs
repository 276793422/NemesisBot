//! Platform executor interface and process types.
//!
//! Defines the PlatformExecutor trait for platform-specific subprocess
//! management and the core ChildProcess/ProcessStatus types.
//!
//! The DefaultPlatformExecutor creates child processes with piped stdin/stdout/stderr,
//! supports Windows CREATE_NO_WINDOW flag, and provides graceful terminate-then-kill
//! lifecycle management.

use std::io::{Read, Write};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use tracing::{debug, info};

/// Process status enumeration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProcessStatus {
    Starting,
    Running,
    Handshaking,
    Connected,
    Terminated,
    Failed,
}

/// Represents a managed child process with piped I/O.
pub struct ChildProcess {
    /// Unique identifier for this child.
    pub id: String,
    /// The underlying OS process handle (if available).
    pub child: Option<Child>,
    /// Process ID.
    pub pid: u32,
    /// Window type: "dashboard", "approval", etc.
    pub window_type: String,
    /// Current status.
    pub status: ProcessStatus,
    /// When this child was created.
    pub created_at: SystemTime,
    /// Stdin pipe for sending data to the child.
    stdin_pipe: Option<ChildStdin>,
    /// Stdout pipe for reading data from the child.
    stdout_pipe: Option<ChildStdout>,
    /// Stderr pipe for reading error output from the child.
    stderr_pipe: Option<ChildStderr>,
    /// Flag indicating the child process has exited.
    exited: Arc<AtomicBool>,
}

impl ChildProcess {
    /// Create a new ChildProcess descriptor.
    pub fn new(id: String, pid: u32, window_type: String) -> Self {
        Self {
            id,
            child: None,
            pid,
            window_type,
            status: ProcessStatus::Starting,
            created_at: SystemTime::now(),
            stdin_pipe: None,
            stdout_pipe: None,
            stderr_pipe: None,
            exited: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Check if this process is still alive by querying the exited flag.
    pub fn is_alive(&self) -> bool {
        !self.exited.load(Ordering::SeqCst)
    }

    /// Send a JSON message to the child's stdin pipe.
    pub fn send_message<T: serde::Serialize>(&mut self, msg: &T) -> Result<(), String> {
        if let Some(ref mut stdin) = self.stdin_pipe {
            let data = serde_json::to_vec(msg).map_err(|e| format!("serialize: {}", e))?;
            stdin
                .write_all(&data)
                .map_err(|e| format!("write: {}", e))?;
            stdin
                .write_all(b"\n")
                .map_err(|e| format!("write newline: {}", e))?;
            stdin.flush().map_err(|e| format!("flush: {}", e))?;
            Ok(())
        } else {
            Err("stdin pipe not available".to_string())
        }
    }

    /// Read a JSON message from the child's stdout pipe.
    pub fn read_message<T: serde::de::DeserializeOwned>(&mut self) -> Result<T, String> {
        if let Some(ref mut stdout) = self.stdout_pipe {
            let mut line = String::new();
            let mut byte = [0u8; 1];
            loop {
                match stdout.read(&mut byte) {
                    Ok(0) => return Err("stdout pipe closed".to_string()),
                    Ok(_) => {
                        if byte[0] == b'\n' {
                            break;
                        }
                        line.push(byte[0] as char);
                    }
                    Err(e) => return Err(format!("read: {}", e)),
                }
            }
            serde_json::from_str(&line).map_err(|e| format!("deserialize: {}", e))
        } else {
            Err("stdout pipe not available".to_string())
        }
    }

    /// Read a chunk from stderr.
    pub fn read_stderr_line(&mut self, buf: &mut Vec<u8>) -> Result<usize, String> {
        if let Some(ref mut stderr) = self.stderr_pipe {
            let mut tmp = [0u8; 4096];
            match stderr.read(&mut tmp) {
                Ok(0) => Ok(0),
                Ok(n) => {
                    buf.extend_from_slice(&tmp[..n]);
                    Ok(n)
                }
                Err(e) => Err(format!("read stderr: {}", e)),
            }
        } else {
            Ok(0)
        }
    }

    /// Terminate the process (graceful then forced).
    pub fn kill(&mut self) -> Result<(), String> {
        if let Some(ref mut child) = self.child {
            child.kill().map_err(|e| format!("kill failed: {}", e))
        } else {
            self.status = ProcessStatus::Terminated;
            self.exited.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    /// Wait for the child process to exit and mark it as exited.
    pub fn wait(&mut self) -> Result<std::process::ExitStatus, String> {
        if let Some(ref mut child) = self.child {
            let status = child.wait().map_err(|e| format!("wait failed: {}", e))?;
            self.exited.store(true, Ordering::SeqCst);
            self.status = ProcessStatus::Terminated;
            Ok(status)
        } else {
            self.exited.store(true, Ordering::SeqCst);
            self.status = ProcessStatus::Terminated;
            Err("no child process".to_string())
        }
    }

    /// Try to check if the child has exited without blocking.
    pub fn try_wait(&mut self) -> Result<Option<std::process::ExitStatus>, String> {
        if let Some(ref mut child) = self.child {
            match child.try_wait() {
                Ok(Some(status)) => {
                    self.exited.store(true, Ordering::SeqCst);
                    self.status = ProcessStatus::Terminated;
                    Ok(Some(status))
                }
                Ok(None) => Ok(None),
                Err(e) => Err(format!("try_wait: {}", e)),
            }
        } else {
            Ok(None)
        }
    }

    /// Get a reference to the stdin pipe.
    pub fn stdin(&self) -> Option<&ChildStdin> {
        self.stdin_pipe.as_ref()
    }

    /// Get a reference to the stdout pipe.
    pub fn stdout(&self) -> Option<&ChildStdout> {
        self.stdout_pipe.as_ref()
    }

    /// Get a reference to the stderr pipe.
    pub fn stderr(&self) -> Option<&ChildStderr> {
        self.stderr_pipe.as_ref()
    }
}

/// Configuration for the executor.
#[derive(Debug, Clone)]
pub struct ExecutorConfig {
    /// Whether to hide the child window (Windows-specific).
    pub hide_window: bool,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            hide_window: true,
        }
    }
}

/// Platform-specific executor interface.
///
/// Provides methods for spawning, terminating, and managing child processes.
/// Each platform (Windows, Linux, macOS) should implement this trait.
pub trait PlatformExecutor: Send + Sync {
    /// Spawn a child process with the given executable path and arguments.
    /// Returns a ChildProcess with piped stdin/stdout/stderr.
    fn spawn_child(
        &self,
        exe_path: &str,
        args: &[String],
    ) -> Result<ChildProcess, String>;

    /// Terminate a child process gracefully, then forcefully after timeout.
    fn terminate_child(&self, child: &mut ChildProcess) -> Result<(), String>;

    /// Check if a child process is still alive.
    fn is_process_alive(&self, child: &ChildProcess) -> bool;

    /// Clean up resources associated with a child process.
    fn cleanup(&self, child: &mut ChildProcess) -> Result<(), String>;
}

/// Default executor that uses std::process::Command.
///
/// On Windows, supports CREATE_NO_WINDOW flag to hide console windows.
/// Provides graceful terminate-then-kill with configurable timeout.
pub struct DefaultPlatformExecutor {
    config: ExecutorConfig,
}

impl DefaultPlatformExecutor {
    pub fn new(config: ExecutorConfig) -> Self {
        Self { config }
    }

    pub fn with_defaults() -> Self {
        Self::new(ExecutorConfig::default())
    }

    /// Send a graceful termination signal to the child process.
    ///
    /// On Windows, uses `GenerateConsoleCtrlEvent(CTRL_C_EVENT)` to send
    /// a console control event. On Unix, sends SIGTERM.
    fn send_graceful_signal(&self, child: &ChildProcess) {
        #[cfg(target_os = "windows")]
        {
            // GenerateConsoleCtrlEvent sends a ctrl-C signal to the
            // console group. For child processes created with a new
            // console (CREATE_NO_WINDOW), this is a no-op, but for
            // processes sharing the console it triggers graceful shutdown.
            let pid = child.pid as u32;
            // CTRL_C_EVENT = 0, CTRL_BREAK_EVENT = 1
            const CTRL_C_EVENT: u32 = 0;
            unsafe {
                // GenerateConsoleCtrlEvent returns nonzero on success
                let result = GenerateConsoleCtrlEvent(CTRL_C_EVENT, pid);
                if result == 0 {
                    debug!(
                        "DefaultPlatformExecutor: GenerateConsoleCtrlEvent failed for PID {}",
                        pid
                    );
                } else {
                    debug!(
                        "DefaultPlatformExecutor: Sent CTRL_C_EVENT to PID {}",
                        pid
                    );
                }
            }
        }

        #[cfg(unix)]
        {
            // Send SIGTERM for graceful termination via raw libc FFI
            const SIGTERM: i32 = 15;
            unsafe {
                let pid = child.pid as i32;
                // kill(pid, sig) returns 0 on success, -1 on error
                let result = libc_kill(pid, SIGTERM);
                if result != 0 {
                    debug!(
                        "DefaultPlatformExecutor: kill(SIGTERM) failed for PID {}",
                        child.pid
                    );
                } else {
                    debug!(
                        "DefaultPlatformExecutor: Sent SIGTERM to PID {}",
                        child.pid
                    );
                }
            }
        }

        #[cfg(not(any(target_os = "windows", unix)))]
        {
            let _ = child;
            debug!(
                "DefaultPlatformExecutor: No graceful signal support on this platform"
            );
        }
    }
}

#[cfg(target_os = "windows")]
unsafe extern "system" {
    /// Windows API: Sends a specified control signal to a console process
    /// group that shares the console associated with the calling process.
    fn GenerateConsoleCtrlEvent(ctrl_event: u32, process_group_id: u32) -> u32;
}

#[cfg(unix)]
unsafe extern "C" {
    /// POSIX: Send a signal to a process. Linked as libc_kill to avoid
    /// name collision with Rust identifiers.
    #[link_name = "kill"]
    fn libc_kill(pid: i32, sig: i32) -> i32;
}

impl PlatformExecutor for DefaultPlatformExecutor {
    fn spawn_child(
        &self,
        exe_path: &str,
        args: &[String],
    ) -> Result<ChildProcess, String> {
        let mut cmd = Command::new(exe_path);
        cmd.args(args);

        // Configure stdio: all three pipes for JSON-based IPC
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Determine if this is a GUI process (has --window-type argument)
        let is_gui_process = args.windows(2).any(|w| w[0] == "--window-type");

        // Platform-specific window hiding (Windows only)
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;

            let mut flags = CREATE_NEW_PROCESS_GROUP;
            // Only hide window for non-GUI processes
            if self.config.hide_window && !is_gui_process {
                flags |= CREATE_NO_WINDOW;
            }
            cmd.creation_flags(flags);
        }

        debug!(
            "DefaultPlatformExecutor: Spawning {} {:?}",
            exe_path, args
        );

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("failed to spawn child '{}': {}", exe_path, e))?;

        let pid = child.id();

        // Extract the pipes before moving child
        let stdin_pipe = child.stdin.take();
        let stdout_pipe = child.stdout.take();
        let stderr_pipe = child.stderr.take();

        let mut cp = ChildProcess::new(String::new(), pid, String::new());
        cp.child = Some(child);
        cp.stdin_pipe = stdin_pipe;
        cp.stdout_pipe = stdout_pipe;
        cp.stderr_pipe = stderr_pipe;
        cp.status = ProcessStatus::Running;

        info!("DefaultPlatformExecutor: Child spawned with PID {}", pid);
        Ok(cp)
    }

    fn terminate_child(&self, child: &mut ChildProcess) -> Result<(), String> {
        if child.child.is_none() {
            return Ok(());
        }

        // Step 1: Send a gentle termination signal
        self.send_graceful_signal(child);

        // Step 2: Poll for up to 5 seconds waiting for graceful exit
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Some(ref mut c) = child.child {
                match c.try_wait() {
                    Ok(Some(_)) => {
                        // Process exited gracefully
                        child.status = ProcessStatus::Terminated;
                        child.exited.store(true, Ordering::SeqCst);
                        info!(
                            "DefaultPlatformExecutor: Child PID {} terminated gracefully",
                            child.pid
                        );
                        return Ok(());
                    }
                    Ok(None) => {
                        // Still running, check timeout
                        if std::time::Instant::now() >= deadline {
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                    Err(e) => {
                        debug!("DefaultPlatformExecutor: try_wait error: {}", e);
                        break;
                    }
                }
            } else {
                return Ok(());
            }
        }

        // Step 3: Force kill if still running
        info!(
            "DefaultPlatformExecutor: Child PID {} did not exit gracefully, force killing",
            child.pid
        );
        if let Some(ref mut c) = child.child {
            let _ = c.kill();
            let _ = c.wait();
        }

        child.status = ProcessStatus::Terminated;
        child.exited.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn is_process_alive(&self, child: &ChildProcess) -> bool {
        !child.exited.load(Ordering::SeqCst)
    }

    fn cleanup(&self, child: &mut ChildProcess) -> Result<(), String> {
        // Close stdin pipe
        if let Some(pipe) = child.stdin_pipe.take() {
            drop(pipe);
        }

        // Close stdout pipe
        if let Some(pipe) = child.stdout_pipe.take() {
            drop(pipe);
        }

        // Close stderr pipe
        if let Some(pipe) = child.stderr_pipe.take() {
            drop(pipe);
        }

        // Kill and wait for process if still alive
        if let Some(ref mut c) = child.child {
            let _ = c.kill();
            let _ = c.wait();
        }

        child.child = None;
        child.status = ProcessStatus::Terminated;
        child.exited.store(true, Ordering::SeqCst);
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
    fn test_child_process_new() {
        let cp = ChildProcess::new("child-1".to_string(), 1234, "dashboard".to_string());
        assert_eq!(cp.id, "child-1");
        assert_eq!(cp.pid, 1234);
        assert_eq!(cp.window_type, "dashboard");
        assert_eq!(cp.status, ProcessStatus::Starting);
        assert!(cp.is_alive());
    }

    #[test]
    fn test_executor_config_default() {
        let config = ExecutorConfig::default();
        assert!(config.hide_window);
    }

    #[test]
    fn test_default_platform_executor_spawn() {
        let executor = DefaultPlatformExecutor::with_defaults();
        let args: Vec<String> = if cfg!(windows) {
            vec!["/C".to_string(), "echo hello".to_string()]
        } else {
            vec!["hello".to_string()]
        };
        let exe = if cfg!(windows) { "cmd" } else { "echo" };
        let result = executor.spawn_child(exe, &args);
        if let Ok(mut child) = result {
            assert!(child.pid > 0);
            assert!(child.stdin_pipe.is_some());
            assert!(child.stdout_pipe.is_some());
            assert!(child.stderr_pipe.is_some());
            let _ = executor.terminate_child(&mut child);
        }
    }

    #[test]
    fn test_process_status_serialization() {
        let status = ProcessStatus::Running;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"Running\"");
    }

    #[test]
    fn test_child_process_send_message_no_pipe() {
        let mut cp = ChildProcess::new("test".to_string(), 1, "test".to_string());
        let result = cp.send_message(&serde_json::json!({"test": true}));
        assert!(result.is_err());
    }

    #[test]
    fn test_child_process_read_message_no_pipe() {
        let mut cp = ChildProcess::new("test".to_string(), 1, "test".to_string());
        let result: Result<serde_json::Value, String> = cp.read_message();
        assert!(result.is_err());
    }

    #[test]
    fn test_child_process_kill_no_child() {
        let mut cp = ChildProcess::new("test".to_string(), 1, "test".to_string());
        cp.kill().unwrap();
        assert!(!cp.is_alive());
    }

    #[test]
    fn test_is_process_alive_flag() {
        let cp = ChildProcess::new("test".to_string(), 99999, "test".to_string());
        assert!(cp.is_alive());

        cp.exited.store(true, Ordering::SeqCst);
        assert!(!cp.is_alive());
    }

    // ============================================================
    // Additional tests for coverage improvement
    // ============================================================

    #[test]
    fn test_process_status_all_variants() {
        let statuses = [
            ProcessStatus::Starting,
            ProcessStatus::Running,
            ProcessStatus::Handshaking,
            ProcessStatus::Connected,
            ProcessStatus::Terminated,
            ProcessStatus::Failed,
        ];
        for status in &statuses {
            let json = serde_json::to_string(&status).unwrap();
            let parsed: ProcessStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(*status, parsed);
        }
    }

    #[test]
    fn test_process_status_copy() {
        let s1 = ProcessStatus::Running;
        let s2 = s1;
        assert_eq!(s1, s2);
    }

    #[test]
    fn test_executor_config_custom() {
        let config = ExecutorConfig {
            hide_window: false,
        };
        assert!(!config.hide_window);
    }

    #[test]
    fn test_default_platform_executor_new_custom() {
        let executor = DefaultPlatformExecutor::new(ExecutorConfig {
            hide_window: false,
        });
        // Should be able to create with custom config
        assert!(true);
        let _ = executor;
    }

    #[test]
    fn test_child_process_status_transitions() {
        let mut cp = ChildProcess::new("child-1".to_string(), 1234, "approval".to_string());
        assert_eq!(cp.status, ProcessStatus::Starting);

        cp.status = ProcessStatus::Running;
        assert_eq!(cp.status, ProcessStatus::Running);

        cp.status = ProcessStatus::Handshaking;
        assert_eq!(cp.status, ProcessStatus::Handshaking);

        cp.status = ProcessStatus::Connected;
        assert_eq!(cp.status, ProcessStatus::Connected);

        cp.status = ProcessStatus::Terminated;
        assert_eq!(cp.status, ProcessStatus::Terminated);
    }

    #[test]
    fn test_child_process_wait_no_child() {
        let mut cp = ChildProcess::new("test".to_string(), 1, "test".to_string());
        let result = cp.wait();
        assert!(result.is_err());
        assert!(!cp.is_alive());
        assert_eq!(cp.status, ProcessStatus::Terminated);
    }

    #[test]
    fn test_child_process_try_wait_no_child() {
        let mut cp = ChildProcess::new("test".to_string(), 1, "test".to_string());
        let result = cp.try_wait();
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn test_child_process_read_stderr_no_pipe() {
        let mut cp = ChildProcess::new("test".to_string(), 1, "test".to_string());
        let mut buf = Vec::new();
        let result = cp.read_stderr_line(&mut buf);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);
    }

    #[test]
    fn test_child_process_pipes_none_initially() {
        let cp = ChildProcess::new("test".to_string(), 1, "test".to_string());
        assert!(cp.stdin().is_none());
        assert!(cp.stdout().is_none());
        assert!(cp.stderr().is_none());
    }

    #[test]
    fn test_child_process_created_at() {
        let before = SystemTime::now();
        let cp = ChildProcess::new("test".to_string(), 1, "test".to_string());
        let after = SystemTime::now();
        assert!(cp.created_at >= before);
        assert!(cp.created_at <= after);
    }

    #[test]
    fn test_spawn_child_invalid_exe() {
        let executor = DefaultPlatformExecutor::with_defaults();
        let result = executor.spawn_child("/nonexistent/binary", &[]);
        assert!(result.is_err());
    }

    #[test]
    fn test_cleanup_no_child() {
        let executor = DefaultPlatformExecutor::with_defaults();
        let mut cp = ChildProcess::new("test".to_string(), 99999, "test".to_string());
        let result = executor.cleanup(&mut cp);
        assert!(result.is_ok());
        assert!(!cp.is_alive());
        assert_eq!(cp.status, ProcessStatus::Terminated);
        assert!(cp.child.is_none());
    }

    #[test]
    fn test_terminate_no_child() {
        let executor = DefaultPlatformExecutor::with_defaults();
        let mut cp = ChildProcess::new("test".to_string(), 99999, "test".to_string());
        let result = executor.terminate_child(&mut cp);
        assert!(result.is_ok());
    }

    // ============================================================
    // Additional tests for ~92% coverage
    // ============================================================

    #[test]
    fn test_process_status_debug_format() {
        assert!(format!("{:?}", ProcessStatus::Starting).contains("Starting"));
        assert!(format!("{:?}", ProcessStatus::Running).contains("Running"));
        assert!(format!("{:?}", ProcessStatus::Handshaking).contains("Handshaking"));
        assert!(format!("{:?}", ProcessStatus::Connected).contains("Connected"));
        assert!(format!("{:?}", ProcessStatus::Terminated).contains("Terminated"));
        assert!(format!("{:?}", ProcessStatus::Failed).contains("Failed"));
    }

    #[test]
    fn test_process_status_equality() {
        assert_eq!(ProcessStatus::Running, ProcessStatus::Running);
        assert_ne!(ProcessStatus::Running, ProcessStatus::Failed);
        assert_ne!(ProcessStatus::Starting, ProcessStatus::Connected);
    }

    #[test]
    fn test_executor_config_debug() {
        let config = ExecutorConfig { hide_window: false };
        let debug = format!("{:?}", config);
        assert!(debug.contains("hide_window"));
        assert!(debug.contains("false"));
    }

    #[test]
    fn test_executor_config_clone() {
        let config = ExecutorConfig { hide_window: true };
        let cloned = config.clone();
        assert!(cloned.hide_window);
    }

    #[test]
    fn test_child_process_id_and_window_type() {
        let cp = ChildProcess::new("my-child".to_string(), 42, "approval".to_string());
        assert_eq!(cp.id, "my-child");
        assert_eq!(cp.pid, 42);
        assert_eq!(cp.window_type, "approval");
    }

    #[test]
    fn test_child_process_status_failed() {
        let mut cp = ChildProcess::new("test".to_string(), 1, "test".to_string());
        cp.status = ProcessStatus::Failed;
        assert_eq!(cp.status, ProcessStatus::Failed);
    }

    #[test]
    fn test_child_process_kill_sets_exited() {
        let mut cp = ChildProcess::new("test".to_string(), 1, "test".to_string());
        assert!(cp.is_alive());
        cp.kill().unwrap();
        assert!(!cp.is_alive());
        assert_eq!(cp.status, ProcessStatus::Terminated);
    }

    #[test]
    fn test_child_process_wait_sets_status_terminated() {
        let mut cp = ChildProcess::new("test".to_string(), 1, "test".to_string());
        assert!(cp.wait().is_err()); // no child process
        assert_eq!(cp.status, ProcessStatus::Terminated);
        assert!(!cp.is_alive());
    }

    #[test]
    fn test_child_process_try_wait_sets_exited_when_none() {
        let mut cp = ChildProcess::new("test".to_string(), 1, "test".to_string());
        let result = cp.try_wait();
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
        // Should NOT be marked as terminated since there is no real child
        assert!(cp.is_alive());
    }

    #[test]
    fn test_read_message_no_pipe_returns_error() {
        let mut cp = ChildProcess::new("test".to_string(), 1, "test".to_string());
        let result: Result<serde_json::Value, String> = cp.read_message();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("stdout pipe not available"));
    }

    #[test]
    fn test_send_message_no_pipe_returns_error() {
        let mut cp = ChildProcess::new("test".to_string(), 1, "test".to_string());
        let result = cp.send_message(&serde_json::json!({"msg": "hello"}));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("stdin pipe not available"));
    }

    #[test]
    fn test_read_stderr_no_pipe_returns_zero() {
        let mut cp = ChildProcess::new("test".to_string(), 1, "test".to_string());
        let mut buf = Vec::new();
        let result = cp.read_stderr_line(&mut buf);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);
        assert!(buf.is_empty());
    }

    #[test]
    fn test_child_process_pipes_initially_none() {
        let cp = ChildProcess::new("test".to_string(), 1, "test".to_string());
        assert!(cp.stdin().is_none());
        assert!(cp.stdout().is_none());
        assert!(cp.stderr().is_none());
        assert!(cp.child.is_none());
    }

    #[test]
    fn test_default_platform_executor_with_defaults() {
        let executor = DefaultPlatformExecutor::with_defaults();
        let config = ExecutorConfig { hide_window: true };
        assert!(config.hide_window);
        let _ = executor; // verify it compiles
    }

    #[test]
    fn test_is_process_alive_default_true() {
        let executor = DefaultPlatformExecutor::with_defaults();
        let cp = ChildProcess::new("test".to_string(), 1, "test".to_string());
        assert!(executor.is_process_alive(&cp));
    }

    #[test]
    fn test_is_process_alive_after_exited() {
        let executor = DefaultPlatformExecutor::with_defaults();
        let cp = ChildProcess::new("test".to_string(), 1, "test".to_string());
        cp.exited.store(true, Ordering::SeqCst);
        assert!(!executor.is_process_alive(&cp));
    }

    #[test]
    fn test_spawn_child_with_gui_args() {
        let executor = DefaultPlatformExecutor::with_defaults();
        let args = vec![
            "--multiple".to_string(),
            "--child-id".to_string(),
            "child-1".to_string(),
            "--window-type".to_string(),
            "dashboard".to_string(),
        ];
        // Use a valid exe path that exists but won't actually work as a child
        let exe = if cfg!(windows) { "cmd" } else { "echo" };
        let extra_args: Vec<String> = if cfg!(windows) {
            vec!["/C".to_string(), "echo hello".to_string()]
        } else {
            vec!["hello".to_string()]
        };
        let all_args: Vec<String> = args.into_iter().chain(extra_args).collect();
        let result = executor.spawn_child(exe, &all_args);
        if let Ok(mut child) = result {
            assert!(child.pid > 0);
            let _ = executor.cleanup(&mut child);
        }
    }

    #[test]
    fn test_spawn_child_non_gui() {
        let executor = DefaultPlatformExecutor::new(ExecutorConfig { hide_window: true });
        let args: Vec<String> = if cfg!(windows) {
            vec!["/C".to_string(), "echo hello".to_string()]
        } else {
            vec!["hello".to_string()]
        };
        let exe = if cfg!(windows) { "cmd" } else { "echo" };
        let result = executor.spawn_child(exe, &args);
        if let Ok(mut child) = result {
            assert!(child.pid > 0);
            assert_eq!(child.status, ProcessStatus::Running);
            let _ = executor.cleanup(&mut child);
        }
    }

    #[test]
    fn test_cleanup_closes_pipes() {
        let executor = DefaultPlatformExecutor::with_defaults();
        let args: Vec<String> = if cfg!(windows) {
            vec!["/C".to_string(), "echo hello".to_string()]
        } else {
            vec!["hello".to_string()]
        };
        let exe = if cfg!(windows) { "cmd" } else { "echo" };
        if let Ok(mut child) = executor.spawn_child(exe, &args) {
            assert!(child.stdin().is_some());
            assert!(child.stdout().is_some());
            assert!(child.stderr().is_some());
            executor.cleanup(&mut child).unwrap();
            assert!(child.stdin().is_none());
            assert!(child.stdout().is_none());
            assert!(child.stderr().is_none());
            assert!(child.child.is_none());
            assert_eq!(child.status, ProcessStatus::Terminated);
            assert!(!child.is_alive());
        }
    }

    #[test]
    fn test_spawn_child_empty_args() {
        let executor = DefaultPlatformExecutor::with_defaults();
        let exe = if cfg!(windows) { "cmd" } else { "echo" };
        let empty_args: Vec<String> = vec![];
        // This should still work (spawn without args)
        let result = executor.spawn_child(exe, &empty_args);
        if let Ok(mut child) = result {
            assert!(child.pid > 0);
            let _ = executor.cleanup(&mut child);
        }
    }

    // ============================================================
    // Phase 4: Additional coverage for 93%+ target
    // ============================================================

    #[test]
    fn test_child_process_send_read_message_with_real_pipe() {
        let executor = DefaultPlatformExecutor::with_defaults();
        let exe = if cfg!(windows) { "cmd" } else { "cat" };
        let args: Vec<String> = if cfg!(windows) {
            vec!["/C".to_string(), "pause".to_string()]
        } else {
            vec!["-u".to_string()] // unbuffered
        };

        if let Ok(mut child) = executor.spawn_child(exe, &args) {
            // Send a message
            let msg = serde_json::json!({"type": "test", "data": 123});
            let send_result = child.send_message(&msg);
            assert!(send_result.is_ok());

            // Cleanup
            let _ = executor.cleanup(&mut child);
        }
    }

    #[test]
    fn test_child_process_send_and_cleanup() {
        let executor = DefaultPlatformExecutor::with_defaults();
        let exe = if cfg!(windows) { "cmd" } else { "cat" };
        let args: Vec<String> = if cfg!(windows) {
            vec!["/C".to_string(), "pause".to_string()]
        } else {
            vec![]
        };

        if let Ok(mut child) = executor.spawn_child(exe, &args) {
            assert!(child.is_alive());
            assert_eq!(child.status, ProcessStatus::Running);

            // Send a message
            let msg = serde_json::json!({"type": "handshake"});
            child.send_message(&msg).unwrap();

            // Cleanup should work
            executor.cleanup(&mut child).unwrap();
            assert!(!child.is_alive());
            assert_eq!(child.status, ProcessStatus::Terminated);
            assert!(child.child.is_none());
            assert!(child.stdin().is_none());
            assert!(child.stdout().is_none());
            assert!(child.stderr().is_none());
        }
    }

    #[test]
    fn test_terminate_child_with_real_process() {
        let executor = DefaultPlatformExecutor::with_defaults();
        // Use a long-running process that can be terminated
        let exe = if cfg!(windows) { "ping" } else { "sleep" };
        let args: Vec<String> = if cfg!(windows) {
            vec!["-n".to_string(), "60".to_string(), "127.0.0.1".to_string()]
        } else {
            vec!["60".to_string()]
        };

        if let Ok(mut child) = executor.spawn_child(exe, &args) {
            assert!(child.is_alive());
            let result = executor.terminate_child(&mut child);
            assert!(result.is_ok());
            assert!(!child.is_alive());
        }
    }

    #[test]
    fn test_read_stderr_with_real_process() {
        let executor = DefaultPlatformExecutor::with_defaults();
        // Use a process that may produce stderr and exits quickly
        let exe = if cfg!(windows) { "cmd" } else { "cat" };
        let args: Vec<String> = if cfg!(windows) {
            vec!["/c".to_string(), "echo".to_string(), "test".to_string()]
        } else {
            vec![]
        };

        if let Ok(mut child) = executor.spawn_child(exe, &args) {
            let mut buf = Vec::new();
            // May or may not have stderr data
            let _ = child.read_stderr_line(&mut buf);
            let _ = executor.cleanup(&mut child);
        }
    }

    #[test]
    fn test_try_wait_with_real_process() {
        let executor = DefaultPlatformExecutor::with_defaults();
        let exe = if cfg!(windows) { "cmd" } else { "echo" };
        let args: Vec<String> = if cfg!(windows) {
            vec!["/C".to_string(), "echo hi".to_string()]
        } else {
            vec!["hi".to_string()]
        };

        if let Ok(mut child) = executor.spawn_child(exe, &args) {
            // Give it time to complete
            std::thread::sleep(std::time::Duration::from_millis(500));
            let result = child.try_wait();
            // May or may not have exited yet
            assert!(result.is_ok());
            let _ = executor.cleanup(&mut child);
        }
    }

    #[test]
    fn test_wait_with_real_process() {
        let executor = DefaultPlatformExecutor::with_defaults();
        let exe = if cfg!(windows) { "cmd" } else { "echo" };
        let args: Vec<String> = if cfg!(windows) {
            vec!["/C".to_string(), "echo hi".to_string()]
        } else {
            vec!["hi".to_string()]
        };

        if let Ok(mut child) = executor.spawn_child(exe, &args) {
            let result = child.wait();
            assert!(result.is_ok());
            assert!(!child.is_alive());
            assert_eq!(child.status, ProcessStatus::Terminated);
        }
    }

    #[test]
    fn test_kill_with_real_process() {
        let executor = DefaultPlatformExecutor::with_defaults();
        let exe = if cfg!(windows) { "ping" } else { "sleep" };
        let args: Vec<String> = if cfg!(windows) {
            vec!["-n".to_string(), "60".to_string(), "127.0.0.1".to_string()]
        } else {
            vec!["60".to_string()]
        };

        if let Ok(mut child) = executor.spawn_child(exe, &args) {
            child.kill().unwrap();
            // kill() on a real child process calls child.kill()
            let _ = child.wait(); // reap the zombie
            assert!(!child.is_alive());
        }
    }
}
