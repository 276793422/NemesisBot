//! A1 运行时完整性清单回归锁：校验器四态（完好 / 缺件 / 篡改 / manifest
//! 缺失）+ 坏 manifest 响亮定性 + 跨平台路径形态。全程 tmpdir，无进程边界。

use super::*;
use std::io::Write;

/// 造一个最小部署目录：manifest 列 static/index.html + nemesisbot-fake.bin
/// 两件，返回 (dir, manifest)。
fn make_deploy(
    name: &str,
    index_body: &[u8],
    bin_body: &[u8],
) -> (tempfile::TempDir, RuntimeManifest) {
    let dir = tempfile::TempDir::with_prefix(name).expect("tmpdir");
    let static_dir = dir.path().join("static");
    std::fs::create_dir_all(&static_dir).expect("mkdir static");
    std::fs::write(static_dir.join("index.html"), index_body).expect("write index");
    std::fs::write(dir.path().join("nemesisbot-fake.bin"), bin_body).expect("write bin");

    let manifest = RuntimeManifest {
        version: "1.2.3".into(),
        git_commit: "6f96d056".into(),
        generated_at: "2026-10-19T00:00:00Z".into(),
        files: vec![
            ManifestEntry {
                path: "static/index.html".into(),
                sha256: sha256_hex(index_body),
                size: index_body.len() as u64,
            },
            ManifestEntry {
                path: "nemesisbot-fake.bin".into(),
                sha256: sha256_hex(bin_body),
                size: bin_body.len() as u64,
            },
        ],
    };
    (dir, manifest)
}

fn write_manifest(dir: &Path, manifest: &RuntimeManifest) {
    let raw = serde_json::to_string_pretty(manifest).expect("serialize");
    std::fs::write(dir.join(MANIFEST_FILENAME), raw).expect("write manifest");
}

/// 态 1：manifest 在且全件匹配 → Verified。
#[test]
fn verified_when_all_files_match() {
    let (dir, manifest) = make_deploy("verified", b"<html></html>", b"MZ-fake-binary");
    write_manifest(dir.path(), &manifest);
    match check_deploy_dir(dir.path()).expect("check") {
        StartupCheck::Verified { files } => assert_eq!(files, 2),
        other => panic!("expected Verified, got {other:?}"),
    }
}

/// 态 2：缺件（清单列出但盘上没有，换装中断形态）→ Failed[Missing]。
#[test]
fn missing_file_reports_violation() {
    let (dir, manifest) = make_deploy("missing", b"<html></html>", b"MZ-fake-binary");
    // 删掉前端入口，模拟 static 部分覆盖。
    std::fs::remove_file(dir.path().join("static").join("index.html")).expect("remove");
    write_manifest(dir.path(), &manifest);
    match check_deploy_dir(dir.path()).expect("check") {
        StartupCheck::Failed(v) => {
            assert_eq!(v.len(), 1);
            assert_eq!(
                v[0],
                ManifestViolation::Missing {
                    path: "static/index.html".into()
                }
            );
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}

/// 态 3：篡改（文件在但内容变，sha256 不符）→ Failed[HashMismatch]。
#[test]
fn tampered_file_reports_hash_mismatch() {
    let (dir, manifest) = make_deploy("tamper", b"<html>original</html>", b"MZ-fake-binary");
    std::fs::write(
        dir.path().join("static").join("index.html"),
        b"<html>tampered!</html>",
    )
    .expect("tamper");
    write_manifest(dir.path(), &manifest);
    match check_deploy_dir(dir.path()).expect("check") {
        StartupCheck::Failed(v) => {
            assert_eq!(v.len(), 1);
            match &v[0] {
                ManifestViolation::HashMismatch {
                    path,
                    expected,
                    actual,
                } => {
                    assert_eq!(path, "static/index.html");
                    assert_eq!(expected, &sha256_hex(b"<html>original</html>"));
                    assert_eq!(actual, &sha256_hex(b"<html>tampered!</html>"));
                    assert_ne!(expected, actual, "digests must differ");
                }
                other => panic!("expected HashMismatch, got {other:?}"),
            }
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}

/// 态 4：manifest 不在（cargo 开发态）→ Absent（不阻塞语义）。
#[test]
fn absent_manifest_is_not_blocking() {
    let dir = tempfile::TempDir::with_prefix("absent").expect("tmpdir");
    match check_deploy_dir(dir.path()).expect("check") {
        StartupCheck::Absent => {}
        other => panic!("expected Absent, got {other:?}"),
    }
}

/// 态 4b：manifest 在但坏 JSON → Err 响亮定性（不当 Absent 静默放行）。
#[test]
fn corrupt_manifest_is_loud_error() {
    let dir = tempfile::TempDir::with_prefix("corrupt").expect("tmpdir");
    let mut f = std::fs::File::create(dir.path().join(MANIFEST_FILENAME)).expect("create");
    f.write_all(b"{ not valid json").expect("write");
    let err = check_deploy_dir(dir.path()).expect_err("must error");
    assert!(
        err.contains(MANIFEST_FILENAME),
        "error must name the file: {err}"
    );
}

/// 跨平台路径形态：清单 path 用正斜杠，Windows 上按平台分隔符解析。
#[test]
fn manifest_paths_use_forward_slash_on_windows() {
    let (dir, manifest) = make_deploy("fwdslash", b"x", b"y");
    write_manifest(dir.path(), &manifest);
    // Windows 下若把 "static/index.html" 当单文件名解析会落 Missing；
    // Verified = 分隔符归一正确。
    match check_deploy_dir(dir.path()).expect("check") {
        StartupCheck::Verified { files } => assert_eq!(files, 2),
        other => panic!("expected Verified, got {other:?}"),
    }
}

/// 大小写：sha256 比较大小写不敏感（生成器小写，手工清单大写也过）。
#[test]
fn hash_comparison_is_case_insensitive() {
    let (dir, mut manifest) = make_deploy("caseins", b"<html></html>", b"MZ");
    for entry in &mut manifest.files {
        entry.sha256 = entry.sha256.to_uppercase();
    }
    write_manifest(dir.path(), &manifest);
    match check_deploy_dir(dir.path()).expect("check") {
        StartupCheck::Verified { files } => assert_eq!(files, 2),
        other => panic!("expected Verified, got {other:?}"),
    }
}
