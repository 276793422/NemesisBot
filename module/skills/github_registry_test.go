// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package skills

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestNewGitHubRegistry(t *testing.T) {
	tests := []struct {
		name        string
		cfg         GitHubConfig
		wantURL     string
		wantTimeout time.Duration
		wantMaxSize int
	}{
		{
			name:        "default config",
			cfg:         GitHubConfig{},
			wantURL:     "https://raw.githubusercontent.com",
			wantTimeout: 30 * time.Second,
			wantMaxSize: 1 * 1024 * 1024,
		},
		{
			name: "custom base URL",
			cfg: GitHubConfig{
				BaseURL: "https://custom.github.com",
			},
			wantURL:     "https://custom.github.com",
			wantTimeout: 30 * time.Second,
			wantMaxSize: 1 * 1024 * 1024,
		},
		{
			name: "custom timeout",
			cfg: GitHubConfig{
				Timeout: 60,
			},
			wantURL:     "https://raw.githubusercontent.com",
			wantTimeout: 60 * time.Second,
			wantMaxSize: 1 * 1024 * 1024,
		},
		{
			name: "custom max size",
			cfg: GitHubConfig{
				MaxSize: 5 * 1024 * 1024,
			},
			wantURL:     "https://raw.githubusercontent.com",
			wantTimeout: 30 * time.Second,
			wantMaxSize: 5 * 1024 * 1024,
		},
		{
			name: "all custom",
			cfg: GitHubConfig{
				BaseURL: "https://custom.github.com",
				Timeout: 120,
				MaxSize: 10 * 1024 * 1024,
			},
			wantURL:     "https://custom.github.com",
			wantTimeout: 120 * time.Second,
			wantMaxSize: 10 * 1024 * 1024,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			reg := NewGitHubRegistry(tt.cfg)

			if reg.baseURL != tt.wantURL {
				t.Errorf("expected baseURL %q, got %q", tt.wantURL, reg.baseURL)
			}

			if reg.timeout != tt.wantTimeout {
				t.Errorf("expected timeout %v, got %v", tt.wantTimeout, reg.timeout)
			}

			if reg.maxSize != tt.wantMaxSize {
				t.Errorf("expected maxSize %d, got %d", tt.wantMaxSize, reg.maxSize)
			}

			if reg.client == nil {
				t.Error("expected client to be initialized")
			}
		})
	}
}

func TestGitHubRegistry_Name(t *testing.T) {
	reg := NewGitHubRegistry(GitHubConfig{})
	if reg.Name() != "github" {
		t.Errorf("expected name 'github', got %q", reg.Name())
	}
}

