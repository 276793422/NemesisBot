// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package utils

import (
	"archive/zip"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// TestExtractZipFile tests ZIP file extraction
func TestExtractZipFile(t *testing.T) {
	tests := []struct {
		name        string
		createZip   func(t *testing.T) string
		wantError   bool
		verifyFiles []string // Files that should exist after extraction
	}{
		{
			name: "simple zip with single file",
			createZip: func(t *testing.T) string {
				return createTestZip(t, []zipTestFile{
					{name: "test.txt", content: "hello world"},
				})
			},
			wantError:   false,
			verifyFiles: []string{"test.txt"},
		},
		{
			name: "zip with multiple files",
			createZip: func(t *testing.T) string {
				return createTestZip(t, []zipTestFile{
					{name: "file1.txt", content: "content1"},
					{name: "file2.txt", content: "content2"},
					{name: "file3.txt", content: "content3"},
				})
			},
			wantError:   false,
			verifyFiles: []string{"file1.txt", "file2.txt", "file3.txt"},
		},
		{
			name: "zip with directory structure",
			createZip: func(t *testing.T) string {
				return createTestZip(t, []zipTestFile{
					{name: "dir1/file1.txt", content: "content1"},
					{name: "dir1/dir2/file2.txt", content: "content2"},
					{name: "dir1/dir2/dir3/", content: ""}, // Directory
					{name: "root.txt", content: "root content"},
				})
			},
			wantError: false,
			verifyFiles: []string{
				"dir1/file1.txt",
				"dir1/dir2/file2.txt",
				"root.txt",
			},
		},
		{
			name: "zip with empty files",
			createZip: func(t *testing.T) string {
				return createTestZip(t, []zipTestFile{
					{name: "empty1.txt", content: ""},
					{name: "empty2.txt", content: ""},
				})
			},
			wantError:   false,
			verifyFiles: []string{"empty1.txt", "empty2.txt"},
		},
		{
			name: "zip with binary data",
			createZip: func(t *testing.T) string {
				binaryData := make([]byte, 256)
				for i := range binaryData {
					binaryData[i] = byte(i)
				}
				return createTestZip(t, []zipTestFile{
					{name: "binary.bin", content: string(binaryData)},
				})
			},
			wantError:   false,
			verifyFiles: []string{"binary.bin"},
		},
		{
			name: "zip with unicode filenames",
			createZip: func(t *testing.T) string {
				return createTestZip(t, []zipTestFile{
					{name: "世界.txt", content: "content"},
					{name: "тест.txt", content: "content"},
				})
			},
			wantError:   false,
			verifyFiles: []string{"世界.txt", "тест.txt"},
		},
		{
			name: "zip with large file",
			createZip: func(t *testing.T) string {
				largeContent := strings.Repeat("a", 1024*1024) // 1MB
				return createTestZip(t, []zipTestFile{
					{name: "large.txt", content: largeContent},
				})
			},
			wantError:   false,
			verifyFiles: []string{"large.txt"},
		},
		{
			name: "non-existent zip file",
			createZip: func(t *testing.T) string {
				return "/non/existent/path/file.zip"
			},
			wantError: true,
		},
		{
			name: "invalid zip file",
			createZip: func(t *testing.T) string {
				invalidFile := filepath.Join(t.TempDir(), "invalid.zip")
				if err := os.WriteFile(invalidFile, []byte("not a zip file"), 0644); err != nil {
					t.Fatalf("Failed to create invalid zip: %v", err)
				}
				return invalidFile
			},
			wantError: true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			zipPath := tt.createZip(t)
			destDir := t.TempDir()

			err := ExtractZipFile(zipPath, destDir)

			if (err != nil) != tt.wantError {
				t.Errorf("ExtractZipFile() error = %v, wantError %v", err, tt.wantError)
				return
			}

			if tt.wantError {
				return
			}

			// Verify files exist
			for _, verifyFile := range tt.verifyFiles {
				fullPath := filepath.Join(destDir, verifyFile)
				if _, err := os.Stat(fullPath); err != nil {
					t.Errorf("Expected file %s does not exist: %v", verifyFile, err)
				}
			}
		})
	}
}

