package workflow

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"
)

// SaveExecution persists an execution to a JSON file.
// File path: {dir}/{workflowName}/{id}.json
func SaveExecution(dir string, exec *Execution) error {
	if dir == "" || exec == nil {
		return nil
	}

	subDir := filepath.Join(dir, exec.WorkflowName)
	if err := os.MkdirAll(subDir, 0755); err != nil {
		return fmt.Errorf("create execution directory: %w", err)
	}

	data, err := json.MarshalIndent(exec, "", "  ")
	if err != nil {
		return fmt.Errorf("marshal execution: %w", err)
	}

	path := filepath.Join(subDir, exec.ID+".json")
	if err := os.WriteFile(path, data, 0644); err != nil {
		return fmt.Errorf("write execution file: %w", err)
	}

	return nil
}

// LoadExecution loads a specific execution from disk.
// Requires both the workflow name and execution ID.
func LoadExecution(dir string, workflowName string, id string) (*Execution, error) {
	path := filepath.Join(dir, workflowName, id+".json")
	return loadExecutionFile(path)
}

// LoadExecutionByID searches all workflow directories for an execution with
// the given ID. This is slower than LoadExecution but does not require
// knowing the workflow name.
func LoadExecutionByID(dir string, id string) (*Execution, error) {
	entries, err := os.ReadDir(dir)
	if err != nil {
		return nil, fmt.Errorf("read directory %s: %w", dir, err)
	}

	for _, entry := range entries {
		if !entry.IsDir() {
			continue
		}

		path := filepath.Join(dir, entry.Name(), id+".json")
		if _, err := os.Stat(path); err == nil {
			return loadExecutionFile(path)
		}
	}

	return nil, fmt.Errorf("execution %q not found in %s", id, dir)
}

// ListExecutions loads all executions for a workflow from disk.
// If workflowName is empty, loads executions across all workflows.
func ListExecutionsFromDisk(dir string, workflowName string) ([]*Execution, error) {
	if dir == "" {
		return nil, nil
	}

	if workflowName != "" {
		return loadExecutionsFromDir(filepath.Join(dir, workflowName))
	}

	// Load from all workflow subdirectories
	entries, err := os.ReadDir(dir)
	if err != nil {
		if os.IsNotExist(err) {
			return nil, nil
		}
		return nil, fmt.Errorf("read directory %s: %w", dir, err)
	}

	var all []*Execution
	for _, entry := range entries {
		if !entry.IsDir() {
			continue
		}
		execs, err := loadExecutionsFromDir(filepath.Join(dir, entry.Name()))
		if err != nil {
			continue // skip unreadable directories
		}
		all = append(all, execs...)
	}

	return all, nil
}

// DeleteExecution removes an execution file from disk.
func DeleteExecution(dir string, workflowName string, id string) error {
	path := filepath.Join(dir, workflowName, id+".json")
	return os.Remove(path)
}

// CleanupOldExecutions removes execution files older than the given maxAge in seconds.
// This is a maintenance function for keeping the persistence directory tidy.
func CleanupOldExecutions(dir string, maxAge int64) (int, error) {
	if dir == "" {
		return 0, nil
	}

	entries, err := os.ReadDir(dir)
	if err != nil {
		if os.IsNotExist(err) {
			return 0, nil
		}
		return 0, err
	}

	now := time.Now().Unix()
	deleted := 0

	for _, entry := range entries {
		if !entry.IsDir() {
			continue
		}

		subDir := filepath.Join(dir, entry.Name())
		files, err := os.ReadDir(subDir)
		if err != nil {
			continue
		}

		for _, f := range files {
			if !strings.HasSuffix(f.Name(), ".json") {
				continue
			}

			info, err := f.Info()
			if err != nil {
				continue
			}

			if maxAge > 0 {
				fileAge := now - info.ModTime().Unix()
				if fileAge > maxAge {
					path := filepath.Join(subDir, f.Name())
					if os.Remove(path) == nil {
						deleted++
					}
				}
			}
		}
	}

	return deleted, nil
}

// --- Internal helpers ---

func loadExecutionFile(path string) (*Execution, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, fmt.Errorf("read execution file %s: %w", path, err)
	}

	var exec Execution
	if err := json.Unmarshal(data, &exec); err != nil {
		return nil, fmt.Errorf("unmarshal execution %s: %w", path, err)
	}

	return &exec, nil
}

func loadExecutionsFromDir(dir string) ([]*Execution, error) {
	entries, err := os.ReadDir(dir)
	if err != nil {
		if os.IsNotExist(err) {
			return nil, nil
		}
		return nil, err
	}

	var executions []*Execution
	for _, entry := range entries {
		if !strings.HasSuffix(entry.Name(), ".json") {
			continue
		}

		path := filepath.Join(dir, entry.Name())
		exec, err := loadExecutionFile(path)
		if err != nil {
			continue // skip corrupt files
		}
		executions = append(executions, exec)
	}

	return executions, nil
}
