//! changeset 测试（P4/E3+E8）。

use super::*;
use std::path::PathBuf;

fn temp_dir(name: &str) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "nemesis-cluster-changeset-{}-{name}-{n}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn sample() -> (ChangesetManifest, Vec<ChangesetContent>) {
    let a = b"hello common\n".to_vec();
    let b = b"fn main() {}\n".to_vec();
    let contents = vec![
        ChangesetContent {
            path: "src/common.h".into(),
            content: a.clone(),
            executable: false,
        },
        ChangesetContent {
            path: "run.sh".into(),
            content: b,
            executable: true,
        },
    ];
    let manifest = ChangesetManifest {
        version: CHANGESET_VERSION,
        base_commit: "abc123".into(),
        upserts: contents
            .iter()
            .map(|c| ChangesetUpsert {
                path: c.path.clone(),
                sha256: sha256_hex(&c.content),
                size: c.content.len() as u64,
                executable: c.executable,
            })
            .collect(),
        deletions: vec!["src/old.rs".into()],
    };
    (manifest, contents)
}

#[test]
fn write_then_read_roundtrip() {
    // 真实管线形态：write_changeset 写变更集目录本身（平面），enqueue 把它
    // 拷为载荷内 changeset/ 子目录，read_changeset 从载荷根读——这里 1:1 复刻。
    let payload_root = temp_dir("roundtrip");
    let cs_dir = payload_root.join(CHANGESET_DIR_NAME);
    let (manifest, contents) = sample();
    write_changeset(&cs_dir, &manifest, &contents).unwrap();

    // 变更集目录内布局：changeset.json + files/<rel>（平面）。
    assert!(cs_dir.join(CHANGESET_MANIFEST_NAME).exists());
    assert!(
        cs_dir
            .join(CHANGESET_FILES_DIR)
            .join("src/common.h")
            .exists()
    );

    let out = read_changeset(&payload_root).unwrap().unwrap();
    assert_eq!(out.0, manifest);
    assert_eq!(out.1.len(), 2);
    assert_eq!(out.1[0].content, b"hello common\n".to_vec());
    assert!(out.1[1].executable, "executable 标记必须随行");
    assert_eq!(out.0.deletions, vec!["src/old.rs".to_string()]);
}

#[test]
fn read_missing_manifest_is_none() {
    let dir = temp_dir("missing");
    assert!(
        read_changeset(&dir).is_none(),
        "无清单 = 纯记录交付，非错误"
    );
}

#[test]
fn read_detects_tampered_file() {
    let payload_root = temp_dir("tamper");
    let cs_dir = payload_root.join(CHANGESET_DIR_NAME);
    let (manifest, mut contents) = sample();
    write_changeset(&cs_dir, &manifest, &contents).unwrap();

    // 篡改落盘文件（绕过 write_changeset 直接改）→ sha 失配诚实拒绝。
    std::fs::write(cs_dir.join(CHANGESET_FILES_DIR).join("run.sh"), b"evil").unwrap();
    let err = read_changeset(&payload_root).unwrap().unwrap_err();
    assert!(err.contains("run.sh"), "报错必须点名失配文件: {err}");
    let _ = &mut contents;
}

#[test]
fn read_detects_missing_file() {
    let payload_root = temp_dir("absent");
    let cs_dir = payload_root.join(CHANGESET_DIR_NAME);
    let (manifest, contents) = sample();
    write_changeset(&cs_dir, &manifest, &contents).unwrap();
    std::fs::remove_file(cs_dir.join(CHANGESET_FILES_DIR).join("src/common.h")).unwrap();
    let err = read_changeset(&payload_root).unwrap().unwrap_err();
    assert!(err.contains("src/common.h"), "{err}");
}

#[test]
fn write_rejects_content_manifest_mismatch() {
    let dir = temp_dir("mismatch");
    let (manifest, mut contents) = sample();
    contents[1].path = "elsewhere.txt".into(); // 与声明错位
    let err = write_changeset(&dir, &manifest, &contents).unwrap_err();
    assert!(err.contains("错位"), "{err}");
}

#[test]
fn read_rejects_malformed_manifest_paths() {
    let dir = temp_dir("malformed");
    let (mut manifest, contents) = sample();
    manifest.upserts[0].path = "../escape.txt".into();
    // 直接写原始清单（write_changeset 会先拦——这里测读取侧自防御）。
    std::fs::create_dir_all(dir.join(CHANGESET_DIR_NAME)).unwrap();
    std::fs::write(
        dir.join(CHANGESET_DIR_NAME).join(CHANGESET_MANIFEST_NAME),
        serde_json::to_string(&manifest).unwrap(),
    )
    .unwrap();
    let err = read_changeset(&dir).unwrap().unwrap_err();
    assert!(err.contains("escape") || err.contains(".."), "{err}");
    let _ = contents;
}
