# 自动化测试流程 Skill 

完整的自动化测试流程，用于测试需要 AI 支持的功能，使用 TestAIServer 作为模拟后端。

---

## 概述

此 Skill 提供了一个完整的自动化测试流程，用于测试 NemesisBot 的各种功能。它使用 TestAIServer 作为模拟 AI 后端，通过 WebSocket API 与 Bot 通信，执行测试用例并验证结果。

### 适用场景

- ✅ 需要调用 LLM 的功能测试
- ✅ 工具调用测试（文件操作、集群通信等）
- ✅ 消息处理和响应验证
- ✅ 回归测试（代码修改后重新测试）
- ❌ 不适用于需要 UI 交互的功能（如安全审批对话框）
- ❌ 不适用于不需要 LLM 的功能测试（在不需要 LLM 的功能测试中，可通过 UT/IT/ST 保证功能有效）

### 测试工作目录

**⚠️ 重要**：本流程使用 `test/autotest/` 作为统一的测试工作目录。

**目录结构**:
```
test/
├── TestAIServer/              # 源码（不变）
│   ├── main.go
│   └── ...
├── websocket_chat_client.go   # 源码（不变）
├── mcp/                       # 源码（不变）
├── cluster/                   # 源码（不变）
└── autotest/                  # 测试工作目录（新建）
    ├── nemesisbot.exe         # 编译产物
    ├── testaiserver.exe       # 编译产物
    ├── websocket_chat_client.exe  # 编译产物
    ├── testaiserver.pid       # 进程 ID
    ├── nemesisbot.pid         # 进程 ID
    ├── nemesisbot.log         # Bot 日志
    └── .nemesisbot/           # Bot 配置（运行时生成）
        ├── config.json
        ├── workspace/
        └── ...
```

**清理方式**: 删除整个 `test/autotest/` 目录

**测试报告**: 保存到 `docs/REPORT/`（不在 `test/autotest/` 中）

---

## WebSocket 接口规范

### 连接信息

```
协议: ws://
地址: 127.0.0.1
端口: 49001  # Bot 的 WebSocket 端口
路径: /ws
完整 URL: ws://127.0.0.1:49001/ws
```

### 客户端消息格式

```json
{
  "type": "message",
  "content": "用户消息内容",
  "timestamp": "2026-03-23T10:30:00Z"
}
```

**字段说明**:
- `type`: 消息类型，固定为 "message"
- `content`: 消息内容（可以是特殊标记，如 `<FILE_OP>...</FILE_OP>`）
- `timestamp`: 可选的时间戳（RFC3339 格式）

### 服务端消息格式

```json
{
  "type": "message",
  "role": "assistant",
  "content": "AI 响应内容",
  "timestamp": "2026-03-23T10:30:05Z"
}
```

**字段说明**:
- `type`: 消息类型
  - "message": 正常消息
  - "error": 错误消息
  - "pong": 心跳响应
- `role`: 角色
  - "assistant": AI 助手响应
  - "user": 用户消息（回显）
- `content`: 消息内容
- `timestamp`: 时间戳

---

## 辅助脚本

本 Skill 提供了两个辅助脚本，用于简化测试流程：

### 脚本位置

```
Skills/automated-testing/scripts/
├── setup-env.sh       # 环境准备（Bash/Linux）
├── setup-env.ps1      # 环境准备（PowerShell/Windows）
├── cleanup-env.sh     # 环境清理（Bash/Linux）
└── cleanup-env.ps1    # 环境清理（PowerShell/Windows）
```

### 脚本职责

#### setup-env.sh / setup-env.ps1

**负责**（机械性工作）:
- ✅ 检查环境（Go、必要目录）
- ✅ 创建 `test/autotest/` 目录
- ✅ 并行编译三个组件
- ✅ 启动 TestAIServer（后台）
- ✅ 保存 TestAIServer PID
- ✅ 健康检查

**不负责**（由 AI 处理）:
- ❌ 执行 onboard default --local
- ❌ 配置模型（model add）
- ❌ 启动 NemesisBot Gateway
- ❌ 执行测试
- ❌ 分析结果
- ❌ 删除测试目录

#### cleanup-env.sh / cleanup-env.ps1

**负责**（机械性工作）:
- ✅ 停止 nemesisbot.exe（通过进程名）
- ✅ 停止 testaiserver.exe（通过进程名）
- ✅ 等待文件释放
- ✅ 验证清理结果