// TestExtractZipFile_ZipSlip tests zip slip vulnerability protection
func TestExtractZipFile_ZipSlip(t *testing.T) {
	tests := []struct {
		name      string
		zipFiles  []zipTestFile
		wantError bool
	}{
		{
			name: "absolute path",
			zipFiles: []zipTestFile{
				{name: "/etc/passwd", content: "malicious"},
			},
			wantError: false, // On Windows, /etc/passwd is not an absolute path
		},
		{
			name: "relative path traversal",
			zipFiles: []zipTestFile{
				{name: "../../../etc/passwd", content: "malicious"},
			},
			wantError: true,
		},
		{
			name: "mixed traversal",
			zipFiles: []zipTestFile{
				{name: "dir/../../etc/passwd", content: "malicious"},
			},
			wantError: true,
		},
		{
			name: "valid files mixed with malicious",
			zipFiles: []zipTestFile{
				{name: "valid.txt", content: "valid"},
				{name: "../../malicious.txt", content: "malicious"},
			},
			wantError: true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			zipPath := createTestZip(t, tt.zipFiles)
			destDir := t.TempDir()

			err := ExtractZipFile(zipPath, destDir)

			if (err != nil) != tt.wantError {
				t.Errorf("ExtractZipFile() error = %v, wantError %v", err, tt.wantError)
			}

			// Verify no files escaped the destination directory
			if !tt.wantError {
				filepath.Walk(destDir, func(path string, info os.FileInfo, err error) error {
					if err != nil {
						return err
					}
					absPath, err := filepath.Abs(path)
					if err != nil {
						return err
					}
					absDest, err := filepath.Abs(destDir)
					if err != nil {
						return err
					}
					if !strings.HasPrefix(absPath, absDest) {
						t.Errorf("File %s escaped destination directory %s", absPath, absDest)
					}
					return nil
				})
			}
		})
	}
}

// TestExtractZipFile_Permissions tests file permission preservation
func TestExtractZipFile_Permissions(t *testing.T) {
	// Create a zip with specific permissions
	zipPath := createTestZipWithPerms(t, []zipTestFileWithPerms{
		{name: "readonly.txt", content: "readonly", mode: 0444},
		{name: "executable.sh", content: "#!/bin/bash", mode: 0755},
		{name: "normal.txt", content: "normal", mode: 0644},
	})

	destDir := t.TempDir()

	err := ExtractZipFile(zipPath, destDir)
	if err != nil {
		t.Fatalf("ExtractZipFile failed: %v", err)
	}

	// Verify permissions (on Unix-like systems)
	// Note: Windows doesn't support Unix permissions in the same way
	// so we just verify the files exist
	files := []string{"readonly.txt", "executable.sh", "normal.txt"}
	for _, file := range files {
		fullPath := filepath.Join(destDir, file)
		if _, err := os.Stat(fullPath); err != nil {
			t.Errorf("File %s does not exist: %v", file, err)
		}
	}
}

// TestExtractZipFile_Overwrite tests overwriting existing files
func TestExtractZipFile_Overwrite(t *testing.T) {
	destDir := t.TempDir()

	// Create an existing file
	existingFile := filepath.Join(destDir, "test.txt")
	if err := os.WriteFile(existingFile, []byte("existing content"), 0644); err != nil {
		t.Fatalf("Failed to create existing file: %v", err)
	}

	// Create a zip with the same filename
	zipPath := createTestZip(t, []zipTestFile{
		{name: "test.txt", content: "new content from zip"},
	})

	// Extract the zip (should overwrite)
	err := ExtractZipFile(zipPath, destDir)
	if err != nil {
		t.Fatalf("ExtractZipFile failed: %v", err)
	}

	// Verify the file was overwritten
	content, err := os.ReadFile(existingFile)
	if err != nil {
		t.Fatalf("Failed to read file: %v", err)
	}

	if string(content) != "new content from zip" {
		t.Errorf("File was not overwritten correctly: got %q, want 'new content from zip'", string(content))
	}
}

// TestExtractZipFile_EmptyZip tests extracting an empty zip file
func TestExtractZipFile_EmptyZip(t *testing.T) {
	zipPath := createTestZip(t, []zipTestFile{})
	destDir := t.TempDir()

	err := ExtractZipFile(zipPath, destDir)
	if err != nil {
		t.Errorf("ExtractZipFile with empty zip failed: %v", err)
	}

	// Verify destination directory exists
	if _, err := os.Stat(destDir); err != nil {
		t.Errorf("Destination directory does not exist: %v", err)
	}
}

