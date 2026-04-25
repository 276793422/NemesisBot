package signature_test

import (
	"context"
	"os"
	"path/filepath"
	"testing"

	"github.com/276793422/NemesisBot/module/security/signature"
)

// ---------------------------------------------------------------------------
// NewTrustStore
// ---------------------------------------------------------------------------

func TestNewTrustStore_EmptyPath(t *testing.T) {
	ts, err := signature.NewTrustStore("")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if ts == nil {
		t.Fatal("expected non-nil trust store")
	}
	if ts.FilePath() != "" {
		t.Errorf("expected empty FilePath, got %q", ts.FilePath())
	}
}

func TestNewTrustStore_NonExistentPath(t *testing.T) {
	dir := t.TempDir()
	tsPath := filepath.Join(dir, "sub", "trust.json")

	ts, err := signature.NewTrustStore(tsPath)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if ts.KeyCount() != 0 {
		t.Errorf("expected 0 keys, got %d", ts.KeyCount())
	}
}

func TestNewTrustStore_ExistingEmptyFile(t *testing.T) {
	dir := t.TempDir()
	tsPath := filepath.Join(dir, "trust.json")
	os.WriteFile(tsPath, []byte{}, 0600)

	ts, err := signature.NewTrustStore(tsPath)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if ts.KeyCount() != 0 {
		t.Errorf("expected 0 keys, got %d", ts.KeyCount())
	}
}

func TestNewTrustStore_InvalidJSON(t *testing.T) {
	dir := t.TempDir()
	tsPath := filepath.Join(dir, "trust.json")
	os.WriteFile(tsPath, []byte("not valid json"), 0600)

	_, err := signature.NewTrustStore(tsPath)
	if err == nil {
		t.Error("expected error for invalid JSON")
	}
}

// ---------------------------------------------------------------------------
// AddTrustedKey / IsTrusted
// ---------------------------------------------------------------------------

func TestTrustStore_AddTrustedKey(t *testing.T) {
	dir := t.TempDir()
	tsPath := filepath.Join(dir, "trust.json")
	ts, _ := signature.NewTrustStore(tsPath)

	pub, _, _ := signature.GenerateKeyPair()
	err := ts.AddTrustedKey(context.Background(), pub, "test-key", signature.TrustCommunity)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if ts.KeyCount() != 1 {
		t.Errorf("expected 1 key, got %d", ts.KeyCount())
	}

	level, trusted := ts.IsTrusted(pub)
	if !trusted {
		t.Error("expected key to be trusted")
	}
	if level != signature.TrustCommunity {
		t.Errorf("expected TrustCommunity, got %v", level)
	}
}

func TestTrustStore_AddTrustedKey_InMemory(t *testing.T) {
	ts, _ := signature.NewTrustStore("")

	pub, _, _ := signature.GenerateKeyPair()
	err := ts.AddTrustedKey(context.Background(), pub, "inmem", signature.TrustVerified)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if ts.KeyCount() != 1 {
		t.Errorf("expected 1 key, got %d", ts.KeyCount())
	}
}

func TestTrustStore_IsTrusted_UnknownKey(t *testing.T) {
	ts, _ := signature.NewTrustStore("")
	pub, _, _ := signature.GenerateKeyPair()

	level, trusted := ts.IsTrusted(pub)
	if trusted {
		t.Error("expected key to be untrusted")
	}
	if level != signature.TrustUnknown {
		t.Errorf("expected TrustUnknown, got %v", level)
	}
}

// ---------------------------------------------------------------------------
// RemoveKey
// ---------------------------------------------------------------------------

func TestTrustStore_RemoveKey(t *testing.T) {
	dir := t.TempDir()
	tsPath := filepath.Join(dir, "trust.json")
	ts, _ := signature.NewTrustStore(tsPath)

	pub, _, _ := signature.GenerateKeyPair()
	ts.AddTrustedKey(context.Background(), pub, "to-remove", signature.TrustCommunity)

	err := ts.RemoveKey("to-remove")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if ts.KeyCount() != 0 {
		t.Errorf("expected 0 keys after remove, got %d", ts.KeyCount())
	}
	_, trusted := ts.IsTrusted(pub)
	if trusted {
		t.Error("expected key to be untrusted after removal")
	}
}