**不负责**（由 AI 处理）:
- ❌ 删除 `test/autotest/` 目录

### 混合执行策略

```
┌─────────────────────────────────────────────────────────────┐
│                      测试流程                                │
├─────────────────────────────────────────────────────────────┤
│                                                               │
│  阶段 1: 预检查 (AI)                                          │
│    └─ 检查测试需求                                            │
│                                                               │
│  阶段 2: 环境准备 (脚本 ⭐)                                   │
│    ├─ 运行 setup-env.sh/ps1                                  │
│    └─ 编译 + 启动 TestAIServer                               │
│                                                               │
│  阶段 3: 本地初始化 (AI)                                      │
│    └─ nemesisbot.exe onboard default --local                 │
│                                                               │
│  阶段 4: 配置 AI (AI)                                         │
│    └─ nemesisbot.exe model add ...                           │
│                                                               │
│  阶段 5: 启动 Bot (AI)                                        │
│    └─ nemesisbot.exe gateway &                               │
│                                                               │
│  阶段 6: 执行测试 (AI)                                        │
│    ├─ 运行 websocket_chat_client.exe                          │
│    └─ 分析测试结果                                            │
│                                                               │
│  阶段 7: 清理环境 (混合 ⭐)                                   │
│    ├─ 运行 cleanup-env.sh/ps1 (停止服务)                      │
│    └─ AI 删除 test/autotest/ 目录                            │
│                                                               │
│  阶段 8: 结果分析 (AI)                                        │
│    └─ 生成报告，决定下一步                                    │
│                                                               │
└─────────────────────────────────────────────────────────────┘
```

**优势**:
- ✅ 脚本处理机械性工作（编译、启停服务）
- ✅ AI 处理决策性工作（配置、测试、分析）
- ✅ 减少工作目录管理问题
- ✅ 提高执行效率和可靠性

---

## 测试阶段

### 阶段 1: 预检查

```yaml
检查项:
  - UI 依赖检查:
      描述: 确认测试功能不需要 UI 交互
      方法: 检查功能描述，确认不涉及对话框、窗口等
      通过条件: 无 UI 依赖

  - AI 支持检查:
      描述: 确认功能需要 LLM 调用
      方法: 检查工具定义、Agent 处理逻辑
      通过条件: 需要 AI 处理

  - TestAIServer 能力检查:
      描述: 确认 TestAIServer 有支持的测试模型
      方法: 查看 test/TestAIServer/README.md
      可用模型:
        - testai-5.0: 文件操作测试
        - testai-3.0: 集群通信测试
        - testai-4.2/4.3: 工具调用测试（sleep）
        - testai-1.1: 基础响应测试
        - testai-2.0: 消息回显测试
      通过条件: 找到匹配的测试模型
```

---

### 阶段 2: 环境准备

**详细文档**: `phases/02-environment-prep.md`（⚠️ 作为参考，优先使用脚本）

**目标**:
- 验证起始目录（必须在项目根目录）
- 创建 `test/autotest/` 工作目录
- 编译所有测试工具到 `test/autotest/`
- 启动 TestAIServer
- **结束时工作目录在 `test/autotest/`**

---

#### ⭐ 推荐方式：使用辅助脚本（快速）

**Windows PowerShell**:
```powershell
# 从项目根目录执行
.\Skills\automated-testing\scripts\setup-env.ps1
```

**Linux/Git Bash**:
```bash
# 从项目根目录执行
bash Skills/automated-testing/scripts/setup-env.sh
```

**脚本功能**:
- ✅ 自动检查环境（Go、必要目录）
- ✅ 自动创建 test/autotest/ 目录
- ✅ 并行编译所有组件（testaiserver、nemesisbot、websocket_chat_client）
- ✅ 自动启动 TestAIServer（后台运行）
- ✅ 保存进程 PID 到 test/autotest/testaiserver.pid
- ✅ 健康检查（等待 TestAIServer 就绪）
- ✅ 输出可解析的状态信息

**脚本输出**:
```
SETUP_START
Creating test\autotest directory...
Compiling test components...
[1/3] Compiling TestAIServer...
[2/3] Compiling NemesisBot...
[3/3] Compiling WebSocket client...
Compilation successful
Starting TestAIServer...
Waiting for TestAIServer to be ready...
SETUP_SUCCESS
TESTAI_PID=7256
TESTAI_PORT=8080
WORK_DIR=C:\AI\NemesisBot\NemesisBot\test\autotest

Environment setup completed successfully!
TestAIServer is running with PID: 7256
TestAIServer endpoint: http://127.0.0.1:8080/v1
```

