# 外部通道功能完整报告

## 执行摘要

您已经成功完成了：
1. ✅ 执行 `onboard default` - 初始化配置
2. ✅ 设置 LLM - 配置 AI 模型

现在可以使用外部通道功能了！

---

## 一、功能概述

### 外部通道是什么？

外部通道允许您连接**自定义的输入/输出程序**到 NemesisBot：

```
输入程序 (您的程序)
    ↓ stdout
NemesisBot
    ↓ LLM
NemesisBot
    ↓ stdin
输出程序 (您的程序)
    ↓
同时显示到 Web 界面
```

### 使用场景

- 🎤 语音助手（语音识别/合成）
- 📢 桌面通知
- 💾 数据记录
- 🔄 格式转换
- 🎨 自定义 UI
- 🔌 多系统集成

---

## 二、快速开始（5分钟）

### 步骤 1: 配置外部通道

```bash
nemesisbot channel external setup
```

**配置向导会问您**：

1. **输入程序路径**：
   ```
   输入: C:\AI\NemesisBot\NemesisBot\examples\external\input.bat
   ```

2. **输出程序路径**：
   ```
   输入: C:\AI\NemesisBot\NemesisBot\examples\external\output.bat
   ```

3. **Chat ID**：
   ```
   直接按 Enter 使用默认值 (external:main)
   ```

4. **Web 同步**：
   ```
   输入 Y (或直接按 Enter)
   ```

### 步骤 2: 启用外部通道

```bash
nemesisbot channel enable external
```

### 步骤 3: 启动网关

```bash
nemesisbot gateway
```

### 步骤 4: 开始使用

打开浏览器：`http://localhost:8080`

---

## 三、命令行工具

### 新增命令

#### 1. 交互式配置（推荐）

```bash
nemesisbot channel external setup
```

功能：
- ✅ 引导式配置流程
- ✅ 自动验证路径
- ✅ 生成合理默认值

#### 2. 查看配置

```bash
nemesisbot channel external config
```

显示：
- 启用状态
- 输入/输出程序路径
- Chat ID
- Web 同步设置

#### 3. 测试程序

```bash
nemesisbot channel external test
```

功能：
- ✅ 测试输入程序
- ✅ 测试输出程序
- ✅ 交互式测试流程

#### 4. 查看帮助

```bash
nemesisbot channel external
```

### 其他相关命令

```bash
# 查看所有通道
nemesisbot channel list

# 查看特定通道状态
nemesisbot channel status external

# 禁用通道
nemesisbot channel disable external
```

---

## 四、配置文件方式

如果您更喜欢手动编辑配置：

### 配置文件位置

```
~/.nemesisbot/config.json
```

或在 Windows：
```
C:\Users\<YourName>\.nemesisbot\config.json
```

### 配置示例

```json
{
  "channels": {
    "external": {
      "enabled": true,
      "input_exe": "C:\\Tools\\input.exe",
      "output_exe": "C:\\Tools\\output.exe",
      "chat_id": "external:main",
      "sync_to_web": true,
      "web_session_id": "",
      "allow_from": []
    }
  }
}
```

---

## 五、示例程序

我们提供了现成的示例程序：

### Windows 批处理

#### input.bat
```batch
@echo off
:loop
set /p line=
if defined line (
    echo %line%
)
goto loop
```

#### output.bat
```batch
@echo off
:loop
set /p line=
if defined line (
    echo AI: %line%
)
goto loop
```

### Python 脚本

#### input.py
```python
import sys

for line in sys.stdin:
    print(line.strip())
    sys.stdout.flush()
```

#### output.py
```python
import sys
from datetime import datetime

for line in sys.stdin:
    timestamp = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    print(f"[{timestamp}] {line.strip()}")
    sys.stdout.flush()
```

**位置**：`examples/external/` 目录

---

## 六、工作流程详解

### 完整消息流程

```
┌──────────────────────────────────────────────────────────┐
│                   用户交互流程                          │
└──────────────────────────────────────────────────────────┘

1. 用户输入
   ↓
2. 输入程序处理 (可选)
   ↓ (stdout)
3. NemesisBot 接收
   ↓
4. 发送到 LLM
   ↓
5. LLM 响应
   ↓
6. NemesisBot 接收
   ├─→ 输出程序 (stdin) ─→ 处理/显示/保存
   └─→ Web 界面 ────────→ 同步显示

会话历史保存在：
workspace/sessions/external_main.json
```

### 程序要求

#### 输入程序
- ✅ 从 stdin 读取
- ✅ 输出到 stdout
- ✅ 持续运行
- ❌ 不要添加额外格式

#### 输出程序
- ✅ 从 stdin 读取
- ✅ 可以进行任何处理
- ✅ 持续运行
- ✅ 可以保存到文件、显示通知等

---

## 七、Web 同步功能

### 如何启用 Web 同步

