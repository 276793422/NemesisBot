// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package security_test

import (
	"context"
	"os"
	"path/filepath"
	"testing"

	integrity "github.com/276793422/NemesisBot/module/security/integrity"
)

func TestMerkleTree_AddLeaf(t *testing.T) {
	tree := integrity.NewMerkleTree()

	// Empty tree should have a default root hash
	initialRoot := tree.RootHash()
	if initialRoot == "" {
		t.Error("expected non-empty root hash for empty tree")
	}
	if tree.Size() != 0 {
		t.Errorf("expected size 0, got %d", tree.Size())
	}

	// Adding a leaf should change the root hash
	leaf1 := tree.AddLeaf([]byte("hello"))
	if leaf1 == "" {
		t.Error("expected non-empty leaf hash")
	}
	if tree.RootHash() == initialRoot {
		t.Error("expected root hash to change after adding a leaf")
	}
	if tree.Size() != 1 {
		t.Errorf("expected size 1, got %d", tree.Size())
	}

	// Adding another leaf should change the root again
	prevRoot := tree.RootHash()
	leaf2 := tree.AddLeaf([]byte("world"))
	if leaf2 == prevRoot {
		t.Error("expected leaf hash to differ from previous root")
	}
	if tree.RootHash() == prevRoot {
		t.Error("expected root hash to change after adding second leaf")
	}
	if tree.Size() != 2 {
		t.Errorf("expected size 2, got %d", tree.Size())
	}

	// Leaves should be different
	if leaf1 == leaf2 {
		t.Error("expected different hashes for different data")
	}
}

func TestMerkleTree_ProofAndVerify(t *testing.T) {
	tree := integrity.NewMerkleTree()

	data := []byte("verify me")
	tree.AddLeaf([]byte("other data 1"))
	tree.AddLeaf(data)
	tree.AddLeaf([]byte("other data 2"))

	proof, err := tree.Proof(1) // index of "verify me"
	if err != nil {
		t.Fatalf("Proof returned error: %v", err)
	}

	// Verify the proof against the tree's root hash
	if !tree.Verify(data, proof) {
		t.Error("expected proof to verify successfully")
	}

	// Wrong data should fail verification
	if tree.Verify([]byte("wrong data"), proof) {
		t.Error("expected verification to fail for wrong data")
	}
}

func TestMerkleTree_MultipleLeaves(t *testing.T) {
	tree := integrity.NewMerkleTree()

	data := [][]byte{
		[]byte("leaf0"),
		[]byte("leaf1"),
		[]byte("leaf2"),
		[]byte("leaf3"),
		[]byte("leaf4"),
	}

	for _, d := range data {
		tree.AddLeaf(d)
	}

	if tree.Size() != 5 {
		t.Fatalf("expected size 5, got %d", tree.Size())
	}

	// Generate and verify proofs for all leaves
	for i, d := range data {
		proof, err := tree.Proof(i)
		if err != nil {
			t.Errorf("Proof(%d) returned error: %v", i, err)
			continue
		}
		if !tree.Verify(d, proof) {
			t.Errorf("proof for leaf %d failed verification", i)
		}
	}
}

func TestMerkleTree_SingleLeaf(t *testing.T) {
	tree := integrity.NewMerkleTree()

	data := []byte("only leaf")
	hash := tree.AddLeaf(data)
	if hash == "" {
		t.Error("expected non-empty leaf hash")
	}

	// Root should equal the single leaf hash
	if tree.RootHash() != hash {
		t.Error("expected root hash to equal single leaf hash")
	}

	// Proof for a single leaf should be empty
	proof, err := tree.Proof(0)
	if err != nil {
		t.Fatalf("Proof returned error: %v", err)
	}
	if len(proof) != 0 {
		t.Errorf("expected empty proof for single leaf, got %d steps", len(proof))
	}

	// Verification should still work
	if !tree.Verify(data, proof) {
		t.Error("expected single leaf to verify")
	}
}

func TestMerkleTree_Empty(t *testing.T) {
	tree := integrity.NewMerkleTree()

	if tree.Size() != 0 {
		t.Errorf("expected size 0, got %d", tree.Size())
	}
	if tree.RootHash() == "" {
		t.Error("expected non-empty root hash for empty tree (SHA256 of empty)")
	}

	// Proof should fail for empty tree
	_, err := tree.Proof(0)
	if err == nil {
		t.Error("expected error for proof on empty tree")
	}
	if err != integrity.ErrIndexOutOfRange {
		t.Errorf("expected ErrIndexOutOfRange, got %v", err)
	}
}

