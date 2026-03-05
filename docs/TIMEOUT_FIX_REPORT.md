# 30分钟超时问题修复报告

**日期**: 2026-03-05
**问题**: Bot 在执行 curl 命令时卡住 30 分钟
**状态**: ✅ 已修复

## 问题分析

### 时间线
```
18:24:58 - exec 工具执行 curl 命令
18:54:36 - exec 工具终于返回（29分38秒后！）
18:55:07 - 最终响应完成
总耗时: 1814.9秒 ≈ 30.25分钟
```

### 根本原因

1. **PowerShell 进程树问题**
   - Go 的 `exec.CommandContext` 在 context 超时时杀死 PowerShell
   - 但 PowerShell 的子进程（curl.exe）可能成为孤儿进程
   - `cmd.Wait()` 等待进程完全释放资源
   - Windows 系统级超时约 30 分钟

2. **curl 网络挂起**
   - wttr.in 等网站可能无法连接
   - curl 会无限期等待
   - 没有命令级别的超时控制

## 修复方案

### 方案 1: 条件编译（默认使用 cmd.exe）

**实现文件：**
- `shell_cmd.go` - 默认实现（使用 cmd.exe）
- `shell_powershell.go` - 可选实现（使用 PowerShell）
- `shell.go` - 主逻辑（包含命令预处理）

**使用方法：**

```bash
# 默认：使用 cmd.exe（推荐）
go build -o nemesisbot.exe ./nemesisbot

# 可选：使用 PowerShell
go build -tags powershell -o nemesisbot.exe ./nemesisbot
```

### 方案 4: 自动添加 curl 超时参数

**自动优化：**
```go
// 输入命令
curl -s http://example.com

// 自动转换为
curl.exe --max-time 10 -s http://example.com
```

**特性：**
- ✅ 自动将 `curl` 转换为 `curl.exe`
- ✅ 自动添加 `--max-time 10`（如果未存在）
- ✅ 保留用户已有的超时设置（如果有）

## 测试结果

### 测试 1: 正常命令执行
```bash
命令: dir
结果: ✅ 正常工作
执行时间: < 1秒
```

### 测试 2: curl 延迟测试
```bash
命令: curl -s http://httpbin.org/delay/30
结果: ✅ 10 秒超时（正常）
执行时间: 10.045秒
之前行为: 会卡住 30 分钟
```

### 测试 3: 编译测试
```bash
✅ 默认编译（cmd.exe）: 成功
✅ PowerShell 编译: 成功
✅ 条件编译切换: 正常工作
```

## 改进效果

### 修复前
- ⛔ curl 网络挂起时卡住 30 分钟
- ⛔ PowerShell 进程树不可靠
- ⛔ 用户体验极差

### 修复后
- ✅ curl 最多等待 10 秒
- ✅ cmd.exe 更可靠
- ✅ 用户快速得到响应

## 构建系统更新

### build.bat 新增功能

```batch
# 默认：使用 cmd.exe
build.bat

# 可选：使用 PowerShell
build.bat nemesisbot.exe powershell
```

## 技术细节

### 条件编译实现

```go
// shell_cmd.go（默认）
// +build !powershell
func (t *ExecTool) buildCommand(...) {
    return exec.CommandContext(cmdCtx, "cmd", "/c", command), nil
}

// shell_powershell.go（可选）
// +build powershell
func (t *ExecTool) buildCommand(...) {
    return exec.CommandContext(cmdCtx, "powershell", "-NoProfile", "-NonInteractive", "-Command", command), nil
}
```

### 命令预处理逻辑

```go
func (t *ExecTool) preprocessWindowsCommand(command string) string {
    // 1. curl -> curl.exe
    command = replaceCurlWithCurlExe(command)

    // 2. 添加 --max-time 10（如果未存在）
    if hasCurl && !hasMaxTime {
        command = addMaxTime(command)
    }

    return command
}
```

## 相关文件

### 修改的文件
- `module/tools/shell.go` - 主逻辑，添加预处理
- `build.bat` - 添加 PowerShell 编译选项

### 新增的文件
- `module/tools/shell_cmd.go` - cmd.exe 实现（默认）
- `module/tools/shell_powershell.go` - PowerShell 实现（可选）
- `docs/WINDOWS_SHELL_CONFIG.md` - 配置说明文档

### 测试文件
- `test/test_timeout_fix.go` - 超时修复验证
- `test/test_exec_shell.go` - shell 执行测试

## 使用建议

### 推荐配置（默认）
```bash
# 使用 cmd.exe，更可靠
go build -o nemesisbot.exe ./nemesisbot
```

### 特殊需求
```bash
# 需要 PowerShell 特有命令时
go build -tags powershell -o nemesisbot.exe ./nemesisbot
```

## 后续优化建议

1. **监控超时命令**
   - 记录哪些命令经常超时
   - 分析是否需要调整超时时间

2. **智能超时**
   - 根据命令类型自动调整超时时间
   - curl 使用 10 秒，其他命令使用不同值

3. **用户配置**
   - 允许用户在配置文件中设置默认超时
   - 支持为不同命令设置不同超时

## 总结

通过使用条件编译和命令预处理，我们成功地：
- ✅ 修复了 30 分钟超时问题
- ✅ 保留了 PowerShell 选项
- ✅ 自动优化 curl 命令
- ✅ 提高了系统可靠性

**测试确认：问题已解决！**
