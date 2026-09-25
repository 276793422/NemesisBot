// transport/pool.rs 覆盖率补充测试（同步池复用活连接臂 / 异步池
// get_with_context 的 dial 超时取消臂）。
//
// 豁免：无。

use super::*;
use std::time::Duration;

/// 同步池：池里存活的连接被 get_or_connect 直接复用（82）——
/// key 指向无监听端口，若走新建连接必失败，Ok 即证明复用臂生效。
#[test]
fn sync_pool_reuses_live_connection() {
    // 先连一个真实监听端点造出活连接。
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let real = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());
    let live = Connection::connect(&real).unwrap();

    let pool = ConnectionPool::new(PoolConfig::default());
    // 活连接挂在"死端口" key 下。
    pool.return_connection("127.0.0.1:1", live);

    let reused = pool.get_or_connect("127.0.0.1:1");
    assert!(reused.is_ok(), "必须复用池内存活连接而不是重连死端口");
}

/// 异步池：get_with_context 在 dial_timeout 内拿不到连接 → 取消臂返回
/// dial timeout（333）。全局信号量被首条连接占满 → 第二次 get 必然悬挂。
#[tokio::test]
async fn get_with_context_dial_timeout_cancels() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();

    let pool = Pool::new(AsyncPoolConfig {
        max_conns: 1,
        max_conns_per_node: 1,
        dial_timeout: Duration::from_millis(300),
        ..Default::default()
    });

    // 占掉全局唯一的信号量配额（permit 被 forget，连接存活即占位）。
    let (_key1, _conn1) = pool.get("n1", &addr).await.unwrap();

    // 不同节点、同一配额 → get_inner 悬挂等信号量 → 外层 dial 超时先到。
    let err = pool.get_with_context("n2", &addr).await.unwrap_err();
    assert!(
        err.contains("dial timeout"),
        "必须走 dial 超时取消臂：{err}"
    );
}
