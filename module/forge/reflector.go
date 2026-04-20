package forge

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"

	"github.com/276793422/NemesisBot/module/providers"
)

// Reflector analyzes experience data to generate insights and improvement suggestions.
// It has two levels: statistical (pure code, zero tokens) and semantic (LLM-based).
type Reflector struct {
	forgeDir   string
	store      *ExperienceStore
	registry   *Registry
	config     *ForgeConfig
	provider   providers.LLMProvider
	traceStore *TraceStore // Phase 5: for conversation-level analysis
}

// NewReflector creates a new reflection engine.
func NewReflector(forgeDir string, store *ExperienceStore, registry *Registry, config *ForgeConfig) *Reflector {
	return &Reflector{
		forgeDir: forgeDir,
		store:    store,
		registry: registry,
		config:   config,
	}
}

// SetProvider sets the LLM provider for semantic reflection.
func (r *Reflector) SetProvider(provider providers.LLMProvider) {
	r.provider = provider
}

// Reflect performs a full reflection cycle and returns the report file path.
func (r *Reflector) Reflect(ctx context.Context, period string, focus string) (string, error) {
	since := r.resolvePeriod(period)

	// Read experience data
	records, err := r.store.ReadAggregated(since)
	if err != nil {
		return "", fmt.Errorf("failed to read experiences: %w", err)
	}

	if len(records) < r.config.Reflection.MinExperiences {
		return "", fmt.Errorf("insufficient experiences (%d < %d minimum)",
			len(records), r.config.Reflection.MinExperiences)
	}

	// Stage 1: Statistical analysis
	stats := r.statisticalAnalysis(records)

	// Stage 1.5: Conversation-level trace analysis (Phase 5)
	var traceStats *TraceStats
	if r.traceStore != nil {
		traceStats = r.analyzeTraces(since)
	}

	// Stage 2: Get existing artifacts for coverage analysis
	artifacts := r.registry.ListAll()

	// Stage 3: Build report
	report := r.buildReport(stats, artifacts, period, focus, traceStats)

	// Stage 4: Semantic analysis (if LLM available and enabled)
	if r.config.Reflection.UseLLM && r.provider != nil {
		llmInsights, err := semanticAnalysis(ctx, r.provider, stats, artifacts, traceStats, r.config)
		if err == nil {
			report.LLMInsights = llmInsights
		}
	}

	// Write report
	return r.writeReport(report)
}

// ReflectionStats holds statistical analysis results.
type ReflectionStats struct {
	TotalRecords   int
	UniquePatterns int
	AvgSuccessRate float64
	TopPatterns    []*PatternInsight
	LowSuccess     []*PatternInsight
	ToolFrequency  map[string]int
}

// PatternInsight holds insight data for a tool usage pattern.
type PatternInsight struct {
	PatternHash   string
	ToolName      string
	Count         int
	AvgDurationMs int64
	SuccessRate   float64
	Suggestion    string
}

// resolvePeriod converts a period string to a time.Time.
func (r *Reflector) resolvePeriod(period string) time.Time {
	switch period {
	case "today":
		now := time.Now().UTC()
		return time.Date(now.Year(), now.Month(), now.Day(), 0, 0, 0, 0, time.UTC)
	case "week":
		return time.Now().UTC().AddDate(0, 0, -7)
	case "all":
		return time.Time{}
	default:
		now := time.Now().UTC()
		return time.Date(now.Year(), now.Month(), now.Day(), 0, 0, 0, 0, time.UTC)
	}
}

