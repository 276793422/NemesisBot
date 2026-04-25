package signature_test

import (
	"context"
	"crypto/ed25519"
	"os"
	"path/filepath"
	"testing"

	"github.com/276793422/NemesisBot/module/security/signature"
)

// ---------------------------------------------------------------------------
// GenerateKeyPair
// ---------------------------------------------------------------------------

func TestGenerateKeyPair(t *testing.T) {
	pub, priv, err := signature.GenerateKeyPair()
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if len(pub) != ed25519.PublicKeySize {
		t.Errorf("expected public key size %d, got %d", ed25519.PublicKeySize, len(pub))
	}
	if len(priv) != ed25519.PrivateKeySize {
		t.Errorf("expected private key size %d, got %d", ed25519.PrivateKeySize, len(priv))
	}
}

// ---------------------------------------------------------------------------
// ExportPublicKey / ImportPublicKey
// ---------------------------------------------------------------------------

func TestExportImportPublicKey(t *testing.T) {
	pub, _, err := signature.GenerateKeyPair()
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	exported := signature.ExportPublicKey(pub)
	if exported == "" {
		t.Error("expected non-empty exported key")
	}

	imported, err := signature.ImportPublicKey(exported)
	if err != nil {
		t.Fatalf("unexpected error importing: %v", err)
	}
	if !pub.Equal(imported) {
		t.Error("imported key does not match original")
	}
}

func TestImportPublicKey_InvalidBase64(t *testing.T) {
	_, err := signature.ImportPublicKey("not-valid-base64!!!")
	if err == nil {
		t.Error("expected error for invalid base64")
	}
}

func TestImportPublicKey_WrongSize(t *testing.T) {
	// Valid base64 but wrong number of bytes.
	imported, err := signature.ImportPublicKey("aG93ZHk=") // "howdy" = 6 bytes
	if err == nil {
		t.Errorf("expected error for wrong key size, got key: %v", imported)
	}
}

func TestImportPublicKey_EmptyString(t *testing.T) {
	_, err := signature.ImportPublicKey("")
	if err == nil {
		t.Error("expected error for empty string")
	}
}

// ---------------------------------------------------------------------------
// SignFile / VerifyFileWithKey
// ---------------------------------------------------------------------------

func TestSignFile_VerifyFileWithKey(t *testing.T) {
	// Create a temp file.
	dir := t.TempDir()
	filePath := filepath.Join(dir, "test.txt")
	content := []byte("hello, this is a test file")
	if err := os.WriteFile(filePath, content, 0644); err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	pub, priv, err := signature.GenerateKeyPair()
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	sig, err := signature.SignFile(context.Background(), filePath, priv)
	if err != nil {
		t.Fatalf("unexpected error signing: %v", err)
	}
	if len(sig) != ed25519.SignatureSize {
		t.Errorf("expected signature size %d, got %d", ed25519.SignatureSize, len(sig))
	}

	result, err := signature.VerifyFileWithKey(filePath, pub, sig)
	if err != nil {
		t.Fatalf("unexpected error verifying: %v", err)
	}
	if !result.Valid {
		t.Errorf("expected valid signature, got error: %q", result.Error)
	}
	if result.FilesVerified != 1 {
		t.Errorf("expected FilesVerified=1, got %d", result.FilesVerified)
	}
}

func TestSignFile_VerifyFileWithKey_TamperedContent(t *testing.T) {
	dir := t.TempDir()
	filePath := filepath.Join(dir, "test.txt")
	if err := os.WriteFile(filePath, []byte("original content"), 0644); err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	pub, priv, _ := signature.GenerateKeyPair()
	sig, _ := signature.SignFile(context.Background(), filePath, priv)

	// Tamper with file.
	if err := os.WriteFile(filePath, []byte("tampered content"), 0644); err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	result, _ := signature.VerifyFileWithKey(filePath, pub, sig)
	if result.Valid {
		t.Error("expected verification to fail for tampered content")
	}
}

