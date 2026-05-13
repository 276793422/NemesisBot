//! ClamAV TCP client for the clamd daemon.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
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
    pub address: String,
    pub timeout: Duration,
}

impl Client {
    pub fn new(address: &str) -> Self {
        Self {
            address: address.to_string(),
            timeout: Duration::from_secs(30),
        }
    }

    pub fn with_timeout(address: &str, timeout: Duration) -> Self {
        Self {
            address: address.to_string(),
            timeout,
        }
    }

    /// Ping clamd to check if it's alive.
    pub fn ping(&self) -> Result<(), String> {
        let resp = self.send_command("PING")?;
        if resp != "PONG" {
            return Err(format!("unexpected ping response: {}", resp));
        }
        Ok(())
    }

    /// Get clamd version.
    pub fn version(&self) -> Result<String, String> {
        self.send_command("VERSION")
    }

    /// Scan a single file by path.
    pub fn scan_file(&self, file_path: &Path) -> Result<ClamavScanResult, String> {
        let cmd = format!("SCAN {}", file_path.display());
        let resp = self.send_command(&cmd)?;
        Ok(parse_scan_response(&resp))
    }

    /// Scan directory without stopping on infected files.
    pub fn cont_scan(&self, path: &Path) -> Result<Vec<ClamavScanResult>, String> {
        let cmd = format!("CONTSCAN {}", path.display());
        let resp = self.send_command(&cmd)?;
        Ok(parse_multi_scan_response(&resp))
    }

    /// Scan content using the INSTREAM protocol.
    pub fn scan_stream(&self, content: &[u8]) -> Result<ClamavScanResult, String> {
        let mut stream = TcpStream::connect_timeout(&self.address.parse().map_err(|e: std::net::AddrParseError| e.to_string())?, self.timeout)
            .map_err(|e| format!("failed to connect to clamd: {}", e))?;
        stream.set_read_timeout(Some(self.timeout)).ok();
        stream.set_write_timeout(Some(self.timeout)).ok();

        // Send INSTREAM command
        let cmd = b"nINSTREAM\n";
        stream.write_all(cmd).map_err(|e| format!("failed to send INSTREAM: {}", e))?;

        // Stream in chunks with 4-byte big-endian length prefix
        let chunk_size = 32 * 1024;
        let mut offset = 0;
        while offset < content.len() {
            let end = (offset + chunk_size).min(content.len());
            let chunk = &content[offset..end];
            let len = chunk.len() as u32;
            stream.write_all(&len.to_be_bytes()).map_err(|e| format!("write chunk len: {}", e))?;
            stream.write_all(chunk).map_err(|e| format!("write chunk data: {}", e))?;
            offset = end;
        }

        // Send termination (0-length chunk)
        stream.write_all(&0u32.to_be_bytes()).map_err(|e| format!("send termination: {}", e))?;

        // Read response
        stream.set_read_timeout(Some(self.timeout)).ok();
        let mut reader = BufReader::new(stream);
        let mut resp_line = String::new();
        reader.read_line(&mut resp_line).map_err(|e| format!("read response: {}", e))?;

        Ok(parse_scan_response(resp_line.trim()))
    }

    /// Reload the virus database.
    pub fn reload(&self) -> Result<(), String> {
        let resp = self.send_command("RELOAD")?;
        if resp.contains("RELOADING") {
            Ok(())
        } else {
            Err(format!("unexpected reload response: {}", resp))
        }
    }

    /// Get clamd statistics.
    pub fn stats(&self) -> Result<String, String> {
        self.send_command("STATS")
    }

