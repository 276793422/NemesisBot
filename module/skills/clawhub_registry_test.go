// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package skills

import (
	"archive/zip"
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestNewClawHubRegistry(t *testing.T) {
	tests := []struct {
		name         string
		cfg          ClawHubConfig
		wantURL      string
		wantTimeout  time.Duration
		wantMaxZip   int64
		wantMaxResp  int64
		wantSearch   string
		wantSkills   string
		wantDownload string
	}{
		{
			name: "default config",
			cfg:  ClawHubConfig{},
			wantURL: "https://clawhub.ai",
			wantTimeout: 30 * time.Second,
			wantMaxZip: 50 * 1024 * 1024,
			wantMaxResp: 2 * 1024 * 1024,
			wantSearch: "/api/v1/search",
			wantSkills: "/api/v1/skills",
			wantDownload: "/api/v1/download",
		},
		{
			name: "custom base URL",
			cfg: ClawHubConfig{
				BaseURL: "https://custom.clawhub.com",
			},
			wantURL: "https://custom.clawhub.com",
			wantTimeout: 30 * time.Second,
			wantMaxZip: 50 * 1024 * 1024,
			wantMaxResp: 2 * 1024 * 1024,
			wantSearch: "/api/v1/search",
			wantSkills: "/api/v1/skills",
			wantDownload: "/api/v1/download",
		},
		{
			name: "custom timeout",
			cfg: ClawHubConfig{
				Timeout: 60,
			},
			wantURL: "https://clawhub.ai",
			wantTimeout: 60 * time.Second,
			wantMaxZip: 50 * 1024 * 1024,
			wantMaxResp: 2 * 1024 * 1024,
			wantSearch: "/api/v1/search",
			wantSkills: "/api/v1/skills",
			wantDownload: "/api/v1/download",
		},
		{
			name: "custom max sizes",
			cfg: ClawHubConfig{
				MaxZipSize:      100 * 1024 * 1024,
				MaxResponseSize: 5 * 1024 * 1024,
			},
			wantURL: "https://clawhub.ai",
			wantTimeout: 30 * time.Second,
			wantMaxZip: 100 * 1024 * 1024,
			wantMaxResp: 5 * 1024 * 1024,
			wantSearch: "/api/v1/search",
			wantSkills: "/api/v1/skills",
			wantDownload: "/api/v1/download",
		},
		{
			name: "custom paths",
			cfg: ClawHubConfig{
				SearchPath:   "/custom/search",
				SkillsPath:   "/custom/skills",
				DownloadPath: "/custom/download",
			},
			wantURL: "https://clawhub.ai",
			wantTimeout: 30 * time.Second,
			wantMaxZip: 50 * 1024 * 1024,
			wantMaxResp: 2 * 1024 * 1024,
			wantSearch: "/custom/search",
			wantSkills: "/custom/skills",
			wantDownload: "/custom/download",
		},
		{
			name: "all custom",
			cfg: ClawHubConfig{
				BaseURL:         "https://custom.clawhub.com",
				Timeout:         120,
				MaxZipSize:      100 * 1024 * 1024,
				MaxResponseSize: 5 * 1024 * 1024,
				SearchPath:      "/v2/search",
				SkillsPath:      "/v2/skills",
				DownloadPath:    "/v2/download",
			},
			wantURL: "https://custom.clawhub.com",
			wantTimeout: 120 * time.Second,
			wantMaxZip: 100 * 1024 * 1024,
			wantMaxResp: 5 * 1024 * 1024,
			wantSearch: "/v2/search",
			wantSkills: "/v2/skills",
			wantDownload: "/v2/download",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			reg := NewClawHubRegistry(tt.cfg)

			if reg.baseURL != tt.wantURL {
				t.Errorf("expected baseURL %q, got %q", tt.wantURL, reg.baseURL)
			}

			if reg.timeout != tt.wantTimeout {
				t.Errorf("expected timeout %v, got %v", tt.wantTimeout, reg.timeout)
			}

			if reg.maxZipSize != tt.wantMaxZip {
				t.Errorf("expected maxZipSize %d, got %d", tt.wantMaxZip, reg.maxZipSize)
			}

			if reg.maxResponseSize != tt.wantMaxResp {
				t.Errorf("expected maxResponseSize %d, got %d", tt.wantMaxResp, reg.maxResponseSize)
			}

			if reg.searchPath != tt.wantSearch {
				t.Errorf("expected searchPath %q, got %q", tt.wantSearch, reg.searchPath)
			}

			if reg.skillsPath != tt.wantSkills {
				t.Errorf("expected skillsPath %q, got %q", tt.wantSkills, reg.skillsPath)
			}

			if reg.downloadPath != tt.wantDownload {
				t.Errorf("expected downloadPath %q, got %q", tt.wantDownload, reg.downloadPath)
			}

			if reg.client == nil {
				t.Error("expected client to be initialized")
			}

			if reg.client.Timeout != tt.wantTimeout {
				t.Errorf("expected client timeout %v, got %v", tt.wantTimeout, reg.client.Timeout)
			}
		})
	}
}

