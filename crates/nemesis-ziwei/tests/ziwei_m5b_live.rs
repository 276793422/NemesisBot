#![cfg(windows)]
//! M5b 实弹（BUG-20261028-98 红证+回归锁）：recover_confirm 锚重生必须
//! 忠实于锚自身。
//!
//! 两个缺陷同段（verify.rs recover_confirm 重生分支）：
//! ① `find(|r| r.function == f.semantic && r.hit)` 只按函数过滤——同函数
//!    多个 byte_pattern 锚时永远取第一个命中锚的 found_rva，第二个锚的
//!    pattern 会被错写成第一个锚命中位置的载荷（锚集损坏）；
//! ② `old_len = pattern.split_whitespace().count()`——连续 hex 形态
//!    （"909090…"，parse_byte_pattern 明确接受的常态输入，candidates 的
//!    payload 即无空格形态）算出 1 而非字节数，重生 pattern 被截断成单字节。
//!
//! V1 镜像字节未变 → 全接受找回的正确语义 = regenerated_anchors == 0 且
//! 锚集 pattern 逐字保持（m5 主测试腿 E 是单函数单锚空格形态，本测试补
//! 多锚同函数 + 连续形态两个缺口）。
//!
//! ⚠ 独立测试文件（独立进程）是刻意的：主 m5_live 的 m5_verify 测试断言
//! 进程级 EVENT_HOOK 的精确事件序列——同进程并行跑会收到本测试 recover
//! 触发的 ziwei.recovered 而假红。

use std::path::{Path, PathBuf};
use std::process::Command;

use nemesis_ziwei::profile::{
    Anchor, AnchorSet, AnchorType, Conv, FunctionDef, LocateKind, LocateSpec, Machine, Profile,
    ProfileStatus, ProfileStore, SCHEMA_VERSION, TargetFingerprint, Verification, VerifyPolicy,
};
use nemesis_ziwei::verify::recover_confirm;

const DUMMY_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../test-tools/dummy-targets"
);

/// 夹具（缺席则现场构建；失败 = 诚实 SKIP）。与 m5_live 同纪律。
fn variant_exe(bin_name: &str, features: &[&str]) -> Option<PathBuf> {
    let p = Path::new(DUMMY_ROOT).join("bin").join("x64").join(bin_name);
    if p.is_file() {
        return Some(p);
    }
    let mut cmd = Command::new("cargo");
    cmd.args(["build", "--release", "--target", "x86_64-pc-windows-msvc"])
        .arg("--manifest-path")
        .arg(Path::new(DUMMY_ROOT).join("dummy-draw").join("Cargo.toml"));
    if !features.is_empty() {
        cmd.arg("--features").arg(features.join(","));
    }
    let built = cmd.status();
    let src = Path::new(DUMMY_ROOT)
        .join("dummy-draw")
        .join("target")
        .join("x86_64-pc-windows-msvc")
        .join("release")
        .join("dummy-draw.exe");
    match built {
        Ok(s) if s.success() && src.is_file() => {
            if std::fs::copy(&src, &p).is_ok() {
                Some(p)
            } else {
                None
            }
        }
        _ => {
            eprintln!("SKIP: {bin_name} 现场构建失败——集成前置不满足");
            None
        }
    }
}

fn temp_ws() -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "ziwei-m5b-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn pattern_loc(p: &str) -> LocateSpec {
    LocateSpec {
        kind: LocateKind::BytePattern,
        module: None,
        pattern: Some(p.to_string()),
        rva: None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn recover_confirm_regen_keeps_anchors_faithful() {
    let Some(exe_v1) = variant_exe("dummy-draw.exe", &[]) else {
        return;
    };
    let ws = temp_ws();
    let store = ProfileStore::new(&ws);
    let id = "zw-pro-m5b";

    let f = |semantic: &str, pattern: &str| FunctionDef {
        semantic: semantic.into(),
        locate: pattern_loc(pattern),
        conv: Conv::MsX64,
        args: vec![],
        capture: None,
    };
    let mut profile = Profile {
        schema_version: SCHEMA_VERSION,
        profile_id: id.into(),
        name: "m5b multi-anchor regen".into(),
        created_at: chrono::Utc::now(),
        last_verified_at: chrono::Utc::now(),
        status: ProfileStatus::Active,
        target: TargetFingerprint {
            exe_name: "dummy-draw.exe".into(),
            exe_path_hint: None,
            machine: Machine::X64,
            file_sha256: Some("f".repeat(64)), // 与 V1 无关——confirm 不看 hash
            version_resource: Default::default(),
            verify_policy: VerifyPolicy::HashOrVersion,
        },
        attach: Default::default(),
        functions: vec![
            f("twin_a", "90 90 90 90 90 90 90 90 B8 8D 7C 6B 5A"),
            f("compact_fn", "90 90 90 90 90 90 90 90 B8 0D F0 AD 0B"),
        ],
        data_structures: vec![],
        tools: vec![],
        verification: Verification {
            on_attach: vec!["file_sha256".into()],
            stale_action: None,
        },
    };
    profile.status = ProfileStatus::Stale; // 找回入口前提（直接摆位；confirm 只认 Stale）

    // twin_a：同函数双 byte_pattern 锚（V1 中都命中、位置不同）；
    // compact_fn：单锚，连续 hex 形态（无空格）。
    let a = |function: &str, pattern: &str| Anchor {
        function: function.into(),
        kind: AnchorType::BytePattern,
        module: None,
        pattern: Some(pattern.into()),
        text: None,
        distance: None,
        value: None,
        imports: None,
        bb_hash: None,
        note: None,
    };
    let anchors = AnchorSet {
        profile_id: id.into(),
        anchors: vec![
            a("twin_a", "90 90 90 90 90 90 90 90 B8 8D 7C 6B 5A"),
            a("twin_a", "90 90 90 90 90 90 90 90 B8 33 22 11 00"),
            a("compact_fn", "9090909090909090B80DF0AD0B"),
        ],
    };
    store
        .save(&profile, Some(&anchors))
        .expect("profile+anchors save");

    let rep = recover_confirm(
        &store,
        &ws,
        id,
        &exe_v1,
        &["twin_a".to_string(), "compact_fn".to_string()],
    )
    .expect("confirm");
    assert_eq!(
        rep["all_recovered"], true,
        "两函数锚在 V1 全命中应全找回：{rep}"
    );
    assert_eq!(
        rep["regenerated_anchors"], 0,
        "V1 字节未变 = 锚逐字保持，不得重生（重生即错位/截断）：{rep}"
    );

    // 锚集逐字未动（双锚各自 pattern 保持原值；连续形态不被截断成单字节）。
    let reloaded = store.load_anchors(id).expect("reload anchors");
    assert_eq!(
        reloaded.anchors[0].pattern.as_deref(),
        Some("90 90 90 90 90 90 90 90 B8 8D 7C 6B 5A"),
        "twin_a 锚0 pattern 须保持"
    );
    assert_eq!(
        reloaded.anchors[1].pattern.as_deref(),
        Some("90 90 90 90 90 90 90 90 B8 33 22 11 00"),
        "twin_a 锚1 pattern 须保持（不得被写成锚0 命中位置的载荷）"
    );
    assert_eq!(
        reloaded.anchors[2].pattern.as_deref(),
        Some("9090909090909090B80DF0AD0B"),
        "compact_fn 连续形态 pattern 须保持（不得截断成单字节）"
    );

    let _ = nemesis_utils::fsutil::remove_dir_all_retry(&ws);
}