    fn send_command(&self, command: &str) -> Result<String, String> {
        let mut stream = TcpStream::connect_timeout(
            &self.address.parse().map_err(|e: std::net::AddrParseError| e.to_string())?,
            self.timeout,
        ).map_err(|e| format!("connect to clamd at {}: {}", self.address, e))?;

        stream.set_read_timeout(Some(self.timeout)).ok();
        stream.set_write_timeout(Some(self.timeout)).ok();

        let cmd = format!("n{}\n", command);
        stream.write_all(cmd.as_bytes()).map_err(|e| format!("send command: {}", e))?;

        let mut reader = BufReader::new(stream);
        let mut lines = Vec::new();

        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
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
mod tests {
    use super::*;

    #[test]
    fn test_parse_clean_response() {
        let result = parse_scan_response("/tmp/test.txt: OK");
        assert!(result.clean());
        assert_eq!(result.path, "/tmp/test.txt");
    }

    #[test]
    fn test_parse_infected_response() {
        let result = parse_scan_response("/tmp/eicar.com: Eicar-Signature FOUND");
        assert!(!result.clean());
        assert_eq!(result.virus, "Eicar-Signature");
    }

    #[test]
    fn test_parse_error_response() {
        let result = parse_scan_response("/tmp/missing: Access denied ERROR");
        assert!(!result.infected);
    }

    #[test]
    fn test_parse_multi_response() {
        let results = parse_multi_scan_response("/tmp/a.txt: OK\n/tmp/b.exe: Trojan FOUND\n");
        assert_eq!(results.len(), 2);
        assert!(results[0].clean());
        assert!(!results[1].clean());
    }

    #[test]
    fn test_parse_scan_response_found_without_path() {
        let result = parse_scan_response("SomeVirus FOUND");
        assert!(result.infected);
        assert_eq!(result.virus, "SomeVirus");
    }

    #[test]
    fn test_parse_scan_response_empty() {
        let result = parse_scan_response("");
        assert!(!result.infected);
        assert!(result.path.is_empty());
    }

    #[test]
    fn test_parse_scan_response_ok_with_path() {
        let result = parse_scan_response("/some/path/file.exe: OK");
        assert!(!result.infected);
        assert_eq!(result.path, "/some/path/file.exe");
    }

    #[test]
    fn test_parse_scan_response_error() {
        let result = parse_scan_response("/tmp/missing: Access denied ERROR");
        assert!(!result.infected);
        assert!(result.raw.contains("ERROR"));
    }

    #[test]
    fn test_clamav_scan_result_clean_method() {
        let clean = ClamavScanResult {
            path: "/tmp/test.txt".to_string(),
            infected: false,
            virus: String::new(),
            raw: String::new(),
        };
        assert!(clean.clean());

        let infected = ClamavScanResult {
            path: "/tmp/test.exe".to_string(),
            infected: true,
            virus: "Trojan".to_string(),
            raw: String::new(),
        };
        assert!(!infected.clean());
    }

    #[test]
    fn test_client_new() {
        let client = Client::new("127.0.0.1:3310");
        assert_eq!(client.address, "127.0.0.1:3310");
        assert_eq!(client.timeout, Duration::from_secs(30));
    }

    #[test]
    fn test_client_with_timeout() {
        let client = Client::with_timeout("127.0.0.1:3310", Duration::from_secs(120));
        assert_eq!(client.timeout, Duration::from_secs(120));
    }

    #[test]
    fn test_is_single_response_command() {
        assert!(is_single_response_command("PING"));
        assert!(is_single_response_command("VERSION"));
        assert!(is_single_response_command("SCAN /tmp/test.txt"));
        assert!(is_single_response_command("CONTSCAN /tmp"));
        assert!(!is_single_response_command("STATS"));
        assert!(!is_single_response_command("RELOAD"));
    }

    #[test]
    fn test_parse_multi_response_empty_lines() {
        let results = parse_multi_scan_response("\n\n");
        assert!(results.is_empty());
    }

    #[test]
    fn test_parse_multi_response_single_line() {
        let results = parse_multi_scan_response("/tmp/a.txt: OK");
        assert_eq!(results.len(), 1);
        assert!(results[0].clean());
    }

    #[test]
    fn test_clamav_scan_result_debug() {
        let result = ClamavScanResult {
            path: "/tmp/test.txt".to_string(),
            infected: false,
            virus: String::new(),
            raw: "OK".to_string(),
        };
        let debug = format!("{:?}", result);
        assert!(debug.contains("/tmp/test.txt"));
        assert!(debug.contains("OK"));
    }

    #[test]
    fn test_client_ping_fails_when_no_daemon() {
        let client = Client::new("127.0.0.1:13310"); // unlikely port
        let result = client.ping();
        assert!(result.is_err());
    }

    #[test]
    fn test_client_version_fails_when_no_daemon() {
        let client = Client::new("127.0.0.1:13310");
        let result = client.version();
        assert!(result.is_err());
    }

    #[test]
    fn test_client_scan_file_fails_when_no_daemon() {
        let client = Client::new("127.0.0.1:13310");
        let result = client.scan_file(Path::new("/tmp/test.txt"));
        assert!(result.is_err());
    }

    #[test]
    fn test_client_cont_scan_fails_when_no_daemon() {
        let client = Client::new("127.0.0.1:13310");
        let result = client.cont_scan(Path::new("/tmp"));
        assert!(result.is_err());
    }

    #[test]
    fn test_client_scan_stream_fails_when_no_daemon() {
        let client = Client::new("127.0.0.1:13310");
        let result = client.scan_stream(b"test content");
        assert!(result.is_err());
    }

    #[test]
    fn test_client_reload_fails_when_no_daemon() {
        let client = Client::new("127.0.0.1:13310");
        let result = client.reload();
        assert!(result.is_err());
    }

    #[test]
    fn test_client_stats_fails_when_no_daemon() {
        let client = Client::new("127.0.0.1:13310");
        let result = client.stats();
        assert!(result.is_err());
    }

    #[test]
    fn test_client_invalid_address() {
        let client = Client::new("not-a-valid-address");
        let result = client.ping();
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_scan_response_status_ok() {
        let result = parse_scan_response("/path/to/file.doc: OK");
        assert!(!result.infected);
        assert_eq!(result.path, "/path/to/file.doc");
        assert!(result.clean());
    }

    #[test]
    fn test_parse_scan_response_found_with_colon_in_path() {
        // Path with colon (Windows-style)
        let result = parse_scan_response("C:\\Users\\test.exe: Malware FOUND");
        assert!(result.infected);
        // The last ": " split should find the status
    }

    #[test]
    fn test_parse_multi_response_multiple_results() {
        let input = "/tmp/a.txt: OK\n/tmp/b.exe: Trojan FOUND\n/tmp/c.txt: OK";
        let results = parse_multi_scan_response(input);
        assert_eq!(results.len(), 3);
        assert!(results[0].clean());
        assert!(!results[1].clean());
        assert!(results[2].clean());
    }
}
