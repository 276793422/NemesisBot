# MCP Test Suite

This directory contains tests for the NemesisBot MCP (Model Context Protocol) implementation.

## 目录结构

```
test/mcp/
├── server/
│   ├── main.go           # Test MCP server implementation
│   ├── go.mod            # Go module file
│   └── Makefile          # Build automation
├── run_tests.bat         # Windows test runner script
├── run_tests.sh          # Linux/Mac test runner script
└── TEST_REPORT.md        # Generated test report (after running tests)
```

## 测试服务器

测试服务器是一个简单的 MCP 服务器实现，提供以下测试工具：

### 可用工具

1. **echo** - 回显输入文本
   - 参数: `text` (string)
   - 返回: "Echo: <text>"

2. **add** - 数值相加
   - 参数: `a`, `b` (numbers)
   - 返回: "<a> + <b> = <result>"

3. **reverse** - 字符串反转
   - 参数: `text` (string)
   - 返回: "Reverse: <text> -> <reversed>"

4. **get_time** - 获取服务器时间
   - 参数: 无
   - 返回: "Current time: <time>"

## 快速开始

### Windows

```cmd
# 进入测试目录
cd C:\AI\NemesisBot\NemesisBot_go\test\mcp

# 运行所有测试
run_tests.bat

# 查看测试报告
type TEST_REPORT.md
```

### Linux/Mac

```bash
# 进入测试目录
cd C:/AI/NemesisBot/NemesisBot_go/test/mcp

# 运行所有测试
chmod +x run_tests.sh
./run_tests.sh

# 查看测试报告
cat TEST_REPORT.md
```

## 手动测试测试服务器

### 编译服务器

```bash
cd server
go build -o mcp-test-server.exe main.go
```

### 测试 initialize

```bash
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}' | ./mcp-test-server.exe
```

### 测试 tools/list

```bash
echo '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' | ./mcp-test-server.exe
```

### 测试 tools/call (echo)

```bash
echo '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"echo","arguments":{"text":"Hello!"}}}' | ./mcp-test-server.exe
```

## 测试覆盖

### 单元测试 (pkg/mcp/*_test.go)

- **types_test.go**: 测试类型序列化/反序列化
  - `TestDecodeResult`
  - `TestToolContent`
  - `TestToolCallResult`
  - `TestInitializeResult`
  - `TestServerInfo`

- **transport/mock_test.go**: 测试传输层
  - `TestMockTransport`
  - `TestMockTransportErrors`
  - `TestMockTransportNotConnected`

### 集成测试 (pkg/mcp/integration_test.go)

- **TestClientWithRealServer**: 完整的客户端测试
  - Initialize 握手
  - 工具列表获取
  - 工具调用 (echo, add, reverse)
  - 错误处理

- **TestAdapter**: 工具适配器测试
  - 工具名称格式
  - 工具描述
  - 工具执行

## 测试报告

测试运行后会生成 `TEST_REPORT.md`，包含：

- 测试概述
- 各阶段测试结果
- 测试统计（通过/失败）
- 功能覆盖清单
- 测试结论和建议

## 故障排除

### 测试服务器无法编译

```bash
# 检查 Go 版本
go version

# 需要 Go 1.21 或更高
```

### 集成测试失败

```bash
# 确保测试服务器已编译
cd server
go build -o mcp-test-server.exe main.go

# 手动测试服务器
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}' | ./mcp-test-server.exe
```

### 路径问题

Windows 和 Linux 路径格式不同：
- Windows: `C:/path/to/file`
- Linux/Mac: `/path/to/file`

测试代码中使用相对路径 `../test/mcp/server/main.go`，需要从项目根目录运行。

## 扩展测试

要添加新的测试工具，修改 `server/main.go` 中的 `registerTools()` 函数：

```go
s.tools["my_tool"] = Tool{
    Name:        "my_tool",
    Description: "My tool description",
    InputSchema: map[string]interface{}{
        "type": "object",
        "properties": map[string]interface{}{
            "param1": map[string]interface{}{
                "type": "string",
                "description": "Parameter description",
            },
        },
        "required": []string{"param1"},
    },
}
```

然后在 `executeTool()` 函数中添加处理逻辑。
