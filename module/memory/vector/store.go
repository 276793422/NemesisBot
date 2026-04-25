package vector

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"math"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	chromem "github.com/philippgille/chromem-go"
)

const (
	collectionName      = "memories"
	persistenceFile     = "vector_store.jsonl"
	defaultLocalDim     = 256
	defaultMaxResults   = 10
	defaultConcurrency  = 4 // parallelism for batch document indexing
)

// StoreConfig holds the configuration for creating a VectorStore.
// This is defined here to avoid an import cycle with the memory package.
type StoreConfig struct {
	EmbeddingTier       string
	LocalDim            int
	PluginPath          string
	PluginModelPath     string
	APIModel            string
	MaxResults          int
	SimilarityThreshold float64
	StoragePath         string
}

// Entry represents a memory entry in the vector store.
// Mirrors memory.Entry but avoids importing the memory package.
type Entry struct {
	ID        string            `json:"id"`
	Type      string            `json:"type"`
	Content   string            `json:"content"`
	Metadata  map[string]string `json:"metadata,omitempty"`
	Tags      []string          `json:"tags,omitempty"`
	Score     float64           `json:"score,omitempty"`
	CreatedAt time.Time         `json:"created_at"`
	UpdatedAt time.Time         `json:"updated_at"`
}

// QueryResult represents the result of a vector search.
type QueryResult struct {
	Entries []Entry
	Total   int
	Query   string
}

// VectorStore wraps chromem-go and provides semantic vector search.
// It uses JSONL for full-entry persistence and chromem-go for similarity search.
type VectorStore struct {
	db          *chromem.DB
	collection  *chromem.Collection
	embedFunc   chromem.EmbeddingFunc
	dim         int
	cfg         StoreConfig
	mu          sync.RWMutex
	persistPath string
}

// EmbeddingProvider is an interface for external embedding API providers.
type EmbeddingProvider interface {
	CreateEmbedding(ctx context.Context, model, text string) ([]float32, error)
}

// NewVectorStore creates a new VectorStore backed by chromem-go.
// The EmbeddingFunc is always explicitly set (never nil) to prevent chromem-go
// from calling OpenAI by default.
func NewVectorStore(cfg StoreConfig, provider EmbeddingProvider) (*VectorStore, error) {
	dim := cfg.LocalDim
	if dim <= 0 {
		dim = defaultLocalDim
	}

	embedFunc := NewEmbeddingFunc(cfg, provider, dim)

	db := chromem.NewDB()
	collection, err := db.CreateCollection(collectionName, nil, embedFunc)
	if err != nil {
		return nil, fmt.Errorf("vector: create collection: %w", err)
	}

	// Determine persistence path
	persistPath := cfg.StoragePath
	if persistPath == "" {
		persistPath = filepath.Join("memory", "vector", persistenceFile)
	}

	vs := &VectorStore{
		db:          db,
		collection:  collection,
		embedFunc:   embedFunc,
		dim:         dim,
		cfg:         cfg,
		persistPath: persistPath,
	}

	// Load persisted data
	if err := vs.loadPersisted(); err != nil {
		return nil, fmt.Errorf("vector: load persisted: %w", err)
	}

	return vs, nil
}

// StoreEntry saves an entry to the vector store and persists it to JSONL.
func (s *VectorStore) StoreEntry(ctx context.Context, entry *Entry) error {
	s.mu.Lock()
	defer s.mu.Unlock()

	if entry == nil {
		return nil
	}

	// Build chromem metadata
	meta := map[string]string{
		"type":       entry.Type,
		"created_at": entry.CreatedAt.Format("2006-01-02T15:04:05Z"),
	}
	for k, v := range entry.Metadata {
		meta["meta_"+k] = v
	}
	for i, t := range entry.Tags {
		meta[fmt.Sprintf("tag_%d", i)] = t
	}

	doc := chromem.Document{
		ID:       entry.ID,
		Content:  entry.Content,
		Metadata: meta,
	}

	if err := s.collection.AddDocument(ctx, doc); err != nil {
		return fmt.Errorf("vector: add document: %w", err)
	}

	return s.persist(entry)
}

