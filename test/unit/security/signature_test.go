// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package security_test

import (
	"context"
	"crypto/ed25519"
	"encoding/json"
	"os"
	"path/filepath"
	"testing"

	signature "github.com/276793422/NemesisBot/module/security/signature"
)

func TestSignature_GenerateKeyPair(t *testing.T) {
	pub, priv, err := signature.GenerateKeyPair()
	if err != nil {
		t.Fatalf("GenerateKeyPair returned error: %v", err)
	}
	if pub == nil {
		t.Error("expected non-nil public key")
	}
	if priv == nil {
		t.Error("expected non-nil private key")
	}
	if len(pub) != ed25519.PublicKeySize {
		t.Errorf("expected public key size %d, got %d", ed25519.PublicKeySize, len(pub))
	}
}

func TestSignature_SignAndVerifyFile(t *testing.T) {
	pub, priv, err := signature.GenerateKeyPair()
	if err != nil {
		t.Fatalf("GenerateKeyPair returned error: %v", err)
	}

	// Create a temp file
	tmpDir := t.TempDir()
	filePath := filepath.Join(tmpDir, "testfile.txt")
	content := []byte("Hello, this is a test file for signing.")
	if err := os.WriteFile(filePath, content, 0644); err != nil {
		t.Fatalf("failed to write test file: %v", err)
	}

	ctx := context.Background()

	// Sign the file
	sigBytes, err := signature.SignFile(ctx, filePath, priv)
	if err != nil {
		t.Fatalf("SignFile returned error: %v", err)
	}
	if len(sigBytes) != ed25519.SignatureSize {
		t.Errorf("expected signature size %d, got %d", ed25519.SignatureSize, len(sigBytes))
	}

	// Verify with VerifyFileWithKey
	result, err := signature.VerifyFileWithKey(filePath, pub, sigBytes)
	if err != nil {
		t.Fatalf("VerifyFileWithKey returned error: %v", err)
	}
	if !result.Valid {
		t.Errorf("expected valid signature, got error: %s", result.Error)
	}
}

func TestSignature_VerifyTamperedFile(t *testing.T) {
	pub, priv, err := signature.GenerateKeyPair()
	if err != nil {
		t.Fatalf("GenerateKeyPair returned error: %v", err)
	}

	tmpDir := t.TempDir()
	filePath := filepath.Join(tmpDir, "tamper_test.txt")
	if err := os.WriteFile(filePath, []byte("original content"), 0644); err != nil {
		t.Fatalf("failed to write test file: %v", err)
	}

	ctx := context.Background()
	sigBytes, err := signature.SignFile(ctx, filePath, priv)
	if err != nil {
		t.Fatalf("SignFile returned error: %v", err)
	}

	// Tamper with the file
	if err := os.WriteFile(filePath, []byte("tampered content"), 0644); err != nil {
		t.Fatalf("failed to tamper test file: %v", err)
	}

	// Verify should fail
	result, err := signature.VerifyFileWithKey(filePath, pub, sigBytes)
	if err != nil {
		t.Fatalf("VerifyFileWithKey returned error: %v", err)
	}
	if result.Valid {
		t.Error("expected signature verification to fail for tampered file")
	}
}

