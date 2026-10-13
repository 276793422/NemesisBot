//! `commands::acp`（CLI 薄壳）的 w5b2 覆盖：缺 config 的 fail-closed 臂与
//! config 在场时进入 run_with_io（注入 EOF 干净收尾）的委派臂。
//!
//! stdin 纪律（docs/REPORT/2026-10-13_stdin-eof-assumption-test-deadlock-
//! fix.md）：测试一律不得触碰进程真实 stdin——旧版直接跑 `run()`（挂真
//! stdin），宿主给 cargo test 一条永不 EOF 的管道时 serve 的 blocking 读
//! 永久占住 stdin 锁，runtime drop 等 blocking 任务 → 测试线程带着
//! GLOBAL_STATE_LOCK 卡死 → 整个测试进程死锁。现走 `run_with_io` 注入
//! duplex：EOF 由测试自己制造，确定性即刻返回，无需超时容忍臂。

#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
mod tests {
    use crate::tests::{EnvHomeGuard, singleton_test_home};

    /// home 无 config.json →「Configuration not found」bail（不隐式 onboard
    /// 纪律）。
    #[tokio::test]
    async fn w5_acp_cmd_missing_config_bails() {
        let tmp = tempfile::tempdir().unwrap();
        let err = crate::commands::acp::run(tmp.path())
            .await
            .expect_err("缺 config 必须 bail");
        assert!(
            err.to_string().contains("Configuration not found"),
            "报错必须点名缺配置: {err}"
        );
    }

    /// config 在场 → 走完 credentials 路径注入 + run_server_with_io 委派，
    /// 注入 EOF（drop duplex client 端）后 serve 必须干净返回。进程级 path
    /// manager 单例先重定向（migrate_nested_session_logs 会触碰 session 根，
    /// 不许落在真实家目录）；run 用独立 temp home（acp::run 不做 pm 一致性
    /// 检查，二者无需同 home）。
    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // GLOBAL_STATE_LOCK 纪律：有意跨 await 持有
    async fn w5_acp_cmd_with_config_enters_server_until_eof() {
        let _guard = crate::GLOBAL_STATE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _pm = singleton_test_home(); // 单例永久指向测试沙箱 home
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join(".nemesisbot");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("config.json"), "{}").unwrap();
        let _env = EnvHomeGuard::point_at(&home);

        // 注入 transport：client 端立刻 drop → server 读端即刻 EOF；出站帧
        // 写 sink（协议帧内容非本测目标）。
        let (_client, server) = tokio::io::duplex(64);
        drop(_client);

        let res = crate::commands::acp::run_with_io(&home, server, tokio::io::sink()).await;
        res.expect("注入 EOF 下 serve 必须干净返回");
    }
}
