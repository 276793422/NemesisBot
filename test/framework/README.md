# NemesisBot Testing Framework

This directory provides comprehensive testing utilities and infrastructure for the NemesisBot project.

## Overview

The testing framework includes:
- **Mock implementations** for all major interfaces
- **Test data builders** for creating test objects
- **Test utilities** for common testing patterns
- **Temporary workspace management** for isolated testing

## Files

### test_framework.go

Core testing utilities including:

- `TempWorkspace`: Manages temporary workspace directories for testing
- `TestContext()`: Creates test contexts with timeout
- `AssertNoError()`, `AssertError()`, `AssertEqual()`: Custom assertions
- `Eventually()`: Retries conditions until success or timeout
- `WaitFor()`: Waits for channels with timeout

**Example:**
```go
func TestSomething(t *testing.T) {
    ws := framework.NewTempWorkspace(t)
    defer ws.Cleanup()

    // Use workspace.Path() for testing
    config := &config.Config{
        Agents: config.AgentsConfig{
            Defaults: config.AgentDefaults{
                Workspace: ws.Path(),
            },
        },
    }

    // Test implementation...
}
```

### mocks.go

Mock implementations for all major interfaces:

#### MockLLMProvider

Configurable mock LLM provider for testing Agent components.

```go
provider := framework.NewMockLLMProvider()
provider.SetResponses([]string{"Response 1", "Response 2"})
provider.SetDelay(100 * time.Millisecond)

// Later...
callCount := provider.GetCallCount()
calls := provider.GetCalls()
```

#### MockMessageBus

In-memory message bus for testing message flows.

```go
msgBus := framework.NewMockMessageBus()

// Publish test message
msg := framework.NewMessageBuilder().
    WithChannel("test").
    WithContent("Hello").
    BuildInbound()

msgBus.PublishInbound(ctx, msg)

// Verify messages
inbound := msgBus.GetInboundMessages()
```

#### MockChannel

Mock channel implementation for testing channel management.

```go
channel := framework.NewMockChannel("test-channel")
channel.SetAllowed(map[string]bool{"user123": true})

// Start and test
if err := channel.Start(); err != nil {
    t.Fatal(err)
}

// Verify state
if !channel.IsRunning() {
    t.Error("Channel should be running")
}
```

#### MockSecurityAuditor

Mock security auditor for testing permission flows.

```go
auditor := framework.NewMockSecurityAuditor()
auditor.SetAllowed(false)
auditor.SetPolicy("file_write", "/tmp/test.txt", true)

allowed, err := auditor.RequestPermission(ctx, "file_write", "/tmp/test.txt", nil)
```

### builders.go

Fluent API builders for creating test data:

#### MessageBuilder

```go
msg := framework.NewMessageBuilder().
    WithChannel("rpc").
    WithSenderID("user123").
    WithChatID("chat456").
    WithContent("Hello, Bot!").
    WithSessionKey("session:789").
    BuildInbound()
```

#### OutboundMessageBuilder

```go
msg := framework.NewOutboundMessageBuilder().
    WithChannel("discord").
    WithChatID("channel-id").
    WithContent("Response").
    WithMetadata("source", "test").
    Build()
```

#### PayloadBuilder

```go
payload := framework.NewPayloadBuilder().
    WithMessage("Test message").
    WithAction("test_action").
    WithSenderID("user123").
    WithTimestamp(time.Now().Unix()).
    Build()
```

#### MockConfigBuilder

```go
config := framework.NewMockConfigBuilder().
    WithWorkspace(ws.Path()).
    WithLLM("mock/model").
    WithMaxTokens(4000).
    WithMaxToolIterations(10).
    WithConcurrentRequestMode("queue").
    WithQueueSize(16).
    WithRestrictToWorkspace(true).
    WithAgent("agent1", "Agent 1").
    Build()
```

## Test Patterns

### Table-Driven Tests

