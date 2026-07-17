    use super::*;

    /// 最小 HTTP mock（std::net，独立线程）：对所有请求回 200 + 固定 JSON。
    /// 最多接 16 个连接后线程退出（够测试用，不永挂）。
    fn start_mock_server() -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            for _ in 0..16 {
                let Ok((mut stream, _)) = listener.accept() else { break };
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let body = r#"{"status":"ok","engaged":false}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });
        port
    }

    fn write_home(home: &std::path::Path, auth_token: &str, web_port: u16) {
        std::fs::create_dir_all(home.join("workspace").join("state")).unwrap();
        std::fs::write(
            home.join("config.json"),
            serde_json::json!({"channels": {"web": {"auth_token": auth_token}}}).to_string(),
        )
        .unwrap();
        std::fs::write(
            home.join("workspace").join("state").join("gateway.json"),
            serde_json::json!({"pid": 123, "web_host": "127.0.0.1", "web_port": web_port})
                .to_string(),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn run_engage_release_status_against_mock() {
        let dir = tempfile::tempdir().unwrap();
        let port = start_mock_server();
        write_home(dir.path(), "secret", port);
        // engage / status / release 三条路径都应 Ok
        assert!(run(dir.path(), false, false).await.is_ok(), "engage");
        assert!(run(dir.path(), false, true).await.is_ok(), "status");
        assert!(run(dir.path(), true, false).await.is_ok(), "release");
    }

    #[tokio::test]
    async fn run_errors_when_config_missing() {
        let dir = tempfile::tempdir().unwrap();
        let r = run(dir.path(), false, false).await;
        assert!(r.is_err(), "缺 config.json 应报错");
    }

    #[tokio::test]
    async fn run_errors_when_gateway_state_missing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.json"),
            serde_json::json!({"channels": {"web": {"auth_token": "x"}}}).to_string(),
        )
        .unwrap();
        let r = run(dir.path(), false, false).await;
        assert!(r.is_err(), "缺 gateway.json 应报错");
    }

    #[tokio::test]
    async fn run_errors_when_gateway_unreachable() {
        let dir = tempfile::tempdir().unwrap();
        // 绑端口拿空闲号、立刻 drop → health check 连不上
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        write_home(dir.path(), "secret", port);
        let r = run(dir.path(), false, false).await;
        assert!(r.is_err(), "gateway 不可达应报错");
    }
