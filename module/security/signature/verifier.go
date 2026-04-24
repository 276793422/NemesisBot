// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

// Package signature provides Ed25519-based skill signature verification for NemesisBot.
//
// Signature format:
//   - Single file: SHA-256(file content) -> Ed25519 sign
//   - Directory:   sorted file paths, each SHA-256 concatenated -> SHA-256 of concatenation -> Ed25519 sign
//
// A skill directory stores its signature in a .signature file at the skill root.
package signature

import (
	"context"
	"crypto/ed25519"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"
)

// TrustLevel represents the trust level of a signing key.
type TrustLevel int

const (
	// TrustUnknown means the key is not in the trust store.
	TrustUnknown TrustLevel = iota
	// TrustCommunity means the key belongs to a community signer.
	TrustCommunity
	// TrustVerified means the key belongs to an officially verified signer.
	TrustVerified
	// TrustRevoked means the key has been revoked and should not be trusted.
	TrustRevoked
)

// String returns a human-readable name for the trust level.
func (t TrustLevel) String() string {
	switch t {
	case TrustUnknown:
		return "unknown"
	case TrustCommunity:
		return "community"
	case TrustVerified:
		return "verified"
	case TrustRevoked:
		return "revoked"
	default:
		return "unknown"
	}
}

// MarshalJSON implements json.Marshaler for TrustLevel.
func (t TrustLevel) MarshalJSON() ([]byte, error) {
	return json.Marshal(t.String())
}

// UnmarshalJSON implements json.Unmarshaler for TrustLevel.
func (t *TrustLevel) UnmarshalJSON(data []byte) error {
	var s string
	if err := json.Unmarshal(data, &s); err != nil {
		return err
	}
	switch strings.ToLower(s) {
	case "community":
		*t = TrustCommunity
	case "verified":
		*t = TrustVerified
	case "revoked":
		*t = TrustRevoked
	default:
		*t = TrustUnknown
	}
	return nil
}

// TrustedKey represents a key entry in the trust store.
type TrustedKey struct {
	PublicKey   string    `json:"public_key"` // base64-encoded Ed25519 public key
	Name        string    `json:"name"`       // signer name / identifier
	Level       TrustLevel `json:"level"`
	AddedAt     time.Time `json:"added_at"`
	Fingerprint string    `json:"fingerprint"` // SHA-256 of public key bytes for display
}

// Config holds the configuration for the signature verifier.
type Config struct {
	Enabled    bool   // whether signature verification is enabled
	Strict     bool   // if true, unsigned skills are rejected; if false, warn only
	TrustStore string // path to the trust store JSON file
}

// VerificationResult holds the outcome of a signature verification.
type VerificationResult struct {
	Valid         bool       `json:"valid"`
	Signer        string     `json:"signer,omitempty"` // name of signer if in trust store
	TrustLevel    TrustLevel `json:"trust_level"`
	Algorithm     string     `json:"algorithm"`               // "ed25519"
	Error         string     `json:"error,omitempty"`          // error description if verification failed
	FilesVerified int        `json:"files_verified"`           // number of files that were verified
	Timestamp     time.Time  `json:"timestamp"`                // when verification was performed
}

// skillSignature is the on-disk signature envelope stored in .signature files.
type skillSignature struct {
	Algorithm  string `json:"algorithm"`            // "ed25519"
	Signature  string `json:"signature"`            // base64-encoded Ed25519 signature
	PublicKey  string `json:"public_key"`            // base64-encoded signer public key
	SignerName string `json:"signer_name,omitempty"` // optional signer name hint
	SignedAt   string `json:"signed_at"`             // ISO 8601 timestamp
	FileCount  int    `json:"file_count"`             // number of files covered
	Hash       string `json:"hash"`                  // SHA-256 hex of the aggregate content
}

const (
	// signatureFileName is the name of the signature file placed in skill directories.
	signatureFileName = ".signature"

	// algorithmName identifies the signing algorithm.
	algorithmName = "ed25519"
)

// Verifier performs Ed25519 signature verification for skills and files.
type Verifier struct {
	trustStore *TrustStore
	config     Config
}

