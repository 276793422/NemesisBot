# 阶段 4-6: 配置测试 AI、启动 Bot、执行测试

本阶段完成测试 AI 配置、Bot 启动和测试执行。

**⚠️ 重要前提**：
- 必须在 `test/autotest/` 目录中执行
- 阶段 2 已完成编译
- 阶段 3 已完成本地环境初始化
- TestAIServer 正在运行

**当前工作目录**: `test/autotest/`

---

## 阶段 4: 配置测试 AI

### 4.1 添加测试模型

**目的**: 配置测试 AI 模型

**当前工作目录**: `test/autotest/`

**命令**:
```bash
# 验证当前目录
echo "=== 阶段 4: 配置测试 AI ==="
echo "📍 当前工作目录: $(pwd)"
echo ""

# 添加测试模型（根据测试需求选择）
echo "添加测试模型..."

# 基础响应测试
./nemesisbot.exe model add \
  --model test/testai-1.1 \
  --base http://127.0.0.1:8080/v1 \
  --key test-key \
  --default

# 预期输出:
# ✓ Model 'testai-1.1' added successfully!
# ✓ Set as default LLM: testai-1.1

echo ""
echo "✅ 测试模型已添加"
echo ""
```

**预期输出**:
```
=== 阶段 4: 配置测试 AI ===
📍 当前工作目录: /c/AI/NemesisBot/NemesisBot/test/autotest

添加测试模型...
✓ Model 'testai-1.1' added successfully!
✓ Set as default LLM: testai-1.1

✅ 测试模型已添加
```

---

### 4.2 验证模型配置

**目的**: 确认模型配置正确

**当前工作目录**: `test/autotest/`

**命令**:
```bash
echo "=== 验证模型配置 ==="

./nemesisbot.exe model list

echo ""
echo "✅ 模型配置验证完成"
echo ""
```

**预期输出**:
```
=== 验证模型配置 ===
Configured Models:
==================

testai-1.1 (default)
  Model: test/testai-1.1
  Base URL: http://127.0.0.1:8080/v1
  API Key: test-key
  Default: yes

✅ 模型配置验证完成
```

---

## 阶段 5: 启动 Bot

### 5.1 启动 Bot 进程

**目的**: 启动 NemesisBot Gateway

**当前工作目录**: `test/autotest/`

**⚠️ 重要**:
- 使用 `gateway` 模式，不是 `agent` 模式
- `agent` 模式会因为 stdin 关闭而退出
- `gateway` 模式会持续运行并监听 WebSocket 连接

**命令**:
```bash
echo "=== 阶段 5: 启动 Bot ==="
echo "📍 当前工作目录: $(pwd)"
echo ""

# 启动 Bot (后台)
# 使用 gateway 模式，日志输出到当前目录
./nemesisbot.exe gateway > nemesisbot.log 2>&1 &

# 等待进程启动
sleep 2

# **⚠️ 重要**: 必须保存 PID 到文件
# PID 文件保存在当前工作目录 (test/autotest/)
# **Windows 和 Unix/Linux 的 PID 获取方式不同**

if [[ "$OSTYPE" == "msys" || "$OSTYPE" == "win32" ]]; then
  # Windows: 使用 tasklist 获取 PID
  BOT_PID=$(tasklist | grep -i "nemesisbot.exe" | awk '{print $2}' | head -1)

  if [ -z "$BOT_PID" ]; then
    echo "❌ 无法获取 NemesisBot PID"
    echo "   请检查进程是否已启动"
    exit 1
  fi

  echo $BOT_PID > nemesisbot.pid
else
  # Unix/Linux: 使用 $!
  BOT_PID=$!
  echo $BOT_PID > nemesisbot.pid
fi

# 验证 PID 文件已创建
if [ ! -f "nemesisbot.pid" ]; then
  echo "❌ PID 文件创建失败"
  exit 1
fi

echo "Bot PID: $BOT_PID"
echo "✅ PID 已保存到 nemesisbot.pid"
echo ""
```

