# NemesisBot WebSocket Client

一个功能完整的 Rust WebSocket 客户端，用于与 NemesisBot 的 WebSocket 通道进行通信。

## 功能特性

- ✅ **交互式命令行界面** - 简单易用的 REPL 循环
- ✅ **自动重连** - 连接断开时自动尝试重新连接，支持指数退避
- ✅ **消息规则** - 支持自定义规则处理错误消息，提升用户体验
- ✅ **消息日志** - 记录所有发送和接收的消息
- ✅ **连接统计** - 实时跟踪消息数量、字节数、重连次数等
- ✅ **彩色输出** - 使用颜色区分不同类型的消息
- ✅ **灵活配置** - 支持通过配置文件自定义行为
- ✅ **单事件循环** - 避免并发读写冲突，确保稳定性

## 版本历史

### v0.4.0 (2026-03-01)
- ✨ **重大更新**: 外部程序集成支持
  - 新增 `-i,--input` 参数：指定外部输入程序（读取stdout作为客户端输入）
  - 新增 `-o,--output` 参数：指定外部输出程序（将响应通过stdin发送）
  - 实现全局请求锁：确保同一时间只处理一个请求
  - 自动重试机制：外部程序失败后自动重启（最多6次，间隔10秒）
  - 降级策略：外部程序失败后自动切换到CLI模式
  - 繁忙控制：请求处理中拒绝后续输入，提示"繁忙中，别那么着急"
  - 配对日志：记录每次Input/Output对应关系

### v0.3.1 (2026-03-01)
- ✨ **新增功能**: 消息规则支持"跳过"模式
  - 添加 `skip` 字段，可以完全跳过某些消息的显示
  - 内置上下文窗口超限规则：自动跳过 "Context window exceeded" 消息
  - 两种规则模式：
    - **替换模式** (`skip=false`): 将消息替换为友好文本
    - **跳过模式** (`skip=true`): 不显示消息，完全忽略
  - 跳过的消息会在日志中记录，控制台显示 `🚫 Message skipped` 提示
- 📝 **文档更新**: 添加 skip 模式说明和使用示例

### v0.3.0 (2026-03-01)
- ✨ **新增功能**: 消息规则处理系统
  - 支持自定义规则匹配和替换服务器返回的错误消息
  - 可配置的模式匹配（支持大小写敏感/不敏感）
  - 内置 API 限流错误处理（429 错误）
  - 规则按顺序匹配，第一条匹配的规则生效
  - 可通过配置文件轻松添加新规则
  - 应用规则时显示提示，便于调试
- 📝 **文档更新**: 添加消息规则使用说明和示例

### v0.2.0 (2026-03-01)
- 🐛 **修复严重 bug**: 修复了自动重连功能完全失效的问题
  - 原因：`external_rx` receiver 在首次连接时被消费，导致重连时无法获取输入
  - 修复：重新设计 receiver 生命周期，支持跨多次连接复用
- 🧹 **代码清理**: 移除未使用的异步版本的函数
- 📝 **更新文档**: 完善使用说明和故障排除指南

### v0.1.0 (2026-02-28)
- 初始版本
- 基本 WebSocket 通信功能
- 自动重连支持
- 消息日志和统计

## 安装

### 前置要求

- Rust 1.70 或更高版本
- NemesisBot WebSocket 服务器运行中

### 编译

```bash
cd C:\AI\NemesisBot\NemesisBot\test\WebSocketClient
cargo build --release
```

编译后的可执行文件位于 `target/release/websocket-client.exe`

## 配置

配置文件位置：
- **Windows**: `%APPDATA%\websocket_client\config.toml`
- **Linux/macOS**: `~/.config/websocket_client/config.toml`

首次运行会自动创建默认配置文件。

### 配置项说明

