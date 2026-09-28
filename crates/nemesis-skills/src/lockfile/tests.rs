//! lockfile 测试：roundtrip + 三路维护 + 漂移检测。

use super::*;

fn write_skill_dir(root: &Path, slug: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
    let dir = root.join("skills").join(slug);
    for (path, content) in files {
        let full = dir.join(path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&full, content).unwrap();
    }
    dir
}

#[test]
fn test_load_missing_file_returns_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let lock = SkillsLockfile::load(tmp.path());
    assert!(lock.skills.is_empty());
    assert_eq!(lock.version, 1);
}

#[test]
fn test_corrupt_file_warns_and_continues() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(SkillsLockfile::path_for(tmp.path()), "{ not json").unwrap();
    let lock = SkillsLockfile::load(tmp.path());
    assert!(lock.skills.is_empty());
}

/// M4（2026-09-27）：损坏文件降级为空表前先备份 .bak（save 会整写覆盖，
/// 不备份就丢排查证据）。
#[test]
fn test_corrupt_file_backed_up_before_fallback() {
    let tmp = tempfile::tempdir().unwrap();
    let path = SkillsLockfile::path_for(tmp.path());
    std::fs::write(&path, "{ not json").unwrap();

    let lock = SkillsLockfile::load(tmp.path());
    assert!(lock.skills.is_empty());

    let bak = path.with_extension("json.bak");
    assert!(bak.exists(), "损坏文件必须先备份为 {:?} 再降级", bak);
    assert_eq!(
        std::fs::read_to_string(&bak).unwrap(),
        "{ not json",
        ".bak 内容必须是损坏现场原样"
    );
}

#[test]
fn test_intact_file_not_backed_up() {
    let tmp = tempfile::tempdir().unwrap();
    let mut lock = SkillsLockfile::new();
    lock.record(LockedSkill {
        slug: "ok".to_string(),
        source: "s".to_string(),
        commit: String::new(),
        files: BTreeMap::new(),
        installed_at: 1,
        verified_state: "trusted".to_string(),
    });
    lock.save(tmp.path()).unwrap();

    let loaded = SkillsLockfile::load(tmp.path());
    assert_eq!(loaded.get("ok").unwrap().slug, "ok");
    assert!(
        !SkillsLockfile::path_for(tmp.path())
            .with_extension("json.bak")
            .exists(),
        "完好文件不得产生 .bak"
    );
}

#[test]
fn test_record_save_load_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let mut lock = SkillsLockfile::new();
    let mut files = BTreeMap::new();
    files.insert("SKILL.md".to_string(), "abc".to_string());
    files.insert("refs/x.md".to_string(), "def".to_string());
    lock.record(LockedSkill {
        slug: "demo".to_string(),
        source: "github:acme/demo".to_string(),
        commit: "c0ffee".to_string(),
        files: files.clone(),
        installed_at: 1_700_000_000,
        verified_state: "review-required".to_string(),
    });
    lock.save(tmp.path()).unwrap();

    let loaded = SkillsLockfile::load(tmp.path());
    let entry = loaded.get("demo").unwrap();
    assert_eq!(entry.source, "github:acme/demo");
    assert_eq!(entry.files, files);
    assert_eq!(entry.verified_state, "review-required");
}

#[test]
fn test_remove_and_update() {
    let mut lock = SkillsLockfile::new();
    lock.record(LockedSkill {
        slug: "a".to_string(),
        source: "s".to_string(),
        commit: String::new(),
        files: BTreeMap::new(),
        installed_at: 1,
        verified_state: "trusted".to_string(),
    });
    // 更新 = 覆盖。
    lock.record(LockedSkill {
        slug: "a".to_string(),
        source: "s2".to_string(),
        commit: "x".to_string(),
        files: BTreeMap::new(),
        installed_at: 2,
        verified_state: "trusted".to_string(),
    });
    assert_eq!(lock.get("a").unwrap().source, "s2");
    assert!(lock.remove("a"));
    assert!(!lock.remove("a"));
    assert!(lock.get("a").is_none());
}