**预期输出**:
```
=== 阶段 5: 启动 Bot ===
📍 当前工作目录: /c/AI/NemesisBot/NemesisBot/test/autotest

Bot PID: 12346
✅ PID 已保存到 nemesisbot.pid
```

---

### 5.2 等待 Bot 启动

**目的**: 等待 Bot 完全启动

**当前工作目录**: `test/autotest/`

**命令**:
```bash
echo "等待 Bot 启动..."
sleep 3

# 验证进程
if [[ "$OSTYPE" == "msys" || "$OSTYPE" == "win32" ]]; then
  # Windows: 使用 tasklist
  if tasklist | grep -i "nemesisbot.exe" > /dev/null 2>&1; then
    REAL_PID=$(tasklist | grep -i "nemesisbot.exe" | awk '{print $2}' | head -1)
    echo "✅ Bot 进程已启动 (PID: $REAL_PID)"
  else
    echo "❌ Bot 进程未运行"
    exit 1
  fi
else
  # Unix/Linux: 使用 ps
  ps -p $BOT_PID > /dev/null
  if [ $? -ne 0 ]; then
    echo "❌ Bot 进程未运行"
    exit 1
  fi

  echo "✅ Bot 进程已启动 (PID: $BOT_PID)"
fi

echo ""
```

**预期输出**:
```
等待 Bot 启动...
✅ Bot 进程已启动 (PID: 12346)
```

---

### 5.3 验证 WebSocket 端口

**目的**: 确认 Bot 的 WebSocket 端口已就绪

**当前工作目录**: `test/autotest/`

**命令**:
```bash
echo "检查 WebSocket 端口..."

if [[ "$OSTYPE" == "msys" || "$OSTYPE" == "win32" ]]; then
  # Windows
  if netstat -an | grep ":49001 " | grep "LISTENING" > /dev/null; then
    echo "✅ WebSocket 端口 49001 已就绪"
  else
    echo "⚠️  WebSocket 端口尚未就绪，等待..."
    sleep 2
    if netstat -an | grep ":49001 " | grep "LISTENING" > /dev/null; then
      echo "✅ WebSocket 端口 49001 已就绪"
    else
      echo "❌ WebSocket 端口未能就绪"
      echo "提示: 查看 nemesisbot.log 了解详情"
      exit 1
    fi
  fi
else
  # Unix/Linux
  if netstat -an 2>/dev/null | grep ":49001 " > /dev/null || \
     ss -ln 2>/dev/null | grep ":49001 " > /dev/null; then
    echo "✅ WebSocket 端口 49001 已就绪"
  else
    echo "⚠️  WebSocket 端口尚未就绪，等待..."
    sleep 2
  fi
fi

echo ""
echo "📍 当前工作目录: $(pwd)"
echo ""
```

**预期输出**:
```
检查 WebSocket 端口...
✅ WebSocket 端口 49001 已就绪

📍 当前工作目录: /c/AI/NemesisBot/NemesisBot/test/autotest
```

---

## 阶段 6: 执行测试

### 6.1 测试执行框架

**目的**: 执行具体的测试场景

**当前工作目录**: `test/autotest/`

**前置条件**:
- ✅ TestAIServer 运行中（PID 在 `testaiserver.pid`）
- ✅ Bot 运行中（PID 在 `nemesisbot.pid`）
- ✅ 测试模型已配置
- ✅ WebSocket 端口已就绪

**测试步骤**:
1. 连接 WebSocket（ws://127.0.0.1:49001/ws）
2. 发送测试消息
3. 接收响应（超时 30 秒）
4. 验证响应内容
5. 记录结果

---

### 6.2 示例：基本聊天测试

**目的**: 测试基本的消息收发功能

**当前工作目录**: `test/autotest/`

