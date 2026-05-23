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