func TestMerkleTree_ProofOutOfRange(t *testing.T) {
	tree := integrity.NewMerkleTree()
	tree.AddLeaf([]byte("data"))

	_, err := tree.Proof(-1)
	if err == nil {
		t.Error("expected error for negative index")
	}

	_, err = tree.Proof(5)
	if err == nil {
		t.Error("expected error for index beyond range")
	}
}

func TestMerkleTree_Leaves(t *testing.T) {
	tree := integrity.NewMerkleTree()
	tree.AddLeaf([]byte("a"))
	tree.AddLeaf([]byte("b"))

	leaves := tree.Leaves()
	if len(leaves) != 2 {
		t.Fatalf("expected 2 leaves, got %d", len(leaves))
	}

	// Verify it's a copy (modifying should not affect the tree)
	leaves[0] = "modified"
	originalLeaves := tree.Leaves()
	if originalLeaves[0] == "modified" {
		t.Error("Leaves() should return a copy")
	}
}

func TestMerkleTree_VerifyFromHash(t *testing.T) {
	tree := integrity.NewMerkleTree()
	tree.AddLeaf([]byte("first"))
	tree.AddLeaf([]byte("second"))

	proof, _ := tree.Proof(0)
	leafHash := tree.Leaves()[0]
	root := tree.RootHash()

	if !integrity.VerifyFromHash(leafHash, proof, root) {
		t.Error("expected VerifyFromHash to succeed")
	}

	// Wrong root should fail
	if integrity.VerifyFromHash(leafHash, proof, "wrong_root") {
		t.Error("expected VerifyFromHash to fail with wrong root")
	}
}

func TestAuditChain_Append(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := integrity.AuditChainConfig{
		Enabled:     true,
		StoragePath: tmpDir,
	}

	chain, err := integrity.NewAuditChain(cfg)
	if err != nil {
		t.Fatalf("NewAuditChain returned error: %v", err)
	}
	t.Cleanup(func() { chain.Close() })

	ctx := context.Background()
	event := &integrity.AuditEvent{
		Operation: "file_read",
		ToolName:  "file_reader",
		User:      "testuser",
		Source:    "test",
		Target:    "/etc/passwd",
		Decision:  "allowed",
		Reason:    "within workspace",
	}

	err = chain.Append(ctx, event)
	if err != nil {
		t.Fatalf("Append returned error: %v", err)
	}

	if chain.Size() != 1 {
		t.Errorf("expected size 1, got %d", chain.Size())
	}
}

func TestAuditChain_Verify(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := integrity.AuditChainConfig{
		Enabled:     true,
		StoragePath: tmpDir,
	}

	chain, err := integrity.NewAuditChain(cfg)
	if err != nil {
		t.Fatalf("NewAuditChain returned error: %v", err)
	}
	t.Cleanup(func() { chain.Close() })

	ctx := context.Background()

	// Append several events
	for i := 0; i < 5; i++ {
		event := &integrity.AuditEvent{
			Operation: "test_op",
			ToolName:  "test_tool",
			User:      "tester",
			Decision:  "allowed",
		}
		if err := chain.Append(ctx, event); err != nil {
			t.Fatalf("Append %d returned error: %v", i, err)
		}
	}

	// Verify the chain
	if err := chain.Verify(ctx); err != nil {
		t.Errorf("Verify returned error: %v", err)
	}
}

