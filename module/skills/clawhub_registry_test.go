package skills

import (
	"archive/zip"
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// --- Mock servers ---

// clawhubSearchMock creates an httptest.Server that mimics the clawhub.ai search API.
type clawhubSearchMock struct {
	server *httptest.Server
}

func newClawhubSearchMock() *clawhubSearchMock {
	m := &clawhubSearchMock{}
	m.server = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/api/search" {
			w.WriteHeader(http.StatusNotFound)
			return
		}
		query := r.URL.Query().Get("q")
		if query == "" {
			w.Header().Set("Content-Type", "application/json")
			json.NewEncoder(w).Encode(clawhubSearchResponse{Results: []clawhubSearchItem{}})
			return
		}

		// Simple mock: return results matching the query
		resp := clawhubSearchResponse{
			Results: []clawhubSearchItem{
				{Score: 3.5, Slug: "test-skill", DisplayName: "Test Skill", Summary: "A test skill for " + query},
				{Score: 2.8, Slug: "another-skill", DisplayName: "Another Skill", Summary: "Another skill matching " + query},
			},
		}
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(resp)
	}))
	return m
}

func (m *clawhubSearchMock) URL() string { return m.server.URL }
func (m *clawhubSearchMock) Close()      { m.server.Close() }

// convexMock creates an httptest.Server that mimics the Convex HTTP API.
type convexMock struct {
	server *httptest.Server
}

func newConvexMock() *convexMock {
	m := &convexMock{}
	m.server = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost || r.URL.Path != "/api/query" {
			w.WriteHeader(http.StatusNotFound)
			return
		}

		var reqBody struct {
			Path   string          `json:"path"`
			Args   json.RawMessage `json:"args"`
			Format string          `json:"format"`
		}
		if err := json.NewDecoder(r.Body).Decode(&reqBody); err != nil {
			w.WriteHeader(http.StatusBadRequest)
			return
		}

		w.Header().Set("Content-Type", "application/json")

		switch reqBody.Path {
		case "skills:list":
			// Return a list of skills using raw JSON to avoid anonymous struct type issues
			items := `[{"slug":"popular-skill","displayName":"Popular Skill","summary":"A very popular skill","stats":{"downloads":5000}},{"slug":"new-skill","displayName":"New Skill","summary":"A brand new skill","stats":{"downloads":10}}]`
			json.NewEncoder(w).Encode(convexResponse{Status: "success", Value: json.RawMessage(items)})

		case "skills:getBySlug":
			var args struct {
				Slug string `json:"slug"`
			}
			json.Unmarshal(reqBody.Args, &args)

			var detailJSON json.RawMessage
			switch args.Slug {
			case "test-skill":
				detailJSON = json.RawMessage(`{"owner":{"handle":"testowner"},"skill":{"slug":"test-skill","displayName":"Test Skill","summary":"A test skill for testing","stats":{"downloads":100}},"latestVersion":{"version":"1.0.0"},"resolvedSlug":"test-skill"}`)
			case "stock-portfolio":
				detailJSON = json.RawMessage(`{"owner":{"handle":"yinshengf"},"skill":{"slug":"stock-portfolio","displayName":"Stock Portfolio","summary":"Stock portfolio manager","stats":{"downloads":148}},"latestVersion":{"version":"1.0.0"},"resolvedSlug":"stock-portfolio"}`)
			case "nonexistent":
				detailJSON = json.RawMessage(`{}`)
			default:
				w.WriteHeader(http.StatusInternalServerError)
				json.NewEncoder(w).Encode(convexResponse{Status: "error", ErrorMessage: "not found"})
				return
			}
			json.NewEncoder(w).Encode(convexResponse{Status: "success", Value: detailJSON})

		default:
			json.NewEncoder(w).Encode(convexResponse{Status: "error", ErrorMessage: "unknown function: " + reqBody.Path})
		}
	}))
	return m
}

func (m *convexMock) URL() string { return m.server.URL }
func (m *convexMock) Close()      { m.server.Close() }

// --- Tests ---

