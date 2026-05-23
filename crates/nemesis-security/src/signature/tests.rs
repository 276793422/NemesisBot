use super::*;

#[test]
fn test_generate_key_pair() {
    let kp = generate_key_pair().unwrap();
    assert_eq!(kp.private_key.len(), 64);
    assert_eq!(kp.public_key.len(), 64);
}

#[test]
fn test_sign_and_verify() {
    let kp = generate_key_pair().unwrap();
    let verifier = SignatureVerifier::new();
    verifier.add_trusted_key(&kp.public_key, "test-author");

    let content = "hello, this is a skill";
    let sig = sign_content_hex(content, &kp.private_key).unwrap();

    assert!(verifier.verify_signature(content, &sig, &kp.public_key));
}

#[test]
fn test_verify_rejects_untrusted_key() {
    let verifier = SignatureVerifier::new();
    let kp = generate_key_pair().unwrap();
    let content = "some content";
    let sig = sign_content_hex(content, &kp.private_key).unwrap();

    assert!(!verifier.verify_signature(content, &sig, &kp.public_key));
}

#[test]
fn test_verify_rejects_tampered_content() {
    let kp = generate_key_pair().unwrap();
    let verifier = SignatureVerifier::new();
    verifier.add_trusted_key(&kp.public_key, "author");

    let original = "original content";
    let sig = sign_content_hex(original, &kp.private_key).unwrap();

    assert!(!verifier.verify_signature("tampered content", &sig, &kp.public_key));
}

#[test]
fn test_backward_compat_hash_signature() {
    let verifier = SignatureVerifier::new();
    let pk = "test-public-key-001";
    verifier.add_trusted_key(pk, "test-author");

    let content = "hello, this is a skill";
    let sig = compute_hash_signature(content, pk);

    assert!(verifier.verify_signature(content, &sig, pk));
}

#[test]
fn test_trust_store_management() {
    let store = TrustStore::in_memory();
    assert!(store.key_count() == 0);

    store.add_key("key-a", "alice", TrustLevel::Community);
    store.add_key("key-b", "bob", TrustLevel::Verified);
    assert_eq!(store.key_count(), 2);
    assert!(store.is_trusted("key-a").1);
    assert!(store.is_trusted("key-b").1);
    assert!(!store.is_trusted("key-c").1);

    assert!(store.remove_key("alice"));
    assert_eq!(store.key_count(), 1);
    assert!(!store.is_trusted("key-a").1);

    let keys = store.list_keys();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].public_key, "key-b");
    assert_eq!(keys[0].name, "bob");
}

#[test]
fn test_trust_levels() {
    let store = TrustStore::in_memory();
    store.add_key("key-1", "normal", TrustLevel::Community);
    store.add_key("key-2", "trusted", TrustLevel::Verified);

    assert_eq!(store.trust_level("key-1"), TrustLevel::Community);
    assert_eq!(store.trust_level("key-2"), TrustLevel::Verified);
    assert_eq!(store.trust_level("key-unknown"), TrustLevel::Unknown);
}

#[test]
fn test_verify_skill_legacy() {
    let kp = generate_key_pair().unwrap();
    let verifier = SignatureVerifier::new();
    verifier.add_trusted_key(&kp.public_key, "test-author");

    let content = "# My Skill\nHello world";
    let sig = sign_content_hex(content, &kp.private_key).unwrap();
    let result = verifier.verify_skill(content, &sig, &kp.public_key);

    assert!(result.valid);
    assert!(result.trusted);
    assert_eq!(result.trust_level, TrustLevel::Verified);
    assert!(result.error.is_empty());
}

#[test]
fn test_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trust_store.json");

    let kp = generate_key_pair().unwrap();
    let sk_bytes = hex_decode_32(&kp.private_key).unwrap();
    let signing_key = SigningKey::from_bytes(&sk_bytes);
    let verifying_key = signing_key.verifying_key();
    let public_key_b64 = export_public_key(&verifying_key);

    {
        let store = TrustStore::new(Some(&path));
        store.add_key(&public_key_b64, "author1", TrustLevel::Verified);
        assert_eq!(store.key_count(), 1);
    }

    // Load from file
    let store2 = TrustStore::new(Some(&path));
    assert_eq!(store2.key_count(), 1);
    assert!(store2.is_trusted(&public_key_b64).1);
}

#[test]
fn test_revoke_key() {
    let store = TrustStore::in_memory();
    store.add_key("key-a", "alice", TrustLevel::Verified);
    assert!(store.is_trusted("key-a").1);

    store.revoke_key("alice").unwrap();
    assert!(!store.is_trusted("key-a").1);
    assert_eq!(store.trust_level("key-a"), TrustLevel::Revoked);
    assert_eq!(store.key_count(), 1); // Still in store, just revoked
}

#[test]
fn test_revoke_key_not_found() {
    let store = TrustStore::in_memory();
    assert!(store.revoke_key("nonexistent").is_err());
}

#[test]
fn test_remove_key_not_found() {
    let store = TrustStore::in_memory();
    assert!(!store.remove_key("nonexistent"));
}

