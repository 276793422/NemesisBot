# 外部通道使用指南

## 目录
- [快速开始](#快速开始)
- [配置方式](#配置方式)
- [示例程序](#示例程序)
- [工作原理](#工作原理)
- [故障排除](#故障排除)

---

## 快速开始

### 方式一：使用命令行工具（推荐）

```bash
# 1. 交互式配置外部通道
nemesisbot channel external setup

# 2. 启用外部通道
nemesisbot channel enable external

# 3. 启动网关
nemesisbot gateway
```

### 方式二：手动配置

编辑 `~/.nemesisbot/config.json`：

```json
{
  "channels": {
    "external": {
      "enabled": true,
      "input_exe": "C:\\Tools\\input.exe",
      "output_exe": "C:\\Tools\\output.exe",
      "chat_id": "external:main",
      "sync_to_web": true,
      "web_session_id": ""
    }
  }
}
```

---

## 配置方式

### 命令行配置命令

#### 1. 交互式配置（最简单）

```bash
nemesisbot channel external setup
```

系统会引导您完成：
- 输入程序路径
- 输出程序路径
- 会话 ID 设置
- Web 同步选项

#### 2. 查看当前配置

```bash
nemesisbot channel external config
```

#### 3. 测试外部程序

```bash
nemesisbot channel external test
```

#### 4. 启用/禁用通道

```bash
# 启用
nemesisbot channel enable external

# 禁用
nemesisbot channel disable external
```

### 手动配置文件

配置文件位置：`~/.nemesisbot/config.json`

```json
{
  "channels": {
    "external": {
      "enabled": true,
      "input_exe": "C:\\Path\\To\\input.exe",
      "output_exe": "C:\\Path\\To\\output.exe",
      "chat_id": "external:main",
      "allow_from": [],
      "sync_to_web": true,
      "web_session_id": ""
    }
  }
}
```

#### 配置参数说明

| 参数 | 类型 | 必需 | 说明 |
|------|------|------|------|
| `enabled` | boolean | ✅ | 是否启用外部通道 |
| `input_exe` | string | ✅ | 输入程序的完整路径 |
| `output_exe` | string | ✅ | 输出程序的完整路径 |
| `chat_id` | string | ✅ | 会话标识符（格式：`external:xxx`） |
| `allow_from` | array | ❌ | 允许的用户列表 |
| `sync_to_web` | boolean | ❌ | 是否同步到 Web 界面（默认：true） |
| `web_session_id` | string | ❌ | 指定 Web 会话 ID（空=广播） |

---

## 示例程序

### Windows 批处理示例

#### 输入程序 (input.bat)

```batch
@echo off
:loop
set /p input=
if defined input (
    echo %input%
)
goto loop
```

#### 输出程序 (output.bat)

```batch
@echo off
:loop
set /p input=
if defined input (
    echo AI Response: %input%
)
goto loop
```

### Python 示例

#### 输入程序 (input.py)

```python
#!/usr/bin/env python3
import sys

def main():
    """读取用户输入并输出到 stdout"""
    try:
        while True:
            line = sys.stdin.readline()
            if not line:
                break
            # 输出到 stdout 供 NemesisBot 读取
            print(line.strip())
            sys.stdout.flush()
    except KeyboardInterrupt:
        pass

if __name__ == "__main__":
    main()
```

#### 输出程序 (output.py)

```python
#!/usr/bin/env python3
import sys

def main():
    """从 stdin 接收 AI 响应"""
    try:
        while True:
            line = sys.stdin.readline()
            if not line:
                break
            # 处理 AI 响应
            message = line.strip()
            print(f"AI says: {message}")
            sys.stdout.flush()
    except KeyboardInterrupt:
        pass

if __name__ == "__main__":
    main()
```

### Go 示例

#### 输入程序 (input.go)

```go
package main

import (
    "bufio"
    "fmt"
    "os"
    "strings"
)

func main() {
    scanner := bufio.NewScanner(os.Stdin)
    for scanner.Scan() {
        line := strings.TrimSpace(scanner.Text())
        if line != "" {
            fmt.Println(line)
        }
    }
}
```

#### 输出程序 (output.go)

```go
package main

import (
    "bufio"
    "fmt"
    "os"
)

func main() {
    scanner := bufio.NewScanner(os.Stdin)
    for scanner.Scan() {
        message := scanner.Text()
        // 处理 AI 响应
        fmt.Printf("Received: %s\n", message)
    }
}
```

---

## 工作原理

### 消息流程

```
┌─────────────────────────────────────────────────────────┐
│                   NemesisBot External Channel            │
├─────────────────────────────────────────────────────────┤
│                                                          │
│  1. 用户输入流程:                                       │
│     用户 → 输入程序(stdout) → NemesisBot → LLM           │
│                                                          │
│  2. AI 响应流程:                                        │
│     LLM → NemesisBot → 输出程序(stdin)                   │
│                              ↓                           │
│                        Web界面（同步显示）                 │
│                                                          │
└─────────────────────────────────────────────────────────┘
```

### 程序要求

#### 输入程序要求
- ✅ 从 **stdin** 读取用户输入
- ✅ 将处理后的内容输出到 **stdout**
- ✅ 每行一个输入
- ❌ 不要添加额外的前缀或后缀

#### 输出程序要求
- ✅ 从 **stdin** 读取 AI 响应
- ✅ 可以进行任何格式化处理
- ✅ 可以保存到文件、显示到 GUI 等
- ❌ 不要阻塞程序退出

### Web 同步说明

当 `sync_to_web: true` 时：
- 用户输入会同时显示在 Web 界面
- AI 响应会同时显示在 Web 界面

访问 Web 界面：`http://localhost:8080`

---

## 故障排除

### 问题 1: 程序无法启动

**错误信息**: `failed to start input exe: fork/exec ...`

**解决方案**:
1. 检查路径是否正确（使用绝对路径）
2. 检查文件是否可执行
3. Windows: 使用 `.exe` 或 `.bat` 文件

```bash
# 查看当前配置
nemesisbot channel external config

# 重新配置
nemesisbot channel external setup
```

### 问题 2: 消息无法接收

**可能原因**:
- 输入程序没有输出到 stdout
- 输出格式不正确

**调试方法**:
```bash
# 手动测试输入程序
echo "test" | input.exe

# 应该看到: test
```

### 问题 3: Web 界面看不到消息

**检查配置**:
```json
{
  "external": {
    "sync_to_web": true  // 必须为 true
  }
}
```

**确认 Web 通道已启用**:
```bash
nemesisbot channel status web
```

### 问题 4: 程序启动后立即退出

**可能原因**:
- 程序需要交互式输入
- 程序在等待特定输入

**解决方案**:
确保程序持续运行并从 stdin 读取。

---

## 完整使用示例

### 示例场景：语音助手

假设您有两个程序：
- `voice_to_text.exe` - 语音转文字
- `text_to_speech.exe` - 文字转语音

#### 步骤 1: 配置

```bash
nemesisbot channel external setup
```

输入：
```
Enter path to input executable: C:\Tools\voice_to_text.exe
Enter path to output executable: C:\Tools\text_to_speech.exe
Chat ID: external:voice
Enable web sync: Y
```

#### 步骤 2: 启用

```bash
nemesisbot channel enable external
```

#### 步骤 3: 启动网关

```bash
nemesisbot gateway
```

#### 步骤 4: 使用

1. 对着麦克风说话
2. `voice_to_text.exe` 转换为文字
3. NemesisBot 发送到 LLM
4. LLM 响应发送到 `text_to_speech.exe`
5. 同时在 Web 界面显示对话

---

## 高级用法

### 消息格式转换

您的输出程序可以转换 AI 响应的格式：

```python
# output.py
import sys
import json

def main():
    for line in sys.stdin:
        response = line.strip()

        # 转换为 JSON
        formatted = json.dumps({
            "type": "ai_response",
            "content": response,
            "timestamp": "...",
            "source": "nemesisbot"
        })

        # 发送到其他系统
        send_to_external_system(formatted)

        # 同时显示
        print(f"AI: {response}")
```

### 日志记录

在输出程序中添加日志：

```python
# output.py
import logging
logging.basicConfig(filename='external.log', level=logging.INFO)

def main():
    for line in sys.stdin:
        response = line.strip()
        logging.info(f"AI Response: {response}")
        # 处理响应...
```

---

## 常见应用场景

1. **语音助手** - 语音识别/合成
2. **桌面通知** - 显示系统通知
3. **数据记录** - 记录对话到数据库
4. **格式转换** - Markdown/HTML 转换
5. **多路输出** - 同时发送到多个系统
6. **自定义 UI** - 构建自己的聊天界面

---

## 需要帮助？

### 查看命令帮助
```bash
nemesisbot channel external
```

### 查看配置
```bash
nemesisbot channel external config
```

### 查看所有通道状态
```bash
nemesisbot channel list
```

---

**提示**: 首次使用建议先用简单的批处理文件测试，确保流程通畅后再使用实际的业务程序。
