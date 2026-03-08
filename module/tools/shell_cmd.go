//go:build !powershell
// +build !powershell

// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package tools

import (
	"context"
	"os/exec"
)

// buildWindowsCommand builds the command for execution using cmd.exe on Windows.
// This file is used when the "powershell" build tag is NOT set (default).
//
// To build with cmd.exe (default):
//
//	go build -o nemesisbot.exe ./nemesisbot
//
// To build with PowerShell instead:
//
//	go build -tags powershell -o nemesisbot.exe ./nemesisbot
func (t *ExecTool) buildWindowsCommand(cmdCtx context.Context, command string) (*exec.Cmd, error) {
	// Use cmd.exe on Windows (more reliable than PowerShell for simple commands)
	// Using /c flag to execute the command and terminate
	//
	// Advantages of cmd.exe:
	// - Faster startup
	// - Lower memory footprint
	// - More reliable process cleanup (no 30-minute hang issue)
	// - Better compatibility with simple commands
	return exec.CommandContext(cmdCtx, "cmd", "/c", command), nil
}