func TestTrustStore_RemoveKey_NotFound(t *testing.T) {
	ts, _ := signature.NewTrustStore("")
	err := ts.RemoveKey("nonexistent")
	if err != nil {
		t.Errorf("expected nil for non-existent key, got: %v", err)
	}
}

func TestTrustStore_RemoveKeyByPublicKey(t *testing.T) {
	dir := t.TempDir()
	tsPath := filepath.Join(dir, "trust.json")
	ts, _ := signature.NewTrustStore(tsPath)

	pub, _, _ := signature.GenerateKeyPair()
	ts.AddTrustedKey(context.Background(), pub, "key1", signature.TrustVerified)

	err := ts.RemoveKeyByPublicKey(pub)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if ts.KeyCount() != 0 {
		t.Errorf("expected 0 keys, got %d", ts.KeyCount())
	}
}

func TestTrustStore_RemoveKeyByPublicKey_NotFound(t *testing.T) {
	ts, _ := signature.NewTrustStore("")
	pub, _, _ := signature.GenerateKeyPair()
	err := ts.RemoveKeyByPublicKey(pub)
	if err != nil {
		t.Errorf("expected nil for non-existent key, got: %v", err)
	}
}

// ---------------------------------------------------------------------------
// RevokeKey
// ---------------------------------------------------------------------------

func TestTrustStore_RevokeKey(t *testing.T) {
	dir := t.TempDir()
	tsPath := filepath.Join(dir, "trust.json")
	ts, _ := signature.NewTrustStore(tsPath)

	pub, _, _ := signature.GenerateKeyPair()
	ts.AddTrustedKey(context.Background(), pub, "revoke-me", signature.TrustVerified)

	err := ts.RevokeKey("revoke-me")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	level, trusted := ts.IsTrusted(pub)
	if trusted {
		t.Error("expected key to not be trusted after revocation")
	}
	if level != signature.TrustRevoked {
		t.Errorf("expected TrustRevoked, got %v", level)
	}
}

func TestTrustStore_RevokeKey_NotFound(t *testing.T) {
	ts, _ := signature.NewTrustStore("")
	err := ts.RevokeKey("nonexistent")
	if err == nil {
		t.Error("expected error for non-existent key")
	}
}

func TestTrustStore_RevokeKeyByPublicKey(t *testing.T) {
	dir := t.TempDir()
	tsPath := filepath.Join(dir, "trust.json")
	ts, _ := signature.NewTrustStore(tsPath)

	pub, _, _ := signature.GenerateKeyPair()
	ts.AddTrustedKey(context.Background(), pub, "key", signature.TrustCommunity)

	err := ts.RevokeKeyByPublicKey(pub)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	level, trusted := ts.IsTrusted(pub)
	if trusted {
		t.Error("expected not trusted after revocation")
	}
	if level != signature.TrustRevoked {
		t.Errorf("expected TrustRevoked, got %v", level)
	}
}

func TestTrustStore_RevokeKeyByPublicKey_NotFound(t *testing.T) {
	ts, _ := signature.NewTrustStore("")
	pub, _, _ := signature.GenerateKeyPair()
	err := ts.RevokeKeyByPublicKey(pub)
	if err == nil {
		t.Error("expected error for non-existent key")
	}
}

// ---------------------------------------------------------------------------
// ListKeys
// ---------------------------------------------------------------------------

func TestTrustStore_ListKeys(t *testing.T) {
	ts, _ := signature.NewTrustStore("")

	pub1, _, _ := signature.GenerateKeyPair()
	pub2, _, _ := signature.GenerateKeyPair()
	ts.AddTrustedKey(context.Background(), pub1, "key1", signature.TrustCommunity)
	ts.AddTrustedKey(context.Background(), pub2, "key2", signature.TrustVerified)

	keys := ts.ListKeys()
	if len(keys) != 2 {
		t.Fatalf("expected 2 keys, got %d", len(keys))
	}

	names := map[string]bool{}
	for _, k := range keys {
		names[k.Name] = true
	}
	if !names["key1"] || !names["key2"] {
		t.Errorf("expected keys key1 and key2, got %v", names)
	}
}

func TestTrustStore_ListKeys_Empty(t *testing.T) {
	ts, _ := signature.NewTrustStore("")
	keys := ts.ListKeys()
	if len(keys) != 0 {
		t.Errorf("expected 0 keys, got %d", len(keys))
	}
}