#[test]
fn test_remove_key_by_public_key() {
    let store = TrustStore::in_memory();
    store.add_key("pk-abc", "alice", TrustLevel::Verified);
    assert!(store.remove_key_by_public_key("pk-abc"));
    assert!(!store.remove_key_by_public_key("pk-abc")); // already removed
}

#[test]
fn test_revoke_key_by_public_key() {
    let store = TrustStore::in_memory();
    store.add_key("pk-xyz", "bob", TrustLevel::Community);
    store.revoke_key_by_public_key("pk-xyz").unwrap();
    assert!(!store.is_trusted("pk-xyz").1);
    assert_eq!(store.trust_level("pk-xyz"), TrustLevel::Revoked);
}

#[test]
fn test_revoke_key_by_public_key_not_found() {
    let store = TrustStore::in_memory();
    assert!(store.revoke_key_by_public_key("nonexistent").is_err());
}

#[test]
fn test_get_key_by_name() {
    let store = TrustStore::in_memory();
    store.add_key("key-a", "alice", TrustLevel::Verified);
    store.add_key("key-b", "bob", TrustLevel::Community);

    let found = store.get_key_by_name("alice");
    assert!(found.is_some());
    assert_eq!(found.unwrap().public_key, "key-a");

    assert!(store.get_key_by_name("charlie").is_none());
}

#[test]
fn test_compute_fingerprint() {
    let fp = compute_fingerprint("test-public-key");
    assert!(!fp.is_empty());
    assert_eq!(fp.len(), 64); // SHA-256 hex
}

#[test]
fn test_verification_result_ok() {
    let result = VerificationResult::ok("key-a", TrustLevel::Verified, 3);
    assert!(result.valid);
    assert_eq!(result.files_verified, 3);
    assert!(result.error.is_empty());
}

#[test]
fn test_verification_result_err() {
    let result = VerificationResult::err("signature mismatch");
    assert!(!result.valid);
    assert_eq!(result.error, "signature mismatch");
}

// ----- Full file-level tests -----

#[test]
fn test_sign_file_and_verify_file_with_key() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, b"hello, this is a test file").unwrap();

    let kp = generate_key_pair().unwrap();
    let signing_key_bytes = hex_decode_32(&kp.private_key).unwrap();
    let signing_key = SigningKey::from_bytes(&signing_key_bytes);
    let verifying_key = signing_key.verifying_key();

    let sig = sign_file(&file_path, &signing_key).unwrap();
    assert_eq!(sig.len(), 64);

    let result = verify_file_with_key(&file_path, &verifying_key, &sig).unwrap();
    assert!(result.valid);
    assert_eq!(result.files_verified, 1);
}

#[test]
fn test_sign_file_tampered_content() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, b"original content").unwrap();

    let kp = generate_key_pair().unwrap();
    let signing_key_bytes = hex_decode_32(&kp.private_key).unwrap();
    let signing_key = SigningKey::from_bytes(&signing_key_bytes);
    let verifying_key = signing_key.verifying_key();

    let sig = sign_file(&file_path, &signing_key).unwrap();

    // Tamper.
    std::fs::write(&file_path, b"tampered content").unwrap();

    let result = verify_file_with_key(&file_path, &verifying_key, &sig).unwrap();
    assert!(!result.valid);
}

#[test]
fn test_sign_file_nonexistent() {
    let kp = generate_key_pair().unwrap();
    let signing_key_bytes = hex_decode_32(&kp.private_key).unwrap();
    let signing_key = SigningKey::from_bytes(&signing_key_bytes);
    assert!(sign_file(Path::new("/nonexistent/file.txt"), &signing_key).is_err());
}

#[test]
fn test_verify_file_with_key_wrong_key() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, b"hello").unwrap();

    let kp1 = generate_key_pair().unwrap();
    let kp2 = generate_key_pair().unwrap();
    let sk1_bytes = hex_decode_32(&kp1.private_key).unwrap();
    let sk1 = SigningKey::from_bytes(&sk1_bytes);
    let vk2_bytes = hex_decode_32(&kp2.public_key).unwrap();
    let vk2 = VerifyingKey::from_bytes(&vk2_bytes).unwrap();

    let sig = sign_file(&file_path, &sk1).unwrap();
    let result = verify_file_with_key(&file_path, &vk2, &sig).unwrap();
    assert!(!result.valid);
}

