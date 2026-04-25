package vector

import (
	"context"
	"fmt"

	chromem "github.com/philippgille/chromem-go"
)

// APIEmbeddingFunc returns an EmbeddingFunc that uses a Provider API endpoint.
// Only enabled when the user explicitly configures an embedding_model.
func APIEmbeddingFunc(provider EmbeddingProvider, model string) chromem.EmbeddingFunc {
	return func(ctx context.Context, text string) ([]float32, error) {
		if provider == nil {
			return nil, fmt.Errorf("vector: embedding provider is nil")
		}
		if model == "" {
			return nil, fmt.Errorf("vector: embedding model is empty")
		}
		vec, err := provider.CreateEmbedding(ctx, model, text)
		if err != nil {
			return nil, fmt.Errorf("vector: API embedding: %w", err)
		}
		return vec, nil
	}
}
