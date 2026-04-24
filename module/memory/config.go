package memory

// Config holds memory system configuration.
type Config struct {
	Vector VectorConfig `json:"vector"`
}

// VectorConfig configures the vector search backend.
type VectorConfig struct {
	Enabled             bool    `json:"enabled"`
	Backend             string  `json:"backend"`              // "local" (default)
	EmbeddingModel      string  `json:"embedding_model"`      // "local" for TF-IDF fallback
	MaxResults          int     `json:"max_results"`          // default 5
	SimilarityThreshold float64 `json:"similarity_threshold"` // default 0.7
	RetentionDays       int     `json:"retention_days"`       // default 90
	StoragePath         string  `json:"storage_path"`         // defaults to {workspace}/memory/vector/
}

// DefaultConfig returns default memory configuration.
func DefaultConfig() *Config {
	return &Config{
		Vector: VectorConfig{
			Enabled:             false,
			Backend:             "local",
			EmbeddingModel:      "local",
			MaxResults:          5,
			SimilarityThreshold: 0.7,
			RetentionDays:       90,
		},
	}
}
