// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package discovery

import (
	"crypto/aes"
	"crypto/cipher"
	"crypto/rand"
	"crypto/sha256"
	"fmt"
)

// DeriveKey derives a 32-byte AES-256 key from a token string using SHA-256.
// Exported so cluster.go can pre-derive the key before creating Discovery.
func DeriveKey(token string) []byte {
	h := sha256.Sum256([]byte(token))
	return h[:]
}

// encryptData encrypts plaintext using AES-256-GCM.
// Output format: [12-byte nonce] + [ciphertext + 16-byte GCM tag].
func encryptData(key []byte, plaintext []byte) ([]byte, error) {
	block, err := aes.NewCipher(key)
	if err != nil {
		return nil, err
	}
	gcm, err := cipher.NewGCM(block)
	if err != nil {
		return nil, err
	}
	nonce := make([]byte, gcm.NonceSize())
	if _, err := rand.Read(nonce); err != nil {
		return nil, err
	}
	return append(nonce, gcm.Seal(nil, nonce, plaintext, nil)...), nil
}

// decryptData decrypts AES-256-GCM encrypted data.
// Expected input format: [12-byte nonce] + [ciphertext + 16-byte GCM tag].
func decryptData(key []byte, data []byte) ([]byte, error) {
	block, err := aes.NewCipher(key)
	if err != nil {
		return nil, err
	}
	gcm, err := cipher.NewGCM(block)
	if err != nil {
		return nil, err
	}
	nonceSize := gcm.NonceSize()
	if len(data) < nonceSize {
		return nil, fmt.Errorf("ciphertext too short")
	}
	nonce, ciphertext := data[:nonceSize], data[nonceSize:]
	return gcm.Open(nil, nonce, ciphertext, nil)
}
