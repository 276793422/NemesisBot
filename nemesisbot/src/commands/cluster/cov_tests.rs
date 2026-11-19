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
                professions: None,
                tier: None,
                capabilities: None,
                runtime_backend: None,
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
        // 双旗标已启用 + token 为空 → WARN + 幂等早退（不重新生成）。
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
        // 空 token 现在先于 bind 被 fail-closed 拦下——
        // 本测的是 bind 冲突臂，补一个字面量 token 让流程走到 bind。
        update_cluster_config(&home, "token", serde_json::json!("w5-node-tok-0001")).unwrap();
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
            broadcast_interval: Some(10),
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
            broadcast_interval: Some(10),
        };
        let res = tokio::time::timeout(Duration::from_secs(20), run(action, false)).await;
        let res = res.expect("fail-closed 必须快速返回");
        let err = res.expect_err("vault 引用断裂必须拒绝启动");
        assert!(err.to_string().contains("fail-closed"), "err: {err:#}");
    }
}

// ---------------------------------------------------------------------------
// wave6：w5 吃剩的离线可达臂——token 双写非对象 JSON 的
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

// ---------------------------------------------------------------------------
// wave7：roaming 管理面（trust/mandate 簿 + 开关写）、Status 身份段、
// init 职能/档位/能力宣告段、firewall 入口、node 空 token fail-closed。
// 全程进程内 + tempdir，不触网；firewall 用非法端口 0——拒绝在命令层
// 显式给出（2026-11 镜像 netsh 已放行 localport=0，失败路径不仰赖外部
// 工具校验；与管理员权限无关，两个平台语义一致地 Err）。
// ---------------------------------------------------------------------------
mod wave7 {
    use super::wave5::{EnvHomeGuard, seed};
    use super::*;

    /// 真实 ed25519 派生的 node_id hex（trust-add/revoke 走 on-curve
    /// 校验，任意 hex 会被拒绝；种子固定，断言可复现）。
    fn valid_node_hex(seed_byte: u8) -> String {
        let mut seed = [0u8; 32];
        seed[0] = seed_byte;
        let key = ed25519_dalek::SigningKey::from_bytes(&seed);
        key.verifying_key()
            .as_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    }

    #[tokio::test]
    async fn w7_status_prints_mandate_and_identity_sections() {
        let g = EnvHomeGuard::new();
        let home = g.home();
        seed(&home);
        // mandate 配置段 + key 文件在场 → 两个状态段都走「有数据」分支。
        std::fs::write(
            common::cluster_config_path(&home),
            "{\"enabled\":true,\"port\":11949,\"rpc_port\":21949,\"broadcast_interval\":30,\
             \"mandate\":{\"mode\":\"verify\",\"sign_enabled\":false}}",
        )
        .unwrap();
        std::fs::write(common::cluster_dir(&home).join("mandate_key.json"), "{}").unwrap();
        let peers_toml = "[node]\nid = \"w7-status\"\nname = \"W7Status\"\nrole = \"worker\"\ncategory = \"development\"\n";
        std::fs::write(common::cluster_dir(&home).join("peers.toml"), peers_toml).unwrap();

        run(ClusterAction::Status, false)
            .await
            .expect("status → Ok");
    }

    /// Config 更新写盘失败（目标文件只读）：诚实报错不谎报成功，旧配置保留。
    #[tokio::test]
    async fn w7_config_write_failure_reports_and_keeps_old() {
        let g = EnvHomeGuard::new();
        let home = g.home();
        seed(&home);
        let path = common::cluster_config_path(&home);
        std::fs::write(&path, "{\"port\":11949}").unwrap();
        let meta = std::fs::metadata(&path).unwrap();
        let mut perms = meta.permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&path, perms).unwrap();

        // 端口值不同 → 进写盘臂 → 只读文件写入失败 → eprintln 后仍 Ok。
        run(
            ClusterAction::Config {
                udp_port: 1,
                rpc_port: 1,
                broadcast_interval: 1,
            },
            false,
        )
        .await
        .expect("写失败已告警 → Ok");