func TestSignFile_NonExistentFile(t *testing.T) {
	_, priv, _ := signature.GenerateKeyPair()
	_, err := signature.SignFile(context.Background(), "/nonexistent/file.txt", priv)
	if err == nil {
		t.Error("expected error for non-existent file")
	}
}

func TestVerifyFileWithKey_WrongKey(t *testing.T) {
	dir := t.TempDir()
	filePath := filepath.Join(dir, "test.txt")
	if err := os.WriteFile(filePath, []byte("hello"), 0644); err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	_, priv1, _ := signature.GenerateKeyPair()
	pub2, _, _ := signature.GenerateKeyPair()

	sig, _ := signature.SignFile(context.Background(), filePath, priv1)
	result, _ := signature.VerifyFileWithKey(filePath, pub2, sig)
	if result.Valid {
		t.Error("expected verification to fail with wrong public key")
	}
}

func TestVerifyFileWithKey_NonExistentFile(t *testing.T) {
	pub, _, _ := signature.GenerateKeyPair()
	sig := make([]byte, ed25519.SignatureSize)
	_, err := signature.VerifyFileWithKey("/nonexistent/file.txt", pub, sig)
	if err == nil {
		t.Error("expected error for non-existent file")
	}
}

// ---------------------------------------------------------------------------
// SignSkill / VerifySkill
// ---------------------------------------------------------------------------

func TestSignSkill_VerifySkill(t *testing.T) {
	dir := t.TempDir()
	skillDir := filepath.Join(dir, "myskill")
	if err := os.MkdirAll(skillDir, 0755); err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	// Create skill files.
	if err := os.WriteFile(filepath.Join(skillDir, "SKILL.md"), []byte("# My Skill\nA test skill."), 0644); err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if err := os.WriteFile(filepath.Join(skillDir, "config.json"), []byte(`{"name":"test"}`), 0644); err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	pub, priv, _ := signature.GenerateKeyPair()

	// Sign the skill.
	err := signature.SignSkill(context.Background(), skillDir, priv, "test-signer")
	if err != nil {
		t.Fatalf("unexpected error signing skill: %v", err)
	}

	// Verify .signature file was created.
	sigPath := filepath.Join(skillDir, ".signature")
	if _, err := os.Stat(sigPath); os.IsNotExist(err) {
		t.Error("expected .signature file to be created")
	}

	// Add key to trust store for verification.
	tsPath := filepath.Join(dir, "truststore.json")
	v, err := signature.NewVerifier(signature.Config{
		Enabled:    true,
		TrustStore: tsPath,
	})
	if err != nil {
		t.Fatalf("unexpected error creating verifier: %v", err)
	}
	v.TrustStoreRef().AddTrustedKey(context.Background(), pub, "test-signer", signature.TrustVerified)

	result, err := v.VerifySkill(context.Background(), skillDir)
	if err != nil {
		t.Fatalf("unexpected error verifying skill: %v", err)
	}
	if !result.Valid {
		t.Errorf("expected valid skill signature, got error: %q", result.Error)
	}
	if result.Signer != "test-signer" {
		t.Errorf("expected signer 'test-signer', got %q", result.Signer)
	}
	if result.TrustLevel != signature.TrustVerified {
		t.Errorf("expected TrustVerified, got %v", result.TrustLevel)
	}
	if result.FilesVerified < 2 {
		t.Errorf("expected at least 2 files verified, got %d", result.FilesVerified)
	}
}

func TestVerifySkill_NoSignatureFile(t *testing.T) {
	dir := t.TempDir()
	skillDir := filepath.Join(dir, "nosig")
	if err := os.MkdirAll(skillDir, 0755); err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	os.WriteFile(filepath.Join(skillDir, "SKILL.md"), []byte("# Test"), 0644)

	tsPath := filepath.Join(dir, "truststore.json")
	v, err := signature.NewVerifier(signature.Config{
		Enabled:    true,
		TrustStore: tsPath,
	})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	result, err := v.VerifySkill(context.Background(), skillDir)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result.Valid {
		t.Error("expected invalid result for missing signature file")
	}
	if result.Error == "" {
		t.Error("expected error description")
	}
}

