//! Security Approval System Integration Test
//!
//! Tests the approval workflow covering:
//! 1. Safe operation auto-approve (low-risk ops auto-approved without handler)
//! 2. Dangerous operation auto-reject (high/critical ops rejected without handler)
//! 3. Handler-based approval (mock handler approves after delay)
//! 4. Timeout scenario (handler takes longer than timeout)
//!
//! Ported from Go approval_dialog_test.go

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use nemesis_security::approval::{
    is_safe_operation, operation_display_name, ApprovalConfig, ApprovalManager, ApprovalStatus,
    ChildProcessFactory, MultiApprovalRequest, MultiProcessApprovalManager,
};
use parking_lot::Mutex;
use tokio::sync::oneshot;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_request(id: &str, operation: &str, target: &str, risk_level: &str, timeout_secs: u64) -> MultiApprovalRequest {
    MultiApprovalRequest {
        request_id: id.to_string(),
        operation: operation.to_string(),
        target: target.to_string(),
        risk_level: risk_level.to_string(),
        reason: format!("test request for {} on {}", operation, target),
        context: HashMap::new(),
        timeout_seconds: timeout_secs,
        timestamp: chrono::Utc::now().timestamp(),
    }
}

/// Count test passes / failures.
struct TestRunner {
    passed: usize,
    failed: usize,
    total: usize,
}

impl TestRunner {
    fn new() -> Self {
        Self {
            passed: 0,
            failed: 0,
            total: 0,
        }
    }

    fn assert_ok(&mut self, name: &str, condition: bool, detail: &str) {
        self.total += 1;
        if condition {
            println!("  PASS: {}", name);
            self.passed += 1;
        } else {
            println!("  FAIL: {} -- {}", name, detail);
            self.failed += 1;
        }
    }

    fn summary(&self) -> bool {
        println!();
        println!("========================================");
        println!("  Approval Test Summary");
        println!("========================================");
        println!("  Total:  {}", self.total);
        println!("  Passed: {}", self.passed);
        println!("  Failed: {}", self.failed);
        println!("========================================");
        self.failed == 0
    }
}

// ---------------------------------------------------------------------------
// Test 1: Safe operation auto-approve
// ---------------------------------------------------------------------------

async fn test_safe_operation_auto_approve(runner: &mut TestRunner) {
    println!();
    println!("--- Test 1: Safe operation auto-approve ---");

    let mgr = MultiProcessApprovalManager::new(30);
    mgr.start().unwrap();

    // No child factory set -- should auto-approve safe operations.

    let safe_ops = [
        ("file_read", "/workspace/data.txt"),
        ("dir_list", "/workspace/projects"),
        ("network_request", "https://api.example.com/status"),
        ("hardware_i2c", "/dev/i2c-1"),
        ("registry_read", "HKLM\\Software\\MyApp"),
    ];

    for (op, target) in &safe_ops {
        let req = make_request(&format!("safe-{}", op), op, target, "LOW", 5);
        let resp = mgr.request_approval(&req).await;

        match resp {
            Ok(r) => {
                runner.assert_ok(
                    &format!("{} is auto-approved", op),
                    r.approved,
                    &format!("expected approved=true, got approved={}", r.approved),
                );
                runner.assert_ok(
                    &format!("{} is not timed_out", op),
                    !r.timed_out,
                    &format!("expected timed_out=false, got timed_out={}", r.timed_out),
                );
            }
            Err(e) => {
                runner.assert_ok(
                    &format!("{} is auto-approved", op),
                    false,
                    &format!("request_approval returned error: {}", e),
                );
            }
        }
    }

    // Also verify the helper function directly
    for (op, _target) in &safe_ops {
        runner.assert_ok(
            &format!("is_safe_operation({}, LOW) == true", op),
            is_safe_operation(op, "LOW"),
            &format!("is_safe_operation returned false for {}", op),
        );
    }

    // Verify that safe ops with non-LOW risk are NOT auto-approved
    runner.assert_ok(
        "file_read with MEDIUM risk is NOT safe",
        !is_safe_operation("file_read", "MEDIUM"),
        "file_read should not be safe at MEDIUM risk",
    );
    runner.assert_ok(
        "file_read with HIGH risk is NOT safe",
        !is_safe_operation("file_read", "HIGH"),
        "file_read should not be safe at HIGH risk",
    );
    runner.assert_ok(
        "file_read with CRITICAL risk is NOT safe",
        !is_safe_operation("file_read", "CRITICAL"),
        "file_read should not be safe at CRITICAL risk",
    );
}