```toml
[server]
# WebSocket 服务器 URL
url = "ws://127.0.0.1:49001/ws"
# 认证令牌（如果不需要认证留空）
token = ""

[reconnect]
# 启用自动重连
enabled = true
# 最大重连次数（0 = 无限重连）
max_attempts = 0
# 初始重连延迟（秒）
initial_delay = 1
# 最大重连延迟（秒）
max_delay = 30
# 重连延迟倍数（指数退避）
delay_multiplier = 2.0

[heartbeat]
# 启用心跳检测（注意：当前版本未实现心跳功能）
enabled = true
# 心跳间隔（秒）
interval = 30
# 等待 pong 超时时间（秒）
timeout = 10

[logging]
# 启用日志
enabled = true
# 日志文件路径（空 = 不记录到文件）
file = "websocket_client.log"
# 日志级别: error, warn, info, debug, trace
level = "info"
# 记录消息内容（可能很大）
log_messages = true

[ui]
# 启用彩色输出
color = true
# 显示时间戳
show_timestamp = true
# 显示消息统计
show_stats = true
# 提示符样式: simple, detailed
prompt_style = "simple"

[statistics]
# 启用统计
enabled = true
# 统计打印间隔（秒）（0 = 禁用自动打印）
print_interval = 0

[message_rules]
# 启用消息规则处理
enabled = true

# 规则列表（按顺序匹配，第一条匹配的规则生效）
[[message_rules.rules]]
# 规则名称/标识符
name = "api-rate-limit"
# 规则描述
description = "API访问量过大，模型繁忙"
# 匹配模式（支持子串匹配，默认不区分大小写）
pattern = "Error processing message: LLM call failed after retries"
# 替换文本
replacement = "【目前我有点忙，要不然你等会再叫我】"
# 是否启用此规则
enabled = true
# 是否区分大小写
case_sensitive = false
```

### 消息规则说明

消息规则允许你自定义处理服务器返回的错误消息，提升用户体验。

#### 工作原理

1. 当收到服务器的 `message` 类型消息时，客户端会按顺序检查所有启用的规则
2. 如果消息内容包含规则的 `pattern`，则根据规则配置执行操作：
   - **替换模式** (`skip = false`): 将消息替换为 `replacement` 文本
   - **跳过模式** (`skip = true`): 不显示此消息，完全忽略
3. 规则匹配成功后，会显示提示：
   - 替换：`🔔 Applied rule: <规则名>`
   - 跳过：`🚫 Message skipped (rule: <规则名>)`
4. 匹配顺序：从上到下，第一条匹配的规则生效

#### 配置示例

```toml
[message_rules]
enabled = true

# 示例 1: API 限流错误（替换模式）
[[message_rules.rules]]
name = "api-rate-limit"
description = "API访问量过大"
pattern = "Error processing message: LLM call failed after retries: API request failed:\nStatus: 429"
replacement = "【目前我有点忙，要不然你等会再叫我】"
enabled = true
case_sensitive = false
skip = false

# 示例 2: 上下文窗口超限（跳过模式）
[[message_rules.rules]]
name = "context-window-exceeded"
description = "上下文窗口超限，压缩历史重试"
pattern = "Context window exceeded. Compressing history and retrying..."
replacement = ""
enabled = true
case_sensitive = false
skip = true  # 跳过此消息，不显示

# 示例 3: 网络超时错误
[[message_rules.rules]]
name = "network-timeout"
description = "网络超时"
pattern = "timeout"
replacement = "【网络似乎有点问题，请稍后再试】"
enabled = true
case_sensitive = false
skip = false

# 示例 4: 通用错误处理
[[message_rules.rules]]
name = "generic-error"
description = "通用错误"
pattern = "Error:"
replacement = "【发生了一些问题，请稍后再试】"
enabled = false  # 可以禁用某个规则
case_sensitive = false
skip = false
```

#### 规则字段说明

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `name` | string | 是 | 规则的唯一标识符，用于日志显示 |
| `description` | string | 是 | 规则的描述，便于理解规则用途 |
| `pattern` | string | 是 | 匹配模式，支持子串匹配（不需要正则表达式） |
| `replacement` | string | 是 | 替换文本，当 `skip=false` 时使用此文本替换整个消息 |
| `enabled` | bool | 否 | 是否启用此规则，默认 `true` |
| `case_sensitive` | bool | 否 | 是否区分大小写，默认 `false` |
| `skip` | bool | 否 | 是否跳过消息（不显示），默认 `false`。设为 `true` 时忽略 `replacement` |

