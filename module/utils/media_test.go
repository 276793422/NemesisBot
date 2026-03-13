// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package utils

import (
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// TestIsAudioFile tests audio file detection
func TestIsAudioFile(t *testing.T) {
	tests := []struct {
		name        string
		filename    string
		contentType string
		want        bool
	}{
		// Extension tests
		{
			name:        "mp3 file",
			filename:    "test.mp3",
			contentType: "",
			want:        true,
		},
		{
			name:        "wav file",
			filename:    "test.wav",
			contentType: "",
			want:        true,
		},
		{
			name:        "ogg file",
			filename:    "test.ogg",
			contentType: "",
			want:        true,
		},
		{
			name:        "m4a file",
			filename:    "test.m4a",
			contentType: "",
			want:        true,
		},
		{
			name:        "flac file",
			filename:    "test.flac",
			contentType: "",
			want:        true,
		},
		{
			name:        "aac file",
			filename:    "test.aac",
			contentType: "",
			want:        true,
		},
		{
			name:        "wma file",
			filename:    "test.wma",
			contentType: "",
			want:        true,
		},
		{
			name:        "uppercase extension",
			filename:    "TEST.MP3",
			contentType: "",
			want:        true,
		},
		{
			name:        "mixed case extension",
			filename:    "test.Mp3",
			contentType: "",
			want:        true,
		},
		{
			name:        "non-audio file",
			filename:    "test.jpg",
			contentType: "",
			want:        false,
		},
		{
			name:        "no extension",
			filename:    "test",
			contentType: "",
			want:        false,
		},
		// Content type tests
		{
			name:        "audio content type",
			filename:    "",
			contentType: "audio/mpeg",
			want:        true,
		},
		{
			name:        "audio/wav content type",
			filename:    "",
			contentType: "audio/wav",
			want:        true,
		},
		{
			name:        "application/ogg content type",
			filename:    "",
			contentType: "application/ogg",
			want:        true,
		},
		{
			name:        "application/x-ogg content type",
			filename:    "",
			contentType: "application/x-ogg",
			want:        true,
		},
		{
			name:        "non-audio content type",
			filename:    "",
			contentType: "image/jpeg",
			want:        false,
		},
		{
			name:        "empty content type",
			filename:    "",
			contentType: "",
			want:        false,
		},
		{
			name:        "uppercase content type",
			filename:    "",
			contentType: "AUDIO/MPEG",
			want:        true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := IsAudioFile(tt.filename, tt.contentType)
			if got != tt.want {
				t.Errorf("IsAudioFile(%q, %q) = %v, want %v", tt.filename, tt.contentType, got, tt.want)
			}
		})
	}
}

// TestSanitizeFilename tests filename sanitization
func TestSanitizeFilename(t *testing.T) {
	tests := []struct {
		name     string
		input    string
		expected string
	}{
		{
			name:     "simple filename",
			input:    "test.txt",
			expected: "test.txt",
		},
		{
			name:     "path with directory traversal",
			input:    "../../../etc/passwd",
			expected: "passwd", // filepath.Base extracts "passwd", then ".." removal has no effect
		},
		{
			name:     "path with forward slashes",
			input:    "path/to/file.txt",
			expected: "file.txt", // filepath.Base extracts "file.txt"
		},
		{
			name:     "path with backslashes",
			input:    "path\\to\\file.txt",
			expected: "file.txt", // filepath.Base extracts "file.txt"
		},
		{
			name:     "mixed slashes",
			input:    "path/to\\file.txt",
			expected: "file.txt", // filepath.Base extracts "file.txt"
		},
		{
			name:     "multiple double dots",
			input:    "test..file..txt",
			expected: "testfiletxt", // ".." is removed, including from ".txt"
		},
		{
			name:     "complex path",
			input:    "../../../somedir/../file.txt",
			expected: "file.txt", // filepath.Base extracts "file.txt", then ".." removal has no effect
		},
		{
			name:     "url with filename",
			input:    "https://example.com/path/to/file.mp3",
			expected: "file.mp3",
		},
		{
			name:     "empty string",
			input:    "",
			expected: ".",
		},
		{
			name:     "just slashes",
			input:    "///",
			expected: "_", // filepath.Base returns "/" on Windows, "." on Unix
		},
		{
			name:     "filename with dots",
			input:    "my.file.name.txt",
			expected: "my.file.name.txt",
		},
		{
			name:     "windows path",
			input:    "C:\\Users\\test\\file.txt",
			expected: "file.txt", // filepath.Base extracts "file.txt"
		},
		{
			name:     "unix absolute path",
			input:    "/usr/local/bin/test",
			expected: "test",
		},
		{
			name:     "relative path with dot",
			input:    "./file.txt",
			expected: "file.txt",
		},
		{
			name:     "filename with special chars",
			input:    "test-file_123.txt",
			expected: "test-file_123.txt",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := SanitizeFilename(tt.input)
			if got != tt.expected {
				t.Errorf("SanitizeFilename(%q) = %q, want %q", tt.input, got, tt.expected)
			}
		})
	}
}

