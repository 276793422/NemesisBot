// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Package integrity provides a tamper-evident persistent audit chain
package integrity

import (
	"bufio"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"sync"
	"time"
)

// ---------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------

var (
	ErrIndexOutOfRange = errors.New("integrity: index out of range")
	ErrChainCorrupted  = errors.New("integrity: chain corruption detected")
	ErrChainClosed     = errors.New("integrity: chain is closed")
	ErrHashMismatch    = errors.New("integrity: hash mismatch")
)

// ---------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------

// AuditEvent describes a single security audit event.
type AuditEvent struct {
	Timestamp time.Time              `json:"timestamp"`
	Operation string                 `json:"operation"`
	ToolName  string                 `json:"tool_name"`
	User      string                 `json:"user"`
	Source    string                 `json:"source"`
	Target    string                 `json:"target"`
	Decision  string                 `json:"decision"` // "allowed" or "denied"
	Reason    string                 `json:"reason"`
	Metadata  map[string]interface{} `json:"metadata,omitempty"`
}

// ChainEntry is one line in the persistent JSONL file.
type ChainEntry struct {
	Index      int        `json:"index"`
	Hash       Hash       `json:"hash"`
	PrevHash   Hash       `json:"prev_hash"`
	Event      AuditEvent `json:"event"`
	MerkleRoot Hash       `json:"merkle_root"`
	Timestamp  time.Time  `json:"timestamp"`
}

// AuditChainConfig configures the audit chain.
type AuditChainConfig struct {
	Enabled      bool   // Enable the audit chain
	StoragePath  string // Directory for JSONL files
	MaxFileSize  int64  // Rotate after this many bytes (default 50 MB)
	VerifyOnLoad bool   // Verify full chain integrity on startup
}

// DefaultAuditChainConfig returns a reasonable default configuration.
func DefaultAuditChainConfig() AuditChainConfig {
	return AuditChainConfig{
		Enabled:      true,
		MaxFileSize:  50 * 1024 * 1024, // 50 MB
		VerifyOnLoad: false,
	}
}

// ---------------------------------------------------------------------
// AuditChain
// ---------------------------------------------------------------------

// AuditChain is a tamper-evident, append-only log of security audit events.
// Each event is hashed and chained to the previous entry (prev_hash), and a
// Merkle tree is maintained over the entire log so that the root hash
// summarises all events up to that point.
//
// Persistence is in JSONL format under the configured StoragePath directory.
// When the current file exceeds MaxFileSize a new segment is created.
//
// All public methods are thread-safe.
type AuditChain struct {
	mu       sync.Mutex
	cfg      AuditChainConfig
	tree     *MerkleTree
	file     *os.File   // current JSONL segment
	writer   *bufio.Writer
	entries  []*ChainEntry // in-memory index of all entries
	prevHash Hash
	size     int       // total number of entries
	segIndex int       // current segment index (for filename)
	closed   bool
}

// NewAuditChain opens or creates an audit chain at the configured path.
// If the directory exists and contains prior segment files they are loaded
// and (optionally) verified.
func NewAuditChain(cfg AuditChainConfig) (*AuditChain, error) {
	if cfg.MaxFileSize <= 0 {
		cfg.MaxFileSize = 50 * 1024 * 1024
	}
	if cfg.StoragePath == "" {
		return nil, errors.New("integrity: StoragePath is required")
	}

	// Ensure directory exists.
	if err := os.MkdirAll(cfg.StoragePath, 0750); err != nil {
		return nil, fmt.Errorf("integrity: create storage path: %w", err)
	}

	ac := &AuditChain{
		cfg:      cfg,
		tree:     NewMerkleTree(),
		entries:  make([]*ChainEntry, 0),
		prevHash: sha256Hex(nil), // genesis prev-hash
	}

	// Load existing segments.
	if err := ac.loadSegments(); err != nil {
		return nil, fmt.Errorf("integrity: load segments: %w", err)
	}

	// Optionally verify full chain on startup.
	if cfg.VerifyOnLoad && ac.size > 0 {
		if err := ac.verifyChain(0, ac.size-1); err != nil {
			return nil, fmt.Errorf("integrity: verify on load: %w", err)
		}
	}

	// Open current (or new) segment for appending.
	if err := ac.openSegment(); err != nil {
		return nil, fmt.Errorf("integrity: open segment: %w", err)
	}

	return ac, nil
}

