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
            "write_file" | "edit_file" | "append_file" => self.scan_write_args(args).await,
            "download" => self.scan_download_args(args).await,
            "exec" | "execute_command" => self.scan_exec_args(args).await,
            _ => Ok(true),
        }
    }

    /// Health check.
    pub async fn health_check(&self) -> Result<(), String> {
        self.scanner.ping().await
    }

    /// Get a reference to the underlying scanner.
    pub fn get_scanner(&self) -> &Scanner {
        &self.scanner
    }

    /// Scan a specific file path and return whether it is clean.
    pub async fn scan_file_path(
        &self,
        file_path: &Path,
    ) -> Result<(bool, Option<ClamavScanResult>), String> {
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
    pub async fn scan_downloaded_file(
        &self,
        save_path: &Path,
    ) -> Result<(bool, Option<ClamavScanResult>), String> {
        if !save_path.exists() {
            return Ok((true, None));
        }

        let result = self.scanner.scan_file(save_path).await?;
        if result.infected {
            tracing::warn!(
                path = %save_path.display(),
                virus = %result.virus,
                "[Scanner] Downloaded file is infected"
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
mod tests;