func TestClawHubRegistry_Name(t *testing.T) {
	reg := NewClawHubRegistry(ClawHubConfig{})
	if reg.Name() != "clawhub" {
		t.Errorf("expected name 'clawhub', got %q", reg.Name())
	}
}

func TestClawHubRegistry_Search(t *testing.T) {
	tests := []struct {
		name       string
		setupServer func() *httptest.Server
		query      string
		limit      int
		wantErr    bool
		errContains string
		wantCount  int
		verify     func(*testing.T, []SearchResult)
	}{
		{
			name: "successful search",
			setupServer: func() *httptest.Server {
				response := struct {
					Results []struct {
						Score       float64 `json:"score"`
						Slug        string  `json:"slug"`
						DisplayName string  `json:"display_name"`
						Summary     string  `json:"summary"`
						Version     string  `json:"version"`
					} `json:"results"`
				}{
					Results: []struct {
						Score       float64 `json:"score"`
						Slug        string  `json:"slug"`
						DisplayName string  `json:"display_name"`
						Summary     string  `json:"summary"`
						Version     string  `json:"version"`
					}{
						{
							Score:       0.95,
							Slug:        "test-skill",
							DisplayName: "Test Skill",
							Summary:     "A test skill",
							Version:     "1.0.0",
						},
					},
				}
				data, _ := json.Marshal(response)
				return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					w.WriteHeader(http.StatusOK)
					w.Write(data)
				}))
			},
			query:     "test",
			limit:     10,
			wantCount: 1,
		},
		{
			name: "search with limit",
			setupServer: func() *httptest.Server {
				response := struct {
					Results []interface{} `json:"results"`
				}{
					Results: make([]interface{}, 5),
				}
				data, _ := json.Marshal(response)
				return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					// Check limit parameter
					if r.URL.Query().Get("limit") != "5" {
						t.Errorf("expected limit parameter '5', got %q", r.URL.Query().Get("limit"))
					}
					w.WriteHeader(http.StatusOK)
					w.Write(data)
				}))
			},
			query:     "test",
			limit:     5,
			wantCount: 5,
		},
		{
			name: "search with auth token",
			setupServer: func() *httptest.Server {
				response := struct {
					Results []interface{} `json:"results"`
				}{}
				data, _ := json.Marshal(response)
				return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					auth := r.Header.Get("Authorization")
					if auth != "Bearer test-token" {
						t.Errorf("expected auth header 'Bearer test-token', got %q", auth)
					}
					w.WriteHeader(http.StatusOK)
					w.Write(data)
				}))
			},
			query: "test",
			limit: 10,
		},
		{
			name: "HTTP error",
			setupServer: func() *httptest.Server {
				return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					w.WriteHeader(http.StatusInternalServerError)
					w.Write([]byte("internal error"))
				}))
			},
			query:      "test",
			limit:      10,
			wantErr:    true,
		},
		{
			name: "invalid JSON response",
			setupServer: func() *httptest.Server {
				return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					w.WriteHeader(http.StatusOK)
					w.Write([]byte("invalid json"))
				}))
			},
			query:   "test",
			limit:   10,
			wantErr: true,
		},
		{
			name: "response too large",
			setupServer: func() *httptest.Server {
				return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					w.Header().Set("Content-Length", "10000000") // 10MB
					w.WriteHeader(http.StatusOK)
				}))
			},
			query:      "test",
			limit:      10,
			wantErr:    true,
			errContains: "too large",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			server := tt.setupServer()
			defer server.Close()

			cfg := ClawHubConfig{
				BaseURL:   strings.TrimPrefix(server.URL, "http://"),
				AuthToken: "test-token",
			}
			reg := NewClawHubRegistry(cfg)
			reg.baseURL = server.URL

			results, err := reg.Search(context.Background(), tt.query, tt.limit)

			if (err != nil) != tt.wantErr {
				t.Errorf("Search() error = %v, wantErr %v", err, tt.wantErr)
				return
			}

			if err != nil && tt.errContains != "" {
				if !strings.Contains(err.Error(), tt.errContains) {
					t.Errorf("error should contain %q, got %q", tt.errContains, err.Error())
				}
			}

			if !tt.wantErr && len(results) != tt.wantCount {
				t.Errorf("Search() returned %d results, want %d", len(results), tt.wantCount)
			}

			if !tt.wantErr && tt.verify != nil {
				tt.verify(t, results)
			}
		})
	}
}

