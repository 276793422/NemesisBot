package vector

import (
	"context"
	"math"
	"strings"
	"unicode"

	chromem "github.com/philippgille/chromem-go"
)

// LocalEmbeddingFunc returns a pure Go character N-gram hash embedding function.
// Zero token cost, zero external dependencies, fully offline.
//
// Algorithm: tokenize → character trigram extraction → FNV dual-hash mapping → L2 normalization
func LocalEmbeddingFunc(dim int) chromem.EmbeddingFunc {
	if dim <= 0 {
		dim = 256
	}
	return func(ctx context.Context, text string) ([]float32, error) {
		return ngramHashEmbed(text, dim), nil
	}
}

// ngramHashEmbed produces a fixed-dimension embedding vector from text using
// character trigram hashing.
func ngramHashEmbed(text string, dim int) []float32 {
	vec := make([]float32, dim)
	if text == "" {
		return vec
	}

	words := tokenizeForEmbedding(text)
	for _, word := range words {
		trigrams := extractTrigrams(word)
		for _, tg := range trigrams {
			// First hash: determine dimension index
			h1 := fnvHash32(tg)
			idx := int(h1) % dim
			if idx < 0 {
				idx = -idx
			}
			// Second hash: determine sign (+1 or -1)
			h2 := fnvHash32(tg + "_sign")
			if h2%2 == 0 {
				vec[idx] += 1.0
			} else {
				vec[idx] -= 1.0
			}
		}
	}

	l2Normalize(vec)
	return vec
}

// tokenizeForEmbedding splits text into lowercase tokens suitable for embedding.
func tokenizeForEmbedding(text string) []string {
	text = strings.ToLower(text)

	var b strings.Builder
	for _, r := range text {
		if unicode.IsPunct(r) || unicode.IsSymbol(r) {
			b.WriteByte(' ')
		} else {
			b.WriteRune(r)
		}
	}

	return strings.Fields(b.String())
}

// extractTrigrams extracts character trigrams from a word with boundary markers.
// For example, "hello" → ["#he", "hel", "ell", "llo", "lo#"]
func extractTrigrams(word string) []string {
	if len(word) == 0 {
		return nil
	}

	// Add boundary markers
	padded := "#" + word + "#"
	if len(padded) < 3 {
		return []string{padded}
	}

	var trigrams []string
	for i := 0; i+3 <= len(padded); i++ {
		trigrams = append(trigrams, padded[i:i+3])
	}
	return trigrams
}

// fnvHash32 computes FNV-1a 32-bit hash.
func fnvHash32(s string) uint32 {
	const (
		prime32  uint32 = 16777619
		offset32 uint32 = 2166136261
	)
	h := offset32
	for i := 0; i < len(s); i++ {
		h ^= uint32(s[i])
		h *= prime32
	}
	return h
}

// l2Normalize normalizes a vector to unit length in-place.
// If the vector is zero, it is left unchanged.
func l2Normalize(vec []float32) {
	var sum float64
	for _, v := range vec {
		sum += float64(v) * float64(v)
	}
	norm := float32(math.Sqrt(sum))
	if norm > 0 {
		for i := range vec {
			vec[i] /= norm
		}
	}
}