// ---------------------------------------------------------------------------
// Test 2: Dangerous operation auto-reject without handler
// ---------------------------------------------------------------------------

async fn test_dangerous_operation_auto_reject(runner: &mut TestRunner) {
    println!();
    println!("--- Test 2: Dangerous operation auto-reject (no handler) ---");

    let mgr = MultiProcessApprovalManager::new(30);
    mgr.start().unwrap();

    // No child factory set -- dangerous ops should be auto-rejected.

    let dangerous_ops = [
        ("file_write", "/etc/hosts", "HIGH"),
        ("file_delete", "/system32/config", "HIGH"),
        ("process_exec", "rm -rf /", "CRITICAL"),
        ("process_kill", "systemd", "CRITICAL"),
        ("registry_write", "HKLM\\SYSTEM\\CurrentControlSet", "CRITICAL"),
        ("system_shutdown", "now", "CRITICAL"),
    ];

    for (op, target, risk) in &dangerous_ops {
        let req = make_request(&format!("danger-{}", op), op, target, risk, 5);
        let resp = mgr.request_approval(&req).await;

        match resp {
            Ok(r) => {
                runner.assert_ok(
                    &format!("{} ({}) is rejected", op, risk),
                    !r.approved,
                    &format!("expected approved=false, got approved={}", r.approved),
                );
                runner.assert_ok(
                    &format!("{} ({}) is not timed_out", op, risk),
                    !r.timed_out,
                    &format!("expected timed_out=false, got timed_out={}", r.timed_out),
                );
            }
            Err(e) => {
                runner.assert_ok(
                    &format!("{} ({}) is rejected", op, risk),
                    false,
                    &format!("request_approval returned error: {}", e),
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Test 3: Dangerous operation with mock handler that approves
// ---------------------------------------------------------------------------

/// A mock factory that simulates a user approving after a configurable delay.
struct DelayedApproveFactory {
    delay_ms: u64,
}

impl ChildProcessFactory for DelayedApproveFactory {
    fn spawn_child(
        &self,
        _window_type: &str,
        _data: HashMap<String, serde_json::Value>,
    ) -> Result<(String, oneshot::Receiver<serde_json::Value>), String> {
        let (tx, rx) = oneshot::channel();
        let delay = Duration::from_millis(self.delay_ms);

        // Spawn a task that sends approval after the delay.
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let _ = tx.send(serde_json::json!({
                "approved": true,
                "reason": "user approved after review"
            }));
        });

        Ok(("mock-approval-child".to_string(), rx))
    }
}

async fn test_dangerous_operation_with_handler_approves(runner: &mut TestRunner) {
    println!();
    println!("--- Test 3: Dangerous operation with handler (approves after 500ms) ---");

    let mgr = MultiProcessApprovalManager::new(30);
    mgr.set_child_factory(Arc::new(DelayedApproveFactory { delay_ms: 500 }));
    mgr.start().unwrap();

    let start = std::time::Instant::now();

    let req = make_request(
        "handler-approve-1",
        "process_exec",
        "rm -rf /important/data",
        "CRITICAL",
        10,
    );
    let resp = mgr.request_approval(&req).await;

    let elapsed = start.elapsed();

    match resp {
        Ok(r) => {
            runner.assert_ok(
                "handler-based approval: approved=true",
                r.approved,
                &format!("expected approved=true, got approved={}", r.approved),
            );
            runner.assert_ok(
                "handler-based approval: not timed out",
                !r.timed_out,
                &format!("expected timed_out=false, got timed_out={}", r.timed_out),
            );
            runner.assert_ok(
                "handler-based approval: elapsed >= 500ms",
                elapsed >= Duration::from_millis(400), // allow small tolerance
                &format!("expected >= 400ms, got {:?}", elapsed),
            );
            runner.assert_ok(
                "handler-based approval: request_id matches",
                r.request_id == "handler-approve-1",
                &format!(
                    "expected request_id=handler-approve-1, got {}",
                    r.request_id
                ),
            );
            runner.assert_ok(
                "handler-based approval: duration_seconds > 0",
                r.duration_seconds > 0.0,
                &format!(
                    "expected duration_seconds > 0, got {}",
                    r.duration_seconds
                ),
            );
        }
        Err(e) => {
            runner.assert_ok(
                "handler-based approval succeeded",
                false,
                &format!("request_approval returned error: {}", e),
            );
        }
    }

    // Also test handler-based deny
    println!();
    println!("--- Test 3b: Dangerous operation with handler (denies) ---");

    struct DenyFactory;
    impl ChildProcessFactory for DenyFactory {
        fn spawn_child(
            &self,
            _window_type: &str,
            _data: HashMap<String, serde_json::Value>,
        ) -> Result<(String, oneshot::Receiver<serde_json::Value>), String> {
            let (tx, rx) = oneshot::channel();
            tx.send(serde_json::json!({"approved": false})).unwrap();
            Ok(("deny-child".to_string(), rx))
        }
    }

    let mgr2 = MultiProcessApprovalManager::new(30);
    mgr2.set_child_factory(Arc::new(DenyFactory));
    mgr2.start().unwrap();

    let req2 = make_request("handler-deny-1", "file_delete", "/etc/passwd", "HIGH", 5);
    let resp2 = mgr2.request_approval(&req2).await;

    match resp2 {
        Ok(r) => {
            runner.assert_ok(
                "handler-based deny: approved=false",
                !r.approved,
                &format!("expected approved=false, got approved={}", r.approved),
            );
            runner.assert_ok(
                "handler-based deny: not timed out",
                !r.timed_out,
                &format!("expected timed_out=false, got timed_out={}", r.timed_out),
            );
        }
        Err(e) => {
            runner.assert_ok(
                "handler-based deny succeeded",
                false,
                &format!("request_approval returned error: {}", e),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Test 4: Timeout scenario
// ---------------------------------------------------------------------------

async fn test_timeout_scenario(runner: &mut TestRunner) {
    println!();
    println!("--- Test 4: Timeout scenario (short timeout, slow handler) ---");

    // The handler never responds (sender kept alive in factory but never sends).
    // With a 1-second timeout, the request should time out.
    // Uses the same pattern as the existing unit tests in approval.rs:
    // hold the sender in a Mutex so it stays alive without sending.

    struct NeverRespondFactory {
        sender: Mutex<Option<oneshot::Sender<serde_json::Value>>>,
    }
    impl ChildProcessFactory for NeverRespondFactory {
        fn spawn_child(
            &self,
            _window_type: &str,
            _data: HashMap<String, serde_json::Value>,
        ) -> Result<(String, oneshot::Receiver<serde_json::Value>), String> {
            let (tx, rx) = oneshot::channel();
            // Store the sender so it stays alive, but never send.
            *self.sender.lock() = Some(tx);
            Ok(("timeout-child".to_string(), rx))
        }
    }

    let sender_holder = Arc::new(NeverRespondFactory {
        sender: Mutex::new(None),
    });

    let mgr = MultiProcessApprovalManager::new(1);
    mgr.set_child_factory(sender_holder);
    mgr.start().unwrap();

    let start = std::time::Instant::now();

    let req = make_request("timeout-1", "file_write", "/system/config", "CRITICAL", 1);
    let resp = mgr.request_approval(&req).await;

    let elapsed = start.elapsed();

    match resp {
        Ok(r) => {
            runner.assert_ok(
                "timeout: approved=false",
                !r.approved,
                &format!("expected approved=false, got approved={}", r.approved),
            );
            runner.assert_ok(
                "timeout: timed_out=true",
                r.timed_out,
                &format!("expected timed_out=true, got timed_out={}", r.timed_out),
            );
            runner.assert_ok(
                "timeout: elapsed approximately 1s",
                elapsed >= Duration::from_millis(800) && elapsed < Duration::from_secs(5),
                &format!("expected ~1s, got {:?}", elapsed),
            );
            runner.assert_ok(
                "timeout: request_id matches",
                r.request_id == "timeout-1",
                &format!("expected timeout-1, got {}", r.request_id),
            );
            runner.assert_ok(
                "timeout: duration_seconds is positive",
                r.duration_seconds > 0.0,
                &format!(
                    "expected duration_seconds > 0, got {}",
                    r.duration_seconds
                ),
            );
        }
        Err(e) => {
            runner.assert_ok(
                "timeout scenario succeeded",
                false,
                &format!("request_approval returned error: {}", e),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Test 5: ApprovalManager (basic in-memory) lifecycle
// ---------------------------------------------------------------------------

fn test_basic_approval_manager(runner: &mut TestRunner) {
    println!();
    println!("--- Test 5: Basic ApprovalManager (in-memory) lifecycle ---");

    let mgr = ApprovalManager::with_default_timeout();

    // Request approval
    let id = mgr.request_approval("file_write /etc/hosts", "agent-1");
    runner.assert_ok(
        "request creates pending request",
        mgr.get(&id).is_some(),
        "request not found after creation",
    );
    runner.assert_ok(
        "initial status is Pending",
        mgr.get(&id).unwrap().status == ApprovalStatus::Pending,
        "expected Pending status",
    );

    // Approve
    mgr.approve(&id).unwrap();
    runner.assert_ok(
        "approve changes status to Approved",
        mgr.get(&id).unwrap().status == ApprovalStatus::Approved,
        "expected Approved status",
    );

    // Double approve fails
    let result = mgr.approve(&id);
    runner.assert_ok(
        "double approve fails",
        result.is_err(),
        "expected error on double approve",
    );

    // Deny a new request
    let id2 = mgr.request_approval("process_exec rm -rf /", "agent-2");
    mgr.deny(&id2, "dangerous operation").unwrap();
    let req2 = mgr.get(&id2).unwrap();
    runner.assert_ok(
        "deny changes status to Denied",
        req2.status == ApprovalStatus::Denied,
        "expected Denied status",
    );
    runner.assert_ok(
        "deny sets reason",
        req2.deny_reason.as_deref() == Some("dangerous operation"),
        &format!(
            "expected reason 'dangerous operation', got {:?}",
            req2.deny_reason
        ),
    );

    // List pending
    let id3 = mgr.request_approval("file_read /tmp/test", "agent-3");
    let pending = mgr.list_pending();
    runner.assert_ok(
        "list_pending includes pending request",
        pending.len() >= 1 && pending.iter().any(|r| r.id == id3),
        &format!("expected at least 1 pending, got {}", pending.len()),
    );

    // Total count
    runner.assert_ok(
        "total_count is correct",
        mgr.total_count() >= 3,
        &format!("expected >= 3, got {}", mgr.total_count()),
    );

    // Nonexistent request
    runner.assert_ok(
        "approve nonexistent fails",
        mgr.approve("does-not-exist").is_err(),
        "expected error for nonexistent request",
    );
    runner.assert_ok(
        "deny nonexistent fails",
        mgr.deny("does-not-exist", "reason").is_err(),
        "expected error for nonexistent request",
    );
}

// ---------------------------------------------------------------------------
// Test 6: Basic ApprovalManager expiration
// ---------------------------------------------------------------------------

fn test_basic_approval_manager_expiration(runner: &mut TestRunner) {
    println!();
    println!("--- Test 6: Basic ApprovalManager expiration ---");

    let mgr = ApprovalManager::new(0); // 0-second timeout = immediate expiration

    let id1 = mgr.request_approval("op-a", "agent");
    let id2 = mgr.request_approval("op-b", "agent");

    // Give a tiny moment so timestamps are captured.
    std::thread::sleep(Duration::from_millis(10));

    let expired_count = mgr.cleanup_expired();
    runner.assert_ok(
        "cleanup_expired returns 2",
        expired_count == 2,
        &format!("expected 2, got {}", expired_count),
    );
    runner.assert_ok(
        "request 1 is Expired",
        mgr.get(&id1).unwrap().status == ApprovalStatus::Expired,
        "expected Expired status",
    );
    runner.assert_ok(
        "request 2 is Expired",
        mgr.get(&id2).unwrap().status == ApprovalStatus::Expired,
        "expected Expired status",
    );
    runner.assert_ok(
        "no pending after cleanup",
        mgr.list_pending().is_empty(),
        "expected empty pending list",
    );

    // Already-approved requests should not be expired by cleanup.
    let mgr2 = ApprovalManager::new(0);
    let id3 = mgr2.request_approval("op-c", "agent");
    mgr2.approve(&id3).unwrap();
    std::thread::sleep(Duration::from_millis(10));
    let expired2 = mgr2.cleanup_expired();
    runner.assert_ok(
        "cleanup does not affect approved requests",
        expired2 == 0,
        &format!("expected 0 expired, got {}", expired2),
    );
}

// ---------------------------------------------------------------------------
// Test 7: MultiProcessApprovalManager config management
// ---------------------------------------------------------------------------

fn test_config_management(runner: &mut TestRunner) {
    println!();
    println!("--- Test 7: MultiProcessApprovalManager config management ---");

    let mgr = MultiProcessApprovalManager::new(30);

    // Default config
    let default_config = mgr.get_config();
    runner.assert_ok(
        "default config: enabled=true",
        default_config.enabled,
        "expected enabled=true",
    );
    runner.assert_ok(
        "default config: timeout=30s",
        default_config.timeout == Duration::from_secs(30),
        &format!(
            "expected 30s, got {:?}",
            default_config.timeout
        ),
    );
    runner.assert_ok(
        "default config: min_risk_level=MEDIUM",
        default_config.min_risk_level == "MEDIUM",
        &format!(
            "expected MEDIUM, got {}",
            default_config.min_risk_level
        ),
    );

    // Custom config
    let custom = ApprovalConfig {
        enabled: false,
        timeout: Duration::from_secs(60),
        min_risk_level: "HIGH".to_string(),
        dialog_width: 800,
        dialog_height: 600,
        enable_sound: false,
        enable_animation: false,
    };
    mgr.set_config(custom);
    let retrieved = mgr.get_config();
    runner.assert_ok(
        "custom config: enabled=false",
        !retrieved.enabled,
        "expected enabled=false",
    );
    runner.assert_ok(
        "custom config: timeout=60s",
        retrieved.timeout == Duration::from_secs(60),
        &format!("expected 60s, got {:?}", retrieved.timeout),
    );
    runner.assert_ok(
        "custom config: min_risk_level=HIGH",
        retrieved.min_risk_level == "HIGH",
        &format!("expected HIGH, got {}", retrieved.min_risk_level),
    );
    runner.assert_ok(
        "custom config: dialog_width=800",
        retrieved.dialog_width == 800,
        &format!("expected 800, got {}", retrieved.dialog_width),
    );
}

// ---------------------------------------------------------------------------
// Test 8: MultiProcessApprovalManager not-running error
// ---------------------------------------------------------------------------

async fn test_not_running_error(runner: &mut TestRunner) {
    println!();
    println!("--- Test 8: Manager not running returns error ---");

    let mgr = MultiProcessApprovalManager::new(30);
    // Do NOT start the manager.

    let req = make_request("not-running-1", "file_write", "/tmp/test", "HIGH", 5);
    let result = mgr.request_approval(&req).await;

    runner.assert_ok(
        "not running returns error",
        result.is_err(),
        "expected error when manager is not running",
    );
    runner.assert_ok(
        "error contains 'not running'",
        result
            .as_ref()
            .err()
            .map(|e| e.contains("not running"))
            .unwrap_or(false),
        &format!(
            "expected 'not running' in error, got {:?}",
            result.as_ref().err()
        ),
    );
}

// ---------------------------------------------------------------------------
// Test 9: Popup not supported fallback
// ---------------------------------------------------------------------------

async fn test_popup_not_supported(runner: &mut TestRunner) {
    println!();
    println!("--- Test 9: Popup not supported fallback ---");

    struct PopupNotSupportedFactory;
    impl ChildProcessFactory for PopupNotSupportedFactory {
        fn spawn_child(
            &self,
            _window_type: &str,
            _data: HashMap<String, serde_json::Value>,
        ) -> Result<(String, oneshot::Receiver<serde_json::Value>), String> {
            Err("popup not supported on this platform".to_string())
        }
    }

    let mgr = MultiProcessApprovalManager::new(30);
    mgr.set_child_factory(Arc::new(PopupNotSupportedFactory));
    mgr.start().unwrap();

    let req = make_request("popup-1", "file_write", "/tmp/test", "HIGH", 5);
    let resp = mgr.request_approval(&req).await;

    match resp {
        Ok(r) => {
            runner.assert_ok(
                "popup not supported: approved=false",
                !r.approved,
                &format!("expected approved=false, got {}", r.approved),
            );
            runner.assert_ok(
                "popup not supported: not timed out",
                !r.timed_out,
                &format!("expected timed_out=false, got {}", r.timed_out),
            );
        }
        Err(e) => {
            runner.assert_ok(
                "popup not supported: succeeded",
                false,
                &format!("unexpected error: {}", e),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Test 10: Validation of MultiApprovalRequest
// ---------------------------------------------------------------------------

fn test_request_validation(runner: &mut TestRunner) {
    println!();
    println!("--- Test 10: MultiApprovalRequest validation ---");

    // Valid request
    let valid = make_request("valid-1", "file_write", "/tmp/test", "HIGH", 30);
    runner.assert_ok(
        "valid request passes validation",
        valid.validate().is_ok(),
        &format!("expected Ok, got {:?}", valid.validate()),
    );

    // Empty request_id
    let bad_id = make_request("", "file_write", "/tmp/test", "HIGH", 30);
    let result = bad_id.validate();
    runner.assert_ok(
        "empty request_id fails validation",
        result.is_err(),
        "expected error for empty request_id",
    );
    runner.assert_ok(
        "error mentions request_id",
        result
            .as_ref()
            .err()
            .map(|e| e.contains("request_id"))
            .unwrap_or(false),
        &format!("got {:?}", result),
    );

    // Empty operation
    let bad_op = make_request("id-1", "", "/tmp/test", "HIGH", 30);
    runner.assert_ok(
        "empty operation fails validation",
        bad_op.validate().is_err(),
        "expected error for empty operation",
    );

    // Empty target
    let bad_target = make_request("id-2", "file_write", "", "HIGH", 30);
    runner.assert_ok(
        "empty target fails validation",
        bad_target.validate().is_err(),
        "expected error for empty target",
    );

    // Invalid risk level
    let bad_risk = make_request("id-3", "file_write", "/tmp/test", "INVALID", 30);
    let result = bad_risk.validate();
    runner.assert_ok(
        "invalid risk_level fails validation",
        result.is_err(),
        "expected error for invalid risk_level",
    );
    runner.assert_ok(
        "error mentions invalid risk_level",
        result
            .as_ref()
            .err()
            .map(|e| e.contains("invalid risk_level"))
            .unwrap_or(false),
        &format!("got {:?}", result),
    );

    // Zero timeout
    let bad_timeout = make_request("id-4", "file_write", "/tmp/test", "HIGH", 0);
    let result = bad_timeout.validate();
    runner.assert_ok(
        "zero timeout fails validation",
        result.is_err(),
        "expected error for zero timeout",
    );
    runner.assert_ok(
        "error mentions timeout_seconds",
        result
            .as_ref()
            .err()
            .map(|e| e.contains("timeout_seconds"))
            .unwrap_or(false),
        &format!("got {:?}", result),
    );

    // All valid risk levels
    for level in &["LOW", "MEDIUM", "HIGH", "CRITICAL"] {
        let req = make_request("valid-risk", "file_write", "/tmp/test", level, 30);
        runner.assert_ok(
            &format!("risk_level {} is valid", level),
            req.validate().is_ok(),
            &format!("expected {} to be valid", level),
        );
    }
}

// ---------------------------------------------------------------------------
// Test 11: Operation display names
// ---------------------------------------------------------------------------

fn test_operation_display_names(runner: &mut TestRunner) {
    println!();
    println!("--- Test 11: Operation display names ---");

    let names = [
        ("file_read", "File Read"),
        ("file_write", "File Write"),
        ("file_delete", "File Delete"),
        ("dir_read", "Directory Listing"),
        ("dir_list", "Directory Listing"),
        ("dir_create", "Create Directory"),
        ("dir_delete", "Delete Directory"),
        ("process_exec", "Execute Command"),
        ("process_spawn", "Spawn Process"),
        ("process_kill", "Kill Process"),
        ("network_request", "Network Request"),
        ("network_download", "Download File"),
        ("network_upload", "Upload File"),
        ("hardware_i2c", "I2C Access"),
        ("hardware_spi", "SPI Access"),
        ("hardware_gpio", "GPIO Access"),
        ("registry_read", "Registry Read"),
        ("registry_write", "Registry Write"),
        ("registry_delete", "Registry Delete"),
        ("system_shutdown", "System Shutdown"),
        ("system_reboot", "System Reboot"),
        ("custom_op", "custom_op"), // unknown -> identity
    ];

    for (op, expected_name) in &names {
        let display = operation_display_name(op);
        runner.assert_ok(
            &format!("display_name({}) == {}", op, expected_name),
            display == *expected_name,
            &format!("expected '{}', got '{}'", expected_name, display),
        );
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    println!("========================================");
    println!("  NemesisBot Security Approval Tests");
    println!("========================================");
    println!();

    let mut runner = TestRunner::new();

    test_safe_operation_auto_approve(&mut runner).await;
    test_dangerous_operation_auto_reject(&mut runner).await;
    test_dangerous_operation_with_handler_approves(&mut runner).await;
    test_timeout_scenario(&mut runner).await;
    test_basic_approval_manager(&mut runner);
    test_basic_approval_manager_expiration(&mut runner);
    test_config_management(&mut runner);
    test_not_running_error(&mut runner).await;
    test_popup_not_supported(&mut runner).await;
    test_request_validation(&mut runner);
    test_operation_display_names(&mut runner);

    let all_passed = runner.summary();

    if all_passed {
        println!();
        println!("All tests PASSED.");
        std::process::exit(0);
    } else {
        println!();
        println!("Some tests FAILED.");
        std::process::exit(1);
    }
}