func TestSignature_SignAndVerifyDirectory(t *testing.T) {
	pub, priv, err := signature.GenerateKeyPair()
	if err != nil {
		t.Fatalf("GenerateKeyPair returned error: %v", err)
	}

	// Create a skill directory with multiple files
	tmpDir := t.TempDir()
	skillDir := filepath.Join(tmpDir, "test_skill")
	if err := os.MkdirAll(skillDir, 0755); err != nil {
		t.Fatalf("failed to create skill directory: %v", err)
	}

	// Create files in the skill directory
	if err := os.WriteFile(filepath.Join(skillDir, "SKILL.md"), []byte("# Test Skill\nA test skill."), 0644); err != nil {
		t.Fatalf("failed to write SKILL.md: %v", err)
	}
	if err := os.WriteFile(filepath.Join(skillDir, "config.json"), []byte(`{"name":"test"}`), 0644); err != nil {
		t.Fatalf("failed to write config.json: %v", err)
	}
	if err := os.MkdirAll(filepath.Join(skillDir, "scripts"), 0755); err != nil {
		t.Fatalf("failed to create scripts subdirectory: %v", err)
	}
	if err := os.WriteFile(filepath.Join(skillDir, "scripts", "run.sh"), []byte("#!/bin/bash\necho hello"), 0644); err != nil {
		t.Fatalf("failed to write run.sh: %v", err)
	}

	ctx := context.Background()

	// Sign the skill directory
	err = signature.SignSkill(ctx, skillDir, priv, "test-signer")
	if err != nil {
		t.Fatalf("SignSkill returned error: %v", err)
	}

	// Verify the .signature file was created
	sigPath := filepath.Join(skillDir, ".signature")
	if _, err := os.Stat(sigPath); os.IsNotExist(err) {
		t.Fatal("expected .signature file to be created")
	}

	// Parse the signature file to verify structure
	sigData, err := os.ReadFile(sigPath)
	if err != nil {
		t.Fatalf("failed to read .signature file: %v", err)
	}
	var sigMap map[string]interface{}
	if err := json.Unmarshal(sigData, &sigMap); err != nil {
		t.Fatalf("failed to parse .signature JSON: %v", err)
	}
	if sigMap["algorithm"] != "ed25519" {
		t.Errorf("expected algorithm 'ed25519', got %v", sigMap["algorithm"])
	}
	if sigMap["signer_name"] != "test-signer" {
		t.Errorf("expected signer_name 'test-signer', got %v", sigMap["signer_name"])
	}

	// Verify using a Verifier with the signing key in the trust store
	trustStorePath := filepath.Join(tmpDir, "truststore.json")
	verifier, err := signature.NewVerifier(signature.Config{
		Enabled:    true,
		TrustStore: trustStorePath,
	})
	if err != nil {
		t.Fatalf("NewVerifier returned error: %v", err)
	}

	// Add the public key to the trust store
	ts := verifier.TrustStoreRef()
	if err := ts.AddTrustedKey(ctx, pub, "test-signer", signature.TrustVerified); err != nil {
		t.Fatalf("AddTrustedKey returned error: %v", err)
	}

	// Verify the skill
	result, err := verifier.VerifySkill(ctx, skillDir)
	if err != nil {
		t.Fatalf("VerifySkill returned error: %v", err)
	}
	if !result.Valid {
		t.Errorf("expected skill verification to succeed, got error: %s", result.Error)
	}
	if result.TrustLevel != signature.TrustVerified {
		t.Errorf("expected TrustVerified, got %v", result.TrustLevel)
	}
	if result.FilesVerified < 3 {
		t.Errorf("expected at least 3 files verified, got %d", result.FilesVerified)
	}
}

func TestSignature_TrustStore(t *testing.T) {
	tmpDir := t.TempDir()
	trustStorePath := filepath.Join(tmpDir, "truststore.json")

	ts, err := signature.NewTrustStore(trustStorePath)
	if err != nil {
		t.Fatalf("NewTrustStore returned error: %v", err)
	}

	pub, _, err := signature.GenerateKeyPair()
	if err != nil {
		t.Fatalf("GenerateKeyPair returned error: %v", err)
	}

	ctx := context.Background()

	// Initially the key should not be trusted
	level, trusted := ts.IsTrusted(pub)
	if trusted {
		t.Error("expected key to not be trusted initially")
	}
	if level != signature.TrustUnknown {
		t.Errorf("expected TrustUnknown, got %v", level)
	}

	// Add the key
	if err := ts.AddTrustedKey(ctx, pub, "test-signer", signature.TrustVerified); err != nil {
		t.Fatalf("AddTrustedKey returned error: %v", err)
	}

	// Now it should be trusted
	level, trusted = ts.IsTrusted(pub)
	if !trusted {
		t.Error("expected key to be trusted after adding")
	}
	if level != signature.TrustVerified {
		t.Errorf("expected TrustVerified, got %v", level)
	}

	// List keys
	keys := ts.ListKeys()
	if len(keys) != 1 {
		t.Errorf("expected 1 key, got %d", len(keys))
	}
	if keys[0].Name != "test-signer" {
		t.Errorf("expected name 'test-signer', got %q", keys[0].Name)
	}

	// Get key by public key
	k, found := ts.GetKey(pub)
	if !found {
		t.Error("expected to find key by public key")
	}
	if k.Name != "test-signer" {
		t.Errorf("expected name 'test-signer', got %q", k.Name)
	}

	// Get key by name
	k, found = ts.GetKeyByName("test-signer")
	if !found {
		t.Error("expected to find key by name")
	}

	// Revoke the key
	if err := ts.RevokeKey("test-signer"); err != nil {
		t.Fatalf("RevokeKey returned error: %v", err)
	}
	level, trusted = ts.IsTrusted(pub)
	if trusted {
		t.Error("expected revoked key to not be trusted")
	}
	if level != signature.TrustRevoked {
		t.Errorf("expected TrustRevoked, got %v", level)
	}
}

