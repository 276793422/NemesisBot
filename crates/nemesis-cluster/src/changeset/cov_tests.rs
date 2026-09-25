// changeset.rs 覆盖率补充测试（写入侧版本/条数/父目录创建失败/指纹不符
// 失败臂 + 读取侧版本/SHA 核验失败臂）。
//
// 豁免：117（`if let Some(parent)` 块收尾 `}`——失败臂 115-116 已由
// 「父目录位是文件」形态覆盖，成功臂由既有 roundtrip 覆盖，该行是
// llvm-cov 区域伪行）。

use super::*;
use std::path::PathBuf;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "nemesis-cluster-chgcov-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn content(path: &str, data: &[u8]) -> ChangesetContent {
    ChangesetContent {
        path: path.into(),
        content: data.to_vec(),
        executable: false,
    }
}

fn upsert_of(c: &ChangesetContent) -> ChangesetUpsert {
    ChangesetUpsert {
        path: c.path.clone(),
        sha256: sha256_hex(&c.content),
        size: c.content.len() as u64,
        executable: false,
    }
}

fn base_manifest(upserts: Vec<ChangesetUpsert>) -> ChangesetManifest {
    ChangesetManifest {
        version: CHANGESET_VERSION,
        base_commit: "abc123".into(),
        upserts,
        deletions: Vec::new(),
    }
}

/// 写入侧：版本非法 / 声明与内容条数不匹配（两道前置闸）。
#[test]
fn write_rejects_wrong_version_and_count_mismatch() {
    let dir = temp_dir("gate");
    let c = content("a.txt", b"data");

    let mut manifest = base_manifest(vec![upsert_of(&c)]);
    manifest.version = 9;
    let err = write_changeset(&dir, &manifest, std::slice::from_ref(&c)).unwrap_err();
    assert!(err.contains("变更集版本非法"), "{err}");

    // 条数不匹配：1 条声明、0 份内容。
    manifest.version = CHANGESET_VERSION;
    let err = write_changeset(&dir, &manifest, &[]).unwrap_err();
    assert!(err.contains("不匹配"), "{err}");
    assert!(err.contains("1 条 upsert"), "{err}");
}

/// 写入侧：第二条 upsert 的父目录刚被第一条写成文件 → 建父目录失败。
#[test]
fn write_parent_dir_creation_failure_is_honest_error() {
    let dir = temp_dir("parent-fail");
    let first = content("sub", b"i-become-a-file");
    let second = content("sub/x.txt", b"my parent is a file now");
    let manifest = base_manifest(vec![upsert_of(&first), upsert_of(&second)]);

    let err = write_changeset(&dir, &manifest, &[first, second]).unwrap_err();
    assert!(err.contains("建 sub/x.txt 父目录失败"), "{err}");
}

/// 写入侧：声明 sha256 与实得内容不符（大小对上也不放行）。
#[test]
fn write_sha_mismatch_is_honest_error() {
    let dir = temp_dir("sha-fail");
    let c = content("a.txt", b"real content");
    let mut up = upsert_of(&c);
    up.sha256 = sha256_hex(b"declared-but-different");
    let manifest = base_manifest(vec![up]);

    let err = write_changeset(&dir, &manifest, &[c]).unwrap_err();
    assert!(err.contains("指纹/大小与声明不符"), "{err}");
}

/// 读取侧：清单版本高于本端支持 → Some(Err)（诚实拒绝，不吞成 None）。
#[test]
fn read_rejects_future_version() {
    let payload_root = temp_dir("read-version");
    let cs = payload_root.join(CHANGESET_DIR_NAME);
    std::fs::create_dir_all(&cs).unwrap();
    let manifest = serde_json::json!({
        "version": 2,
        "base_commit": "abc",
        "upserts": [],
        "deletions": []
    });
    std::fs::write(
        cs.join(CHANGESET_MANIFEST_NAME),
        serde_json::to_string(&manifest).unwrap(),
    )
    .unwrap();

    let out = read_changeset(&payload_root).expect("清单在场必须 Some");
    let err = out.unwrap_err();
    assert!(err.contains("变更集版本非法"), "{err}");
    assert!(err.contains("本端支持 1"), "{err}");
}

/// 读取侧：文件大小对上但内容被换 → SHA-256 核验失败（半套数据不进合并）。
#[test]
fn read_sha_mismatch_is_honest_error() {
    let payload_root = temp_dir("read-sha");
    let real = b"payload bytes";
    let files = payload_root
        .join(CHANGESET_DIR_NAME)
        .join(CHANGESET_FILES_DIR);
    std::fs::create_dir_all(&files).unwrap();
    std::fs::write(files.join("a.txt"), real).unwrap();

    let mut up = upsert_of(&content("a.txt", real));
    up.sha256 = sha256_hex(b"swapped content same size!!"); // 大小不同此处无关，SHA 必炸
    let manifest = base_manifest(vec![up]);
    std::fs::write(
        payload_root
            .join(CHANGESET_DIR_NAME)
            .join(CHANGESET_MANIFEST_NAME),
        serde_json::to_string(&manifest).unwrap(),
    )
    .unwrap();

    let out = read_changeset(&payload_root).expect("清单在场必须 Some");
    let err = out.unwrap_err();
    assert!(err.contains("SHA-256 核验失败"), "{err}");
}
