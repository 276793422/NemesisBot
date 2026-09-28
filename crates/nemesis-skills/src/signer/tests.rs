use super::*;

#[test]
fn test_generate_key_pair() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().to_string_lossy().to_string();
    let result = SkillSigner::generate_key_pair(&output).unwrap();
    assert_eq!(result, output);

    // Check files were created.
    assert!(dir.path().join("skill_sign.key").exists());
    assert!(dir.path().join("skill_sign.pub").exists());
    assert!(dir.path().join("skill_sign.meta.json").exists());

    // Check metadata is valid JSON.
    let meta: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("skill_sign.meta.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(meta["algorithm"], "ed25519");
    assert!(meta["public_key"].as_str().unwrap().len() == 64);
}

#[test]
fn test_verify_skill_no_signature_file() {
    let dir = tempfile::tempdir().unwrap();
    let signer = SkillSigner::new();
    let result = signer.verify_skill(&dir.path().to_string_lossy()).unwrap();
    assert!(!result.valid);
    assert!(result.error.contains("no .signature file"));
}

#[test]
fn test_sign_and_verify_skill() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("test-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "# Test Skill\nHello world").unwrap();

    // Generate keys.
    let key_dir = dir.path().join("keys");
    SkillSigner::generate_key_pair(&key_dir.to_string_lossy()).unwrap();

    // Sign.
    let signer = SkillSigner::new();
    let public_key = std::fs::read_to_string(key_dir.join("skill_sign.pub")).unwrap();
    signer
        .trust_store()
        .add_key(&public_key, "test-author", TrustLevel::Verified);

    let result = signer.sign_skill(
        &skill_dir.to_string_lossy(),
        &key_dir.join("skill_sign.key").to_string_lossy(),
    );
    // Sign should succeed.
    assert!(result.is_ok());

    // Verify.
    let verification = signer.verify_skill(&skill_dir.to_string_lossy()).unwrap();
    assert!(verification.valid);
    assert!(verification.trusted);
}

#[test]
fn test_sign_skill_nonexistent_dir() {
    let signer = SkillSigner::new();
    let result = signer.sign_skill("/nonexistent/path", "/nonexistent/key");
    assert!(result.is_err());
}

#[test]
fn test_verify_skill_nonexistent_dir() {
    let signer = SkillSigner::new();
    let nonexistent = format!("C:/__nonexistent_skill_dir_{}", std::process::id());
    let result = signer.verify_skill(&nonexistent);
    assert!(result.is_err());
}

// ---- New tests ----

#[test]
fn test_trust_store_add_and_check() {
    let ts = TrustStore::new(Option::<String>::None);
    assert!(ts.list_keys().is_empty());

    ts.add_key("abc123", "test-author", TrustLevel::Verified);
    let keys = ts.list_keys();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].public_key, "abc123");
    assert_eq!(keys[0].name, "test-author");
    assert_eq!(keys[0].level, TrustLevel::Verified);
}

#[test]
fn test_trust_store_multiple_keys() {
    let ts = TrustStore::new(Option::<String>::None);
    ts.add_key("key1", "author1", TrustLevel::Verified);
    ts.add_key("key2", "author2", TrustLevel::Community);
    ts.add_key("key3", "author3", TrustLevel::Unknown);

    assert_eq!(ts.list_keys().len(), 3);
}

#[test]
fn test_trust_store_is_trusted() {
    let ts = TrustStore::new(Option::<String>::None);
    ts.add_key("key1", "author1", TrustLevel::Verified);
    ts.add_key("key2", "author2", TrustLevel::Unknown);

    let (level1, ok1) = ts.is_trusted("key1");
    assert!(ok1);
    assert_eq!(level1, TrustLevel::Verified);

    let (_level2, ok2) = ts.is_trusted("key2");
    assert!(ok2); // Unknown is still "present" = trusted by is_trusted

    let (_, ok3) = ts.is_trusted("nonexistent");
    assert!(!ok3);
}