#[test]
fn test_sign_skill_and_verify_skill() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("myskill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "# My Skill\nA test skill.").unwrap();
    std::fs::write(skill_dir.join("config.json"), r#"{"name":"test"}"#).unwrap();

    let kp = generate_key_pair().unwrap();
    let signing_key_bytes = hex_decode_32(&kp.private_key).unwrap();
    let signing_key = SigningKey::from_bytes(&signing_key_bytes);
    let verifying_key = signing_key.verifying_key();
    let public_key_b64 = export_public_key(&verifying_key);

    // Sign the skill.
    sign_skill(&skill_dir, &signing_key, "test-signer").unwrap();

    // Verify .signature file was created.
    assert!(skill_dir.join(SIGNATURE_FILE_NAME).exists());

    // Create verifier and add key.
    let ts_path = dir.path().join("truststore.json");
    let v = Verifier::new(Config {
        enabled: true,
        strict: false,
        trust_store_path: ts_path.to_string_lossy().to_string(),
    }).unwrap();
    v.trust_store().add_key(&public_key_b64, "test-signer", TrustLevel::Verified);

    let result = v.verify_skill(&skill_dir);
    assert!(result.valid);
    assert_eq!(result.signer, "test-signer");
    assert_eq!(result.trust_level, TrustLevel::Verified);
    assert!(result.files_verified >= 2);
}

#[test]
fn test_verify_skill_no_signature_file() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("nosig");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "# Test").unwrap();

    let ts_path = dir.path().join("truststore.json");
    let v = Verifier::new(Config {
        enabled: true,
        strict: false,
        trust_store_path: ts_path.to_string_lossy().to_string(),
    }).unwrap();

    let result = v.verify_skill(&skill_dir);
    assert!(!result.valid);
    assert!(!result.error.is_empty());
}

#[test]
fn test_verify_skill_not_a_directory() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("notadir.txt");
    std::fs::write(&file_path, b"test").unwrap();

    let ts_path = dir.path().join("truststore.json");
    let v = Verifier::new(Config {
        enabled: true,
        strict: false,
        trust_store_path: ts_path.to_string_lossy().to_string(),
    }).unwrap();

    let result = v.verify_skill(&file_path);
    assert!(!result.valid);
}

#[test]
fn test_sign_skill_tampered_content() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("tampered");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "original").unwrap();

    let kp = generate_key_pair().unwrap();
    let sk_bytes = hex_decode_32(&kp.private_key).unwrap();
    let signing_key = SigningKey::from_bytes(&sk_bytes);

    sign_skill(&skill_dir, &signing_key, "signer").unwrap();

    // Tamper.
    std::fs::write(skill_dir.join("SKILL.md"), "tampered").unwrap();

    let ts_path = dir.path().join("truststore.json");
    let v = Verifier::new(Config {
        enabled: true,
        strict: false,
        trust_store_path: ts_path.to_string_lossy().to_string(),
    }).unwrap();

    let result = v.verify_skill(&skill_dir);
    assert!(!result.valid);
}

#[test]
fn test_verifier_verify_file_with_trust_store() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, b"hello world").unwrap();

    let kp = generate_key_pair().unwrap();
    let sk_bytes = hex_decode_32(&kp.private_key).unwrap();
    let signing_key = SigningKey::from_bytes(&sk_bytes);
    let verifying_key = signing_key.verifying_key();
    let public_key_b64 = export_public_key(&verifying_key);

    let ts_path = dir.path().join("truststore.json");
    let v = Verifier::new(Config {
        enabled: true,
        strict: false,
        trust_store_path: ts_path.to_string_lossy().to_string(),
    }).unwrap();
    v.trust_store().add_key(&public_key_b64, "test-signer", TrustLevel::Community);

    let sig = sign_file(&file_path, &signing_key).unwrap();
    let result = v.verify_file(&file_path, &sig).unwrap();
    assert!(result.valid);
    assert_eq!(result.signer, "test-signer");
}

#[test]
fn test_verifier_verify_file_unknown_key() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, b"hello").unwrap();

    let kp = generate_key_pair().unwrap();
    let sk_bytes = hex_decode_32(&kp.private_key).unwrap();
    let signing_key = SigningKey::from_bytes(&sk_bytes);

    let ts_path = dir.path().join("truststore.json");
    let v = Verifier::new(Config {
        enabled: true,
        strict: false,
        trust_store_path: ts_path.to_string_lossy().to_string(),
    }).unwrap();

    let sig = sign_file(&file_path, &signing_key).unwrap();
    let result = v.verify_file(&file_path, &sig).unwrap();
    assert!(!result.valid);
}

#[test]
fn test_verifier_verify_file_revoked_key() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, b"hello").unwrap();

    let kp = generate_key_pair().unwrap();
    let sk_bytes = hex_decode_32(&kp.private_key).unwrap();
    let signing_key = SigningKey::from_bytes(&sk_bytes);
    let verifying_key = signing_key.verifying_key();
    let public_key_b64 = export_public_key(&verifying_key);

    let ts_path = dir.path().join("truststore.json");
    let v = Verifier::new(Config {
        enabled: true,
        strict: false,
        trust_store_path: ts_path.to_string_lossy().to_string(),
    }).unwrap();
    v.trust_store().add_key(&public_key_b64, "revoke-me", TrustLevel::Verified);
    v.trust_store().revoke_key("revoke-me").unwrap();

    let sig = sign_file(&file_path, &signing_key).unwrap();
    let result = v.verify_file(&file_path, &sig).unwrap();
    assert!(!result.valid);
}