// NewVerifier creates a new Verifier with the given configuration.
// If config.TrustStore is empty, a default path is not assumed -- the caller
// should set it explicitly. If the file does not exist an empty trust store
// is used.
func NewVerifier(cfg Config) (*Verifier, error) {
	ts, err := NewTrustStore(cfg.TrustStore)
	if err != nil {
		return nil, fmt.Errorf("failed to load trust store: %w", err)
	}
	return &Verifier{
		trustStore: ts,
		config:     cfg,
	}, nil
}

// TrustStoreRef returns the underlying TrustStore for direct manipulation.
func (v *Verifier) TrustStoreRef() *TrustStore {
	return v.trustStore
}

// VerifySkill verifies the signature of an entire skill directory.
//
// The method reads the .signature file inside skillPath, recomputes the
// aggregate hash over all files (excluding .signature), and checks the
// Ed25519 signature against the embedded public key.
func (v *Verifier) VerifySkill(ctx context.Context, skillPath string) (*VerificationResult, error) {
	now := time.Now().UTC()

	// Validate the path exists and is a directory.
	info, err := os.Stat(skillPath)
	if err != nil {
		return &VerificationResult{
			Valid:     false,
			Algorithm: algorithmName,
			Error:     fmt.Sprintf("cannot access skill path: %v", err),
			Timestamp: now,
		}, fmt.Errorf("cannot access skill path: %w", err)
	}
	if !info.IsDir() {
		return &VerificationResult{
			Valid:     false,
			Algorithm: algorithmName,
			Error:     "skill path is not a directory",
			Timestamp: now,
		}, fmt.Errorf("skill path is not a directory: %s", skillPath)
	}

	// Read the .signature file.
	sigPath := filepath.Join(skillPath, signatureFileName)
	sigData, err := os.ReadFile(sigPath)
	if err != nil {
		if os.IsNotExist(err) {
			return &VerificationResult{
				Valid:     false,
				Algorithm: algorithmName,
				Error:     "no signature file found (.signature)",
				Timestamp: now,
			}, nil
		}
		return &VerificationResult{
			Valid:     false,
			Algorithm: algorithmName,
			Error:     fmt.Sprintf("cannot read signature file: %v", err),
			Timestamp: now,
		}, fmt.Errorf("cannot read signature file: %w", err)
	}

	// Parse the signature envelope.
	var sig skillSignature
	if err := json.Unmarshal(sigData, &sig); err != nil {
		return &VerificationResult{
			Valid:     false,
			Algorithm: algorithmName,
			Error:     fmt.Sprintf("invalid signature format: %v", err),
			Timestamp: now,
		}, fmt.Errorf("invalid signature format: %w", err)
	}

	if sig.Algorithm != algorithmName {
		return &VerificationResult{
			Valid:     false,
			Algorithm: sig.Algorithm,
			Error:     fmt.Sprintf("unsupported algorithm: %s", sig.Algorithm),
			Timestamp: now,
		}, fmt.Errorf("unsupported algorithm: %s", sig.Algorithm)
	}

	// Decode the public key and signature bytes.
	pubKey, err := ImportPublicKey(sig.PublicKey)
	if err != nil {
		return &VerificationResult{
			Valid:     false,
			Algorithm: algorithmName,
			Error:     fmt.Sprintf("invalid public key in signature: %v", err),
			Timestamp: now,
		}, fmt.Errorf("invalid public key in signature: %w", err)
	}

	sigBytes, err := base64.StdEncoding.DecodeString(sig.Signature)
	if err != nil {
		return &VerificationResult{
			Valid:     false,
			Algorithm: algorithmName,
			Error:     fmt.Sprintf("invalid signature encoding: %v", err),
			Timestamp: now,
		}, fmt.Errorf("invalid signature encoding: %w", err)
	}

	// Compute the aggregate hash of all files in the directory (excluding .signature).
	aggregateHash, fileCount, err := computeDirectoryHash(skillPath)
	if err != nil {
		return &VerificationResult{
			Valid:     false,
			Algorithm: algorithmName,
			Error:     fmt.Sprintf("failed to hash directory: %v", err),
			Timestamp: now,
		}, fmt.Errorf("failed to hash directory: %w", err)
	}

	// Check the embedded hash against the computed one.
	computedHex := fmt.Sprintf("%x", aggregateHash)
	if sig.Hash != computedHex {
		return &VerificationResult{
			Valid:         false,
			Algorithm:     algorithmName,
			FilesVerified: fileCount,
			Error:         "content hash mismatch -- files have been modified",
			Timestamp:     now,
		}, nil
	}

	// Verify the Ed25519 signature over the aggregate hash.
	if !ed25519.Verify(pubKey, aggregateHash[:], sigBytes) {
		return &VerificationResult{
			Valid:         false,
			Algorithm:     algorithmName,
			FilesVerified: fileCount,
			Error:         "signature verification failed",
			Timestamp:     now,
		}, nil
	}

	// Look up the signer in the trust store.
	signerName := sig.SignerName
	trustLevel, trusted := v.trustStore.IsTrusted(pubKey)
	if trusted {
		// Prefer the trust store name over the embedded hint.
		keys := v.trustStore.ListKeys()
		for _, k := range keys {
			if k.PublicKey == sig.PublicKey {
				signerName = k.Name
				break
			}
		}
	}

	return &VerificationResult{
		Valid:         true,
		Signer:        signerName,
		TrustLevel:    trustLevel,
		Algorithm:     algorithmName,
		FilesVerified: fileCount,
		Timestamp:     now,
	}, nil
}