#[test]
fn test_trust_level_equality() {
    assert_eq!(TrustLevel::Unknown, TrustLevel::Unknown);
    assert_eq!(TrustLevel::Community, TrustLevel::Community);
    assert_eq!(TrustLevel::Verified, TrustLevel::Verified);
    assert_ne!(TrustLevel::Unknown, TrustLevel::Verified);
}

#[test]
fn test_signer_new() {
    let signer = SkillSigner::new();
    assert!(signer.trust_store().list_keys().is_empty());
}

#[test]
fn test_sign_skill_missing_key() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "# Test").unwrap();

    let signer = SkillSigner::new();
    let result = signer.sign_skill(&skill_dir.to_string_lossy(), "/nonexistent/key");
    assert!(result.is_err());
}

#[test]
fn test_hex_encode_empty() {
    assert_eq!(hex_encode(&[]), "");
}

#[test]
fn test_hex_encode_bytes() {
    assert_eq!(hex_encode(&[0x00]), "00");
    assert_eq!(hex_encode(&[0xff]), "ff");
    assert_eq!(hex_encode(&[0xab, 0xcd]), "abcd");
}

#[test]
fn test_hex_decode_valid() {
    let result = hex_decode("00000000000000000000000000000000000000000000000000000000000000ff");
    assert!(result.is_ok());
    assert_eq!(result.unwrap()[31], 0xff);
}

#[test]
fn test_hex_decode_wrong_length() {
    let result = hex_decode("ab");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("expected 64"));
}

#[test]
fn test_hex_decode_invalid_chars() {
    let result = hex_decode("gggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggg");
    assert!(result.is_err());
}

#[test]
fn test_compute_public_key_fingerprint() {
    let fp1 = compute_public_key_fingerprint("abc123");
    let fp2 = compute_public_key_fingerprint("abc123");
    assert_eq!(fp1, fp2);
    assert!(!fp1.is_empty());
}

#[test]
fn test_compute_public_key_fingerprint_different_inputs() {
    let fp1 = compute_public_key_fingerprint("key1");
    let fp2 = compute_public_key_fingerprint("key2");
    assert_ne!(fp1, fp2);
}

#[test]
fn test_signer_default() {
    let signer = SkillSigner::default();
    assert!(signer.trust_store().list_keys().is_empty());
}

#[test]
fn test_signer_verifier() {
    let signer = SkillSigner::new();
    let _verifier = signer.verifier();
}

#[test]
fn test_signer_with_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("trust.json").to_string_lossy().to_string();
    let signer = SkillSigner::with_persistence(&config_path);
    assert!(signer.trust_store().list_keys().is_empty());
}

#[test]
fn test_sign_skill_not_a_directory() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("not_a_dir.txt");
    std::fs::write(&file_path, "test").unwrap();

    let signer = SkillSigner::new();
    let result = signer.sign_skill(&file_path.to_string_lossy(), "/tmp/key");
    assert!(result.is_err());
}

#[test]
fn test_verify_skill_not_a_directory() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("not_a_dir.txt");
    std::fs::write(&file_path, "test").unwrap();

    let signer = SkillSigner::new();
    let result = signer.verify_skill(&file_path.to_string_lossy());
    assert!(result.is_err());
}

#[test]
fn test_sign_skill_with_invalid_key_content() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "# Test").unwrap();

    let key_path = dir.path().join("bad_key.txt");
    std::fs::write(&key_path, "not-valid-hex").unwrap();

    let signer = SkillSigner::new();
    let result = signer.sign_skill(&skill_dir.to_string_lossy(), &key_path.to_string_lossy());
    assert!(result.is_err());
}

#[test]
fn test_generate_key_pair_creates_valid_keys() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().to_string_lossy().to_string();
    SkillSigner::generate_key_pair(&output).unwrap();

    let private_key = std::fs::read_to_string(dir.path().join("skill_sign.key")).unwrap();
    let public_key = std::fs::read_to_string(dir.path().join("skill_sign.pub")).unwrap();

    assert_eq!(private_key.trim().len(), 64);
    assert_eq!(public_key.trim().len(), 64);

    for ch in private_key.trim().chars() {
        assert!(ch.is_ascii_hexdigit());
    }
    for ch in public_key.trim().chars() {
        assert!(ch.is_ascii_hexdigit());
    }
}