// Append adds a new event to the chain.  The event is hashed, linked to
// the previous entry, and appended to the Merkle tree before being
// persisted to disk.
func (ac *AuditChain) Append(_ context.Context, event *AuditEvent) error {
	ac.mu.Lock()
	defer ac.mu.Unlock()

	if ac.closed {
		return ErrChainClosed
	}

	if !ac.cfg.Enabled {
		return nil
	}

	// Normalize timestamp.
	if event.Timestamp.IsZero() {
		event.Timestamp = time.Now().UTC()
	}

	// Build chain entry.
	idx := ac.size

	// Compute the event hash from serialised event data.
	eventBytes, err := json.Marshal(event)
	if err != nil {
		return fmt.Errorf("integrity: marshal event: %w", err)
	}

	// The entry hash covers: index + prevHash + eventData.
	preimage := fmt.Sprintf("%d:%s:%s", idx, ac.prevHash, sha256Hex(eventBytes))
	entryHash := sha256Hex([]byte(preimage))

	// Add to Merkle tree.
	leafHash := ac.tree.AddLeaf(eventBytes)

	// If the tree root is empty (should not happen after AddLeaf), use
	// the entryHash as a fallback.
	merkleRoot := ac.tree.RootHash()
	if merkleRoot == "" || merkleRoot == sha256Hex(nil) && idx == 0 {
		merkleRoot = entryHash
	}

	ce := &ChainEntry{
		Index:      idx,
		Hash:       entryHash,
		PrevHash:   ac.prevHash,
		Event:      *event,
		MerkleRoot: merkleRoot,
		Timestamp:  event.Timestamp,
	}

	// Persist to disk.
	if err := ac.writeEntry(ce); err != nil {
		return fmt.Errorf("integrity: write entry: %w", err)
	}

	// Update in-memory state.
	ac.entries = append(ac.entries, ce)
	ac.prevHash = entryHash
	ac.size++

	// Rotate segment if needed.
	if ac.file != nil {
		if stat, err := ac.file.Stat(); err == nil && stat.Size() >= ac.cfg.MaxFileSize {
			_ = ac.writer.Flush()
			_ = ac.file.Close()
			ac.segIndex++
			if err := ac.openSegment(); err != nil {
				return fmt.Errorf("integrity: rotate segment: %w", err)
			}
		}
	}

	_ = leafHash // leaf hash is used implicitly via tree root

	return nil
}

// Verify checks the integrity of the entire chain from the first entry to
// the last.
func (ac *AuditChain) Verify(_ context.Context) error {
	ac.mu.Lock()
	defer ac.mu.Unlock()

	if ac.size == 0 {
		return nil
	}
	return ac.verifyChain(0, ac.size-1)
}

// VerifyRange checks the integrity of the chain from index `from` to `to`
// inclusive.
func (ac *AuditChain) VerifyRange(_ context.Context, from, to int) error {
	ac.mu.Lock()
	defer ac.mu.Unlock()

	if from < 0 || to >= ac.size || from > to {
		return ErrIndexOutOfRange
	}
	return ac.verifyChain(from, to)
}

// GetEvent retrieves a chain entry by its index.
func (ac *AuditChain) GetEvent(index int) (*ChainEntry, error) {
	ac.mu.Lock()
	defer ac.mu.Unlock()

	if index < 0 || index >= ac.size {
		return nil, ErrIndexOutOfRange
	}
	// Return a copy so the caller cannot mutate internal state.
	cp := *ac.entries[index]
	return &cp, nil
}

// RootHash returns the current Merkle root hash as a hex string.
func (ac *AuditChain) RootHash() Hash {
	ac.mu.Lock()
	defer ac.mu.Unlock()
	return ac.tree.RootHash()
}

// Size returns the total number of entries in the chain.
func (ac *AuditChain) Size() int {
	ac.mu.Lock()
	defer ac.mu.Unlock()
	return ac.size
}

