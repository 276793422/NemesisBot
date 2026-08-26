//! S7 冲刺覆盖测试：child_mode.rs 的纯逻辑/失败路径 —
//! 非子模式拒绝、plugin-ui DLL 缺失报错（不加载真 DLL）、WS 连接失败
//! 日志路径。真 DLL 加载 / 真窗口创建属结构性不可达，见 S7 报告。

use super::*;

#[tokio::test]
async fn s7_run_child_mode_without_flag_errors() {
    // 测试二进制不带 --multiple 参数（cargo test 的 argv 注定不含它）。
    let err = run_child_mode().await.unwrap_err();
    assert_eq!(err, "not in child mode");
}

#[test]
fn s7_load_and_run_plugin_window_missing_library_errors() {
    // 测试二进制目录（target/...）下没有 plugins/plugin_ui.dll，
    // 查找失败发生在任何 dlopen 之前 —— 无真实 DLL/窗口副作用。
    let err = load_and_run_plugin_window(
        "dashboard",
        &serde_json::json!({"token": "t", "web_port": 1, "web_host": "127.0.0.1"}),
        "key",
        49000,
        "/ws",
    )
    .unwrap_err();
    assert!(
        err.contains("plugin-ui library not found"),
        "unexpected error: {}",
        err
    );
}

#[test]
fn s7_connect_ws_with_handler_reports_connect_failure() {
    // 先占一个端口再释放，保证目标端口上没有任何监听者。
    let port = {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            l.local_addr().unwrap().port()
        }) // listener drop -> 端口空闲
    };

    let handle = connect_ws_with_handler("some-key", port, "/ws", false);
    assert!(handle.is_some());

    // 后台线程会尝试连接（connection refused）并走失败日志分支，
    // block_on 随即结束、线程退出。
    std::thread::sleep(std::time::Duration::from_millis(300));

    // 收尾：置 shutdown 标志并关客户端（幂等、无副作用）。
    handle.unwrap().close();
}