// Query performs semantic search against the vector store.
func (s *VectorStore) Query(ctx context.Context, query string, limit int, typeFilter []string) (*QueryResult, error) {
	s.mu.RLock()
	defer s.mu.RUnlock()

	if limit <= 0 {
		limit = defaultMaxResults
	}

	// chromem-go requires nResults <= document count
	docCount := s.collection.Count()
	if docCount == 0 {
		return &QueryResult{Query: query, Total: 0}, nil
	}
	nResults := limit
	if nResults > docCount {
		nResults = docCount
	}

	results, err := s.collection.Query(ctx, query, nResults, nil, nil)
	if err != nil {
		return nil, fmt.Errorf("vector: query: %w", err)
	}

	// Convert results and apply filters
	threshold := float32(s.cfg.SimilarityThreshold)
	if threshold == 0 {
		threshold = 0.7
	}

	var entries []Entry
	for _, r := range results {
		if r.Similarity < threshold {
			continue
		}

		entry := chromemResultToEntry(r)

		// Filter by type if specified
		if len(typeFilter) > 0 && !matchString(entry.Type, typeFilter) {
			continue
		}

		entry.Score = float64(r.Similarity)
		entries = append(entries, entry)
	}

	total := len(entries)
	if len(entries) > limit {
		entries = entries[:limit]
	}

	return &QueryResult{
		Entries: entries,
		Total:   total,
		Query:   query,
	}, nil
}

// GetByID retrieves a memory entry by ID.
func (s *VectorStore) GetByID(ctx context.Context, id string) (*Entry, error) {
	s.mu.RLock()
	defer s.mu.RUnlock()

	doc, err := s.collection.GetByID(ctx, id)
	if err == nil && doc.ID != "" {
		entry := chromemDocToEntry(doc)
		return &entry, nil
	}

	// Fallback to JSONL persistence
	entries, err := s.readPersisted()
	if err != nil {
		return nil, nil
	}
	for _, e := range entries {
		if e.ID == id {
			return &e, nil
		}
	}
	return nil, nil
}

// DeleteEntry removes an entry by ID.
func (s *VectorStore) DeleteEntry(ctx context.Context, id string) error {
	s.mu.Lock()
	defer s.mu.Unlock()

	_ = s.collection.Delete(ctx, nil, nil, id)
	return s.removeFromPersistence(id)
}

// ListEntries returns entries optionally filtered by type, with pagination.
func (s *VectorStore) ListEntries(typeFilter []string, offset, limit int) (*QueryResult, error) {
	s.mu.RLock()
	defer s.mu.RUnlock()

	entries, err := s.readPersisted()
	if err != nil {
		return nil, fmt.Errorf("vector: list entries: %w", err)
	}

	var filtered []Entry
	for _, e := range entries {
		if len(typeFilter) > 0 && !matchString(e.Type, typeFilter) {
			continue
		}
		filtered = append(filtered, e)
	}

	total := len(filtered)
	if offset > len(filtered) {
		offset = len(filtered)
	}
	filtered = filtered[offset:]
	if limit > 0 && len(filtered) > limit {
		filtered = filtered[:limit]
	}

	return &QueryResult{
		Entries: filtered,
		Total:   total,
	}, nil
}

// Close releases resources.
func (s *VectorStore) Close() error {
	return nil
}

// persist appends an entry to the JSONL file.
func (s *VectorStore) persist(entry *Entry) error {
	dir := filepath.Dir(s.persistPath)
	if err := os.MkdirAll(dir, 0755); err != nil {
		return err
	}

	f, err := os.OpenFile(s.persistPath, os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0644)
	if err != nil {
		return err
	}
	defer f.Close()

	data, err := json.Marshal(entry)
	if err != nil {
		return err
	}
	_, err = fmt.Fprintf(f, "%s\n", data)
	return err
}