func TestClawHubRegistry_GetSkillMeta(t *testing.T) {
	tests := []struct {
		name        string
		setupServer func() *httptest.Server
		slug        string
		wantErr     bool
		errContains string
		verify      func(*testing.T, *SkillMeta)
	}{
		{
			name: "successful metadata retrieval",
			setupServer: func() *httptest.Server {
				meta := SkillMeta{
					Slug:          "test-skill",
					DisplayName:   "Test Skill",
					Summary:       "A test skill",
					LatestVersion: "1.0.0",
					IsSuspicious:  false,
				}
				data, _ := json.Marshal(meta)
				return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					w.WriteHeader(http.StatusOK)
					w.Write(data)
				}))
			},
			slug:    "test-skill",
			wantErr: false,
			verify: func(t *testing.T, meta *SkillMeta) {
				if meta.Slug != "test-skill" {
					t.Errorf("expected slug 'test-skill', got %q", meta.Slug)
				}
				if meta.DisplayName != "Test Skill" {
					t.Errorf("expected display name 'Test Skill', got %q", meta.DisplayName)
				}
				if meta.RegistryName != "clawhub" {
					t.Errorf("expected registry 'clawhub', got %q", meta.RegistryName)
				}
			},
		},
		{
			name: "skill not found",
			setupServer: func() *httptest.Server {
				return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					w.WriteHeader(http.StatusNotFound)
				}))
			},
			slug:        "nonexistent",
			wantErr:     true,
			errContains: "not found",
		},
		{
			name: "invalid slug - path traversal",
			setupServer: func() *httptest.Server {
				return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					w.WriteHeader(http.StatusOK)
					w.Write([]byte("{}"))
				}))
			},
			slug:        "../../../etc/passwd",
			wantErr:     true,
			errContains: "invalid skill slug",
		},
		{
			name: "invalid slug - empty",
			setupServer: func() *httptest.Server {
				return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					w.WriteHeader(http.StatusOK)
					w.Write([]byte("{}"))
				}))
			},
			slug:        "",
			wantErr:     true,
			errContains: "cannot be empty",
		},
		{
			name: "invalid slug - too long",
			setupServer: func() *httptest.Server {
				return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					w.WriteHeader(http.StatusOK)
					w.Write([]byte("{}"))
				}))
			},
			slug:        strings.Repeat("a", 65),
			wantErr:     true,
			errContains: "too long",
		},
		{
			name: "invalid JSON response",
			setupServer: func() *httptest.Server {
				return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					w.WriteHeader(http.StatusOK)
					w.Write([]byte("invalid json"))
				}))
			},
			slug:    "test-skill",
			wantErr: true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			server := tt.setupServer()
			defer server.Close()

			cfg := ClawHubConfig{BaseURL: strings.TrimPrefix(server.URL, "http://")}
			reg := NewClawHubRegistry(cfg)
			reg.baseURL = server.URL

			meta, err := reg.GetSkillMeta(context.Background(), tt.slug)

			if (err != nil) != tt.wantErr {
				t.Errorf("GetSkillMeta() error = %v, wantErr %v", err, tt.wantErr)
				return
			}

			if err != nil && tt.errContains != "" {
				if !strings.Contains(err.Error(), tt.errContains) {
					t.Errorf("error should contain %q, got %q", tt.errContains, err.Error())
				}
			}

			if !tt.wantErr && tt.verify != nil {
				tt.verify(t, meta)
			}
		})
	}
}

