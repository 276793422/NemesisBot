# 外部通道命令行参数参考

## 快速参考

### 命令格式

```bash
nemesisbot channel external <command> [arguments]
```

---

## 可用命令

### 1. setup - 交互式配置

```bash
nemesisbot channel external setup
```

**说明**：通过交互式向导配置外部通道

**适用场景**：首次配置或需要修改多个参数

---

### 2. config - 查看完整配置

```bash
nemesisbot channel external config
```

**输出示例**：
```
External Channel Configuration
===============================

Enabled:         ✅ Yes
Input EXE:       C:\Tools\input.exe
Output EXE:      C:\Tools\output.exe
Chat ID:         external:main
Sync to Web:     ✅ Yes
Web Session ID:  (not set)
```

---

### 3. set - 设置单个参数

```bash
nemesisbot channel external set <parameter> <value>
```

#### 可设置参数

##### input - 设置输入程序路径

```bash
nemesisbot channel external set input "C:\Tools\input.exe"
```

**说明**：
- 设置用户输入程序的完整路径
- 程序必须从 stdin 读取，输出到 stdout
- 使用绝对路径以避免问题

##### output - 设置输出程序路径

```bash
nemesisbot channel external set output "C:\Tools\output.exe"
```

**说明**：
- 设置 AI 响应输出程序的完整路径
- 程序从 stdin 读取 AI 响应
- 可以进行任何处理（显示、保存、通知等）

##### chat_id - 设置会话 ID

```bash
nemesisbot channel external set chat_id "external:myapp"
```

**说明**：
- 设置会话标识符
- 自动添加 `external:` 前缀（如果省略）
- 用于区分不同的外部通道实例

**示例**：
```bash
# 完整格式
nemesisbot channel external set chat_id "external:voice_assistant"

# 简写（自动添加前缀）
nemesisbot channel external set chat_id "voice_assistant"
```

##### sync - 启用/禁用 Web 同步

```bash
# 启用 Web 同步
nemesisbot channel external set sync "true"

# 禁用 Web 同步
nemesisbot channel external set sync "false"
```

**有效值**：
- `true`, `yes`, `y`, `1`, `on` - 启用
- `false`, `no`, `n`, `0`, `off` - 禁用

**说明**：
- 启用后，所有对话会自动显示在 Web 界面
- 禁用后，只在输出程序中显示

##### session - 设置 Web 会话 ID

```bash
# 设置特定会话 ID
nemesisbot channel external set session "abc123"

# 清空会话 ID（广播到所有会话）
nemesisbot channel external set session ""
```

**说明**：
- 设置后，消息只发送到指定的 Web 会话
- 留空则广播到所有活跃的 Web 会话

---

### 4. get - 获取单个参数

```bash
nemesisbot channel external get <parameter>
```

#### 可获取参数

##### input - 获取输入程序路径

```bash
nemesisbot channel external get input
```

**输出示例**：
```
Input executable: C:\Tools\input.exe
```

##### output - 获取输出程序路径

```bash
nemesisbot channel external get output
```

**输出示例**：
```
Output executable: C:\Tools\output.exe
```

##### chat_id - 获取会话 ID

```bash
nemesisbot channel external get chat_id
```

**输出示例**：
```
Chat ID: external:main
```

##### sync - 获取 Web 同步设置

```bash
nemesisbot channel external get sync
```

**输出示例**：
```
Web sync: enabled (true)
```

##### session - 获取 Web 会话 ID

```bash
nemesisbot channel external get session
```

**输出示例**：
```
Web session ID: abc123
```

或：
```
Web session ID: (not set - will broadcast to all sessions)
```

---

### 5. test - 测试外部程序

```bash
nemesisbot channel external test
```

**说明**：
- 测试输入程序是否能正常工作
- 测试输出程序是否能正常工作
- 交互式测试流程

---

## 使用示例

### 场景 1: 快速配置（直接设置参数）

```bash
# 1. 设置输入程序
nemesisbot channel external set input "C:\Tools\input.exe"

# 2. 设置输出程序
nemesisbot channel external set output "C:\Tools\output.exe"

# 3. 启用 Web 同步
nemesisbot channel external set sync "true"

# 4. 启用通道
nemesisbot channel enable external

# 5. 启动网关
nemesisbot gateway
```

### 场景 2: 查看当前配置

```bash
# 查看完整配置
nemesisbot channel external config

# 查看单个参数
nemesisbot channel external get input
nemesisbot channel external get output
```

### 场景 3: 修改特定参数

```bash
# 只修改输入程序
nemesisbot channel external set input "C:\NewTools\new_input.exe"

# 只禁用 Web 同步
nemesisbot channel external set sync "false"

# 只修改会话 ID
nemesisbot channel external set chat_id "external:production"
```

### 场景 4: 使用不同的输出程序