**手动方式（仅当脚本不可用时）**:
1. 验证在项目根目录（检查 go.mod）
2. 创建 `test/autotest/` 目录
3. 编译 TestAIServer: `cd test/TestAIServer && go build -o ../autotest/testaiserver.exe .`
4. 编译 NemesisBot: `go build -o test/autotest/nemesisbot.exe ./nemesisbot`（从根目录）
5. 编译 WebSocket 客户端: `go build -o test/autotest/websocket_chat_client.exe test/websocket_chat_client.go`
6. 启动 TestAIServer: `cd test/autotest && ./testaiserver.exe &`（使用后台运行）
7. 保存 PID: `tasklist | grep -i testaiserver.exe | head -1 | awk '{print $2}'`

**产物**:
- `test/autotest/testaiserver.exe`
- `test/autotest/nemesisbot.exe`
- `test/autotest/websocket_chat_client.exe`
- `test/autotest/testaiserver.pid`

---

### 阶段 3: 本地环境初始化

**详细文档**: `phases/03-local-init.md`

**目标**:
- 在 `test/autotest/` 中创建本地配置
- **工作目录保持在 `test/autotest/`**

**关键步骤**:
1. 验证在 `test/autotest/` 目录
2. 执行: `./nemesisbot.exe onboard default --local`
3. 创建 `test/autotest/.nemesisbot/` 配置目录

**产物**:
- `test/autotest/.nemesisbot/config.json`
- `test/autotest/.nemesisbot/IDENTITY.md`
- `test/autotest/.nemesisbot/SOUL.md`
- `test/autotest/.nemesisbot/USER.md`
- `test/autotest/.nemesisbot/workspace/`

---

### 阶段 4: 配置测试 AI

**详细文档**: `phases/04-06-execution.md`

**目标**:
- 添加测试 AI 模型配置
- **工作目录保持在 `test/autotest/`**

**关键步骤**:
1. 执行: `./nemesisbot.exe model add --model test/testai-X.X --base http://127.0.0.1:8080/v1 --key test-key --default`
2. 验证: `./nemesisbot.exe model list`

---

### 阶段 5: 启动 Bot

**详细文档**: `phases/04-06-execution.md`

**目标**:
- 启动 NemesisBot Gateway
- **工作目录保持在 `test/autotest/`**

**关键步骤**:
1. 启动: `./nemesisbot.exe gateway > nemesisbot.log 2>&1 &`（使用后台运行）
2. 保存 PID: `tasklist | grep -i nemesisbot.exe | head -1 | awk '{print $2}'`
3. 等待就绪（验证端口 49001）

**产物**:
- `test/autotest/nemesisbot.pid`
- `test/autotest/nemesisbot.log`

---

### 阶段 6: 执行测试

**详细文档**: `phases/04-06-execution.md`

**目标**:
- 运行测试场景
- 验证响应
- 记录结果
- **工作目录保持在 `test/autotest/`**

**关键步骤**:
1. 运行客户端: `./websocket_chat_client.exe`
2. 验证响应格式和内容
3. 记录测试结果

---

### 阶段 7: 清理环境

**详细文档**: `phases/07-08-cleanup-analysis.md`（⚠️ 停止服务优先使用脚本，结果分析详见文档）

**目标**:
- 停止所有进程
- 删除 `test/autotest/` 目录
- **返回项目根目录**

---

#### ⭐ 推荐方式：使用辅助脚本（停止进程）

**步骤 1: 停止服务（脚本）**

**Windows PowerShell**:
```powershell
# 从项目根目录执行
.\Skills\automated-testing\scripts\cleanup-env.ps1
```

**Linux/Git Bash**:
```bash
# 从项目根目录执行
bash Skills/automated-testing/scripts/cleanup-env.sh
```

**脚本功能**:
- ✅ 通过进程名停止 nemesisbot.exe
- ✅ 通过进程名停止 testaiserver.exe
- ✅ 读取并使用 PID 文件（如果存在）
- ✅ 等待文件释放
- ✅ 验证清理结果

**脚本输出**:
```
CLEANUP_START
Stopping NemesisBot...
Stopping TestAIServer...
Stopping TestAIServer (PID: 7256)...
Waiting for file handles to be released...
All processes stopped successfully
CLEANUP_SUCCESS

Environment cleanup completed!
Note: test\autotest\ directory was not removed (AI should handle this)
```