#### 规则类型

**替换模式** (`skip = false`)
- 匹配时将消息替换为 `replacement` 文本
- 适用于：错误消息友好化、状态提示等
- 示例：API 限流错误 → "【目前我有点忙，要不然你等会再叫我】"

**跳过模式** (`skip = true`)
- 匹配时不显示消息，完全忽略
- 适用于：中间状态消息、调试信息、不需要用户看到的系统消息
- 示例：上下文压缩提示 → 不显示

#### 添加新规则

1. 打开配置文件（`%APPDATA%\websocket_client\config.toml`）
2. 在 `[[message_rules.rules]]` 部分添加新规则
3. 重启客户端使配置生效

**提示**：
- 将更具体的规则放在前面
- 使用 `enabled: false` 可以临时禁用某个规则而不删除它
- 查看日志文件可以看到哪些规则被应用了

## 使用方法

### 启动客户端

```bash
# 基本使用（纯CLI模式）
./target/release/websocket-client.exe

# 使用外部输入程序
./target/release/websocket-client.exe -i input.exe

# 使用外部输出程序
./target/release/websocket-client.exe -o output.exe

# 同时使用输入和输出程序
./target/release/websocket-client.exe -i input.exe -o output.exe

# 或使用 cargo run
cargo run --release -- -i input.exe -o output.exe
```

### 外部程序集成

#### 架构示意

```
                    ┌─────────────────────────────────────┐
                    │        WebSocket Client             │
                    │                                     │
                    │  ┌─────────────────────────────┐    │
                    │  │   Global Request Lock        │    │
                    │  │  (一次只能一个请求在处理)     │    │
                    │  └─────────────────────────────┘    │
                    │           ↓         ↑               │
                    │  ┌──────────┐      │  ┌──────────┐  │
                    │  │ Input    │      │  │  Output  │  │
                    │  │ Program  │      │  │  Program │  │
                    │  │  (-i)    │      │  │   (-o)   │  │
                    │  └──────────┘      │  └──────────┘  │
                    │       ↓               │      ↓      │
                    │  发送到 WebSocket   │  发送到 stdout │
                    │       ↓               │      ↓      │
                    │  WebSocket Server    │   显示/处理  │
                    │                                     │
                    └─────────────────────────────────────┘
```

#### Input 程序 (-i 参数)

指定外部程序作为输入源：

```bash
websocket-client.exe -i C:/path/to/input.exe
```

**工作原理**：
- 客户端启动 `input.exe`
- 读取 `input.exe` 的 stdout
- 每行作为一条消息发送到 WebSocket 服务器
- 程序退出后自动重启（最多6次，间隔10秒）
- 失败后降级到 CLI 输入

**Input 程序示例**：
```batch
@echo off
:loop
set /p line=
if defined line (
    echo %line%
)
goto loop
```

#### Output 程序 (-o 参数)

指定外部程序接收响应：

```bash
websocket-client.exe -o C:/path/to/output.exe
```

**工作原理**：
- 客户端启动 `output.exe`
- 收到服务器响应后，将内容通过 stdin 发送给它
- 每条消息一行（带 `\n`）
- 程序退出后自动重启（最多6次，间隔10秒）
- 失败后降级到 CLI 输出

**Output 程序示例**：
```batch
@echo off
:loop
set /p line=
echo Processing: %line%
REM 处理消息...
goto loop
```

### 繁忙控制

客户端实现全局请求锁，确保同一时间只处理一个请求：

```bash
➤ 用户输入1
繁忙中，别那么着急
➤ 用户输入2
繁忙中，别那么着急
[服务器响应到达]
➤ 用户输入2  # 现在可以输入了
```

