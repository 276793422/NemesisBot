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
    assert_eq!(client.address(), "127.0.0.1:3310");
    assert_eq!(client.timeout(), Duration::from_secs(30));
}

#[test]
fn test_client_with_timeout() {
    let client = Client::with_timeout("127.0.0.1:3310", Duration::from_secs(120));
    assert_eq!(client.timeout(), Duration::from_secs(120));
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

#[tokio::test]
async fn test_client_ping_fails_when_no_daemon() {
    let client = Client::new("127.0.0.1:13310"); // unlikely port
    let result = client.ping().await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_client_version_fails_when_no_daemon() {
    let client = Client::new("127.0.0.1:13310");
    let result = client.version().await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_client_scan_file_fails_when_no_daemon() {
    let client = Client::new("127.0.0.1:13310");
    let result = client.scan_file(Path::new("/tmp/test.txt")).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_client_cont_scan_fails_when_no_daemon() {
    let client = Client::new("127.0.0.1:13310");
    let result = client.cont_scan(Path::new("/tmp")).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_client_scan_stream_fails_when_no_daemon() {
    let client = Client::new("127.0.0.1:13310");
    let result = client.scan_stream(b"test content").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_client_reload_fails_when_no_daemon() {
    let client = Client::new("127.0.0.1:13310");
    let result = client.reload().await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_client_stats_fails_when_no_daemon() {
    let client = Client::new("127.0.0.1:13310");
    let result = client.stats().await;
    assert!(result.is_err());
}

#[test]
fn test_client_invalid_address() {
    let client = Client::new("not-a-valid-address");
    // With invalid address, the socket_addr falls back to default
    assert_eq!(client.address(), "not-a-valid-address");
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

// ============================================================
// Additional coverage tests
// ============================================================

#[test]
fn test_parse_scan_response_windows_path() {
    let result = parse_scan_response("C:\\Users\\test\\file.exe: Win32.Trojan FOUND");
    assert!(result.infected);
    assert_eq!(result.virus, "Win32.Trojan");
    assert!(result.raw.contains("FOUND"));
}

#[test]
fn test_parse_scan_response_path_with_spaces() {
    let result = parse_scan_response("/path/to/my file.txt: OK");
    assert!(!result.infected);
    assert_eq!(result.path, "/path/to/my file.txt");
    assert!(result.clean());
}

#[test]
fn test_parse_scan_response_found_with_complex_virus_name() {
    let result = parse_scan_response("/tmp/file: Win.Trojan.Agent-12345 FOUND");
    assert!(result.infected);
    assert_eq!(result.virus, "Win.Trojan.Agent-12345");
}

#[test]
fn test_parse_multi_response_with_errors() {
    let input = "/tmp/a.txt: OK\n/tmp/missing: Access denied ERROR\n/tmp/c.exe: Worm FOUND";
    let results = parse_multi_scan_response(input);
    assert_eq!(results.len(), 3);
    assert!(results[0].clean());
    assert!(!results[1].infected);
    assert!(results[2].infected);
}

#[test]
fn test_client_new_default_timeout() {
    let client = Client::new("127.0.0.1:3310");
    assert_eq!(client.timeout(), Duration::from_secs(30));
}

#[test]
fn test_client_with_custom_timeout() {
    let client = Client::with_timeout("127.0.0.1:3310", Duration::from_secs(300));
    assert_eq!(client.timeout(), Duration::from_secs(300));
    assert_eq!(client.address(), "127.0.0.1:3310");
}

#[test]
fn test_client_address_storage() {
    let client = Client::new("10.0.0.1:9999");
    assert_eq!(client.address(), "10.0.0.1:9999");
}

#[test]
fn test_clamav_scan_result_fields() {
    let result = ClamavScanResult {
        path: "/test/file.exe".to_string(),
        infected: true,
        virus: "TestVirus".to_string(),
        raw: "/test/file.exe: TestVirus FOUND".to_string(),
    };
    assert_eq!(result.path, "/test/file.exe");
    assert!(result.infected);
    assert!(!result.clean());
    assert_eq!(result.virus, "TestVirus");
}

#[test]
fn test_parse_scan_response_only_found_no_path() {
    let result = parse_scan_response("Malware.Generic FOUND");
    assert!(result.infected);
    assert_eq!(result.virus, "Malware.Generic");
    assert!(result.path.is_empty());
}

#[test]
fn test_parse_scan_response_unrecognized_format() {
    // Text that doesn't match any known pattern
    let result = parse_scan_response("some random text without format");
    assert!(!result.infected);
    assert!(result.path.is_empty());
    assert!(result.virus.is_empty());
    assert_eq!(result.raw, "some random text without format");
}

#[test]
fn test_is_single_response_command_various() {
    // Single response commands
    assert!(is_single_response_command("PING"));
    assert!(is_single_response_command("VERSION"));
    assert!(is_single_response_command("SCAN /tmp/test"));
    assert!(is_single_response_command("CONTSCAN /tmp"));

    // Multi-line response commands
    assert!(!is_single_response_command("STATS"));
    assert!(!is_single_response_command("RELOAD"));
    assert!(!is_single_response_command("UNKNOWN"));
}

#[test]
fn test_parse_multi_response_trailing_newlines() {
    let results = parse_multi_scan_response("/tmp/a.txt: OK\n\n\n");
    assert_eq!(results.len(), 1);
    assert!(results[0].clean());
}

#[test]
fn test_parse_multi_response_tabs_and_spaces() {
    let results = parse_multi_scan_response("  /tmp/a.txt: OK  \n\t/tmp/b.exe: Trojan FOUND\t");
    assert_eq!(results.len(), 2);
}

#[tokio::test]
async fn test_client_scan_stream_fails_no_daemon() {
    let client = Client::new("127.0.0.1:13310");
    let result = client.scan_stream(b"test data").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_client_scan_stream_with_custom_timeout() {
    let client = Client::with_timeout("127.0.0.1:13310", Duration::from_millis(100));
    let result = client.scan_stream(b"test data").await;
    assert!(result.is_err());
}

#[test]
fn test_parse_scan_response_error_various() {
    // Different error types
    let result = parse_scan_response("/tmp/file: Can't access file ERROR");
    assert!(!result.infected);
    assert!(result.raw.contains("ERROR"));

    let result = parse_scan_response("/tmp/file: lstat() failed ERROR");
    assert!(!result.infected);
}

#[test]
fn test_clamav_scan_result_clean_and_infected() {
    let clean = ClamavScanResult {
        path: "/clean.txt".to_string(),
        infected: false,
        virus: String::new(),
        raw: "/clean.txt: OK".to_string(),
    };
    assert!(clean.clean());
    assert!(!clean.infected);

    let infected = ClamavScanResult {
        path: "/infected.exe".to_string(),
        infected: true,
        virus: "Trojan".to_string(),
        raw: "/infected.exe: Trojan FOUND".to_string(),
    };
    assert!(!infected.clean());
    assert!(infected.infected);
}

#[test]
fn test_client_socket_addr_fallback_on_invalid() {
    let client = Client::new("invalid-addr");
    // Should not panic, falls back to default socket addr
    assert_eq!(client.address(), "invalid-addr");
}

#[test]
fn test_parse_scan_response_only_ok() {
    let result = parse_scan_response("OK");
    assert!(!result.infected);
}

#[test]
fn test_client_new_with_localhost() {
    let client = Client::new("127.0.0.1:3310");
    assert_eq!(client.address(), "127.0.0.1:3310");
}

#[test]
fn test_client_new_with_tcp_prefix() {
    let client = Client::new("tcp://127.0.0.1:3310");
    assert_eq!(client.address(), "tcp://127.0.0.1:3310");
}

#[test]
fn test_clamav_scan_result_construct() {
    let result = ClamavScanResult {
        path: "/test/path".to_string(),
        infected: false,
        virus: String::new(),
        raw: "/test/path: OK".to_string(),
    };
    assert_eq!(result.path, "/test/path");
    assert!(result.clean());
}

#[test]
fn test_parse_scan_response_multiple_colons_in_path() {
    let result = parse_scan_response("C:\\Users\\test:file: Win.Trojan FOUND");
    assert!(result.infected);
    assert_eq!(result.virus, "Win.Trojan");
}

#[test]
fn test_parse_scan_response_ok_file() {
    let result = parse_scan_response("/home/user/document.pdf: OK");
    assert!(!result.infected);
    assert!(result.clean());
    assert!(result.raw.contains("OK"));
}

#[tokio::test]
async fn test_client_scan_file_connection_refused() {
    let client = Client::new("127.0.0.1:1");
    let result = client.scan_file(Path::new("/tmp/nonexistent")).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_client_version_connection_refused() {
    let client = Client::new("127.0.0.1:1");
    let result = client.version().await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_client_ping_connection_refused() {
    let client = Client::new("127.0.0.1:1");
    let result = client.ping().await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_client_stats_connection_refused() {
    let client = Client::new("127.0.0.1:1");
    let result = client.stats().await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_client_reload_connection_refused() {
    let client = Client::new("127.0.0.1:1");
    let result = client.reload().await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_client_cont_scan_connection_refused() {
    let client = Client::new("127.0.0.1:1");
    let result = client.cont_scan(Path::new("/tmp/nonexistent")).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_client_scan_stream_connection_refused() {
    let client = Client::new("127.0.0.1:1");
    let result = client.scan_stream(b"test content").await;
    assert!(result.is_err());
}
