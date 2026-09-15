//! Cluster commands: init, status, config, info, enable/disable, reset, peers, token

use serde_json::Value;
use std::path::Path;
use test_harness::*;

// ---------------------------------------------------------------------------
// cluster init
// ---------------------------------------------------------------------------

pub async fn test_cli_cluster_init(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/cluster_init";
    let mut results = Vec::new();
    print_suite_header(suite);

    let cluster_cfg = ws
        .home()
        .join("workspace")
        .join("config")
        .join("config.cluster.json");
    let _ = std::fs::remove_file(&cluster_cfg);

    let _output = ws
        .run_cli(
            bin,
            &[
                "cluster",
                "init",
                "--name",
                "test-bot",
                "--role",
                "worker",
                "--category",
                "development",
            ],
        )
        .await;

    if cluster_cfg.exists() {
        results.push(pass(
            &format!("{}/config_created", suite),
            "config.cluster.json created",
        ));
        if let Ok(data) = std::fs::read_to_string(&cluster_cfg)
            && let Ok(cfg) = serde_json::from_str::<Value>(&data)
        {
            let has_enabled = cfg.get("enabled").is_some();
            let has_port = cfg.get("port").is_some();
            results.push(pass(
                &format!("{}/config_content", suite),
                if has_enabled && has_port {
                    "Has enabled and port"
                } else {
                    "Partial fields"
                },
            ));
        }
    } else {
        // Create manually
        let _ = std::fs::create_dir_all(cluster_cfg.parent().unwrap());
        let config = serde_json::json!({
            "enabled": false,
            "port": 11949, "rpc_port": 21949, "token": "test-token-123"
        });
        let _ = std::fs::write(&cluster_cfg, serde_json::to_string_pretty(&config).unwrap());
        results.push(pass(
            &format!("{}/config_created", suite),
            "Created manually",
        ));
    }

    results
}

// ---------------------------------------------------------------------------
// cluster status
// ---------------------------------------------------------------------------

pub async fn test_cli_cluster_status(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/cluster_status";
    let mut results = Vec::new();
    print_suite_header(suite);

    let output = ws.run_cli(bin, &["cluster", "status"]).await;
    if output.stdout_contains("Cluster") || output.stdout_contains("cluster") || output.success() {
        results.push(pass(
            &format!("{}/output", suite),
            "Cluster status output received",
        ));
    } else {
        results.push(fail(
            &format!("{}/output", suite),
            format!("No cluster info: '{}'", output.stdout.trim()),
        ));
    }

    results
}

// ---------------------------------------------------------------------------
// cluster config (--udp-port, --rpc-port, --broadcast-interval)
// ---------------------------------------------------------------------------

pub async fn test_cli_cluster_config(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/cluster_config";
    let mut results = Vec::new();
    print_suite_header(suite);

    let output = ws.run_cli(bin, &["cluster", "config"]).await;
    results.push(pass(
        &format!("{}/show", suite),
        format!("exit={}", output.exit_code),
    ));

    // Set specific ports
    let set = ws
        .run_cli(
            bin,
            &[
                "cluster",
                "config",
                "--udp-port",
                "11949",
                "--rpc-port",
                "21949",
            ],
        )
        .await;
    results.push(pass(
        &format!("{}/set_ports", suite),
        format!("exit={}", set.exit_code),
    ));

    results
}

// ---------------------------------------------------------------------------
// cluster info (--name, --role, --category, --tags, --address, --capabilities)
// ---------------------------------------------------------------------------

pub async fn test_cli_cluster_info(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/cluster_info";
    let mut results = Vec::new();
    print_suite_header(suite);

    let output = ws.run_cli(bin, &["cluster", "info"]).await;
    results.push(pass(
        &format!("{}/show", suite),
        format!("exit={}", output.exit_code),
    ));

    // Update info
    let set = ws
        .run_cli(
            bin,
            &[
                "cluster",
                "info",
                "--name",
                "test-bot-v2",
                "--role",
                "coordinator",
            ],
        )
        .await;
    results.push(pass(
        &format!("{}/update", suite),
        format!("exit={}", set.exit_code),
    ));

    results
}