#[test]
fn test_key_metadata_serialization() {
    let meta = KeyMetadata {
        public_key: "abc123".to_string(),
        fingerprint: "sha256hash".to_string(),
        algorithm: "ed25519".to_string(),
    };
    let json = serde_json::to_string(&meta).unwrap();
    let parsed: KeyMetadata = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.public_key, "abc123");
    assert_eq!(parsed.algorithm, "ed25519");
}

// ============================================================
// Coverage improvement: additional signer tests
// ============================================================

#[test]
fn test_build_manifest_with_subdirectory() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("skill-with-subdir");
    let sub_dir = skill_dir.join("docs");
    std::fs::create_dir_all(&sub_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "# Main").unwrap();
    std::fs::write(sub_dir.join("guide.md"), "# Guide").unwrap();

    let signer = SkillSigner::new();
    let manifest = signer.build_manifest(&skill_dir).unwrap();
    assert_eq!(manifest.files.len(), 2);
    // 相对路径规范化：分隔符恒为 `/`（平台无关，跨平台签验一致）。
    let paths: Vec<&str> = manifest.files.iter().map(|f| f.path.as_str()).collect();
    assert!(paths.iter().any(|p| p.contains("SKILL.md")));
    assert!(paths.iter().any(|p| p.contains("guide.md")));
    assert!(
        paths.iter().all(|p| !p.contains('\\')),
        "分隔符不得进载荷: {paths:?}"
    );
    let content = String::from_utf8_lossy(&manifest.content);
    assert!(content.contains("SKILL.md"));
    assert!(content.contains("Guide"));
}

/// 签名载荷确定性（2026-09-26 复查修复回归锁）：此前拼接跟随 read_dir
/// 枚举顺序（排序只作用于 files 列表、不作用于被签名的 combined）——
/// 枚举序漂移 = 同目录两次构建载荷不同 = 签验失败/漏检。现在先排序后
/// 拼接，重复构建必须字节一致。
#[test]
fn test_build_manifest_deterministic_across_builds() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("skill");
    let sub = skill_dir.join("a/b");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(skill_dir.join("z.md"), "zeta").unwrap();
    std::fs::write(skill_dir.join("m.md"), "mu").unwrap();
    std::fs::write(sub.join("k.md"), "kappa").unwrap();

    let signer = SkillSigner::new();
    let m1 = signer.build_manifest(&skill_dir).unwrap();
    let m2 = signer.build_manifest(&skill_dir).unwrap();
    assert_eq!(m1.content, m2.content, "同目录重复构建载荷必须一致");
    assert_eq!(
        m1.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
        m2.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>()
    );
}

/// 非 UTF-8 文件（二进制资产）不再令签名/验证失败（2026-09-26 复查修复
/// 回归锁：此前 read_to_string 遇非 UTF-8 直接 Err）。
#[test]
fn test_sign_and_verify_with_binary_asset() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("binary-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "# Binary asset skill").unwrap();
    std::fs::write(skill_dir.join("logo.bin"), [0xFF, 0xFE, 0x00, 0x80, 0xC3]).unwrap();

    let key_dir = dir.path().join("keys");
    SkillSigner::generate_key_pair(&key_dir.to_string_lossy()).unwrap();

    let signer = SkillSigner::new();
    let public_key = std::fs::read_to_string(key_dir.join("skill_sign.pub")).unwrap();
    signer
        .trust_store()
        .add_key(&public_key, "test", TrustLevel::Verified);

    signer
        .sign_skill(
            &skill_dir.to_string_lossy(),
            &key_dir.join("skill_sign.key").to_string_lossy(),
        )
        .expect("含二进制资产的技能必须可签");

    let verification = signer.verify_skill(&skill_dir.to_string_lossy()).unwrap();
    assert!(verification.valid, "{:?}", verification.error);
    assert!(verification.trusted);

    // 二进制资产被篡改也要验出
    std::fs::write(skill_dir.join("logo.bin"), [0xFF, 0xFE, 0x00, 0x81, 0xC3]).unwrap();
    let verification = signer.verify_skill(&skill_dir.to_string_lossy()).unwrap();
    assert!(!verification.valid, "二进制资产篡改必须验出");
}

