package vector_test

import (
	"testing"

	"github.com/276793422/NemesisBot/module/memory/vector"
)

func TestNewEmbeddingFunc_DefaultToLocal(t *testing.T) {
	cfg := vector.StoreConfig{}
	fn := vector.NewEmbeddingFunc(cfg, nil, 256)
	if fn == nil {
		t.Fatal("EmbeddingFunc should never be nil")
	}
}

func TestNewEmbeddingFunc_NeverNil(t *testing.T) {
	tests := []struct {
		name string
		cfg  vector.StoreConfig
	}{
		{"empty config", vector.StoreConfig{}},
		{"auto tier", vector.StoreConfig{EmbeddingTier: "auto"}},
		{"local tier", vector.StoreConfig{EmbeddingTier: "local"}},
		{"api tier no provider", vector.StoreConfig{EmbeddingTier: "api"}},
		{"plugin tier no plugin", vector.StoreConfig{EmbeddingTier: "plugin"}},
		{"plugin tier nonexistent", vector.StoreConfig{EmbeddingTier: "plugin", PluginPath: "/nonexistent/path.dll"}},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			fn := vector.NewEmbeddingFunc(tt.cfg, nil, 256)
			if fn == nil {
				t.Error("EmbeddingFunc should never be nil")
			}
		})
	}
}

func TestNewEmbeddingFunc_LocalTier(t *testing.T) {
	cfg := vector.StoreConfig{EmbeddingTier: "local"}
	fn := vector.NewEmbeddingFunc(cfg, nil, 128)
	vec, err := fn(nil, "test")
	if err != nil {
		t.Fatal(err)
	}
	if len(vec) != 128 {
		t.Errorf("expected 128 dimensions, got %d", len(vec))
	}
}

func TestNewEmbeddingFunc_APIWithProvider(t *testing.T) {
	provider := &mockProvider{}
	cfg := vector.StoreConfig{
		EmbeddingTier: "api",
		APIModel:      "text-embedding-3-small",
	}
	fn := vector.NewEmbeddingFunc(cfg, provider, 10)
	if fn == nil {
		t.Fatal("EmbeddingFunc should not be nil")
	}
	vec, err := fn(nil, "hello")
	if err != nil {
		t.Fatal(err)
	}
	if !provider.called {
		t.Error("provider should have been called for API tier")
	}
	_ = vec
}

func TestNewEmbeddingFunc_APIFallsBackWithoutProvider(t *testing.T) {
	cfg := vector.StoreConfig{
		EmbeddingTier: "api",
		APIModel:      "some-model",
	}
	fn := vector.NewEmbeddingFunc(cfg, nil, 256)
	if fn == nil {
		t.Fatal("EmbeddingFunc should not be nil")
	}
	vec, err := fn(nil, "test")
	if err != nil {
		t.Fatal(err)
	}
	if len(vec) != 256 {
		t.Errorf("expected 256 dimensions (local fallback), got %d", len(vec))
	}
}

func TestNewEmbeddingFunc_AutoUsesLocalByDefault(t *testing.T) {
	cfg := vector.StoreConfig{EmbeddingTier: "auto"}
	fn := vector.NewEmbeddingFunc(cfg, nil, 256)
	vec, err := fn(nil, "test")
	if err != nil {
		t.Fatal(err)
	}
	if len(vec) != 256 {
		t.Errorf("expected 256 dimensions, got %d", len(vec))
	}
}

func TestNewEmbeddingFunc_EmptyTierDefaultsToAuto(t *testing.T) {
	cfg := vector.StoreConfig{EmbeddingTier: ""}
	fn := vector.NewEmbeddingFunc(cfg, nil, 256)
	if fn == nil {
		t.Fatal("EmbeddingFunc should not be nil")
	}
}