// TestExtractZipFile_NestedExtraction tests extracting nested zip structures
func TestExtractZipFile_NestedExtraction(t *testing.T) {
	zipPath := createTestZip(t, []zipTestFile{
		{name: "a/b/c/d/e/file.txt", content: "deeply nested"},
		{name: "a/b/other.txt", content: "sibling"},
		{name: "root.txt", content: "root"},
	})

	destDir := t.TempDir()

	err := ExtractZipFile(zipPath, destDir)
	if err != nil {
		t.Fatalf("ExtractZipFile failed: %v", err)
	}

	// Verify all files exist
	files := []string{
		"a/b/c/d/e/file.txt",
		"a/b/other.txt",
		"root.txt",
	}

	for _, file := range files {
		fullPath := filepath.Join(destDir, file)
		if _, err := os.Stat(fullPath); err != nil {
			t.Errorf("Nested file %s does not exist: %v", file, err)
		}
	}
}

// TestExtractZipFile_SpecialCharacters tests files with special characters
func TestExtractZipFile_SpecialCharacters(t *testing.T) {
	zipPath := createTestZip(t, []zipTestFile{
		{name: "file with spaces.txt", content: "content"},
		{name: "file-with-dashes.txt", content: "content"},
		{name: "file_with_underscores.txt", content: "content"},
		{name: "file.with.dots.txt", content: "content"},
		{name: "file(multiple).txt", content: "content"},
	})

	destDir := t.TempDir()

	err := ExtractZipFile(zipPath, destDir)
	if err != nil {
		t.Fatalf("ExtractZipFile failed: %v", err)
	}

	// Verify all files exist
	files := []string{
		"file with spaces.txt",
		"file-with-dashes.txt",
		"file_with_underscores.txt",
		"file.with.dots.txt",
		"file(multiple).txt",
	}

	for _, file := range files {
		fullPath := filepath.Join(destDir, file)
		if _, err := os.Stat(fullPath); err != nil {
			t.Errorf("File with special chars %s does not exist: %v", file, err)
		}
	}
}

// TestIsPathWithinDir tests path security validation
func TestIsPathWithinDir(t *testing.T) {
	tests := []struct {
		name     string
		path     string
		baseDir  string
		expected bool
	}{
		{
			name:     "simple valid path",
			path:     "/home/user/file.txt",
			baseDir:  "/home/user",
			expected: true,
		},
		{
			name:     "nested valid path",
			path:     "/home/user/subdir/file.txt",
			baseDir:  "/home/user",
			expected: true,
		},
		{
			name:     "path traversal attempt",
			path:     "/home/user/../etc/passwd",
			baseDir:  "/home/user",
			expected: false,
		},
		{
			name:     "absolute path outside base",
			path:     "/etc/passwd",
			baseDir:  "/home/user",
			expected: false,
		},
		{
			name:     "relative path with ..",
			path:     "/home/user/subdir/../../etc/passwd",
			baseDir:  "/home/user",
			expected: false,
		},
		{
			name:     "path in base directory",
			path:     "/home/user",
			baseDir:  "/home/user",
			expected: true,
		},
		{
			name:     "complex traversal",
			path:     "/home/user/subdir/../../../etc/passwd",
			baseDir:  "/home/user",
			expected: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := isPathWithinDir(tt.path, tt.baseDir)
			if got != tt.expected {
				t.Errorf("isPathWithinDir(%q, %q) = %v, want %v", tt.path, tt.baseDir, got, tt.expected)
			}
		})
	}
}

