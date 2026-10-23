use super::*;

#[test]
fn sha256_hex_shape() {
    assert_eq!(sha256_hex(b"ziwei").len(), 64);
    assert_eq!(
        sha256_hex(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

#[test]
fn missing_arch_rejected() {
    let err = ensure_payload(Path::new("C:/nonexistent-zw"), None, "arm").unwrap_err();
    assert!(err.to_string().contains("未内嵌"));
}

#[test]
fn payload_dir_override_direct() {
    // 覆盖目录：文件不存在 → IO 诚实错误（不回退内嵌）
    let err = ensure_payload(
        Path::new("C:/nonexistent-zw"),
        Some("C:/nonexistent-zw2"),
        "x64",
    )
    .unwrap_err();
    assert!(matches!(err, PayloadError::Io { .. }));
}