// VerifyFile verifies the Ed25519 signature of a single file.
//
// The signature should be the raw Ed25519 signature bytes (not the
// skillSignature envelope). The verification computes SHA-256 of the file
// content and checks it against the provided public key and signature.
func (v *Verifier) VerifyFile(ctx context.Context, filePath string, signature []byte) (*VerificationResult, error) {
	now := time.Now().UTC()

	content, err := os.ReadFile(filePath)
	if err != nil {
		return &VerificationResult{
			Valid:     false,
			Algorithm: algorithmName,
			Error:     fmt.Sprintf("cannot read file: %v", err),
			Timestamp: now,
		}, fmt.Errorf("cannot read file: %w", err)
	}

	hash := sha256.Sum256(content)

	// We need a public key to verify against. Since the signature bytes alone
	// don't carry the public key, we try all keys in the trust store.
	keys := v.trustStore.ListKeys()
	for _, k := range keys {
		if k.Level == TrustRevoked {
			continue
		}
		pubKey, err := ImportPublicKey(k.PublicKey)
		if err != nil {
			continue
		}
		if ed25519.Verify(pubKey, hash[:], signature) {
			return &VerificationResult{
				Valid:         true,
				Signer:        k.Name,
				TrustLevel:    k.Level,
				Algorithm:     algorithmName,
				FilesVerified: 1,
				Timestamp:     now,
			}, nil
		}
	}

	return &VerificationResult{
		Valid:         false,
		Algorithm:     algorithmName,
		FilesVerified: 1,
		Error:         "signature does not match any trusted key",
		Timestamp:     now,
	}, nil
}

// VerifyFileWithKey verifies a single file's signature against a specific public key.
func VerifyFileWithKey(filePath string, pubKey ed25519.PublicKey, signature []byte) (*VerificationResult, error) {
	now := time.Now().UTC()

	content, err := os.ReadFile(filePath)
	if err != nil {
		return &VerificationResult{
			Valid:     false,
			Algorithm: algorithmName,
			Error:     fmt.Sprintf("cannot read file: %v", err),
			Timestamp: now,
		}, fmt.Errorf("cannot read file: %w", err)
	}

	hash := sha256.Sum256(content)

	if !ed25519.Verify(pubKey, hash[:], signature) {
		return &VerificationResult{
			Valid:         false,
			Algorithm:     algorithmName,
			FilesVerified: 1,
			Error:         "signature verification failed",
			Timestamp:     now,
		}, nil
	}

	return &VerificationResult{
		Valid:         true,
		Algorithm:     algorithmName,
		FilesVerified: 1,
		Timestamp:     now,
	}, nil
}

// SignFile creates an Ed25519 signature of a single file.
//
// It computes SHA-256 of the file content and signs it with the provided
// private key. The returned bytes are the raw Ed25519 signature.
func SignFile(ctx context.Context, filePath string, privateKey ed25519.PrivateKey) ([]byte, error) {
	content, err := os.ReadFile(filePath)
	if err != nil {
		return nil, fmt.Errorf("cannot read file: %w", err)
	}

	hash := sha256.Sum256(content)
	return ed25519.Sign(privateKey, hash[:]), nil
}