func TestNewClawHubRegistry(t *testing.T) {
	tests := []struct {
		name        string
		cfg         ClawHubConfig
		wantBaseURL string
		wantConvex  string
		wantTimeout int
	}{
		{
			name:        "default config",
			cfg:         ClawHubConfig{},
			wantBaseURL: defaultClawHubBaseURL,
			wantConvex:  defaultConvexURL,
		},
		{
			name: "custom base URL",
			cfg: ClawHubConfig{
				BaseURL: "https://custom.clawhub.ai",
			},
			wantBaseURL: "https://custom.clawhub.ai",
			wantConvex:  defaultConvexURL,
		},
		{
			name: "custom convex URL",
			cfg: ClawHubConfig{
				ConvexURL: "https://custom.convex.cloud",
			},
			wantBaseURL: defaultClawHubBaseURL,
			wantConvex:  "https://custom.convex.cloud",
		},
		{
			name: "custom timeout",
			cfg: ClawHubConfig{
				Timeout: 60,
			},
			wantBaseURL: defaultClawHubBaseURL,
			wantConvex:  defaultConvexURL,
			wantTimeout: 60,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			reg := NewClawHubRegistry(tt.cfg)

			if reg.baseURL != tt.wantBaseURL {
				t.Errorf("expected baseURL %q, got %q", tt.wantBaseURL, reg.baseURL)
			}

			if reg.convexURL != tt.wantConvex {
				t.Errorf("expected convexURL %q, got %q", tt.wantConvex, reg.convexURL)
			}

			if reg.client == nil {
				t.Error("expected client to be initialized")
			}

			if tt.wantTimeout > 0 {
				got := int(reg.timeout.Seconds())
				if got != tt.wantTimeout {
					t.Errorf("expected timeout %ds, got %ds", tt.wantTimeout, got)
				}
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

func TestClawHubRegistry_SearchVector(t *testing.T) {
	searchMock := newClawhubSearchMock()
	defer searchMock.Close()

	reg := NewClawHubRegistry(ClawHubConfig{BaseURL: searchMock.URL()})
	results, err := reg.Search(context.Background(), "test query", 10)

	if err != nil {
		t.Fatalf("Search() error = %v", err)
	}

	if len(results) != 2 {
		t.Fatalf("expected 2 results, got %d", len(results))
	}

	if results[0].Slug != "test-skill" {
		t.Errorf("expected slug 'test-skill', got %q", results[0].Slug)
	}
	if results[0].RegistryName != "clawhub" {
		t.Errorf("expected registry 'clawhub', got %q", results[0].RegistryName)
	}
	if results[0].DisplayName != "Test Skill" {
		t.Errorf("expected display name 'Test Skill', got %q", results[0].DisplayName)
	}
	if results[0].Score <= 0 || results[0].Score > 1.0 {
		t.Errorf("expected normalized score 0-1, got %f", results[0].Score)
	}
}

func TestClawHubRegistry_SearchList(t *testing.T) {
	convexMock := newConvexMock()
	defer convexMock.Close()

	reg := NewClawHubRegistry(ClawHubConfig{ConvexURL: convexMock.URL()})
	results, err := reg.Search(context.Background(), "", 10)

	if err != nil {
		t.Fatalf("Search() error = %v", err)
	}

	if len(results) != 2 {
		t.Fatalf("expected 2 results, got %d", len(results))
	}

	if results[0].Slug != "popular-skill" {
		t.Errorf("expected slug 'popular-skill', got %q", results[0].Slug)
	}
	if results[0].Downloads != 5000 {
		t.Errorf("expected downloads 5000, got %d", results[0].Downloads)
	}
}

func TestClawHubRegistry_SearchEmptyQuery(t *testing.T) {
	convexMock := newConvexMock()
	defer convexMock.Close()

	reg := NewClawHubRegistry(ClawHubConfig{ConvexURL: convexMock.URL()})
	results, err := reg.Search(context.Background(), "", 10)

	if err != nil {
		t.Fatalf("Search() error = %v", err)
	}

	// Empty query uses Convex skills:list, which returns 2 items from the mock
	if len(results) != 2 {
		t.Errorf("expected 2 results for empty query (Convex list), got %d", len(results))
	}
}

func TestClawHubRegistry_SearchError(t *testing.T) {
	// Use a server that always returns 500
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusInternalServerError)
	}))
	defer server.Close()

	reg := NewClawHubRegistry(ClawHubConfig{BaseURL: server.URL})
	_, err := reg.Search(context.Background(), "test", 10)

	if err == nil {
		t.Fatal("expected error for search failure, got nil")
	}
	if !strings.Contains(err.Error(), "search failed") {
		t.Errorf("error should mention 'search failed', got %q", err.Error())
	}
}