// statisticalAnalysis performs pure-code analysis on experience records.
func (r *Reflector) statisticalAnalysis(records []*AggregatedExperience) *ReflectionStats {
	stats := &ReflectionStats{
		ToolFrequency: make(map[string]int),
	}

	var totalRate float64
	for _, rec := range records {
		stats.TotalRecords += rec.Count
		stats.ToolFrequency[rec.ToolName] += rec.Count
		totalRate += rec.SuccessRate * float64(rec.Count)
	}
	stats.UniquePatterns = len(records)

	if stats.TotalRecords > 0 {
		stats.AvgSuccessRate = totalRate / float64(stats.TotalRecords)
	}

	// Sort by count for top patterns
	sorted := make([]*AggregatedExperience, len(records))
	copy(sorted, records)
	sort.Slice(sorted, func(i, j int) bool {
		return sorted[i].Count > sorted[j].Count
	})

	// Top 10 patterns
	topN := 10
	if len(sorted) < topN {
		topN = len(sorted)
	}
	for i := 0; i < topN; i++ {
		rec := sorted[i]
		stats.TopPatterns = append(stats.TopPatterns, &PatternInsight{
			PatternHash:   rec.PatternHash,
			ToolName:      rec.ToolName,
			Count:         rec.Count,
			AvgDurationMs: rec.AvgDurationMs,
			SuccessRate:   rec.SuccessRate,
			Suggestion:    r.generateSuggestion(rec),
		})
	}

	// Low success patterns (success rate < 0.7)
	for _, rec := range records {
		if rec.SuccessRate < 0.7 && rec.Count >= 3 {
			stats.LowSuccess = append(stats.LowSuccess, &PatternInsight{
				PatternHash:   rec.PatternHash,
				ToolName:      rec.ToolName,
				Count:         rec.Count,
				AvgDurationMs: rec.AvgDurationMs,
				SuccessRate:   rec.SuccessRate,
				Suggestion:    "Low success rate - investigate failure causes and improve error handling",
			})
		}
	}

	return stats
}

// generateSuggestion creates a basic improvement suggestion based on pattern data.
func (r *Reflector) generateSuggestion(rec *AggregatedExperience) string {
	if rec.SuccessRate >= 0.9 && rec.Count >= 5 {
		return fmt.Sprintf("High frequency (%d uses), consider creating a Skill for this pattern", rec.Count)
	}
	if rec.SuccessRate >= 0.7 && rec.Count >= 3 {
		return "Stable pattern, monitor for potential Skill creation"
	}
	if rec.SuccessRate < 0.7 {
		return "Review failure modes and consider adding error handling"
	}
	return "Normal usage pattern"
}

// ReflectionReport represents a generated reflection report.
type ReflectionReport struct {
	Date        string
	Period      string
	Focus       string
	Stats       *ReflectionStats
	Artifacts   []Artifact
	LLMInsights string
	TraceStats  *TraceStats // Phase 5: conversation-level insights
}

// buildReport constructs the reflection report.
func (r *Reflector) buildReport(stats *ReflectionStats, artifacts []Artifact, period, focus string, traceStats *TraceStats) *ReflectionReport {
	return &ReflectionReport{
		Date:       time.Now().UTC().Format("2006-01-02"),
		Period:     period,
		Focus:      focus,
		Stats:      stats,
		Artifacts:  artifacts,
		TraceStats: traceStats,
	}
}

// writeReport writes the reflection report as a Markdown file.
func (r *Reflector) writeReport(report *ReflectionReport) (string, error) {
	reflectionsDir := filepath.Join(r.forgeDir, "reflections")
	if err := os.MkdirAll(reflectionsDir, 0755); err != nil {
		return "", err
	}

	filename := fmt.Sprintf("%s.md", report.Date)
	path := filepath.Join(reflectionsDir, filename)

	content := FormatReport(report)

	if err := os.WriteFile(path, []byte(content), 0644); err != nil {
		return "", err
	}

	return path, nil
}