// SignSkill signs an entire skill directory and writes the .signature file.
//
// It computes the aggregate hash of all files in the directory, signs it with
// the provided private key, and writes a skillSignature envelope to
// {skillPath}/.signature.
func SignSkill(ctx context.Context, skillPath string, privateKey ed25519.PrivateKey, signerName string) error {
	pubKey := privateKey.Public().(ed25519.PublicKey)

	aggregateHash, fileCount, err := computeDirectoryHash(skillPath)
	if err != nil {
		return fmt.Errorf("failed to hash directory: %w", err)
	}

	sigBytes := ed25519.Sign(privateKey, aggregateHash[:])

	sig := skillSignature{
		Algorithm:  algorithmName,
		Signature:  base64.StdEncoding.EncodeToString(sigBytes),
		PublicKey:  ExportPublicKey(pubKey),
		SignerName: signerName,
		SignedAt:   time.Now().UTC().Format(time.RFC3339),
		FileCount:  fileCount,
		Hash:       fmt.Sprintf("%x", aggregateHash),
	}

	sigData, err := json.MarshalIndent(sig, "", "  ")
	if err != nil {
		return fmt.Errorf("failed to marshal signature: %w", err)
	}

	sigPath := filepath.Join(skillPath, signatureFileName)
	if err := os.WriteFile(sigPath, sigData, 0644); err != nil {
		return fmt.Errorf("failed to write signature file: %w", err)
	}

	return nil
}

// GenerateKeyPair generates a new Ed25519 key pair.
func GenerateKeyPair() (ed25519.PublicKey, ed25519.PrivateKey, error) {
	return ed25519.GenerateKey(nil)
}

// ExportPublicKey encodes a public key as a base64 string.
func ExportPublicKey(pub ed25519.PublicKey) string {
	return base64.StdEncoding.EncodeToString(pub)
}

// ImportPublicKey decodes a public key from a base64 string.
func ImportPublicKey(s string) (ed25519.PublicKey, error) {
	b, err := base64.StdEncoding.DecodeString(s)
	if err != nil {
		return nil, fmt.Errorf("invalid base64: %w", err)
	}
	if len(b) != ed25519.PublicKeySize {
		return nil, fmt.Errorf("invalid public key size: expected %d bytes, got %d",
			ed25519.PublicKeySize, len(b))
	}
	return ed25519.PublicKey(b), nil
}

// computeFingerprint returns a human-readable fingerprint (SHA-256 hex) of a
// public key, suitable for display and comparison.
func computeFingerprint(pub ed25519.PublicKey) string {
	h := sha256.Sum256(pub)
	return fmt.Sprintf("%x", h)
}

// computeDirectoryHash computes a deterministic hash over all regular files
// in the given directory (excluding the .signature file). Files are sorted by
// their relative path, and each file's SHA-256 hash is concatenated. The
// SHA-256 of the concatenation is returned as the aggregate hash.
func computeDirectoryHash(dirPath string) ([sha256.Size]byte, int, error) {
	type fileHash struct {
		relPath string
		hash    [sha256.Size]byte
	}

	var entries []fileHash

	err := filepath.WalkDir(dirPath, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		// Skip directories.
		if d.IsDir() {
			return nil
		}
		// Skip the signature file itself.
		if filepath.Base(path) == signatureFileName {
			return nil
		}

		rel, err := filepath.Rel(dirPath, path)
		if err != nil {
			return fmt.Errorf("cannot compute relative path for %s: %w", path, err)
		}
		// Normalize to forward slashes for cross-platform determinism.
		rel = filepath.ToSlash(rel)

		content, err := os.ReadFile(path)
		if err != nil {
			return fmt.Errorf("cannot read file %s: %w", path, err)
		}

		h := sha256.Sum256(content)
		entries = append(entries, fileHash{relPath: rel, hash: h})
		return nil
	})
	if err != nil {
		return [sha256.Size]byte{}, 0, err
	}

	// Sort by relative path for deterministic ordering.
	sort.Slice(entries, func(i, j int) bool {
		return entries[i].relPath < entries[j].relPath
	})

	// Concatenate all hashes.
	var buf []byte
	for _, e := range entries {
		// Include the relative path in the hash to prevent path manipulation.
		buf = append(buf, []byte(e.relPath)...)
		buf = append(buf, e.hash[:]...)
	}

	aggregate := sha256.Sum256(buf)
	return aggregate, len(entries), nil
}
