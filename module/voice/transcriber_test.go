// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package voice

import (
	"context"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// Test NewGroqTranscriber
func TestNewGroqTranscriber(t *testing.T) {
	t.Run("with API key", func(t *testing.T) {
		apiKey := "test_api_key"
		transcriber := NewGroqTranscriber(apiKey)

		if transcriber == nil {
			t.Fatal("NewGroqTranscriber() should not return nil")
		}

		if transcriber.apiKey != apiKey {
			t.Errorf("Expected apiKey '%s', got '%s'", apiKey, transcriber.apiKey)
		}

		if transcriber.apiBase != "https://api.groq.com/openai/v1" {
			t.Errorf("Expected apiBase 'https://api.groq.com/openai/v1', got '%s'", transcriber.apiBase)
		}

		if transcriber.httpClient == nil {
			t.Error("httpClient should be initialized")
		}

		if transcriber.httpClient.Timeout != 60*time.Second {
			t.Errorf("Expected timeout 60s, got %v", transcriber.httpClient.Timeout)
		}
	})

	t.Run("without API key", func(t *testing.T) {
		transcriber := NewGroqTranscriber("")

		if transcriber == nil {
			t.Fatal("NewGroqTranscriber() should not return nil")
		}

		if transcriber.apiKey != "" {
			t.Error("Expected empty apiKey")
		}
	})
}

// Test IsAvailable
func TestIsAvailable(t *testing.T) {
	t.Run("with API key", func(t *testing.T) {
		transcriber := NewGroqTranscriber("test_key")

		if !transcriber.IsAvailable() {
			t.Error("IsAvailable() should return true when API key is set")
		}
	})

	t.Run("without API key", func(t *testing.T) {
		transcriber := NewGroqTranscriber("")

		if transcriber.IsAvailable() {
			t.Error("IsAvailable() should return false when API key is empty")
		}
	})
}

// Test Transcribe
func TestTranscribe(t *testing.T) {
	t.Run("successful transcription", func(t *testing.T) {
		// Create mock server
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			// Verify request
			if r.Method != "POST" {
				t.Errorf("Expected POST request, got %s", r.Method)
			}

			if !strings.Contains(r.URL.Path, "audio/transcriptions") {
				t.Errorf("Expected URL path to contain 'audio/transcriptions', got %s", r.URL.Path)
			}

			auth := r.Header.Get("Authorization")
			if !strings.Contains(auth, "Bearer test_api_key") {
				t.Errorf("Expected Authorization header with 'Bearer test_api_key', got '%s'", auth)
			}

			contentType := r.Header.Get("Content-Type")
			if !strings.Contains(contentType, "multipart/form-data") {
				t.Errorf("Expected Content-Type to contain 'multipart/form-data', got '%s'", contentType)
			}

			// Parse form to verify fields
			if err := r.ParseMultipartForm(10 << 20); err != nil {
				t.Errorf("Failed to parse multipart form: %v", err)
			}

			model := r.FormValue("model")
			if model != "whisper-large-v3" {
				t.Errorf("Expected model 'whisper-large-v3', got '%s'", model)
			}

			responseFormat := r.FormValue("response_format")
			if responseFormat != "json" {
				t.Errorf("Expected response_format 'json', got '%s'", responseFormat)
			}

			// Send success response
			w.Header().Set("Content-Type", "application/json")
			w.WriteHeader(http.StatusOK)
			jsonResponse := `{
				"text": "Hello, this is a test transcription.",
				"language": "en",
				"duration": 1.5
			}`
			w.Write([]byte(jsonResponse))
		}))
		defer server.Close()

		// Create transcriber with mock server URL
		transcriber := NewGroqTranscriber("test_api_key")
		transcriber.apiBase = server.URL

		// Create temporary audio file
		tempDir := t.TempDir()
		audioFile := filepath.Join(tempDir, "test_audio.wav")

		// Create a dummy audio file (just some bytes)
		err := os.WriteFile(audioFile, []byte("dummy audio data"), 0644)
		if err != nil {
			t.Fatalf("Failed to create test audio file: %v", err)
		}

		// Transcribe
		ctx := context.Background()
		result, err := transcriber.Transcribe(ctx, audioFile)

		if err != nil {
			t.Fatalf("Transcribe() failed: %v", err)
		}

		if result == nil {
			t.Fatal("Transcribe() should return a result")
		}

		if result.Text != "Hello, this is a test transcription." {
			t.Errorf("Expected text 'Hello, this is a test transcription.', got '%s'", result.Text)
		}

		if result.Language != "en" {
			t.Errorf("Expected language 'en', got '%s'", result.Language)
		}

		if result.Duration != 1.5 {
			t.Errorf("Expected duration 1.5, got %f", result.Duration)
		}
	})

	t.Run("file not found", func(t *testing.T) {
		transcriber := NewGroqTranscriber("test_api_key")

		ctx := context.Background()
		_, err := transcriber.Transcribe(ctx, "/nonexistent/file.wav")

		if err == nil {
			t.Error("Transcribe() should return error for nonexistent file")
		}

		if !strings.Contains(err.Error(), "failed to open audio file") {
			t.Errorf("Error message should mention file opening failure, got: %v", err)
		}
	})

	t.Run("API error response", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			w.Header().Set("Content-Type", "application/json")
			w.WriteHeader(http.StatusBadRequest)
			jsonResponse := `{
				"error": {
					"message": "Invalid audio file format",
					"type": "invalid_request_error"
				}
			}`
			w.Write([]byte(jsonResponse))
		}))
		defer server.Close()

		transcriber := NewGroqTranscriber("test_api_key")
		transcriber.apiBase = server.URL

		// Create temporary audio file
		tempDir := t.TempDir()
		audioFile := filepath.Join(tempDir, "test_audio.wav")
		err := os.WriteFile(audioFile, []byte("dummy audio data"), 0644)
		if err != nil {
			t.Fatalf("Failed to create test audio file: %v", err)
		}

		ctx := context.Background()
		_, err = transcriber.Transcribe(ctx, audioFile)

		if err == nil {
			t.Error("Transcribe() should return error for API error")
		}

		if !strings.Contains(err.Error(), "API error") {
			t.Errorf("Error message should mention API error, got: %v", err)
		}
	})

	t.Run("invalid JSON response", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			w.Header().Set("Content-Type", "application/json")
			w.WriteHeader(http.StatusOK)
			w.Write([]byte("invalid json"))
		}))
		defer server.Close()

		transcriber := NewGroqTranscriber("test_api_key")
		transcriber.apiBase = server.URL

		// Create temporary audio file
		tempDir := t.TempDir()
		audioFile := filepath.Join(tempDir, "test_audio.wav")
		err := os.WriteFile(audioFile, []byte("dummy audio data"), 0644)
		if err != nil {
			t.Fatalf("Failed to create test audio file: %v", err)
		}

		ctx := context.Background()
		_, err = transcriber.Transcribe(ctx, audioFile)

		if err == nil {
			t.Error("Transcribe() should return error for invalid JSON")
		}

		if !strings.Contains(err.Error(), "failed to unmarshal") {
			t.Errorf("Error message should mention unmarshal failure, got: %v", err)
		}
	})

	t.Run("context cancellation", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			// Simulate slow response
			time.Sleep(2 * time.Second)
			w.WriteHeader(http.StatusOK)
			w.Write([]byte(`{"text": "delayed response"}`))
		}))
		defer server.Close()

		transcriber := NewGroqTranscriber("test_api_key")
		transcriber.apiBase = server.URL

		// Create temporary audio file
		tempDir := t.TempDir()
		audioFile := filepath.Join(tempDir, "test_audio.wav")
		err := os.WriteFile(audioFile, []byte("dummy audio data"), 0644)
		if err != nil {
			t.Fatalf("Failed to create test audio file: %v", err)
		}

		// Create context with short timeout
		ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
		defer cancel()

		_, err = transcriber.Transcribe(ctx, audioFile)

		if err == nil {
			t.Error("Transcribe() should return error when context is cancelled")
		}
	})

	t.Run("minimal response", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			w.Header().Set("Content-Type", "application/json")
			w.WriteHeader(http.StatusOK)
			jsonResponse := `{"text": "Minimal transcription"}`
			w.Write([]byte(jsonResponse))
		}))
		defer server.Close()

		transcriber := NewGroqTranscriber("test_api_key")
		transcriber.apiBase = server.URL

		// Create temporary audio file
		tempDir := t.TempDir()
		audioFile := filepath.Join(tempDir, "test_audio.wav")
		err := os.WriteFile(audioFile, []byte("dummy audio data"), 0644)
		if err != nil {
			t.Fatalf("Failed to create test audio file: %v", err)
		}

		ctx := context.Background()
		result, err := transcriber.Transcribe(ctx, audioFile)

		if err != nil {
			t.Fatalf("Transcribe() failed: %v", err)
		}

		if result.Text != "Minimal transcription" {
			t.Errorf("Expected text 'Minimal transcription', got '%s'", result.Text)
		}

		if result.Language != "" {
			t.Errorf("Expected empty language, got '%s'", result.Language)
		}

		if result.Duration != 0 {
			t.Errorf("Expected zero duration, got %f", result.Duration)
		}
	})
}

