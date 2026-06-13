use super::*;

#[test]
fn test_validate_frame_size_ok() {
    let data = vec![0u8; 1024];
    assert!(validate_frame_size(&data).is_ok());
}

#[test]
fn test_validate_frame_size_too_large() {
    let data = vec![0u8; MAX_FRAME_SIZE + 1];
    assert!(validate_frame_size(&data).is_err());
}

#[test]
fn test_encode_decode_batch() {
    let frames = vec![
        Frame::new(b"frame-1".to_vec()),
        Frame::new(b"frame-2".to_vec()),
        Frame::new(b"frame-3".to_vec()),
    ];

    let encoded = encode_batch(&frames);
    let (decoded, consumed) = decode_all(&encoded);

    assert_eq!(decoded.len(), 3);
    assert_eq!(consumed, encoded.len());
    assert_eq!(decoded[0].data, b"frame-1");
    assert_eq!(decoded[1].data, b"frame-2");
    assert_eq!(decoded[2].data, b"frame-3");
}

#[test]
fn test_decode_partial() {
    let frame = Frame::new(b"partial".to_vec());
    let encoded = frame.encode();

    // Only first half
    let (decoded, _) = decode_all(&encoded[..encoded.len() / 2]);
    assert!(decoded.is_empty());
}

#[test]
fn test_sync_write_read_frame() {
    use std::io::Cursor;

    let data = b"hello world";
    let mut buf = Cursor::new(Vec::new());
    write_frame(&mut buf, data).unwrap();

    buf.set_position(0);
    let read = read_frame(&mut buf).unwrap();
    assert_eq!(read, data);
}

#[tokio::test]
async fn test_async_frame_reader() {
    // Build a framed payload
    let payload = b"async frame data";
    let mut encoded = Vec::new();
    let len = payload.len() as u32;
    encoded.extend_from_slice(&len.to_be_bytes());
    encoded.extend_from_slice(payload);

    let cursor = std::io::Cursor::new(encoded);
    let mut reader = AsyncFrameReader::new(cursor);
    let data = reader.read_frame().await.unwrap();
    assert_eq!(data, payload);
}

#[tokio::test]
async fn test_async_write_frame() {
    let payload = b"async write test";
    let mut buf = Vec::new();
    write_frame_async(&mut buf, payload).await.unwrap();

    // Verify we can read it back
    let cursor = std::io::Cursor::new(buf);
    let mut reader = AsyncFrameReader::new(cursor);
    let data = reader.read_frame().await.unwrap();
    assert_eq!(data, payload);
}

#[tokio::test]
async fn test_async_frame_reader_multiple_frames() {
    let mut encoded = Vec::new();

    for i in 0..5u8 {
        let payload = vec![i; 64];
        let len = payload.len() as u32;
        encoded.extend_from_slice(&len.to_be_bytes());
        encoded.extend_from_slice(&payload);
    }

    let cursor = std::io::Cursor::new(encoded);
    let mut reader = AsyncFrameReader::new(cursor);

    for i in 0..5u8 {
        let data = reader.read_frame().await.unwrap();
        assert_eq!(data, vec![i; 64]);
    }
}

// ===========================================================================
// AEAD encrypt/decrypt tests
// ===========================================================================

#[test]
fn test_derive_key_is_deterministic_and_32_bytes() {
    let k1 = derive_key("cluster-secret-123");
    let k2 = derive_key("cluster-secret-123");
    let k3 = derive_key("different-secret");

    assert_eq!(k1.len(), AES_KEY_SIZE);
    assert_eq!(k1, k2, "same token must derive same key");
    assert_ne!(k1, k3, "different tokens must derive different keys");
}

#[test]
fn test_encrypt_decrypt_round_trip() {
    let key = derive_key("round-trip-token");
    let plaintext = br#"{"id":"msg-1","action":"ping","type":"request"}"#;
    let ciphertext = encrypt_frame(plaintext, &key).expect("encrypt");

    // Ciphertext layout: 12-byte nonce + payload + 16-byte tag.
    assert!(ciphertext.len() >= NONCE_SIZE + TAG_SIZE);
    assert_ne!(
        &ciphertext[..],
        &plaintext[..],
        "ciphertext must not equal plaintext"
    );

    let recovered = decrypt_frame(&ciphertext, &key).expect("decrypt");
    assert_eq!(recovered, plaintext);
}

#[test]
fn test_decrypt_with_wrong_key_fails() {
    let legit_key = derive_key("real-token");
    let attacker_key = derive_key("wrong-token");

    let plaintext = b"sensitive payload";
    let ciphertext = encrypt_frame(plaintext, &legit_key).expect("encrypt");

    let result = decrypt_frame(&ciphertext, &attacker_key);
    assert!(
        result.is_err(),
        "decrypt with wrong key must fail (GCM tag mismatch)"
    );
}

#[test]
fn test_decrypt_rejects_short_data() {
    let key = derive_key("any");

    // Shorter than nonce + tag minimum.
    let too_short = vec![0u8; NONCE_SIZE + TAG_SIZE - 1];
    assert!(decrypt_frame(&too_short, &key).is_err());

    // Empty input.
    assert!(decrypt_frame(&[], &key).is_err());
}

#[test]
fn test_decrypt_rejects_tampered_ciphertext() {
    let key = derive_key("tamper-test");
    let plaintext = b"original payload bytes";
    let mut ciphertext = encrypt_frame(plaintext, &key).expect("encrypt");

    // Flip a bit in the ciphertext body (after the nonce).
    let last = ciphertext.len() - 1;
    ciphertext[last] ^= 0x01;

    let result = decrypt_frame(&ciphertext, &key);
    assert!(
        result.is_err(),
        "tampered ciphertext must fail GCM tag verification"
    );
}

#[test]
fn test_nonce_is_unique_per_encryption() {
    // AES-256-GCM uses a random 12-byte nonce per call. Two encryptions of
    // the same plaintext must produce different ciphertexts (different nonces)
    // but both must decrypt back to the original plaintext.
    let key = derive_key("nonce-uniqueness");
    let plaintext = b"same input bytes";

    let c1 = encrypt_frame(plaintext, &key).expect("encrypt 1");
    let c2 = encrypt_frame(plaintext, &key).expect("encrypt 2");

    assert_ne!(c1, c2, "nonce must differ between calls");
    assert_ne!(&c1[..NONCE_SIZE], &c2[..NONCE_SIZE], "nonce prefix differs");

    assert_eq!(decrypt_frame(&c1, &key).unwrap(), plaintext);
    assert_eq!(decrypt_frame(&c2, &key).unwrap(), plaintext);
}

#[test]
fn test_encrypt_empty_payload() {
    let key = derive_key("empty");
    let ciphertext = encrypt_frame(&[], &key).expect("encrypt empty");
    // Empty plaintext still produces nonce + tag (no body).
    assert_eq!(ciphertext.len(), NONCE_SIZE + TAG_SIZE);
    let recovered = decrypt_frame(&ciphertext, &key).expect("decrypt empty");
    assert!(recovered.is_empty());
}

#[test]
fn test_encrypt_large_payload() {
    // 1 MiB payload — exercises the AEAD on a non-trivial size.
    let key = derive_key("large-payload");
    let plaintext = vec![0xABu8; 1024 * 1024];
    let ciphertext = encrypt_frame(&plaintext, &key).expect("encrypt large");
    let recovered = decrypt_frame(&ciphertext, &key).expect("decrypt large");
    assert_eq!(recovered, plaintext);
}