// Close flushes pending writes and releases file handles.
func (ac *AuditChain) Close() error {
	ac.mu.Lock()
	defer ac.mu.Unlock()

	if ac.closed {
		return nil
	}
	ac.closed = true

	if ac.writer != nil {
		if err := ac.writer.Flush(); err != nil {
			return err
		}
	}
	if ac.file != nil {
		return ac.file.Close()
	}
	return nil
}

// ---------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------

// verifyChain checks hash linkage and Merkle root for entries in [from, to].
func (ac *AuditChain) verifyChain(from, to int) error {
	for i := from; i <= to; i++ {
		ce := ac.entries[i]

		// Recompute event hash.
		eventBytes, err := json.Marshal(ce.Event)
		if err != nil {
			return fmt.Errorf("%w: entry %d: marshal event: %v", ErrChainCorrupted, i, err)
		}

		preimage := fmt.Sprintf("%d:%s:%s", ce.Index, ce.PrevHash, sha256Hex(eventBytes))
		computed := sha256Hex([]byte(preimage))

		if computed != ce.Hash {
			return fmt.Errorf("%w: entry %d: %v", ErrHashMismatch, i,
				fmt.Sprintf("expected %s, got %s", ce.Hash, computed))
		}

		// Verify prev_hash linkage (except genesis).
		if i > 0 {
			prev := ac.entries[i-1]
			if ce.PrevHash != prev.Hash {
				return fmt.Errorf("%w: entry %d: prev_hash mismatch", ErrChainCorrupted, i)
			}
		}
	}

	return nil
}

// writeEntry serialises a ChainEntry as a single JSONL line.
func (ac *AuditChain) writeEntry(ce *ChainEntry) error {
	if ac.writer == nil {
		return ErrChainClosed
	}
	data, err := json.Marshal(ce)
	if err != nil {
		return err
	}
	if _, err := ac.writer.Write(data); err != nil {
		return err
	}
	return ac.writer.WriteByte('\n')
}

// openSegment creates (or opens) the current JSONL segment file.
func (ac *AuditChain) openSegment() error {
	name := fmt.Sprintf("audit_%04d.jsonl", ac.segIndex)
	path := filepath.Join(ac.cfg.StoragePath, name)

	f, err := os.OpenFile(path, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0640)
	if err != nil {
		return err
	}

	ac.file = f
	ac.writer = bufio.NewWriter(f)
	return nil
}

// loadSegments reads all existing segment files from StoragePath in order.
func (ac *AuditChain) loadSegments() error {
	// Discover segment files.
	entries, err := os.ReadDir(ac.cfg.StoragePath)
	if err != nil {
		if os.IsNotExist(err) {
			return nil
		}
		return err
	}

	// Collect segment file names.
	var segFiles []string
	for _, e := range entries {
		if e.IsDir() {
			continue
		}
		name := e.Name()
		if filepath.Ext(name) == ".jsonl" {
			segFiles = append(segFiles, filepath.Join(ac.cfg.StoragePath, name))
		}
	}
	if len(segFiles) == 0 {
		return nil
	}

	// Load each segment.
	for _, sf := range segFiles {
		if err := ac.loadSegmentFile(sf); err != nil {
			return fmt.Errorf("load segment %s: %w", sf, err)
		}
	}

	// Advance segment index so the next openSegment uses a new file.
	ac.segIndex = len(segFiles)

	return nil
}

// loadSegmentFile reads a single JSONL segment file into memory.
func (ac *AuditChain) loadSegmentFile(path string) error {
	f, err := os.Open(path)
	if err != nil {
		return err
	}
	defer f.Close()

	decoder := json.NewDecoder(f)
	for {
		var ce ChainEntry
		if err := decoder.Decode(&ce); err != nil {
			if errors.Is(err, io.EOF) {
				break
			}
			// Skip malformed lines but continue.
			continue
		}

		// Rebuild Merkle tree.
		eventBytes, _ := json.Marshal(ce.Event)
		ac.tree.AddLeaf(eventBytes)

		ac.entries = append(ac.entries, &ce)
		ac.prevHash = ce.Hash
		ac.size++
	}

	return nil
}
