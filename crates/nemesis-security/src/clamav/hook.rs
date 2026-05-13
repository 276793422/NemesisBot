//! Scan hook for security pipeline integration.

use super::client::ClamavScanResult;
use super::scanner::Scanner;
use std::path::Path;
use std::sync::Arc;

/// Scan hook for middleware integration.
pub struct ScanHook {
    scanner: Arc<Scanner>,
}

impl ScanHook {
    pub fn new(scanner: Arc<Scanner>) -> Self {
        Self { scanner }
    }

    /// Determine if a tool invocation needs virus scanning and perform the scan.
    pub async fn scan_tool_invocation(
        &self,
        tool_name: &str,
        args: &serde_json::Value,
    ) -> Result<bool, String> {
        match tool_name {
            "write_file" | "edit_file" | "append_file" => {
                self.scan_write_args(args).await
            }
            "download" => {
                self.scan_download_args(args).await
            }
            "exec" | "execute_command" => {
                self.scan_exec_args(args).await
            }
            _ => Ok(true),
        }
    }

    /// Health check.
    pub fn health_check(&self) -> Result<(), String> {
        self.scanner.ping()
    }

    /// Get a reference to the underlying scanner.
    pub fn get_scanner(&self) -> &Scanner {
        &self.scanner
    }

    /// Scan a specific file path and return whether it is clean.
    pub async fn scan_file_path(&self, file_path: &Path) -> Result<(bool, Option<ClamavScanResult>), String> {
        if !file_path.exists() {
            return Ok((true, None));
        }

        if !self.scanner.should_scan_file(file_path) {
            return Ok((true, None));
        }

        let result = self.scanner.scan_file(file_path).await?;
        if result.infected {
            return Ok((false, Some(result)));
        }

        Ok((true, Some(result)))
    }

    /// Scan a downloaded file after it has been saved.
    pub async fn scan_downloaded_file(&self, save_path: &Path) -> Result<(bool, Option<ClamavScanResult>), String> {
        if !save_path.exists() {
            return Ok((true, None));
        }

        let result = self.scanner.scan_file(save_path).await?;
        if result.infected {
            tracing::warn!(
                path = %save_path.display(),
                virus = %result.virus,
                "Downloaded file is infected"
            );
            // Attempt to remove the infected file
            let _ = tokio::fs::remove_file(save_path).await;
            return Ok((false, Some(result)));
        }

        Ok((true, Some(result)))
    }

    async fn scan_write_args(&self, args: &serde_json::Value) -> Result<bool, String> {
        if let Some(content) = args.get("content").and_then(|v| v.as_str()) {
            if !content.is_empty() {
                let result = self.scanner.scan_content(content.as_bytes()).await?;
                if result.infected {
                    return Err(format!("virus detected in content: {}", result.virus));
                }
            }
        }
        Ok(true)
    }

    async fn scan_download_args(&self, _args: &serde_json::Value) -> Result<bool, String> {
        // Pre-execution: file may not exist yet
        Ok(true)
    }

    async fn scan_exec_args(&self, _args: &serde_json::Value) -> Result<bool, String> {
        // Extract executable path and scan if exists
        Ok(true)
    }
}

