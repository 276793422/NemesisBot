//! quic WSAPI 覆盖率（2026-10-23 与集群解耦，自 handlers/cluster/tests.rs
//! 迁入；QUIC 插件化 M1 起 handler 代理化——此处钉 config 合并/清除语义、
//! status 载荷形状、客户端面两键、handle=None 实时开关契约 + 名册命令表面）。

use super::*;

#[test]
#[cfg(feature = "cluster")]
fn test_relay_config_save_merge_and_clear_semantics() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_str().unwrap();
    let cfg_path = quic_config_path(ws);
    std::fs::create_dir_all(cfg_path.parent().unwrap()).unwrap();
    std::fs::write(
        &cfg_path,
        r#"{
  "relay": { "registry_port": 3341 },
  "client": { "rendezvous_url": "http://rz.example.com:3341" }
}"#,
    )
    .unwrap();

    // 1. 合并写：relay 段既有键保留 + token trim；client 段兄弟键不动
    let p = relay_config_save_impl(
        ws,
        &serde_json::json!({ "http_port": 4000, "tls": "manual", "token": "  s3cret  " }),
    )
    .unwrap()
    .unwrap();
    assert_eq!(p["saved"], serde_json::json!(true));
    assert_eq!(p["requires_restart"], serde_json::json!(true));
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(v["relay"]["http_port"], serde_json::json!(4000));
    assert_eq!(v["relay"]["tls"], serde_json::json!("manual"));
    assert_eq!(v["relay"]["token"], serde_json::json!("s3cret"));
    assert_eq!(v["relay"]["registry_port"], serde_json::json!(3341));
    assert_eq!(
        v["client"]["rendezvous_url"],
        serde_json::json!("http://rz.example.com:3341")
    );
    // 回显载荷 token 永不出现（token_set bool only）
    let echo = serde_json::to_string(&p).unwrap();
    assert!(!echo.contains("s3cret"));
    assert_eq!(p["relay"]["token_set"], serde_json::json!(true));

    // 2. 空串 = 清除该键（其余键保留）
    relay_config_save_impl(ws, &serde_json::json!({ "token": "" })).unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert!(v["relay"].get("token").is_none());
    assert_eq!(v["relay"]["tls"], serde_json::json!("manual"));

    // 3. 非法值 loud 拒（写面是当次意志，不静默纠偏）
    assert!(relay_config_save_impl(ws, &serde_json::json!({ "http_port": 0 })).is_err());
    assert!(relay_config_save_impl(ws, &serde_json::json!({ "quic_port": 70000 })).is_err());
    assert!(relay_config_save_impl(ws, &serde_json::json!({ "tls": "bogus" })).is_err());
    assert!(relay_config_save_impl(ws, &serde_json::json!({ "unknown_key": 1 })).is_err());
    assert!(relay_config_save_impl(ws, &serde_json::json!({ "token": 42 })).is_err());
    // 拒绝后配置未被半写（仍是步骤 2 形态）
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(v["relay"]["http_port"], serde_json::json!(4000));
    assert!(v["relay"].get("token").is_none());
}

#[test]
#[cfg(feature = "cluster")]
fn test_relay_status_payload_shape() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_str().unwrap();
    let p = relay_status_payload(ws, false, serde_json::Value::Null, serde_json::Value::Null)
        .unwrap()
        .unwrap();
    assert_eq!(p["available"], serde_json::json!(true));
    assert_eq!(p["enabled"], serde_json::json!(false));
    assert_eq!(p["active"], serde_json::json!(false));
    assert_eq!(p["requires_restart"], serde_json::json!(false));
    // 默认档：self-signed + 默认端口 + 证书待生成
    assert_eq!(p["relay"]["http_port"], serde_json::json!(3340));
    assert_eq!(p["relay"]["quic_port"], serde_json::json!(7842));
    assert_eq!(p["relay"]["tls"], serde_json::json!("self-signed"));
    assert_eq!(p["relay"]["token_set"], serde_json::json!(false));
    assert_eq!(p["relay"]["cert_state"], serde_json::json!("pending"));
    assert_eq!(p["client"]["relay_url"], serde_json::json!(""));
    assert_eq!(p["client"]["relay_token_set"], serde_json::json!(false));
    assert!(p["http_addr"].is_null());
    assert!(p["quic_addr"].is_null());

    // config 闸开而未跑 → requires_restart
    relay_config_save_impl(ws, &serde_json::json!({ "enabled": true })).unwrap();
    let p = relay_status_payload(ws, false, serde_json::Value::Null, serde_json::Value::Null)
        .unwrap()
        .unwrap();
    assert_eq!(p["enabled"], serde_json::json!(true));
    assert_eq!(p["requires_restart"], serde_json::json!(true));

    // 运行态跟上 → requires_restart=false + 地址面回显（插件回执字符串透传）
    let p = relay_status_payload(
        ws,
        true,
        serde_json::json!("127.0.0.1:3340"),
        serde_json::json!("127.0.0.1:7842"),
    )
    .unwrap()
    .unwrap();
    assert_eq!(p["requires_restart"], serde_json::json!(false));
    assert_eq!(p["http_addr"], serde_json::json!("127.0.0.1:3340"));
    assert_eq!(p["quic_addr"], serde_json::json!("127.0.0.1:7842"));
}