// loadPersisted reads all entries from JSONL and re-indexes them into chromem-go.
func (s *VectorStore) loadPersisted() error {
	entries, err := s.readPersisted()
	if err != nil {
		return err
	}
	if len(entries) == 0 {
		return nil
	}

	docs := make([]chromem.Document, 0, len(entries))
	for i := range entries {
		e := &entries[i]
		meta := map[string]string{
			"type":       e.Type,
			"created_at": e.CreatedAt.Format("2006-01-02T15:04:05Z"),
		}
		for k, v := range e.Metadata {
			meta["meta_"+k] = v
		}
		for j, t := range e.Tags {
			meta[fmt.Sprintf("tag_%d", j)] = t
		}
		docs = append(docs, chromem.Document{
			ID:       e.ID,
			Content:  e.Content,
			Metadata: meta,
		})
	}

	ctx := context.Background()
	return s.collection.AddDocuments(ctx, docs, defaultConcurrency)
}

// readPersisted reads all entries from the JSONL file.
func (s *VectorStore) readPersisted() ([]Entry, error) {
	f, err := os.Open(s.persistPath)
	if err != nil {
		if os.IsNotExist(err) {
			return nil, nil
		}
		return nil, err
	}
	defer f.Close()

	var entries []Entry
	scanner := bufio.NewScanner(f)
	scanner.Buffer(make([]byte, 0, 64*1024), 1024*1024)
	for scanner.Scan() {
		line := scanner.Text()
		if line == "" {
			continue
		}
		var entry Entry
		if err := json.Unmarshal([]byte(line), &entry); err != nil {
			continue
		}
		entries = append(entries, entry)
	}
	return entries, scanner.Err()
}

// removeFromPersistence rewrites the JSONL file without the given ID.
func (s *VectorStore) removeFromPersistence(id string) error {
	entries, err := s.readPersisted()
	if err != nil {
		return err
	}

	dir := filepath.Dir(s.persistPath)
	if err := os.MkdirAll(dir, 0755); err != nil {
		return err
	}

	f, err := os.Create(s.persistPath)
	if err != nil {
		return err
	}
	defer f.Close()

	w := bufio.NewWriter(f)
	for _, e := range entries {
		if e.ID == id {
			continue
		}
		data, err := json.Marshal(e)
		if err != nil {
			return err
		}
		if _, err := fmt.Fprintf(w, "%s\n", data); err != nil {
			return err
		}
	}
	return w.Flush()
}

// chromemResultToEntry converts a chromem Result to an Entry.
func chromemResultToEntry(r chromem.Result) Entry {
	return chromemDocToEntry(chromem.Document{
		ID:       r.ID,
		Content:  r.Content,
		Metadata: r.Metadata,
	})
}

// chromemDocToEntry converts a chromem Document to an Entry.
func chromemDocToEntry(doc chromem.Document) Entry {
	e := Entry{
		ID:      doc.ID,
		Content: doc.Content,
	}
	if t, ok := doc.Metadata["type"]; ok {
		e.Type = t
	}
	if ca, ok := doc.Metadata["created_at"]; ok {
		if t, err := time.Parse("2006-01-02T15:04:05Z", ca); err == nil {
			e.CreatedAt = t
		}
	}
	for k, v := range doc.Metadata {
		if strings.HasPrefix(k, "meta_") {
			if e.Metadata == nil {
				e.Metadata = make(map[string]string)
			}
			e.Metadata[k[5:]] = v
		}
	}
	// Reconstruct tags from tag_0, tag_1, ...
	for i := 0; ; i++ {
		key := fmt.Sprintf("tag_%d", i)
		v, ok := doc.Metadata[key]
		if !ok {
			break
		}
		e.Tags = append(e.Tags, v)
	}
	return e
}

// matchString checks if a string matches any of the filter values.
func matchString(s string, filters []string) bool {
	for _, f := range filters {
		if s == f {
			return true
		}
	}
	return false
}

// CosineSimilarity computes cosine similarity between two vectors.
func CosineSimilarity(a, b []float32) float64 {
	if len(a) != len(b) || len(a) == 0 {
		return 0
	}
	var dot, normA, normB float64
	for i := range a {
		dot += float64(a[i]) * float64(b[i])
		normA += float64(a[i]) * float64(a[i])
		normB += float64(b[i]) * float64(b[i])
	}
	if normA == 0 || normB == 0 {
		return 0
	}
	return dot / (math.Sqrt(normA) * math.Sqrt(normB))
}
