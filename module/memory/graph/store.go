// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package graph

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"
)

// Triple represents a subject-predicate-object relationship in the knowledge graph.
type Triple struct {
	Subject    string            `json:"subject"`
	Predicate  string            `json:"predicate"`
	Object     string            `json:"object"`
	Metadata   map[string]string `json:"metadata,omitempty"`
	Confidence float64           `json:"confidence"`
	CreatedAt  time.Time         `json:"created_at"`
}

// Entity represents a node in the knowledge graph.
type Entity struct {
	Name       string            `json:"name"`
	Type       string            `json:"type"` // person, place, thing, concept
	Properties map[string]string `json:"properties,omitempty"`
	CreatedAt  time.Time         `json:"created_at"`
}

// Config holds knowledge graph store configuration.
type Config struct {
	StoragePath string // directory for JSONL files
}

// Store implements a simple knowledge graph backed by JSONL files.
type Store struct {
	path     string
	mu       sync.RWMutex
	entities map[string]*Entity
	triples  []*Triple
}

// NewStore creates a new graph store, loads existing data from disk.
func NewStore(cfg Config) (*Store, error) {
	if err := os.MkdirAll(cfg.StoragePath, 0755); err != nil {
		return nil, fmt.Errorf("graph: create storage dir: %w", err)
	}

	s := &Store{
		path:     cfg.StoragePath,
		entities: make(map[string]*Entity),
		triples:  []*Triple{},
	}

	if err := s.load(); err != nil {
		return nil, fmt.Errorf("graph: load existing: %w", err)
	}

	return s, nil
}

// load reads entities and triples from JSONL files.
func (s *Store) load() error {
	// Load entities
	entitiesPath := filepath.Join(s.path, "entities.jsonl")
	if data, err := os.Open(entitiesPath); err == nil {
		defer data.Close()
		decoder := json.NewDecoder(data)
		for decoder.More() {
			var e Entity
			if err := decoder.Decode(&e); err != nil {
				break
			}
			s.entities[strings.ToLower(e.Name)] = &e
		}
	}

	// Load triples
	triplesPath := filepath.Join(s.path, "triples.jsonl")
	if data, err := os.Open(triplesPath); err == nil {
		defer data.Close()
		decoder := json.NewDecoder(data)
		for decoder.More() {
			var t Triple
			if err := decoder.Decode(&t); err != nil {
				break
			}
			s.triples = append(s.triples, &t)
		}
	}

	return nil
}

// persistEntities rewrites the entities JSONL file.
func (s *Store) persistEntities() error {
	path := filepath.Join(s.path, "entities.jsonl")
	f, err := os.Create(path)
	if err != nil {
		return fmt.Errorf("graph: create entities file: %w", err)
	}
	defer f.Close()

	writer := bufio.NewWriter(f)
	for _, e := range s.entities {
		data, err := json.Marshal(e)
		if err != nil {
			continue
		}
		if _, err := writer.Write(append(data, '\n')); err != nil {
			return fmt.Errorf("graph: write entity: %w", err)
		}
	}

	return writer.Flush()
}

// persistTriples rewrites the triples JSONL file.
func (s *Store) persistTriples() error {
	path := filepath.Join(s.path, "triples.jsonl")
	f, err := os.Create(path)
	if err != nil {
		return fmt.Errorf("graph: create triples file: %w", err)
	}
	defer f.Close()

	writer := bufio.NewWriter(f)
	for _, t := range s.triples {
		data, err := json.Marshal(t)
		if err != nil {
			continue
		}
		if _, err := writer.Write(append(data, '\n')); err != nil {
			return fmt.Errorf("graph: write triple: %w", err)
		}
	}

	return writer.Flush()
}

// persistAll rewrites both JSONL files.
func (s *Store) persistAll() error {
	if err := s.persistEntities(); err != nil {
		return err
	}
	return s.persistTriples()
}

// AddEntity adds or updates an entity in the knowledge graph.
func (s *Store) AddEntity(ctx context.Context, entity *Entity) error {
	if entity == nil {
		return fmt.Errorf("graph: entity is nil")
	}
	if entity.Name == "" {
		return fmt.Errorf("graph: entity name is required")
	}
	if entity.CreatedAt.IsZero() {
		entity.CreatedAt = time.Now().UTC()
	}
	if entity.Properties == nil {
		entity.Properties = make(map[string]string)
	}

	s.mu.Lock()
	defer s.mu.Unlock()

	key := strings.ToLower(entity.Name)
	s.entities[key] = entity

	return s.persistEntities()
}

// GetEntity retrieves an entity by name (case-insensitive).
func (s *Store) GetEntity(ctx context.Context, name string) (*Entity, error) {
	s.mu.RLock()
	defer s.mu.RUnlock()

	key := strings.ToLower(name)
	e, ok := s.entities[key]
	if !ok {
		return nil, nil
	}
	return e, nil
}

