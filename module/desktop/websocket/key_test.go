//go:build !cross_compile

package websocket

import (
	"errors"
	"testing"
	"time"
)

func TestNewKeyGenerator(t *testing.T) {
	kg := NewKeyGenerator()
	if kg == nil {
		t.Fatal("NewKeyGenerator returned nil")
	}
	if kg.keys == nil {
		t.Error("keys map should be initialized")
	}
	if kg.nextPort != 40000 {
		t.Errorf("Expected initial port 40000, got %d", kg.nextPort)
	}
}

func TestKeyGeneratorGenerate(t *testing.T) {
	kg := NewKeyGenerator()

	key, err := kg.Generate("child-1", 1234)
	if err != nil {
		t.Fatalf("Generate failed: %v", err)
	}
	if key.Key == "" {
		t.Error("Key should not be empty")
	}
	if key.Port != 40000 {
		t.Errorf("Expected port 40000, got %d", key.Port)
	}
	if key.Path != "/child/"+key.Key {
		t.Errorf("Expected path '/child/%s', got '%s'", key.Key, key.Path)
	}
	if !key.Valid {
		t.Error("Key should be valid")
	}
	if key.ChildID != "child-1" {
		t.Errorf("Expected ChildID 'child-1', got '%s'", key.ChildID)
	}
	if key.ChildPID != 1234 {
		t.Errorf("Expected ChildPID 1234, got %d", key.ChildPID)
	}
	if key.CreatedAt.IsZero() {
		t.Error("CreatedAt should be set")
	}
}

func TestKeyGeneratorSequentialPorts(t *testing.T) {
	kg := NewKeyGenerator()

	key1, _ := kg.Generate("child-1", 100)
	key2, _ := kg.Generate("child-2", 200)

	if key1.Port >= key2.Port {
		t.Errorf("Expected sequential ports: %d < %d", key1.Port, key2.Port)
	}
}

func TestKeyGeneratorValidate(t *testing.T) {
	kg := NewKeyGenerator()

	genKey, _ := kg.Generate("child-1", 100)

	validated, err := kg.Validate(genKey.Key)
	if err != nil {
		t.Fatalf("Validate failed: %v", err)
	}
	if validated.Key != genKey.Key {
		t.Errorf("Expected key '%s', got '%s'", genKey.Key, validated.Key)
	}
	if validated.UsedAt.IsZero() {
		t.Error("UsedAt should be set after validation")
	}
}

func TestKeyGeneratorValidateNotFound(t *testing.T) {
	kg := NewKeyGenerator()

	_, err := kg.Validate("nonexistent")
	if err == nil {
		t.Error("Expected error for nonexistent key")
	}
	if !errors.Is(err, ErrKeyNotFound) {
		t.Errorf("Expected ErrKeyNotFound, got: %v", err)
	}
}

func TestKeyGeneratorValidateRevoked(t *testing.T) {
	kg := NewKeyGenerator()

	genKey, _ := kg.Generate("child-1", 100)
	kg.Revoke(genKey.Key)

	_, err := kg.Validate(genKey.Key)
	if err == nil {
		t.Error("Expected error for revoked key")
	}
	if !errors.Is(err, ErrKeyInvalid) {
		t.Errorf("Expected ErrKeyInvalid, got: %v", err)
	}
}

func TestKeyGeneratorRevoke(t *testing.T) {
	kg := NewKeyGenerator()

	genKey, _ := kg.Generate("child-1", 100)
	err := kg.Revoke(genKey.Key)
	if err != nil {
		t.Errorf("Revoke failed: %v", err)
	}

	// Verify key is invalid
	wsKey, ok := kg.keys[genKey.Key]
	if !ok {
		t.Fatal("Key should still exist in map")
	}
	if wsKey.Valid {
		t.Error("Key should be marked as invalid after revoke")
	}
}

func TestKeyGeneratorRevokeNotFound(t *testing.T) {
	kg := NewKeyGenerator()

	err := kg.Revoke("nonexistent")
	if err == nil {
		t.Error("Expected error for revoking nonexistent key")
	}
	if !errors.Is(err, ErrKeyNotFound) {
		t.Errorf("Expected ErrKeyNotFound, got: %v", err)
	}
}

func TestKeyGeneratorCleanup(t *testing.T) {
	kg := NewKeyGenerator()

	// Generate a key
	key1, _ := kg.Generate("child-1", 100)

	// Manually age it by modifying CreatedAt
	kg.keys[key1.Key].CreatedAt = time.Now().Add(-2 * time.Hour)

	// Generate a fresh key
	key2, _ := kg.Generate("child-2", 200)

	// Cleanup keys older than 1 hour
	kg.Cleanup(1 * time.Hour)

	// key1 should be removed, key2 should remain
	if _, ok := kg.keys[key1.Key]; ok {
		t.Error("Old key should be cleaned up")
	}
	if _, ok := kg.keys[key2.Key]; !ok {
		t.Error("Fresh key should remain")
	}
}

func TestKeyGeneratorCleanupNoExpired(t *testing.T) {
	kg := NewKeyGenerator()

	key, _ := kg.Generate("child-1", 100)

	// Cleanup with very short max age (should not remove fresh keys)
	kg.Cleanup(1 * time.Hour)

	if _, ok := kg.keys[key.Key]; !ok {
		t.Error("Fresh key should not be cleaned up")
	}
}
