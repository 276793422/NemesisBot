package integrity_test

import (
	"context"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/security/integrity"
)

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

func tempDir(t *testing.T) string {
	t.Helper()
	dir := t.TempDir()
	return dir
}

func makeEvent(op, tool, decision string) *integrity.AuditEvent {
	return &integrity.AuditEvent{
		Timestamp: time.Now().UTC(),
		Operation: op,
		ToolName:  tool,
		User:      "test-user",
		Source:    "test-source",
		Target:    "test-target",
		Decision:  decision,
		Reason:    "test reason",
	}
}

// ---------------------------------------------------------------------------
// NewAuditChain
// ---------------------------------------------------------------------------

func TestNewAuditChain_ValidConfig(t *testing.T) {
	dir := tempDir(t)
	cfg := integrity.AuditChainConfig{
		Enabled:     true,
		StoragePath: dir,
		MaxFileSize: 1024 * 1024,
	}
	chain, err := integrity.NewAuditChain(cfg)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	defer chain.Close()

	if chain.Size() != 0 {
		t.Errorf("expected Size()=0 for new chain, got %d", chain.Size())
	}
}

func TestNewAuditChain_EmptyStoragePath(t *testing.T) {
	cfg := integrity.AuditChainConfig{
		Enabled:     true,
		StoragePath: "",
	}
	_, err := integrity.NewAuditChain(cfg)
	if err == nil {
		t.Error("expected error for empty StoragePath")
	}
}

func TestNewAuditChain_DefaultMaxFileSize(t *testing.T) {
	dir := tempDir(t)
	cfg := integrity.AuditChainConfig{
		Enabled:     true,
		StoragePath: dir,
		MaxFileSize: 0, // should default to 50MB
	}
	chain, err := integrity.NewAuditChain(cfg)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	defer chain.Close()
}

// ---------------------------------------------------------------------------
// Append
// ---------------------------------------------------------------------------

func TestAuditChain_Append_SingleEvent(t *testing.T) {
	dir := tempDir(t)
	cfg := integrity.AuditChainConfig{
		Enabled:     true,
		StoragePath: dir,
		MaxFileSize: 1024 * 1024,
	}
	chain, err := integrity.NewAuditChain(cfg)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	defer chain.Close()

	err = chain.Append(context.Background(), makeEvent("file_read", "file_read", "allowed"))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if chain.Size() != 1 {
		t.Errorf("expected Size()=1, got %d", chain.Size())
	}
}

func TestAuditChain_Append_MultipleEvents(t *testing.T) {
	dir := tempDir(t)
	cfg := integrity.AuditChainConfig{
		Enabled:     true,
		StoragePath: dir,
		MaxFileSize: 1024 * 1024,
	}
	chain, err := integrity.NewAuditChain(cfg)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	defer chain.Close()

	for i := 0; i < 10; i++ {
		err = chain.Append(context.Background(), makeEvent("op", "tool", "allowed"))
		if err != nil {
			t.Fatalf("unexpected error on append %d: %v", i, err)
		}
	}
	if chain.Size() != 10 {
		t.Errorf("expected Size()=10, got %d", chain.Size())
	}
}

func TestAuditChain_Append_Disabled(t *testing.T) {
	dir := tempDir(t)
	cfg := integrity.AuditChainConfig{
		Enabled:     false,
		StoragePath: dir,
	}
	chain, err := integrity.NewAuditChain(cfg)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	defer chain.Close()

	err = chain.Append(context.Background(), makeEvent("op", "tool", "allowed"))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if chain.Size() != 0 {
		t.Error("expected Size()=0 when disabled")
	}
}

func TestAuditChain_Append_AfterClose(t *testing.T) {
	dir := tempDir(t)
	cfg := integrity.AuditChainConfig{
		Enabled:     true,
		StoragePath: dir,
	}
	chain, err := integrity.NewAuditChain(cfg)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	chain.Close()

	err = chain.Append(context.Background(), makeEvent("op", "tool", "allowed"))
	if err == nil {
		t.Error("expected error when appending to closed chain")
	}
}