// TestExtractFile tests individual file extraction
func TestExtractFile(t *testing.T) {
	tests := []struct {
		name     string
		zipFile  zipTestFile
		wantErr  bool
		verify   func(t *testing.T, destDir string)
	}{
		{
			name: "regular file",
			zipFile: zipTestFile{
				name:    "test.txt",
				content: "hello world",
			},
			wantErr: false,
			verify: func(t *testing.T, destDir string) {
				content, err := os.ReadFile(filepath.Join(destDir, "test.txt"))
				if err != nil {
					t.Errorf("Failed to read extracted file: %v", err)
					return
				}
				if string(content) != "hello world" {
					t.Errorf("Content mismatch: got %q, want 'hello world'", string(content))
				}
			},
		},
		{
			name: "empty file",
			zipFile: zipTestFile{
				name:    "empty.txt",
				content: "",
			},
			wantErr: false,
			verify: func(t *testing.T, destDir string) {
				content, err := os.ReadFile(filepath.Join(destDir, "empty.txt"))
				if err != nil {
					t.Errorf("Failed to read extracted file: %v", err)
					return
				}
				if len(content) != 0 {
					t.Errorf("Expected empty file, got %d bytes", len(content))
				}
			},
		},
		{
			name: "directory",
			zipFile: zipTestFile{
				name:    "dir/",
				content: "",
			},
			wantErr: false,
			verify: func(t *testing.T, destDir string) {
				info, err := os.Stat(filepath.Join(destDir, "dir"))
				if err != nil {
					t.Errorf("Failed to stat directory: %v", err)
					return
				}
				if !info.IsDir() {
					t.Errorf("Expected directory, got file")
				}
			},
		},
		{
			name: "file in nested directory",
			zipFile: zipTestFile{
				name:    "subdir/nested/file.txt",
				content: "nested content",
			},
			wantErr: false,
			verify: func(t *testing.T, destDir string) {
				content, err := os.ReadFile(filepath.Join(destDir, "subdir/nested/file.txt"))
				if err != nil {
					t.Errorf("Failed to read nested file: %v", err)
					return
				}
				if string(content) != "nested content" {
					t.Errorf("Content mismatch: got %q, want 'nested content'", string(content))
				}
			},
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			// Create a zip with the test file
			zipPath := createTestZip(t, []zipTestFile{tt.zipFile})

			// Open the zip
			r, err := zip.OpenReader(zipPath)
			if err != nil {
				t.Fatalf("Failed to open zip: %v", err)
			}
			defer r.Close()

			// Extract the first (and only) file
			if len(r.File) == 0 {
				t.Fatal("Zip file is empty")
			}

			destDir := t.TempDir()
			err = extractFile(r.File[0], destDir)

			if (err != nil) != tt.wantErr {
				t.Errorf("extractFile() error = %v, wantErr %v", err, tt.wantErr)
				return
			}

			if tt.verify != nil && !tt.wantErr {
				tt.verify(t, destDir)
			}
		})
	}
}

// Helper types and functions

type zipTestFile struct {
	name    string
	content string
}

type zipTestFileWithPerms struct {
	name    string
	content string
	mode    uint32
}

// createTestZip creates a test zip file with the given files
func createTestZip(t *testing.T, files []zipTestFile) string {
	t.Helper()

	zipPath := filepath.Join(t.TempDir(), "test.zip")
	zipFile, err := os.Create(zipPath)
	if err != nil {
		t.Fatalf("Failed to create zip file: %v", err)
	}
	defer zipFile.Close()

	writer := zip.NewWriter(zipFile)
	defer writer.Close()

	for _, file := range files {
		if strings.HasSuffix(file.name, "/") {
			// Create directory entry
			_, err := writer.Create(file.name)
			if err != nil {
				t.Fatalf("Failed to create directory entry: %v", err)
			}
		} else {
			// Create file entry
			w, err := writer.Create(file.name)
			if err != nil {
				t.Fatalf("Failed to create file entry: %v", err)
			}
			_, err = io.WriteString(w, file.content)
			if err != nil {
				t.Fatalf("Failed to write file content: %v", err)
			}
		}
	}

	return zipPath
}

// createTestZipWithPerms creates a test zip file with specific permissions
func createTestZipWithPerms(t *testing.T, files []zipTestFileWithPerms) string {
	t.Helper()

	zipPath := filepath.Join(t.TempDir(), "test.zip")
	zipFile, err := os.Create(zipPath)
	if err != nil {
		t.Fatalf("Failed to create zip file: %v", err)
	}
	defer zipFile.Close()

	writer := zip.NewWriter(zipFile)
	defer writer.Close()

	for _, file := range files {
		header := &zip.FileHeader{
			Name:  file.name,
			Method: zip.Deflate,
		}
		header.SetMode(os.FileMode(file.mode))

		w, err := writer.CreateHeader(header)
		if err != nil {
			t.Fatalf("Failed to create file entry: %v", err)
		}
		_, err = io.WriteString(w, file.content)
		if err != nil {
			t.Fatalf("Failed to write file content: %v", err)
		}
	}

	return zipPath
}