func TestClawHubRegistry_DownloadAndInstall(t *testing.T) {
	tests := []struct {
		name        string
		setupServer func() *httptest.Server
		slug        string
		version     string
		wantErr     bool
		errContains string
		verify      func(*testing.T, string, *InstallResult)
	}{
		{
			name: "successful installation",
			setupServer: func() *httptest.Server {
				// Create a ZIP file in memory
				return createZipTestServer(t, map[string]string{
					"SKILL.md": "# Test Skill\n\nThis is a test skill.",
				}, nil)
			},
			slug:    "test-skill",
			version: "",
			wantErr: false,
			verify: func(t *testing.T, targetDir string, result *InstallResult) {
				// Check skill file exists
				skillFile := filepath.Join(targetDir, "SKILL.md")
				if _, err := os.Stat(skillFile); os.IsNotExist(err) {
					t.Errorf("skill file should exist at %s", skillFile)
				}

				// Check result
				if result.IsMalwareBlocked {
					t.Errorf("expected malware blocked to be false")
				}
				if result.IsSuspicious {
					t.Errorf("expected suspicious to be false")
				}
			},
		},
		{
			name: "malware blocked",
			setupServer: func() *httptest.Server {
				return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					if strings.Contains(r.URL.Path, "skills") {
						meta := SkillMeta{
							Slug:            "malware-skill",
							DisplayName:     "Malware Skill",
							Summary:         "Malicious skill",
							LatestVersion:   "1.0.0",
							IsMalwareBlocked: true,
						}
						data, _ := json.Marshal(meta)
						w.WriteHeader(http.StatusOK)
						w.Write(data)
					}
				}))
			},
			slug:        "malware-skill",
			version:     "",
			wantErr:     false, // Returns result with malware flag
			verify: func(t *testing.T, targetDir string, result *InstallResult) {
				if !result.IsMalwareBlocked {
					t.Errorf("expected malware blocked to be true")
				}
			},
		},
		{
			name: "invalid slug",
			setupServer: func() *httptest.Server {
				return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					w.WriteHeader(http.StatusOK)
					w.Write([]byte("{}"))
				}))
			},
			slug:        "../../../etc/passwd",
			version:      "",
			wantErr:      true,
			errContains:  "invalid skill slug",
		},
		{
			name: "metadata fetch fails",
			setupServer: func() *httptest.Server {
				return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					w.WriteHeader(http.StatusInternalServerError)
				}))
			},
			slug:        "test-skill",
			version:      "",
			wantErr:      true,
			errContains:  "failed to get skill metadata",
		},
		{
			name: "download fails",
			setupServer: func() *httptest.Server {
				return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					if strings.Contains(r.URL.Path, "skills") {
						meta := SkillMeta{
							Slug:          "test-skill",
							LatestVersion: "1.0.0",
						}
						data, _ := json.Marshal(meta)
						w.WriteHeader(http.StatusOK)
						w.Write(data)
					} else {
						w.WriteHeader(http.StatusInternalServerError)
					}
				}))
			},
			slug:        "test-skill",
			version:      "",
			wantErr:      true,
			errContains:  "download request failed",
		},
		{
			name: "installation with specific version",
			setupServer: func() *httptest.Server {
				return createZipTestServer(t, map[string]string{
					"SKILL.md": "# Test Skill v1.0",
				}, nil)
			},
			slug:    "test-skill",
			version: "v1.0",
			wantErr: false,
			verify: func(t *testing.T, targetDir string, result *InstallResult) {
				if result.Version != "v1.0" {
					t.Errorf("expected version 'v1.0', got %q", result.Version)
				}
			},
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			server := tt.setupServer()
			defer server.Close()

			cfg := ClawHubConfig{BaseURL: strings.TrimPrefix(server.URL, "http://")}
			reg := NewClawHubRegistry(cfg)
			reg.baseURL = server.URL

			tempDir := t.TempDir()
			targetDir := filepath.Join(tempDir, "test-skill")

			result, err := reg.DownloadAndInstall(context.Background(), tt.slug, tt.version, targetDir)

			if (err != nil) != tt.wantErr {
				t.Errorf("DownloadAndInstall() error = %v, wantErr %v", err, tt.wantErr)
				return
			}

			if err != nil && tt.errContains != "" {
				if !strings.Contains(err.Error(), tt.errContains) {
					t.Errorf("error should contain %q, got %q", tt.errContains, err.Error())
				}
			}

			if !tt.wantErr && tt.verify != nil {
				tt.verify(t, targetDir, result)
			}
		})
	}
}

