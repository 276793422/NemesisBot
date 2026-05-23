use super::*;

#[test]
fn test_dangerous_detection() {
    let tool = AsyncExecTool::new(".", false);
    assert!(tool.is_dangerous("rm -rf /"));
    assert!(tool.is_dangerous("sudo apt install foo"));
    assert!(tool.is_dangerous("shutdown"));
    assert!(!tool.is_dangerous("echo hello"));
    assert!(!tool.is_dangerous("notepad.exe"));
}

#[test]
fn test_extract_process_name() {
    assert_eq!(AsyncExecTool::extract_process_name("notepad.exe"), "notepad");
    assert_eq!(
        AsyncExecTool::extract_process_name("notepad.exe README.md"),
        "notepad"
    );
    assert_eq!(AsyncExecTool::extract_process_name("  calc.exe  "), "calc");
    assert_eq!(AsyncExecTool::extract_process_name(""), "");
}

#[tokio::test]
async fn test_empty_command_rejected() {
    let tool = AsyncExecTool::new(".", false);
    let result = tool
        .execute(&serde_json::json!({"command": "  "}))
        .await;
    assert!(result.is_error);
}

#[tokio::test]
async fn test_dangerous_command_rejected() {
    let tool = AsyncExecTool::new(".", false);
    let result = tool
        .execute(&serde_json::json!({"command": "rm -rf /"}))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("dangerous"));
}

#[tokio::test]
async fn test_missing_command() {
    let tool = AsyncExecTool::new(".", false);
    let result = tool
        .execute(&serde_json::json!({}))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("required"));
}

#[tokio::test]
async fn test_launch_simple_command() {
    let tool = AsyncExecTool::new(".", false);
    let result = tool
        .execute(&serde_json::json!({
            "command": "echo hello_world",
            "wait_seconds": 1
        }))
        .await;
    // echo exits very fast, but the check by name may or may not find it
    // We just verify it doesn't panic or error on command rejection
    assert!(
        !result.for_llm.contains("blocked"),
        "Should not block echo: {}",
        result.for_llm
    );
}

// ============================================================
// Additional tests for missing coverage
// ============================================================

#[test]
fn test_is_dangerous_all_patterns() {
    let tool = AsyncExecTool::new(".", false);
    // Test all known dangerous patterns
    assert!(tool.is_dangerous("rm -rf /"));
    assert!(tool.is_dangerous("rm -rf /*"));
    assert!(tool.is_dangerous("rm -rf ~"));
    assert!(tool.is_dangerous("sudo rm file"));
    assert!(tool.is_dangerous("sudo something"));
    assert!(tool.is_dangerous("mkfs.ext4"));
    assert!(tool.is_dangerous("dd if=/dev/zero"));
    assert!(tool.is_dangerous(":(){ :|:& };:"));
    assert!(tool.is_dangerous("> /dev/sda"));
    assert!(tool.is_dangerous("chmod -R 777 /"));
    assert!(tool.is_dangerous("chown -R user /"));
    assert!(tool.is_dangerous("shutdown"));
    assert!(tool.is_dangerous("reboot"));
    assert!(tool.is_dangerous("halt"));
    assert!(tool.is_dangerous("poweroff"));
    assert!(tool.is_dangerous("init 0"));
    assert!(tool.is_dangerous("init 6"));
    assert!(tool.is_dangerous("format C:"));
    assert!(tool.is_dangerous("del /f /s /q C:"));
    assert!(tool.is_dangerous("rd /s /q C:"));
    assert!(tool.is_dangerous("net user"));
    assert!(tool.is_dangerous("net localgroup administrators"));
}

#[test]
fn test_is_dangerous_safe_commands() {
    let tool = AsyncExecTool::new(".", false);
    assert!(!tool.is_dangerous("echo hello"));
    assert!(!tool.is_dangerous("notepad.exe"));
    assert!(!tool.is_dangerous("python script.py"));
    assert!(!tool.is_dangerous("cargo build"));
    assert!(!tool.is_dangerous("ls -la"));
}

#[test]
fn test_is_dangerous_case_insensitive() {
    let tool = AsyncExecTool::new(".", false);
    assert!(tool.is_dangerous("SHUTDOWN"));
    assert!(tool.is_dangerous("Sudo something"));
    assert!(tool.is_dangerous("REBOOT"));
}