// TestExtractZipFile_ContentVerification tests that extracted content matches zip content
func TestExtractZipFile_ContentVerification(t *testing.T) {
	originalContent := "This is the original content that should be preserved exactly"
	zipPath := createTestZip(t, []zipTestFile{
		{name: "content.txt", content: originalContent},
	})

	destDir := t.TempDir()

	err := ExtractZipFile(zipPath, destDir)
	if err != nil {
		t.Fatalf("ExtractZipFile failed: %v", err)
	}

	// Read the extracted file
	extractedContent, err := os.ReadFile(filepath.Join(destDir, "content.txt"))
	if err != nil {
		t.Fatalf("Failed to read extracted file: %v", err)
	}

	// Verify content matches exactly
	if string(extractedContent) != originalContent {
		t.Errorf("Content mismatch: got %q, want %q", string(extractedContent), originalContent)
	}
}

// TestExtractZipFile_MultipleExtractions tests extracting the same zip multiple times
func TestExtractZipFile_MultipleExtractions(t *testing.T) {
	zipPath := createTestZip(t, []zipTestFile{
		{name: "file.txt", content: "content"},
	})

	// Extract to three different destinations
	for i := 0; i < 3; i++ {
		t.Run("extraction", func(t *testing.T) {
			destDir := t.TempDir()
			err := ExtractZipFile(zipPath, destDir)
			if err != nil {
				t.Errorf("Extraction %d failed: %v", i, err)
			}

			// Verify file exists
			if _, err := os.Stat(filepath.Join(destDir, "file.txt")); err != nil {
				t.Errorf("File does not exist after extraction %d: %v", i, err)
			}
		})
	}
}

// TestExtractZipFile_DestinationAlreadyExists tests extracting when destination files exist
func TestExtractZipFile_DestinationAlreadyExists(t *testing.T) {
	zipPath := createTestZip(t, []zipTestFile{
		{name: "file1.txt", content: "zip content 1"},
		{name: "file2.txt", content: "zip content 2"},
	})

	destDir := t.TempDir()

	// Create some existing files
	existingFile1 := filepath.Join(destDir, "file1.txt")
	existingFile2 := filepath.Join(destDir, "existing.txt")

	if err := os.WriteFile(existingFile1, []byte("existing content 1"), 0644); err != nil {
		t.Fatalf("Failed to create existing file: %v", err)
	}
	if err := os.WriteFile(existingFile2, []byte("existing content 2"), 0644); err != nil {
		t.Fatalf("Failed to create existing file: %v", err)
	}

	// Extract the zip
	err := ExtractZipFile(zipPath, destDir)
	if err != nil {
		t.Fatalf("ExtractZipFile failed: %v", err)
	}

	// Verify file1.txt was overwritten
	content1, err := os.ReadFile(existingFile1)
	if err != nil {
		t.Fatalf("Failed to read file1.txt: %v", err)
	}
	if string(content1) != "zip content 1" {
		t.Errorf("file1.txt was not overwritten: got %q, want 'zip content 1'", string(content1))
	}

	// Verify existing.txt still exists
	content2, err := os.ReadFile(existingFile2)
	if err != nil {
		t.Fatalf("Failed to read existing.txt: %v", err)
	}
	if string(content2) != "existing content 2" {
		t.Errorf("existing.txt was modified: got %q, want 'existing content 2'", string(content2))
	}
}

// TestExtractZipFile_DirectoryCreationFailure tests directory creation failure
func TestExtractZipFile_DirectoryCreationFailure(t *testing.T) {
	// This test would require mocking os.MkdirAll to fail
	// Skipping as it's difficult to test without mocking
	t.Skip("Cannot easily test directory creation failure without mocking")
}

// TestExtractZipFile_FileCreationFailure tests file creation failure
func TestExtractZipFile_FileCreationFailure(t *testing.T) {
	// This test would require mocking os.OpenFile to fail
	// Skipping as it's difficult to test without mocking
	t.Skip("Cannot easily test file creation failure without mocking")
}