#[test]
#[cfg(feature = "cluster")]
fn test_relay_config_save_client_face() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_str().unwrap();
    let cfg_path = quic_config_path(ws);
    // 无文件起步：客户端两键写进 client 段（非 relay 子段）
    let p = relay_config_save_impl(
        ws,
        &serde_json::json!({
            "relay_url": " http://vps.example.com:3340 ",
            "relay_token": " c-tok "
        }),
    )
    .unwrap()
    .unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(
        v["client"]["relay_url"],
        serde_json::json!("http://vps.example.com:3340")
    );
    assert_eq!(v["client"]["relay_token"], serde_json::json!("c-tok"));
    // 回显面：relay_url 原样、relay_token 只有 bool
    assert_eq!(
        p["client"]["relay_url"],
        serde_json::json!("http://vps.example.com:3340")
    );
    assert_eq!(p["client"]["relay_token_set"], serde_json::json!(true));
    assert!(!serde_json::to_string(&p).unwrap().contains("c-tok"));

    // 空串清除：relay_url 键移除（客户端不走向继），relay_token 不动
    relay_config_save_impl(ws, &serde_json::json!({ "relay_url": "" })).unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert!(v["client"].get("relay_url").is_none());
    assert_eq!(v["client"]["relay_token"], serde_json::json!("c-tok"));
    // 服务端 relay 段与客户端 client 段互不侵扰：同一次保存两边都可写
    relay_config_save_impl(
        ws,
        &serde_json::json!({ "http_port": 4000, "relay_url": "http://r2.example.com:3340" }),
    )
    .unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(v["relay"]["http_port"], serde_json::json!(4000));
    assert_eq!(
        v["client"]["relay_url"],
        serde_json::json!("http://r2.example.com:3340")
    );
}

// ---------------------------------------------------------------------------
// 实时开关（2026-10-23）：relay.enable / relay.disable 免重启网关——config 闸
// 落盘 + 插件内拉起/停掉一体（真中继 roundtrip 归插件侧 e2e + gateway 装载
// 冒烟）。此处以 handle=None 验证 config+载荷契约：闸与运行态解耦时的诚实
// 呈现（插件未装载 = requires_restart 亮起，不假成功）。
// ---------------------------------------------------------------------------

#[tokio::test]
#[cfg(feature = "cluster")]
async fn test_relay_toggle_config_contract_without_cluster() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_str().unwrap();
    let cfg_path = quic_config_path(ws);

    // enable（插件句柄未装载）：config 闸落盘；active=false、requires_restart=true
    // 诚实呈现——配置说了算、进程没跟上，面板亮「待启动」而不是假成功。
    let p = relay_toggle_impl(None, ws, true).await.unwrap().unwrap();
    assert_eq!(p["enabled"], serde_json::json!(true));
    assert_eq!(p["active"], serde_json::json!(false));
    assert_eq!(p["requires_restart"], serde_json::json!(true));
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(v["relay"]["enabled"], serde_json::json!(true));

    // disable：闸回落 + requires_restart 熄灭（配置与运行态一致=都没跑）。
    let p = relay_toggle_impl(None, ws, false).await.unwrap().unwrap();
    assert_eq!(p["enabled"], serde_json::json!(false));
    assert_eq!(p["active"], serde_json::json!(false));
    assert_eq!(p["requires_restart"], serde_json::json!(false));
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(v["relay"]["enabled"], serde_json::json!(false));
}

// ---------------------------------------------------------------------------
// 命令表面：L1 清单恒注册 + feature 关诚实降级（双形态锚点）
// ---------------------------------------------------------------------------

#[test]
fn quic_module_commands_surface_is_stable() {
    let h = QuicHandler::new();
    assert_eq!(h.module_name(), "quic");
    let cmds = h.commands();
    for c in [
        "relay.status",
        "relay.config",
        "relay.enable",
        "relay.disable",
        "registry.members",
    ] {
        assert!(cmds.contains(&c), "quic 清单缺 {c}");
    }
}

/// 插件化 M1 表面锚点：registry.members 命令恒注册（诚实降级形态在
/// handler None 臂——插件缺席 = available:false + reason，不伪装空表）。
#[test]
fn registry_members_command_registered_when_plugin_absent() {
    let h = QuicHandler::new();
    assert!(h.commands().contains(&"registry.members"));
}

// ---------------------------------------------------------------------------
// BUG-20261025-15 双形态锚点（cluster 关形态）：解析投影缺席 = status/回显
// 诚实降级，校验+落盘（纯 JSON RMW）照常。裸 `cargo test -p nemesis-web`
// （无 feature）跑这里；全量 feature 构建跑上方 cfg(feature) 臂。
// ---------------------------------------------------------------------------

#[cfg(not(feature = "cluster"))]
#[test]
fn quic_status_payload_degrades_honestly_without_cluster() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_str().unwrap();
    let p = relay_status_payload(ws, false, serde_json::Value::Null, serde_json::Value::Null)
        .unwrap()
        .unwrap();
    assert_eq!(p["available"], serde_json::json!(false));
    assert!(p["reason"].as_str().unwrap().contains("cluster"));
    // 运行态字段透传，不伪装
    assert_eq!(p["active"], serde_json::json!(false));
    assert!(p.get("enabled").is_none(), "降级形态不得伪装 config 投影");
    assert!(p.get("relay").is_none());
}

#[cfg(not(feature = "cluster"))]
#[test]
fn quic_config_save_writes_file_without_cluster_projection() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_str().unwrap();
    let cfg_path = quic_config_path(ws);
    let p = relay_config_save_impl(ws, &serde_json::json!({ "http_port": 4000 }))
        .unwrap()
        .unwrap();
    // 落盘成功 + 降级注记，无投影伪装
    assert_eq!(p["saved"], serde_json::json!(true));
    assert!(p["note"].as_str().unwrap().contains("cluster"));
    assert!(p.get("relay").is_none());
    // 纯 JSON RMW 不依赖 cluster：文件内容正确
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(v["relay"]["http_port"], serde_json::json!(4000));
}
