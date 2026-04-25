package vector_test

import (
	"context"
	"math"
	"testing"

	"github.com/276793422/NemesisBot/module/memory/vector"
)

func TestLocalEmbeddingFunc_Dimension(t *testing.T) {
	fn := vector.LocalEmbeddingFunc(256)
	vec, err := fn(context.Background(), "hello world")
	if err != nil {
		t.Fatal(err)
	}
	if len(vec) != 256 {
		t.Errorf("expected dimension 256, got %d", len(vec))
	}
}

func TestLocalEmbeddingFunc_CustomDimension(t *testing.T) {
	fn := vector.LocalEmbeddingFunc(128)
	vec, err := fn(context.Background(), "hello world")
	if err != nil {
		t.Fatal(err)
	}
	if len(vec) != 128 {
		t.Errorf("expected dimension 128, got %d", len(vec))
	}
}

func TestLocalEmbeddingFunc_DefaultDimension(t *testing.T) {
	fn := vector.LocalEmbeddingFunc(0) // should default to 256
	vec, err := fn(context.Background(), "hello world")
	if err != nil {
		t.Fatal(err)
	}
	if len(vec) != 256 {
		t.Errorf("expected dimension 256 for dim=0, got %d", len(vec))
	}
}

func TestLocalEmbeddingFunc_L2Normalized(t *testing.T) {
	fn := vector.LocalEmbeddingFunc(256)
	vec, err := fn(context.Background(), "hello world this is a test")
	if err != nil {
		t.Fatal(err)
	}

	var norm float64
	for _, v := range vec {
		norm += float64(v) * float64(v)
	}
	norm = math.Sqrt(norm)
	if math.Abs(norm-1.0) > 1e-6 {
		t.Errorf("vector not L2 normalized, norm = %f", norm)
	}
}

func TestLocalEmbeddingFunc_SimilarTexts(t *testing.T) {
	fn := vector.LocalEmbeddingFunc(256)
	vec1, _ := fn(context.Background(), "the cat sat on the mat")
	vec2, _ := fn(context.Background(), "the cat sat on the mat")

	sim := vector.CosineSimilarity(vec1, vec2)
	if math.Abs(sim-1.0) > 1e-6 {
		t.Errorf("identical texts should have similarity 1.0, got %f", sim)
	}

	vec3, _ := fn(context.Background(), "the cat is sitting on the mat")
	sim2 := vector.CosineSimilarity(vec1, vec3)
	if sim2 < 0.5 {
		t.Errorf("similar texts should have similarity > 0.5, got %f", sim2)
	}
}

func TestLocalEmbeddingFunc_DissimilarTexts(t *testing.T) {
	fn := vector.LocalEmbeddingFunc(256)
	vec1, _ := fn(context.Background(), "programming computer software")
	vec2, _ := fn(context.Background(), "cooking recipe kitchen food")

	sim := vector.CosineSimilarity(vec1, vec2)
	if sim > 0.9 {
		t.Errorf("dissimilar texts should have lower similarity, got %f", sim)
	}
}

func TestLocalEmbeddingFunc_EmptyInput(t *testing.T) {
	fn := vector.LocalEmbeddingFunc(256)
	vec, err := fn(context.Background(), "")
	if err != nil {
		t.Fatal(err)
	}
	if len(vec) != 256 {
		t.Errorf("expected dimension 256, got %d", len(vec))
	}
	for _, v := range vec {
		if v != 0 {
			t.Error("expected zero vector for empty input")
			break
		}
	}
}

func TestLocalEmbeddingFunc_NeverCallsExternal(t *testing.T) {
	fn := vector.LocalEmbeddingFunc(256)
	_, err := fn(context.Background(), "test without network")
	if err != nil {
		t.Errorf("local embedding should never fail: %v", err)
	}
}

func TestLocalEmbeddingFunc_SpecialCharacters(t *testing.T) {
	fn := vector.LocalEmbeddingFunc(256)
	vec, err := fn(context.Background(), "hello! @world# $test% &data*")
	if err != nil {
		t.Fatal(err)
	}
	if len(vec) != 256 {
		t.Errorf("expected dimension 256, got %d", len(vec))
	}
}

func TestLocalEmbeddingFunc_Multilingual(t *testing.T) {
	fn := vector.LocalEmbeddingFunc(256)
	vec, err := fn(context.Background(), "你好世界 hello world")
	if err != nil {
		t.Fatal(err)
	}
	if len(vec) != 256 {
		t.Errorf("expected dimension 256, got %d", len(vec))
	}
}

func TestLocalEmbeddingFunc_Deterministic(t *testing.T) {
	fn := vector.LocalEmbeddingFunc(256)
	vec1, _ := fn(context.Background(), "test determinism")
	vec2, _ := fn(context.Background(), "test determinism")
	for i := range vec1 {
		if vec1[i] != vec2[i] {
			t.Error("embedding should be deterministic for same input")
			break
		}
	}
}
