// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package skills

import (
	"context"
	"crypto/ed25519"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"

	"github.com/276793422/NemesisBot/module/security/signature"
)

// SkillSigner provides high-level signing and verification operations for skills.
// It wraps the security/signature package with skill-specific logic.
type SkillSigner struct {
	verifier *signature.Verifier
}

// NewSkillSigner creates a SkillSigner using the trust store at configPath.
// If configPath is empty, an in-memory trust store is used.
func NewSkillSigner(configPath string) (*SkillSigner, error) {
	cfg := signature.Config{
		Enabled:    true,
		Strict:     false,
		TrustStore: configPath,
	}
	verifier, err := signature.NewVerifier(cfg)
	if err != nil {
		return nil, fmt.Errorf("failed to create signature verifier: %w", err)
	}
	return &SkillSigner{
		verifier: verifier,
	}, nil
}

// SignSkill signs all files in the skill directory at skillPath using the private
// key loaded from keyPath and writes a .signature file.
//
// keyPath should point to a file containing the raw Ed25519 private key bytes
// (64 bytes: 32-byte seed concatenated with 32-byte public key).
func (ss *SkillSigner) SignSkill(skillPath string, keyPath string) error {
	// Validate the skill directory exists.
	info, err := os.Stat(skillPath)
	if err != nil {
		return fmt.Errorf("cannot access skill directory: %w", err)
	}
	if !info.IsDir() {
		return fmt.Errorf("skill path is not a directory: %s", skillPath)
	}

	// Load the private key.
	privateKey, err := loadPrivateKey(keyPath)
	if err != nil {
		return fmt.Errorf("failed to load private key: %w", err)
	}

	// Use the signature package to sign the skill directory.
	ctx := context.Background()
	if err := signature.SignSkill(ctx, skillPath, privateKey, ""); err != nil {
		return fmt.Errorf("failed to sign skill: %w", err)
	}

	return nil
}

// VerifySkill verifies the signature of the skill at skillPath.
// It returns a VerificationResult indicating whether the signature is valid,
// who signed it, and the trust level.
func (ss *SkillSigner) VerifySkill(skillPath string) (*signature.VerificationResult, error) {
	ctx := context.Background()
	return ss.verifier.VerifySkill(ctx, skillPath)
}

// GenerateKeyPair generates a new Ed25519 key pair and saves it to outputDir.
// The public key is saved as "skill_sign.pub" and the private key as "skill_sign.key".
// Returns the path to the output directory.
func (ss *SkillSigner) GenerateKeyPair(outputDir string) (string, error) {
	pubKey, privKey, err := signature.GenerateKeyPair()
	if err != nil {
		return "", fmt.Errorf("failed to generate key pair: %w", err)
	}

	// Ensure the output directory exists.
	if err := os.MkdirAll(outputDir, 0700); err != nil {
		return "", fmt.Errorf("failed to create output directory: %w", err)
	}

	// Save the private key (64 bytes: seed + public key).
	privPath := filepath.Join(outputDir, "skill_sign.key")
	if err := os.WriteFile(privPath, privKey, 0600); err != nil {
		return "", fmt.Errorf("failed to write private key: %w", err)
	}

	// Save the public key as raw bytes.
	pubPath := filepath.Join(outputDir, "skill_sign.pub")
	if err := os.WriteFile(pubPath, pubKey, 0644); err != nil {
		return "", fmt.Errorf("failed to write public key: %w", err)
	}

	// Also save a human-readable metadata file with the public key in base64
	// and its fingerprint for easy reference.
	metadata := keyMetadata{
		PublicKey:   base64.StdEncoding.EncodeToString(pubKey),
		Fingerprint: computePublicKeyFingerprint(pubKey),
		Algorithm:   "ed25519",
	}
	metaPath := filepath.Join(outputDir, "skill_sign.meta.json")
	metaData, _ := json.MarshalIndent(metadata, "", "  ")
	_ = os.WriteFile(metaPath, metaData, 0644)

	return outputDir, nil
}

// Verifier returns the underlying signature.Verifier for advanced operations.
func (ss *SkillSigner) Verifier() *signature.Verifier {
	return ss.verifier
}

// --- internal helpers ---

// keyMetadata is a helper struct for the metadata JSON file.
type keyMetadata struct {
	PublicKey   string `json:"public_key"`
	Fingerprint string `json:"fingerprint"`
	Algorithm   string `json:"algorithm"`
}

// loadPrivateKey reads an Ed25519 private key from the given file path.
// The file should contain the raw 64-byte private key.
func loadPrivateKey(keyPath string) (ed25519.PrivateKey, error) {
	data, err := os.ReadFile(keyPath)
	if err != nil {
		return nil, fmt.Errorf("cannot read key file: %w", err)
	}

	if len(data) != ed25519.PrivateKeySize {
		return nil, fmt.Errorf("invalid private key size: expected %d bytes, got %d",
			ed25519.PrivateKeySize, len(data))
	}

	return ed25519.PrivateKey(data), nil
}

// computePublicKeyFingerprint returns a SHA-256 hex fingerprint of the public key,
// matching the format used by the signature package.
func computePublicKeyFingerprint(pubKey ed25519.PublicKey) string {
	return signature.ExportPublicKey(pubKey)
}
