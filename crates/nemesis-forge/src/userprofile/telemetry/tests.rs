//! userprofile::telemetry 单测：写闸（config 门控）/ 30s 去重 / 轮转。
//!
//! 全局 static（DEDUP/GATE）跨测试共享：各测试用**唯一 module/cmd 键**与
//! **独立 tempdir 路径**隔离（缓存键含 config 路径，不同路径互不串扰；
//! 同进程 30s 去重窗只对同键生效）。

use super::*;
use std::path::PathBuf;

/// 摆一个 home + config.json（`ui_telemetry` 缺省 true）。
fn home_with_telemetry(dir: &std::path::Path, ui_telemetry: bool) -> PathBuf {
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let cfg = serde_json::json!({
        "user_profile": {"enabled": true, "ui_telemetry": ui_telemetry}
    });
    std::fs::write(home.join("config.json"), cfg.to_string()).unwrap();
    home
}

#[test]
fn missing_config_gate_blocks_writes() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let home = dir.path().join("empty_home");
    std::fs::create_dir_all(&home).unwrap(); // 无 config.json
    record_wsapi_call(
        Some(ws.to_str().unwrap()),
        Some(home.to_str().unwrap()),
        "mod_gate_none",
        "cmd",
        true,
    );
    assert!(
        !ws.join("logs").join("wsapi_usage.jsonl").exists(),
        "config 缺失 = 学习未配置，不落盘"
    );
}

#[test]
fn ui_telemetry_gate_respects_config_switch() {
    let dir = tempfile::tempdir().unwrap();
    // 关：不落盘（独立路径防 GATE 缓存串扰）。
    let home_off = home_with_telemetry(dir.path(), false);
    let ws_off = dir.path().join("ws_off");
    std::fs::create_dir_all(&ws_off).unwrap();
    record_wsapi_call(
        Some(ws_off.to_str().unwrap()),
        Some(home_off.to_str().unwrap()),
        "mod_gate_off",
        "cmd",
        true,
    );
    assert!(
        !ws_off.join("logs").join("wsapi_usage.jsonl").exists(),
        "ui_telemetry=false 不落盘（存储即隐私闸）"
    );
    // 开：落盘（行形态 {ts_ms, module, cmd, ok}）。
    let home_on = home_with_telemetry(dir.path().join("d2").as_path(), true);
    let ws_on = dir.path().join("ws_on");
    std::fs::create_dir_all(&ws_on).unwrap();
    record_wsapi_call(
        Some(ws_on.to_str().unwrap()),
        Some(home_on.to_str().unwrap()),
        "mod_gate_on",
        "cmd",
        false,
    );
    let path = ws_on.join("logs").join("wsapi_usage.jsonl");
    let content = std::fs::read_to_string(&path).unwrap();
    let v: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
    assert_eq!(v["module"], "mod_gate_on");
    assert_eq!(v["cmd"], "cmd");
    assert_eq!(v["ok"], false);
    assert!(v["ts_ms"].is_i64());
}

#[test]
fn dedup_window_compresses_same_cmd() {
    let dir = tempfile::tempdir().unwrap();
    let home = home_with_telemetry(dir.path(), true);
    let ws = dir.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let ws_s = ws.to_str().unwrap();
    let home_s = home.to_str().unwrap();
    // 同键两次（30s 窗内）→ 1 行。
    record_wsapi_call(Some(ws_s), Some(home_s), "mod_dedup", "same.cmd", true);
    record_wsapi_call(Some(ws_s), Some(home_s), "mod_dedup", "same.cmd", true);
    // 异键照记。
    record_wsapi_call(Some(ws_s), Some(home_s), "mod_dedup", "other.cmd", true);
    let content = std::fs::read_to_string(ws.join("logs").join("wsapi_usage.jsonl")).unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines.len(), 2, "同键 30s 压缩为一条 + 异键独立: {content}");
}

/// ok 分键（复查 2026-11-24）：失败后 30s 内重试成功，两条都落（retry_candidates
/// 依赖失败→成功对）；同 ok 轮询仍压缩为一条。
#[test]
fn dedup_key_includes_ok_fail_then_success_both_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let home = home_with_telemetry(dir.path(), true);
    let ws = dir.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let ws_s = ws.to_str().unwrap();
    let home_s = home.to_str().unwrap();
    record_wsapi_call(Some(ws_s), Some(home_s), "mod_oksplit", "cmd.x", false);
    record_wsapi_call(Some(ws_s), Some(home_s), "mod_oksplit", "cmd.x", true);
    // 同 ok（失败）仍去重。
    record_wsapi_call(Some(ws_s), Some(home_s), "mod_oksplit", "cmd.x", false);
    let content = std::fs::read_to_string(ws.join("logs").join("wsapi_usage.jsonl")).unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(
        lines.len(),
        2,
        "fail→success 对不被吞 + 同 ok 去重: {content}"
    );
    let rows: Vec<serde_json::Value> = lines
        .iter()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(rows[0]["ok"], false);
    assert_eq!(rows[1]["ok"], true);
}

#[test]
fn missing_workspace_or_home_is_noop() {
    // 旁路纪律：任一缺失静默跳过，不 panic 不落盘。
    record_wsapi_call(None, Some("h"), "mod_noop", "cmd", true);
    record_wsapi_call(Some("w"), None, "mod_noop", "cmd", true);
    record_wsapi_call(None, None, "mod_noop", "cmd", true);
}

/// 总闸（复查 2026-11-25）：`user_profile.enabled=false` = 遥测也不落
/// ——此前 `telemetry_enabled` 只看 ui_telemetry，总开关关闭时遥测照落
/// 零消费（与 route 闸/stats 语义不一致的根修）。
#[test]
fn profile_enabled_gate_blocks_telemetry_when_off() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        home.join("config.json"),
        r#"{"user_profile": {"enabled": false}}"#,
    )
    .unwrap();
    let ws = dir.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    record_wsapi_call(
        Some(ws.to_str().unwrap()),
        Some(home.to_str().unwrap()),
        "mod_master_off",
        "cmd",
        true,
    );
    assert!(
        !ws.join("logs").join("wsapi_usage.jsonl").exists(),
        "总闸关 = 遥测不落（enabled ∧ ui_telemetry）"
    );
}

#[test]
fn rotation_moves_to_old_generation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wsapi_usage.jsonl");
    // >8MB 文件 → 轮转到 .old。
    let big = vec![b'x'; ROTATE_BYTES as usize + 1];
    std::fs::write(&path, &big).unwrap();
    rotate_if_oversized(&path);
    assert!(!path.exists(), "主文件已轮转");
    let mut old = path.clone().into_os_string();
    old.push(".old");
    assert_eq!(
        std::fs::metadata(PathBuf::from(old)).unwrap().len() as usize,
        big.len()
    );
    // 未超限不动。
    let path2 = dir.path().join("small.jsonl");
    std::fs::write(&path2, b"small").unwrap();
    rotate_if_oversized(&path2);
    assert!(path2.exists());
}
