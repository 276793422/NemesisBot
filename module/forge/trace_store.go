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

// TraceStore persists conversation traces as JSONL files.
// Directory structure: traces/202604/20260421.jsonl
type TraceStore struct {
	baseDir string
	config  *ForgeConfig
}

// NewTraceStore creates a new TraceStore rooted at forgeDir/traces.
func NewTraceStore(forgeDir string, config *ForgeConfig) *TraceStore {
	return &TraceStore{
		baseDir: filepath.Join(forgeDir, "traces"),
		config:  config,
	}
}

// Append writes a conversation trace to today's JSONL file.
func (s *TraceStore) Append(trace *ConversationTrace) error {
	monthDir := filepath.Join(s.baseDir, trace.StartTime.Format("200601"))
	if err := os.MkdirAll(monthDir, 0755); err != nil {
		return err
	}

	filePath := filepath.Join(monthDir, trace.StartTime.Format("20060102")+".jsonl")

	f, err := os.OpenFile(filePath, os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0644)
	if err != nil {
		return err
	}
	defer f.Close()

	data, err := json.Marshal(trace)
	if err != nil {
		return err
	}

	_, err = fmt.Fprintf(f, "%s\n", data)
	return err
}

// ReadTraces reads all traces since the given time.
func (s *TraceStore) ReadTraces(since time.Time) ([]*ConversationTrace, error) {
	var results []*ConversationTrace

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

// Cleanup removes trace files older than maxAgeDays.
func (s *TraceStore) Cleanup(maxAgeDays int) error {
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

func (s *TraceStore) readJSONL(path string) ([]*ConversationTrace, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	defer f.Close()

	var records []*ConversationTrace
	scanner := bufio.NewScanner(f)
	scanner.Buffer(make([]byte, 1024*1024), 1024*1024)

	for scanner.Scan() {
		line := strings.TrimSpace(scanner.Text())
		if line == "" {
			continue
		}
		var trace ConversationTrace
		if err := json.Unmarshal([]byte(line), &trace); err != nil {
			continue
		}
		records = append(records, &trace)
	}

	return records, scanner.Err()
}

func (s *TraceStore) fileNewerThan(filename string, since time.Time) bool {
	name := strings.TrimSuffix(filename, ".jsonl")
	fileDate, err := time.Parse("20060102", name)
	if err != nil {
		return true
	}
	return fileDate.After(since) || fileDate.Equal(since)
}