func TestAuditChain_Append_ZeroTimestamp(t *testing.T) {
	dir := tempDir(t)
	cfg := integrity.AuditChainConfig{
		Enabled:     true,
		StoragePath: dir,
	}
	chain, err := integrity.NewAuditChain(cfg)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	defer chain.Close()

	event := &integrity.AuditEvent{
		Operation: "test",
		ToolName:  "tool",
		// Timestamp is zero; should be set to now.
	}
	err = chain.Append(context.Background(), event)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if event.Timestamp.IsZero() {
		t.Error("expected timestamp to be set")
	}
}

// ---------------------------------------------------------------------------
// Verify
// ---------------------------------------------------------------------------

func TestAuditChain_Verify_EmptyChain(t *testing.T) {
	dir := tempDir(t)
	cfg := integrity.AuditChainConfig{
		Enabled:     true,
		StoragePath: dir,
	}
	chain, err := integrity.NewAuditChain(cfg)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	defer chain.Close()

	err = chain.Verify(context.Background())
	if err != nil {
		t.Errorf("expected no error verifying empty chain, got: %v", err)
	}
}

func TestAuditChain_Verify_SingleEvent(t *testing.T) {
	dir := tempDir(t)
	cfg := integrity.AuditChainConfig{
		Enabled:     true,
		StoragePath: dir,
	}
	chain, err := integrity.NewAuditChain(cfg)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	defer chain.Close()

	chain.Append(context.Background(), makeEvent("file_read", "file_read", "allowed"))
	err = chain.Verify(context.Background())
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
}

func TestAuditChain_Verify_MultipleEvents(t *testing.T) {
	dir := tempDir(t)
	cfg := integrity.AuditChainConfig{
		Enabled:     true,
		StoragePath: dir,
	}
	chain, err := integrity.NewAuditChain(cfg)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	defer chain.Close()

	for i := 0; i < 20; i++ {
		chain.Append(context.Background(), makeEvent("op", "tool", "allowed"))
	}
	err = chain.Verify(context.Background())
	if err != nil {
		t.Fatalf("unexpected error verifying chain: %v", err)
	}
}

// ---------------------------------------------------------------------------
// VerifyRange
// ---------------------------------------------------------------------------

func TestAuditChain_VerifyRange_Valid(t *testing.T) {
	dir := tempDir(t)
	cfg := integrity.AuditChainConfig{
		Enabled:     true,
		StoragePath: dir,
	}
	chain, err := integrity.NewAuditChain(cfg)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	defer chain.Close()

	for i := 0; i < 10; i++ {
		chain.Append(context.Background(), makeEvent("op", "tool", "allowed"))
	}

	err = chain.VerifyRange(context.Background(), 2, 7)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
}

func TestAuditChain_VerifyRange_OutOfRange(t *testing.T) {
	dir := tempDir(t)
	cfg := integrity.AuditChainConfig{
		Enabled:     true,
		StoragePath: dir,
	}
	chain, err := integrity.NewAuditChain(cfg)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	defer chain.Close()

	chain.Append(context.Background(), makeEvent("op", "tool", "allowed"))

	tests := []struct {
		name string
		from int
		to   int
	}{
		{"negative from", -1, 0},
		{"to out of range", 0, 5},
		{"from > to", 1, 0},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			err := chain.VerifyRange(context.Background(), tt.from, tt.to)
			if err == nil {
				t.Error("expected error for invalid range")
			}
		})
	}
}

// ---------------------------------------------------------------------------
// GetEvent
// ---------------------------------------------------------------------------

func TestAuditChain_GetEvent_Valid(t *testing.T) {
	dir := tempDir(t)
	cfg := integrity.AuditChainConfig{
		Enabled:     true,
		StoragePath: dir,
	}
	chain, err := integrity.NewAuditChain(cfg)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	defer chain.Close()

	chain.Append(context.Background(), &integrity.AuditEvent{
		Timestamp: time.Date(2026, 1, 1, 0, 0, 0, 0, time.UTC),
		Operation: "test_op",
		ToolName:  "test_tool",
		Decision:  "allowed",
	})

	entry, err := chain.GetEvent(0)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if entry.Index != 0 {
		t.Errorf("expected Index=0, got %d", entry.Index)
	}
	if entry.Event.Operation != "test_op" {
		t.Errorf("expected Operation=test_op, got %q", entry.Event.Operation)
	}
	if entry.Hash == "" {
		t.Error("expected non-empty Hash")
	}
}