// AddTriple adds a relationship triple to the knowledge graph.
func (s *Store) AddTriple(ctx context.Context, triple *Triple) error {
	if triple == nil {
		return fmt.Errorf("graph: triple is nil")
	}
	if triple.Subject == "" || triple.Predicate == "" || triple.Object == "" {
		return fmt.Errorf("graph: subject, predicate, and object are required")
	}
	if triple.CreatedAt.IsZero() {
		triple.CreatedAt = time.Now().UTC()
	}
	if triple.Metadata == nil {
		triple.Metadata = make(map[string]string)
	}

	s.mu.Lock()
	defer s.mu.Unlock()

	// Check for duplicate
	for _, t := range s.triples {
		if strings.EqualFold(t.Subject, triple.Subject) &&
			strings.EqualFold(t.Predicate, triple.Predicate) &&
			strings.EqualFold(t.Object, triple.Object) {
			// Update existing triple
			t.Confidence = triple.Confidence
			t.Metadata = triple.Metadata
			return s.persistTriples()
		}
	}

	s.triples = append(s.triples, triple)
	return s.persistTriples()
}

// Query searches for triples matching the given subject, predicate, and/or object.
// Empty strings act as wildcards.
func (s *Store) Query(ctx context.Context, subject, predicate, object string) ([]*Triple, error) {
	s.mu.RLock()
	defer s.mu.RUnlock()

	var results []*Triple
	for _, t := range s.triples {
		if subject != "" && !strings.EqualFold(t.Subject, subject) {
			continue
		}
		if predicate != "" && !strings.EqualFold(t.Predicate, predicate) {
			continue
		}
		if object != "" && !strings.EqualFold(t.Object, object) {
			continue
		}
		results = append(results, t)
	}

	return results, nil
}

// GetRelated returns all relationships within depth hops of the named entity using BFS.
func (s *Store) GetRelated(ctx context.Context, entityName string, depth int) ([]*Triple, error) {
	if depth <= 0 {
		depth = 1
	}

	s.mu.RLock()
	defer s.mu.RUnlock()

	visited := make(map[string]bool)
	var results []*Triple
	queue := []struct {
		name  string
		depth int
	}{{name: strings.ToLower(entityName), depth: 0}}

	visited[strings.ToLower(entityName)] = true

	for len(queue) > 0 {
		current := queue[0]
		queue = queue[1:]

		if current.depth >= depth {
			continue
		}

		for _, t := range s.triples {
			// Check if this triple involves the current entity
			var neighbor string

			if strings.EqualFold(t.Subject, current.name) {
				neighbor = strings.ToLower(t.Object)
			} else if strings.EqualFold(t.Object, current.name) {
				neighbor = strings.ToLower(t.Subject)
			} else {
				continue
			}

			results = append(results, t)

			if !visited[neighbor] {
				visited[neighbor] = true
				queue = append(queue, struct {
					name  string
					depth int
				}{name: neighbor, depth: current.depth + 1})
			}
		}
	}

	return results, nil
}

// Search performs a case-insensitive text search across triples' subject, predicate, and object fields.
func (s *Store) Search(ctx context.Context, query string, limit int) ([]*Triple, error) {
	if limit <= 0 {
		limit = 20
	}

	s.mu.RLock()
	defer s.mu.RUnlock()

	queryLower := strings.ToLower(query)
	var results []*Triple

	for _, t := range s.triples {
		if strings.Contains(strings.ToLower(t.Subject), queryLower) ||
			strings.Contains(strings.ToLower(t.Predicate), queryLower) ||
			strings.Contains(strings.ToLower(t.Object), queryLower) {
			results = append(results, t)
			if len(results) >= limit {
				break
			}
		}
	}

	return results, nil
}

// DeleteEntity removes an entity and all triples that reference it.
func (s *Store) DeleteEntity(ctx context.Context, name string) error {
	s.mu.Lock()
	defer s.mu.Unlock()

	key := strings.ToLower(name)

	// Remove entity
	delete(s.entities, key)

	// Remove all triples referencing this entity
	var remaining []*Triple
	for _, t := range s.triples {
		if strings.EqualFold(t.Subject, name) || strings.EqualFold(t.Object, name) {
			continue
		}
		remaining = append(remaining, t)
	}
	s.triples = remaining

	return s.persistAll()
}

// Close releases resources.
func (s *Store) Close() error {
	return nil
}

// EntityCount returns the number of entities stored.
func (s *Store) EntityCount() int {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return len(s.entities)
}

// TripleCount returns the number of triples stored.
func (s *Store) TripleCount() int {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return len(s.triples)
}