```bash
# 测试环境
nemesisbot channel external set output "C:\DevTools\test_output.exe"

# 生产环境
nemesisbot channel external set output "C:\ProdTools\output.exe"
```

---

## 完整配置流程示例

### 示例：配置语音助手通道

```bash
# 1. 配置输入程序（语音转文字）
nemesisbot channel external set input "C:\VoiceTools\speech_to_text.exe"

# 2. 配置输出程序（文字转语音）
nemesisbot channel external set output "C:\VoiceTools\text_to_speech.exe"

# 3. 设置会话 ID
nemesisbot channel external set chat_id "external:voice_assistant"

# 4. 启用 Web 同步
nemesisbot channel external set sync "true"

# 5. 查看配置确认
nemesisbot channel external config

# 6. 启用通道
nemesisbot channel enable external

# 7. 启动网关
nemesisbot gateway
```

---

## 常用命令组合

### 查看所有设置

```bash
nemesisbot channel external get input
nemesisbot channel external get output
nemesisbot channel external get chat_id
nemesisbot channel external get sync
```

或直接使用：
```bash
nemesisbot channel external config
```

### 快速启用通道

```bash
# 一行命令设置所有参数
nemesisbot channel external set input "C:\Tools\input.exe" && \
nemesisbot channel external set output "C:\Tools\output.exe" && \
nemesisbot channel external set sync "true" && \
nemesisbot channel enable external
```

### 切换输出程序

```bash
# 从测试程序切换到生产程序
nemesisbot channel external set output "C:\ProdTools\output.exe"

# 重启网关生效
nemesisbot gateway
```

---

## 错误处理

### 文件不存在警告

当设置输入或输出程序时：

```
⚠️  Warning: File not found: C:\Tools\input.exe
Continue anyway? (y/N):
```

- 输入 `y` - 仍然保存配置
- 输入 `N` 或其他 - 取消操作

### 无效参数错误

```
❌ Unknown parameter: xxxxx
```

检查参数名称是否正确。

### 无效值错误

```
❌ Invalid value for sync: xxxxx
Valid values: true, false, yes, no, y, n, 1, 0, on, off
```

使用有效的值。

---

## 提示和技巧

### 1. 使用绝对路径

```bash
# ✅ 好
nemesisbot channel external set input "C:\Tools\input.exe"

# ❌ 差（可能有路径问题）
nemesisbot channel external set input "input.exe"
```

### 2. 使用引号包含路径

```bash
# ✅ 好
nemesisbot channel external set input "C:\My Tools\input.exe"

# ❌ 差（路径中有空格会出错）
nemesisbot channel external set input C:\My Tools\input.exe
```

### 3. 验证配置后再启动

```bash
# 1. 配置
nemesisbot channel external set input "C:\Tools\input.exe"

# 2. 验证
nemesisbot channel external get input

# 3. 测试
nemesisbot channel external test

# 4. 启用
nemesisbot channel enable external

# 5. 启动
nemesisbot gateway
```

### 4. 查看帮助

```bash
# 总体帮助
nemesisbot channel external

# 查看配置
nemesisbot channel external config
```

---

## 参数别名

为了方便，某些参数支持别名：

| 参数 | 别名 |
|------|------|
| `chat_id` | `chatid`, `chat-id` |

```bash
# 以下命令等效：
nemesisbot channel external set chat_id "external:app"
nemesisbot channel external set chatid "external:app"
nemesisbot channel external set chat-id "external:app"
```

---

## 配置持久化

所有通过 `set` 命令的更改会自动保存到配置文件：

```
~/.nemesisbot/config.json
```

**注意**：某些更改（如 input/output 路径）需要重启网关才能生效。

---

## 快速参考卡片

```
┌─────────────────────────────────────────────┐
│     外部通道命令行参数速查卡                │
├─────────────────────────────────────────────┤
│                                             │
│ 查看配置：                                  │
│   nemesisbot channel external config        │
│                                             │
│ 设置参数：                                  │
│   nemesisbot channel external set input <path>     │
│   nemesisbot channel external set output <path>    │
│   nemesisbot channel external set chat_id <id>     │
│   nemesisbot channel external set sync <bool>      │
│   nemesisbot channel external set session <id>    │
│                                             │
│ 获取参数：                                  │
│   nemesisbot channel external get input          │
│   nemesisbot channel external get output         │
│   nemesisbot channel external get chat_id        │
│   nemesisbot channel external get sync           │
│   nemesisbot channel external get session        │
│                                             │
│ 测试程序：                                  │
│   nemesisbot channel external test               │
│                                             │
│ 交互式配置：                                │
│   nemesisbot channel external setup              │
│                                             │
└─────────────────────────────────────────────┘
```

---

**提示**：使用 `tab` 键可以自动补全命令（取决于您的终端）。