func TestGitHubRegistry_Search(t *testing.T) {
	tests := []struct {
		name        string
		setupServer func() *httptest.Server
		query       string
		limit       int
		wantErr     bool
		errContains string
		wantCount   int
	}{
		{
			name: "successful search with results",
			setupServer: func() *httptest.Server {
				skills := []githubSkill{
					{Name: "test-skill", Description: "A test skill"},
					{Name: "another-skill", Description: "Another skill"},
				}
				data, _ := json.Marshal(skills)
				return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					w.WriteHeader(http.StatusOK)
					w.Write(data)
				}))
			},
			query:     "test",
			limit:     10,
			wantCount: 1, // Only "test-skill" matches
		},
		{
			name: "search with no results",
			setupServer: func() *httptest.Server {
				skills := []githubSkill{
					{Name: "other-skill", Description: "Other skill"},
				}
				data, _ := json.Marshal(skills)
				return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					w.WriteHeader(http.StatusOK)
					w.Write(data)
				}))
			},
			query:     "test",
			limit:     10,
			wantCount: 0,
		},
		{
			name: "search with limit",
			setupServer: func() *httptest.Server {
				skills := []githubSkill{
					{Name: "test-skill-1", Description: "Test 1"},
					{Name: "test-skill-2", Description: "Test 2"},
					{Name: "test-skill-3", Description: "Test 3"},
				}
				data, _ := json.Marshal(skills)
				return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					w.WriteHeader(http.StatusOK)
					w.Write(data)
				}))
			},
			query:     "test",
			limit:     2,
			wantCount: 2,
		},
		{
			name: "HTTP error",
			setupServer: func() *httptest.Server {
				return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					w.WriteHeader(http.StatusNotFound)
				}))
			},
			query:   "test",
			limit:   10,
			wantErr: true,
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
			name: "empty skills list",
			setupServer: func() *httptest.Server {
				skills := []githubSkill{}
				data, _ := json.Marshal(skills)
				return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					w.WriteHeader(http.StatusOK)
					w.Write(data)
				}))
			},
			query:     "test",
			limit:     10,
			wantCount: 0,
		},
		{
			name: "case insensitive matching",
			setupServer: func() *httptest.Server {
				skills := []githubSkill{
					{Name: "TestSkill", Description: "A TEST skill"},
				}
				data, _ := json.Marshal(skills)
				return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					w.WriteHeader(http.StatusOK)
					w.Write(data)
				}))
			},
			query:     "test",
			limit:     10,
			wantCount: 1, // Returns 1 skill (even though both name and description match)
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			server := tt.setupServer()
			defer server.Close()

			cfg := GitHubConfig{BaseURL: strings.TrimPrefix(server.URL, "http://")}
			reg := NewGitHubRegistry(cfg)
			// Update baseURL to use test server
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

			if !tt.wantErr {
				for _, result := range results {
					if result.RegistryName != "github" {
						t.Errorf("expected registry name 'github', got %q", result.RegistryName)
					}
					if result.Version != "latest" {
						t.Errorf("expected version 'latest', got %q", result.Version)
					}
					if result.Score != 1.0 {
						t.Errorf("expected score 1.0, got %f", result.Score)
					}
				}
			}
		})
	}
}