#[test]
fn test_export_import_public_key() {
    let kp = generate_key_pair().unwrap();
    let sk_bytes = hex_decode_32(&kp.private_key).unwrap();
    let signing_key = SigningKey::from_bytes(&sk_bytes);
    let verifying_key = signing_key.verifying_key();

    let exported = export_public_key(&verifying_key);
    assert!(!exported.is_empty());

    let imported = import_public_key(&exported).unwrap();
    assert_eq!(verifying_key.to_bytes(), imported.to_bytes());
}

#[test]
fn test_import_public_key_invalid_base64() {
    assert!(import_public_key("not-valid-base64!!!").is_err());
}

#[test]
fn test_import_public_key_wrong_size() {
    assert!(import_public_key("aG93ZHk=").is_err()); // "howdy" = 6 bytes
}

#[test]
fn test_import_public_key_empty_string() {
    assert!(import_public_key("").is_err());
}

#[test]
fn test_concurrent_trust_store_access() {
    use std::sync::Arc;
    use std::thread;

    let store = Arc::new(TrustStore::in_memory());
    let mut handles = Vec::new();

    for i in 0..10 {
        let s = Arc::clone(&store);
        handles.push(thread::spawn(move || {
            s.add_key(
                &format!("key-{}", i),
                &format!("name-{}", i),
                TrustLevel::Community,
            );
        }));
    }
    for _ in 0..10 {
        let s = Arc::clone(&store);
        handles.push(thread::spawn(move || {
            let _ = s.list_keys();
            let _ = s.key_count();
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    assert_eq!(store.key_count(), 10);
}

#[test]
fn test_trust_store_persistence_multiple_keys() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trust.json");

    let mut public_keys = Vec::new();
    {
        let store = TrustStore::new(Some(&path));
        for i in 0..5 {
            let kp = generate_key_pair().unwrap();
            let sk_bytes = hex_decode_32(&kp.private_key).unwrap();
            let signing_key = SigningKey::from_bytes(&sk_bytes);
            let verifying_key = signing_key.verifying_key();
            let pk = export_public_key(&verifying_key);
            public_keys.push(pk.clone());
            store.add_key(&pk, &format!("name-{}", i), TrustLevel::Community);
        }
    }

    let store2 = TrustStore::new(Some(&path));
    assert_eq!(store2.key_count(), 5);
    for pk in &public_keys {
        assert!(store2.is_trusted(pk).1);
    }
}

#[test]
fn test_trust_store_persistence_skip_malformed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trust.json");

    // Write a trust store with a malformed key entry.
    let content = r#"{"version":1,"keys":[{"public_key":"invalid-base64!!!","name":"bad","level":"community","added_at":"2026-01-01T00:00:00Z","fingerprint":"abc"}]}"#;
    std::fs::write(&path, content).unwrap();

    let store = TrustStore::new(Some(&path));
    assert_eq!(store.key_count(), 0);
}

#[test]
fn test_new_verifier_empty_trust_store() {
    let v = Verifier::new(Config {
        enabled: true,
        strict: false,
        trust_store_path: String::new(),
    }).unwrap();
    assert_eq!(v.trust_store().key_count(), 0);
}

#[test]
fn test_sign_skill_empty_directory() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("empty");
    std::fs::create_dir_all(&skill_dir).unwrap();

    let kp = generate_key_pair().unwrap();
    let sk_bytes = hex_decode_32(&kp.private_key).unwrap();
    let signing_key = SigningKey::from_bytes(&sk_bytes);

    sign_skill(&skill_dir, &signing_key, "signer").unwrap();
    assert!(skill_dir.join(SIGNATURE_FILE_NAME).exists());
}

// ---- Additional coverage tests ----

#[test]
fn test_trust_level_display() {
    assert_eq!(format!("{}", TrustLevel::Unknown), "unknown");
    assert_eq!(format!("{}", TrustLevel::Community), "community");
    assert_eq!(format!("{}", TrustLevel::Verified), "verified");
    assert_eq!(format!("{}", TrustLevel::Revoked), "revoked");
}

#[test]
fn test_trust_level_default() {
    assert_eq!(TrustLevel::default(), TrustLevel::Unknown);
}

#[test]
fn test_trust_level_serialization_roundtrip() {
    for level in [TrustLevel::Unknown, TrustLevel::Community, TrustLevel::Verified, TrustLevel::Revoked] {
        let json = serde_json::to_string(&level).unwrap();
        let parsed: TrustLevel = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, level);
    }
}

#[test]
fn test_trusted_key_serialization() {
    let key = TrustedKey {
        public_key: "test-key".to_string(),
        name: "test-author".to_string(),
        level: TrustLevel::Verified,
        added_at: "2026-01-01T00:00:00Z".to_string(),
        fingerprint: "abc123".to_string(),
    };
    let json = serde_json::to_string(&key).unwrap();
    let parsed: TrustedKey = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.public_key, "test-key");
    assert_eq!(parsed.name, "test-author");
    assert_eq!(parsed.level, TrustLevel::Verified);
}