// CleanupReports removes reflection reports older than maxAgeDays.
func (r *Reflector) CleanupReports(maxAgeDays int) error {
	reflectionsDir := filepath.Join(r.forgeDir, "reflections")
	cutoff := time.Now().UTC().AddDate(0, 0, -maxAgeDays)

	entries, err := os.ReadDir(reflectionsDir)
	if err != nil {
		if os.IsNotExist(err) {
			return nil
		}
		return err
	}

	for _, entry := range entries {
		if entry.IsDir() {
			continue
		}

		name := entry.Name()
		if len(name) < 10 {
			continue
		}
		fileDate, err := time.Parse("2006-01-02", name[:10])
		if err != nil {
			continue
		}

		if fileDate.Before(cutoff) {
			os.Remove(filepath.Join(reflectionsDir, name))
		}
	}

	return nil
}

// GetLatestReport returns the path to the most recent reflection report.
func (r *Reflector) GetLatestReport() (string, error) {
	reflectionsDir := filepath.Join(r.forgeDir, "reflections")
	entries, err := os.ReadDir(reflectionsDir)
	if err != nil {
		return "", err
	}

	if len(entries) == 0 {
		return "", fmt.Errorf("no reflection reports found")
	}

	var latest string
	var latestTime time.Time
	for _, entry := range entries {
		if entry.IsDir() || len(entry.Name()) < 10 {
			continue
		}
		t, err := time.Parse("2006-01-02", entry.Name()[:10])
		if err != nil {
			continue
		}
		if t.After(latestTime) {
			latestTime = t
			latest = filepath.Join(reflectionsDir, entry.Name())
		}
	}

	if latest == "" {
		return "", fmt.Errorf("no valid reflection reports found")
	}

	return latest, nil
}

// MergedInsights holds the result of merging local and remote reflection reports.
type MergedInsights struct {
	LocalPatterns    []*PatternInsight
	RemotePatterns   []*PatternInsight
	MergedPatterns   []*PatternInsight
	CommonTools      map[string]int
	UniqueRemoteTools []string
}

// MergeRemoteReflections reads remote reflection reports and merges their insights
// with local patterns. It extracts tool usage patterns from remote reports and
// combines them with local data for richer analysis.
func (r *Reflector) MergeRemoteReflections(remoteReports []string) *MergedInsights {
	result := &MergedInsights{
		CommonTools:      make(map[string]int),
		UniqueRemoteTools: []string{},
	}

	// Collect local patterns for comparison
	localStats := r.getLocalPatterns()
	result.LocalPatterns = localStats.TopPatterns

	// Track local tool names
	localTools := make(map[string]bool)
	for _, p := range localStats.TopPatterns {
		localTools[p.ToolName] = true
	}

	// Extract patterns from remote reports
	remoteToolFreq := make(map[string]int)
	for _, reportPath := range remoteReports {
		toolFreq := r.extractToolPatternsFromReport(reportPath)
		for tool, count := range toolFreq {
			remoteToolFreq[tool] += count
		}
	}

	// Build remote patterns
	for tool, count := range remoteToolFreq {
		result.RemotePatterns = append(result.RemotePatterns, &PatternInsight{
			ToolName: tool,
			Count:    count,
		})
		if localTools[tool] {
			result.CommonTools[tool] = count
		} else {
			result.UniqueRemoteTools = append(result.UniqueRemoteTools, tool)
		}
	}

	// Merge: start with local patterns, add unique remote patterns
	merged := make([]*PatternInsight, len(localStats.TopPatterns))
	copy(merged, localStats.TopPatterns)

	for _, rp := range result.RemotePatterns {
		found := false
		for _, mp := range merged {
			if mp.ToolName == rp.ToolName {
				mp.Count += rp.Count
				found = true
				break
			}
		}
		if !found {
			merged = append(merged, rp)
		}
	}
	result.MergedPatterns = merged

	return result
}

// getLocalPatterns runs a lightweight statistical analysis on local experiences.
func (r *Reflector) getLocalPatterns() *ReflectionStats {
	records, err := r.store.ReadAggregated(time.Time{})
	if err != nil || len(records) == 0 {
		return &ReflectionStats{ToolFrequency: make(map[string]int)}
	}
	return r.statisticalAnalysis(records)
}