**命令**:
```bash
echo "=== 阶段 6: 执行测试 ==="
echo "📍 当前工作目录: $(pwd)"
echo ""

# 运行 WebSocket 测试客户端
./websocket_chat_client.exe

echo ""
echo "✅ 测试执行完成"
echo ""
```

**预期输出**:
```
=== 阶段 6: 执行测试 ===
📍 当前工作目录: /c/AI/NemesisBot/NemesisBot/test/autotest

连接到 ws://127.0.0.1:49001/ws...
✅ 已连接

📤 发送消息: 你好，请简单介绍一下自己
✅ 消息已发送

⏳ 等待响应...

📥 收到第 1 条消息:
   类型: message
   角色: system
   内容: Connected to NemesisBot WebSocket channel. Client ID: client_1774239845

📥 收到第 2 条消息:
   类型: message
   角色: assistant
   内容: 好的，我知道了

============================================================
✅ 测试通过：消息收发功能正常
✅ 收到 AI 响应（21 字符）
============================================================

✅ 测试执行完成
```

---

### 6.3 自定义测试场景

**目的**: 根据需要创建不同的测试场景

**当前工作目录**: `test/autotest/`

**测试场景示例**:

#### 场景 1: 文件读取操作测试

```yaml
名称: 文件读取操作
模型: testai-5.0

输入:
  content: '<FILE_OP>{"operation":"file_read","path":"test.txt"}</FILE_OP>'

预期:
  type: "message"
  role: "assistant"
  content_contains: "file_read" 或 "test.txt"

验证:
  - 响应时间 < 10 秒
  - 无错误消息
  - 工具调用正确
```

#### 场景 2: 基本聊天测试

```yaml
名称: 基本聊天
模型: testai-1.1

输入:
  content: "你好"

预期:
  type: "message"
  role: "assistant"
  content: 任意响应

验证:
  - 响应时间 < 30 秒
  - 无错误消息
```

#### 场景 3: 工具调用测试

```yaml
名称: 工具调用
模型: testai-4.2

输入:
  content: "请等待 3 秒"

预期:
  type: "message"
  role: "assistant"
  content: 包含工具调用结果

验证:
  - 响应时间 > 3 秒（sleep 工具执行时间）
  - 无错误消息
```

---

### 6.4 查看测试日志

**目的**: 检查 Bot 运行日志

**当前工作目录**: `test/autotest/`

**命令**:
```bash
# 查看 Bot 日志
echo "=== Bot 日志 ==="
if [ -f "./nemesisbot.log" ]; then
  tail -50 nemesisbot.log
else
  echo "⚠️  日志文件不存在"
fi
echo ""

# 检查请求日志
echo "=== 请求日志 ==="
if [ -d "./.nemesisbot/workspace/logs/request_logs" ]; then
  # 列出最近的请求日志
  ls -lt ./.nemesisbot/workspace/logs/request_logs/ | head -5
else
  echo "⚠️  请求日志目录不存在"
fi
echo ""
```

---

## 目录结构说明

**本阶段完成后的目录结构**:
```
test/autotest/              # 当前工作目录
├── testaiserver.exe         # 已编译
├── testaiserver.pid         # TestAIServer 进程 ID
├── nemesisbot.exe           # 已编译
├── nemesisbot.pid           # Bot 进程 ID
├── nemesisbot.log           # Bot 日志（运行时生成）
├── websocket_chat_client.exe  # 已编译
└── .nemesisbot/             # Bot 配置和运行时数据
    ├── config.json
    ├── IDENTITY.md
    ├── SOUL.md
    ├── USER.md
    └── workspace/
        ├── agents/
        ├── cluster/
        ├── logs/
        │   ├── request_logs/   # 请求日志（运行时生成）
        │   │   └── 2026-03-23_XX-XX-XX_XXXXXX/
        │   │       ├── 01.request.md
        │   │       ├── 02.AI.Request.md
        │   │       ├── 03.AI.Response.md
        │   │       └── 04.response.md
        │   └── security_logs/
        ├── sessions/
        │   └── agent_main_main.json  # 会话数据（运行时生成）
        └── state/
            └── state.json  # 状态数据（运行时生成）
```

