//! exec 段两键热读测试（2026-11-20 C4 热生效语义）。
//!
//! 语义钉死：
//! - path 缺席 = 构造期字段旧行为（`new`/`with_max_timeout_secs` 逐字节不变）；
//! - path 在场 = mtime 变化才重读 `tools.exec` 段（首读也触发）；
//! - 热读语义与 registry 装配臂同源：开+非空 = 自定义词表（替换），
//!   其余 = 内置默认；`max_timeout_secs` 0 = 600；
//! - config 读坏 = 诚实降级保持上次生效值（缓存空 = 构造期值），不炸不静默换值。

use super::Tool;
use super::exec::{ExecTool, deny_hit_in};
use crate::context::RequestContext;

fn write_config(path: &std::path::Path, body: &str) {
    std::fs::write(path, body).unwrap();
}

fn exec_cfg(max_timeout_secs: u64, enabled: bool, patterns: &[&str]) -> String {
    // serde_json 构造：regex 词边界 `\b` 必须落盘为 JSON `\\b`（手拼会把
    // `\b` 写成 JSON 退格转义，pattern 语义静默漂移）。
    serde_json::json!({
        "tools": {
            "exec": {
                "max_timeout_secs": max_timeout_secs,
                "enable_deny_patterns": enabled,
                "custom_deny_patterns": patterns,
            }
        }
    })
    .to_string()
}

/// path 缺席 = 构造期字段：`with_max_timeout_secs` 的值生效、词表 = 内置
/// 默认（deny_hit 层既有语义，快照路径同源）。
#[test]
fn hot_read_none_uses_construction_fields() {
    let t = ExecTool::new("/tmp/ws", true).with_max_timeout_secs(1234);
    let snap = t.runtime_snapshot();
    assert_eq!(snap.max_timeout_secs, 1234);
    assert!(deny_hit_in(&snap.deny_patterns, "sudo rm -rf /").is_some());
    // 默认构造 = 600。
    let t2 = ExecTool::new("/tmp/ws", true);
    assert_eq!(t2.runtime_snapshot().max_timeout_secs, 600);
}

/// path 在场 = 首读 + mtime 变化现读：自定义词表替换语义 + max 生效；
/// 改 config 后下次快照即新值（无需重建工具）。
#[test]
fn hot_read_picks_up_config_change() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("config.json");
    write_config(&cfg_path, &exec_cfg(777, true, &["dangercmd\\b"]));

    let t = ExecTool::new("/tmp/ws", true).with_exec_config_path(Some(cfg_path.clone()));
    let snap = t.runtime_snapshot();
    assert_eq!(snap.max_timeout_secs, 777, "首读 = config 值");
    assert!(
        deny_hit_in(&snap.deny_patterns, "run dangercmd now").is_some(),
        "自定义词表命中"
    );
    assert!(
        deny_hit_in(&snap.deny_patterns, "sudo rm -rf /").is_none(),
        "自定义词表 = 替换语义（默认条目不命中）"
    );
    // 缓存命中（mtime 未变）：再取一次同值。
    assert_eq!(t.runtime_snapshot().max_timeout_secs, 777);

    // 改 config（内容变化 → mtime 变化）→ 下次快照即新值。
    std::thread::sleep(std::time::Duration::from_millis(50));
    write_config(
        &cfg_path,
        &exec_cfg(888, true, &["dangercmd\\b", "otherbad\\b"]),
    );
    let snap2 = t.runtime_snapshot();
    assert_eq!(snap2.max_timeout_secs, 888, "mtime 变化 → 现读新值");
    assert!(deny_hit_in(&snap2.deny_patterns, "dangercmd").is_some());
    assert!(
        deny_hit_in(&snap2.deny_patterns, "echo otherbad").is_some(),
        "新词表条目生效"
    );
}

