//! [`qmp`] 单测：纯函数分类 + 对 fake-qmp 真服务的握手/执行/错误/超时面。
//! 全部用户态（无 KVM/WHPX 依赖），CI 可跑。

use std::time::Duration;

use super::*;
use crate::vm::qmp::QmpLine;

#[test]
fn classify_greeting_event_return_error() {
    assert_eq!(
        classify_line(r#"{"QMP":{"version":{}}}"#).unwrap(),
        QmpLine::Greeting
    );
    assert_eq!(
        classify_line(r#"{"event":"RESET","data":{}}"#).unwrap(),
        QmpLine::Event
    );
    assert_eq!(
        classify_line(r#"{"return":{}}"#).unwrap(),
        QmpLine::Response(Ok(serde_json::json!({})))
    );
    assert!(matches!(
        classify_line(r#"{"error":{"class":"GenericError","desc":"x"}}"#).unwrap(),
        QmpLine::Response(Err(_))
    ));
    // 未知形态 = 诚实错误带原文（不猜）。
    assert!(classify_line("garbage").is_err());
    assert!(classify_line(r#"{"something-else":1}"#).is_err());
}

async fn connected(handle: &fake_qmp::FakeQmpHandle) -> QmpClient {
    QmpClient::connect(handle.addr(), Duration::from_secs(5))
        .await
        .expect("connect + handshake")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handshake_and_execute_roundtrip() {
    let handle = fake_qmp::spawn().await.expect("spawn fake-qmp");
    let mut client = connected(&handle).await;

    let ret = client
        .execute("savevm", None, Duration::from_secs(5))
        .await
        .expect("savevm ok");
    assert_eq!(ret, serde_json::json!({}));

    let status = client.query_status(Duration::from_secs(5)).await.unwrap();
    assert_eq!(status, "running");

    // 已收命令记账：握手 + savevm + query-status 真的发到了服务端。
    let cmds = handle.commands();
    assert!(cmds.contains(&"qmp_capabilities".to_string()), "{cmds:?}");
    assert!(cmds.contains(&"savevm".to_string()), "{cmds:?}");
    assert!(cmds.contains(&"query-status".to_string()), "{cmds:?}");
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn command_rejection_surfaces_class_and_desc() {
    let handle = fake_qmp::spawn().await.expect("spawn fake-qmp");
    let mut client = connected(&handle).await;
    let err = client
        .execute("no-such-command", None, Duration::from_secs(5))
        .await
        .unwrap_err();
    match err {
        QmpError::Protocol(m) => {
            assert!(m.contains("CommandNotFound"), "{m}");
            assert!(m.contains("has not been found"), "{m}");
        }
        other => panic!("expected Protocol, got {other:?}"),
    }
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn error_injection_surfaces_class_and_desc() {
    let handle = fake_qmp::spawn().await.expect("spawn fake-qmp");
    handle.set_behavior(
        "savevm",
        fake_qmp::Behavior::err("GenericError", "snapshot disk failure (injected)"),
    );
    let mut client = connected(&handle).await;
    let err = client
        .execute("savevm", None, Duration::from_secs(5))
        .await
        .unwrap_err();
    match err {
        QmpError::Protocol(m) => assert!(m.contains("injected"), "{m}"),
        other => panic!("expected Protocol, got {other:?}"),
    }
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hang_budget_times_out_honestly() {
    let handle = fake_qmp::spawn().await.expect("spawn fake-qmp");
    handle.set_behavior("savevm", fake_qmp::Behavior::Hang);
    let mut client = connected(&handle).await;
    let started = std::time::Instant::now();
    let err = client
        .execute("savevm", None, Duration::from_millis(400))
        .await
        .unwrap_err();
    assert!(matches!(err, QmpError::Timeout), "{err:?}");
    assert!(started.elapsed() >= Duration::from_millis(350));
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dead_server_gives_io_or_connect_not_hang() {
    // 杀 QEMU（此处 = fake 服务关停）后的诚实错误面：连接失败 / IO 错 /
    // 预算内超时——不挂死、不吞错。平台分岔（本机实测 2026-10-17）：
    // Linux 对死端口 connect 快速 ECONNREFUSED；Windows 对刚关闭的
    // listener 端口 SYN 黑洞（连 RST 都不给）→ 诚实超时。两者都受
    // connect 预算约束——本质断言 = 有界时间 + 诚实错误。
    let handle = fake_qmp::spawn().await.expect("spawn fake-qmp");
    let addr = handle.addr();
    handle.shutdown().await;
    // `.err()` 规避 QmpClient 非 Debug（只需要错误面）。
    let started = std::time::Instant::now();
    let err = QmpClient::connect(addr, Duration::from_secs(2))
        .await
        .err()
        .expect("connect should fail");
    assert!(
        matches!(
            err,
            QmpError::Connect(_) | QmpError::Io(_) | QmpError::Timeout
        ),
        "{err:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(5), "有界失败");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connect_timeout_honest() {
    // 不可达地址（保留段黑洞端口）：连接预算超时 = Timeout。
    let started = std::time::Instant::now();
    let err = QmpClient::connect("127.0.0.1:1".parse().unwrap(), Duration::from_millis(500))
        .await
        .err()
        .expect("connect should fail");
    assert!(
        matches!(err, QmpError::Connect(_) | QmpError::Timeout),
        "{err:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(5), "不挂死");
}

#[test]
fn hmp_error_detection_pure() {
    // HMP 成功：空输出 / 非错误文本。
    assert!(!hmp_output_is_error(""));
    assert!(!hmp_output_is_error("\n"));
    assert!(!hmp_output_is_error("info: saved 2048 MB\n"));
    // HMP 失败：首非空行 Error 开头（QEMU 多版本一致形态）。
    assert!(hmp_output_is_error("Error: Error while writing VM state"));
    assert!(hmp_output_is_error("\n  Error: snapshot failed\n"));
    // `Errors occurred`（复数名词开头）不是错误行——不误伤。
    assert!(!hmp_output_is_error("Errors occurred: none"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn human_monitor_success_and_migrate_face() {
    let handle = fake_qmp::spawn().await.expect("spawn fake-qmp");
    let mut client = connected(&handle).await;

    // savevm 走 HMP 桥：成功 = 空输出。
    let out = client
        .human_monitor("savevm p5sleep", Duration::from_secs(5))
        .await
        .expect("savevm via HMP ok");
    assert_eq!(out, "");

    // migrate 受理 + 状态轮询。
    client
        .migrate("file:/tmp/vm.state", Duration::from_secs(5))
        .await
        .expect("migrate accepted");
    let status = client.query_migrate(Duration::from_secs(5)).await.unwrap();
    assert_eq!(status, "completed");

    let cmds = handle.commands();
    assert!(
        cmds.contains(&"human-monitor-command".to_string()),
        "{cmds:?}"
    );
    assert!(cmds.contains(&"migrate".to_string()), "{cmds:?}");
    assert!(cmds.contains(&"query-migrate".to_string()), "{cmds:?}");
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hmp_text_error_surfaces_as_protocol() {
    // HMP 失败形态：error 不以 error 对象出现，而是 return 字符串里
    // `Error:` 文本——诚实转 Protocol（带原文），不吞不猜成功。
    let handle = fake_qmp::spawn().await.expect("spawn fake-qmp");
    handle.set_behavior(
        "human-monitor-command",
        fake_qmp::Behavior::Return(serde_json::Value::String(
            "Error: Could not create VM snapshot\n".into(),
        )),
    );
    let mut client = connected(&handle).await;
    let err = client
        .human_monitor("savevm p5sleep", Duration::from_secs(5))
        .await
        .unwrap_err();
    match err {
        QmpError::Protocol(m) => {
            assert!(m.contains("Could not create VM snapshot"), "{m}");
        }
        other => panic!("expected Protocol, got {other:?}"),
    }
    handle.shutdown().await;
}
