// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Security Rule Engine Checker
//
// This is an independent test tool to verify the security rule engine's ability
// to parse and enforce rules. It validates that rules are correctly matched
// against operations and that dangerous operations are properly blocked.
//
// Usage:
//   go run test/tools/security-rule-checker/main.go

package main

import (
	"fmt"
	"os"
	"runtime"

	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/security"
)

// TestCase represents a single rule test case
type TestCase struct {
	Name        string
	Description string
	Pattern     string
	TestInput   string
	Action      string
	MatcherType string // "file", "command", "domain"
	Expected    string // "allow", "deny"
	ShouldMatch bool
}

// TestResult represents the result of a test case
type TestResult struct {
	TestCase TestCase
	Passed   bool
	Actual   bool
	Error    error
}

func main() {
	fmt.Println("🔒 NemesisBot Security Rule Engine Test Suite")
	fmt.Println("=================================================")
	fmt.Println()

	// Detect platform
	platform := runtime.GOOS
	fmt.Printf("📟 Platform: %s\n", platform)
	fmt.Printf("🔧 Testing rule engine for platform: %s\n\n", config.GetPlatformDisplayName())

	// Run all tests
	results := runAllTests(platform)

	// Print summary
	printSummary(results)

	// Exit with appropriate code
	if !allTestsPassed(results) {
		os.Exit(1)
	}
}

func runAllTests(platform string) []TestResult {
	var allResults []TestResult

	// File pattern tests
	fmt.Println("📋 Testing: File Path Pattern Matching")
	fileTests := getFilePatternTests(platform)
	for _, test := range fileTests {
		result := runFilePatternTest(test)
		allResults = append(allResults, result)
		printTestResult(result)
	}

	// Command pattern tests
	fmt.Println("\n📋 Testing: Command Pattern Matching")
	commandTests := getCommandPatternTests(platform)
	for _, test := range commandTests {
		result := runCommandPatternTest(test)
		allResults = append(allResults, result)
		printTestResult(result)
	}

	// Domain pattern tests
	fmt.Println("\n📋 Testing: Network Domain Pattern Matching")
	domainTests := getDomainPatternTests()
	for _, test := range domainTests {
		result := runDomainPatternTest(test)
		allResults = append(allResults, result)
		printTestResult(result)
	}

	return allResults
}

func getFilePatternTests(platform string) []TestCase {
	tests := []TestCase{
		{
			Name:        "Exact file match",
			Description: "Should match exact file path",
			Pattern:     "/etc/passwd",
			TestInput:   "/etc/passwd",
			MatcherType: "file",
			Expected:    "deny",
			ShouldMatch: true,
		},
		{
			Name:        "Single wildcard in filename",
			Description: "Should match any .key file",
			Pattern:     "*.key",
			TestInput:   "/home/user/test.key",
			MatcherType: "file",
			Expected:    "deny",
			ShouldMatch: true,
		},
		{
			Name:        "Double wildcard for directories",
			Description: "Should match files in any subdirectory",
			Pattern:     "/home/**.txt",
			TestInput:   "/home/user/documents/test.txt",
			MatcherType: "file",
			Expected:    "deny",
			ShouldMatch: true,
		},
		{
			Name:        "Windows path with forward slashes",
			Description: "Should match Windows system directories",
			Pattern:     "C:/Windows/**",
			TestInput:   "C:/Windows/System32/drivers/etc/hosts",
			MatcherType: "file",
			Expected:    "deny",
			ShouldMatch: true,
		},
		{
			Name:        "Windows path with backslashes",
			Description: "Should handle Windows backslash paths",
			Pattern:     "C:/Program Files/**",
			TestInput:   `C:\Program Files\MyApp\config.ini`,
			MatcherType: "file",
			Expected:    "deny",
			ShouldMatch: true,
		},
		{
			Name:        "Non-matching pattern",
			Description: "Should not match unrelated paths",
			Pattern:     "/etc/**",
			TestInput:   "/home/user/file.txt",
			MatcherType: "file",
			Expected:    "allow",
			ShouldMatch: false,
		},
	}

	// Add platform-specific cases
	if platform == "windows" {
		tests = append(tests, TestCase{
			Name:        "Windows Program Files x86",
			Description: "Should match Windows x86 program files",
			Pattern:     "C:/Program Files (x86)/**",
			TestInput:   "C:/Program Files (x86)/MyApp/app.exe",
			MatcherType: "file",
			Expected:    "deny",
			ShouldMatch: true,
		})
	} else if platform == "linux" {
		tests = append(tests, []TestCase{
			{
				Name:        "Linux system binaries",
				Description: "Should protect Linux system binaries",
				Pattern:     "/usr/bin/**",
				TestInput:   "/usr/bin/apt-get",
				MatcherType: "file",
				Expected:    "deny",
				ShouldMatch: true,
			},
			{
				Name:        "Linux shadow file",
				Description: "Should protect password files",
				Pattern:     "/etc/shadow",
				TestInput:   "/etc/shadow",
				MatcherType: "file",
				Expected:    "deny",
				ShouldMatch: true,
			},
		}...)
	} else if platform == "darwin" {
		tests = append(tests, []TestCase{
			{
				Name:        "macOS System directory",
				Description: "Should protect macOS System directory",
				Pattern:     "/System/**",
				TestInput:   "/System/Library/Extensions/my.kext",
				MatcherType: "file",
				Expected:    "deny",
				ShouldMatch: true,
			},
			{
				Name:        "macOS Library directory",
				Description: "Should protect macOS Library directory",
				Pattern:     "/Library/**",
				TestInput:   "/Library/Application Support/MyApp/config.plist",
				MatcherType: "file",
				Expected:    "deny",
				ShouldMatch: true,
			},
		}...)
	}

	return tests
}