/// config 读坏 = 诚实降级：保持上次生效值；首次读就坏 = 构造期值。
#[test]
fn hot_read_broken_config_keeps_last_values() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("config.json");
    write_config(&cfg_path, &exec_cfg(777, true, &["dangercmd\\b"]));

    let t = ExecTool::new("/tmp/ws", true).with_exec_config_path(Some(cfg_path.clone()));
    assert_eq!(t.runtime_snapshot().max_timeout_secs, 777);

    // 写坏 JSON → 快照保持上次热读值（777 + 旧词表），不 panic 不换值。
    std::thread::sleep(std::time::Duration::from_millis(50));
    std::fs::write(&cfg_path, "{not json").unwrap();
    let snap = t.runtime_snapshot();
    assert_eq!(snap.max_timeout_secs, 777, "读坏 → 保持上次生效值");
    assert!(deny_hit_in(&snap.deny_patterns, "dangercmd").is_some());

    // 全新工具 + 首读就坏 = 构造期值（缓存空回落）。
    let t2 = ExecTool::new("/tmp/ws", true).with_exec_config_path(Some(cfg_path.clone()));
    assert_eq!(
        t2.runtime_snapshot().max_timeout_secs,
        600,
        "首读失败 = 构造期默认"
    );
    assert!(
        deny_hit_in(&t2.runtime_snapshot().deny_patterns, "sudo rm -rf /").is_some(),
        "首读失败词表 = 构造期默认词表"
    );
}

/// 热读语义与 registry 装配臂同源：关（或缺席语义）= 内置默认词表
/// （自定义被忽略）；`max_timeout_secs` 0 = 600；缺 exec 段 = 默认。
#[test]
fn hot_read_matches_registry_assembly_semantics() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("config.json");

    // 开关关：自定义词表被忽略 → 内置默认。
    write_config(&cfg_path, &exec_cfg(500, false, &["dangercmd\\b"]));
    let t = ExecTool::new("/tmp/ws", true).with_exec_config_path(Some(cfg_path.clone()));
    let snap = t.runtime_snapshot();
    assert_eq!(snap.max_timeout_secs, 500);
    assert!(deny_hit_in(&snap.deny_patterns, "sudo rm -rf /").is_some());
    assert!(deny_hit_in(&snap.deny_patterns, "dangercmd").is_none());

    // max 0 = 600（与装配臂 `(n>0).then_some(n).unwrap_or(600)` 同源）。
    std::thread::sleep(std::time::Duration::from_millis(50));
    write_config(&cfg_path, &exec_cfg(0, true, &["dangercmd\\b"]));
    let snap2 = t.runtime_snapshot();
    assert_eq!(snap2.max_timeout_secs, 600, "0 = 600 旧行为");
    assert!(deny_hit_in(&snap2.deny_patterns, "dangercmd").is_some());

    // 缺 exec 段 = 全默认。
    std::thread::sleep(std::time::Duration::from_millis(50));
    std::fs::write(&cfg_path, r#"{"agents":{"defaults":{}}}"#).unwrap();
    let snap3 = t.runtime_snapshot();
    assert_eq!(snap3.max_timeout_secs, 600);
    assert!(deny_hit_in(&snap3.deny_patterns, "sudo rm -rf /").is_some());
}

/// execute 消费点走热读快照：deny 拒绝消息带自定义 pattern 原文
/// （端到端——不经 runtime_snapshot，直接打 execute）。
#[tokio::test]
async fn hot_read_execute_blocks_custom_pattern() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("config.json");
    write_config(&cfg_path, &exec_cfg(777, true, &["forbiddenbinary\\b"]));

    let t = ExecTool::new(dir.path().to_string_lossy().as_ref(), true)
        .with_exec_config_path(Some(cfg_path.clone()));
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let err = t
        .execute(r#"{"command":"run forbiddenbinary --now"}"#, &ctx)
        .await
        .expect_err("自定义词表必须拒绝");
    assert!(
        err.contains("forbiddenbinary"),
        "拒绝消息带 pattern 原文: {err}"
    );
    // 改 config 放行（词表换掉）→ 同一工具实例下次执行放行（真的跑命令）。
    std::thread::sleep(std::time::Duration::from_millis(50));
    write_config(&cfg_path, &exec_cfg(777, true, &["nevermatches\\b"]));
    let out = t
        .execute(r#"{"command":"echo hotread-ok"}"#, &ctx)
        .await
        .expect("换词表后放行");
    assert!(out.contains("hotread-ok"), "放行后真执行: {out}");
}
