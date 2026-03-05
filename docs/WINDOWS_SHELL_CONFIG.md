# Windows Shell 执行配置

## 概述

NemesisBot 的 exec 工具支持在 Windows 上使用不同的 shell 执行命令。默认使用 **cmd.exe**，更可靠且避免 30 分钟的超时问题。

## 可用选项

### 1. cmd.exe（默认）

**优点：**
- ✅ 更可靠，不会出现进程树卡住的问题
- ✅ 启动速度快
- ✅ 内存占用小
- ✅ 避免了 PowerShell 的 30 分钟系统超时

**使用方法：**
```bash
# 默认编译，自动使用 cmd.exe
go build -o nemesisbot.exe ./nemesisbot
```

### 2. PowerShell（可选）

**优点：**
- ✅ 支持更复杂的 PowerShell 命令
- ✅ 更好的错误处理

**缺点：**
- ❌ 可能在网络超时时卡住 30 分钟
- ❌ 启动速度较慢
- ❌ 内存占用较大

**使用方法：**
```bash
# 添加 powershell 编译标签
go build -tags powershell -o nemesisbot.exe ./nemesisbot
```

## 自动优化

无论使用哪个 shell，exec 工具都会自动优化 curl 命令：

### 1. curl 别名修复
```bash
# 输入命令
curl http://example.com

# 自动转换为（避免 PowerShell 别名问题）
curl.exe http://example.com
```

### 2. 自动添加超时
```bash
# 输入命令
curl.exe http://example.com

# 自动添加 10 秒超时（防止网络挂起）
curl.exe --max-time 10 http://example.com
```

**注意：** 如果命令已经包含 `--max-time` 或 `-m` 参数，则不会重复添加。

## 测试

### 测试 cmd.exe 版本（默认）
```bash
cd test
go run test_exec_shell.go
```

### 测试 PowerShell 版本
```bash
cd test
go run -tags powershell test_exec_shell.go
```

## 编译示例

### 使用 cmd.exe（推荐）
```bash
cd nemesisbot
go build -o nemesisbot.exe .
```

### 使用 PowerShell
```bash
cd nemesisbot
go build -tags powershell -o nemesisbot.exe .
```

## 问题排查

### Q: 为什么默认改用 cmd.exe？

A: 因为 PowerShell 在某些情况下（特别是网络请求超时）会导致进程树卡住约 30 分钟才能恢复。cmd.exe 更可靠，不会有这个问题。

### Q: 如果我需要使用 PowerShell 特有的命令怎么办？

A: 使用编译标签 `-tags powershell` 重新编译即可。

### Q: 如何查看当前使用的是哪个 shell？

A: 检查编译的二进制文件名或查看编译日志：
- 使用 cmd.exe: 默认行为
- 使用 PowerShell: 编译时需要 `-tags powershell`

## 技术细节

### 条件编译实现

使用 Go 的 build tags 实现条件编译：

- `shell_cmd.go`: 默认文件（使用 cmd.exe）
- `shell_powershell.go`: 需要标签（使用 PowerShell）

### 构建标记

```go
// shell_cmd.go
// +build !powershell

// shell_powershell.go
// +build powershell
```

这确保了：
- 没有标签时，使用 `shell_cmd.go`
- 有 `powershell` 标签时，使用 `shell_powershell.go`

## 更新日志

- **2026-03-05**: 添加条件编译支持，默认使用 cmd.exe 避免超时问题
- **2026-03-05**: 自动为 curl 命令添加 `--max-time 10` 参数
