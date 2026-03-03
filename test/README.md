# NemesisBot Tests

This directory contains all tests for the NemesisBot project.

## Quick Start

```bash
# Run all tests
go test -v ./...

# Run tests for specific module
go test -v ./test/cluster/...

# Run integration tests
cd test/cluster
go run direct_rpc.go
go run integration_stress.go
```

## Directory Structure

```
test/
├── cluster/              # Cluster module tests
│   ├── transport/        # Transport layer tests
│   │   ├── frame_test.go
│   │   ├── conn_test.go
│   │   └── pool_test.go
│   ├── rpc/              # RPC layer tests
│   │   └── server_test.go
│   ├── integration_stress.go   # Integration & stress tests
│   ├── direct_rpc.go           # Direct RPC test
│   ├── integration.go          # Cluster integration test
│   └── README.md               # Cluster tests documentation
├── mcp/                  # MCP server tests
├── tools/                # Test tools
│   └── cluster-test/     # Cluster testing utility
└── unit/                 # Unit tests (if any)
```

## Test Categories

### 1. Unit Tests
Located in module-specific subdirectories, these test individual components:
- Transport layer: frame encoding, TCP connections, connection pooling
- RPC layer: server implementation, request handling

### 2. Integration Tests
Test multiple components working together:
- **direct_rpc.go**: End-to-end TCP RPC communication
- **integration_stress.go**: Comprehensive stress testing suite
- **integration.go**: Full cluster discovery and communication

### 3. Test Tools
Utilities for testing and debugging:
- **cluster-test**: Command-line tool for cluster testing

## Running Tests

### All Tests
```bash
go test -v ./...
```

### Cluster Module Tests
```bash
# Transport tests
go test -v ./test/cluster/transport

# RPC tests
go test -v ./test/cluster/rpc

# All cluster tests
go test -v ./test/cluster/...
```

### Integration Tests
```bash
cd test/cluster

# Direct RPC test
go run direct_rpc.go

# Stress test suite
go run integration_stress.go

# Full cluster test
go run integration.go
```

## Test Coverage

Current test coverage for the cluster module:
- ✅ Transport layer: 95%+
- ✅ RPC layer: 85%+
- ✅ Integration: 100% (6/6 tests pass)

For detailed test results, see [cluster/TEST_REPORT.md](cluster/TEST_REPORT.md)

## Test Organization

### Black-Box Testing
All unit tests use black-box testing methodology:
- Tests are in separate packages with `_test` suffix
- Only public APIs are tested
- No direct access to internal state
- Tests verify behavior, not implementation

### File Naming
- `*_test.go`: Standard Go test files (run with `go test`)
- `*.go`: Standalone test programs (run with `go run`)

## Contributing Tests

When adding new tests:
1. Place unit tests in `test/<module>/<component>/`
2. Use black-box testing (package `<module>_test`)
3. Follow existing test patterns
4. Add documentation for complex tests
5. Run all tests before committing

## Known Issues

### Single-Machine Testing
Cluster integration tests on a single machine have limitations:
- UDP discovery port conflicts
- Cannot fully simulate multi-machine environment
- For full testing, deploy on multiple machines

## See Also

- [cluster/README.md](cluster/README.md) - Cluster tests documentation
- [cluster/TEST_REPORT.md](cluster/TEST_REPORT.md) - Complete test report
- [../module/cluster](../module/cluster) - Cluster module source code
