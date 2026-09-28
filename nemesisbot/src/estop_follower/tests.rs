//! estop_follower 单测。
//!
//! 覆盖：mirror 四转移（变化才应用）；discover 的发现面契约（无文件 /
//! web_port=0 / 完整形态）；poll_once 对真实 HTTP 往返的解析（最小 TCP
//! 服务桩，断言请求头与 cmd 载荷 + engaged 回传）。spawn 的无限轮询循环
//! 不直测（胶水层，部件已被上三者钉死）。

use super::*;
use std::io::{Read, Write};
use std::sync::Mutex;

/// 最小 HTTP 服务桩：accept 一次，回固定 JSON 响应，并把收到的请求字节
/// 存进捕获器供断言。
fn spawn_one_shot_http(addr_captor: Arc<Mutex<Option<String>>>, body: &'static str) -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).unwrap_or(0);
            *addr_captor.lock().unwrap() = Some(String::from_utf8_lossy(&buf[..n]).into_owned());
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
        }
    });
    port
}

#[test]
fn mirror_applies_only_on_change() {
    let s = nemesis_agent::estop::EstopState::new();
    // false → false：no-op
    assert!(!mirror(&s, false));
    assert!(!s.is_engaged());
    // false → true：engage
    assert!(mirror(&s, true));
    assert!(s.is_engaged());
    // true → true：no-op
    assert!(!mirror(&s, true));
    assert!(s.is_engaged());
    // true → false：release
    assert!(mirror(&s, false));
    assert!(!s.is_engaged());
}

#[test]
fn discover_missing_everything_is_none() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(
        discover(tmp.path()).is_none(),
        "无 config 无 state = 不跟随"
    );
}

#[test]
fn discover_web_port_zero_is_none() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    std::fs::write(
        home.join("config.json"),
        r#"{"channels":{"web":{"auth_token":"tok"}}}"#,
    )
    .unwrap();
    let state_dir = home.join("workspace").join("state");
    std::fs::create_dir_all(&state_dir).unwrap();
    std::fs::write(
        state_dir.join("gateway.json"),
        r#"{"pid":1,"web_host":"127.0.0.1","web_port":0}"#,
    )
    .unwrap();
    assert!(discover(home).is_none(), "web_port=0 = 占位态，不可跟");
}

#[test]
fn discover_full_shape_resolves_url_and_token() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    std::fs::write(
        home.join("config.json"),
        r#"{"channels":{"web":{"auth_token":"sekrit"}}}"#,
    )
    .unwrap();
    let state_dir = home.join("workspace").join("state");
    std::fs::create_dir_all(&state_dir).unwrap();
    std::fs::write(
        state_dir.join("gateway.json"),
        r#"{"pid":42,"web_host":"127.0.0.1","web_port":49000}"#,
    )
    .unwrap();
    let t = discover(home).expect("target");
    assert_eq!(t.base_url, "http://127.0.0.1:49000");
    assert_eq!(t.auth_token, "sekrit");
}

/// poll_with + mirror 端到端：真实 HTTP 往返（请求头/cmd 断言）→ engaged
/// 回传 → 本地 state 被 engage。
#[tokio::test]
async fn poll_once_parses_engaged_and_mirror_engages_local() {
    let captured = Arc::new(Mutex::new(None::<String>));
    let port = spawn_one_shot_http(Arc::clone(&captured), r#"{"status":"ok","engaged":true}"#);
    let target = FollowerTarget {
        base_url: format!("http://127.0.0.1:{port}"),
        auth_token: "sekrit".into(),
    };

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .expect("client");
    let engaged = poll_with(&client, &target).await.expect("poll ok");
    assert_eq!(engaged, Some(true));

    let req = captured.lock().unwrap().clone().expect("request captured");
    assert!(req.contains("POST /api/internal"), "request line: {req}");
    assert!(
        req.contains("x-auth-token: sekrit") || req.contains("X-Auth-Token: sekrit"),
        "token header: {req}"
    );
    assert!(req.contains("estop_status"), "cmd payload: {req}");

    // 镜像落地：本地 state 进入急停。
    let local = nemesis_agent::estop::EstopState::new();
    assert!(mirror(&local, engaged.unwrap()));
    assert!(local.is_engaged());
}

/// 非 2xx（如 token 不匹配 401）→ Err，engaged 无信号。
#[tokio::test]
async fn poll_once_non_2xx_is_err() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(
                b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    });
    let target = FollowerTarget {
        base_url: format!("http://127.0.0.1:{port}"),
        auth_token: "wrong".into(),
    };
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .expect("client");
    assert!(poll_with(&client, &target).await.is_err());
}