        // 测试收尾：还原可写，让 TempDir 能清目录。
        let meta = std::fs::metadata(&path).unwrap();
        let mut perms = meta.permissions();
        perms.set_readonly(false);
        std::fs::set_permissions(&path, perms).unwrap();
        let data = std::fs::read_to_string(&path).unwrap();
        assert_eq!(data, "{\"port\":11949}", "写失败不得改动旧配置");
    }

    #[tokio::test]
    async fn w7_init_writes_professions_tier_capabilities_backend() {
        let g = EnvHomeGuard::new();
        let home = g.home();
        run(
            ClusterAction::Init {
                name: Some("W7Node".into()),
                role: None,
                category: None,
                tags: Some("rust, cli".into()),
                address: None,
                professions: Some("dev".into()),
                tier: Some("Mini".into()),
                capabilities: Some("browser, chrome".into()),
                runtime_backend: Some("VM".into()),
            },
            false,
        )
        .await
        .expect("init 全字段宣告 → Ok");
        let data = std::fs::read_to_string(common::cluster_dir(&home).join("peers.toml")).unwrap();
        assert!(data.contains("tier = \"mini\""), "tier 归一小写: {data}");
        assert!(data.contains("\"dev\""), "职能写入: {data}");
        assert!(data.contains("browser"), "能力词写入: {data}");
        assert!(
            data.contains("runtime_backend = \"vm\""),
            "后端宣告: {data}"
        );
        assert!(data.contains("rust"), "tags 写入: {data}");
    }

    /// 未显式 --tier 时按职能目录推断（dev → normal）。
    #[tokio::test]
    async fn w7_init_infers_tier_from_professions_when_absent() {
        let g = EnvHomeGuard::new();
        let home = g.home();
        run(
            ClusterAction::Init {
                name: None,
                role: None,
                category: None,
                tags: None,
                address: None,
                professions: Some("dev".into()),
                tier: None,
                capabilities: None,
                runtime_backend: None,
            },
            false,
        )
        .await
        .expect("init 推断档位 → Ok");
        let data = std::fs::read_to_string(common::cluster_dir(&home).join("peers.toml")).unwrap();
        assert!(data.contains("tier = \"normal\""), "推断档位: {data}");
    }

    /// config.cluster.json 位置被目录占用 → 系统参数写盘失败 → WARN 不致命，
    /// peers.toml 照常写（init 继续走完）。
    #[tokio::test]
    async fn w7_init_config_write_failure_warns_but_continues() {
        let g = EnvHomeGuard::new();
        let home = g.home();
        let cfg_path = common::cluster_config_path(&home);
        std::fs::create_dir_all(&cfg_path).unwrap();

        run(
            ClusterAction::Init {
                name: Some("W7Dir".into()),
                role: None,
                category: None,
                tags: None,
                address: None,
                professions: None,
                tier: None,
                capabilities: None,
                runtime_backend: None,
            },
            false,
        )
        .await
        .expect("写失败 WARN 不致命 → Ok");
        assert!(cfg_path.is_dir(), "目录占位不被删除");
        assert!(
            common::cluster_dir(&home).join("peers.toml").exists(),
            "peers.toml 照常落盘"
        );
    }

    #[tokio::test]
    async fn w7_roaming_status_without_key_or_trust_prints_fresh_state() {
        let g = EnvHomeGuard::new();
        let home = g.home();
        run(
            ClusterAction::Roaming {
                action: RoamingAction::Status,
            },
            false,
        )
        .await
        .expect("status 空状态 → Ok");
        // 无 key 无簿：开关/密钥/信任名单三段都走「未启用」分支（不失败）。
        assert!(
            !common::workspace_path(&home).join("trust.toml").exists(),
            "status 不得写簿"
        );
    }

    #[tokio::test]
    async fn w7_roaming_status_with_key_and_trust_book() {
        let g = EnvHomeGuard::new();
        let home = g.home();
        let workspace = common::workspace_path(&home);
        std::fs::create_dir_all(workspace.join("cluster")).unwrap();
        std::fs::write(workspace.join("cluster").join("roaming.key"), "").unwrap();
        use nemesis_cluster::roaming::trust::{TrustBook, TrustBookData};
        let mut book = TrustBook::from_data(TrustBookData::default());
        book.trust(&valid_node_hex(1));
        book.revoke(&valid_node_hex(2));
        book.save(&workspace).unwrap();

        run(
            ClusterAction::Roaming {
                action: RoamingAction::Status,
            },
            false,
        )
        .await
        .expect("status 有簿 → Ok");
    }

    #[tokio::test]
    async fn w7_roaming_trust_list_with_entries_prints_book() {
        let g = EnvHomeGuard::new();
        let home = g.home();
        let workspace = common::workspace_path(&home);
        std::fs::create_dir_all(&workspace).unwrap();
        use nemesis_cluster::roaming::trust::{TrustBook, TrustBookData};
        let mut book = TrustBook::from_data(TrustBookData::default());
        book.trust(&valid_node_hex(3));
        book.revoke(&valid_node_hex(4));
        book.save(&workspace).unwrap();

        run(
            ClusterAction::Roaming {
                action: RoamingAction::TrustList,
            },
            false,
        )
        .await
        .expect("trust-list 有簿 → Ok");
    }

    #[tokio::test]
    async fn w7_roaming_trust_list_without_book_prints_hint() {
        let _g = EnvHomeGuard::new();
        run(
            ClusterAction::Roaming {
                action: RoamingAction::TrustList,
            },
            false,
        )
        .await
        .expect("trust-list 无簿 → Ok");
    }

    /// revoke/unrevoke 回环：入簿（trust-add 语义同路）、出 revoked、再查簿。
    #[tokio::test]
    async fn w7_roaming_revoke_then_unrevoke_roundtrip() {
        let g = EnvHomeGuard::new();
        let home = g.home();
        let workspace = common::workspace_path(&home);
        std::fs::create_dir_all(&workspace).unwrap();
        use nemesis_cluster::roaming::trust::TrustBook;

        let input = valid_node_hex(5);
        run(
            ClusterAction::Roaming {
                action: RoamingAction::Revoke {
                    node_id: input.clone(),
                },
            },
            false,
        )
        .await
        .expect("revoke → Ok");
        let hex = valid_node_hex(5);
        let book = TrustBook::load(&workspace).unwrap().expect("簿已激活");
        assert!(book.data().revoked.contains(&hex), "吊销入簿");
        assert!(!book.data().trusted.contains(&hex), "吊销移出 trusted");

        run(
            ClusterAction::Roaming {
                action: RoamingAction::Unrevoke { node_id: input },
            },
            false,
        )
        .await
        .expect("unrevoke → Ok");
        let book = TrustBook::load(&workspace).unwrap().expect("簿在");
        assert!(!book.data().revoked.contains(&hex), "解除吊销后 revoked 空");
    }

    #[tokio::test]
    async fn w7_roaming_mandate_list_with_entries_prints_states() {
        let g = EnvHomeGuard::new();
        let home = g.home();
        let workspace = common::workspace_path(&home);
        std::fs::create_dir_all(&workspace).unwrap();
        // 先 pin 一条（激活簿），再 revoke 另一条 → mandate-list 同时展示
        // pinned / revoked / absent 三态 + revoked 段循环。
        run(
            ClusterAction::Roaming {
                action: RoamingAction::MandatePin {
                    node_id: "w7-node-a".into(),
                    pubkey: format!("02{}", "ab".repeat(32)),
                },
            },
            false,
        )
        .await
        .expect("mandate-pin → Ok");
        run(
            ClusterAction::Roaming {
                action: RoamingAction::MandateRevoke {
                    node_id: "w7-node-b".into(),
                },
            },
            false,
        )
        .await
        .expect("mandate-revoke → Ok");

        run(
            ClusterAction::Roaming {
                action: RoamingAction::MandateList,
            },
            false,
        )
        .await
        .expect("mandate-list → Ok");
    }

    #[tokio::test]
    async fn w7_roaming_mandate_list_without_book_prints_hint() {
        let _g = EnvHomeGuard::new();
        run(
            ClusterAction::Roaming {
                action: RoamingAction::MandateList,
            },
            false,
        )
        .await
        .expect("mandate-list 无簿 → Ok");
    }

    /// 非法公钥形态 → loud 报错（66/130 hex 之外一律拒绝）。
    #[tokio::test]
    async fn w7_roaming_mandate_pin_bad_pubkey_bails() {
        let _g = EnvHomeGuard::new();
        let res = run(
            ClusterAction::Roaming {
                action: RoamingAction::MandatePin {
                    node_id: "w7-node-x".into(),
                    pubkey: "zz-not-hex".into(),
                },
            },
            false,
        )
        .await;
        let err = res.expect_err("非法公钥必须报错");
        assert!(err.to_string().contains("mandate pin 失败"), "err: {err:#}");
    }

    #[tokio::test]
    async fn w7_roaming_mandate_unrevoke_restores_state() {
        let g = EnvHomeGuard::new();
        let home = g.home();
        let workspace = common::workspace_path(&home);
        std::fs::create_dir_all(&workspace).unwrap();
        run(
            ClusterAction::Roaming {
                action: RoamingAction::MandatePin {
                    node_id: "w7-node-c".into(),
                    pubkey: format!("03{}", "cd".repeat(32)),
                },
            },
            false,
        )
        .await
        .expect("mandate-pin → Ok");
        run(
            ClusterAction::Roaming {
                action: RoamingAction::MandateRevoke {
                    node_id: "w7-node-c".into(),
                },
            },
            false,
        )
        .await
        .expect("mandate-revoke → Ok");
        run(
            ClusterAction::Roaming {
                action: RoamingAction::MandateUnrevoke {
                    node_id: "w7-node-c".into(),
                },
            },
            false,
        )
        .await
        .expect("mandate-unrevoke → Ok");
        use nemesis_cluster::roaming::trust::{MandateKeyVerdict, mandate_key_state};
        assert_eq!(
            mandate_key_state(&workspace, "w7-node-c"),
            MandateKeyVerdict::Pinned(format!("03{}", "cd".repeat(32))),
            "解除吊销后 pin 保留、验签态恢复"
        );
    }

    /// enable 先写开关再装载插件：测试环境插件缺席 → 显式报错不降级，
    /// 但开关已写盘（闸不回滚）。
    #[tokio::test]
    async fn w7_roaming_enable_bails_when_plugin_absent_but_switch_persisted() {
        let g = EnvHomeGuard::new();
        let home = g.home();
        seed(&home);
        let res = run(
            ClusterAction::Roaming {
                action: RoamingAction::Enable,
            },
            false,
        )
        .await;
        let err = res.expect_err("插件缺席必须显式报错");
        assert!(err.to_string().contains("plugin_roaming"), "err: {err:#}");
        let cfg = std::fs::read_to_string(nemesis_path::resolve_cluster_config_path_in_workspace(
            &common::workspace_path(&home),
        ))
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&cfg).unwrap();
        assert_eq!(
            v["roaming"]["enabled"].as_bool(),
            Some(true),
            "闸先写盘不回滚"
        );
    }

    #[tokio::test]
    async fn w7_roaming_disable_writes_switch_offline() {
        let g = EnvHomeGuard::new();
        let home = g.home();
        run(
            ClusterAction::Roaming {
                action: RoamingAction::Disable,
            },
            false,
        )
        .await
        .expect("disable 无需插件 → Ok");
        let cfg = std::fs::read_to_string(nemesis_path::resolve_cluster_config_path_in_workspace(
            &common::workspace_path(&home),
        ))
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&cfg).unwrap();
        assert_eq!(
            v["roaming"]["enabled"].as_bool(),
            Some(false),
            "开关落盘 false"
        );
    }

    /// 损坏的 cluster 配置：读改写环拒绝覆盖（防系统参数整段丢失）。
    #[tokio::test]
    async fn w7_roaming_enable_on_corrupt_cluster_config_fails_loud() {
        let g = EnvHomeGuard::new();
        let home = g.home();
        let cfg =
            nemesis_path::resolve_cluster_config_path_in_workspace(&common::workspace_path(&home));
        std::fs::create_dir_all(cfg.parent().unwrap()).unwrap();
        std::fs::write(&cfg, "{broken").unwrap();

        let res = run(
            ClusterAction::Roaming {
                action: RoamingAction::Enable,
            },
            false,
        )
        .await;
        let err = res.expect_err("损坏配置必须拒绝写入");
        assert!(err.to_string().contains("解析失败"), "err: {err:#}");
        assert_eq!(
            std::fs::read_to_string(&cfg).unwrap(),
            "{broken",
            "原配置不变"
        );
    }

    /// cluster 配置是目录 → 读失败 → 报错不静默当空对象。
    #[tokio::test]
    async fn w7_roaming_disable_on_unreadable_cluster_config_fails_loud() {
        let g = EnvHomeGuard::new();
        let home = g.home();
        let cfg =
            nemesis_path::resolve_cluster_config_path_in_workspace(&common::workspace_path(&home));
        std::fs::create_dir_all(&cfg).unwrap();

        let res = run(
            ClusterAction::Roaming {
                action: RoamingAction::Disable,
            },
            false,
        )
        .await;
        let err = res.expect_err("读失败必须报错");
        assert!(
            err.to_string().contains("读取 cluster 配置失败"),
            "err: {err:#}"
        );
    }

    /// firewall add 用端口 0：命令层显式拒绝（2026-11 镜像 netsh 已放行
    /// localport=0，确定性失败路径收敛到命令层，与权限/工具版本无关）。
    #[tokio::test]
    async fn w7_firewall_add_invalid_port_fails_fast() {
        let g = EnvHomeGuard::new();
        let _ = &g;
        let res = run(
            ClusterAction::Firewall {
                action: FirewallAction::Add {
                    udp_port: 0,
                    tcp_port: 0,
                },
            },
            false,
        )
        .await;
        assert!(res.is_err(), "端口 0 必须被拒绝（命令层校验）");
        // 兜底清理：即便极端环境把规则加上去了，也顺手删掉。
        let _ = run(
            ClusterAction::Firewall {
                action: FirewallAction::Remove {
                    udp_port: 0,
                    tcp_port: 0,
                },
            },
            false,
        )
        .await;
    }

    #[tokio::test]
    async fn w7_firewall_remove_reports_without_bail() {
        let g = EnvHomeGuard::new();
        let _ = &g;
        run(
            ClusterAction::Firewall {
                action: FirewallAction::Remove {
                    udp_port: 11949,
                    tcp_port: 21949,
                },
            },
            false,
        )
        .await
        .expect("remove 不因规则缺席而失败");
    }

    /// 空 token fail-closed：node 子命令显式起 worker，无 token 拒绝启动 RPC。
    #[tokio::test]
    async fn w7_node_empty_token_fails_closed_before_bind() {
        let g = EnvHomeGuard::new();
        let home = g.home();
        seed(&home);
        // token 键缺席 = 空 → fail-closed（在 RPC bind 之前）。
        let action = ClusterAction::Node {
            udp_port: Some(0),
            rpc_port: Some(0),
            name: None,
            broadcast_interval: Some(10),
        };
        let res =
            tokio::time::timeout(std::time::Duration::from_secs(20), run(action, false)).await;
        let res = res.expect("fail-closed 必须快速返回");
        let err = res.expect_err("空 token 必须拒绝启动");
        assert!(err.to_string().contains("fail-closed"), "err: {err:#}");
    }

    /// 空地址 peer 被跳过（continue 臂）+ 正常 peer 装载后进入等待循环；
    /// 未来 7s 被 timeout 掐掉（等待循环本身只由 Ctrl+C 退出，测试以
    /// 超时即「未提前 bail」为通过判据）。
    #[tokio::test]
    async fn w7_node_runs_wait_loop_until_timeout() {
        let g = EnvHomeGuard::new();
        let home = g.home();
        seed(&home);
        update_cluster_config(&home, "token", serde_json::json!("w7-node-tok")).unwrap();
        let peers_toml = r#"
[node]
id = "w7-node"
name = "W7Node"
role = "worker"
category = "development"

[peers.w7-empty]
address = ""
name = "W7Empty"

[peers.w7-live]
address = "127.0.0.1:11195"
name = "W7Live"
role = "worker"
rpc_port = 21195
"#;
        std::fs::create_dir_all(common::cluster_dir(&home)).unwrap();
        std::fs::write(common::cluster_dir(&home).join("peers.toml"), peers_toml).unwrap();

        let action = ClusterAction::Node {
            udp_port: Some(0),
            rpc_port: Some(0),
            name: Some("W7Runner".into()),
            broadcast_interval: Some(10),
        };
        let res = tokio::time::timeout(std::time::Duration::from_secs(7), run(action, false)).await;
        // 超时 = 走完装载 + RPC 启动 + 发现启动 + 进入等待循环（未被 bail）。
        assert!(res.is_err(), "node 应进入等待循环直到超时，而非提前退出");
    }
}