func TestSignature_TrustStore_Persistence(t *testing.T) {
	tmpDir := t.TempDir()
	trustStorePath := filepath.Join(tmpDir, "truststore.json")

	pub, _, err := signature.GenerateKeyPair()
	if err != nil {
		t.Fatalf("GenerateKeyPair returned error: %v", err)
	}

	ctx := context.Background()

	// Create and add a key
	ts1, err := signature.NewTrustStore(trustStorePath)
	if err != nil {
		t.Fatalf("NewTrustStore returned error: %v", err)
	}
	ts1.AddTrustedKey(ctx, pub, "persistent-signer", signature.TrustCommunity)

	// Verify file was written
	if _, err := os.Stat(trustStorePath); os.IsNotExist(err) {
		t.Fatal("expected trust store file to be created on disk")
	}

	// Reopen the trust store
	ts2, err := signature.NewTrustStore(trustStorePath)
	if err != nil {
		t.Fatalf("NewTrustStore (reopen) returned error: %v", err)
	}

	// The key should still be there
	level, trusted := ts2.IsTrusted(pub)
	if !trusted {
		t.Error("expected key to persist after reload")
	}
	if level != signature.TrustCommunity {
		t.Errorf("expected TrustCommunity, got %v", level)
	}

	// Verify by name
	k, found := ts2.GetKeyByName("persistent-signer")
	if !found {
		t.Error("expected to find key by name after persistence")
	}
	if k.Name != "persistent-signer" {
		t.Errorf("expected name 'persistent-signer', got %q", k.Name)
	}
}

func TestSignature_ExportImportPublicKey(t *testing.T) {
	pub, _, err := signature.GenerateKeyPair()
	if err != nil {
		t.Fatalf("GenerateKeyPair returned error: %v", err)
	}

	exported := signature.ExportPublicKey(pub)
	if exported == "" {
		t.Error("expected non-empty exported key")
	}

	imported, err := signature.ImportPublicKey(exported)
	if err != nil {
		t.Fatalf("ImportPublicKey returned error: %v", err)
	}
	if !pub.Equal(imported) {
		t.Error("expected imported key to match original")
	}
}

func TestSignature_ImportPublicKeyInvalid(t *testing.T) {
	_, err := signature.ImportPublicKey("not-valid-base64!!!")
	if err == nil {
		t.Error("expected error for invalid base64")
	}

	_, err = signature.ImportPublicKey("dG9vIHNob3J0") // "too short" in base64
	if err == nil {
		t.Error("expected error for wrong key size")
	}
}

func TestSignature_InMemoryTrustStore(t *testing.T) {
	ts, err := signature.NewTrustStore("")
	if err != nil {
		t.Fatalf("NewTrustStore('') returned error: %v", err)
	}

	pub, _, _ := signature.GenerateKeyPair()
	ctx := context.Background()

	ts.AddTrustedKey(ctx, pub, "memory-key", signature.TrustVerified)

	// Key should be there
	_, trusted := ts.IsTrusted(pub)
	if !trusted {
		t.Error("expected key in in-memory trust store")
	}

	// KeyCount
	if ts.KeyCount() != 1 {
		t.Errorf("expected key count 1, got %d", ts.KeyCount())
	}

	// FilePath should be empty
	if ts.FilePath() != "" {
		t.Errorf("expected empty FilePath for in-memory store, got %q", ts.FilePath())
	}
}

func TestSignature_RemoveKey(t *testing.T) {
	tmpDir := t.TempDir()
	ts, _ := signature.NewTrustStore(filepath.Join(tmpDir, "truststore.json"))
	ctx := context.Background()

	pub, _, _ := signature.GenerateKeyPair()
	ts.AddTrustedKey(ctx, pub, "removable-key", signature.TrustVerified)

	if ts.KeyCount() != 1 {
		t.Fatalf("expected 1 key, got %d", ts.KeyCount())
	}

	// Remove by name
	if err := ts.RemoveKey("removable-key"); err != nil {
		t.Fatalf("RemoveKey returned error: %v", err)
	}
	if ts.KeyCount() != 0 {
		t.Errorf("expected 0 keys after removal, got %d", ts.KeyCount())
	}

	// Remove non-existent key should not error
	if err := ts.RemoveKey("nonexistent"); err != nil {
		t.Errorf("expected nil for removing non-existent key, got: %v", err)
	}
}