func getCommandPatternTests(platform string) []TestCase {
	tests := []TestCase{
		{
			Name:        "Simple command match",
			Description: "Should match exact command",
			Pattern:     "git status",
			TestInput:   "git status",
			MatcherType: "command",
			Expected:    "allow",
			ShouldMatch: true,
		},
		{
			Name:        "Command with wildcard arguments",
			Description: "Should match command with any arguments",
			Pattern:     "git *",
			TestInput:   "git commit -m 'test message'",
			MatcherType: "command",
			Expected:    "allow",
			ShouldMatch: true,
		},
		{
			Name:        "Dangerous command pattern",
			Description: "Should block dangerous rm command",
			Pattern:     "rm -rf *",
			TestInput:   "rm -rf /tmp/test",
			MatcherType: "command",
			Expected:    "deny",
			ShouldMatch: true,
		},
		{
			Name:        "Wildcard in middle of command",
			Description: "Should match command with wildcard in middle",
			Pattern:     "*sudo*",
			TestInput:   "sudo apt-get install python",
			MatcherType: "command",
			Expected:    "deny",
			ShouldMatch: true,
		},
		{
			Name:        "Non-matching command",
			Description: "Should not match unrelated command",
			Pattern:     "rm *",
			TestInput:   "ls -la",
			MatcherType: "command",
			Expected:    "allow",
			ShouldMatch: false,
		},
	}

	// Add platform-specific command cases
	if platform == "linux" {
		tests = append(tests, []TestCase{
			{
				Name:        "Linux systemctl command",
				Description: "Should require approval for systemctl",
				Pattern:     "systemctl *",
				TestInput:   "systemctl restart nginx",
				MatcherType: "command",
				Expected:    "ask",
				ShouldMatch: true,
			},
			{
				Name:        "Linux package manager",
				Description: "Should require approval for apt",
				Pattern:     "apt *",
				TestInput:   "apt install python3",
				MatcherType: "command",
				Expected:    "ask",
				ShouldMatch: true,
			},
		}...)
	} else if platform == "darwin" {
		tests = append(tests, []TestCase{
			{
				Name:        "macOS launchctl command",
				Description: "Should require approval for launchctl",
				Pattern:     "launchctl *",
				TestInput:   "launchctl load com.example.service",
				MatcherType: "command",
				Expected:    "ask",
				ShouldMatch: true,
			},
			{
				Name:        "macOS Homebrew command",
				Description: "Should require approval for brew",
				Pattern:     "brew *",
				TestInput:   "brew install python",
				MatcherType: "command",
				Expected:    "ask",
				ShouldMatch: true,
			},
		}...)
	}

	return tests
}