func TestClawHubRegistry_GetSkillMeta(t *testing.T) {
	convexMock := newConvexMock()
	defer convexMock.Close()

	reg := NewClawHubRegistry(ClawHubConfig{ConvexURL: convexMock.URL()})
	meta, err := reg.GetSkillMeta(context.Background(), "test-skill")

	if err != nil {
		t.Fatalf("GetSkillMeta() error = %v", err)
	}

	if meta.Slug != "test-skill" {
		t.Errorf("expected slug 'test-skill', got %q", meta.Slug)
	}
	if meta.DisplayName != "Test Skill" {
		t.Errorf("expected display name 'Test Skill', got %q", meta.DisplayName)
	}
	if meta.Summary != "A test skill for testing" {
		t.Errorf("expected summary 'A test skill for testing', got %q", meta.Summary)
	}
	if meta.RegistryName != "clawhub" {
		t.Errorf("expected registry 'clawhub', got %q", meta.RegistryName)
	}
	if meta.LatestVersion != "1.0.0" {
		t.Errorf("expected version '1.0.0', got %q", meta.LatestVersion)
	}
}

func TestClawHubRegistry_GetSkillMeta_NotFound(t *testing.T) {
	convexMock := newConvexMock()
	defer convexMock.Close()

	reg := NewClawHubRegistry(ClawHubConfig{ConvexURL: convexMock.URL()})
	_, err := reg.GetSkillMeta(context.Background(), "nonexistent")

	if err == nil {
		t.Fatal("expected error for not found, got nil")
	}
	if !strings.Contains(err.Error(), "not found") {
		t.Errorf("error should contain 'not found', got %q", err.Error())
	}
}

func TestClawHubRegistry_GetSkillMeta_InvalidSlug(t *testing.T) {
	reg := NewClawHubRegistry(ClawHubConfig{})

	tests := []struct {
		name        string
		slug        string
		errContains string
	}{
		{"empty", "", "cannot be empty"},
		{"path traversal", "../../../etc/passwd", "path separators"},
		{"too long", strings.Repeat("a", 65), "too long"},
		{"slash", "test/skill", "path separators"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			_, err := reg.GetSkillMeta(context.Background(), tt.slug)
			if err == nil {
				t.Fatal("expected error, got nil")
			}
			if !strings.Contains(err.Error(), tt.errContains) {
				t.Errorf("error should contain %q, got %q", tt.errContains, err.Error())
			}
		})
	}
}

func TestClawHubRegistry_DownloadAndInstall_ZIP(t *testing.T) {
	convexMock := newConvexMock()
	defer convexMock.Close()

	// Create a ZIP containing the skill files
	zipBuf := new(bytes.Buffer)
	zw := zip.NewWriter(zipBuf)
	f, _ := zw.CreateHeader(&zip.FileHeader{Name: "SKILL.md", Method: zip.Deflate})
	f.Write([]byte("# Test Skill\n\nFrom ZIP download."))
	f2, _ := zw.CreateHeader(&zip.FileHeader{Name: "scripts/run.sh", Method: zip.Deflate})
	f2.Write([]byte("#!/bin/bash\necho hello"))
	f3, _ := zw.CreateHeader(&zip.FileHeader{Name: "references/api.md", Method: zip.Deflate})
	f3.Write([]byte("# API Reference"))
	zw.Close()

	// Mock server that serves the ZIP download
	siteMock := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if strings.Contains(r.URL.Path, "/api/v1/download") {
			w.Header().Set("Content-Type", "application/zip")
			w.Header().Set("Content-Disposition", `attachment; filename="test-skill-1.0.0.zip"`)
			w.WriteHeader(http.StatusOK)
			w.Write(zipBuf.Bytes())
			return
		}
		w.WriteHeader(http.StatusNotFound)
	}))
	defer siteMock.Close()

	reg := NewClawHubRegistry(ClawHubConfig{ConvexURL: convexMock.URL()})
	reg.convexSiteURL = siteMock.URL
	tempDir := t.TempDir()

	result, err := reg.DownloadAndInstall(context.Background(), "test-skill", "", tempDir)
	if err != nil {
		t.Fatalf("DownloadAndInstall() error = %v", err)
	}

	// Verify SKILL.md was extracted
	data, err := os.ReadFile(filepath.Join(tempDir, "SKILL.md"))
	if err != nil {
		t.Fatalf("SKILL.md should exist: %v", err)
	}
	if string(data) != "# Test Skill\n\nFrom ZIP download." {
		t.Errorf("SKILL.md content mismatch, got %q", string(data))
	}

	// Verify subdirectory files were extracted
	data, err = os.ReadFile(filepath.Join(tempDir, "scripts", "run.sh"))
	if err != nil {
		t.Fatalf("scripts/run.sh should exist: %v", err)
	}
	if string(data) != "#!/bin/bash\necho hello" {
		t.Errorf("scripts/run.sh content mismatch, got %q", string(data))
	}

	data, err = os.ReadFile(filepath.Join(tempDir, "references", "api.md"))
	if err != nil {
		t.Fatalf("references/api.md should exist: %v", err)
	}
	if string(data) != "# API Reference" {
		t.Errorf("references/api.md content mismatch, got %q", string(data))
	}

	if result.Version != "1.0.0" {
		t.Errorf("expected version '1.0.0', got %q", result.Version)
	}
	if result.Summary != "A test skill for testing" {
		t.Errorf("expected summary 'A test skill for testing', got %q", result.Summary)
	}
}

