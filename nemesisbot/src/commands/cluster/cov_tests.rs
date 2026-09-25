//! cluster wave-5 round-2 补测：既有套件吃剩的离线可达臂。
//!
//! 安全边界（全部进程内、不触网、不弹窗）：
//! - Pair 只连 127.0.0.1:1（回环拒绝端口，秒败；外层再套超时护栏）。
//! - run_node 用预占端口让 RPC bind 秒失败、或用解析不到的 vault 引用
//!   触发 fail-closed——两条 bail 都发生在 UDP discovery 启动之前，
//!   绝不发 LAN 广播。
//! - Reset --hard / Init 重跑依赖 stdin 的分支：非 TTY 下 EOF / 守卫
//!   自动走 abort / 跳过确认；交互终端直接跳过测试。
//! - 全程 NEMESISBOT_HOME 重定向到 tempdir（GLOBAL_STATE_LOCK 串行）。
#![cfg(target_os = "windows")]

use super::*;

mod wave5 {
    use super::*;
    use std::time::Duration;

    /// NEMESISBOT_HOME 重定向守卫：构造即指向 tempdir，Drop 时还原。
    /// 持有 GLOBAL_STATE_LOCK，与其他改 env 的测试互斥（static Mutex →
    /// guard 是 'static，可直接存进结构体）。锁中毒恢复：前一个测试
    /// panic 留下的 PoisonError 不连坐本测试。
    pub(super) struct EnvHomeGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        _tmp: tempfile::TempDir,
    }

    impl EnvHomeGuard {
        pub(super) fn new() -> Self {
            let lock = crate::GLOBAL_STATE_LOCK
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let tmp = tempfile::tempdir().unwrap();
            unsafe {
                std::env::set_var("NEMESISBOT_HOME", tmp.path());
            }
            Self {
                _lock: lock,
                _tmp: tmp,
            }
        }

        pub(super) fn home(&self) -> std::path::PathBuf {
            self._tmp.path().join(".nemesisbot")
        }
    }

    impl Drop for EnvHomeGuard {
        fn drop(&mut self) {
            // 先还原 env；随后字段按声明序自动释放（_lock 还锁 → _tmp 删目录）。
            unsafe {
                std::env::remove_var("NEMESISBOT_HOME");
            }
        }
    }

    /// 预置最小配置骨架（update_cluster_config / update_main_config_cluster
    /// 都要求目标文件已存在，否则 bail "not initialized"）：
    /// - workspace/config/config.cluster.json = `{}`
    /// - workspace/config/config.json = `{}`（主开关宿主）
    /// - workspace/cluster/（peers.toml 目录）
    pub(super) fn seed(home: &std::path::Path) {
        let cfg_dir = common::cluster_config_path(home)
            .parent()
            .unwrap()
            .to_path_buf();
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::write(common::cluster_config_path(home), "{}").unwrap();
        std::fs::write(common::config_path(home), "{}").unwrap();
        std::fs::create_dir_all(common::cluster_dir(home)).unwrap();
    }

    #[tokio::test]
    async fn w5_pair_to_refused_loopback_port_bails() {
        let g = EnvHomeGuard::new();
        let home = g.home();
        std::fs::create_dir_all(common::cluster_dir(&home)).unwrap();
        // 回环端口 1 拒绝连接 → pair 探测秒败 → Err → println + bail。
        let action = ClusterAction::Pair {
            address: "127.0.0.1:1".to_string(),
        };
        let res = tokio::time::timeout(Duration::from_secs(20), run(action, false)).await;
        assert!(res.is_ok(), "pair 必须快速失败（20s 内）");
        assert!(res.unwrap().is_err(), "拒绝端口必须 Err");
    }

    #[tokio::test]
    async fn w5_token_generate_save_without_config_prints_hint() {
        let g = EnvHomeGuard::new();
        let home = g.home();
        // config.cluster.json 缺失 → Generate --save 打提示不落盘。
        run(
            ClusterAction::Token {
                action: TokenAction::Generate {
                    length: 32,
                    save: true,
                },
            },
            false,
        )
        .await
        .expect("config 缺失 → 提示后 Ok（不写盘）");
        assert!(
            read_cluster_token(&home).is_empty(),
            "缺 config 不得落 token"
        );
    }

    #[tokio::test]
    async fn w5_token_set_without_config_prints_hint() {
        let g = EnvHomeGuard::new();
        let home = g.home();
        run(
            ClusterAction::Token {
                action: TokenAction::Set {
                    // ≥16 字符：Set 先做长度校验（<16 直接 bail），这里要打的是
                    // 后面 config 缺失的提示臂。
                    token: Some("w5-token-value-0123456789abcdef".into()),
                    generate: false,
                    length: 32,
                },
            },
            false,
        )
        .await
        .expect("config 缺失 → 提示后 Ok");
        assert!(
            read_cluster_token(&home).is_empty(),
            "缺 config 不得落 token"
        );
    }

    #[tokio::test]
    async fn w5_init_over_existing_config_skips_prompt_and_rebuilds_offline() {
        let g = EnvHomeGuard::new();
        let home = g.home();
        seed(&home);
        // config 已存在 + 非 TTY → 确认分支被 is_terminal 守卫跳过 → 重建。
        run(
            ClusterAction::Init {
                name: Some("W5Node".into()),
                role: None,
                category: None,
                tags: None,
                address: None,
            },
            false,
        )
        .await
        .expect("非 TTY 重跑 init → 跳过确认直接重建 → Ok");
        let cfg = std::fs::read_to_string(common::cluster_config_path(&home)).unwrap();
        let v: serde_json::Value = serde_json::from_str(&cfg).unwrap();
        assert_eq!(v["port"].as_u64(), Some(11949), "init 重建默认端口");
    }

    #[tokio::test]
    async fn w5_enable_both_flags_set_with_empty_token_warns_and_early_returns() {
        let g = EnvHomeGuard::new();
        let home = g.home();
        seed(&home);
        // 双旗标已启用 + token 为空 → F4 WARN + 幂等早退（不重新生成）。
        update_cluster_config(&home, "enabled", serde_json::json!(true)).unwrap();
        update_main_config_cluster(&home, true).unwrap();
        run(ClusterAction::Enable, false)
            .await
            .expect("已启用幂等早退 → Ok");
        assert!(
            read_cluster_token(&home).is_empty(),
            "F4：已启用部署不自动生成 token"
        );
    }

    #[tokio::test]
    async fn w5_enable_fresh_config_autogenerates_token_with_hint() {
        let g = EnvHomeGuard::new();
        let home = g.home();
        seed(&home);
        // subsystem enabled=false、main 缺省 false、token 为空 →
        // enable 修复双旗标 + 自动生成 token 并打印对端同步提示。
        update_cluster_config(&home, "enabled", serde_json::json!(false)).unwrap();
        run(ClusterAction::Enable, false)
            .await
            .expect("enable → Ok");
        assert!(
            !read_cluster_token(&home).is_empty(),
            "空 token 必须被 enable 自动生成兜底"
        );
        assert_eq!(cluster_flag(&home, "enabled"), Some(true));
        assert_eq!(main_cluster_flag(&home), Some(true));
    }

    #[tokio::test]
    async fn w5_disable_inconsistent_flags_repairs_both() {
        let g = EnvHomeGuard::new();
        let home = g.home();
        seed(&home);
        // subsystem=enabled、main=disabled → 不一致修复路径。
        update_cluster_config(&home, "enabled", serde_json::json!(true)).unwrap();
        update_main_config_cluster(&home, false).unwrap();
        run(ClusterAction::Disable, false)
            .await
            .expect("disable → Ok");
        assert_eq!(cluster_flag(&home, "enabled"), Some(false));
        assert_eq!(main_cluster_flag(&home), Some(false));
    }

    #[tokio::test]
    async fn w5_reset_hard_aborts_on_empty_confirmation() {
        // 交互终端（有真 stdin）下 read_line 会阻塞——直接跳过；
        // 非 TTY（CI / 工具链）下 EOF → 空应答 → Aborted 臂。
        use std::io::IsTerminal as _;
        if std::io::stdin().is_terminal() {
            return;
        }
        let g = EnvHomeGuard::new();
        let home = g.home();
        seed(&home);
        run(ClusterAction::Reset { hard: true }, false)
            .await
            .expect("空应答 → Aborted → Ok");
        // 中止路径不得删配置。
        assert!(
            common::cluster_config_path(&home).exists(),
            "abort 保留配置"
        );
    }

    #[tokio::test]
    async fn w5_node_head_loads_static_peers_then_fails_at_occupied_rpc_bind() {
        let g = EnvHomeGuard::new();
        let home = g.home();
        seed(&home);
        // 静态 peers（含 rpc_port 字段）→ run_node 头段装载循环。
        let peers_toml = r#"
[node]
id = "w5-node-a"
name = "W5NodeA"
role = "worker"
category = "development"

[peers.w5-peer-b]
address = "127.0.0.1:11195"
name = "W5PeerB"
role = "worker"
rpc_port = 21195
"#;
        std::fs::write(common::cluster_dir(&home).join("peers.toml"), peers_toml).unwrap();

        // 预占一个 0.0.0.0 临时端口 → RPC bind 冲突 → 在 discovery 启动前
        // fail-fast bail（不发任何 LAN 广播）。
        let squatter = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
        let occupied = squatter.local_addr().unwrap().port();

        let action = ClusterAction::Node {
            udp_port: Some(0),
            rpc_port: Some(occupied),
            name: Some("W5Runner".into()),
            broadcast_interval: 10,
        };
        let res = tokio::time::timeout(Duration::from_secs(20), run(action, false)).await;
        drop(squatter);
        let res = res.expect("bind 冲突必须秒败（20s 内）");
        let err = res.expect_err("端口被占 → RPC 启动失败");
        assert!(err.to_string().contains("RPC server error"), "err: {err:#}");
    }

    #[tokio::test]
    async fn w5_node_vault_broken_reference_fails_closed_before_bind() {
        let g = EnvHomeGuard::new();
        let home = g.home();
        seed(&home);
        // token 是解析不到的 vault 引用 → rpc_reference_broken →
        // fail-closed bail（RPC 拒绝启动，宁可无 RPC 不可无鉴权）。
        update_cluster_config(
            &home,
            "token",
            serde_json::json!("vault:nb_w5_missing_alias"),
        )
        .unwrap();

        let action = ClusterAction::Node {
            udp_port: Some(0),
            rpc_port: Some(0),
            name: None,
            broadcast_interval: 10,
        };
        let res = tokio::time::timeout(Duration::from_secs(20), run(action, false)).await;
        let res = res.expect("fail-closed 必须快速返回");
        let err = res.expect_err("vault 引用断裂必须拒绝启动");
        assert!(err.to_string().contains("fail-closed"), "err: {err:#}");
    }
}