func TestVerifySkill_NotADirectory(t *testing.T) {
	dir := t.TempDir()
	filePath := filepath.Join(dir, "notadir.txt")
	os.WriteFile(filePath, []byte("test"), 0644)

	tsPath := filepath.Join(dir, "truststore.json")
	v, _ := signature.NewVerifier(signature.Config{
		Enabled:    true,
		TrustStore: tsPath,
	})

	_, err := v.VerifySkill(context.Background(), filePath)
	if err == nil {
		t.Error("expected error for non-directory path")
	}
}

func TestVerifySkill_NonExistentPath(t *testing.T) {
	dir := t.TempDir()
	tsPath := filepath.Join(dir, "truststore.json")
	v, _ := signature.NewVerifier(signature.Config{
		Enabled:    true,
		TrustStore: tsPath,
	})

	_, err := v.VerifySkill(context.Background(), "/nonexistent/path")
	if err == nil {
		t.Error("expected error for non-existent path")
	}
}

func TestSignSkill_TamperedContent(t *testing.T) {
	dir := t.TempDir()
	skillDir := filepath.Join(dir, "tampered")
	os.MkdirAll(skillDir, 0755)
	os.WriteFile(filepath.Join(skillDir, "SKILL.md"), []byte("original"), 0644)

	_, priv, _ := signature.GenerateKeyPair()
	signature.SignSkill(context.Background(), skillDir, priv, "signer")

	// Tamper with content after signing.
	os.WriteFile(filepath.Join(skillDir, "SKILL.md"), []byte("tampered"), 0644)

	tsPath := filepath.Join(dir, "truststore.json")
	v, _ := signature.NewVerifier(signature.Config{
		Enabled:    true,
		TrustStore: tsPath,
	})

	result, err := v.VerifySkill(context.Background(), skillDir)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result.Valid {
		t.Error("expected verification to fail for tampered content")
	}
}

func TestSignSkill_EmptyDirectory(t *testing.T) {
	dir := t.TempDir()
	skillDir := filepath.Join(dir, "empty")
	os.MkdirAll(skillDir, 0755)

	_, priv, _ := signature.GenerateKeyPair()
	err := signature.SignSkill(context.Background(), skillDir, priv, "signer")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
}

// ---------------------------------------------------------------------------
// VerifyFile (with trust store)
// ---------------------------------------------------------------------------

func TestVerifier_VerifyFile(t *testing.T) {
	dir := t.TempDir()
	filePath := filepath.Join(dir, "test.txt")
	os.WriteFile(filePath, []byte("hello world"), 0644)

	pub, priv, _ := signature.GenerateKeyPair()

	tsPath := filepath.Join(dir, "truststore.json")
	v, err := signature.NewVerifier(signature.Config{
		Enabled:    true,
		TrustStore: tsPath,
	})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	// Add key to trust store.
	v.TrustStoreRef().AddTrustedKey(context.Background(), pub, "test-signer", signature.TrustCommunity)

	sig, _ := signature.SignFile(context.Background(), filePath, priv)
	result, err := v.VerifyFile(context.Background(), filePath, sig)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if !result.Valid {
		t.Errorf("expected valid, got error: %q", result.Error)
	}
	if result.Signer != "test-signer" {
		t.Errorf("expected signer 'test-signer', got %q", result.Signer)
	}
}

func TestVerifier_VerifyFile_UnknownKey(t *testing.T) {
	dir := t.TempDir()
	filePath := filepath.Join(dir, "test.txt")
	os.WriteFile(filePath, []byte("hello"), 0644)

	_, priv, _ := signature.GenerateKeyPair()

	tsPath := filepath.Join(dir, "truststore.json")
	v, _ := signature.NewVerifier(signature.Config{
		Enabled:    true,
		TrustStore: tsPath,
	})

	sig, _ := signature.SignFile(context.Background(), filePath, priv)
	result, _ := v.VerifyFile(context.Background(), filePath, sig)
	if result.Valid {
		t.Error("expected verification to fail with unknown key")
	}
}

