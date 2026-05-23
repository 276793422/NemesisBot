use super::*;

#[test]
fn test_sign_and_verify() {
    let crypto = CryptoService::new("test-secret".into());
    let payload = b"hello world";
    let sig = crypto.sign(payload);

    assert!(crypto.verify(payload, &sig));
    assert!(!crypto.verify(b"tampered", &sig));
}

#[test]
fn test_no_auth_mode() {
    let crypto = CryptoService::no_auth();
    assert!(crypto.verify(b"anything", "any-signature"));
    assert!(crypto.sign(b"test").is_empty());
}

#[test]
fn test_node_id_hash_deterministic() {
    let hash1 = CryptoService::node_id_hash("node-1", "10.0.0.1:9000");
    let hash2 = CryptoService::node_id_hash("node-1", "10.0.0.1:9000");
    assert_eq!(hash1, hash2);
    assert_eq!(hash1.len(), 16);
}

#[test]
fn test_node_id_hash_different_inputs() {
    let hash1 = CryptoService::node_id_hash("node-1", "10.0.0.1:9000");
    let hash2 = CryptoService::node_id_hash("node-2", "10.0.0.1:9000");
    assert_ne!(hash1, hash2);
}

// --- AES-256-GCM tests ---

#[test]
fn test_derive_key_deterministic() {
    let k1 = derive_key("my-token");
    let k2 = derive_key("my-token");
    assert_eq!(k1, k2);
}

#[test]
fn test_derive_key_different_tokens() {
    let k1 = derive_key("token-a");
    let k2 = derive_key("token-b");
    assert_ne!(k1, k2);
}

#[test]
fn test_encrypt_decrypt_roundtrip() {
    let key = derive_key("test-secret");
    let plaintext = b"hello discovery world";
    let encrypted = encrypt_data(&key, plaintext).unwrap();
    let decrypted = decrypt_data(&key, &encrypted).unwrap();
    assert_eq!(decrypted, plaintext);
}

#[test]
fn test_encrypt_produces_different_ciphertexts() {
    let key = derive_key("test-secret");
    let plaintext = b"same data";
    let e1 = encrypt_data(&key, plaintext).unwrap();
    let e2 = encrypt_data(&key, plaintext).unwrap();
    // Random nonces → different ciphertext
    assert_ne!(e1, e2);
}

#[test]
fn test_decrypt_with_wrong_key_fails() {
    let key1 = derive_key("correct-key");
    let key2 = derive_key("wrong-key");
    let encrypted = encrypt_data(&key1, b"secret").unwrap();
    assert!(decrypt_data(&key2, &encrypted).is_err());
}

#[test]
fn test_decrypt_too_short_fails() {
    let key = derive_key("any");
    assert!(decrypt_data(&key, b"short").is_err());
}

#[test]
fn test_decrypt_tampered_fails() {
    let key = derive_key("test");
    let mut encrypted = encrypt_data(&key, b"data").unwrap();
    // Flip a byte in the ciphertext
    let last = encrypted.len() - 1;
    encrypted[last] ^= 0xff;
    assert!(decrypt_data(&key, &encrypted).is_err());
}

#[test]
fn test_encrypt_empty_plaintext() {
    let key = derive_key("test");
    let encrypted = encrypt_data(&key, b"").unwrap();
    let decrypted = decrypt_data(&key, &encrypted).unwrap();
    assert!(decrypted.is_empty());
}

#[test]
fn test_encrypt_large_payload() {
    let key = derive_key("test");
    let plaintext = vec![0xAB_u8; 4096];
    let encrypted = encrypt_data(&key, &plaintext).unwrap();
    let decrypted = decrypt_data(&key, &encrypted).unwrap();
    assert_eq!(decrypted, plaintext);
}