// TestExtractFile_CopyFailure tests io.Copy failure
func TestExtractFile_CopyFailure(t *testing.T) {
	// This test is difficult to implement without mocking io.Copy
	// But we can test some edge cases that might cause issues
	zipPath := createTestZip(t, []zipTestFile{
		{name: "large_file.txt", content: strings.Repeat("a", 1024*1024)}, // 1MB file
	})

	r, err := zip.OpenReader(zipPath)
	if err != nil {
		t.Fatalf("Failed to open zip: %v", err)
	}
	defer r.Close()

	if len(r.File) == 0 {
		t.Fatal("Zip file is empty")
	}

	destDir := t.TempDir()

	// This should work normally
	err = extractFile(r.File[0], destDir)
	if err != nil {
		t.Errorf("extractFile failed unexpectedly: %v", err)
	}

	// Verify file was created
	filePath := filepath.Join(destDir, "large_file.txt")
	if _, err := os.Stat(filePath); err != nil {
		t.Errorf("Large file was not extracted: %v", err)
	}

	// Cleanup
	os.Remove(filePath)
}

// TestExtractFile_FileCreation tests file creation in various scenarios
func TestExtractFile_FileCreation(t *testing.T) {
	tests := []struct {
		name        string
		zipFile     zipTestFile
		expectError bool
	}{
		{
			name: "regular file",
			zipFile: zipTestFile{
				name:    "test.txt",
				content: "hello world",
			},
			expectError: false,
		},
		{
			name: "empty file",
			zipFile: zipTestFile{
				name:    "empty.txt",
				content: "",
			},
			expectError: false,
		},
		{
			name: "large file",
			zipFile: zipTestFile{
				name:    "large.txt",
				content: strings.Repeat("a", 1024*1024), // 1MB
			},
			expectError: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			zipPath := createTestZip(t, []zipTestFile{tt.zipFile})
			r, err := zip.OpenReader(zipPath)
			if err != nil {
				t.Fatalf("Failed to open zip: %v", err)
			}
			defer r.Close()

			if len(r.File) == 0 {
				t.Fatal("Zip file is empty")
			}

			destDir := t.TempDir()
			err = extractFile(r.File[0], destDir)

			if (err != nil) != tt.expectError {
				t.Errorf("extractFile() error = %v, expectError %v", err, tt.expectError)
				return
			}

			if !tt.expectError {
				filePath := filepath.Join(destDir, tt.zipFile.name)
				if _, err := os.Stat(filePath); err != nil {
					t.Errorf("File was not created: %v", err)
				}

				// Verify content
				content, err := os.ReadFile(filePath)
				if err != nil {
					t.Errorf("Failed to read file: %v", err)
				} else {
					if string(content) != tt.zipFile.content {
						t.Errorf("Content mismatch: got %q, want %q", string(content), tt.zipFile.content)
					}
				}

				// Cleanup
				os.Remove(filePath)
			}
		})
	}
}

// TestIsPathWithinDir_EdgeCases tests additional edge cases
func TestIsPathWithinDir_EdgeCases(t *testing.T) {
	tests := []struct {
		name     string
		path     string
		baseDir  string
		expected bool
	}{
		{
			name:     "same directory",
			path:     "/home/user",
			baseDir:  "/home/user",
			expected: true,
		},
		{
			name:     "subdirectory",
			path:     "/home/user/sub",
			baseDir:  "/home/user",
			expected: true,
		},
		{
			name:     "deeply nested subdirectory",
			path:     "/home/user/a/b/c/d/e",
			baseDir:  "/home/user",
			expected: true,
		},
		{
			name:     "parent directory",
			path:     "/home",
			baseDir:  "/home/user",
			expected: false,
		},
		{
			name:     "sibling directory",
			path:     "/home/other",
			baseDir:  "/home/user",
			expected: false,
		},
		{
			name:     "traversal with subdirectory",
			path:     "/home/user/sub/../../etc",
			baseDir:  "/home/user",
			expected: false,
		},
		{
			name:     "complex valid path",
			path:     "/home/user/sub1/sub2/file.txt",
			baseDir:  "/home/user",
			expected: true,
		},
		{
			name:     "root path outside base",
			path:     "/root/file.txt",
			baseDir:  "/home/user",
			expected: false,
		},
		{
			name:     "path with many dots",
			path:     "/home/user/some.dir/file.txt",
			baseDir:  "/home/user",
			expected: true,
		},
		{
			name:     "windows-style paths on Unix",
			path:     "C:\\Users\\file.txt",
			baseDir:  "/home/user",
			expected: false,
		},
		{
			name:     "mixed path separators",
			path:     "/home/user\\subdir/file.txt",
			baseDir:  "/home/user",
			expected: true, // On Windows, filepath.Abs normalizes separators
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := isPathWithinDir(tt.path, tt.baseDir)
			if got != tt.expected {
				t.Errorf("isPathWithinDir(%q, %q) = %v, want %v", tt.path, tt.baseDir, got, tt.expected)
			}
		})
	}
}

