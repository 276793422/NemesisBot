use super::*;
    use crate::types::JSONRPC_VERSION;

    #[test]
    fn create_transport() {
        let t = StdioTransport::new("echo", vec![], vec![]);
        assert_eq!(t.name(), "stdio");
        assert!(!t.is_connected());
    }

    #[test]
    fn create_from_config() {
        let config = crate::types::ServerConfig::new("test", "node")
            .arg("server.js")
            .env("FOO=bar")
            .timeout(10);

        let t = StdioTransport::from_config(&config);
        assert_eq!(t.command, "node");
        assert_eq!(t.args, vec!["server.js"]);
        assert_eq!(t.env, vec!["FOO=bar"]);
        assert!(!t.is_connected());
    }

    /// Test connect/close lifecycle with a simple echo-like program.
    /// On Windows, `cmd /C echo` exits immediately, so we just test that
    /// connect succeeds and close cleans up.
    #[tokio::test]
    async fn connect_and_close_lifecycle() {
        // Use a long-running command so the process stays alive during the test.
        // `ping -t localhost` on Windows runs indefinitely.
        #[cfg(target_os = "windows")]
        let mut t = StdioTransport::new("ping", vec!["-t".to_string(), "localhost".to_string()], vec![]);
        #[cfg(not(target_os = "windows"))]
        let mut t = StdioTransport::new("sleep", vec!["60".to_string()], vec![]);

        assert!(!t.is_connected());

        // Connect should succeed.
        t.connect().await.unwrap();
        assert!(t.is_connected());

        // Close should succeed.
        t.close().await.unwrap();
        assert!(!t.is_connected());

        // Double close should be fine.
        t.close().await.unwrap();
        assert!(!t.is_connected());
    }

    #[tokio::test]
    async fn send_when_not_connected_fails() {
        let mut t = StdioTransport::new("nonexistent", vec![], vec![]);
        let req = TransportRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(serde_json::Value::Number(1.into())),
            method: "ping".to_string(),
            params: None,
        };
        let result = t.send(&req, 1000).await;
        assert!(result.is_err());
    }

    /// End-to-end test: spawn a simple JSON-RPC echo server using Python,
    /// send a request, and verify the response. Skips if Python is unavailable.
    #[tokio::test]
    async fn e2e_jsonrpc_echo() {
        // Simple Python script that reads a JSON-RPC request from stdin and
        // echoes back a response with the same id.
        let python_script = r#"
import sys, json
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    try:
        req = json.loads(line)
        resp = {"jsonrpc": "2.0", "id": req.get("id"), "result": {"echo": req.get("method")}}
        sys.stdout.write(json.dumps(resp) + "\n")
        sys.stdout.flush()
    except Exception:
        break
"#;

        let mut t = StdioTransport::new(
            "python",
            vec!["-c".to_string(), python_script.to_string()],
            vec![],
        );

        // Skip if python is not available.
        if t.connect().await.is_err() {
            eprintln!("Skipping e2e test: python not available");
            return;
        }

        let req = TransportRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(serde_json::Value::Number(42.into())),
            method: "test/method".to_string(),
            params: None,
        };

        let resp = t.send(&req, 5000).await.unwrap();
        assert_eq!(resp.id, serde_json::Value::Number(42.into()));
        assert!(resp.result.is_some());
        assert_eq!(resp.result.unwrap()["echo"], "test/method");

        t.close().await.unwrap();
    }

    // ---- New tests ----

    #[test]
    fn transport_name_is_stdio() {
        let t = StdioTransport::new("test", vec![], vec![]);
        assert_eq!(t.name(), "stdio");
    }

    #[test]
    fn new_transport_not_connected() {
        let t = StdioTransport::new("test", vec!["arg1".to_string()], vec!["KEY=VAL".to_string()]);
        assert!(!t.is_connected());
        assert_eq!(t.command, "test");
        assert_eq!(t.args, vec!["arg1"]);
        assert_eq!(t.env, vec!["KEY=VAL"]);
    }

    #[tokio::test]
    async fn close_without_connect_is_ok() {
        let mut t = StdioTransport::new("test", vec![], vec![]);
        t.close().await.unwrap();
        assert!(!t.is_connected());
    }

    #[tokio::test]
    async fn connect_nonexistent_command_fails() {
        let mut t = StdioTransport::new(
            "/absolutely/nonexistent/command/that/does/not/exist",
            vec![],
            vec![],
        );
        let result = t.connect().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn double_connect_is_ok() {
        #[cfg(target_os = "windows")]
        let mut t = StdioTransport::new("ping", vec!["-t".to_string(), "localhost".to_string()], vec![]);
        #[cfg(not(target_os = "windows"))]
        let mut t = StdioTransport::new("sleep", vec!["60".to_string()], vec![]);

        t.connect().await.unwrap();
        assert!(t.is_connected());
        t.connect().await.unwrap(); // Second connect is a no-op
        assert!(t.is_connected());
        t.close().await.unwrap();
    }

    #[test]
    fn from_config_preserves_fields() {
        let config = crate::types::ServerConfig::new("my-server", "/usr/bin/node")
            .arg("index.js")
            .arg("--verbose")
            .env("NODE_ENV=production")
            .env("PORT=3000")
            .timeout(60);

        let t = StdioTransport::from_config(&config);
        assert_eq!(t.command, "/usr/bin/node");
        assert_eq!(t.args, vec!["index.js", "--verbose"]);
        assert_eq!(t.env.len(), 2);
    }

    #[test]
    fn from_config_no_env() {
        let config = crate::types::ServerConfig::new("srv", "cmd");
        let t = StdioTransport::from_config(&config);
        assert!(t.env.is_empty());
    }

    #[tokio::test]
    async fn send_after_close_fails() {
        #[cfg(target_os = "windows")]
        let mut t = StdioTransport::new("ping", vec!["-t".to_string(), "localhost".to_string()], vec![]);
        #[cfg(not(target_os = "windows"))]
        let mut t = StdioTransport::new("sleep", vec!["60".to_string()], vec![]);

        t.connect().await.unwrap();
        t.close().await.unwrap();

        let req = TransportRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(serde_json::Value::Number(1.into())),
            method: "test".to_string(),
            params: None,
        };
        let result = t.send(&req, 1000).await;
        assert!(result.is_err());
    }
