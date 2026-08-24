//! exec_world 测试（P5-2）：通道装配 + 严格闸门接线。
//!
//! Windows 专属（cfg all(sandbox, windows)）：临时 home 下 Sandboxie 永远不
//! 就绪 → 装配走 stdio 通道但闸门已挂；strict=true 时 spawn_and_call 在
//! spawn 之前拒绝。**必须** cfg windows：Linux 上临时 home 的装配结果一样
//! （stdio 通道），但闸门会做 detect_backend 真探测——后端可用时放行、真
//! spawn 一个 executor 子进程，测试就不再是无副作用的结构验证。

#[cfg(all(feature = "sandbox", windows))]
mod strict_channel_refusal {
    use crate::exec_world::build_executor_channel;

    #[tokio::test]
    async fn strict_refuses_when_sandboxie_not_ready_at_construction() {
        // 临时 home：Start.exe 不存在 → will_attach=false → stdio 通道 +
        // Windows 闸门捕获 attached_start_exe=None（构造结果，而非磁盘现状）。
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("config.json"),
            r#"{ "executor": { "enabled": true, "sandbox": true, "strict": true } }"#,
        )
        .expect("seed config.json");
        let store = nemesis_config::ConfigStore::load(&dir.path().join("config.json"))
            .expect("load config store");
        let handle = store.handle();

        let channel = build_executor_channel(dir.path(), dir.path(), handle)
            .expect("build_executor_channel")
            .expect("enabled=true → Some(channel)");

        let ctx = nemesis_agent::context::RequestContext::new("test", "chat", "user", "sess");
        let err = channel
            .spawn_and_call("exec", "{}", &ctx)
            .await
            .expect_err("strict must refuse before spawn");
        assert!(err.contains("strict mode"), "err: {err}");
        // 构造时没挂上盒 → 闸门按「通道无盒」拒绝（提示 start + 重启），
        // 而不是按磁盘上有没有 Start.exe。
        assert!(err.contains("no box attached"), "err: {err}");
        assert!(!err.contains("failed to spawn"), "refusal must precede spawn: {err}");
    }

    #[tokio::test]
    async fn strict_off_keeps_fail_open_gate_wiring() {
        // 同样的不就绪环境，strict=false（默认）→ 闸门秒过。注意不能走
        // spawn_and_call 全程验证：fail-open 下它会真 spawn current_exe——
        // 而 bin 的 test 构建 harness 不跑 main()（executor 短路不生效），
        // 子进程会重跑整套测试（上一版就是这么挂死的）。改为直接调闸门。
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("config.json"),
            r#"{ "executor": { "enabled": true, "sandbox": true } }"#,
        )
        .expect("seed config.json");
        let store = nemesis_config::ConfigStore::load(&dir.path().join("config.json"))
            .expect("load config store");
        let handle = store.handle();

        let channel = build_executor_channel(dir.path(), dir.path(), handle)
            .expect("build_executor_channel")
            .expect("enabled=true → Some(channel)");

        let gate = channel
            .strict_gate
            .as_ref()
            .expect("gate attached on the stdio channel too");
        gate().expect("strict=false → gate passes (fail-open, 现状)");
    }
}