**特性**：
- 无论从 CLI 还是 Input 程序输入，都受同一个锁控制
- 请求处理中，所有后续输入被拒绝
- 收到响应后立即解除繁忙状态
- 显示提示：`繁忙中，别那么着急`

### 参数组合

| 参数 | 效果 |
|------|------|
| 无 | 纯 CLI 模式 |
| `-i` | Input 程序输入 + CLI 输出 |
| `-o` | CLI 输入 + Output 程序输出 |
| `-i -o` | Input 程序输入 + Output 程序输出 |

### 降级机制

**Input 程序失败**：
```
尝试 1/6: 失败，10秒后重试
尝试 2/6: 失败，10秒后重试
...
尝试 6/6: 失败
⚠️  Input program failed after 6 retries, switching to CLI input
```

**Output 程序失败**：
```
程序启动
处理消息...
程序异常退出
尝试 1/6: 重启...
...
尝试 6/6: 失败
⚠️  Output program failed after 6 retries, switching to CLI output
```

### 启动客户端

```bash
# 直接运行（使用默认配置）
./target/release/websocket-client.exe

# 或使用 cargo run
cargo run --release

# 开发模式运行（带日志）
RUST_LOG=debug cargo run
```

### 命令

在客户端中可以使用的命令：

- `/help` 或 `/h` - 显示帮助信息
- `/quit`, `/exit`, `/q` - 退出客户端
- `/stats` - 显示连接统计信息
- `/clear` 或 `/c` - 清屏

其他任何输入都会作为消息发送到服务器。

### 消息格式

**发送消息**（客户端 → 服务器）：
```json
{
  "type": "message",
  "content": "你好",
  "timestamp": "2026-03-01T12:00:00+08:00"
}
```

**接收消息**（服务器 → 客户端）：
```json
{
  "type": "message",
  "role": "assistant",
  "content": "你好！有什么我可以帮助你的吗？",
  "timestamp": "2026-03-01T12:00:01.123456+08:00"
}
```

支持的消息类型：
- `message` - 普通消息（包含 `role`, `content`, `timestamp`）
- `pong` - 心跳响应
- `error` - 错误消息（包含 `error` 字段）

### 输出示例

```
╔════════════════════════════════════════════════════════╗
║
║  🤖 NemesisBot WebSocket Client v0.2.0
║
╚════════════════════════════════════════════════════════╝

📁 Configuration:
   Server URL: ws://127.0.0.1:49001/ws
   Auto-reconnect: ✅
   Heartbeat: ✅
   Logging: ✅

✅ Ready! Type your messages below.

📖 Available Commands:
  /help - Show this help message
  /quit, /exit - Exit the client
  /stats - Show connection statistics
  /clear - Clear the screen
  ... - Any other text will be sent as a message to the server

➤ 你好
📤 [TX] 你好
✅ Sent
📥 [RX] 152 bytes
[12:00:01] 🤖 Assistant: 你好！有什么我可以帮助你的吗？

➤ /quit

🔌 Connection closed

📊 Sent: 1 msgs | Received: 1 msgs | Reconnects: 0
```

## 开发

### 项目结构

```
WebSocketClient/
├── Cargo.toml          # 依赖配置
├── config.toml         # 默认配置（本地使用）
├── src/
│   ├── main.rs         # 程序入口和 CLI
│   ├── client.rs       # WebSocket 客户端实现
│   └── config.rs       # 配置管理
└── README.md           # 本文件
```

### 依赖库

- `tokio-tungstenite` - WebSocket 客户端
- `tokio` - 异步运行时
- `serde` / `serde_json` - JSON 序列化
- `anyhow` / `thiserror` - 错误处理
- `colored` / `crossterm` - 终端 UI
- `chrono` - 时间处理
- `log` / `env_logger` - 日志

### 架构设计

#### 单事件循环模型

客户端采用类似 JavaScript 的单事件循环模型，确保不会有并发的读写操作：

```rust
loop {
    tokio::select! {
        // 接收来自 WebSocket 的消息
        msg = ws_receiver.next() => { /* 处理 */ }

        // 接收来自 CLI 的输入
        msg = cli_rx.recv() => { /* 发送到 WebSocket */ }
    }
}
```

