# 异步执行工具开发计划

**日期**: 2026-03-05
**功能**: 添加异步命令执行工具 `exec_async`
**方案**: 使用两个独立的工具（`exec` 和 `exec_async`）

---

## 开发计划

### 阶段 1: 创建异步执行工具

#### 步骤 1.1: 创建文件

**文件**: `module/tools/async_shell.go`

**内容结构**:
```go
package tools

import (
    "context"
    "fmt"
    "os/exec"
    "regexp"
    "runtime"
    "strings"
    "time"
)

type AsyncExecTool struct {
    workingDir          string
    defaultWaitTime     time.Duration
    denyPatterns        []*regexp.Regexp
    allowPatterns       []*regexp.Regexp
    restrictToWorkspace bool
}
```

#### 步骤 1.2: 实现核心方法

**方法**:
1. `Name()` - 返回工具名称 "exec_async"
2. `Description()` - 描述工具功能和使用场景
3. `Parameters()` - 定义工具参数
4. `Execute(ctx, args)` - 执行异步命令
5. `checkProcessRunning()` - 检查进程是否仍在运行

#### 步骤 1.3: 实现平台特定的进程检查

**Windows**:
```go
// 使用 tasklist 或 PowerShell 检查进程
func checkProcessWindows(processName string) (bool, error)
```

**Linux/macOS**:
```go
// 使用 ps 或 pgrep 检查进程
func checkProcessUnix(processName string) (bool, error)
```

#### 步骤 1.4: 注册工具

在 `module/tools/registry.go` 中注册新工具。

---

### 阶段 2: 更新工具描述

#### 步骤 2.1: 更新 `exec` 工具描述

**文件**: `module/tools/shell.go`

**更新 `Description()` 方法**:
```go
func (t *ExecTool) Description() string {
    return `执行命令并等待完成，返回完整输出。

适用场景：
- 需要返回输出的命令（如：cat, ls, grep, curl）
- 编译和构建命令
- 任何需要等待结果完成的命令

注意事项：
- 此工具会等待命令执行完成（最多 60 秒）
- 对于 GUI 应用程序，请使用 exec_async 工具
- 执行时间较长的命令可能会超时`
}
```

#### 步骤 2.2: 实现 `exec_async` 工具

**实现完整的 `AsyncExecTool`**:
- 实现所有必需的方法
- 实现 Windows 进程检查
- 实现 Unix 进程检查
- 添加进程闪退检测

---

### 阶段 3: 测试

#### 步骤 3.1: 编译验证

```bash
go build -o nemesisbot.exe ./nemesisbot/
```

#### 步骤 3.2: 功能测试

**测试用例**:
1. **同步执行** (`exec`)
   ```
   命令: dir
   预期: 返回目录列表
   ```

2. **异步执行 - 记事本** (`exec_async`)
   ```
   命令: notepad.exe
   预期: 3 秒后返回成功消息
   ```

3. **异步执行 - 计算器** (`exec_async`)
   ```
   命令: calc.exe
   预期: 3 秒后返回成功消息
   ```

4. **异步执行 - 带参数** (`exec_async`)
   ```
   命令: notepad.exe README.md
   预期: 3 秒后返回成功消息
   ```

5. **进程检查验证**
   ```
   命令: 程序启动后检查是否仍在运行
   预期: 正确检测进程状态
   ```

---

### 阶段 4: 文档更新

#### 步骤 4.1: 创建使用文档

**文件**: `docs/ASYNC_EXEC_USAGE.md`

**内容**:
- 工具对比表格
- 使用示例
- 最佳实践
- 常见问题

---

## 开发步骤

### Step 1: 创建 async_shell.go

创建 `module/tools/async_shell.go` 文件，实现完整的异步执行工具。

### Step 2: 更新 shell.go 描述

更新 `exec` 工具的 `Description()` 方法，清楚说明适用场景。

### Step 3: 注册新工具

在工具注册表中注册 `AsyncExecTool`。

### Step 4: 编译验证

编译项目，确保无错误。

### Step 5: 功能测试

测试同步和异步执行功能。

### Step 6: 更新文档

创建使用文档和示例。

---

## 验收标准

- [ ] `async_shell.go` 文件创建完成
- [ ] `AsyncExecTool` 实现完整
- [ ] Windows 进程检查功能正常
- [ ] Unix 进程检查功能正常
- [ ] `exec` 工具描述已更新
- [ ] 工具已注册到注册表
- [ ] 编译成功无错误
- [ ] 异步执行功能测试通过
- [ ] 文档创建完成

---

## 开始执行

现在开始按照计划执行开发...