#[test]
fn test_compute_dir_hashes_skips_dotfiles() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = write_skill_dir(
        tmp.path(),
        "demo",
        &[("SKILL.md", "hello"), ("sub/ref.md", "world")],
    );
    std::fs::write(dir.join(".signature"), "{}").unwrap();
    std::fs::write(dir.join(".skill-origin.json"), "{}").unwrap();

    let hashes = SkillsLockfile::compute_dir_hashes(&dir).unwrap();
    assert_eq!(hashes.len(), 2);
    assert!(hashes.contains_key("SKILL.md"));
    assert!(hashes.contains_key("sub/ref.md"));
    // 确定性：两次计算一致。
    let again = SkillsLockfile::compute_dir_hashes(&dir).unwrap();
    assert_eq!(hashes, again);
}

#[test]
fn test_verify_drift_clean() {
    let tmp = tempfile::tempdir().unwrap();
    write_skill_dir(tmp.path(), "demo", &[("SKILL.md", "v1")]);
    let mut lock = SkillsLockfile::new();
    let hashes = SkillsLockfile::compute_dir_hashes(&tmp.path().join("skills/demo")).unwrap();
    lock.record(LockedSkill {
        slug: "demo".to_string(),
        source: "s".to_string(),
        commit: String::new(),
        files: hashes,
        installed_at: 1,
        verified_state: "trusted".to_string(),
    });
    let report = lock.verify_drift(tmp.path(), "demo");
    assert!(report.clean, "{:?}", report.summary());
}

#[test]
fn test_verify_drift_detects_modification_and_missing() {
    let tmp = tempfile::tempdir().unwrap();
    write_skill_dir(tmp.path(), "demo", &[("SKILL.md", "v1"), ("a.txt", "aaa")]);
    let mut lock = SkillsLockfile::new();
    let hashes = SkillsLockfile::compute_dir_hashes(&tmp.path().join("skills/demo")).unwrap();
    lock.record(LockedSkill {
        slug: "demo".to_string(),
        source: "s".to_string(),
        commit: String::new(),
        files: hashes,
        installed_at: 1,
        verified_state: "trusted".to_string(),
    });

    // 改一个 + 删一个。
    std::fs::write(tmp.path().join("skills/demo/SKILL.md"), "tampered").unwrap();
    std::fs::remove_file(tmp.path().join("skills/demo/a.txt")).unwrap();

    let report = lock.verify_drift(tmp.path(), "demo");
    assert!(!report.clean);
    assert_eq!(report.modified_files, vec!["SKILL.md".to_string()]);
    assert_eq!(report.missing_files, vec!["a.txt".to_string()]);
    assert!(report.summary().contains("已修改"));
    assert!(report.summary().contains("已丢失"));
}

#[test]
fn test_verify_drift_detects_new_file() {
    let tmp = tempfile::tempdir().unwrap();
    write_skill_dir(tmp.path(), "demo", &[("SKILL.md", "v1")]);
    let mut lock = SkillsLockfile::new();
    let hashes = SkillsLockfile::compute_dir_hashes(&tmp.path().join("skills/demo")).unwrap();
    lock.record(LockedSkill {
        slug: "demo".to_string(),
        source: "s".to_string(),
        commit: String::new(),
        files: hashes,
        installed_at: 1,
        verified_state: "trusted".to_string(),
    });
    std::fs::write(tmp.path().join("skills/demo/injected.sh"), "boom").unwrap();

    let report = lock.verify_drift(tmp.path(), "demo");
    assert!(!report.clean);
    assert_eq!(report.new_files, vec!["injected.sh".to_string()]);
}

#[test]
fn test_verify_drift_unknown_skill_and_missing_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let lock = SkillsLockfile::new();
    let report = lock.verify_drift(tmp.path(), "ghost");
    assert!(!report.clean);

    // 记账在、目录被整删。
    let mut lock2 = SkillsLockfile::new();
    let mut files = BTreeMap::new();
    files.insert("SKILL.md".to_string(), "x".to_string());
    lock2.record(LockedSkill {
        slug: "gone".to_string(),
        source: "s".to_string(),
        commit: String::new(),
        files,
        installed_at: 1,
        verified_state: "trusted".to_string(),
    });
    let report2 = lock2.verify_drift(tmp.path(), "gone");
    assert!(!report2.clean);
    assert_eq!(report2.missing_files, vec!["SKILL.md".to_string()]);
}

#[test]
fn test_binary_file_hash_stable() {
    // 非 UTF-8 文件按字节哈希不炸。
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("skills/bin");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("logo.bin"), [0u8, 159, 146, 150]).unwrap();
    let hashes = SkillsLockfile::compute_dir_hashes(&dir).unwrap();
    assert_eq!(hashes.len(), 1);
}
