package forge

import (
	"bufio"
	"encoding/json"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"strings"
	"time"
)

// CycleStore persists learning cycles as JSONL files.
// Directory structure: learning/202604/20260421.jsonl
type CycleStore struct {
	baseDir string
	config  *ForgeConfig
}

// NewCycleStore creates a new CycleStore rooted at forgeDir/learning.
func NewCycleStore(forgeDir string, config *ForgeConfig) *CycleStore {
	return &CycleStore{
		baseDir: filepath.Join(forgeDir, "learning"),
		config:  config,
	}
}

// Append writes a learning cycle to today's JSONL file.
func (s *CycleStore) Append(cycle *LearningCycle) error {
	monthDir := filepath.Join(s.baseDir, cycle.StartedAt.Format("200601"))
	if err := os.MkdirAll(monthDir, 0755); err != nil {
		return err
	}

	filePath := filepath.Join(monthDir, cycle.StartedAt.Format("20060102")+".jsonl")

	f, err := os.OpenFile(filePath, os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0644)
	if err != nil {
		return err
	}
	defer f.Close()

	data, err := json.Marshal(cycle)
	if err != nil {
		return err
	}

	_, err = fmt.Fprintf(f, "%s\n", data)
	return err
}

// ReadCycles reads all learning cycles since the given time.
func (s *CycleStore) ReadCycles(since time.Time) ([]*LearningCycle, error) {
	var results []*LearningCycle

	err := filepath.WalkDir(s.baseDir, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return nil
		}
		if d.IsDir() || !strings.HasSuffix(d.Name(), ".jsonl") {
			return nil
		}
		if !s.fileNewerThan(d.Name(), since) {
			return nil
		}

		records, err := s.readJSONL(path)
		if err != nil {
			return nil
		}
		results = append(results, records...)
		return nil
	})

	return results, err
}

// LoadLatestCycle returns the most recent learning cycle.
func (s *CycleStore) LoadLatestCycle() (*LearningCycle, error) {
	cycles, err := s.ReadCycles(time.Time{}) // read all
	if err != nil || len(cycles) == 0 {
		return nil, fmt.Errorf("no learning cycles found")
	}
	return cycles[len(cycles)-1], nil
}

// Cleanup removes cycle files older than maxAgeDays.
func (s *CycleStore) Cleanup(maxAgeDays int) error {
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

func (s *CycleStore) readJSONL(path string) ([]*LearningCycle, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	defer f.Close()

	var records []*LearningCycle
	scanner := bufio.NewScanner(f)
	scanner.Buffer(make([]byte, 1024*1024), 1024*1024)

	for scanner.Scan() {
		line := strings.TrimSpace(scanner.Text())
		if line == "" {
			continue
		}
		var cycle LearningCycle
		if err := json.Unmarshal([]byte(line), &cycle); err != nil {
			continue
		}
		records = append(records, &cycle)
	}

	return records, scanner.Err()
}

func (s *CycleStore) fileNewerThan(filename string, since time.Time) bool {
	name := strings.TrimSuffix(filename, ".jsonl")
	fileDate, err := time.Parse("20060102", name)
	if err != nil {
		return true
	}
	return fileDate.After(since) || fileDate.Equal(since)
}
