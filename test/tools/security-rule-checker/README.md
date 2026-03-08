# Security Rule Engine Test Suite

## Overview

This test suite validates the NemesisBot security rule engine's ability to parse and enforce security rules across different platforms.

## Purpose

The security rule engine test suite ensures that:
- ✅ Rules are correctly parsed and loaded
- ✅ Pattern matching works for files, commands, and domains
- ✅ Wildcard patterns are properly expanded
- ✅ Platform-specific rules are enforced
- ✅ Dangerous operations are correctly blocked

## Location

**Main Test Tool**: `test/tools/security-rule-checker/main.go`

This follows the project's convention for independent test tools, similar to:
- `test/tools/cluster-test/main.go`
- `test/tools/examples/`

## Running the Tests

### Method 1: Using the test scripts (Recommended)

**Windows:**
```bash
test\run_security_tests.bat
```

**Linux/macOS:**
```bash
chmod +x test/run_security_tests.sh
./test/run_security_tests.sh
```

### Method 2: Direct Go execution

```bash
go run test/tools/security-rule-checker/main.go
```

### Method 3: Compile and run

```bash
cd test/tools/security-rule-checker
go build -o security-rule-checker .
./security-rule-checker  # or security-rule-checker.exe on Windows
```

## Test Categories

### 1. File Path Pattern Matching
Tests file path pattern matching including:
- Exact file paths
- Single wildcard patterns (`*.key`)
- Double wildcard patterns (`/home/**.txt`)
- Windows paths (`C:/Windows/**`)
- Cross-platform path handling

### 2. Command Pattern Matching
Tests command pattern matching including:
- Exact command matches
- Wildcard arguments (`git *`)
- Dangerous command detection (`rm -rf *`)
- Platform-specific commands (`systemctl`, `launchctl`)

### 3. Network Domain Pattern Matching
Tests network domain pattern matching including:
- Exact domain matches
- Subdomain wildcards (`*.github.com`)
- Multi-level subdomains

### 4. Platform-Specific Rules
Tests platform-specific security rules:
- **Windows**: Registry paths, Program Files, system directories
- **Linux**: System binaries, systemd, package managers
- **macOS**: System directory, launch daemons, Homebrew

## Test Output

The test suite provides detailed output including:

```
🔒 NemesisBot Security Rule Engine Test Suite
============================================================

📟 Platform: windows
🔧 Testing rule engine for platform: Windows

📋 Testing: File Path Pattern Matching
   Test 1/6: Exact file match
   Description: Should match exact file path
   ✅ PASS

   Test 2/6: Single wildcard in filename
   Description: Should match any .key file
   ✅ PASS

...

📊 Test Summary
============================================================

Total tests: 18
Passed: 18 (100.0%)
Failed: 0 (0.0%)

📋 Breakdown by category:
  file_read: 6/6 passed (100.0%)
  process_exec: 5/5 passed (100.0%)
  network_request: 6/6 passed (100.0%)
  platform_rules: 1/1 passed (100.0%)

🎉 All tests passed!
```

## Code Organization

### Why `test/tools/security-rule-checker/`?

This location follows the project's established structure:

1. **`test/`** - Test directory (not for source code tests)
2. **`test/tools/`** - Independent test tools and utilities
3. **`test/tools/security-rule-checker/`** - Security rule verification tool
4. **`test/tools/cluster-test/`** - Cluster testing tool (similar structure)

### Build Tags

The file uses `//go:build ignore` to prevent it from being included in:
- Standard `go test` runs
- Production builds
- Package dependencies

This allows it to be an independent standalone tool while coexisting with the test suite.

## Adding Custom Tests

To add custom tests, modify the test case functions in `main.go`:

```go
func getFilePatternTests(platform string) []TestCase {
    tests := []TestCase{
        // ... existing tests ...
        {
            Name:        "My custom test",
            Description: "Should test my specific pattern",
            Pattern:     "/my/pattern/**",
            TestInput:   "/my/pattern/test.txt",
            MatcherType: "file",
            Expected:    "deny",
            ShouldMatch: true,
        },
    }
    return tests
}
```

## Platform-Specific Testing

The test suite automatically detects the current platform and adds appropriate tests:

- **Windows**: Tests `C:/Windows/**`, `C:/Program Files/**`, registry patterns
- **Linux**: Tests `/usr/bin/**`, `/etc/shadow`, `systemctl *`, `apt *`
- **macOS**: Tests `/System/**`, `/Library/**`, `launchctl *`, `brew *`

## Continuous Integration

To integrate these tests into your CI/CD pipeline:

```yaml
# Example GitHub Actions workflow
- name: Run Security Rule Tests
  run: |
    go run test/tools/security-rule-checker/main.go
```

## Troubleshooting

### Tests fail to find modules
**Problem**: `cannot find package`
**Solution**: Ensure you're running from the project root directory

### Pattern matching fails unexpectedly
**Problem**: Tests fail for valid patterns
**Solution**: Check the pattern syntax in the security config files

### Platform-specific tests missing
**Problem**: Platform-specific tests don't run
**Solution**: Verify the platform detection logic in the test functions

## Technical Details

### Tested Functions

The test suite validates these core functions from `module/security/matcher.go`:

- `MatchPattern(pattern, target string) bool` - File path matching
- `MatchCommandPattern(pattern, command string) bool` - Command matching
- `MatchDomainPattern(pattern, domain string) bool` - Domain matching

### Test Architecture

```go
TestCase → runFilePatternTest() → security.MatchPattern()
TestCase → runCommandPatternTest() → security.MatchCommandPattern()
TestCase → runDomainPatternTest() → security.MatchDomainPattern()
```

## Contributing

To add new test cases:
1. Identify the rule type and category
2. Add test cases to the appropriate function
3. Include clear descriptions and expected results
4. Test on all supported platforms
5. Update this README with new test information

## License

MIT License - See main project LICENSE file for details.

## Support

For issues or questions:
1. Check the test output for specific error messages
2. Review the security configuration files
3. Examine the `module/security/` package implementation
4. Consult the main project documentation

---

**Last Updated**: 2026-03-08
**Version**: 1.0.0
**Location**: `test/tools/security-rule-checker/`
**Maintained By**: NemesisBot Project
