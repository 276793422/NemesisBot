//go:build !cross_compile

package integration

import (
	"encoding/json"
	"fmt"
	"os"
	"testing"

	"github.com/276793422/NemesisBot/module/desktop/process"
)

// TestMain intercepts --multiple to support child-process tests.
//
// When the test binary is spawned as a child process (via os.Executable()),
// the Go test harness would reject unknown flags like --multiple with
// "flag provided but not defined". This TestMain detects child mode
// and runs the child handshake protocol instead of running tests.
func TestMain(m *testing.M) {
	if process.HasChildModeFlag() {
		os.Exit(runChildMode())
	}
	os.Exit(m.Run())
}

// runChildMode runs the child side of the pipe handshake protocol.
// This mirrors what nemesisbot.exe does in child mode, but without
// the full Wails window setup (which requires a real GUI environment).
func runChildMode() int {
	stdin := process.NewReadCloser(os.Stdin)
	stdout := process.NewWriteCloser(os.Stdout)

	// Perform handshake
	result, err := process.ChildHandshake(stdin, stdout)
	if err != nil {
		fmt.Fprintf(os.Stderr, "[Child] Handshake failed: %v\n", err)
		return 1
	}
	if !result.Success {
		fmt.Fprintf(os.Stderr, "[Child] Handshake unsuccessful\n")
		return 1
	}

	fmt.Fprintf(os.Stderr, "[Child] Handshake completed, waiting for WS key...\n")

	// Receive WS key (required by protocol, but we just ACK it)
	_, _, _, err = process.ReceiveWSKey(stdin, stdout)
	if err != nil {
		fmt.Fprintf(os.Stderr, "[Child] ReceiveWSKey failed: %v\n", err)
		return 1
	}

	fmt.Fprintf(os.Stderr, "[Child] WS key received, waiting for window data...\n")

	// Receive window data
	_, err = process.ReceiveWindowData(stdin, stdout)
	if err != nil {
		fmt.Fprintf(os.Stderr, "[Child] ReceiveWindowData failed: %v\n", err)
		return 1
	}

	fmt.Fprintf(os.Stderr, "[Child] All data received, staying alive\n")

	// Stay alive until stdin is closed (parent terminates us)
	var dummy struct{}
	_ = json.NewDecoder(os.Stdin).Decode(&dummy)

	return 0
}