#[test]
fn test_build_manifest_skips_hidden_files() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "# Main").unwrap();
    std::fs::write(skill_dir.join(".hidden"), "hidden content").unwrap();

    let signer = SkillSigner::new();
    let manifest = signer.build_manifest(&skill_dir).unwrap();
    assert_eq!(manifest.files.len(), 1);
    assert_eq!(manifest.files[0].path, "SKILL.md");
}

#[test]
fn test_build_manifest_empty_directory() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("empty-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();

    let signer = SkillSigner::new();
    let manifest = signer.build_manifest(&skill_dir).unwrap();
    assert!(manifest.files.is_empty());
    assert!(manifest.content.is_empty());
}

#[test]
fn test_sign_and_verify_tampered_content() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("tampered-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "# Original").unwrap();

    let key_dir = dir.path().join("keys");
    SkillSigner::generate_key_pair(&key_dir.to_string_lossy()).unwrap();

    let signer = SkillSigner::new();
    let public_key = std::fs::read_to_string(key_dir.join("skill_sign.pub")).unwrap();
    signer
        .trust_store()
        .add_key(&public_key, "test", TrustLevel::Verified);

    signer
        .sign_skill(
            &skill_dir.to_string_lossy(),
            &key_dir.join("skill_sign.key").to_string_lossy(),
        )
        .unwrap();

    // Tamper with content after signing
    std::fs::write(skill_dir.join("SKILL.md"), "# Tampered!").unwrap();

    let verification = signer.verify_skill(&skill_dir.to_string_lossy()).unwrap();
    assert!(
        !verification.valid,
        "Tampered content should fail verification"
    );
}

#[test]
fn test_verify_skill_invalid_signature_format() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("bad-sig");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "# Test").unwrap();
    std::fs::write(
        skill_dir.join(".signature"),
        r#"{"public_key":"abc","signature":"def","files":[]}"#,
    )
    .unwrap();

    let signer = SkillSigner::new();
    let result = signer.verify_skill(&skill_dir.to_string_lossy()).unwrap();
    assert!(!result.valid);
}

#[test]
fn test_sign_skill_with_binary_key() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "# Test").unwrap();

    let key_dir = dir.path().join("keys");
    SkillSigner::generate_key_pair(&key_dir.to_string_lossy()).unwrap();

    let signer = SkillSigner::new();
    let result = signer.sign_skill(
        &skill_dir.to_string_lossy(),
        &key_dir.join("skill_sign.key").to_string_lossy(),
    );
    assert!(result.is_ok());

    // Verify the .signature file was created
    assert!(skill_dir.join(".signature").exists());
    let sig: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(skill_dir.join(".signature")).unwrap())
            .unwrap();
    assert_eq!(sig["algorithm"], "ed25519");
    assert!(sig["public_key"].as_str().unwrap().len() == 64);
    assert!(sig["signature"].as_str().unwrap().len() == 128);
}

#[test]
fn test_generate_key_pair_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().to_string_lossy().to_string();

    SkillSigner::generate_key_pair(&output).unwrap();
    let key1 = std::fs::read_to_string(dir.path().join("skill_sign.key")).unwrap();

    SkillSigner::generate_key_pair(&output).unwrap();
    let key2 = std::fs::read_to_string(dir.path().join("skill_sign.key")).unwrap();

    assert_ne!(
        key1, key2,
        "Regenerating keys should produce different keys"
    );
}

#[test]
fn test_hex_decode_roundtrip() {
    let bytes: [u8; 32] = [
        0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54, 0x32,
        0x10, 0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
        0xee, 0xff,
    ];
    let encoded = hex_encode(&bytes);
    let decoded = hex_decode(&encoded).unwrap();
    assert_eq!(bytes, decoded);
}
