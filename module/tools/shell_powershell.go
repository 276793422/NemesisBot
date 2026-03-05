// +build powershell

// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package tools

import (
	"context"
	"os/exec"
)

// buildWindowsCommand builds the command for execution using PowerShell on Windows.
// This file is used when the "powershell" build tag IS set.
//
// To build with PowerShell:
//   go build -tags powershell -o nemesisbot.exe ./nemesisbot
//
// To build with cmd.exe instead (default):
//   go build -o nemesisbot.exe ./nemesisbot
func (t *ExecTool) buildWindowsCommand(cmdCtx context.Context, command string) (*exec.Cmd, error) {
	// Use PowerShell on Windows when build tag is set
	//
	// PowerShell flags:
	// -NoProfile: Don't load user profile (faster startup)
	// -NonInteractive: Don't allow interactive prompts (prevents hangs)
	// -Command: Execute the specified command
	//
	// Note: PowerShell may have issues with process cleanup in certain scenarios
	// (e.g., network timeouts causing 30-minute hangs). cmd.exe is recommended
	// for most use cases unless you specifically need PowerShell features.
	return exec.CommandContext(cmdCtx, "powershell", "-NoProfile", "-NonInteractive", "-Command", command), nil
}
