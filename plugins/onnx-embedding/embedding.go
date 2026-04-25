package main

import (
	"context"
	"fmt"
	"math"
	"os"
	"runtime"
	"strings"

	onnx "github.com/yalue/onnxruntime_go"
)

// embeddingEngine wraps ONNX Runtime for text embedding inference.
type embeddingEngine struct {
	session   onnx.DynamicAdvancedSession
	dim       int
	modelPath string
}

// newEmbeddingEngine creates a new embedding engine from an ONNX model file.
//
// The onnxruntime shared library must be discoverable:
//   - Windows: onnxruntime.dll in PATH or current directory
//   - Linux:   libonnxruntime.so in LD_LIBRARY_PATH
func newEmbeddingEngine(modelPath string, dim int) (*embeddingEngine, error) {
	if _, err := os.Stat(modelPath); err != nil {
		return nil, fmt.Errorf("model file not found: %w", err)
	}

	// Set platform-appropriate ONNX Runtime shared library path.
	if runtime.GOOS == "windows" {
		onnx.SetSharedLibraryPath("onnxruntime.dll")
	} else {
		onnx.SetSharedLibraryPath("libonnxruntime.so")
	}

	if err := onnx.InitializeEnvironment(); err != nil {
		return nil, fmt.Errorf("init ONNX environment: %w", err)
	}

	// Create session with dynamic input shapes for variable-length text.
	// These input/output names match BERT-style models (e.g. all-MiniLM-L6-v2).
	inputShapes := map[string][]int64{
		"input_ids":      {1, -1}, // batch=1, seq_len=dynamic
		"attention_mask": {1, -1},
	}
	outputShapes := map[string][]int64{
		"last_hidden_state": {1, -1, int64(dim)},
	}

	session, err := onnx.NewDynamicAdvancedSession(modelPath,
		[]string{"input_ids", "attention_mask"},
		[]string{"last_hidden_state"},
		inputShapes,
		outputShapes,
	)
	if err != nil {
		onnx.DestroyEnvironment()
		return nil, fmt.Errorf("create ONNX session: %w", err)
	}

	return &embeddingEngine{
		session:   session,
		dim:       dim,
		modelPath: modelPath,
	}, nil
}

// embed produces an embedding vector for the given text.
func (e *embeddingEngine) embed(ctx context.Context, text string) ([]float32, error) {
	// SKETCH: Tokenization. Production use requires a proper tokenizer matching
	// the ONNX model's vocabulary (e.g. HuggingFace tokenizer JSON). This
	// whitespace tokenizer is a placeholder that produces hash-based token IDs.
	tokens := tokenize(text)
	if len(tokens) == 0 {
		return make([]float32, e.dim), nil
	}

	inputIDs := make([]int64, len(tokens))
	attentionMask := make([]int64, len(tokens))
	for i, token := range tokens {
		inputIDs[i] = hashToken(token)
		attentionMask[i] = 1
	}

	inputTensor, err := onnx.NewTensor(inputIDs)
	if err != nil {
		return nil, fmt.Errorf("create input tensor: %w", err)
	}
	maskTensor, err := onnx.NewTensor(attentionMask)
	if err != nil {
		return nil, fmt.Errorf("create mask tensor: %w", err)
	}

	output := make([]float32, len(tokens)*e.dim)
	outputTensor, err := onnx.NewTensor(output)
	if err != nil {
		return nil, fmt.Errorf("create output tensor: %w", err)
	}

	err = e.session.Run(
		[]onnx.ArbitraryTensor{inputTensor, maskTensor},
		[]onnx.ArbitraryTensor{outputTensor},
	)
	if err != nil {
		return nil, fmt.Errorf("ONNX inference: %w", err)
	}

	// Mean pooling over sequence length
	result := make([]float32, e.dim)
	for i := 0; i < len(tokens); i++ {
		for j := 0; j < e.dim; j++ {
			result[j] += output[i*e.dim+j] / float32(len(tokens))
		}
	}

	l2Normalize(result)
	return result, nil
}

// close releases ONNX Runtime resources.
func (e *embeddingEngine) close() {
	if e.session != nil {
		e.session.Destroy()
	}
	onnx.DestroyEnvironment()
}

// tokenize splits text on whitespace. SKETCH: replace with model-matched tokenizer.
func tokenize(text string) []string {
	return strings.Fields(text)
}

// hashToken produces a deterministic token ID via FNV hash. SKETCH: use real vocabulary.
func hashToken(token string) int64 {
	h := uint32(2166136261)
	for i := 0; i < len(token); i++ {
		h ^= uint32(token[i])
		h *= 16777619
	}
	return int64(h % 30522) // BERT vocab size
}

func l2Normalize(vec []float32) {
	var sum float64
	for _, v := range vec {
		sum += float64(v) * float64(v)
	}
	if sum > 0 {
		norm := float32(1.0 / math.Sqrt(sum))
		for i := range vec {
			vec[i] *= norm
		}
	}
}
