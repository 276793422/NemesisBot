# 同步/异步命令执行预研报告

**日期**: 2026-03-05
**问题**: GUI 应用程序阻塞 bot 执行
**目标**: 提供更灵活的命令执行方案

---

## 一、问题回顾

### 当前问题

当 bot 执行 `notepad.exe` 等命令时，会一直等待直到程序退出，导致 bot "卡住"。

**当前代码** (`shell.go:194`):
```go
err := cmd.Run()  // 同步阻塞调用
```

**执行流程**:
```
用户: "打开记事本"
    ↓
执行: notepad.exe
    ↓
[阻塞等待记事本关闭...]
    ↓
60 秒后超时 OR 用户关闭
    ↓
Bot 继续
```

---

## 二、用户提出的方案

### 核心思路

**提供两个独立的工具/命令**：

1. **同步执行** (`exec`)
   - 用途：需要等待返回内容的命令
   - 行为：持续等待直到命令完成
   - 示例：`dir`, `type`, `grep`, `curl` 等

2. **异步执行** (`exec_async` 或 `start`)
   - 用途：启动应用程序，不需要等待返回内容
   - 行为：等待 3-5 秒确认启动成功（无闪退）后立即返回
   - 示例：`notepad`, `calc`, `explorer` 等

### 执行策略

| 场景 | 执行方式 | 等待时间 | 返回内容 |
|------|----------|----------|----------|
| 需要输出 | 同步 | 直到完成 | 完整输出 |
| 启动应用 | 异步 | 3-5 秒 | 启动状态 |
| 需要状态确认 | 异步 | 5-10 秒 | 状态信息 |

---

## 三、方案分析

### 方案对比

#### 方案 A：自动检测 GUI 应用（我之前的方案）

**实现方式**：
- 在 exec 工具内部检测 GUI 应用程序
- 自动添加 `start` 前缀

**优点**：
- ✅ 对现有代码改动较小
- ✅ 对 LLM 透明，无需关心
- ✅ 自动处理常见 GUI 应用

**缺点**：
- ❌ 需要维护 GUI 应用列表
- ❌ 可能误判（某些命令行程序也可能是 GUI）
- ❌ 无法处理未知 GUI 应用
- ❌ LLM 无法控制行为
- ❌ 平台相关（Windows start 命令）

**示例问题**：
```
用户: "用 notepad++ 打开文件"
LLM: 生成 "notepad++.exe file.txt"
系统: 不在列表中，仍然阻塞 ❌
```

---

#### 方案 B：双工具/命令方案（用户提出的方案）

**实现方式**：
- 提供 `exec` 和 `exec_async` 两个工具
- LLM 根据需要选择使用哪个

**优点**：
- ✅ 语义清晰：同步 vs 异步
- ✅ LLM 完全控制：根据场景选择
- ✅ 不需要猜测：用户意图明确
- ✅ 平台无关：概念通用
- ✅ 可扩展：可以添加更多控制选项
- ✅ 行为可预测：明确的等待时间

**缺点**：
- ⚠️ 需要新增工具（代码增加）
- ⚠️ LLM 需要理解区别（需要在 description 中说明清楚）

---

### 详细对比

| 维度 | 方案 A（自动检测） | 方案 B（双工具） |
|------|-------------------|-----------------|
| 代码复杂度 | 中等 | 较低 |
| 维护成本 | 高（需要维护列表） | 低 |
| 灵活性 | 低（自动判断） | 高（LLM 选择） |
| 可控性 | 低（黑盒） | 高（白盒） |
| 用户体验 | 好（自动） | 更好（明确） |
| 扩展性 | 低（需修改代码） | 高（工具配置） |
| 平台兼容性 | 差（Windows start） | 好（通用概念） |

---

## 四、方案 B 的技术设计

### 4.1 工具定义

#### 同步执行工具 (`exec`)

**名称**: `exec`
**描述**: 执行命令并等待完成，返回完整输出

**参数**:
```json
{
  "type": "object",
  "properties": {
    "command": {
      "type": "string",
      "description": "要执行的命令（会等待直到完成）"
    },
    "working_dir": {
      "type": "string",
      "description": "工作目录（可选）"
    }
  },
  "required": ["command"]
}
```

**行为**:
- 等待命令执行完成
- 返回完整的标准输出和错误输出
- 超时时间：60 秒（可配置）

**适用场景**:
- 查看文件内容：`cat file.txt`
- 列出目录：`ls -la`
- 搜索内容：`grep "pattern" file.txt`
- 网络请求：`curl https://api.example.com`
- 编译代码：`go build`

---

#### 异步执行工具 (`exec_async` 或 `start`)

**名称**: `exec_async` (或 `start`)
**描述**: 启动应用程序，等待确认后立即返回

**参数**:
```json
{
  "type": "object",
  "properties": {
    "command": {
      "type": "string",
      "description": "要启动的应用程序命令"
    },
    "working_dir": {
      "type": "string",
      "description": "工作目录（可选）"
    },
    "wait_seconds": {
      "type": "integer",
      "description": "等待确认启动成功的秒数（默认 3，范围 1-10）",
      "default": 3,
      "minimum": 1,
      "maximum": 10
    }
  },
  "required": ["command"]
}
```

