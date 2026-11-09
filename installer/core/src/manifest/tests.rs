use super::*;
use anyhow::Result;

fn sample(offset: u64, len: u64) -> Manifest {
    Manifest {
        format_version: crate::FORMAT_VERSION,
        bot_version: "1.0.0".into(),
        platform: "windows".into(),
        created_at: 1700000000,
        payload_offset: offset,
        payload_len: len,
        payload_sha256: "aa".repeat(32),
        total_size: 10,
        files: vec![
            ManifestFileEntry {
                path: "a.txt".into(),
                sha256: "bb".repeat(32),
                size: 6,
                mode: 0o644,
            },
            ManifestFileEntry {
                path: "bin/b.txt".into(),
                sha256: "cc".repeat(32),
                size: 4,
                mode: 0o755,
            },
        ],
    }
}

#[test]
fn json_roundtrip_preserves_fields() -> Result<()> {
    let m = sample(0, 100);
    let bytes = m.to_json_vec()?;
    let back = Manifest::from_json_slice(&bytes)?;
    assert_eq!(back.format_version, m.format_version);
    assert_eq!(back.files, m.files);
    assert_eq!(back.payload_sha256, m.payload_sha256);
    Ok(())
}

#[test]
fn validate_accepts_adjacent_sorted() -> Result<()> {
    sample(100, 200).validate(300)?;
    Ok(())
}

#[test]
fn validate_rejects_structural_violations() -> Result<()> {
    let manifest_offset = 300u64;

    // 版本不符。
    let mut m = sample(100, 200);
    m.format_version = 99;
    assert!(m.validate(manifest_offset).is_err());

    // 平台值域。
    let mut m = sample(100, 200);
    m.platform = "macos".into();
    assert!(m.validate(manifest_offset).is_err());

    // 三区不相邻。
    let m = sample(100, 199);
    assert!(m.validate(manifest_offset).is_err());

    // 溢出防御（offset+len u64 溢出不 panic）。
    let m = sample(u64::MAX - 5, 100);
    assert!(m.validate(manifest_offset).is_err());

    // 空清单。
    let mut m = sample(100, 200);
    m.files.clear();
    assert!(m.validate(manifest_offset).is_err());

    // 未排序。
    let mut m = sample(100, 200);
    m.files.reverse();
    assert!(m.validate(manifest_offset).is_err());
    Ok(())
}

#[test]
fn entry_map_lookup() -> Result<()> {
    let m = sample(0, 100);
    let map = m.entry_map();
    assert_eq!(map.len(), 2);
    assert_eq!(map.get("a.txt").unwrap().sha256, "bb".repeat(32));
    assert!(!map.contains_key("nope"));
    Ok(())
}

#[test]
fn platforms_catalog() {
    assert_eq!(PLATFORMS, ["windows", "linux"]);
}
