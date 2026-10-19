//! iroh 腿探针（③a，feature=iroh 编译——iroh 依赖重，默认不进 workspace
//! 全量构建）。生产同源 API 直驱：身份密钥/端点装配/echo handler/ping
//! 与 `cluster roaming` 运行时同一模块（`nemesis_cluster::roaming`），地
//! 址解析复用 `parse_roaming_address`（`iroh:<hex>[@<ip:port>]` 生产语法）。
//!
//! - `iroh-self`：绑定端点（N0 relay 预设或 `--direct` Minimal）+ echo
//!   serve，打印 node_id / direct_addrs 供对端 ping；`--serve-secs` 到点
//!   自动退出（0 = 常驻到 Ctrl+C）。
//! - `iroh-ping`：对端往返（真实帧 echo，非 TCP connect 假成功），打印
//!   RTT；失败现场 = 错误链原文。

use std::path::PathBuf;
use std::time::Duration;

use nemesis_cluster::roaming::bridge::parse_roaming_address;
use nemesis_cluster::roaming::endpoint::{
    RoamingEndpoint, RoamingIdentity, RoamingProtocolHandler,
};

use crate::{EXIT_FAIL, EXIT_PASS};

fn default_workspace() -> PathBuf {
    let base = std::env::temp_dir().join("verify-probe-iroh");
    let _ = std::fs::create_dir_all(&base);
    base
}

/// iroh 腿：本机端点自检 + echo serve 常驻。
pub async fn self_probe(workspace: Option<&str>, serve_secs: u64, direct: bool) -> i32 {
    let ws = workspace
        .map(PathBuf::from)
        .unwrap_or_else(default_workspace);
    let (identity, created) = RoamingIdentity::load_or_create(&ws)
        .map_err(|e| format!("身份密钥装配失败: {e}"))
        .unwrap_or_else(|e| {
            eprintln!("FAIL: {e}");
            std::process::exit(EXIT_FAIL);
        });
    println!(
        "identity: node_id={} ({}), key_path={}",
        identity.node_id_hex(),
        if created { "新生成" } else { "已有" },
        nemesis_cluster::roaming::endpoint::roaming_key_path(&ws).display()
    );

    let endpoint = if direct {
        RoamingEndpoint::bind_direct(&identity, true).await
    } else {
        RoamingEndpoint::bind(&identity, true).await
    };
    let endpoint = match endpoint {
        Ok(ep) => ep,
        Err(e) => {
            eprintln!("FAIL: 端点绑定失败（relay 不可达/端口被占等，现场如下）：\n{e}");
            return EXIT_FAIL;
        }
    };
    println!(
        "preset: {}",
        if direct {
            "Minimal（无 relay 纯直连）"
        } else {
            "N0（relay+DNS 发现）"
        }
    );
    println!("node_id: {}", endpoint.node_id_hex());
    let addrs = endpoint.direct_addrs();
    if addrs.is_empty() {
        println!("direct_addrs: <空——出口枚举暂缺，对端须走 relay/DNS 发现>");
    } else {
        println!("direct_addrs:");
        for a in &addrs {
            println!("  {a}");
        }
        // bound_sockets 报的是监听套接字（常为 0.0.0.0 通配）——通配地址不
        // 可直接拨号；端口才是有效信息，IP 由对端按本机可达地址替换。
        let ports: Vec<u16> = addrs.iter().map(|a| a.port()).collect();
        println!(
            "对端 ping 命令形态（<IP> 换成本机可达地址，端口取上列之一）：iroh-ping iroh:{}@<IP>:{} 或 iroh:{}（走发现）",
            endpoint.node_id_hex(),
            ports[0],
            endpoint.node_id_hex()
        );
    }
    if let Err(e) = endpoint.serve(RoamingProtocolHandler::echo()).await {
        eprintln!("FAIL: echo serve 装配失败：\n{e}");
        endpoint.shutdown().await;
        return EXIT_FAIL;
    }
    println!("serve: echo handler 已挂（ALPN nemesis-cluster/1）");
    if serve_secs == 0 {
        println!("常驻中（Ctrl+C 退出）…");
        tokio::signal::ctrl_c().await.ok();
    } else {
        println!("保持 {serve_secs}s 供对端 ping…");
        tokio::time::sleep(Duration::from_secs(serve_secs)).await;
    }
    endpoint.shutdown().await;
    println!("PASS: 端点装配 + echo serve 正常（窗口期无装配错误）");
    EXIT_PASS
}

/// iroh 腿：ping 对端（`iroh:<hex>[@<ip:port>]` 生产地址语法）。
pub async fn ping(addr: &str, workspace: Option<&str>, timeout_secs: u64, direct: bool) -> i32 {
    let ws = workspace
        .map(PathBuf::from)
        .unwrap_or_else(default_workspace);
    let (identity, _) = RoamingIdentity::load_or_create(&ws).unwrap_or_else(|e| {
        eprintln!("FAIL: 身份密钥装配失败: {e}");
        std::process::exit(EXIT_FAIL);
    });
    let endpoint = if direct {
        RoamingEndpoint::bind_direct(&identity, false).await
    } else {
        RoamingEndpoint::bind(&identity, false).await
    };
    let endpoint = match endpoint {
        Ok(ep) => ep,
        Err(e) => {
            eprintln!("FAIL: 本端端点绑定失败：\n{e}");
            return EXIT_FAIL;
        }
    };
    let parsed = match parse_roaming_address(addr) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("FAIL: 地址非法（生产语法 `iroh:<hex>[@<ip:port>]`）：\n{e}");
            endpoint.shutdown().await;
            return EXIT_FAIL;
        }
    };
    println!(
        "pinging {addr}（preset: {}，timeout {timeout_secs}s）…",
        if direct { "Minimal" } else { "N0" }
    );
    match endpoint
        .ping_addr(parsed, Duration::from_secs(timeout_secs))
        .await
    {
        Ok(rtt_ms) => {
            println!("PASS: echo 往返成立，RTT = {rtt_ms} ms");
            endpoint.shutdown().await;
            EXIT_PASS
        }
        Err(e) => {
            eprintln!("FAIL: ping 失败（现场如下）：\n{e}");
            endpoint.shutdown().await;
            EXIT_FAIL
        }
    }
}