func TestSignature_RemoveKeyByPublicKey(t *testing.T) {
	tmpDir := t.TempDir()
	ts, _ := signature.NewTrustStore(filepath.Join(tmpDir, "truststore.json"))
	ctx := context.Background()

	pub, _, _ := signature.GenerateKeyPair()
	ts.AddTrustedKey(ctx, pub, "pk-remove-test", signature.TrustCommunity)

	if err := ts.RemoveKeyByPublicKey(pub); err != nil {
		t.Fatalf("RemoveKeyByPublicKey returned error: %v", err)
	}
	if ts.KeyCount() != 0 {
		t.Errorf("expected 0 keys, got %d", ts.KeyCount())
	}
}

func TestSignature_RevokeKeyByPublicKey(t *testing.T) {
	tmpDir := t.TempDir()
	ts, _ := signature.NewTrustStore(filepath.Join(tmpDir, "truststore.json"))
	ctx := context.Background()

	pub, _, _ := signature.GenerateKeyPair()
	ts.AddTrustedKey(ctx, pub, "revoke-pk-test", signature.TrustVerified)

	if err := ts.RevokeKeyByPublicKey(pub); err != nil {
		t.Fatalf("RevokeKeyByPublicKey returned error: %v", err)
	}

	level, trusted := ts.IsTrusted(pub)
	if trusted {
		t.Error("expected revoked key to not be trusted")
	}
	if level != signature.TrustRevoked {
		t.Errorf("expected TrustRevoked, got %v", level)
	}
}

func TestSignature_TrustLevelString(t *testing.T) {
	tests := []struct {
		level    signature.TrustLevel
		expected string
	}{
		{signature.TrustUnknown, "unknown"},
		{signature.TrustCommunity, "community"},
		{signature.TrustVerified, "verified"},
		{signature.TrustRevoked, "revoked"},
		{signature.TrustLevel(99), "unknown"}, // unknown value
	}

	for _, tt := range tests {
		got := tt.level.String()
		if got != tt.expected {
			t.Errorf("TrustLevel(%d).String() = %q, want %q", tt.level, got, tt.expected)
		}
	}
}

func TestSignature_TrustLevelJSON(t *testing.T) {
	tests := []struct {
		level    signature.TrustLevel
		expected string
	}{
		{signature.TrustCommunity, `"community"`},
		{signature.TrustVerified, `"verified"`},
		{signature.TrustRevoked, `"revoked"`},
	}

	for _, tt := range tests {
		data, err := json.Marshal(tt.level)
		if err != nil {
			t.Errorf("Marshal(%v) returned error: %v", tt.level, err)
			continue
		}
		if string(data) != tt.expected {
			t.Errorf("Marshal(%v) = %s, want %s", tt.level, data, tt.expected)
		}

		// Unmarshal back
		var unmarshaled signature.TrustLevel
		if err := json.Unmarshal(data, &unmarshaled); err != nil {
			t.Errorf("Unmarshal returned error: %v", err)
			continue
		}
		if unmarshaled != tt.level {
			t.Errorf("Unmarshal roundtrip: got %v, want %v", unmarshaled, tt.level)
		}
	}
}

func TestSignature_VerifySkillTampered(t *testing.T) {
	pub, priv, err := signature.GenerateKeyPair()
	if err != nil {
		t.Fatalf("GenerateKeyPair returned error: %v", err)
	}

	tmpDir := t.TempDir()
	skillDir := filepath.Join(tmpDir, "tamper_skill")
	os.MkdirAll(skillDir, 0755)
	os.WriteFile(filepath.Join(skillDir, "SKILL.md"), []byte("original content"), 0644)

	ctx := context.Background()
	signature.SignSkill(ctx, skillDir, priv, "test-signer")

	// Tamper with the file
	os.WriteFile(filepath.Join(skillDir, "SKILL.md"), []byte("tampered content"), 0644)

	// Verify should fail
	trustStorePath := filepath.Join(tmpDir, "truststore.json")
	verifier, _ := signature.NewVerifier(signature.Config{
		Enabled:    true,
		TrustStore: trustStorePath,
	})
	verifier.TrustStoreRef().AddTrustedKey(ctx, pub, "test-signer", signature.TrustVerified)

	result, err := verifier.VerifySkill(ctx, skillDir)
	if err != nil {
		t.Fatalf("VerifySkill returned error: %v", err)
	}
	if result.Valid {
		t.Error("expected verification to fail for tampered skill directory")
	}
	if result.Error == "" {
		t.Error("expected non-empty error description for tampered skill")
	}
}