**行为**:
1. 启动应用程序（异步）
2. 等待指定时间（默认 3 秒）
3. 检查进程是否仍在运行
4. 如果仍在运行：返回成功消息
5. 如果已退出（闪退）：返回失败消息和退出代码

**适用场景**:
- 打开编辑器：`notepad.exe`, `code.exe`
- 打开工具：`calc.exe`, `mspaint.exe`
- 打开浏览器：`chrome.exe`, `firefox.exe`
- 打开文件管理器：`explorer.exe`

---

### 4.2 实现方式

#### 方式 1：两个独立的工具

```go
// module/tools/shell.go (现有)
type ExecTool struct { ... }

// module/tools/async_shell.go (新增)
type AsyncExecTool struct { ... }
```

**优点**：
- 代码分离清晰
- 可以独立优化

**缺点**：
- 代码有重复
- 需要维护两份代码

---

#### 方式 2：一个工具，两种模式

```go
// module/tools/shell.go
type ExecTool struct {
    mode  ExecutionMode  // "sync" | "async"
}

type ExecutionMode string

const (
    ModeSync  ExecutionMode = "sync"
    ModeAsync ExecutionMode = "async"
)
```

**LLM 调用**:
```json
// 同步执行
{
  "tool": "exec",
  "arguments": {
    "command": "dir"
  }
}

// 异步执行
{
  "tool": "exec_async",
  "arguments": {
    "command": "notepad.exe",
    "wait_seconds": 3
  }
}
```

**优点**：
- 代码复用
- 维护简单

**缺点**：
- 工具定义稍复杂

---

### 4.3 平台兼容性

#### Windows

**同步**:
```go
cmd := exec.Command("powershell", "-Command", command)
cmd.Run()  // 等待完成
```

**异步**:
```go
cmd := exec.Command("powershell", "-Command", "Start-Process "+command)
cmd.Start()  // 立即返回

// 等待确认
time.Sleep(3 * time.Second)

// 检查进程
processRunning := checkProcessStillRunning(command)
```

#### Linux/macOS

**同步**:
```go
cmd := exec.Command("sh", "-c", command)
cmd.Run()  // 等待完成
```

**异步**:
```go
cmd := exec.Command("sh", "-c", command+" &")
cmd.Start()  // 后台启动

// 等待确认
time.Sleep(3 * time.Second)

// 检查进程
processRunning := checkProcessStillRunning(command)
```

---

## 五、推荐方案

### 推荐：方案 B（双工具方案）

**推荐理由**：

1. **语义清晰**
   - `exec` = 等待结果
   - `exec_async` = 启动即返回

2. **LLM 友好**
   - 工具名称明确表达意图
   - Description 说明清楚适用场景
   - LLM 可以根据用户意图选择

3. **可预测性**
   - 行为明确，不依赖自动检测
   - 等待时间可控

4. **可扩展性**
   - 未来可以添加更多执行选项
   - 可以支持更复杂的场景

### 推荐实现方式

**使用两个独立的工具**（方式 1）

**理由**：
- 代码更清晰
- 工具定义更明确
- 便于后续优化和扩展

---

## 六、实施计划

### 阶段 1：创建异步执行工具

1. 创建 `module/tools/async_shell.go`
2. 实现 `AsyncExecTool` 结构体
3. 实现进程检查逻辑
4. 注册到工具注册表

### 阶段 2：更新工具描述

1. 更新 `exec` 工具的 description
2. 说明同步等待的行为
3. 说明适用场景

### 阶段 3：测试

1. 同步执行测试（现有功能）
2. 异步执行测试（记事本、计算器等）
3. 进程检查测试
4. 闪退检测测试

### 阶段 4：文档更新

1. 更新工具使用文档
2. 提供示例
3. 说明最佳实践

---

## 七、预期效果

### 用户体验

**场景 1：查看文件**
```
用户: "查看 README.md 的内容"
LLM: 调用 exec(command="cat README.md")
Bot: [返回文件内容]
```

**场景 2：打开记事本**
```
用户: "打开记事本"
LLM: 调用 exec_async(command="notepad.exe")
Bot: [等待 3 秒]
Bot: "记事本已启动，还有其他需要吗？"
```

**场景 3：打开文件**
```
用户: "用记事本打开 config.json"
LLM: 调用 exec_async(command="notepad.exe config.json")
Bot: [等待 3 秒]
Bot: "config.json 已在记事本中打开"
```

---

## 八、风险评估

| 风险 | 影响 | 缓解措施 |
|------|------|----------|
| LLM 选择错误 | 中 | 在 description 中清楚说明适用场景 |
| 进程检查失败 | 低 | 提供降级方案（总是返回成功） |
| 平台兼容性 | 中 | 分别实现 Windows/Linux/macOS 版本 |
| 性能影响 | 低 | 异步工具很快返回 |

---

## 九、总结

### 用户方案的优势

1. **更清晰** - 同步 vs 异步语义明确
2. **更灵活** - LLM 根据场景选择
3. **更可控** - 等待时间可配置
4. **更通用** - 概念适用于所有平台

### 推荐

✅ **推荐采用方案 B（双工具方案）**

这是更优雅、更灵活、更可扩展的解决方案。

---

## 十、后续步骤

等待用户评估此预研方案，批准后进入开发计划阶段。