// extractToolPatternsFromReport reads a remote report file and extracts tool usage
// patterns by scanning for common report markers.
func (r *Reflector) extractToolPatternsFromReport(reportPath string) map[string]int {
	freq := make(map[string]int)

	content, err := os.ReadFile(reportPath)
	if err != nil {
		return freq
	}

	text := string(content)

	// Look for tool names in common report sections
	// Pattern: lines like "| tool_name | count |" in markdown tables
	lines := strings.Split(text, "\n")
	for _, line := range lines {
		line = strings.TrimSpace(line)
		// Skip separator lines
		if strings.HasPrefix(line, "|-") || strings.HasPrefix(line, "| ---") {
			continue
		}
		// Look for table rows with tool data
		if strings.HasPrefix(line, "|") && strings.Count(line, "|") >= 3 {
			fields := strings.Split(line, "|")
			// Clean fields
			for i := range fields {
				fields[i] = strings.TrimSpace(fields[i])
			}
			// Try to find tool name + count pattern
			for i := 1; i < len(fields)-1; i++ {
				toolName := fields[i]
				if toolName != "" && !strings.Contains(toolName, "---") && !strings.Contains(toolName, "工具") && !strings.Contains(toolName, "Tool") && !strings.Contains(toolName, "名称") && !strings.Contains(toolName, "Name") {
					// Check if next field looks like a count
					if i+1 < len(fields) {
						count := 0
						fmt.Sscanf(fields[i+1], "%d", &count)
						if count > 0 {
							freq[toolName] += count
							break // one tool per row
						}
					}
				}
			}
		}
	}

	// Also look for "高频模式" or tool mentions in section headers
	toolKeywords := []string{"read_file", "write_file", "edit_file", "exec", "file_read",
		"file_write", "file_edit", "process_exec", "network_request", "http_request",
		"web_search", "code_execute", "shell", "bash"}
	for _, tool := range toolKeywords {
		count := strings.Count(text, tool)
		if count > 0 {
			freq[tool] += count
		}
	}

	return freq
}

// --- Phase 5: Conversation-level trace analysis ---

// TraceStats holds conversation-level statistical analysis results.
type TraceStats struct {
	TotalTraces       int
	AvgRounds         float64
	AvgDurationMs     int64
	ToolChainPatterns []*ToolChainPattern
	RetryPatterns     []*RetryPattern
	EfficiencyScore   float64 // 0-1, higher is better
	SignalSummary     map[string]int
}

// ToolChainPattern represents a frequently occurring tool call sequence.
type ToolChainPattern struct {
	Chain       string  // "read_file→edit_file→exec"
	Count       int
	AvgRounds   float64
	SuccessRate float64
}

// RetryPattern represents a tool that was retried after failure.
type RetryPattern struct {
	ToolName    string
	RetryCount  int
	SuccessRate float64 // success rate after retry
}

// traceStore is an interface for reading traces, injected into Reflector for testability.
type traceReader interface {
	ReadTraces(since time.Time) ([]*ConversationTrace, error)
}