**⚠️ 重要**：
- 所有运行时生成的文件都在 `test/autotest/` 中
- 包括日志、会话、状态等
- 测试报告应保存到 `docs/REPORT/`（不在 `test/autotest/` 中）

---

## 测试报告

**⚠️ 重要**：测试报告必须保存到 `docs/REPORT/` 目录，不要保存在 `test/autotest/` 中，避免被清理时删除。

**生成测试报告**:
```bash
# 返回项目根目录
cd ../..

# 创建报告目录
mkdir -p docs/REPORT

# 生成测试报告（根据实际测试结果）
REPORT_FILE="docs/REPORT/TEST_$(date +%Y%m%d_%H%M%S).md"

cat > "$REPORT_FILE" << 'EOF'
# 测试报告

**测试日期**: $(date +%Y-%m-%d)
**测试时间**: $(date +%H:%M:%S)
**测试版本**: $(git rev-parse --short HEAD 2>/dev/null || echo "unknown")

---

## 测试目标

[描述测试的具体目标]

---

## 测试环境

- **操作系统**: Windows 11
- **TestAIServer**: testai-1.1
- **NemesisBot**: [版本]
- **测试工作目录**: test/autotest/

---

## 测试场景

### 场景 1: 基本聊天

**输入**: "你好"

**预期**: 收到 AI 响应

**实际**: [记录实际响应]

**结果**: ✅ 通过 / ❌ 失败

---

## 测试结果

| 场景 | 结果 | 响应时间 | 备注 |
|------|------|----------|------|
| 基本聊天 | ✅ | 2.3s | - |

**通过率**: 100%

---

## 结论

[总体评价]

EOF

echo "✅ 测试报告已保存: $REPORT_FILE"
```

---

## 故障排查

### 问题 1: 模型配置失败

**症状**: `model add` 返回错误

**可能原因**:
- TestAIServer 未运行
- 网络连接问题

**解决方案**:
```bash
# 检查 TestAIServer
curl http://127.0.0.1:8080/v1/models

# 检查 TestAIServer 进程
tasklist | grep testaiserver  # Windows
ps aux | grep testaiserver    # Unix/Linux
```

---

### 问题 2: Bot 启动失败

**症状**: `gateway` 命令后进程退出

**解决方案**:
```bash
# 查看日志
cat nemesisbot.log

# 检查配置
cat .nemesisbot/config.json

# 尝试前台运行（查看错误）
./nemesisbot.exe gateway
```

---

### 问题 3: WebSocket 连接失败

**症状**: 客户端无法连接

**解决方案**:
```bash
# 检查端口
netstat -an | grep 49001

# 检查 Bot 进程
tasklist | grep nemesisbot  # Windows
ps aux | grep nemesisbot    # Unix/Linux

# 检查日志
tail -100 nemesisbot.log
```

---

## 检查点

**测试执行完成检查点**:

- [ ] **当前工作目录在 `test/autotest/`**
- [ ] Bot 进程已启动（gateway 模式）
- [ ] **Bot PID 已保存到 `test/autotest/nemesisbot.pid`** ⚠️ **必须**
- [ ] WebSocket 连接成功
- [ ] 测试消息已发送
- [ ] 收到 Bot 响应
- [ ] 响应格式正确
- [ ] 响应内容符合预期
- [ ] 响应时间在可接受范围
- [ ] 测试结果已记录
- [ ] **测试报告已保存到 `docs/REPORT/`** ⚠️ **不在 test/autotest/**

**状态**: ✅ 通过 / ❌ 失败

---

**下一步**: 阶段 7 - 清理环境（返回项目根目录执行）