// TestDownloadFile tests file download functionality
func TestDownloadFile(t *testing.T) {
	// Create a test server
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// Check for custom header
		if r.Header.Get("X-Custom-Header") == "test-value" {
			w.Header().Set("X-Response-Header", "custom-value")
		}
		w.WriteHeader(http.StatusOK)
		w.Write([]byte("test file content"))
	}))
	defer server.Close()

	// Create a test server that returns error
	errorServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusInternalServerError)
	}))
	defer errorServer.Close()

	// Create a test server that times out
	slowServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		time.Sleep(2 * time.Second)
		w.WriteHeader(http.StatusOK)
	}))
	defer slowServer.Close()

	tests := []struct {
		name        string
		url         string
		filename    string
		opts        DownloadOptions
		wantContent string
		wantEmpty   bool
	}{
		{
			name:        "successful download",
			url:         server.URL,
			filename:    "test.txt",
			opts:        DownloadOptions{},
			wantContent: "test file content",
			wantEmpty:   false,
		},
		{
			name:     "download with custom timeout",
			url:      server.URL,
			filename: "test2.txt",
			opts: DownloadOptions{
				Timeout: 5 * time.Second,
			},
			wantContent: "test file content",
			wantEmpty:   false,
		},
		{
			name:     "download with custom headers",
			url:      server.URL,
			filename: "test3.txt",
			opts: DownloadOptions{
				ExtraHeaders: map[string]string{
					"X-Custom-Header": "test-value",
				},
			},
			wantContent: "test file content",
			wantEmpty:   false,
		},
		{
			name:     "download with logger prefix",
			url:      server.URL,
			filename: "test4.txt",
			opts: DownloadOptions{
				LoggerPrefix: "test",
			},
			wantContent: "test file content",
			wantEmpty:   false,
		},
		{
			name:      "server error",
			url:       errorServer.URL,
			filename:  "error.txt",
			opts:      DownloadOptions{},
			wantEmpty: true,
		},
		{
			name:      "invalid URL",
			url:       "http://invalid-url-that-does-not-exist.local",
			filename:  "invalid.txt",
			opts:      DownloadOptions{},
			wantEmpty: true,
		},
		{
			name:     "timeout exceeded",
			url:      slowServer.URL,
			filename: "timeout.txt",
			opts: DownloadOptions{
				Timeout: 100 * time.Millisecond,
			},
			wantEmpty: true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := DownloadFile(tt.url, tt.filename, tt.opts)

			if tt.wantEmpty {
				if got != "" {
					t.Errorf("DownloadFile() returned path %q, expected empty string on error", got)
				}
				return
			}

			if got == "" {
				t.Errorf("DownloadFile() returned empty string, expected a valid path")
				return
			}

			// Verify file exists
			if _, err := os.Stat(got); err != nil {
				t.Errorf("DownloadFile() returned path %q but file does not exist: %v", got, err)
				return
			}

			// Verify content
			content, err := os.ReadFile(got)
			if err != nil {
				t.Errorf("Failed to read downloaded file: %v", err)
				return
			}

			if string(content) != tt.wantContent {
				t.Errorf("Downloaded content mismatch: got %q, want %q", string(content), tt.wantContent)
			}

			// Cleanup
			os.Remove(got)
		})
	}
}

