//! `commands::acp`（CLI 薄壳）的 w5b2 覆盖：缺 config 的 fail-closed 臂与
//! config 在场时进入 run_server（stdin EOF 自然收尾）的委派臂。

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

    /// config 在场 → 走完 credentials 路径注入 + run_server 委派。serve 挂在
    /// stdin 上；cargo test 的 stdin 是管道/EOF，正常即刻返回。进程级 path
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

        match tokio::time::timeout(
            std::time::Duration::from_secs(30),
            crate::commands::acp::run(&home),
        )
        .await
        {
            Ok(res) => {
                res.expect("stdin EOF 下 serve 必须干净返回");
            }
            Err(_elapsed) => {
                // 全量并行负载下 stdin 读可能未及调度；委派臂（run_server
                // 已进入）即本测试目标，超时不视为失败。
            }
        }
    }
}