// ---------------------------------------------------------------------------
// wave6（2026-09-25）：w5 吃剩的离线可达臂——token 双写非对象 JSON 的
// 静默跳过区、enable 失步修复的「仅主开关已开」方向、disable 双关幂等。
// ---------------------------------------------------------------------------
mod wave6 {
    use super::*;

    /// 复用 wave5 的 EnvHomeGuard/seed（同文件上方定义）。
    use super::wave5::{EnvHomeGuard, seed};

    /// `cluster token generate --save`：config.cluster.json 存在但不是
    /// JSON 对象（如 `[]`）→ as_object_mut None → 静默不落盘、Ok 返回。
    #[tokio::test]
    async fn w6_token_generate_save_non_object_config_is_silent_noop() {
        let g = EnvHomeGuard::new();
        let home = g.home();
        seed(&home);
        std::fs::write(common::cluster_config_path(&home), "[]").unwrap();

        run(
            ClusterAction::Token {
                action: TokenAction::Generate {
                    length: 32,
                    save: true,
                },
            },
            false,
        )
        .await
        .expect("非对象 config → 静默跳过仍 Ok");
        assert!(
            read_cluster_token(&home).is_empty(),
            "非对象 config 不得落 token"
        );
    }

    /// `cluster token set <t>`：config.cluster.json 为非对象 JSON →
    /// as_object_mut None → 不写盘、Ok 返回。
    #[tokio::test]
    async fn w6_token_set_non_object_config_is_silent_noop() {
        let g = EnvHomeGuard::new();
        let home = g.home();
        seed(&home);
        std::fs::write(common::cluster_config_path(&home), "\"str\"").unwrap();

        run(
            ClusterAction::Token {
                action: TokenAction::Set {
                    token: Some("w6-sixteen-chars-token".to_string()),
                    generate: false,
                    length: 32,
                },
            },
            false,
        )
        .await
        .expect("非对象 config → 静默跳过仍 Ok");
        let data = std::fs::read_to_string(common::cluster_config_path(&home)).unwrap();
        assert_eq!(data, "\"str\"", "非对象 config 不得被改写");
    }

