//! verify 门面单测（T5.1 纯逻辑面；PE 依赖的降级/找回链路在 m5_live 实弹）。

use super::{pre_attach_guard, sha256_hex, verify_profile, write_once_report};
use crate::profile::{
    AttachInfo, FunctionDef, LocateKind, LocateSpec, Machine, Profile, ProfileStatus, ProfileStore,
    SCHEMA_VERSION, TargetFingerprint, Verification, VerifyPolicy,
};
use std::path::PathBuf;

fn minimal_profile(file_sha256: Option<String>) -> Profile {
    Profile {
        schema_version: SCHEMA_VERSION,
        profile_id: "zw-pro-vt0001".to_string(),
        name: "verify-test".to_string(),
        created_at: chrono::Utc::now(),
        last_verified_at: chrono::Utc::now(),
        status: ProfileStatus::Active,
        target: TargetFingerprint {
            exe_name: "target.exe".to_string(),
            exe_path_hint: None,
            machine: Machine::X64,
            file_sha256,
            version_resource: Default::default(),
            verify_policy: VerifyPolicy::HashOrVersion,
        },
        attach: AttachInfo::default(),
        functions: vec![FunctionDef {
            semantic: "推送".into(),
            locate: LocateSpec {
                kind: LocateKind::BytePattern,
                module: None,
                pattern: Some("90 90".into()),
                rva: None,
            },
            conv: nemesis_ziwei_profile::model::Conv::MsX64,
            args: vec![],
            capture: None,
        }],
        data_structures: vec![],
        tools: vec![],
        verification: Verification {
            on_attach: vec!["file_sha256".into(), "anchors_vote>=1".into()],
            stale_action: Some("notify_user".into()),
        },
    }
}

fn tempdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ziwei_verify_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("tempdir");
    d
}

#[test]
fn status_transition_matrix() {
    use ProfileStatus::{Active, Revoked, Stale};
    assert!(Active.can_transition_to(Stale).is_ok());
    assert!(Stale.can_transition_to(Active).is_ok());
    assert!(Active.can_transition_to(Revoked).is_ok());
    assert!(Stale.can_transition_to(Revoked).is_ok());
    // 同态 no-op
    assert!(Active.can_transition_to(Active).is_ok());
    assert!(Stale.can_transition_to(Stale).is_ok());
    // Revoked 不可复活
    assert!(Revoked.can_transition_to(Active).is_err());
    assert!(Revoked.can_transition_to(Stale).is_err());
}

#[test]
fn pre_attach_guards() {
    let mut p = minimal_profile(Some("a".repeat(64)));
    assert!(pre_attach_guard(&p).is_ok());
    p.status = ProfileStatus::Stale;
    let e = pre_attach_guard(&p).unwrap_err();
    assert!(
        e.contains("已失效") && e.contains("recover"),
        "stale 文案带找回引导：{e}"
    );
    p.status = ProfileStatus::Revoked;
    assert!(pre_attach_guard(&p).unwrap_err().contains("已吊销"));
}

#[test]
fn first_verification_baselines_hash() {
    let ws = tempdir("baseline");
    let store = ProfileStore::new(&ws);
    let p = minimal_profile(None);
    store.save(&p, None).expect("入库");
    let exe = ws.join("target.exe");
    std::fs::write(&exe, b"V1-PAYLOAD").unwrap();
    let out = verify_profile(&store, &ws, &p, &exe).expect("首次验证");
    assert_eq!(out.report.verdict, super::Verdict::Active);
    assert!(out.report.notes.iter().any(|n| n.contains("基准已锚定")));
    let updated = out.profile_updated.expect("基准写回");
    assert_eq!(
        updated.target.file_sha256.as_deref(),
        Some(sha256_hex(b"V1-PAYLOAD").as_str())
    );
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn hash_match_is_active_without_rescan() {
    let ws = tempdir("hashok");
    let store = ProfileStore::new(&ws);
    let h = sha256_hex(b"V1-PAYLOAD");
    let p = minimal_profile(Some(h.clone()));
    store.save(&p, None).expect("入库");
    let exe = ws.join("target.exe");
    std::fs::write(&exe, b"V1-PAYLOAD").unwrap();
    let out = verify_profile(&store, &ws, &p, &exe).expect("验证");
    assert_eq!(out.report.verdict, super::Verdict::Active);
    assert!(out.report.hash_match);
    assert!(out.report.anchors.is_none(), "hash 一致不重扫");
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn revoked_profile_refuses_verify() {
    let ws = tempdir("revoked");
    let store = ProfileStore::new(&ws);
    let mut p = minimal_profile(Some("a".repeat(64)));
    p.status = ProfileStatus::Revoked;
    let exe = ws.join("target.exe");
    std::fs::write(&exe, b"x").unwrap();
    let e = verify_profile(&store, &ws, &p, &exe).unwrap_err();
    assert!(e.contains("已吊销"));
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn write_once_report_dedups_same_hash() {
    let ws = tempdir("once");
    let body = "{}";
    let (p1, e1) =
        write_once_report(&ws, "zw-pro-x", "deadbeef00deadbeef00", "stale", body).unwrap();
    assert!(e1, "首次落盘");
    let (p2, e2) =
        write_once_report(&ws, "zw-pro-x", "deadbeef00deadbeef00", "stale", body).unwrap();
    assert_eq!(p1, p2);
    assert!(!e2, "同 (profile, hash) 不重复打扰");
    // 不同 hash = 新事件，重新报告（marker 按前 8 hex 截断）
    let (_, e3) =
        write_once_report(&ws, "zw-pro-x", "feedface11feedface11", "stale", body).unwrap();
    assert!(e3);
    let _ = std::fs::remove_dir_all(&ws);
}

// 注：stale/degraded 全链（锚投票 → 状态机 → 通知/事件 → 一次性去重）依赖
// 真实 PE 镜像做锚重扫——实弹覆盖在 tests/ziwei_m5_live.rs（dummy-draw 真
// 夹具），此处不留假 PE。