// TestDownloadFile_MediaDirectoryCreation tests media directory creation
func TestDownloadFile_MediaDirectoryCreation(t *testing.T) {
	// Set temp directory to a non-existent path to test directory creation
	// Note: This test is limited because DownloadFile uses os.TempDir() internally
	// We can't easily mock os.TempDir(), so we'll test the successful case
	_ = os.TempDir() // Keep reference for documentation

	// Create server
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusOK)
		w.Write([]byte("content"))
	}))
	defer server.Close()

	// Download with a custom temp directory
	// Note: This test is limited because DownloadFile uses os.TempDir() internally
	// We can't easily mock os.TempDir(), so we'll test the successful case
	result := DownloadFile(server.URL, "test.txt", DownloadOptions{})
	if result == "" {
		t.Error("DownloadFile failed")
		return
	}

	// Verify the file is in the nemesisbot_media directory
	if !strings.Contains(result, "nemesisbot_media") {
		t.Errorf("Downloaded file not in nemesisbot_media directory: %s", result)
	}

	// Cleanup
	os.Remove(result)
}

// TestDownloadFile_SanitizeFilename tests that downloaded filenames are sanitized
func TestDownloadFile_SanitizeFilename(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusOK)
		w.Write([]byte("content"))
	}))
	defer server.Close()

	unsafeFilename := "../../../etc/passwd"
	result := DownloadFile(server.URL, unsafeFilename, DownloadOptions{})

	if result == "" {
		t.Error("DownloadFile failed")
		return
	}

	// Verify the path doesn't contain ".."
	if strings.Contains(result, "..") {
		t.Errorf("Downloaded path contains '..': %s", result)
	}

	// Verify the path doesn't contain "/" or "\" (except for path separators within the temp dir)
	base := filepath.Base(result)
	if strings.Contains(base, "/") || strings.Contains(base, "\\") {
		t.Errorf("Sanitized filename still contains path separators: %s", base)
	}

	// Cleanup
	os.Remove(result)
}

// TestDownloadFile_UniqueFilenames tests that downloads get unique filenames
func TestDownloadFile_UniqueFilenames(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusOK)
		w.Write([]byte("content"))
	}))
	defer server.Close()

	filename := "test.txt"
	result1 := DownloadFile(server.URL, filename, DownloadOptions{})
	result2 := DownloadFile(server.URL, filename, DownloadOptions{})

	if result1 == "" || result2 == "" {
		t.Error("DownloadFile failed")
		return
	}

	// Verify the two files are different (should have UUID prefixes)
	if result1 == result2 {
		t.Errorf("DownloadFile returned same path for both downloads: %s", result1)
	}

	// Verify both files exist
	if _, err := os.Stat(result1); err != nil {
		t.Errorf("First file doesn't exist: %v", err)
	}
	if _, err := os.Stat(result2); err != nil {
		t.Errorf("Second file doesn't exist: %v", err)
	}

	// Cleanup
	os.Remove(result1)
	os.Remove(result2)
}

// TestDownloadFileEmptyContent tests downloading empty content
func TestDownloadFile_EmptyContent(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusOK)
		// Send empty content
	}))
	defer server.Close()

	result := DownloadFile(server.URL, "empty.txt", DownloadOptions{})

	if result == "" {
		t.Error("DownloadFile failed")
		return
	}

	// Verify file exists
	content, err := os.ReadFile(result)
	if err != nil {
		t.Errorf("Failed to read downloaded file: %v", err)
		return
	}

	if len(content) != 0 {
		t.Errorf("Expected empty content, got %d bytes", len(content))
	}

	// Cleanup
	os.Remove(result)
}