// Test TranscriptionResponse
func TestTranscriptionResponse(t *testing.T) {
	t.Run("full response", func(t *testing.T) {
		resp := TranscriptionResponse{
			Text:     "Test transcription",
			Language: "en",
			Duration: 2.5,
		}

		if resp.Text != "Test transcription" {
			t.Errorf("Expected Text 'Test transcription', got '%s'", resp.Text)
		}

		if resp.Language != "en" {
			t.Errorf("Expected Language 'en', got '%s'", resp.Language)
		}

		if resp.Duration != 2.5 {
			t.Errorf("Expected Duration 2.5, got %f", resp.Duration)
		}
	})

	t.Run("minimal response", func(t *testing.T) {
		resp := TranscriptionResponse{
			Text: "Minimal",
		}

		if resp.Text != "Minimal" {
			t.Errorf("Expected Text 'Minimal', got '%s'", resp.Text)
		}

		if resp.Language != "" {
			t.Errorf("Expected empty Language, got '%s'", resp.Language)
		}

		if resp.Duration != 0 {
			t.Errorf("Expected zero Duration, got %f", resp.Duration)
		}
	})
}

// Test multipart form creation
func TestMultipartFormCreation(t *testing.T) {
	t.Run("audio file is included in request", func(t *testing.T) {
		requestReceived := false
		var receivedContentType string

		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			requestReceived = true
			receivedContentType = r.Header.Get("Content-Type")

			// Parse multipart form
			if err := r.ParseMultipartForm(10 << 20); err != nil {
				t.Errorf("Failed to parse multipart form: %v", err)
			}

			// Verify file was uploaded
			file, handler, err := r.FormFile("file")
			if err != nil {
				t.Errorf("Failed to get form file: %v", err)
			}
			defer file.Close()

			if handler == nil {
				t.Error("File handler should not be nil")
			}

			// Verify file content
			buffer := make([]byte, 100)
			n, _ := file.Read(buffer)
			content := string(buffer[:n])
			if content != "dummy audio data" {
				t.Errorf("Expected file content 'dummy audio data', got '%s'", content)
			}

			w.WriteHeader(http.StatusOK)
			w.Write([]byte(`{"text": "success"}`))
		}))
		defer server.Close()

		transcriber := NewGroqTranscriber("test_api_key")
		transcriber.apiBase = server.URL

		// Create temporary audio file
		tempDir := t.TempDir()
		audioFile := filepath.Join(tempDir, "test_audio.wav")
		err := os.WriteFile(audioFile, []byte("dummy audio data"), 0644)
		if err != nil {
			t.Fatalf("Failed to create test audio file: %v", err)
		}

		ctx := context.Background()
		_, err = transcriber.Transcribe(ctx, audioFile)

		if err != nil {
			t.Fatalf("Transcribe() failed: %v", err)
		}

		if !requestReceived {
			t.Error("Server should have received the request")
		}

		if !strings.Contains(receivedContentType, "multipart/form-data") {
			t.Errorf("Expected Content-Type to contain 'multipart/form-data', got '%s'", receivedContentType)
		}
	})
}

