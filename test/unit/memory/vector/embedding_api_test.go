package vector_test

import (
	"context"
	"testing"

	"github.com/276793422/NemesisBot/module/memory/vector"
)

// mockProvider is a mock EmbeddingProvider for testing.
type mockProvider struct {
	embeddings map[string][]float32
	err        error
	called     bool
	lastModel  string
}

func (m *mockProvider) CreateEmbedding(ctx context.Context, model, text string) ([]float32, error) {
	m.called = true
	m.lastModel = model
	if m.err != nil {
		return nil, m.err
	}
	if emb, ok := m.embeddings[text]; ok {
		return emb, nil
	}
	vec := make([]float32, 10)
	for i := range vec {
		vec[i] = 0.1
	}
	return vec, nil
}

func TestAPIEmbeddingFunc_Success(t *testing.T) {
	expectedVec := []float32{0.1, 0.2, 0.3}
	provider := &mockProvider{
		embeddings: map[string][]float32{
			"hello": expectedVec,
		},
	}

	fn := vector.APIEmbeddingFunc(provider, "text-embedding-3-small")
	vec, err := fn(context.Background(), "hello")
	if err != nil {
		t.Fatal(err)
	}
	if len(vec) != 3 {
		t.Errorf("expected 3 dimensions, got %d", len(vec))
	}
	if !provider.called {
		t.Error("provider should have been called")
	}
	if provider.lastModel != "text-embedding-3-small" {
		t.Errorf("expected model text-embedding-3-small, got %s", provider.lastModel)
	}
}

func TestAPIEmbeddingFunc_NilProvider(t *testing.T) {
	fn := vector.APIEmbeddingFunc(nil, "test-model")
	_, err := fn(context.Background(), "hello")
	if err == nil {
		t.Error("expected error for nil provider")
	}
}

func TestAPIEmbeddingFunc_EmptyModel(t *testing.T) {
	fn := vector.APIEmbeddingFunc(&mockProvider{}, "")
	_, err := fn(context.Background(), "hello")
	if err == nil {
		t.Error("expected error for empty model")
	}
}

func TestAPIEmbeddingFunc_ProviderError(t *testing.T) {
	provider := &mockProvider{
		err: context.Canceled,
	}
	fn := vector.APIEmbeddingFunc(provider, "test-model")
	_, err := fn(context.Background(), "hello")
	if err == nil {
		t.Error("expected error from provider")
	}
}