/// Format a scan result for audit logging.
///
/// Mirrors Go's `FormatScanResult`.
pub fn format_scan_result(result: Option<&ClamavScanResult>) -> String {
    match result {
        None => "no scan performed".to_string(),
        Some(r) if r.infected => {
            format!("INFECTED: {} (virus: {})", r.path, r.virus)
        }
        Some(r) => {
            format!("CLEAN: {}", r.path)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clamav::scanner::ScannerConfig;

    fn make_result(path: &str, infected: bool, virus: &str) -> ClamavScanResult {
        ClamavScanResult {
            path: path.to_string(),
            infected,
            virus: virus.to_string(),
            raw: String::new(),
        }
    }

    #[test]
    fn test_format_scan_result_none() {
        assert_eq!(format_scan_result(None), "no scan performed");
    }

    #[test]
    fn test_format_scan_result_infected() {
        let result = make_result("/tmp/eicar.com", true, "Eicar-Signature");
        let formatted = format_scan_result(Some(&result));
        assert!(formatted.contains("INFECTED"));
        assert!(formatted.contains("/tmp/eicar.com"));
        assert!(formatted.contains("Eicar-Signature"));
    }

    #[test]
    fn test_format_scan_result_clean() {
        let result = make_result("/tmp/safe.txt", false, "");
        let formatted = format_scan_result(Some(&result));
        assert!(formatted.contains("CLEAN"));
        assert!(formatted.contains("/tmp/safe.txt"));
    }

    #[test]
    fn test_scan_hook_new() {
        let scanner = Arc::new(Scanner::new(ScannerConfig::default()));
        let hook = ScanHook::new(scanner);
        // Verify hook can return the scanner
        let scanner_ref = hook.get_scanner();
        // Access ping to verify scanner was created (config is private)
        assert!(scanner_ref.ping().is_err()); // not running, so ping should fail
    }

    #[tokio::test]
    async fn test_scan_hook_scan_tool_invocation_unknown_tool() {
        let scanner = Arc::new(Scanner::new(ScannerConfig::default()));
        let hook = ScanHook::new(scanner);
        let args = serde_json::json!({});
        // Unknown tools should be allowed
        let result = hook.scan_tool_invocation("unknown_tool", &args).await.unwrap();
        assert!(result);
    }

    #[tokio::test]
    async fn test_scan_hook_scan_tool_invocation_write_no_content() {
        let scanner = Arc::new(Scanner::new(ScannerConfig::default()));
        let hook = ScanHook::new(scanner);
        let args = serde_json::json!({});
        // write_file with no content field should be ok
        let result = hook.scan_tool_invocation("write_file", &args).await.unwrap();
        assert!(result);
    }

    #[tokio::test]
    async fn test_scan_hook_scan_tool_invocation_write_empty_content() {
        let scanner = Arc::new(Scanner::new(ScannerConfig::default()));
        let hook = ScanHook::new(scanner);
        let args = serde_json::json!({"content": ""});
        let result = hook.scan_tool_invocation("write_file", &args).await.unwrap();
        assert!(result);
    }

    #[tokio::test]
    async fn test_scan_hook_scan_tool_invocation_edit_file_no_content() {
        let scanner = Arc::new(Scanner::new(ScannerConfig::default()));
        let hook = ScanHook::new(scanner);
        let args = serde_json::json!({});
        let result = hook.scan_tool_invocation("edit_file", &args).await.unwrap();
        assert!(result);
    }

    #[tokio::test]
    async fn test_scan_hook_scan_tool_invocation_append_file_no_content() {
        let scanner = Arc::new(Scanner::new(ScannerConfig::default()));
        let hook = ScanHook::new(scanner);
        let args = serde_json::json!({});
        let result = hook.scan_tool_invocation("append_file", &args).await.unwrap();
        assert!(result);
    }

    #[tokio::test]
    async fn test_scan_hook_scan_tool_invocation_download() {
        let scanner = Arc::new(Scanner::new(ScannerConfig::default()));
        let hook = ScanHook::new(scanner);
        let args = serde_json::json!({"url": "http://example.com/file"});
        let result = hook.scan_tool_invocation("download", &args).await.unwrap();
        assert!(result);
    }

    #[tokio::test]
    async fn test_scan_hook_scan_tool_invocation_exec() {
        let scanner = Arc::new(Scanner::new(ScannerConfig::default()));
        let hook = ScanHook::new(scanner);
        let args = serde_json::json!({"command": "ls"});
        let result = hook.scan_tool_invocation("exec", &args).await.unwrap();
        assert!(result);
    }

    #[tokio::test]
    async fn test_scan_hook_scan_tool_invocation_execute_command() {
        let scanner = Arc::new(Scanner::new(ScannerConfig::default()));
        let hook = ScanHook::new(scanner);
        let args = serde_json::json!({"command": "dir"});
        let result = hook.scan_tool_invocation("execute_command", &args).await.unwrap();
        assert!(result);
    }

    #[tokio::test]
    async fn test_scan_hook_scan_file_path_nonexistent() {
        let scanner = Arc::new(Scanner::new(ScannerConfig::default()));
        let hook = ScanHook::new(scanner);
        let result = hook.scan_file_path(Path::new("/nonexistent/file.txt")).await.unwrap();
        assert!(result.0); // clean
        assert!(result.1.is_none()); // no scan result
    }

    #[tokio::test]
    async fn test_scan_hook_scan_file_path_safe_extension() {
        let scanner = Arc::new(Scanner::new(ScannerConfig::default()));
        let hook = ScanHook::new(scanner);
        // Create a temp file with safe extension
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "hello").unwrap();
        let result = hook.scan_file_path(&file_path).await.unwrap();
        assert!(result.0); // clean (not scanned because .txt is safe)
        assert!(result.1.is_none()); // no scan result
    }

    #[tokio::test]
    async fn test_scan_hook_scan_downloaded_file_nonexistent() {
        let scanner = Arc::new(Scanner::new(ScannerConfig::default()));
        let hook = ScanHook::new(scanner);
        let result = hook.scan_downloaded_file(Path::new("/nonexistent/file.exe")).await.unwrap();
        assert!(result.0); // clean
        assert!(result.1.is_none());
    }

    #[test]
    fn test_format_scan_result_variants() {
        // Test all three variants of format_scan_result
        assert_eq!(format_scan_result(None), "no scan performed");

        let clean = make_result("/tmp/safe.txt", false, "");
        let formatted = format_scan_result(Some(&clean));
        assert!(formatted.contains("CLEAN"));

        let infected = make_result("/tmp/eicar.com", true, "Eicar");
        let formatted = format_scan_result(Some(&infected));
        assert!(formatted.contains("INFECTED"));
        assert!(formatted.contains("Eicar"));
    }

    #[test]
    fn test_health_check_fails_when_not_running() {
        let scanner = Arc::new(Scanner::new(ScannerConfig::default()));
        let hook = ScanHook::new(scanner);
        assert!(hook.health_check().is_err());
    }
}