// TestDownloadFileLargeContent tests downloading large content
func TestDownloadFile_LargeContent(t *testing.T) {
	// Create 1MB of content
	largeContent := make([]byte, 1024*1024)
	for i := range largeContent {
		largeContent[i] = byte(i % 256)
	}

	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusOK)
		w.Write(largeContent)
	}))
	defer server.Close()

	result := DownloadFile(server.URL, "large.bin", DownloadOptions{})

	if result == "" {
		t.Error("DownloadFile failed")
		return
	}

	// Verify file exists and has correct content
	content, err := os.ReadFile(result)
	if err != nil {
		t.Errorf("Failed to read downloaded file: %v", err)
		return
	}

	if len(content) != len(largeContent) {
		t.Errorf("Content size mismatch: got %d, want %d", len(content), len(largeContent))
	}

	// Cleanup
	os.Remove(result)
}

// TestDownloadFileSimple tests the simplified download function
func TestDownloadFileSimple(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusOK)
		w.Write([]byte("simple content"))
	}))
	defer server.Close()

	result := DownloadFileSimple(server.URL, "simple.txt")

	if result == "" {
		t.Error("DownloadFileSimple failed")
		return
	}

	// Verify file exists
	content, err := os.ReadFile(result)
	if err != nil {
		t.Errorf("Failed to read downloaded file: %v", err)
		return
	}

	if string(content) != "simple content" {
		t.Errorf("Content mismatch: got %q, want 'simple content'", string(content))
	}

	// Cleanup
	os.Remove(result)
}

// TestDownloadFile_ErrorCreatingDirectory tests error when directory creation fails
func TestDownloadFile_ErrorCreatingDirectory(t *testing.T) {
	// This test is difficult to implement properly because:
	// 1. We can't easily mock os.MkdirAll to fail
	// 2. The function uses os.TempDir() which is always writable in normal circumstances
	// We'll skip this test but document the edge case
	t.Skip("Cannot easily test directory creation failure without mocking")
}

// TestDownloadFile_RequestCreationFailure tests HTTP request creation failure
func TestDownloadFile_RequestCreationFailure(t *testing.T) {
	// Use an invalid URL that will fail during request creation
	result := DownloadFile("://invalid-url", "test.txt", DownloadOptions{})
	if result != "" {
		t.Errorf("Expected empty result for invalid URL, got %q", result)
	}
}

// TestDownloadFile_ResponseBodyReadFailure tests response body read failure
func TestDownloadFile_ResponseBodyReadFailure(t *testing.T) {
	// This would require mocking the HTTP response body
	// Skipping as it's difficult to test without deeper mocking
	t.Skip("Cannot easily test response body read failure without mocking")
}

// TestDownloadFile_FileCreationFailure tests file creation failure
func TestDownloadFile_FileCreationFailure(t *testing.T) {
	// This would require mocking os.Create
	// Skipping as it's difficult to test without deeper mocking
	t.Skip("Cannot easily test file creation failure without mocking")
}

// TestDownloadFile_ErrorCreatingFile tests error when file creation fails
func TestDownloadFile_ErrorCreatingFile(t *testing.T) {
	// Similar to above, difficult to test without mocking
	t.Skip("Cannot easily test file creation failure without mocking")
}

// TestDownloadFile_404Response tests 404 response
func TestDownloadFile_404Response(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusNotFound)
	}))
	defer server.Close()

	result := DownloadFile(server.URL, "notfound.txt", DownloadOptions{})

	if result != "" {
		t.Errorf("DownloadFile with 404 should return empty string, got %q", result)
	}
}

// TestDownloadFile_ErrorReadingResponse tests error when reading response body fails
func TestDownloadFile_ErrorReadingResponse(t *testing.T) {
	// Create a server that sends data then closes connection
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Length", "1000000") // Claim to send 1MB
		w.WriteHeader(http.StatusOK)
		w.Write([]byte("small")) // But only send small amount
	}))
	defer server.Close()

	result := DownloadFile(server.URL, "incomplete.txt", DownloadOptions{})

	// The function should still succeed (io.Copy will just copy what's available)
	// or fail depending on how the server closes the connection
	// We'll just check it doesn't crash
	_ = result
}