func TestClawHubRegistry_DownloadAndInstall_ZIP_TopLevelDir(t *testing.T) {
	convexMock := newConvexMock()
	defer convexMock.Close()

	// Create a ZIP with a single top-level directory that should be flattened
	zipBuf := new(bytes.Buffer)
	zw := zip.NewWriter(zipBuf)
	f, _ := zw.CreateHeader(&zip.FileHeader{Name: "test-skill/SKILL.md", Method: zip.Deflate})
	f.Write([]byte("# Test Skill\n\nFlattened from top-level dir."))
	f2, _ := zw.CreateHeader(&zip.FileHeader{Name: "test-skill/config.json", Method: zip.Deflate})
	f2.Write([]byte(`{"key":"value"}`))
	zw.Close()

	siteMock := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if strings.Contains(r.URL.Path, "/api/v1/download") {
			w.Header().Set("Content-Type", "application/zip")
			w.WriteHeader(http.StatusOK)
			w.Write(zipBuf.Bytes())
			return
		}
		w.WriteHeader(http.StatusNotFound)
	}))
	defer siteMock.Close()

	reg := NewClawHubRegistry(ClawHubConfig{ConvexURL: convexMock.URL()})
	reg.convexSiteURL = siteMock.URL
	tempDir := t.TempDir()

	result, err := reg.DownloadAndInstall(context.Background(), "test-skill", "", tempDir)
	if err != nil {
		t.Fatalf("DownloadAndInstall() error = %v", err)
	}

	// Files should be flattened (no "test-skill/" subdirectory)
	data, err := os.ReadFile(filepath.Join(tempDir, "SKILL.md"))
	if err != nil {
		t.Fatalf("SKILL.md should exist at target root: %v", err)
	}
	if string(data) != "# Test Skill\n\nFlattened from top-level dir." {
		t.Errorf("SKILL.md content mismatch, got %q", string(data))
	}

	data, err = os.ReadFile(filepath.Join(tempDir, "config.json"))
	if err != nil {
		t.Fatalf("config.json should exist at target root: %v", err)
	}
	if string(data) != `{"key":"value"}` {
		t.Errorf("config.json content mismatch, got %q", string(data))
	}

	_ = result
}

func TestClawHubRegistry_DownloadAndInstall_FallbackToTreesAPI(t *testing.T) {
	t.Skip("Skipping: fallback hits real GitHub API (openclaw/skills) which is too large for unit tests. " +
		"The Trees API download path is tested via GitHub registry tests.")

	convexMock := newConvexMock()
	defer convexMock.Close()

	// Mock server that fails ZIP download to trigger fallback
	siteMock := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusNotFound)
	}))
	defer siteMock.Close()

	reg := NewClawHubRegistry(ClawHubConfig{ConvexURL: convexMock.URL()})
	reg.convexSiteURL = siteMock.URL

	// Use short timeout to avoid hanging on real GitHub API
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	tempDir := t.TempDir()

	// The fallback calls the real GitHub API, so it will fail because
	// testowner/test-skill doesn't exist on real GitHub.
	_, err := reg.DownloadAndInstall(ctx, "test-skill", "", tempDir)
	if err == nil {
		// If it somehow succeeded, verify files exist
		skillFile := filepath.Join(tempDir, "SKILL.md")
		if _, statErr := os.Stat(skillFile); os.IsNotExist(statErr) {
			t.Error("SKILL.md should exist after successful install")
		}
	} else {
		if !strings.Contains(err.Error(), "all download strategies failed") && !errors.Is(ctx.Err(), context.DeadlineExceeded) {
			t.Errorf("expected 'all download strategies failed' error, got %q", err.Error())
		}
	}
}

