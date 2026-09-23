// Arc / warn 仅 open_plugin_window（desktop 且非 android）消费；info 另有
// print_agent_startup_info（无门）消费——随门收放免裁剪构建 unused import。
#[cfg(all(feature = "desktop", not(target_os = "android")))]
use std::sync::Arc;
use tracing::info;
#[cfg(all(feature = "desktop", not(target_os = "android")))]
use tracing::warn;

use crate::common;

/// Open a URL in the default browser.
#[cfg(all(feature = "desktop", not(target_os = "android")))]
pub(crate) fn open_browser(url: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        // gateway 以托盘/无控制台运行(release windows 子系统)时,console
        // 子进程 cmd 会各自弹新控制台;输出无人收集,压掉(先例
        // background_registry.rs CREATE_NO_WINDOW 纪律)。
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        std::process::Command::new("cmd")
            .creation_flags(CREATE_NO_WINDOW)
            .raw_arg(format!("/c start {}", url))
            .spawn()
            .map_err(|e| format!("opening browser: {}", e))?;
        Ok(())
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .spawn()
            .map_err(|e| format!("opening browser: {}", e))?;
        Ok(())
    }

    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .spawn()
            .map_err(|e| format!("opening browser: {}", e))?;
        Ok(())
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        let _ = url;
        Err("unsupported platform".to_string())
    }
}

/// Open a plugin window using ProcessManager for lifecycle and deduplication.
///
/// **Single-instance**: Only one window per type is allowed. If a window of
/// the same type already exists, a `window.bring_to_front` notification is
/// sent via WebSocket. If that fails (child dead or unresponsive), the stale
/// child is terminated and a new one is spawned.
///
/// Falls back to browser if the plugin-ui library is not found.
#[cfg(all(feature = "desktop", not(target_os = "android")))]
pub(crate) fn open_plugin_window(
    process_manager: &Arc<nemesis_desktop::process::ProcessManager>,
    window_type: &str,
    backend_url: &str,
    auth_token: &str,
) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("get exe path: {}", e))?;
    let exe_dir = exe.parent().ok_or("no parent dir")?;

    // Check if plugin-ui library exists
    if nemesis_utils::find_plugin_library_in(exe_dir, "plugin_ui").is_none() {
        warn!("[Gateway] plugin-ui library not found, falling back to browser");
        return open_browser(backend_url);
    }

    // --- Dedup: check if a child of this type already exists ---
    if let Some(child_id) = process_manager.get_child_by_type(window_type) {
        info!(
            "[Gateway] Plugin window '{}' already running (child_id: {}), sending bring_to_front",
            window_type, child_id
        );
        // Try to notify the existing child to bring its window to front
        match process_manager.notify_child(
            &child_id,
            "window.bring_to_front",
            serde_json::json!({}),
        ) {
            Ok(()) => {
                info!(
                    "[Gateway] Sent bring_to_front notification to child {}",
                    child_id
                );
                return Ok(());
            }
            Err(e) => {
                // Notification failed — child may be dead. Clean up and respawn.
                warn!(
                    "[Gateway] Failed to notify child {} ({}), cleaning up and respawning",
                    child_id, e
                );
                let _ = process_manager.terminate_child(&child_id);
                process_manager.cleanup_stale();
            }
        }
    }

    // Build window data
    let window_data = match window_type {
        "dashboard" => serde_json::json!({
            "token": auth_token,
            "web_port": backend_url.split(':').next_back().and_then(|p| p.parse::<u16>().ok()).unwrap_or(49000),
            "web_host": backend_url.split("://").nth(1).and_then(|s| s.split(':').next()).unwrap_or("127.0.0.1"),
        }),
        "approval" => serde_json::json!({}),
        _ => serde_json::json!({}),
    };

    // Spawn new child via ProcessManager (handles pipe handshake + WS key + window data)
    match process_manager.spawn_child(window_type, &window_data) {
        Ok((child_id, _result_rx)) => {
            info!(
                "[Gateway] Plugin window '{}' spawned (child_id: {})",
                window_type, child_id
            );
            Ok(())
        }
        Err(e) => {
            warn!(
                "[Gateway] Failed to spawn plugin window '{}': {}",
                window_type, e
            );
            Err(format!("spawn failed: {}", e))
        }
    }
}