func TestTrustStore_ListKeys_ReturnsCopy(t *testing.T) {
	ts, _ := signature.NewTrustStore("")
	pub, _, _ := signature.GenerateKeyPair()
	ts.AddTrustedKey(context.Background(), pub, "key1", signature.TrustCommunity)

	keys := ts.ListKeys()
	keys[0] = signature.TrustedKey{} // tamper with copy
	original := ts.ListKeys()
	if original[0].Name != "key1" {
		t.Error("ListKeys should return a copy")
	}
}

// ---------------------------------------------------------------------------
// GetKey / GetKeyByName
// ---------------------------------------------------------------------------

func TestTrustStore_GetKey(t *testing.T) {
	ts, _ := signature.NewTrustStore("")
	pub, _, _ := signature.GenerateKeyPair()
	ts.AddTrustedKey(context.Background(), pub, "findme", signature.TrustVerified)

	key, found := ts.GetKey(pub)
	if !found {
		t.Fatal("expected key to be found")
	}
	if key.Name != "findme" {
		t.Errorf("expected Name=findme, got %q", key.Name)
	}
	if key.Level != signature.TrustVerified {
		t.Errorf("expected TrustVerified, got %v", key.Level)
	}
}

func TestTrustStore_GetKey_NotFound(t *testing.T) {
	ts, _ := signature.NewTrustStore("")
	pub, _, _ := signature.GenerateKeyPair()
	_, found := ts.GetKey(pub)
	if found {
		t.Error("expected key to not be found")
	}
}

func TestTrustStore_GetKeyByName(t *testing.T) {
	ts, _ := signature.NewTrustStore("")
	pub, _, _ := signature.GenerateKeyPair()
	ts.AddTrustedKey(context.Background(), pub, "byname", signature.TrustCommunity)

	key, found := ts.GetKeyByName("byname")
	if !found {
		t.Fatal("expected key to be found by name")
	}
	if key.Name != "byname" {
		t.Errorf("expected Name=byname, got %q", key.Name)
	}
}

func TestTrustStore_GetKeyByName_NotFound(t *testing.T) {
	ts, _ := signature.NewTrustStore("")
	_, found := ts.GetKeyByName("nonexistent")
	if found {
		t.Error("expected key to not be found")
	}
}

// ---------------------------------------------------------------------------
// KeyCount
// ---------------------------------------------------------------------------

func TestTrustStore_KeyCount(t *testing.T) {
	ts, _ := signature.NewTrustStore("")
	if ts.KeyCount() != 0 {
		t.Errorf("expected 0 keys, got %d", ts.KeyCount())
	}

	pub1, _, _ := signature.GenerateKeyPair()
	ts.AddTrustedKey(context.Background(), pub1, "key1", signature.TrustCommunity)
	if ts.KeyCount() != 1 {
		t.Errorf("expected 1 key, got %d", ts.KeyCount())
	}

	pub2, _, _ := signature.GenerateKeyPair()
	ts.AddTrustedKey(context.Background(), pub2, "key2", signature.TrustVerified)
	if ts.KeyCount() != 2 {
		t.Errorf("expected 2 keys, got %d", ts.KeyCount())
	}
}

// ---------------------------------------------------------------------------
// FilePath
// ---------------------------------------------------------------------------

func TestTrustStore_FilePath(t *testing.T) {
	dir := t.TempDir()
	tsPath := filepath.Join(dir, "trust.json")
	ts, _ := signature.NewTrustStore(tsPath)
	if ts.FilePath() != tsPath {
		t.Errorf("expected FilePath=%q, got %q", tsPath, ts.FilePath())
	}
}

// ---------------------------------------------------------------------------
// Persistence — save and reload
// ---------------------------------------------------------------------------

