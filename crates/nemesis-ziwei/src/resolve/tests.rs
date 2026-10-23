//! resolve 纯函数面单测（平台中立——Linux nightly 可跑）。

use nemesis_ziwei_profile::{
    Conv, LocateKind, LocateSpec, Machine, Profile, ProfileStatus, SCHEMA_VERSION,
    TargetFingerprint, Verification, VerifyPolicy,
};

use super::{collect_profile_locates, conv_to_code, find_in, match_at};

// ── match_at / find_in ─────────────────────────────────────────────────

#[test]
fn match_at_mask_semantics() {
    let bytes = [0x48, 0x89, 0x5C];
    let mask = [1, 1, 1];
    assert!(match_at(&[0x48, 0x89, 0x5C], &bytes, &mask));
    assert!(!match_at(&[0x48, 0x8A, 0x5C], &bytes, &mask));
    // mask=0 通配
    let mask = [1, 0, 1];
    assert!(match_at(&[0x48, 0x00, 0x5C], &bytes, &mask));
    assert!(match_at(&[0x48, 0xFF, 0x5C], &bytes, &mask));
}

#[test]
fn find_in_positions() {
    let bytes = [0x90, 0xE9];
    let mask = [1, 1];
    assert_eq!(find_in(&[0x00, 0x90, 0xE9, 0x00], &bytes, &mask), Some(1));
    assert_eq!(find_in(&[0x00, 0x90, 0x00, 0xE9], &bytes, &mask), None);
    assert_eq!(find_in(&[], &bytes, &mask), None);
    assert_eq!(find_in(&[0x90], &bytes, &mask), None);
    // 尾边界贴齐
    assert_eq!(find_in(&[0xE9, 0x90], &bytes, &mask), None);
}

#[test]
fn find_in_all_wildcard_first_jump() {
    // 首字节通配：快跳键取首个非通配位
    let bytes = [0x00, 0xAB];
    let mask = [0, 1];
    assert_eq!(find_in(&[0x11, 0x22, 0xAB], &bytes, &mask), Some(1));
}

#[test]
fn find_in_pattern_longer_than_window() {
    let bytes = [0x01, 0x02, 0x03];
    let mask = [1, 1, 1];
    assert_eq!(find_in(&[0x01, 0x02], &bytes, &mask), None);
}

// ── conv_to_code ───────────────────────────────────────────────────────

#[test]
fn conv_codes_match_payload_contract() {
    assert_eq!(conv_to_code(Conv::MsX64), crate::dispatch::CONV_MSX64);
    assert_eq!(conv_to_code(Conv::Stdcall), 1);
    assert_eq!(conv_to_code(Conv::Cdecl), 2);
}

// ── collect_profile_locates ────────────────────────────────────────────

fn loc_pattern(pat: &str) -> LocateSpec {
    LocateSpec {
        kind: LocateKind::BytePattern,
        module: None,
        pattern: Some(pat.to_string()),
        rva: None,
    }
}

fn mk_profile(functions: Vec<nemesis_ziwei_profile::FunctionDef>) -> Profile {
    Profile {
        schema_version: SCHEMA_VERSION,
        profile_id: "p1".into(),
        name: "n".into(),
        created_at: chrono::Utc::now(),
        last_verified_at: chrono::Utc::now(),
        status: ProfileStatus::Active,
        target: TargetFingerprint {
            exe_name: "dummy.exe".into(),
            exe_path_hint: None,
            machine: Machine::X64,
            file_sha256: None,
            version_resource: Default::default(),
            verify_policy: VerifyPolicy::HashOrVersion,
        },
        attach: Default::default(),
        functions,
        data_structures: vec![],
        tools: vec![],
        verification: Verification {
            on_attach: vec!["file_sha256".into()],
            stale_action: None,
        },
    }
}

#[test]
fn collect_keys_functions_and_chain_steps() {
    let mut f = nemesis_ziwei_profile::FunctionDef {
        semantic: "push_record".into(),
        locate: loc_pattern("90 E9"),
        conv: Conv::MsX64,
        args: vec![],
        capture: None,
    };
    let plain = f.clone();
    // 带 2 级链的函数
    f.semantic = "feed_render".into();
    f.capture = Some(nemesis_ziwei_profile::CaptureSpec {
        decode: Some("struct:D".into()),
        emit: None,
        chain: vec![
            nemesis_ziwei_profile::ChainStep {
                locate: loc_pattern("48 89"),
                decode: None,
                note: None,
            },
            nemesis_ziwei_profile::ChainStep {
                locate: LocateSpec {
                    kind: LocateKind::Rva,
                    module: None,
                    pattern: None,
                    rva: Some(0x40),
                },
                decode: None,
                note: None,
            },
        ],
    });
    let profile = mk_profile(vec![plain, f]);

    let keys = collect_profile_locates(&profile)
        .into_iter()
        .map(|(k, _)| k)
        .collect::<Vec<_>>();
    assert!(keys.contains(&"push_record".to_string()));
    assert!(keys.contains(&"feed_render".to_string()));
    assert!(keys.contains(&"feed_render::chain0".to_string()));
    assert!(keys.contains(&"feed_render::chain1".to_string()));
    assert_eq!(keys.len(), 4, "函数 2 + 链步骤 2");
}

#[test]
fn report_to_json_shape() {
    let mut rep = super::ResolveReport::default();
    rep.addrs.insert("f".into(), 0x1400);
    rep.notes.push("g: 锚未命中".into());
    let v = rep.to_json();
    assert_eq!(v["resolved"]["f"], "0x1400");
    assert_eq!(v["notes"][0], "g: 锚未命中");
}