**步骤 2: 删除测试目录（AI 执行）**
```bash
# 返回项目根目录
cd /c/AI/NemesisBot/NemesisBot

# 删除测试目录
rm -rf test/autotest

# 验证
ls test/autotest  # 应该提示不存在
```

**手动方式（仅当脚本不可用时）**:
1. 从 `test/autotest/` 返回项目根目录
2. 停止 NemesisBot: `taskkill //F //IM nemesisbot.exe` (Windows)
3. 停止 TestAIServer: `taskkill //F //IM testaiserver.exe` (Windows)
4. 等待文件释放
5. 删除目录: `rm -rf test/autotest`
6. 验证清理结果

**清理内容**:
- 所有编译产物
- 所有配置文件
- 所有日志文件
- 所有会话数据
- **整个 `test/autotest/` 目录**

---

### 阶段 8: 结果分析和迭代

**详细文档**: `phases/07-08-cleanup-analysis.md`（结果分析和报告生成部分）

**目标**:
- 分析测试结果
- 生成测试报告
- **测试报告保存到 `docs/REPORT/`**（不在 `test/autotest/` 中）

**报告模板**: 见阶段 8 文档

---

## 快速参考

### 常用命令

```bash
# === 从项目根目录开始 ===

# 阶段 2: 环境准备
mkdir -p test/autotest

# 编译 TestAIServer（在其模块目录中编译）
cd test/TestAIServer && go build -o ../autotest/testaiserver.exe .
cd ../..

# 编译 NemesisBot（从根目录编译）
go build -o test/autotest/nemesisbot.exe ./nemesisbot

# 编译 WebSocket 客户端
go build -o test/autotest/websocket_chat_client.exe test/websocket_chat_client.go

# 启动 TestAIServer（使用后台运行）
cd test/autotest && ./testaiserver.exe &

# 保存 PID（Windows 使用 tasklist）
tasklist | grep -i testaiserver.exe | head -1 | awk '{print $2}' > testaiserver.pid

# 阶段 3: 本地初始化
./nemesisbot.exe onboard default --local

# 阶段 4: 配置模型
./nemesisbot.exe model add --model test/testai-1.1 --base http://127.0.0.1:8080/v1 --key test-key --default

# 阶段 5: 启动 Bot
./nemesisbot.exe gateway > nemesisbot.log 2>&1 &

# 保存 PID（Windows 使用 tasklist）
tasklist | grep -i nemesisbot.exe | head -1 | awk '{print $2}' > nemesisbot.pid

# 阶段 6: 执行测试
./websocket_chat_client.exe

# === 返回根目录清理 ===
cd ../..
taskkill //F //IM nemesisbot.exe
taskkill //F //IM testaiserver.exe
rm -rf test/autotest
```

### TestAIServer 模型快速参考

| 模型 | 用途 | 特殊标记 |
|------|------|----------|
| testai-1.1 | 基础响应测试 | 无 |
| testai-2.0 | 消息回显 | 无 |
| testai-3.0 | 集群通信 | `<PEER_CHAT>{}</PEER_CHAT>` |
| testai-4.2 | 工具调用(30s) | 返回 sleep 工具 |
| testai-4.3 | 工具调用(300s) | 返回 sleep 工具 |
| testai-5.0 | 文件操作 | `<FILE_OP>{}</FILE_OP>` |

---

## ⚠️ 注意事项

### 测试工作目录

**重要**：所有测试操作都在 `test/autotest/` 目录中进行。

**优点**:
- ✅ 规范的测试位置
- ✅ 简化清理（删除单个目录）
- ✅ 不污染项目根目录
- ✅ 源码位置不变

**工作目录流转**:
```
项目根目录
  ↓ (阶段 2)
test/autotest/
  ↓ (阶段 3-6，保持不变)
test/autotest/
  ↓ (阶段 7)
项目根目录 (清理)
```

### 测试报告保存

**⚠️ 重要**：测试报告必须保存到 `docs/REPORT/` 目录。

**原因**:
- `test/autotest/` 会在清理时被删除
- 测试报告需要保留
- 便于历史记录和追踪

**示例**:
```bash
# 正确
cd ../..  # 返回根目录
cat > "docs/REPORT/TEST_$(date +%Y%m%d).md" << EOF
...测试报告内容...
EOF

# 错误
cat > "test_report.md" << EOF  # 会被删除
EOF
```

