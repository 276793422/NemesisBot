package forge

import (
	"bufio"
	"encoding/json"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"
)

// AggregatedExperience represents a deduplicated pattern aggregate.
type AggregatedExperience struct {
	PatternHash   string    `json:"pattern_hash"`
	ToolName      string    `json:"tool_name"`
	Count         int       `json:"count"`
	AvgDurationMs int64     `json:"avg_duration_ms"`
	SuccessRate   float64   `json:"success_rate"`
	LastSeen      time.Time `json:"last_seen"`
}

// ExperienceStore manages JSONL-based persistence of experience data.
// Files are organized by month: experiences/202604/20260420.jsonl
type ExperienceStore struct {
	baseDir string
	config  *ForgeConfig
}

// NewExperienceStore creates a new experience store rooted at forgeDir.
func NewExperienceStore(forgeDir string, config *ForgeConfig) *ExperienceStore {
	return &ExperienceStore{
		baseDir: filepath.Join(forgeDir, "experiences"),
		config:  config,
	}
}

// AppendAggregated writes an aggregated experience record to today's JSONL file.
func (s *ExperienceStore) AppendAggregated(rec *AggregatedExperience) error {
	now := time.Now().UTC()
	monthDir := filepath.Join(s.baseDir, now.Format("200601"))
	if err := os.MkdirAll(monthDir, 0755); err != nil {
		return err
	}

	filePath := filepath.Join(monthDir, now.Format("20060102")+".jsonl")

	// Check daily limit
	if s.config != nil && s.config.Collection.MaxExperiencesPerDay > 0 {
		count, err := s.countLines(filePath)
		if err == nil && count >= s.config.Collection.MaxExperiencesPerDay {
			return nil // Skip silently
		}
	}

	f, err := os.OpenFile(filePath, os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0644)
	if err != nil {
		return err
	}
	defer f.Close()

	data, err := json.Marshal(rec)
	if err != nil {
		return err
	}

	_, err = fmt.Fprintf(f, "%s\n", data)
	return err
}

// ReadAggregated reads all aggregated experiences for the given time range.
func (s *ExperienceStore) ReadAggregated(since time.Time) ([]*AggregatedExperience, error) {
	var results []*AggregatedExperience

	// Walk through month directories
	err := filepath.WalkDir(s.baseDir, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return nil // Skip errors
		}
		if d.IsDir() || !strings.HasSuffix(d.Name(), ".jsonl") {
			return nil
		}

		// Check file date from name
		if !s.fileNewerThan(d.Name(), since) {
			return nil
		}

		// Read JSONL file
		records, err := s.readJSONL(path)
		if err != nil {
			return nil
		}
		results = append(results, records...)
		return nil
	})

	return results, err
}

// ReadAggregatedByDay reads aggregated experiences grouped by day.
func (s *ExperienceStore) ReadAggregatedByDay(since time.Time) (map[string][]*AggregatedExperience, error) {
	records, err := s.ReadAggregated(since)
	if err != nil {
		return nil, err
	}

	grouped := make(map[string][]*AggregatedExperience)
	for _, r := range records {
		day := r.LastSeen.Format("2006-01-02")
		grouped[day] = append(grouped[day], r)
	}
	return grouped, nil
}

// GetTopPatterns returns the top N patterns by count in the given time range.
func (s *ExperienceStore) GetTopPatterns(since time.Time, topN int) ([]*AggregatedExperience, error) {
	records, err := s.ReadAggregated(since)
	if err != nil {
		return nil, err
	}

	// Merge by pattern hash
	merged := make(map[string]*AggregatedExperience)
	for _, r := range records {
		if existing, ok := merged[r.PatternHash]; ok {
			existing.Count += r.Count
			existing.AvgDurationMs = (existing.AvgDurationMs + r.AvgDurationMs) / 2
			existing.SuccessRate = (existing.SuccessRate + r.SuccessRate) / 2
			if r.LastSeen.After(existing.LastSeen) {
				existing.LastSeen = r.LastSeen
			}
		} else {
			merged[r.PatternHash] = &AggregatedExperience{
				PatternHash:   r.PatternHash,
				ToolName:      r.ToolName,
				Count:         r.Count,
				AvgDurationMs: r.AvgDurationMs,
				SuccessRate:   r.SuccessRate,
				LastSeen:      r.LastSeen,
			}
		}
	}

	// Sort by count descending
	sorted := make([]*AggregatedExperience, 0, len(merged))
	for _, r := range merged {
		sorted = append(sorted, r)
	}
	sort.Slice(sorted, func(i, j int) bool {
		return sorted[i].Count > sorted[j].Count
	})

	if topN > 0 && len(sorted) > topN {
		sorted = sorted[:topN]
	}

	return sorted, nil
}

// Cleanup removes experience files older than maxAgeDays.
func (s *ExperienceStore) Cleanup(maxAgeDays int) error {
	cutoff := time.Now().UTC().AddDate(0, 0, -maxAgeDays)

	return filepath.WalkDir(s.baseDir, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return nil
		}
		if d.IsDir() {
			return nil
		}
		if !strings.HasSuffix(d.Name(), ".jsonl") {
			return nil
		}

		if !s.fileNewerThan(d.Name(), cutoff) {
			os.Remove(path)
		}
		return nil
	})
}

// readJSONL reads all records from a JSONL file.
func (s *ExperienceStore) readJSONL(path string) ([]*AggregatedExperience, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	defer f.Close()

	var records []*AggregatedExperience
	scanner := bufio.NewScanner(f)
	scanner.Buffer(make([]byte, 1024*1024), 1024*1024) // 1MB buffer

	for scanner.Scan() {
		line := strings.TrimSpace(scanner.Text())
		if line == "" {
			continue
		}
		var rec AggregatedExperience
		if err := json.Unmarshal([]byte(line), &rec); err != nil {
			continue // Skip malformed lines
		}
		records = append(records, &rec)
	}

	return records, scanner.Err()
}

// countLines counts lines in a file.
func (s *ExperienceStore) countLines(path string) (int, error) {
	f, err := os.Open(path)
	if err != nil {
		return 0, err
	}
	defer f.Close()

	count := 0
	scanner := bufio.NewScanner(f)
	for scanner.Scan() {
		if strings.TrimSpace(scanner.Text()) != "" {
			count++
		}
	}
	return count, scanner.Err()
}

// fileNewerThan checks if a JSONL filename (YYYYMMDD.jsonl) is newer than the given time.
func (s *ExperienceStore) fileNewerThan(filename string, since time.Time) bool {
	// Extract date from filename like "20260420.jsonl"
	name := strings.TrimSuffix(filename, ".jsonl")
	fileDate, err := time.Parse("20060102", name)
	if err != nil {
		return true // If we can't parse, include it
	}
	return fileDate.After(since) || fileDate.Equal(since)
}

// GetStats returns summary statistics for the experience store.
func (s *ExperienceStore) GetStats() (totalRecords int, uniquePatterns int, err error) {
	records, err := s.ReadAggregated(time.Time{}) // All time
	if err != nil {
		return 0, 0, err
	}

	patterns := make(map[string]bool)
	for _, r := range records {
		totalRecords += r.Count
		patterns[r.PatternHash] = true
	}

	return totalRecords, len(patterns), nil
}
