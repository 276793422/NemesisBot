// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package voice

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"
	"time"
)

// TestTranscribe_ServerSuccess tests transcription with a mock server returning success.
func TestTranscribe_ServerSuccess(t *testing.T) {
	response := TranscriptionResponse{
		Text:     "Hello world",
		Language: "en",
		Duration: 3.5,
	}
	responseBody, _ := json.Marshal(response)

	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// Verify request headers
		if r.Header.Get("Authorization") != "Bearer test-key" {
			t.Error("expected Authorization header with Bearer token")
		}
		if r.Method != "POST" {
			t.Errorf("expected POST method, got %s", r.Method)
		}

		// Verify it's multipart form
		contentType := r.Header.Get("Content-Type")
		if contentType == "" {
			t.Error("expected Content-Type header")
		}

		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusOK)
		w.Write(responseBody)
	}))
	defer server.Close()

	// Create transcriber with custom base URL
	transcriber := NewGroqTranscriber("test-key")
	transcriber.apiBase = server.URL
	transcriber.httpClient = server.Client()

	// Create a test audio file
	tmpDir := t.TempDir()
	audioPath := filepath.Join(tmpDir, "test.wav")
	os.WriteFile(audioPath, []byte("fake audio data"), 0644)

	result, err := transcriber.Transcribe(context.Background(), audioPath)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result.Text != "Hello world" {
		t.Errorf("expected 'Hello world', got %q", result.Text)
	}
	if result.Language != "en" {
		t.Errorf("expected 'en', got %q", result.Language)
	}
	if result.Duration != 3.5 {
		t.Errorf("expected 3.5, got %f", result.Duration)
	}
}

// TestTranscribe_ServerError tests transcription with server error.
func TestTranscribe_ServerError(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusUnauthorized)
		w.Write([]byte(`{"error": "invalid API key"}`))
	}))
	defer server.Close()

	transcriber := NewGroqTranscriber("bad-key")
	transcriber.apiBase = server.URL
	transcriber.httpClient = server.Client()

	tmpDir := t.TempDir()
	audioPath := filepath.Join(tmpDir, "test.wav")
	os.WriteFile(audioPath, []byte("fake audio data"), 0644)

	_, err := transcriber.Transcribe(context.Background(), audioPath)
	if err == nil {
		t.Fatal("expected error for server error response")
	}
}

// TestTranscribe_FileNotFound tests transcription with missing file.
func TestTranscribe_FileNotFound(t *testing.T) {
	transcriber := NewGroqTranscriber("test-key")

	_, err := transcriber.Transcribe(context.Background(), "/nonexistent/audio.wav")
	if err == nil {
		t.Fatal("expected error for missing file")
	}
}

// TestTranscribe_InvalidJSON tests transcription with invalid JSON response.
func TestTranscribe_InvalidJSON(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusOK)
		w.Write([]byte(`not valid json`))
	}))
	defer server.Close()

	transcriber := NewGroqTranscriber("test-key")
	transcriber.apiBase = server.URL
	transcriber.httpClient = server.Client()

	tmpDir := t.TempDir()
	audioPath := filepath.Join(tmpDir, "test.wav")
	os.WriteFile(audioPath, []byte("fake audio data"), 0644)

	_, err := transcriber.Transcribe(context.Background(), audioPath)
	if err == nil {
		t.Fatal("expected error for invalid JSON")
	}
}

// TestTranscribe_CancelledContext tests transcription with cancelled context.
func TestTranscribe_CancelledContext(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// Slow response
		select {}
	}))
	defer server.Close()

	transcriber := NewGroqTranscriber("test-key")
	transcriber.apiBase = server.URL
	transcriber.httpClient = server.Client()

	tmpDir := t.TempDir()
	audioPath := filepath.Join(tmpDir, "test.wav")
	os.WriteFile(audioPath, []byte("fake audio data"), 0644)

	ctx, cancel := context.WithCancel(context.Background())
	cancel() // Cancel immediately

	_, err := transcriber.Transcribe(ctx, audioPath)
	if err == nil {
		t.Fatal("expected error for cancelled context")
	}
}

// TestTranscribe_EmptyFile tests transcription with an empty file.
func TestTranscribe_EmptyFile(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusOK)
		w.Write([]byte(`{"text": "", "language": "", "duration": 0}`))
	}))
	defer server.Close()

	transcriber := NewGroqTranscriber("test-key")
	transcriber.apiBase = server.URL
	transcriber.httpClient = server.Client()

	tmpDir := t.TempDir()
	audioPath := filepath.Join(tmpDir, "empty.wav")
	os.WriteFile(audioPath, []byte(""), 0644)

	result, err := transcriber.Transcribe(context.Background(), audioPath)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result.Text != "" {
		t.Errorf("expected empty text, got %q", result.Text)
	}
}

// TestTranscribe_WithLanguageAndDuration tests transcription with all response fields.
func TestTranscribe_WithLanguageAndDuration(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusOK)
		w.Write([]byte(`{"text": "Hello world, this is a test", "language": "english", "duration": 5.2}`))
	}))
	defer server.Close()

	transcriber := NewGroqTranscriber("test-key")
	transcriber.apiBase = server.URL
	transcriber.httpClient = server.Client()

	tmpDir := t.TempDir()
	audioPath := filepath.Join(tmpDir, "test.wav")
	os.WriteFile(audioPath, []byte("fake audio data"), 0644)

	result, err := transcriber.Transcribe(context.Background(), audioPath)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result.Text != "Hello world, this is a test" {
		t.Errorf("expected 'Hello world, this is a test', got %q", result.Text)
	}
	if result.Language != "english" {
		t.Errorf("expected 'english', got %q", result.Language)
	}
	if result.Duration != 5.2 {
		t.Errorf("expected 5.2, got %f", result.Duration)
	}
}

// TestTranscribe_500Error tests transcription with internal server error.
func TestTranscribe_500Error(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusInternalServerError)
		w.Write([]byte(`{"error": "internal server error"}`))
	}))
	defer server.Close()

	transcriber := NewGroqTranscriber("test-key")
	transcriber.apiBase = server.URL
	transcriber.httpClient = server.Client()

	tmpDir := t.TempDir()
	audioPath := filepath.Join(tmpDir, "test.wav")
	os.WriteFile(audioPath, []byte("fake audio data"), 0644)

	_, err := transcriber.Transcribe(context.Background(), audioPath)
	if err == nil {
		t.Fatal("expected error for 500 response")
	}
}

// TestTranscribe_WithContextTimeout tests transcription with context timeout.
func TestTranscribe_WithContextTimeout(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusOK)
		w.Write([]byte(`{"text": "Completed", "language": "en", "duration": 1.0}`))
	}))
	defer server.Close()

	transcriber := NewGroqTranscriber("test-key")
	transcriber.apiBase = server.URL
	transcriber.httpClient = server.Client()

	tmpDir := t.TempDir()
	audioPath := filepath.Join(tmpDir, "test.wav")
	os.WriteFile(audioPath, []byte("fake audio data"), 0644)

	// Use a context with adequate timeout
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	result, err := transcriber.Transcribe(ctx, audioPath)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result.Text != "Completed" {
		t.Errorf("expected 'Completed', got %q", result.Text)
	}
}
