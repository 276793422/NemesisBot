# Cluster Tests

This directory contains all tests for the cluster module, organized by component.

## Directory Structure

```
test/cluster/
├── transport/           # Transport layer tests
│   ├── frame_test.go         # Frame encoding/decoding tests
│   ├── conn_test.go          # TCP connection tests
│   └── pool_test.go          # Connection pool tests
├── rpc/                  # RPC layer tests
│   └── server_test.go        # RPC server tests
├── integration_stress.go  # Integration and stress tests
├── direct_rpc.go          # Direct RPC communication test
└── TEST_REPORT.md         # Complete test report
```

## Running Tests

### Run All Cluster Tests
```bash
# From project root
go test -v ./test/cluster/...

# Or run specific package tests
go test -v ./test/cluster/transport
go test -v ./test/cluster/rpc
```

### Run Integration Tests
```bash
cd test/cluster
go run direct_rpc.go
go run integration_stress.go
```

## Test Coverage

### Transport Layer (transport/)
- **frame_test.go**: Frame encoding/decoding, frame reader/writer
- **conn_test.go**: TCP connection lifecycle, send/receive, concurrent access
- **pool_test.go**: Connection pool management, limits, statistics

### RPC Layer (rpc/)
- **server_test.go**: RPC server, handlers, concurrent connections

### Integration Tests
- **direct_rpc.go**: End-to-end TCP RPC communication
- **integration_stress.go**: 6 comprehensive stress tests
  - Basic RPC communication
  - Concurrent RPC calls (10 simultaneous)
  - Sequential RPC calls (50 calls)
  - Large payload transfer (1MB)
  - Timeout handling
  - Connection pool multiple connections

## Test Results

All tests pass:
- ✅ 32 transport tests (100%)
- ✅ 6 RPC server tests (100%)
- ✅ 6 integration tests (100%)

## Black-Box Testing

All tests in this directory use black-box testing methodology:
- Package names use `_test` suffix (e.g., `package transport_test`)
- Tests access only exported APIs
- No direct access to internal state
- Tests public behavior and interfaces

For more details, see [TEST_REPORT.md](TEST_REPORT.md)
