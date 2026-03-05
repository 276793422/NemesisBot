//go:build ignore

// +build ignore

// NemesisBot - Cluster Commands Test
// This file tests all cluster command line parameters
// Usage: go run test_cluster_params.go

package main

import (
	"flag"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"
)

// TestResult holds the result of a single test
type TestResult struct {
	Command  string
	Args     []string
	Success  bool
	Output   string
	Error    error
	Duration time.Duration
}

// Color codes for terminal output
const (
	Reset  = "\033[0m"
	Red    = "\033[31m"
	Green  = "\033[32m"
	Yellow = "\033[33m"
	Blue   = "\033[34m"
)

func main() {
	// Parse flags
	flag.Parse()

	testDir := "./test_cluster_params"
	if len(os.Args) > 1 {
		testDir = os.Args[1]
	}

	fmt.Printf("🧪 Cluster Commands Parameters Test\n")
	fmt.Printf("=====================================\n\n")

	// Setup test environment
	fmt.Printf("📁 Setting up test environment...\n")
	testWorkspace := filepath.Join(testDir, "workspace")
	if err := os.MkdirAll(testWorkspace, 0755); err != nil {
		fmt.Printf("❌ Failed to create test directory: %v\n", err)
		os.Exit(1)
	}

	// Create necessary subdirectories
	os.MkdirAll(filepath.Join(testWorkspace, "cluster"), 0755)
	os.MkdirAll(filepath.Join(testWorkspace, "config"), 0755)
	os.MkdirAll(filepath.Join(testWorkspace, "logs", "cluster"), 0755)

	// Create minimal config.json
	configContent := `{
  "agents": {
    "defaults": {
      "workspace": "` + testWorkspace + `"
    }
  }
}`
	configPath := filepath.Join(testDir, "config.json")
	if err := os.WriteFile(configPath, []byte(configContent), 0644); err != nil {
		fmt.Printf("❌ Failed to create config.json: %v\n", err)
		os.Exit(1)
	}

	// Set NEMESISBOT_HOME environment variable
	os.Setenv("NEMESISBOT_HOME", testDir)

	fmt.Printf("✅ Test environment ready: %s\n\n", testDir)

	// Run tests
	results := []TestResult{}

	// Test 1: cluster init with no parameters
	results = append(results, runTest("cluster init (default)", []string{"cluster", "init"}))

	// Test 2: cluster init with --name only
	results = append(results, runTest("cluster init --name", []string{"cluster", "init", "--name", "TestBot"}))

	// Test 3: cluster init with --name and --role
	results = append(results, runTest("cluster init --name --role", []string{"cluster", "init", "--name", "CTO", "--role", "manager"}))

	// Test 4: cluster init with all parameters
	results = append(results, runTest("cluster init (all params)", []string{"cluster", "init",
		"--name", "FullBot",
		"--role", "coordinator",
		"--category", "development",
		"--tags", "production,senior",
		"--address", "192.168.1.100",
	}))

	// Test 5: cluster init with short flags
	results = append(results, runTest("cluster init (short flags)", []string{"cluster", "init",
		"-n", "ShortBot",
		"-r", "worker",
		"-c", "testing",
	}))

	// Test 6: cluster status
	results = append(results, runTest("cluster status", []string{"cluster", "status"}))

	// Test 7: cluster info
	results = append(results, runTest("cluster info", []string{"cluster", "info"}))

	// Test 8: cluster info --name
	results = append(results, runTest("cluster info --name", []string{"cluster", "info", "--name", "UpdatedBot"}))

	// Test 9: cluster info --role
	results = append(results, runTest("cluster info --role", []string{"cluster", "info", "--role", "manager"}))

	// Test 10: cluster config
	results = append(results, runTest("cluster config", []string{"cluster", "config"}))

	// Test 11: cluster config --udp-port
	results = append(results, runTest("cluster config --udp-port", []string{"cluster", "config", "--udp-port", "11950"}))

	// Test 12: cluster config --rpc-port
	results = append(results, runTest("cluster config --rpc-port", []string{"cluster", "config", "--rpc-port", "21950"}))

	// Test 13: cluster config both ports
	results = append(results, runTest("cluster config (both ports)", []string{"cluster", "config", "--udp-port", "11951", "--rpc-port", "21951"}))

	// Test 14: cluster enable
	results = append(results, runTest("cluster enable", []string{"cluster", "enable"}))

	// Test 15: cluster disable
	results = append(results, runTest("cluster disable", []string{"cluster", "disable"}))

	// Test 16: cluster start (alias)
	results = append(results, runTest("cluster start", []string{"cluster", "start"}))

	// Test 17: cluster stop (alias)
	results = append(results, runTest("cluster stop", []string{"cluster", "stop"}))

	// Test 18: cluster reset
	results = append(results, runTest("cluster reset", []string{"cluster", "reset"}))

	// Test 19: cluster reset --hard
	results = append(results, runTest("cluster reset --hard", []string{"cluster", "reset", "--hard"}))

	// Test 20: cluster peers
	results = append(results, runTest("cluster peers", []string{"cluster", "peers"}))

	// Test 21: cluster init --help
	results = append(results, runTest("cluster init --help", []string{"cluster", "init", "--help"}))

	// Test 22: cluster (no subcommand - should show help)
	results = append(results, runTest("cluster (help)", []string{"cluster"}))

	// Test 23: cluster invalid (should show error)
	results = append(results, runTest("cluster invalid", []string{"cluster", "invalid_command"}))

	// Print results summary
	printSummary(results)

	// Cleanup
	fmt.Printf("\n🧹 Cleaning up test environment...\n")
	if err := os.RemoveAll(testDir); err != nil {
		fmt.Printf("⚠️  Warning: Failed to cleanup: %v\n", err)
	} else {
		fmt.Printf("✅ Cleanup complete\n")
	}

	// Exit with appropriate code
	failCount := 0
	for _, r := range results {
		if !r.Success {
			failCount++
		}
	}
	if failCount > 0 {
		os.Exit(1)
	}
}