// Test concurrent transcriptions
func TestConcurrentTranscriptions(t *testing.T) {
	requestCount := 0
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		requestCount++
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusOK)
		w.Write([]byte(`{"text": "concurrent test"}`))
	}))
	defer server.Close()

	transcriber := NewGroqTranscriber("test_api_key")
	transcriber.apiBase = server.URL

	// Create temporary audio file
	tempDir := t.TempDir()
	audioFile := filepath.Join(tempDir, "test_audio.wav")
	err := os.WriteFile(audioFile, []byte("dummy audio data"), 0644)
	if err != nil {
		t.Fatalf("Failed to create test audio file: %v", err)
	}

	// Run concurrent transcriptions
	numGoroutines := 5
	done := make(chan bool, numGoroutines)

	for i := 0; i < numGoroutines; i++ {
		go func(id int) {
			ctx := context.Background()
			_, err := transcriber.Transcribe(ctx, audioFile)
			if err != nil {
				t.Errorf("Goroutine %d: Transcribe() failed: %v", id, err)
			}
			done <- true
		}(i)
	}

	// Wait for all goroutines to complete
	for i := 0; i < numGoroutines; i++ {
		<-done
	}

	// Verify all requests were handled
	if requestCount != numGoroutines {
		t.Errorf("Expected %d requests, got %d", numGoroutines, requestCount)
	}
}

// Benchmark tests
func BenchmarkNewGroqTranscriber(b *testing.B) {
	for i := 0; i < b.N; i++ {
		_ = NewGroqTranscriber("test_api_key")
	}
}

func BenchmarkIsAvailable(b *testing.B) {
	transcriber := NewGroqTranscriber("test_api_key")
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		transcriber.IsAvailable()
	}
}

func BenchmarkTranscribe(b *testing.B) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusOK)
		w.Write([]byte(`{"text": "benchmark test"}`))
	}))
	defer server.Close()

	transcriber := NewGroqTranscriber("test_api_key")
	transcriber.apiBase = server.URL

	// Create temporary audio file
	tempDir := b.TempDir()
	audioFile := filepath.Join(tempDir, "test_audio.wav")
	err := os.WriteFile(audioFile, []byte("dummy audio data"), 0644)
	if err != nil {
		b.Fatalf("Failed to create test audio file: %v", err)
	}

	ctx := context.Background()
	b.ResetTimer()

	for i := 0; i < b.N; i++ {
		_, _ = transcriber.Transcribe(ctx, audioFile)
	}
}
