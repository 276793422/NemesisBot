// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package signature

import (
	"context"
	"crypto/ed25519"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sync"
	"time"
)

// trustStoreFile is the JSON envelope stored on disk.
type trustStoreFile struct {
	Version int          `json:"version"`
	Keys    []TrustedKey `json:"keys"`
}

// TrustStore manages trusted Ed25519 public keys with persistence.
type TrustStore struct {
	mu       sync.RWMutex
	keys     map[string]TrustedKey // keyed by base64-encoded public key
	filePath string                // path to the JSON persistence file
}

// NewTrustStore creates or loads a TrustStore from the given file path.
// If the file does not exist, an empty trust store is returned.
// If filePath is empty, the trust store operates in-memory only.
func NewTrustStore(filePath string) (*TrustStore, error) {
	ts := &TrustStore{
		keys:     make(map[string]TrustedKey),
		filePath: filePath,
	}

	if filePath == "" {
		return ts, nil
	}

	data, err := os.ReadFile(filePath)
	if err != nil {
		if os.IsNotExist(err) {
			return ts, nil
		}
		return nil, fmt.Errorf("cannot read trust store: %w", err)
	}

	if len(data) == 0 {
		return ts, nil
	}

	var f trustStoreFile
	if err := json.Unmarshal(data, &f); err != nil {
		return nil, fmt.Errorf("invalid trust store format: %w", err)
	}

	for _, k := range f.Keys {
		// Validate the public key format.
		if _, err := ImportPublicKey(k.PublicKey); err != nil {
			continue // skip malformed entries
		}
		ts.keys[k.PublicKey] = k
	}

	return ts, nil
}

// AddTrustedKey adds a public key to the trust store with the given name and
// trust level, then persists the store to disk.
func (ts *TrustStore) AddTrustedKey(ctx context.Context, pubKey ed25519.PublicKey, name string, level TrustLevel) error {
	ts.mu.Lock()
	defer ts.mu.Unlock()

	b64 := ExportPublicKey(pubKey)
	fp := computeFingerprint(pubKey)

	ts.keys[b64] = TrustedKey{
		PublicKey:   b64,
		Name:        name,
		Level:       level,
		AddedAt:     time.Now().UTC(),
		Fingerprint: fp,
	}

	return ts.persistLocked()
}

// RemoveKey removes a key by signer name and persists the change.
// Returns nil even if the key was not found.
func (ts *TrustStore) RemoveKey(name string) error {
	ts.mu.Lock()
	defer ts.mu.Unlock()

	for b64, k := range ts.keys {
		if k.Name == name {
			delete(ts.keys, b64)
			return ts.persistLocked()
		}
	}
	return nil
}

// RemoveKeyByPublicKey removes a key by its public key bytes and persists the change.
func (ts *TrustStore) RemoveKeyByPublicKey(pubKey ed25519.PublicKey) error {
	ts.mu.Lock()
	defer ts.mu.Unlock()

	b64 := ExportPublicKey(pubKey)
	if _, exists := ts.keys[b64]; exists {
		delete(ts.keys, b64)
		return ts.persistLocked()
	}
	return nil
}

// IsTrusted checks whether the given public key is in the trust store.
// It returns the trust level and true if the key is found (and not revoked).
// If the key is revoked, it returns (TrustRevoked, false).
func (ts *TrustStore) IsTrusted(pubKey ed25519.PublicKey) (TrustLevel, bool) {
	ts.mu.RLock()
	defer ts.mu.RUnlock()

	b64 := ExportPublicKey(pubKey)
	k, exists := ts.keys[b64]
	if !exists {
		return TrustUnknown, false
	}
	if k.Level == TrustRevoked {
		return TrustRevoked, false
	}
	return k.Level, true
}

// ListKeys returns all keys currently in the trust store.
// The returned slice is a copy; callers can mutate it freely.
func (ts *TrustStore) ListKeys() []TrustedKey {
	ts.mu.RLock()
	defer ts.mu.RUnlock()

	result := make([]TrustedKey, 0, len(ts.keys))
	for _, k := range ts.keys {
		result = append(result, k)
	}
	return result
}

// RevokeKey marks a key as revoked by its signer name.
func (ts *TrustStore) RevokeKey(name string) error {
	ts.mu.Lock()
	defer ts.mu.Unlock()

	for b64, k := range ts.keys {
		if k.Name == name {
			k.Level = TrustRevoked
			ts.keys[b64] = k
			return ts.persistLocked()
		}
	}
	return fmt.Errorf("key not found: %s", name)
}

// RevokeKeyByPublicKey marks a key as revoked by its public key bytes.
func (ts *TrustStore) RevokeKeyByPublicKey(pubKey ed25519.PublicKey) error {
	ts.mu.Lock()
	defer ts.mu.Unlock()

	b64 := ExportPublicKey(pubKey)
	k, exists := ts.keys[b64]
	if !exists {
		return fmt.Errorf("key not found")
	}
	k.Level = TrustRevoked
	ts.keys[b64] = k
	return ts.persistLocked()
}

// GetKey retrieves a key by its public key bytes.
// Returns the TrustedKey and true if found, or a zero TrustedKey and false.
func (ts *TrustStore) GetKey(pubKey ed25519.PublicKey) (TrustedKey, bool) {
	ts.mu.RLock()
	defer ts.mu.RUnlock()

	b64 := ExportPublicKey(pubKey)
	k, exists := ts.keys[b64]
	return k, exists
}

// GetKeyByName retrieves a key by its signer name.
// Returns the TrustedKey and true if found, or a zero TrustedKey and false.
func (ts *TrustStore) GetKeyByName(name string) (TrustedKey, bool) {
	ts.mu.RLock()
	defer ts.mu.RUnlock()

	for _, k := range ts.keys {
		if k.Name == name {
			return k, true
		}
	}
	return TrustedKey{}, false
}

// KeyCount returns the number of keys in the trust store.
func (ts *TrustStore) KeyCount() int {
	ts.mu.RLock()
	defer ts.mu.RUnlock()
	return len(ts.keys)
}

// FilePath returns the persistence file path, or empty string if in-memory.
func (ts *TrustStore) FilePath() string {
	return ts.filePath
}

// persistLocked writes the trust store to disk. Caller must hold ts.mu.
func (ts *TrustStore) persistLocked() error {
	if ts.filePath == "" {
		return nil // in-memory only
	}

	f := trustStoreFile{
		Version: 1,
		Keys:    make([]TrustedKey, 0, len(ts.keys)),
	}
	for _, k := range ts.keys {
		f.Keys = append(f.Keys, k)
	}

	data, err := json.MarshalIndent(f, "", "  ")
	if err != nil {
		return fmt.Errorf("failed to marshal trust store: %w", err)
	}

	// Ensure the parent directory exists.
	dir := filepath.Dir(ts.filePath)
	if err := os.MkdirAll(dir, 0755); err != nil {
		return fmt.Errorf("failed to create trust store directory: %w", err)
	}

	// Write atomically via a temp file.
	tmp := ts.filePath + ".tmp"
	if err := os.WriteFile(tmp, data, 0600); err != nil {
		return fmt.Errorf("failed to write trust store: %w", err)
	}

	if err := os.Rename(tmp, ts.filePath); err != nil {
		os.Remove(tmp) // clean up on failure
		return fmt.Errorf("failed to rename trust store: %w", err)
	}

	return nil
}
