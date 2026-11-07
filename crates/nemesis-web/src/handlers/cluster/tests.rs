use super::*;
use std::time::Duration;

#[test]
fn format_duration_zero() {
    assert_eq!(format_duration(Duration::from_secs(0)), "0s");
}

#[test]
fn format_duration_seconds_only() {
    assert_eq!(format_duration(Duration::from_secs(45)), "45s");
}

#[test]
fn format_duration_minutes() {
    assert_eq!(format_duration(Duration::from_secs(125)), "2m 5s");
}

#[test]
fn format_duration_hours() {
    assert_eq!(format_duration(Duration::from_secs(3725)), "1h 2m");
}

#[test]
fn format_duration_days() {
    assert_eq!(format_duration(Duration::from_secs(90061)), "1d 1h");
}

#[test]
fn format_ago_branches() {
    assert_eq!(format_ago(Duration::from_secs(5)), "5s ago");
    assert_eq!(format_ago(Duration::from_secs(120)), "2m ago");
    assert_eq!(format_ago(Duration::from_secs(7200)), "2h ago");
    assert_eq!(format_ago(Duration::from_secs(86400 * 3)), "3d ago");
}

#[test]
fn format_bytes_units() {
    assert_eq!(format_bytes(512), "512B");
    assert_eq!(format_bytes(2048), "2KB");
    assert_eq!(format_bytes(1024 * 1024 * 5), "5.0MB");
}

#[test]
fn truncate_str_no_truncation_when_short_enough() {
    assert_eq!(truncate_str("hello", 10), "hello");
    assert_eq!(truncate_str("hello", 5), "hello");
}

#[test]
fn truncate_str_ascii_truncates_with_ellipsis() {
    assert_eq!(truncate_str("hello world", 5), "hello...");
}

#[test]
fn truncate_str_utf8_lands_on_char_boundary() {
    // Each emoji is 4 bytes; max_len=6 must not split a multi-byte char.
    let out = truncate_str("🎯🎯🎯🎯", 6);
    assert!(out.ends_with("..."));
    // Should contain exactly one full emoji before the ellipsis (4 bytes ≤ 6).
    assert_eq!(out, "🎯...");
}

#[test]
fn test_broadcast_flag_passes_on_any_platform() {
    // Binding 0.0.0.0:0 always succeeds; setting SO_BROADCAST is universally
    // supported. This verifies the success-shape of the diagnostic helper.
    let r = test_broadcast_flag();
    assert_eq!(r["name"], "broadcast_flag");
    assert_eq!(r["pass"], true);
}

// iroh 跨网漫游开关面（config 闸 + enable/disable） ---

#[test]
fn test_roaming_config_write_and_gate_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let cfg_path = cluster_config_path(ws.to_str().unwrap());
    // 初始：无 config = 关
    assert!(!Cluster::roaming_enabled_in_config(&cfg_path));
    // 写 true → 闸读回 true
    update_cluster_config_roaming(ws.to_str().unwrap(), true).unwrap();
    assert!(Cluster::roaming_enabled_in_config(&cfg_path));
    // 写 false → 闸读回 false（且文件其余键保留）
    update_cluster_config_roaming(ws.to_str().unwrap(), false).unwrap();
    assert!(!Cluster::roaming_enabled_in_config(&cfg_path));
    let raw = std::fs::read_to_string(&cfg_path).unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(v["roaming"]["enabled"], serde_json::json!(false));
}

#[tokio::test]
async fn test_roaming_status_payload_default_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let payload =
        roaming_status_payload(dir.path().to_str().unwrap(), None, serde_json::Value::Null)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(payload["available"], serde_json::json!(true));
    assert_eq!(payload["enabled"], serde_json::json!(false));
    assert_eq!(payload["active"], serde_json::json!(false));
    assert_eq!(payload["requires_restart"], serde_json::json!(false));
    assert_eq!(payload["key_present"], serde_json::json!(false));
    assert!(payload["node_id"].is_null());
    assert_eq!(payload["trust"]["enabled"], serde_json::json!(false));
}