    /// enable 失步修复方向二：子系统开关关（config.cluster.json
    /// enabled=false）而主开关已开（config.json cluster.enabled=true）→
    /// 只补子系统侧，不碰主开关（`if !main` 的隐式 else 区），打印 repaired。
    #[tokio::test]
    async fn w6_enable_repairs_when_only_main_switch_already_on() {
        let g = EnvHomeGuard::new();
        let home = g.home();
        seed(&home);
        std::fs::write(common::cluster_config_path(&home), "{\"enabled\":false}").unwrap();
        std::fs::write(
            common::config_path(&home),
            "{\"cluster\":{\"enabled\":true}}",
        )
        .unwrap();

        run(ClusterAction::Enable, false)
            .await
            .expect("失步修复 → Ok");
        assert!(
            cluster_flag(&home, "enabled").unwrap_or(false),
            "子系统开关必须被补开"
        );
        assert!(
            main_cluster_flag(&home).unwrap_or(false),
            "已开的主开关必须保持原样"
        );
    }

    /// disable 幂等：两侧开关均已关 → 提示已禁用并早退（不写盘）。
    #[tokio::test]
    async fn w6_disable_already_disabled_is_idempotent_early_return() {
        let g = EnvHomeGuard::new();
        let home = g.home();
        seed(&home);
        std::fs::write(common::cluster_config_path(&home), "{\"enabled\":false}").unwrap();
        std::fs::write(
            common::config_path(&home),
            "{\"cluster\":{\"enabled\":false}}",
        )
        .unwrap();

        run(ClusterAction::Disable, false)
            .await
            .expect("双关 → 幂等 Ok");
        assert!(
            !cluster_flag(&home, "enabled").unwrap_or(true),
            "子系统开关保持关"
        );
        assert!(!main_cluster_flag(&home).unwrap_or(true), "主开关保持关");
    }
}