func TestAuditChain_GetEvent(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := integrity.AuditChainConfig{
		Enabled:     true,
		StoragePath: tmpDir,
	}

	chain, err := integrity.NewAuditChain(cfg)
	if err != nil {
		t.Fatalf("NewAuditChain returned error: %v", err)
	}
	t.Cleanup(func() { chain.Close() })

	ctx := context.Background()

	event := &integrity.AuditEvent{
		Operation: "file_write",
		ToolName:  "writer",
		User:      "admin",
		Decision:  "denied",
		Reason:    "security policy violation",
	}
	if err := chain.Append(ctx, event); err != nil {
		t.Fatalf("Append returned error: %v", err)
	}

	// Retrieve the event
	entry, err := chain.GetEvent(0)
	if err != nil {
		t.Fatalf("GetEvent returned error: %v", err)
	}
	if entry.Event.Operation != "file_write" {
		t.Errorf("expected operation 'file_write', got %q", entry.Event.Operation)
	}
	if entry.Event.Decision != "denied" {
		t.Errorf("expected decision 'denied', got %q", entry.Event.Decision)
	}
	if entry.Index != 0 {
		t.Errorf("expected index 0, got %d", entry.Index)
	}

	// Out of range
	_, err = chain.GetEvent(99)
	if err == nil {
		t.Error("expected error for out-of-range index")
	}
	if err != integrity.ErrIndexOutOfRange {
		t.Errorf("expected ErrIndexOutOfRange, got %v", err)
	}

	// Negative index
	_, err = chain.GetEvent(-1)
	if err == nil {
		t.Error("expected error for negative index")
	}
}

func TestAuditChain_CloseAndReopen(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := integrity.AuditChainConfig{
		Enabled:     true,
		StoragePath: tmpDir,
	}

	// Create and populate
	chain, err := integrity.NewAuditChain(cfg)
	if err != nil {
		t.Fatalf("NewAuditChain returned error: %v", err)
	}
	ctx := context.Background()

	for i := 0; i < 3; i++ {
		event := &integrity.AuditEvent{
			Operation: "persist_test",
			ToolName:  "tool",
			User:      "user",
			Decision:  "allowed",
		}
		if err := chain.Append(ctx, event); err != nil {
			t.Fatalf("Append returned error: %v", err)
		}
	}

	// Close
	if err := chain.Close(); err != nil {
		t.Fatalf("Close returned error: %v", err)
	}

	// Reopen
	chain2, err := integrity.NewAuditChain(cfg)
	if err != nil {
		t.Fatalf("NewAuditChain (reopen) returned error: %v", err)
	}
	t.Cleanup(func() { chain2.Close() })

	if chain2.Size() != 3 {
		t.Errorf("expected size 3 after reopen, got %d", chain2.Size())
	}

	// Verify integrity of reopened chain
	if err := chain2.Verify(ctx); err != nil {
		t.Errorf("Verify after reopen returned error: %v", err)
	}

	// Verify we can still append
	event := &integrity.AuditEvent{
		Operation: "post_reopen",
		ToolName:  "tool",
		User:      "user",
		Decision:  "allowed",
	}
	if err := chain2.Append(ctx, event); err != nil {
		t.Fatalf("Append after reopen returned error: %v", err)
	}
	if chain2.Size() != 4 {
		t.Errorf("expected size 4 after post-reopen append, got %d", chain2.Size())
	}
}

func TestAuditChain_VerifyEmptyChain(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := integrity.AuditChainConfig{
		Enabled:     true,
		StoragePath: tmpDir,
	}

	chain, err := integrity.NewAuditChain(cfg)
	if err != nil {
		t.Fatalf("NewAuditChain returned error: %v", err)
	}
	t.Cleanup(func() { chain.Close() })

	ctx := context.Background()
	if err := chain.Verify(ctx); err != nil {
		t.Errorf("Verify on empty chain should not error, got: %v", err)
	}
}

func TestAuditChain_Disabled(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := integrity.AuditChainConfig{
		Enabled:     false,
		StoragePath: tmpDir,
	}

	chain, err := integrity.NewAuditChain(cfg)
	if err != nil {
		t.Fatalf("NewAuditChain returned error: %v", err)
	}
	t.Cleanup(func() { chain.Close() })

	ctx := context.Background()
	event := &integrity.AuditEvent{
		Operation: "test",
		Decision:  "allowed",
	}
	if err := chain.Append(ctx, event); err != nil {
		t.Fatalf("Append returned error: %v", err)
	}
	// When disabled, entries should not be stored
	if chain.Size() != 0 {
		t.Errorf("expected size 0 when disabled, got %d", chain.Size())
	}
}