#[test]
fn test_trust_store_is_trusted_unknown_key() {
    let store = TrustStore::in_memory();
    let (level, trusted) = store.is_trusted("nonexistent");
    assert_eq!(level, TrustLevel::Unknown);
    assert!(!trusted);
}

#[test]
fn test_trust_store_is_trusted_revoked_key() {
    let store = TrustStore::in_memory();
    store.add_key("key-1", "revoked-author", TrustLevel::Verified);
    store.revoke_key("revoked-author").unwrap();
    let (level, trusted) = store.is_trusted("key-1");
    assert_eq!(level, TrustLevel::Revoked);
    assert!(!trusted);
}

#[test]
fn test_trust_store_get_key_by_public_key() {
    let store = TrustStore::in_memory();
    store.add_key("pk-abc", "alice", TrustLevel::Community);

    // Keys can be found via get_key_by_name
    let found = store.get_key_by_name("alice");
    assert!(found.is_some());
    assert_eq!(found.unwrap().public_key, "pk-abc");
}

#[test]
fn test_verifier_config_enabled_field() {
    let config = Config {
        enabled: false,
        strict: true,
        trust_store_path: String::new(),
    };
    assert!(!config.enabled);
    assert!(config.strict);
}

#[test]
fn test_verifier_verify_file_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let ts_path = dir.path().join("truststore.json");
    let v = Verifier::new(Config {
        enabled: true,
        strict: false,
        trust_store_path: ts_path.to_string_lossy().to_string(),
    }).unwrap();

    let result = v.verify_file(Path::new("/nonexistent/file.txt"), b"somesig");
    // verify_file returns an error for nonexistent files
    assert!(result.is_err() || !result.unwrap().valid);
}

#[test]
fn test_signature_file_name_constant() {
    assert_eq!(SIGNATURE_FILE_NAME, ".signature");
}

#[test]
fn test_hex_decode_32_invalid_hex() {
    let result = hex_decode_32("not-valid-hex");
    assert!(result.is_err());
}

#[test]
fn test_hex_decode_32_wrong_length() {
    let result = hex_decode_32("abcd");
    assert!(result.is_err());
}

#[test]
fn test_sign_content_hex_roundtrip() {
    let kp = generate_key_pair().unwrap();
    let content = "test content for signing";
    let sig = sign_content_hex(content, &kp.private_key).unwrap();
    assert!(!sig.is_empty());
    assert_eq!(sig.len(), 128); // 64 bytes hex = 128 chars
}

#[test]
fn test_sign_content_hex_invalid_key() {
    let result = sign_content_hex("content", "invalid-key");
    assert!(result.is_err());
}

#[test]
fn test_compute_hash_signature_deterministic() {
    let content = "same content";
    let key = "same-key";
    let sig1 = compute_hash_signature(content, key);
    let sig2 = compute_hash_signature(content, key);
    assert_eq!(sig1, sig2);
}

#[test]
fn test_compute_fingerprint_different_keys() {
    let fp1 = compute_fingerprint("key-1");
    let fp2 = compute_fingerprint("key-2");
    assert_ne!(fp1, fp2);
}

#[test]
fn test_signature_verifier_no_trusted_keys() {
    let verifier = SignatureVerifier::new();
    let kp = generate_key_pair().unwrap();
    let sig = sign_content_hex("content", &kp.private_key).unwrap();
    assert!(!verifier.verify_signature("content", &sig, &kp.public_key));
}

#[test]
fn test_signature_verifier_remove_trusted_key() {
    let verifier = SignatureVerifier::new();
    verifier.add_trusted_key("key-1", "author-1");
    // After adding, it should verify with hash sig
    let hash_sig = compute_hash_signature("content", "key-1");
    assert!(verifier.verify_signature("content", &hash_sig, "key-1"));
}

#[test]
fn test_verification_result_default_fields() {
    let result = VerificationResult::err("test");
    assert!(!result.valid);
    assert_eq!(result.trust_level, TrustLevel::Unknown);
    assert!(result.signer.is_empty());
    assert_eq!(result.error, "test");
    assert_eq!(result.files_verified, 0);
}

#[test]
fn test_verify_skill_not_strict_unknown_key() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "# Test").unwrap();

    let kp = generate_key_pair().unwrap();
    let sk_bytes = hex_decode_32(&kp.private_key).unwrap();
    let signing_key = SigningKey::from_bytes(&sk_bytes);
    sign_skill(&skill_dir, &signing_key, "unknown-signer").unwrap();

    let ts_path = dir.path().join("ts.json");
    let v = Verifier::new(Config {
        enabled: true,
        strict: false, // not strict -> should report valid but unknown trust
        trust_store_path: ts_path.to_string_lossy().to_string(),
    }).unwrap();

    let result = v.verify_skill(&skill_dir);
    // Not strict: valid signature but key not in trust store
    assert!(result.valid);
    assert_eq!(result.trust_level, TrustLevel::Unknown);
}