func TestGitHubRegistry_GetSkillMeta(t *testing.T) {
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
				skills := []githubSkill{
					{Name: "test-skill", Description: "A test skill"},
				}
				data, _ := json.Marshal(skills)
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
				if meta.DisplayName != "test-skill" {
					t.Errorf("expected display name 'test-skill', got %q", meta.DisplayName)
				}
				if meta.Summary != "A test skill" {
					t.Errorf("expected summary 'A test skill', got %q", meta.Summary)
				}
				if meta.LatestVersion != "latest" {
					t.Errorf("expected version 'latest', got %q", meta.LatestVersion)
				}
				if meta.RegistryName != "github" {
					t.Errorf("expected registry 'github', got %q", meta.RegistryName)
				}
			},
		},
		{
			name: "skill not found",
			setupServer: func() *httptest.Server {
				skills := []githubSkill{
					{Name: "other-skill", Description: "Other skill"},
				}
				data, _ := json.Marshal(skills)
				return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					w.WriteHeader(http.StatusOK)
					w.Write(data)
				}))
			},
			slug:        "nonexistent",
			wantErr:     true,
			errContains: "not found",
		},
		{
			name: "invalid slug",
			setupServer: func() *httptest.Server {
				return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					w.WriteHeader(http.StatusOK)
					w.Write([]byte("[]"))
				}))
			},
			slug:        "../../../etc/passwd",
			wantErr:     true,
			errContains: "invalid slug",
		},
		{
			name: "HTTP error",
			setupServer: func() *httptest.Server {
				return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					w.WriteHeader(http.StatusInternalServerError)
				}))
			},
			slug:    "test-skill",
			wantErr: true,
		},
		{
			name: "invalid JSON",
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

			cfg := GitHubConfig{BaseURL: strings.TrimPrefix(server.URL, "http://")}
			reg := NewGitHubRegistry(cfg)
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

func TestGitHubRegistry_DownloadAndInstall(t *testing.T) {
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
			name: "successful installation from skills repo",
			setupServer: func() *httptest.Server {
				skills := []githubSkill{
					{Name: "test-skill", Description: "A test skill"},
				}
				skillsData, _ := json.Marshal(skills)
				return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					if strings.HasSuffix(r.URL.Path, "skills.json") {
						w.WriteHeader(http.StatusOK)
						w.Write(skillsData)
					} else if strings.HasSuffix(r.URL.Path, "SKILL.md") {
						w.WriteHeader(http.StatusOK)
						w.Write([]byte("# Test Skill\n\nThis is a test skill."))
					} else {
						w.WriteHeader(http.StatusNotFound)
					}
				}))
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
				if result.Version != "latest" {
					t.Errorf("expected version 'latest', got %q", result.Version)
				}
				if result.IsMalwareBlocked {
					t.Errorf("expected malware blocked to be false")
				}
				if result.IsSuspicious {
					t.Errorf("expected suspicious to be false")
				}
			},
		},
		{
			name: "installation with specific version",
			setupServer: func() *httptest.Server {
				return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					w.WriteHeader(http.StatusOK)
					w.Write([]byte("# Test Skill v1.0"))
				}))
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
		{
			name: "fallback to main when no version",
			setupServer: func() *httptest.Server {
				return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					w.WriteHeader(http.StatusOK)
					w.Write([]byte("# Test Skill"))
				}))
			},
			slug:    "test-skill",
			version: "",
			wantErr: false,
			verify: func(t *testing.T, targetDir string, result *InstallResult) {
				if result.Version != "main" {
					t.Errorf("expected version 'main', got %q", result.Version)
				}
			},
		},
		{
			name: "invalid slug",
			setupServer: func() *httptest.Server {
				return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					w.Write([]byte("[]"))
				}))
			},
			slug:        "../../../etc/passwd",
			version:     "",
			wantErr:     true,
			errContains: "invalid slug",
		},
		{
			name: "HTTP error - skill not found",
			setupServer: func() *httptest.Server {
				return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					w.WriteHeader(http.StatusNotFound)
				}))
			},
			slug:        "nonexistent",
			version:     "",
			wantErr:     true,
			errContains: "failed to download",
		},
		{
			name: "metadata fetch fails but installation succeeds",
			setupServer: func() *httptest.Server {
				return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					if strings.HasSuffix(r.URL.Path, "skills.json") {
						w.WriteHeader(http.StatusInternalServerError)
					} else {
						w.WriteHeader(http.StatusOK)
						w.Write([]byte("# Test Skill"))
					}
				}))
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

			cfg := GitHubConfig{BaseURL: strings.TrimPrefix(server.URL, "http://")}
			reg := NewGitHubRegistry(cfg)
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

