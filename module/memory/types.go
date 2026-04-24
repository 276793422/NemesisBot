package memory

import (
	"context"
	"time"
)

// MemoryType categorizes the kind of memory.
type MemoryType int

const (
	MemoryShortTerm MemoryType = iota // Current conversation context
	MemoryLongTerm                    // Persistent knowledge
	MemoryEpisodic                    // Conversation episodes/experiences
	MemoryGraph                       // Knowledge graph entities
	MemoryDaily                       // Daily notes
)

func (m MemoryType) String() string {
	switch m {
	case MemoryShortTerm:
		return "short_term"
	case MemoryLongTerm:
		return "long_term"
	case MemoryEpisodic:
		return "episodic"
	case MemoryGraph:
		return "graph"
	case MemoryDaily:
		return "daily"
	default:
		return "unknown"
	}
}

// ParseMemoryType converts a string to MemoryType.
func ParseMemoryType(s string) MemoryType {
	switch s {
	case "short_term":
		return MemoryShortTerm
	case "long_term":
		return MemoryLongTerm
	case "episodic":
		return MemoryEpisodic
	case "graph":
		return MemoryGraph
	case "daily":
		return MemoryDaily
	default:
		return MemoryLongTerm
	}
}

// Entry represents a single memory entry.
type Entry struct {
	ID        string            `json:"id"`
	Type      MemoryType        `json:"type"`
	Content   string            `json:"content"`
	Metadata  map[string]string `json:"metadata,omitempty"`
	Tags      []string          `json:"tags,omitempty"`
	Score     float64           `json:"score,omitempty"` // relevance score for search results
	CreatedAt time.Time         `json:"created_at"`
	UpdatedAt time.Time         `json:"updated_at"`
}

// SearchResult represents a memory search result.
type SearchResult struct {
	Entries []Entry `json:"entries"`
	Total   int     `json:"total"`
	Query   string  `json:"query"`
}

// Store is the interface that memory backends must implement.
type Store interface {
	// Store saves a memory entry.
	Store(ctx context.Context, entry *Entry) error
	// Query searches memories by text query.
	Query(ctx context.Context, query string, limit int, types []MemoryType) (*SearchResult, error)
	// Get retrieves a memory by ID.
	Get(ctx context.Context, id string) (*Entry, error)
	// Delete removes a memory entry.
	Delete(ctx context.Context, id string) error
	// List lists all memories, optionally filtered by type.
	List(ctx context.Context, types []MemoryType, offset, limit int) (*SearchResult, error)
	// Close releases resources.
	Close() error
}