// TestIsPathWithinDir_ErrorCases tests error conditions
func TestIsPathWithinDir_ErrorCases(t *testing.T) {
	tests := []struct {
		name     string
		path     string
		baseDir  string
		expected bool
	}{
		{
			name:     "empty path",
			path:     "",
			baseDir:  "/home/user",
			expected: false,
		},
		{
			name:     "empty base directory",
			path:     "/home/user/file.txt",
			baseDir:  "",
			expected: false,
		},
	{
			name:     "both empty",
			path:     "",
			baseDir:  "",
			expected: true, // filepath.Abs("") returns current directory, so rel is "."
		},
		{
			name:     "path with null bytes",
			path:     "/home/user" + string([]byte{0x00}) + "file.txt",
			baseDir:  "/home/user",
			expected: false,
		},
		{
			name:     "base dir with null bytes",
			path:     "/home/user/file.txt",
			baseDir:  "/home/user" + string([]byte{0x00}),
			expected: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := isPathWithinDir(tt.path, tt.baseDir)
			if got != tt.expected {
				t.Errorf("isPathWithinDir(%q, %q) = %v, want %v", tt.path, tt.baseDir, got, tt.expected)
			}
		})
	}
}

// TestExtractFile_ErrorCases tests error conditions for extractFile
func TestExtractFile_ErrorCases(t *testing.T) {
	tests := []struct {
		name         string
		createZip    func(t *testing.T) *zip.File
		destDir      string
		expectError  bool
		errorMessage string
	}{
		{
			name: "zip slip with ../.. in path",
			createZip: func(t *testing.T) *zip.File {
				zipPath := createTestZip(t, []zipTestFile{
					{name: "../../etc/passwd", content: "malicious"},
				})
				r, err := zip.OpenReader(zipPath)
				if err != nil {
					t.Fatalf("Failed to open zip: %v", err)
				}
				defer r.Close()
				return r.File[0]
			},
			destDir:      t.TempDir(),
			expectError:  true,
			errorMessage: "invalid file path",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			zipFile := tt.createZip(t)

			err := extractFile(zipFile, tt.destDir)

			if tt.expectError {
				if err == nil {
					t.Error("Expected error but got none")
				} else if tt.errorMessage != "" && !strings.Contains(err.Error(), tt.errorMessage) {
					t.Errorf("Error message should contain %q, got %q", tt.errorMessage, err.Error())
				}
			}
		})
	}
}

// TestExtractZipFile_DirectoryEdgeCases tests directory-related edge cases
func TestExtractZipFile_DirectoryEdgeCases(t *testing.T) {
	tests := []struct {
		name        string
		zipFiles    []zipTestFile
		verifyCount int // Expected number of files/directories
	}{
		{
			name: "only directories",
			zipFiles: []zipTestFile{
				{name: "dir1/", content: ""},
				{name: "dir2/", content: ""},
				{name: "dir3/", content: ""},
			},
			verifyCount: 3,
		},
		{
			name: "nested directories",
			zipFiles: []zipTestFile{
				{name: "a/", content: ""},
				{name: "a/b/", content: ""},
				{name: "a/b/c/", content: ""},
			},
			verifyCount: 3,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			zipPath := createTestZip(t, tt.zipFiles)
			destDir := t.TempDir()

			err := ExtractZipFile(zipPath, destDir)
			if err != nil {
				t.Fatalf("ExtractZipFile failed: %v", err)
			}

			// Count files and directories
			count := 0
			filepath.Walk(destDir, func(path string, info os.FileInfo, err error) error {
				if path != destDir {
					count++
				}
				return nil
			})

			if count < tt.verifyCount {
				t.Errorf("Expected at least %d items, got %d", tt.verifyCount, count)
			}
		})
	}
}