### Windows 环境特殊处理

**进程管理**:
```bash
# Windows: 使用 taskkill
taskkill //F //IM nemesisbot.exe
taskkill //F //IM testaiserver.exe

# Unix/Linux: 使用 kill
kill $PID
```

**后台进程启动**:
```bash
# Windows Git Bash: 使用 run_in_background 参数（Claude Code Bash 工具）
cd test/autotest && ./testaiserver.exe &  # 使用 Bash 工具的 run_in_background: true

# 错误方式（不要使用）:
start /B testaiserver.exe > testaiserver.log 2>&1  # Git Bash 中重定向不生效
echo $! > testaiserver.pid  # $! 在 Windows bash 中不可用
```

**PID 获取**:
```bash
# Windows: 使用 tasklist 和 awk
tasklist | grep -i testaiserver.exe | head -1 | awk '{print $2}'

# 不要使用（Windows bash 不支持）:
echo $!  # Unix 变量，Windows 中不可用
```

---

## 故障排除

### 编译问题

#### 问题 1: TestAIServer 编译失败

**错误信息**:
```
main module (github.com/276793422/NemesisBot) does not contain package github.com/276793422/NemesisBot/test/TestAIServer
```

**原因**: TestAIServer 是独立的 Go 模块，不能从主模块直接编译

**解决方案**:
```bash
# ✅ 正确：在 TestAIServer 模块目录中编译
cd test/TestAIServer && go build -o ../autotest/testaiserver.exe .

# ❌ 错误：从 autotest 目录编译
cd test/autotest && go build -o testaiserver.exe ../TestAIServer
```

---

#### 问题 2: 单文件编译失败

**错误信息**:
```
package testaiserver/handlers is not in std
no required module provides package github.com/gin-gonic/gin
```

**原因**: 单文件编译无法解析本地包导入

**解决方案**:
```bash
# ✅ 正确：使用模块路径编译整个模块
cd test/TestAIServer && go build -o ../autotest/testaiserver.exe .

# ❌ 错误：只编译 main.go
cd test/autotest && go build -o testaiserver.exe ../TestAIServer/main.go
```

---

#### 问题 3: NemesisBot 编译路径错误

**错误信息**:
```
stat C:\AI\NemesisBot\NemesisBot\test\TestAIServer\nemesisbot: directory not found
```

**原因**: 工作目录不在项目根目录，相对路径错误

**解决方案**:
```bash
# ✅ 正确：先返回根目录，使用正确的相对路径
cd /path/to/project/root
go build -o test/autotest/nemesisbot.exe ./nemesisbot

# ❌ 错误：从 TestAIServer 目录使用错误的相对路径
cd test/TestAIServer && go build -o test/autotest/nemesisbot.exe ./nemesisbot
```

---

### 进程管理问题

#### 问题 4: 后台进程启动失败

**错误信息**:
```
Exit code 1
```

**原因**: Git Bash 中 `start /B` 和重定向 `>` 不兼容

**解决方案**:
```bash
# ✅ 正确：使用 Bash 工具的 run_in_background 参数
# 在 Claude Code 中：
# Bash tool: command="./testaiserver.exe &", run_in_background=true

# ❌ 错误：使用 Windows start 命令
start /B testaiserver.exe > testaiserver.log 2>&1
```

---

#### 问题 5: PID 变量不可用

**错误信息**: 变量为空或无效

**原因**: `$!` 是 Unix/Linux 特有变量，Windows bash 中不可用

**解决方案**:
```bash
# ✅ 正确（Windows）：使用 tasklist 查找 PID
tasklist | grep -i nemesisbot.exe | head -1 | awk '{print $2}'

# ✅ 正确（Unix/Linux）：
echo $!

# ❌ 错误（Windows）：使用 Unix 变量
echo $! > nemesisbot.pid
```

---

### 常见错误速查表

| 错误类型 | 症状 | 解决方案 |
|---------|------|---------|
| **跨模块编译** | `does not contain package` | 在目标模块目录中使用 `go build .` |
| **本地包解析失败** | `package is not in std` | 使用模块路径 `.` 编译整个模块 |
| **相对路径错误** | `directory not found` | 确保在正确的目录，使用正确的相对路径 |
| **后台进程失败** | `Exit code 1` | 使用 `run_in_background: true` |
| **PID 获取失败** | 变量为空 | 使用 `tasklist \| grep \| awk` |
| **端口占用** | `address already in use` | 检查并停止占用端口的进程 |
| **配置未找到** | `no such file or directory` | 确保先执行 `onboard default --local` |

