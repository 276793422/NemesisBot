package memory

// Config holds memory system configuration.
type Config struct {
	Vector VectorConfig `json:"vector"`
}

// VectorConfig configures the vector search backend.
type VectorConfig struct {
	Enabled             bool    `json:"enabled"`
	Backend             string  `json:"backend"`               // "local" (default) | "chromem"
	EmbeddingTier       string  `json:"embedding_tier"`        // "auto" | "plugin" | "api" | "local"
	EmbeddingModel      string  `json:"embedding_model"`       // legacy: "local" for TF-IDF fallback
	LocalDim            int     `json:"local_dim"`             // local hash dimension (default 256)
	PluginPath          string  `json:"plugin_path"`           // ONNX DLL/SO path
	PluginModelPath     string  `json:"plugin_model_path"`     // ONNX model file path
	APIModel            string  `json:"api_model"`             // Provider API embedding model name
	MaxResults          int     `json:"max_results"`           // default 5
	SimilarityThreshold float64 `json:"similarity_threshold"`  // default 0.7
	RetentionDays       int     `json:"retention_days"`        // default 90
	StoragePath         string  `json:"storage_path"`          // defaults to {workspace}/memory/vector/
}

// DefaultConfig returns default memory configuration.
func DefaultConfig() *Config {
	return &Config{
		Vector: VectorConfig{
			Enabled:             false,
			Backend:             "local",
			EmbeddingTier:       "auto",
			LocalDim:            256,
			MaxResults:          5,
			SimilarityThreshold: 0.7,
			RetentionDays:       90,
		},
	}
}
