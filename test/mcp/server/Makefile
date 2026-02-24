# Makefile for MCP Test Server

.PHONY: build run test clean

# Build the test server
build:
	@echo "Building MCP test server..."
	@go build -o mcp-test-server.exe main.go
	@echo "Built successfully: mcp-test-server.exe"

# Run the test server
run:
	@echo "Starting MCP test server..."
	@go run main.go

# Test the server manually
test:
	@echo "Testing MCP server..."
	@echo "Sending initialize request..."
	@echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test-client","version":"1.0.0"}}}' | go run main.go
	@echo ""
	@echo "Sending tools/list request..."
	@echo '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' | go run main.go
	@echo ""
	@echo "Sending tools/call request..."
	@echo '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"echo","arguments":{"text":"Hello MCP!"}}}' | go run main.go

# Clean build artifacts
clean:
	@echo "Cleaning..."
	@rm -f mcp-test-server.exe
	@echo "Cleaned successfully"

# Help
help:
	@echo "Available targets:"
	@echo "  build  - Build the test server binary"
	@echo "  run    - Run the test server (interactive)"
	@echo "  test   - Run automated tests"
	@echo "  clean  - Remove build artifacts"