func TestGitHubRegistry_DoGet(t *testing.T) {
	tests := []struct {
		name        string
		setupServer func() *httptest.Server
		wantErr     bool
		errContains string
		verify      func(*testing.T, []byte)
	}{
		{
			name: "successful GET request",
			setupServer: func() *httptest.Server {
				return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					w.WriteHeader(http.StatusOK)
					w.Write([]byte("response body"))
				}))
			},
			wantErr: false,
			verify: func(t *testing.T, body []byte) {
				if string(body) != "response body" {
					t.Errorf("expected 'response body', got %q", string(body))
				}
			},
		},
		{
			name: "HTTP error status",
			setupServer: func() *httptest.Server {
				return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					w.WriteHeader(http.StatusNotFound)
					w.Write([]byte("not found"))
				}))
			},
			wantErr:     true,
			errContains: "404",
		},
		{
			name: "response size limit",
			setupServer: func() *httptest.Server {
				return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					w.WriteHeader(http.StatusOK)
					// Send more than default max size
					w.Write(make([]byte, 2*1024*1024))
				}))
			},
			wantErr: false,
			verify: func(t *testing.T, body []byte) {
				// Should be limited to maxSize (1MB default)
				if len(body) > 1*1024*1024 {
					t.Errorf("response should be limited to 1MB, got %d bytes", len(body))
				}
			},
		},
		{
			name: "context cancellation",
			setupServer: func() *httptest.Server {
				return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					// This won't be called due to context cancellation
					w.WriteHeader(http.StatusOK)
				}))
			},
			wantErr: true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			server := tt.setupServer()
			defer server.Close()

			cfg := GitHubConfig{BaseURL: strings.TrimPrefix(server.URL, "http://")}
			reg := NewGitHubRegistry(cfg)
			reg.baseURL = server.URL

			ctx := context.Background()
			if tt.name == "context cancellation" {
				var cancel context.CancelFunc
				ctx, cancel = context.WithCancel(ctx)
				cancel()
			}

			url := server.URL + "/test"
			body, err := reg.doGet(ctx, url)

			if (err != nil) != tt.wantErr {
				t.Errorf("doGet() error = %v, wantErr %v", err, tt.wantErr)
				return
			}

			if err != nil && tt.errContains != "" {
				if !strings.Contains(err.Error(), tt.errContains) {
					t.Errorf("error should contain %q, got %q", tt.errContains, err.Error())
				}
			}

			if !tt.wantErr && tt.verify != nil {
				tt.verify(t, body)
			}
		})
	}
}

func TestContains(t *testing.T) {
	tests := []struct {
		name     string
		s        string
		substr   string
		expected bool
	}{
		{
			name:     "exact match",
			s:        "test",
			substr:   "test",
			expected: true,
		},
		{
			name:     "substring at start",
			s:        "test skill",
			substr:   "test",
			expected: true,
		},
		{
			name:     "substring at end",
			s:        "skill test",
			substr:   "test",
			expected: true,
		},
		{
			name:     "substring in middle",
			s:        "skill test skill",
			substr:   "test",
			expected: true,
		},
		{
			name:     "no match",
			s:        "skill",
			substr:   "test",
			expected: false,
		},
		{
			name:     "empty substring",
			s:        "test",
			substr:   "",
			expected: true,
		},
		{
			name:     "substring longer than string",
			s:        "test",
			substr:   "test skill",
			expected: false,
		},
		{
			name:     "case insensitive",
			s:        "Test",
			substr:   "test",
			expected: true, // Case-insensitive matching
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := contains(tt.s, tt.substr)
			if result != tt.expected {
				t.Errorf("contains(%q, %q) = %v, want %v", tt.s, tt.substr, result, tt.expected)
			}
		})
	}
}

func TestGitHubSkill_Structure(t *testing.T) {
	skill := githubSkill{
		Name:        "test-skill",
		Description: "A test skill",
		Repository:  "user/repo",
		Author:      "Test Author",
		Tags:        []string{"test", "example"},
	}

	data, err := json.Marshal(skill)
	if err != nil {
		t.Fatalf("failed to marshal githubSkill: %v", err)
	}

	var decoded githubSkill
	if err := json.Unmarshal(data, &decoded); err != nil {
		t.Fatalf("failed to unmarshal githubSkill: %v", err)
	}

	if decoded.Name != skill.Name {
		t.Errorf("expected name %q, got %q", skill.Name, decoded.Name)
	}

	if decoded.Description != skill.Description {
		t.Errorf("expected description %q, got %q", skill.Description, decoded.Description)
	}

	if decoded.Repository != skill.Repository {
		t.Errorf("expected repository %q, got %q", skill.Repository, decoded.Repository)
	}

	if decoded.Author != skill.Author {
		t.Errorf("expected author %q, got %q", skill.Author, decoded.Author)
	}

	if len(decoded.Tags) != len(skill.Tags) {
		t.Errorf("expected %d tags, got %d", len(skill.Tags), len(decoded.Tags))
	}
}