func TestClawHubRegistry_ExtractZipFile(t *testing.T) {
	tests := []struct {
		name        string
		createZip   func(string) error
		wantErr     bool
		errContains string
		verify      func(*testing.T, string)
	}{
		{
			name: "successful extraction",
			createZip: func(zipPath string) error {
				return createTestZip(zipPath, map[string]string{
					"SKILL.md":   "# Test Skill",
					"README.md":  "README content",
					"extra.txt":  "extra content",
				})
			},
			wantErr: false,
			verify: func(t *testing.T, targetDir string) {
				// Check extracted files
				files := []string{
					"SKILL.md",
					"README.md",
					"extra.txt",
				}
				for _, file := range files {
					filePath := filepath.Join(targetDir, file)
					if _, err := os.Stat(filePath); os.IsNotExist(err) {
						t.Errorf("file should exist: %s", filePath)
					}
				}
			},
		},
		{
			name: "ZIP file not found",
			createZip: func(zipPath string) error {
				return nil // Don't create the file
			},
			wantErr:     true,
			errContains: "failed to open ZIP",
		},
		{
			name: "path traversal attack",
			createZip: func(zipPath string) error {
				// Create a ZIP with path traversal
				file, err := os.Create(zipPath)
				if err != nil {
					return err
				}
				defer file.Close()

				writer := zip.NewWriter(file)
				defer writer.Close()

				// Try to write a file outside the target directory
				_, err = writer.Create("../../../etc/passwd")
				return err
			},
			wantErr:     true,
			errContains: "unsafe path",
		},
		{
			name: "absolute path attack",
			createZip: func(zipPath string) error {
				file, err := os.Create(zipPath)
				if err != nil {
					return err
				}
				defer file.Close()

				writer := zip.NewWriter(file)
				defer writer.Close()

				_, err = writer.Create("/etc/passwd")
				return err
			},
			wantErr:     true,
			errContains: "unsafe path",
		},
		{
			name: "Windows path separator attack",
			createZip: func(zipPath string) error {
				file, err := os.Create(zipPath)
				if err != nil {
					return err
				}
				defer file.Close()

				writer := zip.NewWriter(file)
				defer writer.Close()

				_, err = writer.Create("\\..\\..\\etc\\passwd")
				return err
			},
			wantErr:     true,
			errContains: "unsafe path",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			tempDir := t.TempDir()
			zipPath := filepath.Join(tempDir, "test.zip")
			targetDir := filepath.Join(tempDir, "extracted")

			if err := tt.createZip(zipPath); err != nil {
				t.Fatalf("failed to create test ZIP: %v", err)
			}

			reg := NewClawHubRegistry(ClawHubConfig{})
			err := reg.extractZipFile(zipPath, targetDir)

			if (err != nil) != tt.wantErr {
				t.Errorf("extractZipFile() error = %v, wantErr %v", err, tt.wantErr)
				return
			}

			if err != nil && tt.errContains != "" {
				if !strings.Contains(err.Error(), tt.errContains) {
					t.Errorf("error should contain %q, got %q", tt.errContains, err.Error())
				}
			}

			if !tt.wantErr && tt.verify != nil {
				tt.verify(t, targetDir)
			}
		})
	}
}

func TestClawHubRegistry_DoRequestWithRetry(t *testing.T) {
	tests := []struct {
		name        string
		setupServer func() *httptest.Server
		wantErr     bool
		errContains string
	}{
		{
			name: "successful request",
			setupServer: func() *httptest.Server {
				return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					w.WriteHeader(http.StatusOK)
					w.Write([]byte("success"))
				}))
			},
			wantErr: false,
		},
		{
			name: "rate limiting with retry",
			setupServer: func() *httptest.Server {
				attempts := 0
				return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					attempts++
					if attempts < 3 {
						w.WriteHeader(http.StatusTooManyRequests)
					} else {
						w.WriteHeader(http.StatusOK)
						w.Write([]byte("success after retries"))
					}
				}))
			},
			wantErr: false,
		},
		{
			name: "max retries exceeded",
			setupServer: func() *httptest.Server {
				return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					w.WriteHeader(http.StatusTooManyRequests)
				}))
			},
			wantErr:     true,
			errContains: "failed after",
		},
		{
			name: "network error",
			setupServer: func() *httptest.Server {
				server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					w.WriteHeader(http.StatusOK)
				}))
				server.Close() // Close immediately
				return server
			},
			wantErr: true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			server := tt.setupServer()
			if server != nil {
				defer server.Close()
			}

			cfg := ClawHubConfig{BaseURL: strings.TrimPrefix(server.URL, "http://")}
			reg := NewClawHubRegistry(cfg)
			reg.baseURL = server.URL

			req, err := http.NewRequest("GET", server.URL, nil)
			if err != nil {
				t.Fatalf("failed to create request: %v", err)
			}

			resp, err := reg.doRequestWithRetry(req)

			if (err != nil) != tt.wantErr {
				t.Errorf("doRequestWithRetry() error = %v, wantErr %v", err, tt.wantErr)
				return
			}

			if err != nil && tt.errContains != "" {
				if !strings.Contains(err.Error(), tt.errContains) {
					t.Errorf("error should contain %q, got %q", tt.errContains, err.Error())
				}
			}

			if !tt.wantErr && resp != nil {
				resp.Body.Close()
			}
		})
	}
}