// ---------------------------------------------------------------------------
// Gateway banner
// ---------------------------------------------------------------------------

/// Print the gateway startup banner.
pub(crate) fn print_gateway_banner(
    web_host: &str,
    web_port: i64,
    auth_token: &str,
    channels_enabled: usize,
    gateway_host: &str,
    gateway_port: i64,
) {
    println!();
    println!("{}", "=".repeat(50));
    println!("NemesisBot Gateway");
    println!("{}", "=".repeat(50));
    println!("  Web Interface: http://{}:{}", web_host, web_port);
    println!("  Auth Token: {}", common::format_token(auth_token));

    if channels_enabled > 0 {
        println!("  OK {} channel(s) enabled", channels_enabled);
    } else {
        println!("  WARNING: No channels enabled");
    }

    println!("  OK Gateway started on {}:{}", gateway_host, gateway_port);
    println!();
    println!("  Press Ctrl+C to stop");
    println!("{}", "=".repeat(50));
    println!();
}

/// G9（2026-09-09 结构修复）：web host 解析——绑定地址与展示地址分离。
///
/// 配置 host `"0.0.0.0"`/空 + `bind_all` = 绑定所有网卡。需要远端可达的
/// 形态（集群启动、`--relay` 纯中继服务端）必须**如实绑定**：worker 跨机
/// 拉资产走 HTTP、桥接入与状态页走公网，绑定回环则 bundle 广告出去的
/// LAN IP 全是空头支票（旧逻辑无条件翻译成 127.0.0.1，逼用户手工把 host
/// 配成 LAN IP 才能跑通 G9 场景；`--relay` 传 false 曾使 VPS 上 0.0.0.0
/// 被静默回环——2026-09-19 真机验收修正）。单机场景维持保守回环绑定，
/// dashboard 不无谓暴露局域网。展示地址（gateway state / banner / 浏览器
/// URL）永远用可进地址栏的地址——0.0.0.0 不是合法浏览器地址。
/// 返回 (绑定 host, 展示 host)。
pub(crate) fn web_bind_and_display_hosts(configured: &str, bind_all: bool) -> (String, String) {
    let h = configured.trim();
    if h == "0.0.0.0" || h.is_empty() {
        if bind_all {
            ("0.0.0.0".to_string(), "127.0.0.1".to_string())
        } else {
            ("127.0.0.1".to_string(), "127.0.0.1".to_string())
        }
    } else {
        (h.to_string(), h.to_string())
    }
}

/// G9：对外资产基址 host 选择——只在 socket **真实监听所有网卡**（绑定
/// unspecified 地址）时才广告 LAN IP（此时跨机可达为真，与 cluster 节点
/// 注册挑 announce 地址同策略）；绑定回环或具体网卡时如实广告实际绑定
/// 地址（回环绑定 + 广告 LAN IP = 承诺不可达 URL，正是 G9 病灶）。
#[cfg_attr(
    not(all(feature = "board", feature = "cluster")),
    allow(dead_code) // 唯一调用点在 board+cluster 资产基址块；裁剪构建下不参与
)]
pub(crate) fn advertise_host_for(bound_ip: std::net::IpAddr, lan_ip: Option<String>) -> String {
    if bound_ip.is_unspecified() {
        lan_ip.unwrap_or_else(|| "127.0.0.1".to_string())
    } else {
        bound_ip.to_string()
    }
}

/// G9：从本机 IP 列表与集群注册表已知 peer 地址推导**对外 NIC**——与任一
/// peer 同网段的本机 IP 优先（peer 已证明在该网段内活动，本机同网段地址
/// 即为跨机可达的最优候选），无信号回落首个非回环 IP。多重网卡机器
/// （ICS/VPN/Hyper-V 虚拟适配器并存）靠这个选中真正连集群的网卡，而不是
/// `get_all_local_ips` 的首猜。纯函数便于单测。
#[cfg_attr(
    not(all(feature = "board", feature = "cluster")),
    allow(dead_code) // 同上
)]
pub(crate) fn select_advertised_lan_ip(
    local_ips: &[String],
    peer_addrs: &[String],
) -> Option<String> {
    let peer_hosts: Vec<&str> = peer_addrs
        .iter()
        .filter_map(|a| a.rsplit_once(':').map(|(h, _)| h))
        .collect();
    // 同 /24 网段的 IPv4 优先（前三个八位组一致）。
    for ip in local_ips {
        if ip.starts_with("127.") {
            continue;
        }
        let Some((a, b, c, _)) = parse_ipv4_octets(ip) else {
            continue;
        };
        for ph in &peer_hosts {
            if let Some((x, y, z, _)) = parse_ipv4_octets(ph)
                && (a, b, c) == (x, y, z)
            {
                return Some(ip.clone());
            }
        }
    }
    local_ips.iter().find(|ip| !ip.starts_with("127.")).cloned()
}