func getDomainPatternTests() []TestCase {
	return []TestCase{
		{
			Name:        "Exact domain match",
			Description: "Should match exact domain",
			Pattern:     "github.com",
			TestInput:   "github.com",
			MatcherType: "domain",
			Expected:    "allow",
			ShouldMatch: true,
		},
		{
			Name:        "Subdomain wildcard",
			Description: "Should match any subdomain",
			Pattern:     "*.github.com",
			TestInput:   "api.github.com",
			MatcherType: "domain",
			Expected:    "allow",
			ShouldMatch: true,
		},
		{
			Name:        "Subdomain wildcard multiple",
			Description: "Should match multiple different subdomains",
			Pattern:     "*.github.com",
			TestInput:   "gist.github.com",
			MatcherType: "domain",
			Expected:    "allow",
			ShouldMatch: true,
		},
		{
			Name:        "OpenAI domain wildcard",
			Description: "Should match OpenAI subdomains",
			Pattern:     "*.openai.com",
			TestInput:   "api.openai.com",
			MatcherType: "domain",
			Expected:    "allow",
			ShouldMatch: true,
		},
		{
			Name:        "Anthropic domain wildcard",
			Description: "Should match Anthropic subdomains",
			Pattern:     "*.anthropic.com",
			TestInput:   "api.anthropic.com",
			MatcherType: "domain",
			Expected:    "allow",
			ShouldMatch: true,
		},
		{
			Name:        "Non-matching domain",
			Description: "Should not match unrelated domain",
			Pattern:     "*.github.com",
			TestInput:   "example.com",
			MatcherType: "domain",
			Expected:    "deny",
			ShouldMatch: false,
		},
	}
}

func runFilePatternTest(test TestCase) TestResult {
	matched := security.MatchPattern(test.Pattern, test.TestInput)
	return TestResult{
		TestCase: test,
		Passed:   matched == test.ShouldMatch,
		Actual:   matched,
	}
}

func runCommandPatternTest(test TestCase) TestResult {
	matched := security.MatchCommandPattern(test.Pattern, test.TestInput)
	return TestResult{
		TestCase: test,
		Passed:   matched == test.ShouldMatch,
		Actual:   matched,
	}
}

func runDomainPatternTest(test TestCase) TestResult {
	matched := security.MatchDomainPattern(test.Pattern, test.TestInput)
	return TestResult{
		TestCase: test,
		Passed:   matched == test.ShouldMatch,
		Actual:   matched,
	}
}

func printTestResult(result TestResult) {
	if result.Passed {
		fmt.Printf("   ✅ PASS: %s\n", result.TestCase.Name)
		fmt.Printf("      %s\n", result.TestCase.Description)
	} else {
		fmt.Printf("   ❌ FAIL: %s\n", result.TestCase.Name)
		fmt.Printf("      %s\n", result.TestCase.Description)
		fmt.Printf("      Pattern: %s\n", result.TestCase.Pattern)
		fmt.Printf("      Input: %s\n", result.TestCase.TestInput)
		fmt.Printf("      Expected match: %v, Got: %v\n", result.TestCase.ShouldMatch, result.Actual)
	}
}

func printSummary(results []TestResult) {
	fmt.Println("\n📊 Test Summary")
	fmt.Println("=================================================")

	total := len(results)
	passed := 0
	failed := 0

	for _, result := range results {
		if result.Passed {
			passed++
		} else {
			failed++
		}
	}

	fmt.Printf("\nTotal tests: %d\n", total)
	fmt.Printf("Passed: %d (%.1f%%)\n", passed, float64(passed)*100/float64(total))
	fmt.Printf("Failed: %d (%.1f%%)\n", failed, float64(failed)*100/float64(total))

	// Print failed tests details
	if failed > 0 {
		fmt.Println("\n❌ Failed tests:")
		for _, result := range results {
			if !result.Passed {
				fmt.Printf("  - %s: %s\n", result.TestCase.Name, result.TestCase.Description)
				fmt.Printf("    Pattern: %s\n", result.TestCase.Pattern)
				fmt.Printf("    Input: %s\n", result.TestCase.TestInput)
				fmt.Printf("    Expected match: %v, Got: %v\n", result.TestCase.ShouldMatch, result.Actual)
			}
		}
	}

	fmt.Println()

	// Print final status
	if failed == 0 {
		fmt.Println("🎉 All tests passed!")
	} else {
		fmt.Println("⚠️  Some tests failed. Please review the failures above.")
	}
}

func allTestsPassed(results []TestResult) bool {
	for _, result := range results {
		if !result.Passed {
			return false
		}
	}
	return true
}