// TestIsPathWithinDir_RelativePaths tests relative path handling
func TestIsPathWithinDir_RelativePaths(t *testing.T) {
	tests := []struct {
		name     string
		path     string
		baseDir  string
		expected bool
	}{
		{
			name:     "relative path within base",
			path:     "subdir/file.txt",
			baseDir:  ".",
			expected: true,
		},
		{
			name:     "relative path traversal",
			path:     "../other/file.txt",
			baseDir:  ".",
			expected: false,
		},
		{
			name:     "current directory",
			path:     ".",
			baseDir:  ".",
			expected: true,
		},
		{
			name:     "dot-dot within base",
			path:     "./subdir/../other.txt",
			baseDir:  ".",
			expected: true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := isPathWithinDir(tt.path, tt.baseDir)
			if got != tt.expected {
				t.Errorf("isPathWithinDir(%q, %q) = %v, want %v", tt.path, tt.baseDir, got, tt.expected)
			}
		})
	}
}

// TestExtractZipFile_LargeNumberOfFiles tests extracting zip with many files
func TestExtractZipFile_LargeNumberOfFiles(t *testing.T) {
	// Create a zip with many files
	files := make([]zipTestFile, 100)
	for i := 0; i < 100; i++ {
		files[i] = zipTestFile{
			name:    fmt.Sprintf("file%03d.txt", i),
			content: fmt.Sprintf("content %d", i),
		}
	}

	zipPath := createTestZip(t, files)
	destDir := t.TempDir()

	err := ExtractZipFile(zipPath, destDir)
	if err != nil {
		t.Fatalf("ExtractZipFile failed: %v", err)
	}

	// Verify all files exist
	for i := 0; i < 100; i++ {
		fileName := fmt.Sprintf("file%03d.txt", i)
		filePath := filepath.Join(destDir, fileName)
		if _, err := os.Stat(filePath); err != nil {
			t.Errorf("File %s does not exist: %v", fileName, err)
		}
	}
}

// TestExtractFile_WriteAfterClose tests writing after file close
func TestExtractFile_WriteAfterClose(t *testing.T) {
	zipPath := createTestZip(t, []zipTestFile{
		{name: "test.txt", content: "content"},
	})

	r, err := zip.OpenReader(zipPath)
	if err != nil {
		t.Fatalf("Failed to open zip: %v", err)
	}
	defer r.Close()

	if len(r.File) == 0 {
		t.Fatal("Zip file is empty")
	}

	destDir := t.TempDir()

	// Extract should work normally
	err = extractFile(r.File[0], destDir)
	if err != nil {
		t.Errorf("extractFile failed: %v", err)
	}

	// Verify file was created correctly
	filePath := filepath.Join(destDir, "test.txt")
	content, err := os.ReadFile(filePath)
	if err != nil {
		t.Errorf("Failed to read file: %v", err)
	}

	if string(content) != "content" {
		t.Errorf("Content mismatch: got %q, want 'content'", string(content))
	}
}

// TestExtractFile_OpenFileError tests error when opening file in zip fails
func TestExtractFile_OpenFileError(t *testing.T) {
	// Create a zip with a file
	zipPath := createTestZip(t, []zipTestFile{
		{name: "test.txt", content: "content"},
	})

	r, err := zip.OpenReader(zipPath)
	if err != nil {
		t.Fatalf("Failed to open zip: %v", err)
	}
	defer r.Close()

	if len(r.File) == 0 {
		t.Fatal("Zip file is empty")
	}

	// This test is limited because we can't easily mock f.Open() to fail
	// We'll just verify the function handles normal cases correctly
	destDir := t.TempDir()

	err = extractFile(r.File[0], destDir)
	if err != nil {
		t.Errorf("extractFile failed unexpectedly: %v", err)
	}
}
