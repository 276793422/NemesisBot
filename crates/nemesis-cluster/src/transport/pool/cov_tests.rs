// transport/pool.rs 覆盖率补充测试（同步池复用活连接臂）。
//
// 豁免：无。

use super::*;

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

/// LOW-10（2026-10-17 四轮审计）：close() 后迟到的 return_connection 不
/// 触碰三账——active_count 不下溢（debug 构建下 usize 回绕会 panic）、
/// 不复活池内条目、许可补还保持信号量自洽。
#[test]
fn return_connection_after_close_is_benign() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let real = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());
    let rt = tokio::runtime::Runtime::new().unwrap();

    let pool = Pool::new(AsyncPoolConfig {
        max_conns: 2,
        max_conns_per_node: 2,
        ..Default::default()
    });

    // close 前取出一条连接（许可被 forget 消耗、active_count=1）。
    let (_key, conn) = rt.block_on(async { pool.get("n1", &real).await.unwrap() });

    // close：池内条目清零、三账归零——**在外连接的账已丢**（这正是缺陷
    // 现场：旧代码迟归还时对已归零的 active_count 做 fetch_sub）。
    pool.close();

    // 迟到的归还：必须走幕帘守卫——关连接 + 补还许可，不碰 active_count。
    pool.return_connection("n1:real".into(), conn);

    let stats = pool.get_stats();
    assert_eq!(
        stats.active_conns, 0,
        "close 后迟归还不得触碰 active_count（旧代码在此下溢）"
    );
    assert_eq!(
        stats.available_slots, 2,
        "许可账自洽：close 归还 0（池内无条目）+ 迟归还补 1 = max"
    );
    assert!(
        !pool.conns.lock().contains_key("n1:real"),
        "close 后迟归还不得复活池内条目"
    );
    assert_eq!(
        pool.semaphore.available_permits(),
        2,
        "真许可账自洽：forget 消耗 1 → close 归还 0（池内无条目）→ 迟归还补 1"
    );
}

// 注：get_with_context（test-only API）已删除——生产零调用方，select 双
// ready 竞态漏许可的分析结论记录于四轮审计报告 §3。
