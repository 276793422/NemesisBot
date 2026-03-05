# 同步/异步命令执行工具使用指南

**日期**: 2026-03-05
**功能**: 提供两种命令执行方式，满足不同使用场景

---

## 工具对比

| 特性 | exec (同步执行) | exec_async (异步执行) |
|------|-----------------|---------------------|
| **用途** | 需要返回输出的命令 | 启动应用程序 |
| **等待行为** | 等待命令完成 | 等待 3-5 秒确认启动 |
| **返回内容** | 完整输出 | 启动状态 |
| **超时时间** | 60 秒 | 可配置 (1-10 秒) |
| **适用场景** | 文件查看、搜索、编译 | 打开 GUI 应用 |

---

## exec - 同步执行工具

### 用途

执行命令并等待完成，返回完整的输出内容。

### 适用场景

✅ **适合使用 exec 的场景**：
- 查看文件内容：`cat file.txt`, `type file.txt`
- 列出目录：`ls`, `dir`
- 搜索内容：`grep "pattern" file.txt`
- 网络请求：`curl https://api.example.com`
- 编译构建：`go build`, `make`
- 任何需要等待结果的命令

### 示例

```
用户: "查看 README.md 的内容"
Bot: exec(command="cat README.md")

用户: "列出当前目录的文件"
Bot: exec(command="ls -la")

用户: "搜索包含 'error' 的日志文件"
Bot: exec(command="grep 'error' app.log")
```

### 重要提示

⚠️ **不要使用 exec 打开 GUI 应用程序**！
- `exec(command="notepad.exe")` 会阻塞直到关闭记事本
- 请使用 `exec_async` 代替

---

## exec_async - 异步执行工具

### 用途

启动应用程序，等待确认启动成功后立即返回，不等待应用程序退出。

### 适用场景

✅ **适合使用 exec_async 的场景**：
- 打开编辑器：`notepad.exe`, `code.exe`, `sublime_text.exe`
- 打开工具：`calc.exe`, `mspaint.exe`
- 打开文件管理器：`explorer.exe`
- 打开浏览器：`chrome.exe`, `firefox.exe`
- 打开终端：`cmd.exe`, `powershell.exe`

### 参数

| 参数 | 类型 | 必需 | 默认值 | 说明 |
|------|------|------|--------|------|
| command | string | ✅ | - | 要执行的命令 |
| working_dir | string | ❌ | 当前目录 | 工作目录 |
| wait_seconds | integer | ❌ | 3 | 等待确认秒数 (1-10) |

### 示例

```
用户: "打开记事本"
Bot: exec_async(command="notepad.exe")

用户: "打开计算器"
Bot: exec_async(command="calc.exe")

用户: "用记事本打开 config.json"
Bot: exec_async(command="notepad.exe config.json")

用户: "打开 VS Code 并等待 5 秒确认"
Bot: exec_async(command="code.exe", wait_seconds=5)
```

### 返回结果

**成功启动**：
```
应用程序已启动: notepad.exe
进程正在运行中
```

**启动失败（闪退）**：
```
应用程序已启动但快速退出（可能闪退）: notepad.exe
```

---

## 决策流程图

```
需要执行命令
    ↓
是否需要返回输出内容？
    ↓
    ├─ 是 → 使用 exec (同步执行)
    │         等待完成并返回结果
    │
    └─ 否 → 使用 exec_async (异步执行)
              启动后立即返回
```

---

## 常见场景示例

### 场景 1: 查看文件

```
用户: "查看 package.json 的内容"
Bot: exec(command="cat package.json")
     [等待执行完成]
Bot: [返回文件内容]
```

### 场景 2: 打开文件编辑

```
用户: "用记事本打开 package.json"
Bot: exec_async(command="notepad.exe package.json")
     [等待 3 秒]
Bot: package.json 已在记事本中打开
```

### 场景 3: 搜索并打开

```
用户: "搜索包含 TODO 的代码文件，然后用 VS Code 打开它"
Bot: exec(command="grep -r 'TODO' ./src")
     [获取文件列表]
Bot: 找到 app.js 有 TODO 标记
Bot: exec_async(command="code.exe src/app.js")
     [启动 VS Code]
Bot: VS Code 已打开 app.js
```

### 场景 4: 编译并运行

```
用户: "编译并运行程序"
Bot: exec(command="go build -o app.exe")
     [等待编译完成]
Bot: 编译成功，大小: 5MB
Bot: exec_async(command="app.exe")
     [启动应用程序]
Bot: 应用程序已启动
```

---

## 最佳实践

### 1. 明确区分使用场景

| 场景 | 使用工具 | 理由 |
|------|----------|------|
| 查看文件 | exec | 需要返回文件内容 |
| 列出目录 | exec | 需要返回目录列表 |
| 搜索内容 | exec | 需要返回搜索结果 |
| 网络请求 | exec | 需要返回响应内容 |
| 打开编辑器 | exec_async | 不需要等待退出 |
| 打开工具 | exec_async | 不需要等待退出 |
| 启动服务 | exec_async | 长期运行的服务 |

### 2. 等待时间选择

| 应用类型 | 推荐等待时间 | 理由 |
|----------|-------------|------|
| 小工具 (记事本、计算器) | 3 秒 | 启动很快 |
| 大应用 (VS Code, Office) | 5 秒 | 启动较慢 |
| 需要加载配置的应用 | 5-10 秒 | 需要初始化 |

### 3. 错误处理

**exec 同步执行错误**：
```
命令执行失败
- 检查命令是否正确
- 检查文件是否存在
- 查看错误输出
```

**exec_async 异步执行错误**：
```
应用程序启动失败或闪退
- 检查应用程序路径
- 查看应用程序日志
- 尝试同步执行查看错误信息
```

---

## 平台差异

### Windows

**同步执行**：
```
exec(command="dir")
exec(command="type file.txt")
```

**异步执行**：
```
exec_async(command="notepad.exe")
exec_async(command="calc.exe")
```

### Linux/macOS

**同步执行**：
```
exec(command="ls -la")
exec(command="cat file.txt")
```

**异步执行**：
```
exec_async(command="gedit file.txt")
exec_async(command="nautilus .")
```

---

## 常见问题

### Q1: 为什么不自动判断是 GUI 应用？

**A**:
- 自动判断不可靠（命令行程序也可能有 GUI）
- 让 LLM 根据意图选择更准确
- 行为更可预测

### Q2: 如何判断使用哪个工具？

**A**:
- 需要看输出 → 使用 `exec`
- 需要启动应用 → 使用 `exec_async`

### Q3: 异步执行后如何知道应用状态？

**A**:
- `exec_async` 会检查进程是否仍在运行
- 如果闪退（快速退出），会返回错误
- 可以使用 `exec` 同步执行查看详细错误

### Q4: 可以同时打开多个应用吗？

**A**:
- 可以！因为 `exec_async` 立即返回
- 可以连续调用多次：
  ```
  exec_async(command="notepad.exe")
  exec_async(command="calc.exe")
  exec_async(command="mspaint.exe")
  ```

---

## 总结

### 核心原则

1. **需要输出** → 用 `exec`
2. **启动应用** → 用 `exec_async`
3. **不确定** → 用 `exec`（更安全）

### LLM 使用指导

当用户说：
- "打开/启动 X" → 使用 `exec_async`
- "查看/列出/搜索 X" → 使用 `exec`
- "编辑 X" → 先用 `exec_async` 打开编辑器

---

**更新日期**: 2026-03-05