func TestAuditChain_RootHash(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := integrity.AuditChainConfig{
		Enabled:     true,
		StoragePath: tmpDir,
	}

	chain, err := integrity.NewAuditChain(cfg)
	if err != nil {
		t.Fatalf("NewAuditChain returned error: %v", err)
	}
	t.Cleanup(func() { chain.Close() })

	ctx := context.Background()
	rootBefore := chain.RootHash()

	event := &integrity.AuditEvent{Operation: "test", Decision: "allowed"}
	chain.Append(ctx, event)

	rootAfter := chain.RootHash()
	if rootAfter == rootBefore {
		t.Error("expected root hash to change after appending an event")
	}
}

func TestAuditChain_NoStoragePath(t *testing.T) {
	cfg := integrity.AuditChainConfig{
		Enabled:     true,
		StoragePath: "",
	}
	_, err := integrity.NewAuditChain(cfg)
	if err == nil {
		t.Error("expected error for empty StoragePath")
	}
}

func TestAuditChain_VerifyOnLoad(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := integrity.AuditChainConfig{
		Enabled:      true,
		StoragePath:  tmpDir,
		VerifyOnLoad: true,
	}

	// Create and populate
	chain, _ := integrity.NewAuditChain(cfg)
	ctx := context.Background()
	chain.Append(ctx, &integrity.AuditEvent{Operation: "test", Decision: "allowed"})
	chain.Close()

	// Reopen with VerifyOnLoad
	chain2, err := integrity.NewAuditChain(cfg)
	if err != nil {
		t.Fatalf("NewAuditChain with VerifyOnLoad returned error: %v", err)
	}
	chain2.Close()
}

func TestAuditChain_DoubleClose(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := integrity.AuditChainConfig{
		Enabled:     true,
		StoragePath: tmpDir,
	}

	chain, _ := integrity.NewAuditChain(cfg)
	if err := chain.Close(); err != nil {
		t.Fatalf("first Close returned error: %v", err)
	}
	if err := chain.Close(); err != nil {
		t.Fatalf("second Close returned error: %v", err)
	}
}

func TestAuditChain_AppendAfterClose(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := integrity.AuditChainConfig{
		Enabled:     true,
		StoragePath: tmpDir,
	}

	chain, _ := integrity.NewAuditChain(cfg)
	chain.Close()

	ctx := context.Background()
	err := chain.Append(ctx, &integrity.AuditEvent{Operation: "test", Decision: "allowed"})
	if err == nil {
		t.Error("expected error when appending to closed chain")
	}
	if err != integrity.ErrChainClosed {
		t.Errorf("expected ErrChainClosed, got %v", err)
	}
}

func TestAuditChain_VerifyRange(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := integrity.AuditChainConfig{
		Enabled:     true,
		StoragePath: tmpDir,
	}

	chain, _ := integrity.NewAuditChain(cfg)
	t.Cleanup(func() { chain.Close() })
	ctx := context.Background()

	for i := 0; i < 5; i++ {
		chain.Append(ctx, &integrity.AuditEvent{
			Operation: "test",
			Decision:  "allowed",
		})
	}

	// Verify a range
	err := chain.VerifyRange(ctx, 1, 3)
	if err != nil {
		t.Errorf("VerifyRange(1, 3) returned error: %v", err)
	}

	// Out of range
	err = chain.VerifyRange(ctx, -1, 3)
	if err == nil {
		t.Error("expected error for out-of-range VerifyRange")
	}

	err = chain.VerifyRange(ctx, 3, 1)
	if err == nil {
		t.Error("expected error for invalid range (from > to)")
	}
}

func TestAuditChain_SegmentFiles(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := integrity.AuditChainConfig{
		Enabled:     true,
		StoragePath: tmpDir,
	}

	chain, _ := integrity.NewAuditChain(cfg)
	ctx := context.Background()

	chain.Append(ctx, &integrity.AuditEvent{Operation: "test", Decision: "allowed"})
	chain.Close()

	// Verify that segment files were created
	files, err := filepath.Glob(filepath.Join(tmpDir, "*.jsonl"))
	if err != nil {
		t.Fatalf("failed to list segment files: %v", err)
	}
	if len(files) == 0 {
		t.Error("expected at least one segment file to be created")
	}

	// Verify the file is readable
	for _, f := range files {
		data, err := os.ReadFile(f)
		if err != nil {
			t.Errorf("failed to read segment file %s: %v", f, err)
		}
		if len(data) == 0 {
			t.Errorf("segment file %s is empty", f)
		}
	}
}
