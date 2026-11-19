//! instance_identity 单测（W1 #31）：复用/损坏重生成/回落不炸。

use super::*;
use std::fs;

#[test]
fn creates_and_persists_identity() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    let id1 = read_or_create(&root).unwrap();
    assert!(!id1.instance_id.trim().is_empty());
    assert!(!id1.created_at.is_empty());
    assert!(root.join("identity.json").exists());

    // 复用：再读同 root → 同一 id（workspace 级持久语义）。
    let id2 = read_or_create(&root).unwrap();
    assert_eq!(id1.instance_id, id2.instance_id);
    // 盘上形态可独立解析。
    let raw = fs::read_to_string(root.join("identity.json")).unwrap();
    let parsed: InstanceIdentity = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed.instance_id, id1.instance_id);
}

#[test]
fn corrupt_or_invalid_file_regenerates() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    fs::create_dir_all(&root).unwrap();

    // 损坏 JSON → 视为不存在，重新生成。
    fs::write(root.join("identity.json"), "not-json{{{").unwrap();
    let id = read_or_create(&root).unwrap();
    assert!(!id.instance_id.is_empty());

    // 空 id → 同样重新生成。
    fs::write(
        root.join("identity.json"),
        r#"{"instance_id": "  ", "created_at": "x"}"#,
    )
    .unwrap();
    let id2 = read_or_create(&root).unwrap();
    assert!(!id2.instance_id.trim().is_empty());
    assert_ne!(id2.instance_id.trim(), "");
}

#[test]
fn resolve_or_fallback_survives_unwritable_root() {
    // root 是「文件」不是目录 → create_dir_all 失败 → 瞬态回落（不炸）。
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("blocker");
    fs::write(&blocker, "x").unwrap();
    let id = resolve_or_fallback(&blocker);
    assert!(!id.instance_id.is_empty());
    // 幂等性：同 root 反复解析不炸（fs 失败时每次瞬态 id 允许不同）。
    let _ = resolve_or_fallback(&blocker);
}