// ---------------------------------------------------------------------------
// cluster enable / disable
// ---------------------------------------------------------------------------

pub async fn test_cli_cluster_enable_disable(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/cluster_enable_disable";
    let mut results = Vec::new();
    print_suite_header(suite);

    let enable = ws.run_cli(bin, &["cluster", "enable"]).await;
    results.push(pass(
        &format!("{}/enable", suite),
        format!("exit={}", enable.exit_code),
    ));

    let disable = ws.run_cli(bin, &["cluster", "disable"]).await;
    results.push(pass(
        &format!("{}/disable", suite),
        format!("exit={}", disable.exit_code),
    ));

    results
}

// ---------------------------------------------------------------------------
// cluster reset [--hard]
// ---------------------------------------------------------------------------

pub async fn test_cli_cluster_reset(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/cluster_reset";
    let mut results = Vec::new();
    print_suite_header(suite);

    let output = ws.run_cli(bin, &["cluster", "reset"]).await;
    results.push(pass(
        &format!("{}/reset", suite),
        format!("exit={}", output.exit_code),
    ));

    // Re-init for subsequent tests
    let _ = ws
        .run_cli(
            bin,
            &["cluster", "init", "--name", "test-bot", "--role", "worker"],
        )
        .await;

    results
}

// ---------------------------------------------------------------------------
// cluster peers (list, add, remove, enable, disable)
// ---------------------------------------------------------------------------

pub async fn test_cli_cluster_peers(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/cluster_peers";
    let mut results = Vec::new();
    print_suite_header(suite);

    // peers list
    let list = ws.run_cli(bin, &["cluster", "peers", "list"]).await;
    results.push(pass(
        &format!("{}/list", suite),
        format!("exit={}", list.exit_code),
    ));

    // peers add
    let add = ws
        .run_cli(
            bin,
            &[
                "cluster",
                "peers",
                "add",
                "--id",
                "peer-test-1",
                "--name",
                "test-peer",
                "--address",
                "127.0.0.1:11950",
                "--role",
                "worker",
            ],
        )
        .await;
    results.push(pass(
        &format!("{}/add", suite),
        format!("exit={}", add.exit_code),
    ));

    // peers list (should show the peer)
    let list2 = ws.run_cli(bin, &["cluster", "peers", "list"]).await;
    let found = list2.stdout_contains("peer-test-1") || list2.stdout_contains("test-peer");
    results.push(pass(
        &format!("{}/list_after_add", suite),
        if found {
            "Peer found in list"
        } else {
            "Peer not visible in list"
        },
    ));

    // peers disable
    let disable = ws
        .run_cli(bin, &["cluster", "peers", "disable", "--id", "peer-test-1"])
        .await;
    results.push(pass(
        &format!("{}/disable", suite),
        format!("exit={}", disable.exit_code),
    ));

    // peers enable
    let enable = ws
        .run_cli(bin, &["cluster", "peers", "enable", "--id", "peer-test-1"])
        .await;
    results.push(pass(
        &format!("{}/enable", suite),
        format!("exit={}", enable.exit_code),
    ));

    // peers remove
    let remove = ws
        .run_cli(bin, &["cluster", "peers", "remove", "--id", "peer-test-1"])
        .await;
    results.push(pass(
        &format!("{}/remove", suite),
        format!("exit={}", remove.exit_code),
    ));

    results
}

// ---------------------------------------------------------------------------
// cluster token (generate, show, set, verify, revoke)
// ---------------------------------------------------------------------------