#[tokio::test]
async fn test_roaming_status_payload_requires_restart_consistency() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_str().unwrap();
    update_cluster_config_roaming(ws, true).unwrap();
    // config 开了但运行态未知（None→false）：requires_restart=true
    let p = roaming_status_payload(ws, None, serde_json::Value::Null)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(p["requires_restart"], serde_json::json!(true));
    // 运行态跟上（active=true）：requires_restart=false
    let p = roaming_status_payload(ws, Some(true), serde_json::Value::Null)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(p["requires_restart"], serde_json::json!(false));
    assert_eq!(p["active"], serde_json::json!(true));
}

#[tokio::test]
async fn test_roaming_status_payload_carries_rendezvous_state() {
    // rendezvous 段透传：同步状态原样进 payload；Null = 循环未挂（形态）。
    // QUIC 插件化：node_id 读走桥（测试环境插件缺席 = null 诚实降级）。
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_str().unwrap();
    let state = nemesis_cluster::roaming::rendezvous::RendezvousClientState {
        registry_url: "http://vps.example.com:3341".into(),
        last_sync_ok: true,
        last_sync_ms: 1_760_000_000_000,
        member_count: 4,
        last_error: String::new(),
    };
    let rv = serde_json::to_value(&state).unwrap();
    let p = roaming_status_payload(ws, Some(true), rv)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        p["rendezvous"]["registry_url"],
        serde_json::json!("http://vps.example.com:3341")
    );
    assert_eq!(p["rendezvous"]["last_sync_ok"], serde_json::json!(true));
    assert_eq!(p["rendezvous"]["member_count"], serde_json::json!(4));
    let p = roaming_status_payload(ws, Some(true), serde_json::Value::Null)
        .await
        .unwrap()
        .unwrap();
    assert!(
        p["rendezvous"].is_null(),
        "循环未挂 = null（前端据此显示未启用）"
    );
}

#[test]
fn test_cluster_gate_payload_reads_both_config_layers() {
    // 集群闸链快照：与 gateway cluster_should_start 同源两道闸的读面回归。
    // 用户只开叠加层（roaming.enabled）而本体闸未开时
    // 重启永远不生效——payload 必须把闸链原值带出来，前端才能区分
    // 「待重启」与「集群未启用」。
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let ws = dir.path().join("workspace");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&ws).unwrap();
    let home_s = home.to_str().unwrap();
    let ws_s = ws.to_str().unwrap();

    // 双闸全关（两配置文件都缺）：open=false、原值 false、token_broken 透传
    let g = cluster_gate_payload(Some(home_s), ws_s, false);
    assert_eq!(g["main_enabled"], serde_json::json!(false));
    assert_eq!(g["cluster_enabled"], serde_json::json!(false));
    assert_eq!(g["open"], serde_json::json!(false));
    assert_eq!(g["token_broken"], serde_json::json!(false));

    // 只开子系统闸（config.cluster.json enabled=true，实际形态）：
    // 闸链仍关——重启不生效的根因形态
    let cfg_path = cluster_config_path(ws_s);
    std::fs::create_dir_all(cfg_path.parent().unwrap()).unwrap();
    std::fs::write(
        &cfg_path,
        serde_json::json!({"enabled": true, "roaming": {"enabled": true}}).to_string(),
    )
    .unwrap();
    let g = cluster_gate_payload(Some(home_s), ws_s, false);
    assert_eq!(g["cluster_enabled"], serde_json::json!(true));
    assert_eq!(g["main_enabled"], serde_json::json!(false));
    assert_eq!(g["open"], serde_json::json!(false));

    // 补开主开关（config.json cluster.enabled）：闸链全开
    std::fs::write(
        home.join("config.json"),
        serde_json::json!({"cluster": {"enabled": true}}).to_string(),
    )
    .unwrap();
    let g = cluster_gate_payload(Some(home_s), ws_s, true);
    assert_eq!(g["main_enabled"], serde_json::json!(true));
    assert_eq!(g["open"], serde_json::json!(true));
    assert_eq!(g["token_broken"], serde_json::json!(true));

    // home 缺席（None）= 主闸诚实 false，不 panic、不臆测
    let g = cluster_gate_payload(None, ws_s, false);
    assert_eq!(g["main_enabled"], serde_json::json!(false));
    assert_eq!(g["open"], serde_json::json!(false));
}

