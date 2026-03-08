// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package tools

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

// validatePath ensures the given path is within the workspace if restrict is true.
func validatePath(path, workspace string, restrict bool) (string, error) {
	if workspace == "" {
		return path, nil
	}

	absWorkspace, err := filepath.Abs(workspace)
	if err != nil {
		return "", fmt.Errorf("failed to resolve workspace path: %w", err)
	}

	var absPath string
	if filepath.IsAbs(path) {
		absPath = filepath.Clean(path)
	} else {
		absPath, err = filepath.Abs(filepath.Join(absWorkspace, path))
		if err != nil {
			return "", fmt.Errorf("failed to resolve file path: %w", err)
		}
	}

	if restrict {
		if !isWithinWorkspace(absPath, absWorkspace) {
			return "", fmt.Errorf("access denied: path is outside the workspace")
		}

		workspaceReal := absWorkspace
		if resolved, err := filepath.EvalSymlinks(absWorkspace); err == nil {
			workspaceReal = resolved
		}

		if resolved, err := filepath.EvalSymlinks(absPath); err == nil {
			if !isWithinWorkspace(resolved, workspaceReal) {
				return "", fmt.Errorf("access denied: symlink resolves outside workspace")
			}
		} else if os.IsNotExist(err) {
			if parentResolved, err := resolveExistingAncestor(filepath.Dir(absPath)); err == nil {
				if !isWithinWorkspace(parentResolved, workspaceReal) {
					return "", fmt.Errorf("access denied: symlink resolves outside workspace")
				}
			} else if !os.IsNotExist(err) {
				return "", fmt.Errorf("failed to resolve path: %w", err)
			}
		} else {
			return "", fmt.Errorf("failed to resolve path: %w", err)
		}
	}

	return absPath, nil
}

func resolveExistingAncestor(path string) (string, error) {
	for current := filepath.Clean(path); ; current = filepath.Dir(current) {
		if resolved, err := filepath.EvalSymlinks(current); err == nil {
			return resolved, nil
		} else if !os.IsNotExist(err) {
			return "", err
		}
		if filepath.Dir(current) == current {
			return "", os.ErrNotExist
		}
	}
}

func isWithinWorkspace(candidate, workspace string) bool {
	rel, err := filepath.Rel(filepath.Clean(workspace), filepath.Clean(candidate))
	return err == nil && rel != ".." && !strings.HasPrefix(rel, ".."+string(os.PathSeparator))
}

type ReadFileTool struct {
	workspace string
	restrict  bool
}

// NewReadFileTool creates a new file reading tool.
// It restricts file access to the specified workspace for security.
//
// Parameters:
//   - workspace: The base directory for file operations. If empty, no restrictions apply.
//   - restrict: If true, only files within the workspace can be accessed.
//
// Returns:
//
//	A configured ReadFileTool ready for use.
//
// Security:
//   - When restrict is true, symbolic links are resolved to prevent escape attacks
//   - Absolute paths outside workspace are denied
//   - Relative paths are resolved against the workspace
func NewReadFileTool(workspace string, restrict bool) *ReadFileTool {
	return &ReadFileTool{workspace: workspace, restrict: restrict}
}

func (t *ReadFileTool) Name() string {
	return "read_file"
}

func (t *ReadFileTool) Description() string {
	return "Read the contents of a file"
}

func (t *ReadFileTool) Parameters() map[string]interface{} {
	return map[string]interface{}{
		"type": "object",
		"properties": map[string]interface{}{
			"path": map[string]interface{}{
				"type":        "string",
				"description": "Path to the file to read",
			},
		},
		"required": []string{"path"},
	}
}

func (t *ReadFileTool) Execute(ctx context.Context, args map[string]interface{}) *ToolResult {
	path, ok := args["path"].(string)
	if !ok {
		return ErrorResult("path is required")
	}

	resolvedPath, err := validatePath(path, t.workspace, t.restrict)
	if err != nil {
		return ErrorResult(err.Error())
	}

	content, err := os.ReadFile(resolvedPath)
	if err != nil {
		return ErrorResult(fmt.Sprintf("failed to read file: %v", err))
	}

	return NewToolResult(string(content))
}