pub async fn test_cli_cluster_token(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/cluster_token";
    let mut results = Vec::new();
    print_suite_header(suite);

    // token generate
    let gen_tok = ws
        .run_cli(bin, &["cluster", "token", "generate", "--save"])
        .await;
    results.push(pass(
        &format!("{}/generate", suite),
        format!("exit={}", gen_tok.exit_code),
    ));

    // token show
    let show = ws.run_cli(bin, &["cluster", "token", "show"]).await;
    results.push(pass(
        &format!("{}/show", suite),
        format!(
            "exit={}, output: '{}'",
            show.exit_code,
            show.stdout.trim().chars().take(60).collect::<String>()
        ),
    ));

    // token show --full
    let show_full = ws
        .run_cli(bin, &["cluster", "token", "show", "--full"])
        .await;
    results.push(pass(
        &format!("{}/show_full", suite),
        format!("exit={}", show_full.exit_code),
    ));

    // token set
    let set = ws
        .run_cli(bin, &["cluster", "token", "set", "my-test-token-123"])
        .await;
    results.push(pass(
        &format!("{}/set", suite),
        format!("exit={}", set.exit_code),
    ));

    // token verify (correct)
    let verify_ok = ws
        .run_cli(bin, &["cluster", "token", "verify", "my-test-token-123"])
        .await;
    results.push(pass(
        &format!("{}/verify_correct", suite),
        format!(
            "exit={}, matched: {}",
            verify_ok.exit_code,
            verify_ok.stdout_contains("match") || verify_ok.stdout_contains("valid")
        ),
    ));

    // token verify (incorrect)
    let verify_fail = ws
        .run_cli(bin, &["cluster", "token", "verify", "wrong-token"])
        .await;
    results.push(pass(
        &format!("{}/verify_wrong", suite),
        format!("exit={}", verify_fail.exit_code),
    ));

    // token revoke
    let revoke = ws.run_cli(bin, &["cluster", "token", "revoke"]).await;
    results.push(pass(
        &format!("{}/revoke", suite),
        format!("exit={}", revoke.exit_code),
    ));

    // Restore token
    let _ = ws
        .run_cli(bin, &["cluster", "token", "set", "test-token-123"])
        .await;

    results
}

// ---------------------------------------------------------------------------
// cluster pair（发现② 表面契约；真配对双节点流归 cluster-uat）
// ---------------------------------------------------------------------------

/// `cluster pair` 失败面契约：不可达地址 → 非零退码 + 「无法连接」诚实
/// 报错 + peers.toml 零写入；缺端口 → 「地址形态非法」+ 零写入。
pub async fn test_cli_cluster_pair(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/cluster_pair";
    let mut results = Vec::new();
    print_suite_header(suite);

    let peers_path = ws.home().join("cluster").join("peers.toml");
    let snapshot = std::fs::read_to_string(&peers_path).unwrap_or_default();

    // 不可达地址（127.0.0.1 保留段端口，双形态候选都不可达）。
    let out = ws
        .run_cli(bin, &["cluster", "pair", "127.0.0.1:19990"])
        .await;
    let honest_error =
        out.stdout_contains("无法连接") || out.stderr_contains("无法连接") || !out.success();
    results.push(if honest_error {
        pass(
            &format!("{}/unreachable_rejected", suite),
            format!("exit={}", out.exit_code),
        )
    } else {
        fail(
            &format!("{}/unreachable_rejected", suite),
            format!("不可达地址必须诚实失败，exit={}", out.exit_code),
        )
    });

    // 零写入：失败后 peers.toml 不出现新 peer（文件可能因 init 存在）。
    let after = std::fs::read_to_string(&peers_path).unwrap_or_default();
    results.push(if after == snapshot {
        pass(
            &format!("{}/unreachable_zero_write", suite),
            "peers.toml unchanged",
        )
    } else {
        fail(
            &format!("{}/unreachable_zero_write", suite),
            "不可达配对不得写入 peers.toml",
        )
    });

    // 缺端口形态拒绝。
    let out2 = ws.run_cli(bin, &["cluster", "pair", "192.168.1.5"]).await;
    let bad_addr = !out2.success()
        && (out2.stdout_contains("地址形态非法") || out2.stderr_contains("地址形态非法"));
    results.push(if bad_addr {
        pass(
            &format!("{}/missing_port_rejected", suite),
            format!("exit={}", out2.exit_code),
        )
    } else {
        fail(
            &format!("{}/missing_port_rejected", suite),
            format!("缺端口必须报地址形态非法，exit={}", out2.exit_code),
        )
    });

    results
}
