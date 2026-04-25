package vector

// EmbeddingPlugin defines the interface for ONNX embedding plugins loaded via DLL/SO.
type EmbeddingPlugin interface {
	// Init initializes the plugin with the model path and output dimension.
	Init(modelPath string, dim int) error
	// Embed produces an embedding vector for the given text.
	Embed(text string) ([]float32, error)
	// Close releases plugin resources.
	Close()
}