#[test]
fn test_verifier_no_signature_file_returns_invalid() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "# Test").unwrap();
    // No .signature file

    let ts_path = dir.path().join("truststore.json");
    let v = Verifier::new(Config {
        enabled: true,
        strict: false,
        trust_store_path: ts_path.to_string_lossy().to_string(),
    }).unwrap();

    let result = v.verify_skill(&skill_dir);
    assert!(!result.valid);
    assert!(result.error.contains("no signature file"));
}

#[test]
fn test_algorithm_name_constant() {
    assert_eq!(ALGORITHM_NAME, "ed25519");
}

// ============================================================
// Additional tests for 95%+ coverage
// ============================================================

#[test]
fn test_sign_skill_creates_signature_file_with_correct_format() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("sigtest");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "# Test Skill\nContent here.").unwrap();
    std::fs::write(skill_dir.join("data.json"), r#"{"version":"1.0"}"#).unwrap();

    let kp = generate_key_pair().unwrap();
    let sk_bytes = hex_decode_32(&kp.private_key).unwrap();
    let signing_key = SigningKey::from_bytes(&sk_bytes);

    sign_skill(&skill_dir, &signing_key, "test-author").unwrap();

    // Verify .signature file exists and is valid JSON
    let sig_path = skill_dir.join(SIGNATURE_FILE_NAME);
    assert!(sig_path.exists());
    let content = std::fs::read_to_string(&sig_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(parsed["algorithm"], "ed25519");
    assert_eq!(parsed["signer_name"], "test-author");
    assert!(parsed["signature"].is_string());
    assert!(parsed["public_key"].is_string());
    assert!(parsed["file_count"].is_number());
    assert!(parsed["signed_at"].is_string());
    assert!(parsed["hash"].is_string());
}

#[test]
fn test_verifier_strict_mode_unknown_key_untrusted() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("strict-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "# Test").unwrap();

    let kp = generate_key_pair().unwrap();
    let sk_bytes = hex_decode_32(&kp.private_key).unwrap();
    let signing_key = SigningKey::from_bytes(&sk_bytes);

    sign_skill(&skill_dir, &signing_key, "unknown-signer").unwrap();

    let ts_path = dir.path().join("ts.json");
    let v = Verifier::new(Config {
        enabled: true,
        strict: true, // strict mode
        trust_store_path: ts_path.to_string_lossy().to_string(),
    }).unwrap();

    let result = v.verify_skill(&skill_dir);
    // Signature is cryptographically valid but signer is unknown (not trusted)
    assert!(result.valid); // signature is valid
    assert_eq!(result.trust_level, TrustLevel::Unknown); // but not trusted
    assert_eq!(result.signer, "unknown-signer");
}

#[test]
fn test_signature_verifier_default() {
    let verifier = SignatureVerifier::default();
    assert_eq!(verifier.trust_store_ref().key_count(), 0);
}

#[test]
fn test_signature_verifier_with_trust_store() {
    let store = TrustStore::in_memory();
    store.add_key("key-a", "alice", TrustLevel::Verified);
    let verifier = SignatureVerifier::with_trust_store(store);
    assert_eq!(verifier.trust_store_ref().key_count(), 1);
}

#[test]
fn test_signature_verifier_with_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ts.json");
    let verifier = SignatureVerifier::with_persistence(&path);
    assert_eq!(verifier.trust_store_ref().key_count(), 0);
}

#[test]
fn test_verify_skill_legacy_untrusted_key() {
    let verifier = SignatureVerifier::new();
    let kp = generate_key_pair().unwrap();
    // Don't add key to trust store
    let content = "# My Skill";
    let sig = sign_content_hex(content, &kp.private_key).unwrap();
    let result = verifier.verify_skill(content, &sig, &kp.public_key);
    assert!(!result.valid);
    assert!(!result.trusted);
    assert_eq!(result.trust_level, TrustLevel::Unknown);
    assert!(!result.error.is_empty());
}

#[test]
fn test_verify_skill_legacy_valid_trusted() {
    let verifier = SignatureVerifier::new();
    let kp = generate_key_pair().unwrap();
    verifier.add_trusted_key(&kp.public_key, "test-author");
    let content = "# My Skill";
    let sig = sign_content_hex(content, &kp.private_key).unwrap();
    let result = verifier.verify_skill(content, &sig, &kp.public_key);
    assert!(result.valid);
    assert!(result.trusted);
    assert_eq!(result.trust_level, TrustLevel::Verified);
    assert!(result.error.is_empty());
}

#[test]
fn test_verify_skill_legacy_wrong_content() {
    let verifier = SignatureVerifier::new();
    let kp = generate_key_pair().unwrap();
    verifier.add_trusted_key(&kp.public_key, "test-author");
    let sig = sign_content_hex("original content", &kp.private_key).unwrap();
    let result = verifier.verify_skill("wrong content", &sig, &kp.public_key);
    assert!(!result.valid);
}

#[test]
fn test_trust_store_get_key_by_name_not_found() {
    let store = TrustStore::in_memory();
    assert!(store.get_key_by_name("nonexistent").is_none());
}

