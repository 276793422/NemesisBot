# Shell 执行实现重构说明

**日期**: 2026-03-05
**重构目的**: 改善代码结构，减少重复，提高可维护性

## 重构前后对比

### ❌ 重构前的问题

```go
// shell_cmd.go
func (t *ExecTool) buildCommand(cmdCtx context.Context, command string) (*exec.Cmd, error) {
	if runtime.GOOS == "windows" {
		return exec.CommandContext(cmdCtx, "cmd", "/c", command), nil
	}
	return exec.CommandContext(cmdCtx, "sh", "-c", command), nil  // ← Unix 逻辑重复
}

// shell_powershell.go
func (t *ExecTool) buildCommand(cmdCtx context.Context, command string) (*exec.Cmd, error) {
	if runtime.GOOS == "windows" {
		return exec.CommandContext(cmdCtx, "powershell", ...), nil
	}
	return exec.CommandContext(cmdCtx, "sh", "-c", command), nil  // ← Unix 逻辑重复
}
```

**问题：**
1. ❌ Unix 逻辑在两个文件中重复
2. ❌ 维护 Unix 逻辑需要修改两个文件
3. ❌ 职责不清晰，违反单一职责原则

### ✅ 重构后的结构

```go
// shell.go（主文件）
func (t *ExecTool) Execute(ctx context.Context, args map[string]interface{}) *ToolResult {
	// ... 参数检查和预处理 ...

	var cmd *exec.Cmd
	var err error
	if runtime.GOOS == "windows" {
		// Windows: 使用平台特定实现
		cmd, err = t.buildWindowsCommand(cmdCtx, command)
		if err != nil {
			return ErrorResult(...)
		}
	} else {
		// Unix: 直接实现（只在这里维护一次）
		cmd = exec.CommandContext(cmdCtx, "sh", "-c", command)
	}

	// ... 执行命令 ...
}

// shell_cmd.go（只包含 Windows + cmd）
func (t *ExecTool) buildWindowsCommand(cmdCtx context.Context, command string) (*exec.Cmd, error) {
	return exec.CommandContext(cmdCtx, "cmd", "/c", command), nil
}

// shell_powershell.go（只包含 Windows + PowerShell）
func (t *ExecTool) buildWindowsCommand(cmdCtx context.Context, command string) (*exec.Cmd, error) {
	return exec.CommandContext(cmdCtx, "powershell", "-NoProfile", "-NonInteractive", "-Command", command), nil
}
```

## 代码结构

### 文件职责

| 文件 | 职责 | 平台特定逻辑 |
|------|------|-------------|
| `shell.go` | 主逻辑、参数验证、命令预处理、Unix 实现 | ✅ Unix (`sh -c`) |
| `shell_cmd.go` | Windows + cmd.exe 实现 | ✅ Windows (cmd) |
| `shell_powershell.go` | Windows + PowerShell 实现 | ✅ Windows (PowerShell) |

### 调用流程

```
Execute() [shell.go]
  │
  ├─ 参数验证
  ├─ 命令预处理 (preprocessWindowsCommand)
  │
  ├─ 判断 OS
  │   │
  │   ├─ Windows ──→ buildWindowsCommand() [shell_cmd.go 或 shell_powershell.go]
  │   │                   (通过条件编译选择)
  │   │
  │   └─ Unix ──────────→ exec.CommandContext("sh", "-c") [shell.go]
  │                       (直接实现，无重复)
  │
  └─ 执行命令 (cmd.Run())
```

## 优势

### 1. 减少代码重复
- ✅ Unix 逻辑只在 `shell.go` 中维护一次
- ✅ 避免了两个文件中的重复代码

### 2. 职责清晰
- ✅ `shell.go`: 主逻辑 + Unix 实现
- ✅ `shell_cmd.go`: 只关注 cmd.exe
- ✅ `shell_powershell.go`: 只关注 PowerShell

### 3. 更容易维护
- ✅ 修改 Unix 逻辑：只需改 `shell.go`
- ✅ 修改 Windows 逻辑：只需改对应的文件
- ✅ 添加新的 Windows shell 方式：新建文件，添加 `buildWindowsCommand` 实现

### 4. 符合设计原则
- ✅ **单一职责原则**: 每个文件只负责一种实现
- ✅ **开闭原则**: 对扩展开放（新增 shell 方式），对修改关闭（不改变现有代码）
- ✅ **DRY 原则**: Don't Repeat Yourself

## 条件编译机制

### Build Tags

```go
// shell_cmd.go
// +build !powershell
// 当没有 powershell 标签时使用此文件

// shell_powershell.go
// +build powershell
// 当有 powershell 标签时使用此文件
```

### 编译命令

```bash
# 默认：使用 cmd.exe
go build -o nemesisbot.exe ./nemesisbot

# 使用 PowerShell
go build -tags powershell -o nemesisbot.exe ./nemesisbot
```

## 函数签名

### buildWindowsCommand

```go
// buildWindowsCommand 创建 Windows 平台的命令
// 此方法在不同的文件中有不同的实现：
// - shell_cmd.go: 使用 cmd.exe
// - shell_powershell.go: 使用 PowerShell
//
// 参数：
//   - cmdCtx: 命令上下文（包含超时控制）
//   - command: 要执行的命令字符串
//
// 返回：
//   - *exec.Cmd: 配置好的命令对象
//   - error: 错误信息（如果有）
func (t *ExecTool) buildWindowsCommand(cmdCtx context.Context, command string) (*exec.Cmd, error)
```

## 测试验证

### 编译测试
```bash
✅ go build -o nemesisbot.exe ./nemesisbot
✅ go build -tags powershell -o nemesisbot.exe ./nemesisbot
```

### 功能测试
```bash
✅ dir 命令正常执行
✅ curl 命令自动添加 --max-time 10
✅ 超时问题修复（10秒内返回）
```

## 总结

通过这次重构：
1. **消除了代码重复**：Unix 逻辑不再重复
2. **提高了可维护性**：修改逻辑只需改一个地方
3. **职责更清晰**：每个文件专注于一种实现
4. **保持了功能**：所有原有功能正常工作

**特别感谢用户提出的宝贵建议！** 🙏