func TestVerifier_VerifyFile_RevokedKey(t *testing.T) {
	dir := t.TempDir()
	filePath := filepath.Join(dir, "test.txt")
	os.WriteFile(filePath, []byte("hello"), 0644)

	pub, priv, _ := signature.GenerateKeyPair()

	tsPath := filepath.Join(dir, "truststore.json")
	v, _ := signature.NewVerifier(signature.Config{
		Enabled:    true,
		TrustStore: tsPath,
	})

	v.TrustStoreRef().AddTrustedKey(context.Background(), pub, "revoke-me", signature.TrustVerified)
	v.TrustStoreRef().RevokeKey("revoke-me")

	sig, _ := signature.SignFile(context.Background(), filePath, priv)
	result, _ := v.VerifyFile(context.Background(), filePath, sig)
	if result.Valid {
		t.Error("expected verification to fail with revoked key")
	}
}

func TestVerifier_VerifyFile_NonExistentFile(t *testing.T) {
	dir := t.TempDir()
	tsPath := filepath.Join(dir, "truststore.json")
	v, _ := signature.NewVerifier(signature.Config{
		Enabled:    true,
		TrustStore: tsPath,
	})

	sig := make([]byte, ed25519.SignatureSize)
	_, err := v.VerifyFile(context.Background(), "/nonexistent/file.txt", sig)
	if err == nil {
		t.Error("expected error for non-existent file")
	}
}

// ---------------------------------------------------------------------------
// NewVerifier
// ---------------------------------------------------------------------------

func TestNewVerifier(t *testing.T) {
	dir := t.TempDir()
	tsPath := filepath.Join(dir, "truststore.json")

	v, err := signature.NewVerifier(signature.Config{
		Enabled:    true,
		TrustStore: tsPath,
	})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if v == nil {
		t.Fatal("expected non-nil verifier")
	}
}

func TestNewVerifier_EmptyTrustStore(t *testing.T) {
	v, err := signature.NewVerifier(signature.Config{
		Enabled:    true,
		TrustStore: "",
	})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if v == nil {
		t.Fatal("expected non-nil verifier")
	}
}

// ---------------------------------------------------------------------------
// TrustLevel
// ---------------------------------------------------------------------------

func TestTrustLevel_String(t *testing.T) {
	tests := []struct {
		level signature.TrustLevel
		want  string
	}{
		{signature.TrustUnknown, "unknown"},
		{signature.TrustCommunity, "community"},
		{signature.TrustVerified, "verified"},
		{signature.TrustRevoked, "revoked"},
		{signature.TrustLevel(99), "unknown"},
	}
	for _, tt := range tests {
		if got := tt.level.String(); got != tt.want {
			t.Errorf("TrustLevel(%d).String() = %q, want %q", tt.level, got, tt.want)
		}
	}
}

func TestTrustLevel_MarshalJSON(t *testing.T) {
	tests := []struct {
		level signature.TrustLevel
		want  string
	}{
		{signature.TrustVerified, `"verified"`},
		{signature.TrustCommunity, `"community"`},
	}
	for _, tt := range tests {
		data, err := tt.level.MarshalJSON()
		if err != nil {
			t.Errorf("unexpected error: %v", err)
		}
		if string(data) != tt.want {
			t.Errorf("MarshalJSON() = %q, want %q", string(data), tt.want)
		}
	}
}

func TestTrustLevel_UnmarshalJSON(t *testing.T) {
	tests := []struct {
		json  string
		want  signature.TrustLevel
	}{
		{`"verified"`, signature.TrustVerified},
		{`"community"`, signature.TrustCommunity},
		{`"revoked"`, signature.TrustRevoked},
		{`"unknown"`, signature.TrustUnknown},
		{`"bogus"`, signature.TrustUnknown},
	}
	for _, tt := range tests {
		var level signature.TrustLevel
		if err := level.UnmarshalJSON([]byte(tt.json)); err != nil {
			t.Errorf("unexpected error for %s: %v", tt.json, err)
		}
		if level != tt.want {
			t.Errorf("UnmarshalJSON(%s) = %d, want %d", tt.json, level, tt.want)
		}
	}
}