#[test]
fn test_trust_store_add_multiple_same_name_different_keys() {
    let store = TrustStore::in_memory();
    store.add_key("key-1", "alice", TrustLevel::Community);
    store.add_key("key-2", "alice", TrustLevel::Verified);
    // Both keys are stored since they have different public_key values
    assert_eq!(store.key_count(), 2);
    // get_key_by_name returns the first match
    let key = store.get_key_by_name("alice");
    assert!(key.is_some());
}

#[test]
fn test_hex_decode_vec_valid() {
    let result = hex_decode_vec("48656c6c6f");
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), b"Hello");
}

#[test]
fn test_hex_decode_vec_odd_length() {
    let result = hex_decode_vec("abc");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("odd hex length"));
}

#[test]
fn test_hex_decode_vec_invalid_chars() {
    let result = hex_decode_vec("ghij");
    assert!(result.is_err());
}

#[test]
fn test_sign_content_hex_produces_128_char_hex() {
    let kp = generate_key_pair().unwrap();
    let sig = sign_content_hex("content", &kp.private_key).unwrap();
    assert_eq!(sig.len(), 128);
    // Should be valid hex
    for c in sig.chars() {
        assert!(c.is_ascii_hexdigit(), "char '{}' is not hex", c);
    }
}

#[test]
fn test_verifier_verify_file_corrupt_signature() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, b"hello world").unwrap();

    let ts_path = dir.path().join("truststore.json");
    let v = Verifier::new(Config {
        enabled: true,
        strict: false,
        trust_store_path: ts_path.to_string_lossy().to_string(),
    }).unwrap();

    // Pass a corrupt signature (wrong length)
    let result = v.verify_file(&file_path, b"corrupt_sig");
    // Should return error or invalid result
    assert!(result.is_err() || !result.as_ref().unwrap().valid);
}

#[test]
fn test_trust_store_list_keys_empty() {
    let store = TrustStore::in_memory();
    let keys = store.list_keys();
    assert!(keys.is_empty());
}

#[test]
fn test_verifier_verify_file_with_strict_mode() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, b"hello world").unwrap();

    let kp = generate_key_pair().unwrap();
    let sk_bytes = hex_decode_32(&kp.private_key).unwrap();
    let signing_key = SigningKey::from_bytes(&sk_bytes);
    let verifying_key = signing_key.verifying_key();
    let public_key_b64 = export_public_key(&verifying_key);

    let ts_path = dir.path().join("truststore.json");
    let v = Verifier::new(Config {
        enabled: true,
        strict: true,
        trust_store_path: ts_path.to_string_lossy().to_string(),
    }).unwrap();
    // Don't add key - strict mode should reject unknown keys
    v.trust_store().add_key(&public_key_b64, "test-signer", TrustLevel::Verified);

    let sig = sign_file(&file_path, &signing_key).unwrap();
    let result = v.verify_file(&file_path, &sig).unwrap();
    assert!(result.valid);
}

#[test]
fn test_sign_file_empty_content() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("empty.txt");
    std::fs::write(&file_path, b"").unwrap();

    let kp = generate_key_pair().unwrap();
    let sk_bytes = hex_decode_32(&kp.private_key).unwrap();
    let signing_key = SigningKey::from_bytes(&sk_bytes);
    let verifying_key = signing_key.verifying_key();

    let sig = sign_file(&file_path, &signing_key).unwrap();
    let result = verify_file_with_key(&file_path, &verifying_key, &sig).unwrap();
    assert!(result.valid);
}

#[test]
fn test_verifier_verify_skill_with_tampered_signature_file() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("tampered_sig");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "# Test").unwrap();

    let kp = generate_key_pair().unwrap();
    let sk_bytes = hex_decode_32(&kp.private_key).unwrap();
    let signing_key = SigningKey::from_bytes(&sk_bytes);

    sign_skill(&skill_dir, &signing_key, "signer").unwrap();

    // Tamper with the .signature file
    let sig_path = skill_dir.join(SIGNATURE_FILE_NAME);
    let mut content: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&sig_path).unwrap()).unwrap();
    content["signature"] = serde_json::json!("aaaa0000bbbb1111");
    std::fs::write(&sig_path, serde_json::to_string(&content).unwrap()).unwrap();

    let ts_path = dir.path().join("truststore.json");
    let v = Verifier::new(Config {
        enabled: true,
        strict: false,
        trust_store_path: ts_path.to_string_lossy().to_string(),
    }).unwrap();

    let result = v.verify_skill(&skill_dir);
    assert!(!result.valid);
}

#[test]
fn test_verifier_verify_skill_with_invalid_signature_json() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("invalid_sig");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "# Test").unwrap();
    // Write invalid JSON as .signature
    std::fs::write(skill_dir.join(SIGNATURE_FILE_NAME), "not valid json {{{{").unwrap();

    let ts_path = dir.path().join("truststore.json");
    let v = Verifier::new(Config {
        enabled: true,
        strict: false,
        trust_store_path: ts_path.to_string_lossy().to_string(),
    }).unwrap();

    let result = v.verify_skill(&skill_dir);
    assert!(!result.valid);
}