func TestClawHubRegistry_DownloadAndInstall(t *testing.T) {
	convexMock := newConvexMock()
	defer convexMock.Close()

	reg := NewClawHubRegistry(ClawHubConfig{ConvexURL: convexMock.URL()})

	// Without site URL override, the ZIP download will fail (empty derived URL).
	// The fallback to GitHub Trees API will also fail (or timeout).
	// Use a short timeout to avoid hanging on the large openclaw/skills repo.
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	_, err := reg.DownloadAndInstall(ctx, "test-skill", "", t.TempDir())
	if err == nil {
		// If it succeeded, verify the file exists
		skillFile := filepath.Join(t.TempDir(), "SKILL.md")
		if _, statErr := os.Stat(skillFile); os.IsNotExist(statErr) {
			t.Error("SKILL.md should exist after successful install")
		}
	} else {
		// Expected: download failed
		if !strings.Contains(err.Error(), "all download strategies failed") && !errors.Is(ctx.Err(), context.DeadlineExceeded) {
			t.Errorf("expected download failure, got %q", err.Error())
		}
	}
}

func TestClawHubRegistry_DownloadAndInstall_InvalidSlug(t *testing.T) {
	reg := NewClawHubRegistry(ClawHubConfig{})

	_, err := reg.DownloadAndInstall(context.Background(), "../../../etc/passwd", "", t.TempDir())
	if err == nil {
		t.Fatal("expected error for invalid slug, got nil")
	}
	if !strings.Contains(err.Error(), "invalid skill slug") {
		t.Errorf("error should contain 'invalid skill slug', got %q", err.Error())
	}
}

func TestClawHubRegistry_DownloadAndInstall_NoOwner(t *testing.T) {
	convexMock := newConvexMock()
	defer convexMock.Close()

	reg := NewClawHubRegistry(ClawHubConfig{ConvexURL: convexMock.URL()})
	_, err := reg.DownloadAndInstall(context.Background(), "nonexistent", "", t.TempDir())
	if err == nil {
		t.Fatal("expected error for missing owner, got nil")
	}
	if !strings.Contains(err.Error(), "owner handle not found") {
		t.Errorf("error should contain 'owner handle not found', got %q", err.Error())
	}
}

func TestValidateSkillIdentifier(t *testing.T) {
	tests := []struct {
		name        string
		slug        string
		wantErr     bool
		errContains string
	}{
		{"valid slug", "test-skill", false, ""},
		{"valid with numbers", "test-skill-123", false, ""},
		{"valid with underscores", "test_skill", false, ""},
		{"empty", "", true, "cannot be empty"},
		{"whitespace only", "   ", true, "cannot be empty"},
		{"forward slash", "test/skill", true, "path separators"},
		{"backslash", "test\\skill", true, "path separators"},
		{"double dot", "../test", true, "path separators"},
		{"too long", strings.Repeat("a", 65), true, "too long"},
		{"path traversal", "../../../etc/passwd", true, "path separators"},
		{"exactly 64 chars", strings.Repeat("a", 64), false, ""},
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

func TestClawHubRegistry_ConvexResponseParsing(t *testing.T) {
	// Test that callConvex correctly unwraps the {"status":"success","value":...} envelope
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		resp := convexResponse{
			Status: "success",
			Value:  json.RawMessage(`{"slug":"test","name":"Test"}`),
		}
		json.NewEncoder(w).Encode(resp)
	}))
	defer server.Close()

	reg := NewClawHubRegistry(ClawHubConfig{ConvexURL: server.URL})
	value, err := reg.callConvex(context.Background(), "test:function", map[string]string{"key": "val"})
	if err != nil {
		t.Fatalf("callConvex() error = %v", err)
	}

	var result map[string]string
	if err := json.Unmarshal(value, &result); err != nil {
		t.Fatalf("failed to unmarshal value: %v", err)
	}

	if result["slug"] != "test" {
		t.Errorf("expected slug 'test', got %q", result["slug"])
	}
}

func TestClawHubRegistry_ConvexError(t *testing.T) {
	// Test that callConvex correctly handles error responses
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		resp := convexResponse{
			Status:       "error",
			ErrorMessage: "something went wrong",
		}
		json.NewEncoder(w).Encode(resp)
	}))
	defer server.Close()

	reg := NewClawHubRegistry(ClawHubConfig{ConvexURL: server.URL})
	_, err := reg.callConvex(context.Background(), "test:function", nil)
	if err == nil {
		t.Fatal("expected error for convex error response, got nil")
	}
	if !strings.Contains(err.Error(), "something went wrong") {
		t.Errorf("error should contain 'something went wrong', got %q", err.Error())
	}
}