type WriteFileTool struct {
	workspace string
	restrict  bool
}

// NewWriteFileTool creates a new file writing tool.
// It restricts file access to the specified workspace for security.
//
// Parameters:
//   - workspace: The base directory for file operations. If empty, no restrictions apply.
//   - restrict: If true, only files within the workspace can be written.
//
// Returns:
//
//	A configured WriteFileTool ready for use.
//
// Security:
//   - When restrict is true, symbolic links are resolved to prevent escape attacks
//   - Creates parent directories if they don't exist
//   - Files are written with permissions 0644
func NewWriteFileTool(workspace string, restrict bool) *WriteFileTool {
	return &WriteFileTool{workspace: workspace, restrict: restrict}
}

func (t *WriteFileTool) Name() string {
	return "write_file"
}

func (t *WriteFileTool) Description() string {
	return "Write content to a file"
}

func (t *WriteFileTool) Parameters() map[string]interface{} {
	return map[string]interface{}{
		"type": "object",
		"properties": map[string]interface{}{
			"path": map[string]interface{}{
				"type":        "string",
				"description": "Path to the file to write",
			},
			"content": map[string]interface{}{
				"type":        "string",
				"description": "Content to write to the file",
			},
		},
		"required": []string{"path", "content"},
	}
}

func (t *WriteFileTool) Execute(ctx context.Context, args map[string]interface{}) *ToolResult {
	path, ok := args["path"].(string)
	if !ok {
		return ErrorResult("path is required")
	}

	content, ok := args["content"].(string)
	if !ok {
		return ErrorResult("content is required")
	}

	resolvedPath, err := validatePath(path, t.workspace, t.restrict)
	if err != nil {
		return ErrorResult(err.Error())
	}

	dir := filepath.Dir(resolvedPath)
	if err := os.MkdirAll(dir, 0755); err != nil {
		return ErrorResult(fmt.Sprintf("failed to create directory: %v", err))
	}

	if err := os.WriteFile(resolvedPath, []byte(content), 0644); err != nil {
		return ErrorResult(fmt.Sprintf("failed to write file: %v", err))
	}

	return SilentResult(fmt.Sprintf("File written: %s", path))
}

type ListDirTool struct {
	workspace string
	restrict  bool
}

// NewListDirTool creates a new directory listing tool.
// It restricts directory access to the specified workspace for security.
//
// Parameters:
//   - workspace: The base directory for file operations. If empty, no restrictions apply.
//   - restrict: If true, only directories within the workspace can be listed.
//
// Returns:
//
//	A configured ListDirTool ready for use.
//
// Security:
//   - When restrict is true, symbolic links are resolved to prevent escape attacks
//   - Lists both files and directories with metadata
func NewListDirTool(workspace string, restrict bool) *ListDirTool {
	return &ListDirTool{workspace: workspace, restrict: restrict}
}

func (t *ListDirTool) Name() string {
	return "list_dir"
}

func (t *ListDirTool) Description() string {
	return "List files and directories in a path"
}

func (t *ListDirTool) Parameters() map[string]interface{} {
	return map[string]interface{}{
		"type": "object",
		"properties": map[string]interface{}{
			"path": map[string]interface{}{
				"type":        "string",
				"description": "Path to list",
			},
		},
		"required": []string{"path"},
	}
}

func (t *ListDirTool) Execute(ctx context.Context, args map[string]interface{}) *ToolResult {
	path, ok := args["path"].(string)
	if !ok {
		path = "."
	}

	resolvedPath, err := validatePath(path, t.workspace, t.restrict)
	if err != nil {
		return ErrorResult(err.Error())
	}

	entries, err := os.ReadDir(resolvedPath)
	if err != nil {
		return ErrorResult(fmt.Sprintf("failed to read directory: %v", err))
	}

	result := ""
	for _, entry := range entries {
		if entry.IsDir() {
			result += "DIR:  " + entry.Name() + "\n"
		} else {
			result += "FILE: " + entry.Name() + "\n"
		}
	}

	return NewToolResult(result)
}