#[tokio::test]
async fn test_roaming_set_enabled_gate_write_and_plugin_absent_note() {
    // QUIC 插件化 语义：身份备料面（keygen）在 plugin_roaming——闸写是
    // 主操作照常落盘；插件缺席（测试环境桥空）= key_created=false + node_id
    // null + identity_note 诚实注记（闸写不回滚，gateway 启动时叠加层诚实
    // 降级）。插件在场的备料语义由 live 形态覆盖（本处是宿主面回归锁）。
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    // enable：闸写生效 + 插件缺席诚实注记
    let p = roaming_set_enabled_impl(ws.to_str().unwrap(), true)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(p["key_created"], serde_json::json!(false));
    assert!(p["node_id"].is_null());
    assert!(
        p["identity_note"]
            .as_str()
            .unwrap()
            .contains("plugin_roaming"),
        "缺席注记要点名插件: {:?}",
        p["identity_note"]
    );
    assert!(!roaming_key_path(ws).exists());
    // config 闸同步为 true
    assert!(Cluster::roaming_enabled_in_config(&cluster_config_path(
        ws.to_str().unwrap()
    )));
    // 二次 enable：闸幂等、注记稳定
    let p2 = roaming_set_enabled_impl(ws.to_str().unwrap(), true)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(p2["key_created"], serde_json::json!(false));
    assert!(p2["node_id"].is_null());
    // disable：只落闸，requires_restart=true
    let p3 = roaming_set_enabled_impl(ws.to_str().unwrap(), false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(p3["enabled"], serde_json::json!(false));
    assert_eq!(p3["requires_restart"], serde_json::json!(true));
    assert!(!Cluster::roaming_enabled_in_config(&cluster_config_path(
        ws.to_str().unwrap()
    )));
}

// QUIC 自建中继管理面 ---

#[test]
fn test_roaming_enabled_write_preserves_relay_siblings() {
    // 回归锁：roaming.enabled 单键写曾整段替换 roaming 对象，把段内兄弟键
    // 静默抹掉（relay WSAPI 面上线前修复）。 解耦后
    // QUIC 键真相源迁 config.quic.json，cluster.json 段内残留 = 无害遗留
    //（首读迁移只拷贝不删除）——单键写依旧不碰它们。
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_str().unwrap();
    let cfg_path = cluster_config_path(ws);
    std::fs::create_dir_all(cfg_path.parent().unwrap()).unwrap();
    std::fs::write(
        &cfg_path,
        r#"{
  "enabled": true,
  "token": "cluster-tok",
  "roaming": {
    "enabled": true,
    "relay_url": "http://vps.example.com:3340",
    "relay_token": "client-tok",
    "relay": { "enabled": true, "http_port": 3340, "tls": "manual" }
  }
}"#,
    )
    .unwrap();

    update_cluster_config_roaming(ws, false).unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    // 单键翻转生效
    assert_eq!(v["roaming"]["enabled"], serde_json::json!(false));
    // 段内兄弟键 + 顶层兄弟键全保留
    assert_eq!(
        v["roaming"]["relay_url"],
        serde_json::json!("http://vps.example.com:3340")
    );
    assert_eq!(v["roaming"]["relay_token"], serde_json::json!("client-tok"));
    assert_eq!(v["roaming"]["relay"]["tls"], serde_json::json!("manual"));
    assert_eq!(v["roaming"]["relay"]["enabled"], serde_json::json!(true));
    assert_eq!(v["enabled"], serde_json::json!(true));
    assert_eq!(v["token"], serde_json::json!("cluster-tok"));
}

// （三期）：roaming.trust.* 互信 allowlist 管理面（表面契约） ---

/// 测试身份：真实 ed25519 派生（管理面入参走 node_id_from_hex on-curve
/// 校验——与生产同口径）。递增 seed：多次调用互异。
fn valid_iroh_hex(seed: u64) -> String {
    let mut s = [0u8; 32];
    s[..8].copy_from_slice(&seed.to_le_bytes());
    hex::encode(
        ed25519_dalek::SigningKey::from_bytes(&s)
            .verifying_key()
            .as_bytes(),
    )
}

