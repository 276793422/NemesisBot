//! ClamAV TCP client for the clamd daemon.

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

/// Scan result from clamd.
#[derive(Debug, Clone)]
pub struct ClamavScanResult {
    pub path: String,
    pub infected: bool,
    pub virus: String,
    pub raw: String,
}

impl ClamavScanResult {
    pub fn clean(&self) -> bool {
        !self.infected
    }
}

/// TCP client for clamd.
pub struct Client {
    address: String,
    timeout: Duration,
    socket_addr: SocketAddr,
}

impl Client {
    pub fn new(address: &str) -> Self {
        let socket_addr: SocketAddr = address.parse().unwrap_or("127.0.0.1:3310".parse().unwrap());
        Self {
            address: address.to_string(),
            timeout: Duration::from_secs(30),
            socket_addr,
        }
    }

    pub fn with_timeout(address: &str, timeout: Duration) -> Self {
        let socket_addr: SocketAddr = address.parse().unwrap_or("127.0.0.1:3310".parse().unwrap());
        Self {
            address: address.to_string(),
            timeout,
            socket_addr,
        }
    }

    pub fn address(&self) -> &str {
        &self.address
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Ping clamd to check if it's alive.
    pub async fn ping(&self) -> Result<(), String> {
        let resp = self.send_command("PING").await?;
        if resp != "PONG" {
            return Err(format!("unexpected ping response: {}", resp));
        }
        Ok(())
    }

    /// Get clamd version.
    pub async fn version(&self) -> Result<String, String> {
        self.send_command("VERSION").await
    }

    /// Scan a single file by path.
    pub async fn scan_file(&self, file_path: &Path) -> Result<ClamavScanResult, String> {
        // clamd runs with its own working directory (clamav_path, set in
        // daemon.rs), so a *relative* path is resolved against clamd's cwd,
        // not ours. clamd then can't find the file and returns a not-found
        // response, which `parse_scan_response` reads as CLEAN (infected=false)
        // — a silent false-negative. Force an absolute path. On Windows,
        // canonicalize() prepends the `\\?\` verbatim prefix that clamd rejects
        // on the SCAN command line, so strip it.
        let abs = match std::fs::canonicalize(file_path) {
            Ok(p) => {
                let s = p.to_string_lossy().into_owned();
                std::path::PathBuf::from(s.strip_prefix(r#"\\?\"#).unwrap_or(&s).to_string())
            }
            Err(_) => file_path.to_path_buf(),
        };
        let cmd = format!("SCAN {}", abs.display());
        let resp = self.send_command(&cmd).await?;
        Ok(parse_scan_response(&resp))
    }

    /// Scan directory without stopping on infected files.
    pub async fn cont_scan(&self, path: &Path) -> Result<Vec<ClamavScanResult>, String> {
        let cmd = format!("CONTSCAN {}", path.display());
        let resp = self.send_command(&cmd).await?;
        Ok(parse_multi_scan_response(&resp))
    }

    /// Scan content using the INSTREAM protocol.
    pub async fn scan_stream(&self, content: &[u8]) -> Result<ClamavScanResult, String> {
        let result = tokio::time::timeout(self.timeout, async {
            let mut stream = TcpStream::connect(&self.socket_addr)
                .await
                .map_err(|e| format!("failed to connect to clamd: {}", e))?;

            // Send INSTREAM command
            let cmd = b"nINSTREAM\n";
            stream.write_all(cmd).await.map_err(|e| format!("failed to send INSTREAM: {}", e))?;

            // Stream in chunks with 4-byte big-endian length prefix
            let chunk_size = 32 * 1024;
            let mut offset = 0;
            while offset < content.len() {
                let end = (offset + chunk_size).min(content.len());
                let chunk = &content[offset..end];
                let len = chunk.len() as u32;
                stream.write_all(&len.to_be_bytes()).await.map_err(|e| format!("write chunk len: {}", e))?;
                stream.write_all(chunk).await.map_err(|e| format!("write chunk data: {}", e))?;
                offset = end;
            }

            // Send termination (0-length chunk)
            stream.write_all(&0u32.to_be_bytes()).await.map_err(|e| format!("send termination: {}", e))?;

            // Read response
            let mut reader = BufReader::new(stream);
            let mut resp_line = String::new();
            reader.read_line(&mut resp_line).await.map_err(|e| format!("read response: {}", e))?;

            Ok::<ClamavScanResult, String>(parse_scan_response(resp_line.trim()))
        })
        .await
        .map_err(|_| "scan_stream timed out".to_string())?;

        result
    }

    /// Reload the virus database.
    pub async fn reload(&self) -> Result<(), String> {
        let resp = self.send_command("RELOAD").await?;
        if resp.contains("RELOADING") {
            Ok(())
        } else {
            Err(format!("unexpected reload response: {}", resp))
        }
    }

    /// Get clamd statistics.
    pub async fn stats(&self) -> Result<String, String> {
        self.send_command("STATS").await
    }

    async fn send_command(&self, command: &str) -> Result<String, String> {
        let stream = tokio::time::timeout(self.timeout, TcpStream::connect(&self.socket_addr))
            .await
            .map_err(|_| format!("connect to clamd at {} timed out", self.address))?
            .map_err(|e| format!("connect to clamd at {}: {}", self.address, e))?;

        let cmd = format!("n{}\n", command);
        let (read_half, mut write_half) = stream.into_split();
        write_half.write_all(cmd.as_bytes()).await.map_err(|e| format!("send command: {}", e))?;

        let mut reader = BufReader::new(read_half);
        let mut lines = Vec::new();

        loop {
            let mut line = String::new();
            match reader.read_line(&mut line).await {
                Ok(0) => break,
                Ok(_) => {
                    let trimmed = line.trim().to_string();
                    if !trimmed.is_empty() {
                        lines.push(trimmed);
                    }
                    if is_single_response_command(command) {
                        break;
                    }
                }
                Err(_) => break,
            }
        }

        if lines.is_empty() {
            return Err("empty response from clamd".to_string());
        }

        Ok(lines.join("\n"))
    }
}

fn is_single_response_command(cmd: &str) -> bool {
    matches!(cmd, "PING" | "VERSION") || cmd.starts_with("SCAN ") || cmd.starts_with("CONTSCAN ")
}

fn parse_scan_response(raw: &str) -> ClamavScanResult {
    let raw = raw.to_string();

    if raw.ends_with(" ERROR") {
        return ClamavScanResult {
            path: String::new(),
            infected: false,
            virus: String::new(),
            raw,
        };
    }

    if let Some(idx) = raw.rfind(": ") {
        let path = raw[..idx].to_string();
        let status = &raw[idx + 2..];

        if status.ends_with(" FOUND") {
            return ClamavScanResult {
                path,
                infected: true,
                virus: status.trim_end_matches(" FOUND").to_string(),
                raw,
            };
        } else if status == "OK" {
            return ClamavScanResult {
                path,
                infected: false,
                virus: String::new(),
                raw,
            };
        }
    }

    if raw.ends_with(" FOUND") {
        return ClamavScanResult {
            path: String::new(),
            infected: true,
            virus: raw.trim_end_matches(" FOUND").to_string(),
            raw,
        };
    }

    ClamavScanResult {
        path: String::new(),
        infected: false,
        virus: String::new(),
        raw,
    }
}

fn parse_multi_scan_response(raw: &str) -> Vec<ClamavScanResult> {
    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| parse_scan_response(l.trim()))
        .collect()
}

#[cfg(test)]
mod tests;