func runTest(name string, args []string) TestResult {
	fmt.Printf("Testing: %s\n", name)
	start := time.Now()

	// Build command
	cmd := exec.Command("./nemesisbot.exe", args...)
	cmd.Env = append(os.Environ(), "NEMESISBOT_HOME=./test_cluster_params")

	// Run command
	output, err := cmd.CombinedOutput()
	duration := time.Since(start)

	result := TestResult{
		Command:  "./nemesisbot.exe " + strings.Join(args, " "),
		Args:     args,
		Success:  err == nil,
		Output:   string(output),
		Error:    err,
		Duration: duration,
	}

	// Print result
	if result.Success {
		fmt.Printf("  ✅ PASS (%.2fs)\n", duration.Seconds())
	} else {
		fmt.Printf("  ❌ FAIL (%.2fs)\n", duration.Seconds())
		if len(output) > 0 {
			// Show first 200 chars of error output
			outputPreview := string(output)
			if len(outputPreview) > 200 {
				outputPreview = outputPreview[:200] + "..."
			}
			fmt.Printf("     Error: %s\n", outputPreview)
		}
	}

	return result
}

func printSummary(results []TestResult) {
	fmt.Printf("\n")
	fmt.Printf("=====================================\n")
	fmt.Printf("📊 Test Summary\n")
	fmt.Printf("=====================================\n\n")

	total := len(results)
	passed := 0
	failed := 0

	for _, r := range results {
		if r.Success {
			passed++
		} else {
			failed++
		}
	}

	fmt.Printf("Total Tests: %d\n", total)
	fmt.Printf("%s✅ Passed: %d%s\n", Green, passed, Reset)
	fmt.Printf("%s❌ Failed: %d%s\n", Red, failed, Reset)
	fmt.Printf("\n")

	// Show failed tests
	if failed > 0 {
		fmt.Printf("%sFailed Tests:%s\n", Red, Reset)
		for _, r := range results {
			if !r.Success {
				fmt.Printf("  - %s\n", r.Command)
				if len(r.Output) > 0 {
					fmt.Printf("    Output: %s\n", truncate(r.Output, 100))
				}
			}
		}
		fmt.Printf("\n")
	}

	// Calculate success rate
	successRate := float64(passed) / float64(total) * 100
	fmt.Printf("Success Rate: %.1f%%\n", successRate)

	if successRate >= 90 {
		fmt.Printf("\n%s✅ Excellent! Most tests passed.%s\n", Green, Reset)
	} else if successRate >= 70 {
		fmt.Printf("\n%s⚠️  Warning: Some tests failed.%s\n", Yellow, Reset)
	} else {
		fmt.Printf("\n%s❌ Critical: Many tests failed.%s\n", Red, Reset)
	}
}

func truncate(s string, maxLen int) string {
	if len(s) <= maxLen {
		return s
	}
	return s[:maxLen] + "..."
}
