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
        assert!(
            !err.contains("failed to spawn"),
            "refusal must precede spawn: {err}"
        );
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

/// world 装配描述符（M2 补测，2026-08-25）：enabled → Some + stdio 语义 +
/// 写守卫根 + Spawn 车道 cwd 守卫（拒在 spawn 之前）。Tool 车道归一化不在此
/// 测——spawn_and_call 会 spawn current_exe（测试 harness 二进制会重跑整套
/// 测试，见上方 strict 测试的注释），真链路由 tests/executor.rs 覆盖。
#[cfg(all(feature = "sandbox", windows))]
mod world_descriptors {
    use crate::exec_world::build_workflow_world;
    use nemesis_sandbox::exec_world::{ExecOp, ExecutionWorld, SpawnOp, SpawnSemantics};

    fn seed_home(config: &str) -> (tempfile::TempDir, nemesis_config::ConfigHandle) {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("config.json"), config).expect("seed config.json");
        let store = nemesis_config::ConfigStore::load(&dir.path().join("config.json"))
            .expect("load config store");
        (dir, store.handle())
    }

    #[tokio::test]
    async fn build_workflow_world_some_with_stdio_semantics_when_enabled() {
        let (dir, handle) = seed_home(r#"{ "executor": { "enabled": true, "sandbox": true } }"#);
        let world = build_workflow_world(
            dir.path(),
            dir.path(),
            vec![dir.path().join("w")],
            vec![dir.path().join("s")],
            handle,
        )
        .expect("build")
        .expect("enabled=true → Some(world)");

        assert_eq!(world.name(), "executor-channel");
        assert!(
            world.supports_tool_calls(),
            "executor world has the tool lane"
        );
        // 临时 home 下 Sandboxie 未就绪 → 通道无 Start.exe → 描述轴 = ExecutorChild。
        assert_eq!(world.spawn_semantics(), SpawnSemantics::ExecutorChild);
        assert_eq!(world.writable_roots(), vec![dir.path().join("w")]);
        // 默认写守卫：根内 Ok / 根外 Err
        assert!(world.check_writable(&dir.path().join("w/def.json")).is_ok());
        assert!(
            world
                .check_writable(&dir.path().join("elsewhere/def.json"))
                .is_err()
        );

        // Spawn 车道 cwd 守卫：根外 cwd 在 spawn 之前被拒（不会真起进程）。
        let err = world
            .run(ExecOp::Spawn(SpawnOp {
                program: "cmd".into(),
                args: vec!["/c".into(), "echo".into(), "hi".into()],
                cwd: Some(dir.path().join("outside")),
                stdin: None,
                timeout_secs: Some(5),
            }))
            .await
            .expect_err("cwd outside spawn roots must be denied");
        assert!(err.contains("spawn roots"), "err: {err}");
    }

    #[tokio::test]
    async fn build_workflow_world_none_when_disabled_or_absent() {
        // 显式 enabled=false
        let (dir, handle) = seed_home(r#"{ "executor": { "enabled": false } }"#);
        let world = build_workflow_world(
            dir.path(),
            dir.path(),
            vec![dir.path().join("w")],
            vec![],
            handle,
        )
        .expect("build");
        assert!(world.is_none(), "Layer 0: disabled → no world");

        // config 无 executor 段 → 同样 Layer 0
        let (dir2, handle2) = seed_home("{}");
        let world2 = build_workflow_world(
            dir2.path(),
            dir2.path(),
            vec![dir2.path().join("w")],
            vec![],
            handle2,
        )
        .expect("build");
        assert!(world2.is_none(), "no executor section → no world");
    }
}

/// 直接对 `build_executor_channel` 的 Layer 0 / live probe 断言（不经过
/// `build_workflow_world` 包装）。probe 闭包持有的是 live ConfigStore 句柄
/// ——`store.update` 翻转后**同一通道**的 probe/gate 读数必须立刻变化，
/// 这是「dashboard 停/起沙盒不重启生效」的机制保证。
#[cfg(all(feature = "sandbox", windows))]
mod layer0_and_live_probe {
    use crate::exec_world::build_executor_channel;

    fn seed(config: &str) -> (tempfile::TempDir, nemesis_config::ConfigStore) {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("config.json"), config).expect("seed config.json");
        let store = nemesis_config::ConfigStore::load(&dir.path().join("config.json"))
            .expect("load config store");
        (dir, store)
    }

    #[test]
    fn layer0_disabled_yields_none_channel() {
        let (dir, store) = seed(r#"{ "executor": { "enabled": false, "sandbox": true } }"#);
        let channel = build_executor_channel(dir.path(), dir.path(), store.handle())
            .expect("build must not error on disabled config");
        assert!(channel.is_none(), "enabled=false → Layer 0, no channel");
    }

    #[test]
    fn layer0_absent_executor_section_yields_none_channel() {
        let (dir, store) = seed("{}");
        let channel = build_executor_channel(dir.path(), dir.path(), store.handle())
            .expect("build must not error on missing executor section");
        assert!(channel.is_none(), "no executor section → Layer 0");
    }

    #[tokio::test]
    async fn sandbox_probe_flips_live_via_store_update() {
        // 构造时 sandbox=false → probe 读 false；store.update 翻 true 后同一
        // 通道的 probe 必须读 true（ConfigStore 内存读，不落盘轮询）。
        let (dir, store) = seed(r#"{ "executor": { "enabled": true, "sandbox": false } }"#);
        let channel = build_executor_channel(dir.path(), dir.path(), store.handle())
            .expect("build")
            .expect("enabled=true → Some");
        assert!(!(channel.sandbox_probe)(), "sandbox=false at construction");

        store
            .update(|c| {
                c.executor = Some(nemesis_config::ExecutorSeparationConfig {
                    enabled: true,
                    sandbox: true,
                    allow_network: false,
                    strict: false,
                })
            })
            .expect("update store");
        assert!((channel.sandbox_probe)(), "same channel, live probe flip");

        // 翻回去也必须立即生效（停沙盒路径）。
        store
            .update(|c| {
                c.executor.as_mut().expect("executor present").sandbox = false;
            })
            .expect("update store back");
        assert!(!(channel.sandbox_probe)(), "flip back visible immediately");
    }

    #[tokio::test]
    async fn strict_gate_flips_live_via_store_update() {
        // 临时 home 下通道无盒（构造时 Start.exe 不存在）——strict=false 闸门
        // 秒过；live 翻 strict=true 后同一闸门必须拒绝（构造结果未挂盒）。
        let (dir, store) =
            seed(r#"{ "executor": { "enabled": true, "sandbox": true, "strict": false } }"#);
        let channel = build_executor_channel(dir.path(), dir.path(), store.handle())
            .expect("build")
            .expect("enabled=true → Some");
        let gate = channel.strict_gate.as_ref().expect("gate attached");
        gate().expect("strict=false → gate passes");

        store
            .update(|c| {
                c.executor.as_mut().expect("executor present").strict = true;
            })
            .expect("update store");
        let err = gate().expect_err("strict=true with no attached box must refuse");
        assert!(err.contains("no box attached"), "err: {err}");
    }
}