// analyzeTraces performs pure-code analysis on conversation traces.
func (r *Reflector) analyzeTraces(since time.Time) *TraceStats {
	// Use injected trace store if available
	if r.traceStore == nil {
		return nil
	}

	traces, err := r.traceStore.ReadTraces(since)
	if err != nil || len(traces) == 0 {
		return nil
	}

	stats := &TraceStats{
		TotalTraces:   len(traces),
		SignalSummary: make(map[string]int),
	}

	var totalRounds float64
	var totalDuration int64
	var totalSteps int
	var totalTokens int

	// Track tool chains and retry patterns
	chainCounts := make(map[string]*chainStats)
	toolRetries := make(map[string]*retryStats)

	for _, t := range traces {
		totalRounds += float64(t.TotalRounds)
		totalDuration += t.DurationMs
		totalSteps += len(t.ToolSteps)
		totalTokens += t.TokensUsed

		// Extract tool chain per trace
		chain := extractToolChain(t.ToolSteps)
		if chain != "" {
			if cs, ok := chainCounts[chain]; ok {
				cs.count++
				cs.totalRounds += float64(t.TotalRounds)
				if len(t.Signals) == 0 {
					cs.successes++
				}
			} else {
				chainCounts[chain] = &chainStats{
					count:       1,
					totalRounds: float64(t.TotalRounds),
					successes:   0,
				}
				if len(t.Signals) == 0 {
					chainCounts[chain].successes = 1
				}
			}
		}

		// Aggregate signals
		for _, sig := range t.Signals {
			stats.SignalSummary[sig.Type]++
		}

		// Track retry patterns per tool
		for _, step := range t.ToolSteps {
			if rs, ok := toolRetries[step.ToolName]; ok {
				rs.totalCalls++
				if step.Success {
					rs.successes++
				}
			} else {
				rs := &retryStats{totalCalls: 1}
				if step.Success {
					rs.successes = 1
				}
				toolRetries[step.ToolName] = rs
			}
		}
	}

	stats.AvgRounds = totalRounds / float64(len(traces))
	stats.AvgDurationMs = totalDuration / int64(len(traces))

	// Efficiency: ratio of tool steps to rounds (lower rounds per step = more efficient)
	if totalSteps > 0 && totalRounds > 0 {
		stats.EfficiencyScore = float64(totalSteps) / totalRounds
		if stats.EfficiencyScore > 1.0 {
			stats.EfficiencyScore = 1.0
		}
	}

	// Build top tool chain patterns
	chains := make([]*ToolChainPattern, 0, len(chainCounts))
	for chain, cs := range chainCounts {
		avgR := cs.totalRounds / float64(cs.count)
		sr := float64(cs.successes) / float64(cs.count)
		chains = append(chains, &ToolChainPattern{
			Chain:       chain,
			Count:       cs.count,
			AvgRounds:   avgR,
			SuccessRate: sr,
		})
	}
	sort.Slice(chains, func(i, j int) bool {
		return chains[i].Count > chains[j].Count
	})
	if len(chains) > 5 {
		chains = chains[:5]
	}
	stats.ToolChainPatterns = chains

	// Build retry patterns for tools that appear in retry signals
	for tool, rs := range toolRetries {
		if rs.totalCalls >= 2 && stats.SignalSummary["retry"] > 0 {
			stats.RetryPatterns = append(stats.RetryPatterns, &RetryPattern{
				ToolName:    tool,
				RetryCount:  rs.totalCalls,
				SuccessRate: float64(rs.successes) / float64(rs.totalCalls),
			})
		}
	}
	sort.Slice(stats.RetryPatterns, func(i, j int) bool {
		return stats.RetryPatterns[i].RetryCount > stats.RetryPatterns[j].RetryCount
	})
	if len(stats.RetryPatterns) > 5 {
		stats.RetryPatterns = stats.RetryPatterns[:5]
	}

	return stats
}

type chainStats struct {
	count       int
	totalRounds float64
	successes   int
}

type retryStats struct {
	totalCalls int
	successes  int
}

// extractToolChain builds a "tool1→tool2→tool3" string from tool steps.
func extractToolChain(steps []ToolStep) string {
	if len(steps) == 0 {
		return ""
	}
	names := make([]string, 0, len(steps))
	for _, s := range steps {
		names = append(names, s.ToolName)
	}
	return strings.Join(names, "→")
}

// SetTraceStore injects a trace store for conversation-level analysis.
func (r *Reflector) SetTraceStore(store *TraceStore) {
	r.traceStore = store
}

// AnalyzeTracesForTest exposes analyzeTraces for testing.
func (r *Reflector) AnalyzeTracesForTest(since time.Time) *TraceStats {
	return r.analyzeTraces(since)
}