---

### 调试技巧

1. **检查工作目录**: 每次操作前使用 `pwd` 确认当前位置
2. **验证文件存在**: 使用 `ls` 或 `ls -la` 确认文件/目录存在
3. **检查进程状态**: 使用 `tasklist \| grep` 确认进程是否运行
4. **查看日志**: 检查 `.nemesisbot/` 和 `test/autotest/` 中的日志文件
5. **测试端口连接**: 使用 `curl http://127.0.0.1:8080/v1/models` 测试 TestAIServer
6. **验证 WebSocket**: 使用浏览器开发者工具或专门的 WebSocket 客户端测试连接

---

### 通用注意事项

1. **工作目录管理**: 每次操作前使用 `pwd` 确认当前位置，特别是在编译阶段
2. **多模块编译**: TestAIServer 是独立模块，必须在自身目录使用 `go build .` 编译
3. **进程管理**: 确保在测试结束后清理所有后台进程
4. **环境隔离**: 所有测试内容在 `test/autotest/` 中
5. **端口冲突**: 确保 8080 和 49001 端口未被占用
6. **日志备份**: 测试日志在 `test/autotest/` 中，清理前如需保留请备份
7. **错误处理**: 每个阶段都应有错误检查和处理
8. **超时设置**: WebSocket 消息响应超时设为 30 秒
9. **Windows 平台**: 使用 `tasklist` + `grep` + `awk` 获取 PID，不要使用 `$!`

---

## 完整流程总结

```
预检查 → 环境准备 → 本地初始化 → 配置AI → 启动Bot → 执行测试 → 清理环境 → 结果分析
   ↓         ↓          ↓          ↓        ↓       ↓        ↓         ↓         ↓
  通过?    创建目录    配置创建   模型配置   进程运行  WebSocket  删除目录   记录报告
           编译工具    结构完整             连接成功            验证清理   保存到
           启动AI                                          返回根目录  docs/REPORT/

关键改进:
  - 规范测试工作目录到 test/autotest/
  - 源码位置不变，编译产物集中管理
  - 简化清理为删除单个目录
  - 测试报告隔离保存
  - 多模块编译：TestAIServer 在自身目录编译
  - Windows 后台进程：使用 run_in_background 参数
  - PID 获取：Windows 使用 tasklist + grep + awk
```

---

### 正确的编译和启动命令序列

```bash
# === 项目根目录 ===

# 1. 创建测试工作目录
mkdir -p test/autotest

# 2. 编译 TestAIServer（必须在自身模块目录）
cd test/TestAIServer
go build -o ../autotest/testaiserver.exe .
cd ../..

# 3. 编译 NemesisBot（从根目录）
go build -o test/autotest/nemesisbot.exe ./nemesisbot

# 4. 编译 WebSocket 客户端
go build -o test/autotest/websocket_chat_client.exe test/websocket_chat_client.go

# 5. 进入测试目录
cd test/autotest

# 6. 启动 TestAIServer（后台运行）
./testaiserver.exe &  # 使用 Claude Code Bash 工具的 run_in_background: true

# 7. 保存 TestAIServer PID
tasklist | grep -i testaiserver.exe | head -1 | awk '{print $2}' > testaiserver.pid

# 8. 初始化本地配置
./nemesisbot.exe onboard default --local

# 9. 配置测试模型
./nemesisbot.exe model add --model test/testai-1.1 --base http://127.0.0.1:8080/v1 --key test-key --default

# 10. 启动 NemesisBot Gateway（后台运行）
./nemesisbot.exe gateway > nemesisbot.log 2>&1 &  # 使用 run_in_background: true

# 11. 保存 NemesisBot PID
tasklist | grep -i nemesisbot.exe | head -1 | awk '{print $2}' > nemesisbot.pid

# 12. 执行测试
./websocket_chat_client.exe

# === 清理（返回根目录）===
cd ../..
taskkill //F //IM nemesisbot.exe
taskkill //F //IM testaiserver.exe
rm -rf test/autotest
```

---

**最后更新**: 2026-03-23
**更新内容**:
- 修正阶段2编译命令（TestAIServer 必须在其模块目录编译）
- 添加 Windows 后台进程启动方法（使用 run_in_background）
- 更新 PID 获取方法（Windows 使用 tasklist）
- 添加故障排除章节，记录常见错误和解决方案
