# 外部通道命令行配置指南

## 新功能：命令行参数设置

除了交互式配置（`setup`），您现在可以使用命令行直接设置任何参数！

---

## 快速开始

### 方式一：命令行直接设置（新增）⚡

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

### 方式二：交互式配置

```bash
nemesisbot channel external setup
```

---

## 完整命令参考

### set 命令 - 设置参数

```bash
nemesisbot channel external set <parameter> <value>
```

#### 可设置的参数

| 参数 | 说明 | 示例 |
|------|------|------|
| `input` | 输入程序路径 | `C:\Tools\input.exe` |
| `output` | 输出程序路径 | `C:\Tools\output.exe` |
| `chat_id` | 会话 ID | `external:myapp` 或 `myapp` |
| `sync` | Web 同步 | `true` 或 `false` |
| `session` | Web 会话 ID | `abc123` 或空字符串 |

#### 使用示例

```bash
# 设置输入程序
nemesisbot channel external set input "C:\AI\NemesisBot\NemesisBot\examples\external\input.bat"

# 设置输出程序
nemesisbot channel external set output "C:\AI\NemesisBot\NemesisBot\examples\external\output.bat"

# 设置会话 ID
nemesisbot channel external set chat_id "external:test"

# 启用 Web 同步
nemesisbot channel external set sync "true"

# 禁用 Web 同步
nemesisbot channel external set sync "false"

# 设置 Web 会话 ID
nemesisbot channel external set session "abc123def456"

# 清空 Web 会话 ID（广播到所有）
nemesisbot channel external set session ""
```

### get 命令 - 获取参数

```bash
nemesisbot channel external get <parameter>
```

#### 可获取的参数

| 参数 | 说明 |
|------|------|
| `input` | 获取输入程序路径 |
| `output` | 获取输出程序路径 |
| `chat_id` | 获取会话 ID |
| `sync` | 获取 Web 同步状态 |
| `session` | 获取 Web 会话 ID |

#### 使用示例

```bash
# 获取输入程序路径
nemesisbot channel external get input

# 获取输出程序路径
nemesisbot channel external get output

# 获取会话 ID
nemesisbot channel external get chat_id

# 获取 Web 同步状态
nemesisbot channel external get sync

# 获取 Web 会话 ID
nemesisbot channel external get session
```

---

## 实际使用场景

### 场景 1：快速配置（推荐）

```bash
# 使用示例程序快速测试
nemesisbot channel external set input "C:\AI\NemesisBot\NemesisBot\examples\external\input.bat"
nemesisbot channel external set output "C:\AI\NemesisBot\NemesisBot\examples\external\output.bat"
nemesisbot channel external set sync "true"
nemesisbot channel enable external
nemesisbot gateway
```

### 场景 2：生产环境部署

```bash
# 设置生产程序路径
nemesisbot channel external set input "C:\Production\voice_input.exe"
nemesisbot channel external set output "C:\Production\voice_output.exe"
nemesisbot channel external set chat_id "external:production"
nemesisbot channel external set sync "true"
nemesisbot channel enable external
```

### 场景 3：只修改特定参数

```bash
# 只修改输出程序
nemesisbot channel external set output "C:\NewTools\new_output.exe"

# 只禁用 Web 同步
nemesisbot channel external set sync "false"

# 只修改会话 ID
nemesisbot channel external set chat_id "external:newapp"
```

### 场景 4：查询配置

```bash
# 查看所有配置
nemesisbot channel external config

# 查看特定参数
nemesisbot channel external get input
nemesisbot channel external get output
nemesisbot channel external get sync
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

### 验证配置后启动

```bash
# 1. 配置
nemesisbot channel external set input "C:\Tools\input.exe"

# 2. 验证
nemesisbot channel external get input

# 3. 查看完整配置
nemesisbot channel external config

# 4. 启用
nemesisbot channel enable external

# 5. 启动
nemesisbot gateway
```

---

## 参数详解

### input 参数

**作用**：设置用户输入程序的路径

**要求**：
- 程序必须从 stdin 读取输入
- 程序必须输出到 stdout
- 程序应该持续运行（不退出）

**示例**：
```bash
# Windows 批处理
nemesisbot channel external set input "C:\Tools\input.bat"

# Python 脚本
nemesisbot channel external set input "C:\Tools\input.py"

# Go 程序
nemesisbot channel external set input "C:\Tools\input.exe"
```

### output 参数

**作用**：设置 AI 响应输出程序的路径

**要求**：
- 程序从 stdin 读取 AI 响应
- 程序可以自由处理（显示、保存、通知等）
- 程序应该持续运行

**示例**：
```bash
# 简单输出
nemesisbot channel external set output "C:\Tools\output.bat"

# 带日志记录
nemesisbot channel external set output "C:\Tools\output_with_log.exe"