这种设计的优点：
- 避免了 WebSocket 写入冲突（gorilla/websocket 不支持并发写入）
- 代码逻辑清晰，易于调试
- 自动避免资源竞争

#### 重连机制

1. 在 `start()` 方法中，`external_rx` receiver 被一次性取出
2. 每次 `connect_and_run()` 调用时，receiver 以可变引用传入
3. 当连接断开时，receiver 仍然可用，可以继续接收 CLI 输入
4. 支持指数退避算法，延迟从 1 秒开始，每次翻倍，最大 30 秒

#### 线程模型

- **主线程（async runtime）**: 运行 WebSocket 客户端
- **CLI 线程（同步）**: 运行输入循环，避免阻塞异步运行时（Windows 平台重要）

两个线程之间通过 `mpsc::unbounded_channel` 通信。

## 故障排除

### 连接失败

**症状**: 启动后显示 "Connection error"

**解决方案**:
1. 确保 NemesisBot 正在运行：`.\nemesisbot.exe gateway`
2. 检查配置文件中的 `url` 是否正确（默认 `ws://127.0.0.1:49001/ws`）
3. 确认端口没有被其他程序占用：`netstat -ano | findstr 49001`
4. 如果需要认证，检查 `token` 配置

### 自动重连不工作

**症状**: 连接断开后没有自动重连

**解决方案**:
1. 检查配置文件中的 `reconnect.enabled` 是否为 `true`
2. 检查是否达到最大重连次数（`reconnect.max_attempts`）
3. 查看日志文件了解详细错误信息

### 无法输入消息

**症状**: 客户端已连接，但键盘输入无响应

**解决方案**:
1. 这是 Windows 平台的已知问题，已通过使用独立线程解决
2. 确保使用的是最新版本（v0.2.0+）
3. 尝试重新启动客户端

### 日志未记录

**症状**: 消息没有写入日志文件

**解决方案**:
1. 检查配置文件中的 `logging.enabled` 是否为 `true`
2. 检查 `logging.file` 路径是否有效
3. 确保程序有写入权限

### 编译错误

**症状**: `cargo build` 失败

**解决方案**:
1. 确保使用的是 Rust 1.70 或更高版本：`rustc --version`
2. 更新依赖：`cargo update`
3. 清理重新编译：`cargo clean && cargo build`

## 测试

### 单元测试

```bash
cargo test
```

### 集成测试

需要先启动 NemesisBot WebSocket 服务器：

```bash
# Terminal 1: 启动 NemesisBot
.\nemesisbot.exe gateway

# Terminal 2: 运行客户端
cargo run --release
```

### 手动测试流程

1. 启动客户端
2. 等待连接成功（显示 "✅ Connected!"）
3. 发送测试消息：`hello`
4. 查看是否收到回复
5. 测试重连：重启 NemesisBot，观察客户端是否自动重连
6. 测试命令：`/stats`, `/clear`, `/help`
7. 退出：`/quit`

## 已知问题

1. **心跳功能未实现**: 配置中有 `[heartbeat]` 选项，但当前版本未实现实际的 ping/pong 机制
2. **Windows 控制台编码**: 在某些 Windows 终端中，中文字符可能显示不正确，建议使用 Windows Terminal 或 PowerShell

## 许可证

MIT License

## 贡献

欢迎提交 Issue 和 Pull Request！

### 贡献指南

1. Fork 项目
2. 创建特性分支：`git checkout -b feature/amazing-feature`
3. 提交更改：`git commit -m 'Add amazing feature'`
4. 推送分支：`git push origin feature/amazing-feature`
5. 提交 Pull Request

## 联系方式

- 项目主页: [NemesisBot](https://github.com/276793422/NemesisBot)
- 问题反馈: [GitHub Issues](https://github.com/276793422/NemesisBot/issues)

## 致谢

感谢所有 NemesisBot 项目的贡献者！