**方式一：命令行**
```bash
nemesisbot channel external setup
# 当问 "Enable web sync? (Y/n):" 时输入 Y
```

**方式二：配置文件**
```json
{
  "external": {
    "sync_to_web": true
  }
}
```

### Web 同步的好处

- ✅ 所有对话自动显示在 Web 界面
- ✅ 可以同时使用多个输入方式
- ✅ 完整的聊天历史记录
- ✅ 美观的聊天界面

### 访问 Web 界面

```
http://localhost:8080
```

---

## 八、故障排除

### 问题 1: 命令找不到

**症状**：
```
'nemesisbot' is not recognized as an internal or external command
```

**解决方案**：
```bash
# 使用完整路径
.\nemesisbot.exe channel external setup

# 或者添加到 PATH
```

### 问题 2: 程序路径错误

**症状**：
```
⚠️  Warning: File not found: C:\Tools\input.exe
```

**解决方案**：
1. 使用绝对路径
2. 确保文件存在
3. 检查文件扩展名（.exe 或 .bat）

### 问题 3: Web 界面无消息

**检查清单**：
```bash
# 1. 确认 external 通道启用
nemesisbot channel list

# 2. 确认 Web 同步开启
nemesisbot channel external config

# 3. 确认 Web 通道启用
nemesisbot channel status web

# 4. 重启网关
nemesisbot gateway
```

### 问题 4: 程序启动后立即退出

**原因**：程序需要持续运行并从 stdin 读取

**解决方案**：
确保程序有类似这样的循环：
```batch
:loop
set /p line=
rem 处理 line
goto loop
```

---

## 九、新增文件清单

### 命令行工具
1. `nemesisbot/command/channel_external.go` (新增)
   - 外部通道专用命令
   - 交互式配置流程
   - 测试工具

### 文档
2. `docs/EXTERNAL_CHANNEL_GUIDE.md` (新增)
   - 完整使用指南
   - 示例代码
   - 故障排除

3. `docs/EXTERNAL_CHANNEL_QUICKSTART.md` (新增)
   - 5分钟快速入门
   - 步骤详解
   - 验证清单

### 示例程序
4. `examples/external/input.bat` (新增)
   - Windows 批处理输入示例

5. `examples/external/output.bat` (新增)
   - Windows 批处理输出示例

6. `examples/external/input.py` (新增)
   - Python 输入示例

7. `examples/external/output.py` (新增)
   - Python 输出示例

8. `examples/external/README.md` (新增)
   - 示例程序说明

---

## 十、功能对比

| 功能 | 之前 | 现在 |
|------|------|------|
| **支持平台** | 13 个 | 14 个 (+ External) |
| **配置方式** | 手动编辑 | 命令行 + 手动 |
| **示例程序** | ❌ 无 | ✅ 4 个 |
| **测试工具** | ❌ 无 | ✅ 有 |
| **文档** | ❌ 无 | ✅ 完整 |
| **Web 同步** | ❌ 无 | ✅ 支持 |

---

## 十一、快速参考

### 常用命令速查

```bash
# 配置外部通道
nemesisbot channel external setup

# 查看配置
nemesisbot channel external config

# 启用通道
nemesisbot channel enable external

# 测试程序
nemesisbot channel external test

# 启动网关
nemesisbot gateway

# 查看所有通道
nemesisbot channel list
```

### 配置文件路径

```
~/.nemesisbot/config.json
```

### 会话历史路径

```
~/.nemesisbot/workspace/sessions/external_main.json
```

### Web 访问地址

```
http://localhost:8080
```

---

## 十二、总结

### ✅ 已完成

1. ✅ 扩展命令行工具支持 external 通道
2. ✅ 创建交互式配置向导
3. ✅ 添加配置查看命令
4. ✅ 添加程序测试工具
5. ✅ 提供 4 个示例程序
6. ✅ 编写完整文档
7. ✅ 创建快速入门指南

### 🎯 您现在可以

1. 使用 `nemesisbot channel external setup` 配置通道
2. 使用 `nemesisbot channel external config` 查看配置
3. 使用 `nemesisbot channel external test` 测试程序
4. 同时在 Web 界面看到对话内容
5. 根据示例程序自定义您的功能

### 📚 文档位置

- **快速入门**: `docs/EXTERNAL_CHANNEL_QUICKSTART.md`
- **完整指南**: `docs/EXTERNAL_CHANNEL_GUIDE.md`
- **示例程序**: `examples/external/`

---

## 下一步建议

1. **先试用示例程序**
   ```bash
   nemesisbot channel external setup
   # 使用 examples/external/ 中的程序
   ```

2. **测试基本功能**
   - 确认输入输出正常
   - 确认 Web 界面同步

3. **自定义您的程序**
   - 基于示例程序修改
   - 添加您需要的功能

4. **集成到生产环境**
   - 替换为实际的业务程序
   - 配置好路径和参数

---

**祝使用愉快！** 🎉

如有问题，随时查看文档或使用帮助命令。
