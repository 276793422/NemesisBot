# MCP (Model Context Protocol) Tools

你有一些通过 MCP (Model Context Protocol) 提供的额外工具。这些工具来自外部服务器，扩展了你的能力。

## MCP 工具命名规则

所有 MCP 工具都以前缀 `mcp_` 开头，格式为：

```
mcp_<server-name>_<tool-name>
```

例如：
- `mcp_test-mcp-server_echo` - 回显文本
- `mcp_filesystem_read_file` - 读取文件
- `mcp_github_create_issue` - 创建 GitHub Issue

## MCP 工具说明

MCP 工具描述中包含 `[MCP:server-name]` 前缀，这表示该工具来自哪个 MCP 服务器。

例如：
- `[MCP:test-mcp-server] Echoes back the input text` - 来自 test-mcp-server 的 echo 工具
- `[MCP:filesystem] Read file contents` - 来自 filesystem 服务器的文件读取工具

## 如何使用 MCP 工具

MCP 工具就像普通工具一样使用：
1. 在工具列表中找到合适的 MCP 工具
2. 查看工具的参数说明
3. 调用工具并传递所需的参数

## 重要提示

- **自动注册**: MCP 工具在 Agent 启动时自动注册
- **动态可用**: 可用的 MCP 工具取决于配置的 MCP 服务器
- **工具前缀**: 所有 MCP 工具都有 `mcp_` 前缀，便于识别
- **服务器来源**: 工具描述中的 `[MCP:...]` 前缀显示工具来自哪个服务器

## 当前可用的 MCP 服务器

运行 `nemesisbot mcp list` 查看当前配置的 MCP 服务器。

## 示例

如果有一个 `mcp_filesystem_read_file` 工具：

```json
{
  "path": "/path/to/file.txt"
}
```

调用这个工具就像调用任何其他工具一样。