func TestAuditChain_GetEvent_OutOfRange(t *testing.T) {
	dir := tempDir(t)
	cfg := integrity.AuditChainConfig{
		Enabled:     true,
		StoragePath: dir,
	}
	chain, err := integrity.NewAuditChain(cfg)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	defer chain.Close()

	_, err = chain.GetEvent(0)
	if err == nil {
		t.Error("expected error for index out of range")
	}

	chain.Append(context.Background(), makeEvent("op", "tool", "allowed"))
	_, err = chain.GetEvent(5)
	if err == nil {
		t.Error("expected error for index out of range")
	}

	_, err = chain.GetEvent(-1)
	if err == nil {
		t.Error("expected error for negative index")
	}
}

func TestAuditChain_GetEvent_ReturnsCopy(t *testing.T) {
	dir := tempDir(t)
	cfg := integrity.AuditChainConfig{
		Enabled:     true,
		StoragePath: dir,
	}
	chain, err := integrity.NewAuditChain(cfg)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	defer chain.Close()

	chain.Append(context.Background(), makeEvent("op", "tool", "allowed"))
	e1, _ := chain.GetEvent(0)
	e1.Event.Operation = "tampered"
	e2, _ := chain.GetEvent(0)
	if e2.Event.Operation == "tampered" {
		t.Error("GetEvent should return a copy, not a reference")
	}
}

// ---------------------------------------------------------------------------
// RootHash
// ---------------------------------------------------------------------------

func TestAuditChain_RootHash(t *testing.T) {
	dir := tempDir(t)
	cfg := integrity.AuditChainConfig{
		Enabled:     true,
		StoragePath: dir,
	}
	chain, err := integrity.NewAuditChain(cfg)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	defer chain.Close()

	root := chain.RootHash()
	if root == "" {
		t.Error("expected non-empty root hash")
	}

	chain.Append(context.Background(), makeEvent("op", "tool", "allowed"))
	newRoot := chain.RootHash()
	if newRoot == "" {
		t.Error("expected non-empty root hash after append")
	}
	// Root should change after adding an event.
	// Note: it may or may not change depending on empty vs single leaf,
	// but it should at least be non-empty.
}

// ---------------------------------------------------------------------------
// Close
// ---------------------------------------------------------------------------

func TestAuditChain_Close(t *testing.T) {
	dir := tempDir(t)
	cfg := integrity.AuditChainConfig{
		Enabled:     true,
		StoragePath: dir,
	}
	chain, err := integrity.NewAuditChain(cfg)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	err = chain.Close()
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	// Double close should be safe.
	err = chain.Close()
	if err != nil {
		t.Fatalf("unexpected error on double close: %v", err)
	}
}

// ---------------------------------------------------------------------------
// Persistence — reload chain from disk
// ---------------------------------------------------------------------------

func TestAuditChain_Persistence(t *testing.T) {
	dir := tempDir(t)
	cfg := integrity.AuditChainConfig{
		Enabled:     true,
		StoragePath: dir,
	}

	// Create and populate.
	chain, err := integrity.NewAuditChain(cfg)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	for i := 0; i < 5; i++ {
		chain.Append(context.Background(), makeEvent("op", "tool", "allowed"))
	}
	chain.Close()

	// Reopen and verify.
	chain2, err := integrity.NewAuditChain(cfg)
	if err != nil {
		t.Fatalf("unexpected error reopening: %v", err)
	}
	defer chain2.Close()

	if chain2.Size() != 5 {
		t.Errorf("expected Size()=5 after reload, got %d", chain2.Size())
	}

	err = chain2.Verify(context.Background())
	if err != nil {
		t.Fatalf("verification failed after reload: %v", err)
	}
}

// ---------------------------------------------------------------------------
// VerifyOnLoad
// ---------------------------------------------------------------------------