/// 真「非曲线点」编码：随机 32 字节约 50% 过 on-curve（0xAA×32 实证通过），
/// 固定魔数不可靠——运行时从计数器 seed 找一个 node_id_from_hex 明确拒绝
/// 的形态（找不到 = 校验器本身坏了，测试自爆）。
fn invalid_iroh_hex() -> String {
    for seed in 1..1000u64 {
        let mut s = [0u8; 32];
        s[..8].copy_from_slice(&seed.to_le_bytes());
        let candidate = hex::encode(s);
        if nemesis_cluster::roaming::node_id_from_hex(&candidate).is_err() {
            return candidate;
        }
    }
    panic!("1000 个候选里找不到非曲线点编码——on-curve 校验器疑似失效");
}

#[test]
fn test_roaming_trust_mutate_surface_contract() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_str().unwrap();

    // 簿缺席快照：enabled=false 空列表（regime 未激活）。
    let snap = roaming_trust_payload(Path::new(ws)).unwrap();
    assert_eq!(snap["enabled"], serde_json::json!(false));
    assert_eq!(snap["trusted"], serde_json::json!([]));
    assert_eq!(snap["revoked"], serde_json::json!([]));

    // add：首次写入即激活（文件出现）+ `iroh:` 前缀/大写归一入簿。
    let a = valid_iroh_hex(1);
    let p = roaming_trust_mutate_impl(ws, &format!("iroh:{}", a.to_uppercase()), TrustOp::Add)
        .unwrap()
        .unwrap();
    assert_eq!(p["added"], serde_json::json!(a));
    assert_eq!(p["trust"]["enabled"], serde_json::json!(true));
    assert_eq!(p["trust"]["trusted"], serde_json::json!([a]));
    let tp = nemesis_cluster::roaming::trust::trust_book_path(Path::new(ws));
    assert!(tp.exists(), "首次写盘 = regime 激活唯一路径");

    // revoke：从 trusted 移除 + revoked 入册留痕（响应双列表如实反映）。
    let b = valid_iroh_hex(2);
    roaming_trust_mutate_impl(ws, &b, TrustOp::Add)
        .unwrap()
        .unwrap();
    let p = roaming_trust_mutate_impl(ws, &b, TrustOp::Revoke)
        .unwrap()
        .unwrap();
    assert_eq!(p["revoked"], serde_json::json!(b));
    assert_eq!(p["trust"]["trusted"], serde_json::json!([a]), "a 不受影响");
    assert_eq!(p["trust"]["revoked"], serde_json::json!([b]));

    // unrevoke：移出 revoked，不自动恢复 trusted（重信是显式 add）。
    let p = roaming_trust_mutate_impl(ws, &b, TrustOp::Unrevoke)
        .unwrap()
        .unwrap();
    assert_eq!(p["unrevoked"], serde_json::json!(b));
    assert_eq!(p["trust"]["revoked"], serde_json::json!([]));
    assert_eq!(p["trust"]["trusted"], serde_json::json!([a]));

    // 幂等：重复 add 不产生重复条目。
    roaming_trust_mutate_impl(ws, &a, TrustOp::Add)
        .unwrap()
        .unwrap();
    let snap = roaming_trust_payload(Path::new(ws)).unwrap();
    assert_eq!(snap["trusted"], serde_json::json!([a]));

    // 垃圾 hex（非 hex 字符 / 非曲线点编码）loud 拒——管理面入簿条目影响
    // 插件 accept 闸，垃圾条目 = 无声死区，必须在门口拦。
    let err = roaming_trust_mutate_impl(ws, &invalid_iroh_hex(), TrustOp::Add)
        .expect_err("非曲线点必须拒");
    assert!(err.contains("node_id 非法"), "{err}");
    let err = roaming_trust_mutate_impl(ws, "zz", TrustOp::Add).expect_err("非 hex 必须拒");
    assert!(err.contains("node_id 非法"), "{err}");
    // 拒绝路径不得落盘变更。
    let snap = roaming_trust_payload(Path::new(ws)).unwrap();
    assert_eq!(snap["trusted"], serde_json::json!([a]));
}
