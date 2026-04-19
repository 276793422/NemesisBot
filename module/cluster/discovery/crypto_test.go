// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package discovery

import (
	"bytes"
	"testing"
)

func TestDeriveKey(t *testing.T) {
	key := DeriveKey("test-token")
	if len(key) != 32 {
		t.Errorf("Expected 32-byte key, got %d bytes", len(key))
	}
}

func TestDeriveKey_Consistent(t *testing.T) {
	key1 := DeriveKey("test-token")
	key2 := DeriveKey("test-token")
	if !bytes.Equal(key1, key2) {
		t.Error("Same token should produce same key")
	}
}

func TestDeriveKey_DifferentTokens(t *testing.T) {
	key1 := DeriveKey("token-a")
	key2 := DeriveKey("token-b")
	if bytes.Equal(key1, key2) {
		t.Error("Different tokens should produce different keys")
	}
}

func TestEncryptDecrypt_Roundtrip(t *testing.T) {
	key := DeriveKey("test-token")
	plaintext := []byte(`{"type":"announce","node_id":"test-node"}`)

	encrypted, err := encryptData(key, plaintext)
	if err != nil {
		t.Fatalf("encryptData failed: %v", err)
	}

	decrypted, err := decryptData(key, encrypted)
	if err != nil {
		t.Fatalf("decryptData failed: %v", err)
	}

	if !bytes.Equal(decrypted, plaintext) {
		t.Errorf("Decrypted data doesn't match: got %s, want %s", decrypted, plaintext)
	}
}

func TestEncryptDecrypt_EmptyPlaintext(t *testing.T) {
	key := DeriveKey("test-token")
	plaintext := []byte{}

	encrypted, err := encryptData(key, plaintext)
	if err != nil {
		t.Fatalf("encryptData failed: %v", err)
	}

	decrypted, err := decryptData(key, encrypted)
	if err != nil {
		t.Fatalf("decryptData failed: %v", err)
	}

	if !bytes.Equal(decrypted, plaintext) {
		t.Error("Decrypted empty data doesn't match")
	}
}

func TestEncryptDecrypt_LargeData(t *testing.T) {
	key := DeriveKey("test-token")
	plaintext := make([]byte, 2048)
	for i := range plaintext {
		plaintext[i] = byte(i % 256)
	}

	encrypted, err := encryptData(key, plaintext)
	if err != nil {
		t.Fatalf("encryptData failed: %v", err)
	}

	decrypted, err := decryptData(key, encrypted)
	if err != nil {
		t.Fatalf("decryptData failed: %v", err)
	}

	if !bytes.Equal(decrypted, plaintext) {
		t.Error("Decrypted large data doesn't match")
	}
}

func TestDecrypt_WrongKey(t *testing.T) {
	key1 := DeriveKey("token-a")
	key2 := DeriveKey("token-b")

	plaintext := []byte("hello world")
	encrypted, err := encryptData(key1, plaintext)
	if err != nil {
		t.Fatalf("encryptData failed: %v", err)
	}

	_, err = decryptData(key2, encrypted)
	if err == nil {
		t.Error("Expected error when decrypting with wrong key")
	}
}

func TestDecrypt_TamperedData(t *testing.T) {
	key := DeriveKey("test-token")
	plaintext := []byte("hello world")
	encrypted, err := encryptData(key, plaintext)
	if err != nil {
		t.Fatalf("encryptData failed: %v", err)
	}

	// Tamper with ciphertext (flip a byte in the ciphertext portion)
	if len(encrypted) > 15 {
		encrypted[15] ^= 0xFF
	}

	_, err = decryptData(key, encrypted)
	if err == nil {
		t.Error("Expected error when decrypting tampered data")
	}
}

func TestDecrypt_TruncatedData(t *testing.T) {
	key := DeriveKey("test-token")

	// Too short to contain even a nonce
	_, err := decryptData(key, []byte{0x01, 0x02, 0x03})
	if err == nil {
		t.Error("Expected error when decrypting truncated data")
	}
}

func TestEncrypt_DifferentNonce(t *testing.T) {
	key := DeriveKey("test-token")
	plaintext := []byte("same data")

	encrypted1, err := encryptData(key, plaintext)
	if err != nil {
		t.Fatalf("First encryptData failed: %v", err)
	}

	encrypted2, err := encryptData(key, plaintext)
	if err != nil {
		t.Fatalf("Second encryptData failed: %v", err)
	}

	if bytes.Equal(encrypted1, encrypted2) {
		t.Error("Two encryptions of the same plaintext should produce different ciphertext (random nonce)")
	}

	// Both should still decrypt to the same plaintext
	dec1, err := decryptData(key, encrypted1)
	if err != nil {
		t.Fatalf("decryptData 1 failed: %v", err)
	}
	dec2, err := decryptData(key, encrypted2)
	if err != nil {
		t.Fatalf("decryptData 2 failed: %v", err)
	}

	if !bytes.Equal(dec1, plaintext) || !bytes.Equal(dec2, plaintext) {
		t.Error("Both should decrypt to original plaintext")
	}
}