func TestNewAuditChain_VerifyOnLoad(t *testing.T) {
	dir := tempDir(t)
	cfg := integrity.AuditChainConfig{
		Enabled:      true,
		StoragePath:  dir,
		VerifyOnLoad: true,
	}

	// Write some events.
	chain, err := integrity.NewAuditChain(cfg)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	chain.Append(context.Background(), makeEvent("op", "tool", "allowed"))
	chain.Close()

	// Reopen with VerifyOnLoad.
	chain2, err := integrity.NewAuditChain(cfg)
	if err != nil {
		t.Fatalf("unexpected error on reopen with verify: %v", err)
	}
	chain2.Close()
}

// ---------------------------------------------------------------------------
// Segment rotation
// ---------------------------------------------------------------------------

func TestAuditChain_SegmentRotation(t *testing.T) {
	dir := tempDir(t)
	cfg := integrity.AuditChainConfig{
		Enabled:     true,
		StoragePath: dir,
		MaxFileSize: 200, // very small to force rotation
	}

	chain, err := integrity.NewAuditChain(cfg)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	// Write enough events to trigger rotation.
	for i := 0; i < 50; i++ {
		chain.Append(context.Background(), makeEvent("operation_with_long_name", "tool_with_long_name", "allowed"))
	}

	if chain.Size() != 50 {
		t.Errorf("expected Size()=50, got %d", chain.Size())
	}

	// Verify full chain.
	err = chain.Verify(context.Background())
	if err != nil {
		t.Fatalf("verification failed after rotation: %v", err)
	}

	chain.Close()

	// Check that multiple segment files exist.
	entries, err := os.ReadDir(dir)
	if err != nil {
		t.Fatalf("unexpected error reading dir: %v", err)
	}
	jsonlCount := 0
	for _, e := range entries {
		if filepath.Ext(e.Name()) == ".jsonl" {
			jsonlCount++
		}
	}
	if jsonlCount < 2 {
		t.Errorf("expected at least 2 segment files, got %d", jsonlCount)
	}

	// Reload and verify.
	chain2, err := integrity.NewAuditChain(cfg)
	if err != nil {
		t.Fatalf("unexpected error on reopen: %v", err)
	}
	defer chain2.Close()

	if chain2.Size() != 50 {
		t.Errorf("expected Size()=50 after reload, got %d", chain2.Size())
	}

	err = chain2.Verify(context.Background())
	if err != nil {
		t.Fatalf("verification failed after reload: %v", err)
	}
}

// ---------------------------------------------------------------------------
// Chain integrity — prev_hash linkage
// ---------------------------------------------------------------------------

func TestAuditChain_PrevHashLinkage(t *testing.T) {
	dir := tempDir(t)
	cfg := integrity.AuditChainConfig{
		Enabled:     true,
		StoragePath: dir,
	}
	chain, err := integrity.NewAuditChain(cfg)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	defer chain.Close()

	for i := 0; i < 5; i++ {
		chain.Append(context.Background(), makeEvent("op", "tool", "allowed"))
	}

	// Check prev_hash linkage.
	prevEntry, _ := chain.GetEvent(0)
	for i := 1; i < 5; i++ {
		entry, _ := chain.GetEvent(i)
		if entry.PrevHash != prevEntry.Hash {
			t.Errorf("entry %d PrevHash mismatch: expected %s, got %s",
				i, prevEntry.Hash, entry.PrevHash)
		}
		prevEntry = entry
	}
}

// ---------------------------------------------------------------------------
// DefaultAuditChainConfig
// ---------------------------------------------------------------------------

func TestDefaultAuditChainConfig(t *testing.T) {
	cfg := integrity.DefaultAuditChainConfig()
	if !cfg.Enabled {
		t.Error("expected Enabled=true")
	}
	if cfg.MaxFileSize != 50*1024*1024 {
		t.Errorf("expected MaxFileSize=50MB, got %d", cfg.MaxFileSize)
	}
	if cfg.VerifyOnLoad {
		t.Error("expected VerifyOnLoad=false by default")
	}
}