/// 解析点分 IPv4 的四个八位组；非 v4 形态返回 None。
fn parse_ipv4_octets(s: &str) -> Option<(u8, u8, u8, u8)> {
    let mut it = s.trim().split('.');
    let a = it.next()?.parse().ok()?;
    let b = it.next()?.parse().ok()?;
    let c = it.next()?.parse().ok()?;
    let d = it.next()?.parse().ok()?;
    if it.next().is_some() {
        return None;
    }
    Some((a, b, c, d))
}

/// G9 装配侧取数：本机 IP 列表 + 集群注册表已知 peer 地址（**排除本节点
/// 自己**——本节点注册地址是 get_all_local_ips 首猜，可能恰是要纠正的错
/// NIC），交给纯函数选对外 NIC。
/// `#[cfg]` 整段摘除（非 cfg_attr+dead_code）：签名/函数体引用
/// `nemesis_cluster::`，feature 关闭时类型路径必须整体出编译（2026-09-12
/// CI feature-matrix E0433 根修；两个调用点均在 all(board,cluster) 块内）。
#[cfg(all(feature = "board", feature = "cluster"))]
pub(crate) fn select_lan_ip_for_advertisement(
    cluster: &nemesis_cluster::cluster::Cluster,
) -> Option<String> {
    let local_ips = nemesis_cluster::network::get_all_local_ips();
    let self_id = cluster.node_id();
    let peer_addrs: Vec<String> = cluster
        .list_nodes()
        .into_iter()
        .filter(|n| n.base.id != self_id)
        .map(|n| n.base.address)
        .collect();
    select_advertised_lan_ip(&local_ips, &peer_addrs)
}

/// Count enabled channels.
pub(crate) fn count_enabled_channels(cfg: &nemesis_config::Config) -> usize {
    let mut count = 0;
    if cfg.channels.web.enabled {
        count += 1;
    }
    if cfg.channels.websocket.enabled {
        count += 1;
    }
    if cfg.channels.telegram.enabled {
        count += 1;
    }
    if cfg.channels.discord.enabled {
        count += 1;
    }
    if cfg.channels.feishu.enabled {
        count += 1;
    }
    if cfg.channels.slack.enabled {
        count += 1;
    }
    if cfg.channels.external.enabled {
        count += 1;
    }
    if cfg.channels.whatsapp.enabled {
        count += 1;
    }
    if cfg.channels.dingtalk.enabled {
        count += 1;
    }
    if cfg.channels.qq.enabled {
        count += 1;
    }
    if cfg.channels.line.enabled {
        count += 1;
    }
    if cfg.channels.onebot.enabled {
        count += 1;
    }
    if cfg.channels.maixcam.enabled {
        count += 1;
    }
    count
}

/// Print agent startup information.
pub(crate) fn print_agent_startup_info(home: &std::path::Path, total_tools: usize) {
    // Use register_default_tools just for counting display purposes
    let tools = nemesis_agent::register_default_tools();
    let default_count = tools.len();

    let skills_dir = home.join("workspace").join("skills");
    let skill_count = std::fs::read_dir(&skills_dir)
        .map(|d| {
            d.filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .count()
        })
        .unwrap_or(0);

    println!();
    println!("  Agent Status:");
    // saturating：注册 skew（total < default）是显示问题不是 panic 理由
    // （回归锁：test_print_agent_startup_info_no_panic 传小总数）。
    println!(
        "    Tools: {} loaded ({} default + {} extended)",
        total_tools,
        default_count,
        total_tools.saturating_sub(default_count)
    );
    println!("    Skills: {} available", skill_count);
    info!(
        "[Gateway] Agent initialized ({} tools, {} skills)",
        total_tools, skill_count
    );
}