// DeleteFileTool deletes a file
type DeleteFileTool struct {
	workspace string
	restrict  bool
}

// NewDeleteFileTool creates a new file deletion tool
func NewDeleteFileTool(workspace string, restrict bool) *DeleteFileTool {
	return &DeleteFileTool{workspace: workspace, restrict: restrict}
}

func (t *DeleteFileTool) Name() string {
	return "delete_file"
}

func (t *DeleteFileTool) Description() string {
	return "Delete a file"
}

func (t *DeleteFileTool) Parameters() map[string]interface{} {
	return map[string]interface{}{
		"type": "object",
		"properties": map[string]interface{}{
			"path": map[string]interface{}{
				"type":        "string",
				"description": "Path to the file to delete",
			},
		},
		"required": []string{"path"},
	}
}

func (t *DeleteFileTool) Execute(ctx context.Context, args map[string]interface{}) *ToolResult {
	path, ok := args["path"].(string)
	if !ok {
		return ErrorResult("path is required")
	}

	resolvedPath, err := validatePath(path, t.workspace, t.restrict)
	if err != nil {
		return ErrorResult(err.Error())
	}

	if err := os.Remove(resolvedPath); err != nil {
		return ErrorResult(fmt.Sprintf("failed to delete file: %v", err))
	}

	return SilentResult(fmt.Sprintf("File deleted: %s", path))
}

// CreateDirTool creates a directory
type CreateDirTool struct {
	workspace string
	restrict  bool
}

// NewCreateDirTool creates a new directory creation tool
func NewCreateDirTool(workspace string, restrict bool) *CreateDirTool {
	return &CreateDirTool{workspace: workspace, restrict: restrict}
}

func (t *CreateDirTool) Name() string {
	return "create_dir"
}

func (t *CreateDirTool) Description() string {
	return "Create a directory"
}

func (t *CreateDirTool) Parameters() map[string]interface{} {
	return map[string]interface{}{
		"type": "object",
		"properties": map[string]interface{}{
			"path": map[string]interface{}{
				"type":        "string",
				"description": "Path to the directory to create",
			},
		},
		"required": []string{"path"},
	}
}

func (t *CreateDirTool) Execute(ctx context.Context, args map[string]interface{}) *ToolResult {
	path, ok := args["path"].(string)
	if !ok {
		return ErrorResult("path is required")
	}

	resolvedPath, err := validatePath(path, t.workspace, t.restrict)
	if err != nil {
		return ErrorResult(err.Error())
	}

	if err := os.MkdirAll(resolvedPath, 0755); err != nil {
		return ErrorResult(fmt.Sprintf("failed to create directory: %v", err))
	}

	return SilentResult(fmt.Sprintf("Directory created: %s", path))
}

// DeleteDirTool deletes a directory
type DeleteDirTool struct {
	workspace string
	restrict  bool
}

// NewDeleteDirTool creates a new directory deletion tool
func NewDeleteDirTool(workspace string, restrict bool) *DeleteDirTool {
	return &DeleteDirTool{workspace: workspace, restrict: restrict}
}

func (t *DeleteDirTool) Name() string {
	return "delete_dir"
}

func (t *DeleteDirTool) Description() string {
	return "Delete a directory and all its contents"
}

func (t *DeleteDirTool) Parameters() map[string]interface{} {
	return map[string]interface{}{
		"type": "object",
		"properties": map[string]interface{}{
			"path": map[string]interface{}{
				"type":        "string",
				"description": "Path to the directory to delete",
			},
		},
		"required": []string{"path"},
	}
}

func (t *DeleteDirTool) Execute(ctx context.Context, args map[string]interface{}) *ToolResult {
	path, ok := args["path"].(string)
	if !ok {
		return ErrorResult("path is required")
	}

	resolvedPath, err := validatePath(path, t.workspace, t.restrict)
	if err != nil {
		return ErrorResult(err.Error())
	}

	if err := os.RemoveAll(resolvedPath); err != nil {
		return ErrorResult(fmt.Sprintf("failed to delete directory: %v", err))
	}

	return SilentResult(fmt.Sprintf("Directory deleted: %s", path))
}