func TestSignature_VerifySkillNoSignature(t *testing.T) {
	tmpDir := t.TempDir()
	skillDir := filepath.Join(tmpDir, "unsigned_skill")
	os.MkdirAll(skillDir, 0755)
	os.WriteFile(filepath.Join(skillDir, "SKILL.md"), []byte("no signature"), 0644)

	trustStorePath := filepath.Join(tmpDir, "truststore.json")
	verifier, _ := signature.NewVerifier(signature.Config{
		Enabled:    true,
		TrustStore: trustStorePath,
	})

	ctx := context.Background()
	result, err := verifier.VerifySkill(ctx, skillDir)
	if err != nil {
		t.Fatalf("VerifySkill returned error: %v", err)
	}
	if result.Valid {
		t.Error("expected verification to fail for unsigned skill")
	}
	if result.Error == "" {
		t.Error("expected non-empty error description")
	}
}

func TestSignature_VerifySkillNotDirectory(t *testing.T) {
	tmpDir := t.TempDir()
	filePath := filepath.Join(tmpDir, "not_a_dir.txt")
	os.WriteFile(filePath, []byte("test"), 0644)

	trustStorePath := filepath.Join(tmpDir, "truststore.json")
	verifier, _ := signature.NewVerifier(signature.Config{
		Enabled:    true,
		TrustStore: trustStorePath,
	})

	ctx := context.Background()
	result, err := verifier.VerifySkill(ctx, filePath)
	if err == nil {
		t.Error("expected error when verifying a file instead of directory")
	}
	if result.Valid {
		t.Error("expected invalid result for non-directory path")
	}
}

func TestSignature_VerifyFileWithTrustedKeys(t *testing.T) {
	pub, priv, err := signature.GenerateKeyPair()
	if err != nil {
		t.Fatalf("GenerateKeyPair returned error: %v", err)
	}

	tmpDir := t.TempDir()
	filePath := filepath.Join(tmpDir, "signed_file.txt")
	os.WriteFile(filePath, []byte("test content"), 0644)

	ctx := context.Background()
	sigBytes, _ := signature.SignFile(ctx, filePath, priv)

	trustStorePath := filepath.Join(tmpDir, "truststore.json")
	verifier, _ := signature.NewVerifier(signature.Config{
		Enabled:    true,
		TrustStore: trustStorePath,
	})
	verifier.TrustStoreRef().AddTrustedKey(ctx, pub, "file-signer", signature.TrustVerified)

	result, err := verifier.VerifyFile(ctx, filePath, sigBytes)
	if err != nil {
		t.Fatalf("VerifyFile returned error: %v", err)
	}
	if !result.Valid {
		t.Errorf("expected valid file signature, got error: %s", result.Error)
	}
	if result.Signer != "file-signer" {
		t.Errorf("expected signer 'file-signer', got %q", result.Signer)
	}
}

func TestSignature_VerifyFileWithWrongKey(t *testing.T) {
	_, priv, _ := signature.GenerateKeyPair()
	otherPub, _, _ := signature.GenerateKeyPair()

	tmpDir := t.TempDir()
	filePath := filepath.Join(tmpDir, "wrong_key_file.txt")
	os.WriteFile(filePath, []byte("test content"), 0644)

	ctx := context.Background()
	sigBytes, _ := signature.SignFile(ctx, filePath, priv)

	trustStorePath := filepath.Join(tmpDir, "truststore.json")
	verifier, _ := signature.NewVerifier(signature.Config{
		Enabled:    true,
		TrustStore: trustStorePath,
	})
	// Add a DIFFERENT key to the trust store
	verifier.TrustStoreRef().AddTrustedKey(ctx, otherPub, "wrong-signer", signature.TrustVerified)

	result, err := verifier.VerifyFile(ctx, filePath, sigBytes)
	if err != nil {
		t.Fatalf("VerifyFile returned error: %v", err)
	}
	if result.Valid {
		t.Error("expected verification to fail when trust store has wrong key")
	}
}

func TestSignature_SignFileNonExistent(t *testing.T) {
	_, priv, _ := signature.GenerateKeyPair()
	ctx := context.Background()

	_, err := signature.SignFile(ctx, "/nonexistent/path/file.txt", priv)
	if err == nil {
		t.Error("expected error for non-existent file")
	}
}

func TestSignature_VerifyFileWithKeyNonExistent(t *testing.T) {
	pub, _, _ := signature.GenerateKeyPair()

	_, err := signature.VerifyFileWithKey("/nonexistent/path/file.txt", pub, []byte("sig"))
	if err == nil {
		t.Error("expected error for non-existent file")
	}
}