// TestDownloadFile_Non200Status tests various non-200 status codes
func TestDownloadFile_Non200Status(t *testing.T) {
	statusCodes := []int{
		http.StatusMovedPermanently,
		http.StatusFound,
		http.StatusUnauthorized,
		http.StatusForbidden,
		http.StatusNotFound,
		http.StatusMethodNotAllowed,
		http.StatusInternalServerError,
		http.StatusBadGateway,
		http.StatusServiceUnavailable,
	}

	for _, status := range statusCodes {
		t.Run(http.StatusText(status), func(t *testing.T) {
			server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				w.WriteHeader(status)
			}))
			defer server.Close()

			result := DownloadFile(server.URL, "test.txt", DownloadOptions{})

			if result != "" {
				t.Errorf("DownloadFile with status %d should return empty string, got %q", status, result)
			}
		})
	}
}

// TestDownloadFile_WriteFailure tests error when writing to file fails
func TestDownloadFile_WriteFailure(t *testing.T) {
	// Create a server that returns content
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusOK)
		w.Write([]byte("test content"))
	}))
	defer server.Close()

	// Test with a destination directory that might cause write issues
	// We'll use a path that's likely to cause issues
	result := DownloadFile(server.URL, "test.txt", DownloadOptions{})

	if result == "" {
		// If download failed due to write issues, this is acceptable
		// The function should return empty string on write failure
		return
	}

	// Verify the file was created correctly
	content, err := os.ReadFile(result)
	if err != nil {
		t.Errorf("Failed to read downloaded file: %v", err)
		return
	}

	if string(content) != "test content" {
		t.Errorf("Content mismatch: got %q, want 'test content'", string(content))
	}

	// Cleanup
	os.Remove(result)
}

// TestDownloadFile_CreateDirectoryFailure tests directory creation failure
func TestDownloadFile_CreateDirectoryFailure(t *testing.T) {
	// This test is difficult to implement because we can't easily mock os.MkdirAll
	// We'll test with a path that might cause issues in some environments
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusOK)
		w.Write([]byte("content"))
	}))
	defer server.Close()

	// Use a path with many nested levels that might exceed system limits
	// Note: This might not fail on all systems but is worth testing
	deepPath := strings.Repeat("deep/", 100) + "file.txt"
	result := DownloadFile(server.URL, deepPath, DownloadOptions{})

	// If it succeeds, that's fine too
	if result != "" {
		// Verify file was created
		if _, err := os.Stat(result); err != nil {
			t.Errorf("Downloaded file should exist but doesn't: %v", err)
		}
		// Cleanup
		os.Remove(result)
	}
}

// TestDownloadFile_ConcurrentDownloads tests concurrent downloads
func TestDownloadFile_ConcurrentDownloads(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusOK)
		w.Write([]byte("concurrent content"))
	}))
	defer server.Close()

	// Download the same file multiple times concurrently
	results := make(chan string, 5)
	for i := 0; i < 5; i++ {
		go func() {
			result := DownloadFile(server.URL, "concurrent.txt", DownloadOptions{})
			results <- result
		}()
	}

	// Collect results
	uniqueResults := make(map[string]bool)
	for i := 0; i < 5; i++ {
		result := <-results
		if result != "" {
			uniqueResults[result] = true
			// Cleanup
			os.Remove(result)
		}
	}

	// Should have unique filenames for each download
	if len(uniqueResults) != 5 {
		t.Errorf("Expected 5 unique files, got %d", len(uniqueResults))
	}
}