#[test]
fn test_extract_process_name_various() {
    assert_eq!(AsyncExecTool::extract_process_name("notepad.exe"), "notepad");
    assert_eq!(AsyncExecTool::extract_process_name("notepad.exe file.txt"), "notepad");
    assert_eq!(AsyncExecTool::extract_process_name("  calc.exe  "), "calc");
    assert_eq!(AsyncExecTool::extract_process_name(""), "");
    assert_eq!(AsyncExecTool::extract_process_name("python"), "python");
    assert_eq!(AsyncExecTool::extract_process_name("python script.py --arg"), "python");
    assert_eq!(AsyncExecTool::extract_process_name("code ."), "code");
    // Quoted path with spaces: first token after quote stripping is "C:\Program" (split by space)
    assert_eq!(AsyncExecTool::extract_process_name("\"C:\\Program Files\\app.exe\""), "Program");
}

#[test]
fn test_guard_command_restricts_path_traversal() {
    let mut tool = AsyncExecTool::new(".", true);
    tool.set_restrict_to_workspace(true);

    let result = tool.guard_command("cat ../etc/passwd");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("path traversal"));

    let result = tool.guard_command("cat ..\\windows\\system32");
    assert!(result.is_err());
}

#[test]
fn test_guard_command_allows_when_unrestricted() {
    let tool = AsyncExecTool::new(".", false);
    let result = tool.guard_command("cat ../somefile");
    assert!(result.is_ok());
}

#[test]
fn test_tool_interface() {
    let tool = AsyncExecTool::new(".", false);
    assert_eq!(tool.name(), "exec_async");
    assert!(!tool.description().is_empty());
    let params = tool.parameters();
    assert_eq!(params["type"], "object");
    assert!(params["properties"]["command"].is_object());
}

#[tokio::test]
async fn test_set_default_wait() {
    let mut tool = AsyncExecTool::new(".", false);
    tool.set_default_wait(std::time::Duration::from_secs(5));
    // Verify tool still works
    let result = tool
        .execute(&serde_json::json!({"command": "echo test", "wait_seconds": 1}))
        .await;
    assert!(!result.for_llm.contains("blocked"));
}

#[tokio::test]
async fn test_sudo_command_rejected() {
    let tool = AsyncExecTool::new(".", false);
    let result = tool
        .execute(&serde_json::json!({"command": "sudo apt install something"}))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("dangerous"));
}

#[tokio::test]
async fn test_reboot_command_rejected() {
    let tool = AsyncExecTool::new(".", false);
    let result = tool
        .execute(&serde_json::json!({"command": "reboot"}))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("dangerous"));
}

#[tokio::test]
async fn test_custom_working_dir() {
    let dir = tempfile::tempdir().unwrap();
    let tool = AsyncExecTool::new(".", false);

    let result = tool
        .execute(&serde_json::json!({
            "command": "echo custom_wd",
            "working_dir": dir.path().to_string_lossy().to_string(),
            "wait_seconds": 1
        }))
        .await;
    assert!(!result.for_llm.contains("blocked"), "Should not block: {}", result.for_llm);
}

#[test]
fn test_set_allow_patterns_no_op() {
    let mut tool = AsyncExecTool::new(".", false);
    // Should not panic or change behavior
    tool.set_allow_patterns(&["echo"]);
    assert!(tool.guard_command("echo hello").is_ok());
}

#[test]
fn test_new_with_config_default_patterns() {
    let tool = AsyncExecTool::new_with_config(".", false, None, true);
    // Default patterns should block dangerous commands
    assert!(tool.is_dangerous("shutdown"));
    assert!(tool.is_dangerous("rm -rf /"));
    assert!(!tool.is_dangerous("echo hello"));
}

#[test]
fn test_new_with_config_custom_patterns() {
    let tool = AsyncExecTool::new_with_config(
        ".", false,
        Some(&[r"shutdown", r"reboot"]),
        true,
    );
    // Custom patterns: should match the custom regex patterns
    assert!(tool.is_dangerous("shutdown"));
    assert!(tool.is_dangerous("reboot"));
    // "rm -rf /" is NOT in the custom patterns list, so it should be allowed
    assert!(!tool.is_dangerous("rm -rf /"));
}

#[test]
fn test_new_with_config_disabled_patterns() {
    let tool = AsyncExecTool::new_with_config(".", false, None, false);
    // When patterns are disabled, nothing should be dangerous
    assert!(!tool.is_dangerous("shutdown"));
    assert!(!tool.is_dangerous("rm -rf /"));
    assert!(!tool.is_dangerous("reboot"));
}