# 语音合成
nemesisbot channel external set output "C:\Tools\text_to_speech.exe"
```

### chat_id 参数

**作用**：设置会话标识符

**格式**：`external:<name>`

**说明**：
- 用于区分不同的外部通道实例
- 自动添加 `external:` 前缀（如果省略）
- 影响会话历史文件名

**示例**：
```bash
# 完整格式
nemesisbot channel external set chat_id "external:voice"

# 简写（自动添加前缀）
nemesisbot channel external set chat_id "voice"

# 自定义名称
nemesisbot channel external set chat_id "external:my_custom_app"
```

**会话历史文件**：
```
~/.nemesisbot/workspace/sessions/external_voice.json
~/.nemesisbot/workspace/sessions/external_my_custom_app.json
```

### sync 参数

**作用**：启用/禁用 Web 界面同步

**有效值**：
- 启用：`true`, `yes`, `y`, `1`, `on`
- 禁用：`false`, `no`, `n`, `0`, `off`

**示例**：
```bash
# 启用 Web 同步
nemesisbot channel external set sync "true"
nemesisbot channel external set sync "yes"
nemesisbot channel external set sync "1"

# 禁用 Web 同步
nemesisbot channel external set sync "false"
nemesisbot channel external set sync "no"
```

### session 参数

**作用**：设置目标 Web 会话 ID

**说明**：
- 设置后，消息只发送到指定的 Web 会话
- 留空（默认）则广播到所有活跃的 Web 会话

**示例**：
```bash
# 只发送到特定会话
nemesisbot channel external set session "abc123def456"

# 广播到所有会话（默认）
nemesisbot channel external set session ""
```

---

## 命令对比

### 交互式 vs 命令行

| 需求 | 推荐方式 | 原因 |
|------|----------|------|
| 首次配置 | `setup` | 引导式，不容易出错 |
| 修改单个参数 | `set` | 快速直接 |
| 查看所有配置 | `config` | 一目了然 |
| 查看单个参数 | `get` | 精确获取 |
| 测试程序 | `test` | 独立测试 |

---

## 常见问题

### Q: 如何切换输入/输出程序？

**A**:
```bash
# 重新设置路径
nemesisbot channel external set input "C:\NewTools\new_input.exe"
nemesisbot channel external set output "C:\NewTools\new_output.exe"
```

### Q: 如何只查看一个参数？

**A**:
```bash
nemesisbot channel external get input
nemesisbot channel external get sync
```

### Q: 设置后需要重启吗？

**A**: 部分设置需要重启网关：
- ✅ 需要重启：`input`, `output`, `chat_id`
- ❌ 不需要重启：`sync`, `session`

### Q: 如何快速切换测试/生产环境？

**A**: 保存配置文件为不同版本

```bash
# 开发环境
cp ~/.nemesisbot/config.json ~/.nemesisbot/config.dev.json

# 生产环境
cp ~/.nemesisbot/config.json ~/.nemesisbot/config.prod.json

# 切换时复制回来
cp ~/.nemesisbot/config.dev.json ~/.nemesisbot/config.json
```

---

## 高级技巧

### 1. 使用脚本批量配置

**Windows 批处理**：
```batch
@echo off
nemesisbot channel external set input "%1"
nemesisbot channel external set output "%2"
nemesisbot channel external set sync "true"
nemesisbot channel enable external
```

使用：
```batch
configure.bat "C:\Tools\input.exe" "C:\Tools\output.exe"
```

### 2. 验证配置完整性

```bash
# 检查所有必需参数
nemesisbot channel external get input
nemesisbot channel external get output

# 确认配置正确
nemesisbot channel external config
```

### 3. 测试后再启用

```bash
# 1. 配置
nemesisbot channel external set input "C:\Tools\input.exe"
nemesisbot channel external set output "C:\Tools\output.exe"

# 2. 测试程序
nemesisbot channel external test

# 3. 确认无问题后启用
nemesisbot channel enable external
```

---

## 总结

### 新增的命令

```bash
# 设置参数
nemesisbot channel external set <parameter> <value>

# 获取参数
nemesisbot channel external get <parameter>
```

### 可用的参数

- `input` - 输入程序路径
- `output` - 输出程序路径
- `chat_id` - 会话 ID
- `sync` - Web 同步开关
- `session` - Web 会话 ID

### 完整配置流程

```bash
# 1. 设置参数
nemesisbot channel external set input "C:\Tools\input.exe"
nemesisbot channel external set output "C:\Tools\output.exe"
nemesisbot channel external set sync "true"

# 2. 验证配置
nemesisbot channel external config

# 3. 启用通道
nemesisbot channel enable external

# 4. 启动网关
nemesisbot gateway
```

---

**提示**：所有配置更改都会自动保存到 `~/.nemesisbot/config.json`
