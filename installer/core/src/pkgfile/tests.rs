use super::*;
use anyhow::Result;

#[test]
fn sanitize_entry_path_matrix() {
    let p = |s: &str| sanitize_entry_path(std::path::Path::new(s));
    // 正常形态。
    assert_eq!(p("bin/nemesisbot.exe").unwrap(), "bin/nemesisbot.exe");
    assert_eq!(
        p(r"bin\nemesisbot.exe").unwrap(),
        "bin/nemesisbot.exe",
        "反斜杠归一"
    );
    assert_eq!(p("README.md").unwrap(), "README.md");

    // 绝对路径。
    assert!(p("/etc/passwd").is_err());
    assert!(p(r"C:\Windows\system32\evil.exe").is_err(), "盘符形态拒绝");

    // 越界。
    assert!(p("../evil").is_err());
    assert!(p("a/../../evil").is_err());
    assert!(p("a/../b").is_err());

    // 空段。
    assert!(p("").is_err());
    assert!(p("a//b").is_err());
    assert!(p("a/").is_err());
}

#[test]
fn take_region_bounds_and_streaming() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let f_path = tmp.path().join("f.bin");
    let body: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(&f_path, &body)?;
    let mut f = std::fs::File::open(&f_path)?;

    // 中段区间。
    let mut r = take_region(&mut f, 10, 20)?;
    let mut got = Vec::new();
    r.read_to_end(&mut got)?;
    assert_eq!(got, &body[10..30]);

    // 越界区间：take 截断到文件尾（Take 语义），长度对账由调用方 hash 门兜。
    let mut f = std::fs::File::open(&f_path)?;
    let mut r = take_region(&mut f, 990, 1000)?;
    let mut got = Vec::new();
    r.read_to_end(&mut got)?;
    assert_eq!(got.len(), 10, "Take 不越文件尾");
    Ok(())
}

#[test]
fn verify_payload_region_ok_and_mismatch() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let f_path = tmp.path().join("f.bin");
    let body = b"payload-body-0123456789".repeat(64);
    std::fs::write(&f_path, &body)?;

    let sha = sha2::Sha256::digest(&body);
    let manifest = crate::manifest::Manifest {
        format_version: crate::FORMAT_VERSION,
        bot_version: "t".into(),
        platform: "windows".into(),
        created_at: 0,
        payload_offset: 0,
        payload_len: body.len() as u64,
        payload_sha256: hex::encode(sha),
        total_size: 0,
        files: vec![],
    };
    let mut f = std::fs::File::open(&f_path)?;
    verify_payload_region(&mut f, &manifest)?;

    // 篡改一个字节 → mismatch。
    let mut bad = body.clone();
    bad[7] ^= 0xff;
    std::fs::write(&f_path, &bad)?;
    let mut f = std::fs::File::open(&f_path)?;
    assert!(
        verify_payload_region(&mut f, &manifest).is_err(),
        "单字节改动必须红"
    );
    Ok(())
}

#[test]
fn inspect_rejects_tiny_and_garbage() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let tiny = tmp.path().join("tiny");
    std::fs::write(&tiny, b"ab")?;
    assert!(inspect(&tiny).is_err());

    let garbage = tmp.path().join("garbage");
    std::fs::write(&garbage, vec![0xEEu8; 8192])?;
    assert!(inspect(&garbage).is_err(), "无尾注 = 非安装包");
    Ok(())
}

#[test]
fn progress_enum_shapes() {
    // 进度事件形状（GUI 进度条接线契约）。
    let p = Progress::Hashing(0.5);
    assert!(matches!(p, Progress::Hashing(v) if (0.0..=1.0).contains(&v)));
    let p = Progress::Extracting(3, 10);
    assert!(matches!(p, Progress::Extracting(done, total) if done <= total));
    // 可克隆（跨线程投递）。
    let q = p.clone();
    assert_eq!(format!("{q:?}"), format!("{p:?}"));
}