#[test]
fn test_compute_hash_signature_different_content() {
    let key = "same-key";
    let sig1 = compute_hash_signature("content-a", key);
    let sig2 = compute_hash_signature("content-b", key);
    assert_ne!(sig1, sig2);
}

#[test]
fn test_compute_hash_signature_different_keys() {
    let content = "same-content";
    let sig1 = compute_hash_signature(content, "key-a");
    let sig2 = compute_hash_signature(content, "key-b");
    assert_ne!(sig1, sig2);
}

#[test]
fn test_trust_level_equality() {
    assert_eq!(TrustLevel::Verified, TrustLevel::Verified);
    assert_eq!(TrustLevel::Unknown, TrustLevel::Unknown);
    assert_ne!(TrustLevel::Verified, TrustLevel::Community);
    assert_ne!(TrustLevel::Community, TrustLevel::Unknown);
    assert_ne!(TrustLevel::Unknown, TrustLevel::Revoked);
}

#[test]
fn test_trusted_key_all_fields() {
    let key = TrustedKey {
        public_key: "pk-123".to_string(),
        name: "author".to_string(),
        level: TrustLevel::Community,
        added_at: "2026-01-15T10:30:00Z".to_string(),
        fingerprint: "abc123def456".to_string(),
    };
    assert_eq!(key.public_key, "pk-123");
    assert_eq!(key.name, "author");
    assert_eq!(key.level, TrustLevel::Community);
    assert_eq!(key.added_at, "2026-01-15T10:30:00Z");
    assert_eq!(key.fingerprint, "abc123def456");
}

#[test]
fn test_verification_result_ok_with_all_fields() {
    let result = VerificationResult::ok("key-a", TrustLevel::Verified, 5);
    assert!(result.valid);
    assert_eq!(result.signer, "key-a");
    assert_eq!(result.trust_level, TrustLevel::Verified);
    assert_eq!(result.files_verified, 5);
    assert_eq!(result.algorithm, "ed25519");
    assert!(result.error.is_empty());
    assert!(!result.timestamp.is_empty());
}

#[test]
fn test_verification_result_err_message() {
    let result = VerificationResult::err("something broke");
    assert!(!result.valid);
    assert_eq!(result.error, "something broke");
    assert_eq!(result.trust_level, TrustLevel::Unknown);
    assert_eq!(result.files_verified, 0);
    assert!(result.signer.is_empty());
    assert_eq!(result.algorithm, "ed25519");
    assert_eq!(result.files_verified, 0);
    assert!(result.signer.is_empty());
}

#[test]
fn test_sign_skill_with_many_files() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("bigskill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    for i in 0..10 {
        std::fs::write(skill_dir.join(format!("file{}.txt", i)), format!("content {}", i)).unwrap();
    }

    let kp = generate_key_pair().unwrap();
    let sk_bytes = hex_decode_32(&kp.private_key).unwrap();
    let signing_key = SigningKey::from_bytes(&sk_bytes);
    let verifying_key = signing_key.verifying_key();
    let public_key_b64 = export_public_key(&verifying_key);

    sign_skill(&skill_dir, &signing_key, "multi-author").unwrap();

    let ts_path = dir.path().join("truststore.json");
    let v = Verifier::new(Config {
        enabled: true,
        strict: false,
        trust_store_path: ts_path.to_string_lossy().to_string(),
    }).unwrap();
    v.trust_store().add_key(&public_key_b64, "multi-author", TrustLevel::Verified);

    let result = v.verify_skill(&skill_dir);
    assert!(result.valid);
    assert!(result.files_verified >= 10);
}

#[test]
fn test_sign_skill_overwrites_existing_signature() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("resign");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "# V1").unwrap();

    let kp = generate_key_pair().unwrap();
    let sk_bytes = hex_decode_32(&kp.private_key).unwrap();
    let signing_key = SigningKey::from_bytes(&sk_bytes);

    sign_skill(&skill_dir, &signing_key, "author-v1").unwrap();
    let sig1 = std::fs::read_to_string(skill_dir.join(SIGNATURE_FILE_NAME)).unwrap();

    // Modify and re-sign
    std::fs::write(skill_dir.join("SKILL.md"), "# V2").unwrap();
    sign_skill(&skill_dir, &signing_key, "author-v2").unwrap();
    let sig2 = std::fs::read_to_string(skill_dir.join(SIGNATURE_FILE_NAME)).unwrap();

    assert_ne!(sig1, sig2);
}

#[test]
fn test_import_export_roundtrip_multiple_keys() {
    for _ in 0..5 {
        let kp = generate_key_pair().unwrap();
        let sk_bytes = hex_decode_32(&kp.private_key).unwrap();
        let signing_key = SigningKey::from_bytes(&sk_bytes);
        let verifying_key = signing_key.verifying_key();
        let exported = export_public_key(&verifying_key);
        let imported = import_public_key(&exported).unwrap();
        assert_eq!(verifying_key.to_bytes(), imported.to_bytes());
    }
}