```go
func TestFunction(t *testing.T) {
    tests := []struct {
        name    string
        input   string
        want    string
        wantErr bool
    }{
        {"basic", "input", "output", false},
        {"error", "", "", true},
    }

    for _, tt := range tests {
        t.Run(tt.name, func(t *testing.T) {
            got, err := Function(tt.input)
            if (err != nil) != tt.wantErr {
                t.Errorf("Function() error = %v, wantErr %v", err, tt.wantErr)
                return
            }
            if got != tt.want {
                t.Errorf("Function() = %v, want %v", got, tt.want)
            }
        })
    }
}
```

### Concurrent Testing

```go
func TestConcurrent(t *testing.T) {
    var wg sync.WaitGroup
    numGoroutines := 100

    for i := 0; i < numGoroutines; i++ {
        wg.Add(1)
        go func(id int) {
            defer wg.Done()
            // Concurrent operation
        }(i)
    }

    wg.Wait()
}
```

### Temporary File Testing

```go
func TestFileOperations(t *testing.T) {
    ws := framework.NewTempWorkspace(t)

    // Create test file
    err := ws.CreateFile("test.txt", "content")
    if err != nil {
        t.Fatal(err)
    }

    // Test file operations...
}
```

### Eventually Pattern

```go
func TestAsyncOperation(t *testing.T) {
    startAsyncOperation()

    framework.Eventually(t, func() bool {
        return checkCondition()
    }, 5*time.Second, "condition not met")
}
```

## Running Tests

### Run All Tests

```bash
go test ./module/...
```

### Run Specific Module Tests

```bash
go test ./module/agent/...
go test ./module/channels/...
```

### Run with Coverage

```bash
go test -coverprofile=coverage.out ./module/...
go tool cover -html=coverage.out
```

### Run with Race Detector

```bash
go test -race ./module/...
```

### Run Verbose Tests

```bash
go test -v ./module/...
```

## Coverage Reports

Use the coverage report script to generate comprehensive coverage reports:

```bash
./test/scripts/coverage_report.sh
```

This generates:
- Console output with module-by-module coverage
- Combined coverage file at `test/coverage/coverage.out`
- HTML report at `test/coverage/coverage.html`

## Conventions

### Test File Organization

- Unit tests: `test/unit/<module>/...`
- Integration tests: `test/integration/...`
- Performance tests: `test/performance/...`

### Test Naming

- Test functions: `Test<FunctionName>` or `Test<FunctionName>_<Scenario>`
- Example: `TestLoop_ProcessMessage_Success`

### Setup/Teardown

Use `t.Cleanup()` for cleanup:
```go
func TestSomething(t *testing.T) {
    file := setupFile(t)
    defer t.Cleanup(func() {
        os.Remove(file)
    })

    // Test...
}
```

### Parallel Tests

Mark independent tests as parallel:
```go
func TestParallelOperation(t *testing.T) {
    t.Parallel()
    // Test...
}
```

## Best Practices

1. **Use table-driven tests** for multiple scenarios
2. **Test both success and error paths**
3. **Use mocks** to isolate dependencies
4. **Test concurrent access** with race detector
5. **Use temporary directories** via `t.TempDir()` or `TempWorkspace`
6. **Keep tests fast** - avoid sleep, use channels/context
7. **Make tests readable** - clear names, good structure
8. **Test edge cases** - empty inputs, nil values, boundaries

## Troubleshooting

### Tests Timing Out

- Use shorter timeouts in test contexts
- Avoid long delays in mocks
- Check for deadlocks

### Race Detector Failures

- Add mutexes around shared state
- Use atomic operations for counters
- Check for data races in mocks

### Flaky Tests

- Avoid time-based assertions
- Use Eventually pattern for async operations
- Ensure proper cleanup between tests

## Contributing

When adding new tests:
1. Use the framework utilities and builders
2. Follow existing patterns and conventions
3. Ensure >95% coverage for new code
4. Run tests with race detector before committing
5. Update this documentation for new patterns