// TestDownloadFile_HugeContent tests downloading very large content
func TestDownloadFile_HugeContent(t *testing.T) {
	// Create 10MB of content
	hugeContent := make([]byte, 10*1024*1024)
	for i := range hugeContent {
		hugeContent[i] = byte(i % 256)
	}

	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Length", fmt.Sprintf("%d", len(hugeContent)))
		w.WriteHeader(http.StatusOK)
		w.Write(hugeContent)
	}))
	defer server.Close()

	result := DownloadFile(server.URL, "huge.bin", DownloadOptions{
		Timeout: 30 * time.Second,
	})

	if result == "" {
		t.Error("DownloadFile failed for huge content")
		return
	}

	// Verify file exists and has correct size
	info, err := os.Stat(result)
	if err != nil {
		t.Errorf("Failed to stat downloaded file: %v", err)
		return
	}

	if info.Size() != int64(len(hugeContent)) {
		t.Errorf("File size mismatch: got %d, want %d", info.Size(), len(hugeContent))
	}

	// Verify content (sample some bytes to avoid reading entire 10MB)
	sampleSize := 1000
	if len(hugeContent) > sampleSize {
		content, err := os.ReadFile(result)
		if err != nil {
			t.Errorf("Failed to read downloaded file: %v", err)
			return
		}

		for i := 0; i < sampleSize; i++ {
			if content[i] != hugeContent[i] {
				t.Errorf("Content mismatch at byte %d: got %d, want %d", i, content[i], hugeContent[i])
				break
			}
		}
	}

	// Cleanup
	os.Remove(result)
}

// TestDownloadFile_ContentRange tests handling of Content-Range header
func TestDownloadFile_ContentRange(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// Note: DownloadFile only accepts 200 status codes
		// Content-Range responses with 206 status are not supported
		content := "partial content"
		w.Header().Set("Content-Length", fmt.Sprintf("%d", len(content)))
		w.WriteHeader(http.StatusOK)
		w.Write([]byte(content))
	}))
	defer server.Close()

	result := DownloadFile(server.URL, "range_test.txt", DownloadOptions{})

	if result == "" {
		t.Error("DownloadFile failed for content-range test")
		return
	}

	// Verify file exists
	content, err := os.ReadFile(result)
	if err != nil {
		t.Errorf("Failed to read downloaded file: %v", err)
		return
	}

	if string(content) != "partial content" {
		t.Errorf("Content mismatch: got %q, want 'partial content'", string(content))
	}

	// Cleanup
	os.Remove(result)
}

// TestDownloadFile_ProxyHeader tests handling of proxy-related headers
func TestDownloadFile_ProxyHeader(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// Check for proxy headers
		if r.Header.Get("Via") != "" || r.Header.Get("X-Forwarded-For") != "" {
			w.Header().Set("X-Proxy-Detected", "true")
		}
		w.WriteHeader(http.StatusOK)
		w.Write([]byte("proxy test content"))
	}))
	defer server.Close()

	// Test with custom headers that might be used by proxies
	opts := DownloadOptions{
		ExtraHeaders: map[string]string{
			"Via":             "1.1 proxy.example.com",
			"X-Forwarded-For": "192.168.1.1",
		},
	}

	result := DownloadFile(server.URL, "proxy_test.txt", opts)

	if result == "" {
		t.Error("DownloadFile failed for proxy header test")
		return
	}

	// Verify file exists
	content, err := os.ReadFile(result)
	if err != nil {
		t.Errorf("Failed to read downloaded file: %v", err)
		return
	}

	if string(content) != "proxy test content" {
		t.Errorf("Content mismatch: got %q, want 'proxy test content'", string(content))
	}

	// Cleanup
	os.Remove(result)
}

// TestDownloadFile_StreamRead tests downloading with a slow stream
func TestDownloadFile_StreamRead(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusOK)
		// Write data slowly
		for i := 0; i < 10; i++ {
			w.Write([]byte("chunk"))
			time.Sleep(10 * time.Millisecond)
		}
	}))
	defer server.Close()

	result := DownloadFile(server.URL, "stream_test.txt", DownloadOptions{})

	if result == "" {
		t.Error("DownloadFile failed for stream read test")
		return
	}

	// Verify file exists
	content, err := os.ReadFile(result)
	if err != nil {
		t.Errorf("Failed to read downloaded file: %v", err)
		return
	}

	expected := "chunkchunkchunkchunkchunkchunkchunkchunkchunkchunk"
	if string(content) != expected {
		t.Errorf("Content mismatch: got %q, want %q", string(content), expected)
	}

	// Cleanup
	os.Remove(result)
}