func TestValidateSkillIdentifier(t *testing.T) {
	tests := []struct {
		name        string
		slug        string
		wantErr     bool
		errContains string
	}{
		{
			name:    "valid slug",
			slug:    "test-skill",
			wantErr: false,
		},
		{
			name:    "valid slug with numbers",
			slug:    "test-skill-123",
			wantErr: false,
		},
		{
			name:    "valid slug with underscores",
			slug:    "test_skill",
			wantErr: false,
		},
		{
			name:        "empty slug",
			slug:        "",
			wantErr:     true,
			errContains: "cannot be empty",
		},
		{
			name:        "whitespace only",
			slug:        "   ",
			wantErr:     true,
			errContains: "cannot be empty",
		},
		{
			name:        "contains forward slash",
			slug:        "test/skill",
			wantErr:     true,
			errContains: "path separators",
		},
		{
			name:        "contains backslash",
			slug:        "test\\skill",
			wantErr:     true,
			errContains: "path separators",
		},
		{
			name:        "contains double dot",
			slug:        "../test",
			wantErr:     true,
			errContains: "path separators",
		},
		{
			name:        "too long",
			slug:        strings.Repeat("a", 65),
			wantErr:     true,
			errContains: "too long",
		},
		{
			name:        "path traversal attempt",
			slug:        "../../../etc/passwd",
			wantErr:     true,
			errContains: "path separators",
		},
		{
			name:    "exactly 64 characters",
			slug:    strings.Repeat("a", 64),
			wantErr: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			err := ValidateSkillIdentifier(tt.slug)

			if (err != nil) != tt.wantErr {
				t.Errorf("ValidateSkillIdentifier() error = %v, wantErr %v", err, tt.wantErr)
				return
			}

			if err != nil && tt.errContains != "" {
				if !strings.Contains(err.Error(), tt.errContains) {
					t.Errorf("error should contain %q, got %q", tt.errContains, err.Error())
				}
			}
		})
	}
}

// Helper functions

func createZipTestServer(t *testing.T, files map[string]string, meta *SkillMeta) *httptest.Server {
	return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if strings.Contains(r.URL.Path, "skills") {
			if meta != nil {
				data, _ := json.Marshal(meta)
				w.WriteHeader(http.StatusOK)
				w.Write(data)
			} else {
				defaultMeta := SkillMeta{
					Slug:          "test-skill",
					DisplayName:   "Test Skill",
					Summary:       "A test skill",
					LatestVersion: "1.0.0",
				}
				data, _ := json.Marshal(defaultMeta)
				w.WriteHeader(http.StatusOK)
				w.Write(data)
			}
		} else if strings.Contains(r.URL.Path, "download") {
			// Create ZIP file
			zipData, err := createInMemoryZip(files)
			if err != nil {
				w.WriteHeader(http.StatusInternalServerError)
				return
			}
			w.Header().Set("Content-Type", "application/zip")
			w.WriteHeader(http.StatusOK)
			w.Write(zipData)
		} else {
			w.WriteHeader(http.StatusNotFound)
		}
	}))
}

func createInMemoryZip(files map[string]string) ([]byte, error) {
	var buf []byte
	// Use a pipe to write ZIP
	pr, pw := io.Pipe()

	go func() {
		writer := zip.NewWriter(pw)
		for name, content := range files {
			w, err := writer.Create(name)
			if err != nil {
				pw.CloseWithError(err)
				return
			}
			_, err = io.WriteString(w, content)
			if err != nil {
				pw.CloseWithError(err)
				return
			}
		}
		writer.Close()
		pw.Close()
	}()

	buf, err := io.ReadAll(pr)
	if err != nil {
		return nil, err
	}

	return buf, nil
}

func createTestZip(zipPath string, files map[string]string) error {
	file, err := os.Create(zipPath)
	if err != nil {
		return err
	}
	defer file.Close()

	writer := zip.NewWriter(file)
	defer writer.Close()

	for name, content := range files {
		w, err := writer.Create(name)
		if err != nil {
			return err
		}
		_, err = io.WriteString(w, content)
		if err != nil {
			return err
		}
	}

	return nil
}