func TestTrustStore_Persistence(t *testing.T) {
	dir := t.TempDir()
	tsPath := filepath.Join(dir, "trust.json")

	pub, _, _ := signature.GenerateKeyPair()
	pubB64 := signature.ExportPublicKey(pub)

	// Create and add key.
	ts1, _ := signature.NewTrustStore(tsPath)
	ts1.AddTrustedKey(context.Background(), pub, "persistent-key", signature.TrustVerified)
	if ts1.KeyCount() != 1 {
		t.Fatalf("expected 1 key, got %d", ts1.KeyCount())
	}

	// Verify file was written.
	if _, err := os.Stat(tsPath); os.IsNotExist(err) {
		t.Fatal("expected trust store file to exist")
	}

	// Reload from disk.
	ts2, err := signature.NewTrustStore(tsPath)
	if err != nil {
		t.Fatalf("unexpected error reloading: %v", err)
	}
	if ts2.KeyCount() != 1 {
		t.Fatalf("expected 1 key after reload, got %d", ts2.KeyCount())
	}

	level, trusted := ts2.IsTrusted(pub)
	if !trusted {
		t.Error("expected key to be trusted after reload")
	}
	if level != signature.TrustVerified {
		t.Errorf("expected TrustVerified, got %v", level)
	}

	// Verify the public key was preserved.
	keys := ts2.ListKeys()
	if len(keys) != 1 || keys[0].PublicKey != pubB64 {
		t.Errorf("public key mismatch: expected %q", pubB64)
	}
}

func TestTrustStore_Persistence_MultipleKeys(t *testing.T) {
	dir := t.TempDir()
	tsPath := filepath.Join(dir, "trust.json")

	ts, _ := signature.NewTrustStore(tsPath)
	for i := 0; i < 5; i++ {
		pub, _, _ := signature.GenerateKeyPair()
		name := string(rune('a' + i))
		ts.AddTrustedKey(context.Background(), pub, name, signature.TrustCommunity)
	}

	ts2, _ := signature.NewTrustStore(tsPath)
	if ts2.KeyCount() != 5 {
		t.Errorf("expected 5 keys after reload, got %d", ts2.KeyCount())
	}
}

func TestTrustStore_Persistence_SkipMalformedKeys(t *testing.T) {
	dir := t.TempDir()
	tsPath := filepath.Join(dir, "trust.json")

	// Write a trust store with a malformed key entry.
	content := `{"version":1,"keys":[{"public_key":"invalid-base64!!!","name":"bad","level":"community","added_at":"2026-01-01T00:00:00Z","fingerprint":"abc"}]}`
	os.WriteFile(tsPath, []byte(content), 0600)

	ts, err := signature.NewTrustStore(tsPath)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if ts.KeyCount() != 0 {
		t.Errorf("expected 0 keys (malformed skipped), got %d", ts.KeyCount())
	}
}

// ---------------------------------------------------------------------------
// Concurrent access
// ---------------------------------------------------------------------------

func TestTrustStore_ConcurrentAccess(t *testing.T) {
	ts, _ := signature.NewTrustStore("")
	done := make(chan bool, 20)

	for i := 0; i < 10; i++ {
		go func(i int) {
			pub, _, _ := signature.GenerateKeyPair()
			ts.AddTrustedKey(context.Background(), pub, string(rune('a'+i)), signature.TrustCommunity)
			done <- true
		}(i)
	}
	for i := 0; i < 10; i++ {
		go func() {
			ts.ListKeys()
			ts.KeyCount()
			done <- true
		}()
	}

	for i := 0; i < 20; i++ {
		<-done
	}

	if ts.KeyCount() != 10 {
		t.Errorf("expected 10 keys, got %d", ts.KeyCount())
	}
}

// ---------------------------------------------------------------------------
// TrustedKey fields
// ---------------------------------------------------------------------------

func TestTrustedKey_Fields(t *testing.T) {
	ts, _ := signature.NewTrustStore("")
	pub, _, _ := signature.GenerateKeyPair()
	ts.AddTrustedKey(context.Background(), pub, "field-test", signature.TrustVerified)

	key, found := ts.GetKey(pub)
	if !found {
		t.Fatal("expected key to be found")
	}
	if key.PublicKey == "" {
		t.Error("expected non-empty PublicKey")
	}
	if key.Name != "field-test" {
		t.Errorf("expected Name=field-test, got %q", key.Name)
	}
	if key.Level != signature.TrustVerified {
		t.Errorf("expected TrustVerified, got %v", key.Level)
	}
	if key.AddedAt.IsZero() {
		t.Error("expected non-zero AddedAt")
	}
	if key.Fingerprint == "" {
		t.Error("expected non-empty Fingerprint")
	}
}
