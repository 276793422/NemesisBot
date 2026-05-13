# 阶段 2: 环境准备

> **⚠️ 推荐方式**: 使用辅助脚本（2026-03-23 更新）
>
> 本阶段的大部分步骤已由 `setup-env.sh/ps1` 脚本自动化。
>
> **强烈建议优先使用脚本**：
> ```bash
> # Windows PowerShell
> .\Skills\automated-testing\scripts\setup-env.ps1
>
> # Linux/Git Bash
> bash Skills/automated-testing/scripts/setup-env.sh
> ```
>
> **本文档的作用**：
> - ✅ 理解脚本做了什么
> - ✅ 脚本失败时的备用方案
> - ✅ 故障排查参考
> - ✅ 学习底层逻辑
>
> **如果脚本执行成功，可以直接跳到阶段 3**
>
> ---

准备测试所需的所有环境和组件。

**⚠️ 重要前提**：此阶段必须在**项目根目录**下执行。

**项目根目录特征**：
- 包含 `go.mod` 文件
- 包含 `nemesisbot/` 目录
- 包含 `test/` 目录

**阶段目标**：
- 创建测试工作目录 `test/autotest/`
- 编译所有测试工具到 `test/autotest/`
- 启动 TestAIServer
- **本阶段结束时，工作目录将切换到 `test/autotest/`**

---

## 步骤

### 2.0 验证起始目录

**目的**: 确保从项目根目录开始执行

**当前工作目录**: 项目根目录（例如：`C:/AI/NemesisBot/NemesisBot`）

**命令**:
```bash
# 验证当前目录是否为项目根目录
echo "=== 验证起始目录 ==="

if [ ! -f "go.mod" ]; then
  echo "❌ 错误: 当前目录不是项目根目录"
  echo "   请在包含 go.mod 的目录中执行此流程"
  exit 1
fi

if [ ! -d "nemesisbot" ]; then
  echo "❌ 错误: 找不到 nemesisbot/ 目录"
  exit 1
fi

if [ ! -d "test" ]; then
  echo "❌ 错误: 找不到 test/ 目录"
  exit 1
fi

echo "✅ 当前目录验证通过: $(pwd)"
echo ""
```

**预期输出**:
```
=== 验证起始目录 ===
✅ 当前目录验证通过: /c/AI/NemesisBot/NemesisBot
```

---

### 2.1 创建测试工作目录

**目的**: 创建 `test/autotest/` 作为测试工作目录

**当前工作目录**: 项目根目录

**命令**:
```bash
# 创建测试工作目录
echo "=== 创建测试工作目录 ==="

mkdir -p test/autotest

if [ ! -d "test/autotest" ]; then
  echo "❌ 测试工作目录创建失败"
  exit 1
fi

echo "✅ 测试工作目录已创建: test/autotest/"
echo ""

# 切换到测试工作目录
cd test/autotest
echo "📍 当前工作目录: $(pwd)"
echo ""
```

**预期输出**:
```
=== 创建测试工作目录 ===
✅ 测试工作目录已创建: test/autotest/

📍 当前工作目录: /c/AI/NemesisBot/NemesisBot/test/autotest
```

**⚠️ 重要**: 从此步开始，所有命令都在 `test/autotest/` 目录中执行

---

### 2.2 编译 TestAIServer

**目的**: 编译测试用 AI 服务器

**当前工作目录**: `test/autotest/`

**源码位置**: `../TestAIServer/`（相对于 `test/autotest/`）

**编译产物**: `test/autotest/testaiserver.exe`

**⚠️ 重要**: TestAIServer 是一个独立的 Go 模块，必须在**其模块目录**中编译

**命令**:
```bash
# 编译 TestAIServer
echo "=== 编译 TestAIServer ==="

# **重要**: TestAIServer 是独立模块，必须在其目录中编译
# 从项目根目录进入 TestAIServer 目录编译
cd ../TestAIServer && go build -o ../autotest/testaiserver.exe . && cd ../autotest

# 验证编译结果
if [ ! -f "testaiserver.exe" ]; then
  echo "❌ TestAIServer 编译失败"
  exit 1
fi

echo "✅ TestAIServer 编译成功"
ls -lh testaiserver.exe
echo ""
```

**预期输出**:
```
=== 编译 TestAIServer ===
✅ TestAIServer 编译成功
-rwxr-xr-x 1 user group 12M Mar 23 10:30 testaiserver.exe
```

---

### 2.3 编译 NemesisBot

**目的**: 编译主程序

**当前工作目录**: `test/autotest/`

**源码位置**: `../../nemesisbot/`（相对于 `test/autotest/`）

**编译产物**: `test/autotest/nemesisbot.exe`

**命令**:
```bash
# 编译 NemesisBot
echo "=== 编译 NemesisBot ==="

# 从源码位置编译
# ../../nemesisbot 是相对于 test/autotest/ 的路径
go build -o nemesisbot.exe ../../nemesisbot

# 验证编译结果
if [ ! -f "nemesisbot.exe" ]; then
  echo "❌ NemesisBot 编译失败"
  exit 1
fi

echo "✅ NemesisBot 编译成功"
ls -lh nemesisbot.exe
echo ""
```

**预期输出**:
```
=== 编译 NemesisBot ===
✅ NemesisBot 编译成功
-rwxr-xr-x 1 user group 15M Mar 23 10:32 nemesisbot.exe
```

---

### 2.4 编译 WebSocket 测试客户端

**目的**: 编译 WebSocket 测试工具

**当前工作目录**: `test/autotest/`

**源码位置**: `../websocket_chat_client.go`（相对于 `test/autotest/`）

**编译产物**: `test/autotest/websocket_chat_client.exe`

**命令**:
```bash
# 编译 WebSocket 测试客户端
echo "=== 编译 WebSocket 测试客户端 ==="

# 从源码位置编译
# ../websocket_chat_client.go 是相对于 test/autotest/ 的路径
go build -o websocket_chat_client.exe ../websocket_chat_client.go

# 验证编译结果
if [ ! -f "websocket_chat_client.exe" ]; then
  echo "❌ WebSocket 客户端编译失败"
  exit 1
fi

echo "✅ WebSocket 客户端编译成功"
ls -lh websocket_chat_client.exe
echo ""
```

**预期输出**:
```
=== 编译 WebSocket 测试客户端 ===
✅ WebSocket 客户端编译成功
-rwxr-xr-x 1 user group 1.2M Mar 23 10:33 websocket_chat_client.exe
```

---

### 2.5 启动 TestAIServer

**目的**: 在后台启动测试 AI 服务器

**当前工作目录**: `test/autotest/`

**可执行文件**: `./testaiserver.exe`

**PID 文件**: `testaiserver.pid`（保存在 `test/autotest/`）

**命令**:
```bash
# 后台启动 TestAIServer
echo "=== 启动 TestAIServer ==="

./testaiserver.exe &

# 等待进程启动
sleep 2

# **⚠️ 重要**: 必须保存 PID 到文件，用于后续清理
# **Windows 和 Unix/Linux 的 PID 获取方式不同**

if [[ "$OSTYPE" == "msys" || "$OSTYPE" == "win32" ]]; then
  # Windows: 使用 tasklist 获取 PID
  TESTAI_PID=$(tasklist | grep -i "testaiserver.exe" | awk '{print $2}' | head -1)

  if [ -z "$TESTAI_PID" ]; then
    echo "❌ 无法获取 TestAIServer PID"
    echo "   请检查进程是否已启动"
    exit 1
  fi

  echo $TESTAI_PID > testaiserver.pid
else
  # Unix/Linux: 使用 $!
  TESTAI_PID=$!
  echo $TESTAI_PID > testaiserver.pid
fi

# 验证 PID 文件已创建
if [ ! -f "testaiserver.pid" ]; then
  echo "❌ PID 文件创建失败"
  exit 1
fi

echo "TestAIServer PID: $TESTAI_PID"
echo "✅ PID 已保存到 testaiserver.pid"

# 验证进程
if [[ "$OSTYPE" == "msys" || "$OSTYPE" == "win32" ]]; then
  # Windows: 验证进程存在
  if tasklist | grep -i "testaiserver.exe" > /dev/null 2>&1; then
    echo "✅ TestAIServer 进程已启动"
  else
    echo "❌ TestAIServer 进程未运行"
    exit 1
  fi
else
  # Unix/Linux: 验证进程存在
  ps -p $TESTAI_PID > /dev/null 2>&1
  if [ $? -ne 0 ]; then
    echo "❌ TestAIServer 进程未运行"
    exit 1
  fi

  echo "✅ TestAIServer 进程已启动"
fi

echo ""
```

**预期输出**:
```
=== 启动 TestAIServer ===
TestAIServer PID: 12345
✅ PID 已保存到 testaiserver.pid
✅ TestAIServer 进程已启动
```

---

### 2.6 等待 TestAIServer 就绪

**目的**: 等待服务器完全启动并可以接受请求

**当前工作目录**: `test/autotest/`

**命令**:
```bash
# 等待 2 秒让服务器启动
echo "=== 等待 TestAIServer 就绪 ==="
sleep 2

# 测试服务器是否就绪
for i in {1..10}; do
  if curl -s http://127.0.0.1:8080/v1/models > /dev/null 2>&1; then
    echo "✅ TestAIServer 已就绪"
    break
  fi

  if [ $i -eq 10 ]; then
    echo "❌ TestAIServer 启动超时"
    exit 1
  fi

  echo "等待 TestAIServer 就绪... ($i/10)"
  sleep 1
done
echo ""
```

**预期输出**:
```
=== 等待 TestAIServer 就绪 ===
✅ TestAIServer 已就绪
```

**验证测试**:
```bash
# 测试模型列表端点
curl http://127.0.0.1:8080/v1/models
```

**预期响应**:
```json
{
  "object": "list",
  "data": [
    {
      "id": "testai-1.1",
      "object": "model"
    },
    {
      "id": "testai-5.0",
      "object": "model"
    }
  ]
}
```

---

### 2.7 环境验证

**目的**: 验证所有组件都已正确准备

**当前工作目录**: `test/autotest/`

**命令**:
```bash
echo "=== 环境验证 ==="
echo "📍 当前工作目录: $(pwd)"
echo ""

# **⚠️ Windows 特殊处理**: 检测操作系统
if [[ "$OSTYPE" == "msys" || "$OSTYPE" == "win32" ]]; then
  echo "检测到 Windows 环境"
else
  echo "检测到 Unix/Linux 环境"
fi
echo ""

# 1. 检查 TestAIServer 进程
echo "[1/5] TestAIServer 进程..."
if [[ "$OSTYPE" == "msys" || "$OSTYPE" == "win32" ]]; then
  # Windows: 使用 tasklist
  if tasklist | grep -i "testaiserver.exe" > /dev/null 2>&1; then
    REAL_PID=$(tasklist | grep -i "testaiserver.exe" | awk '{print $2}' | head -1)
    echo "✅ TestAIServer 进程运行中 (PID: $REAL_PID)"
  else
    echo "❌ TestAIServer 进程未运行"
    exit 1
  fi
else
  # Unix/Linux: 使用 ps
  if ps -p $TESTAI_PID > /dev/null; then
    echo "✅ TestAIServer 进程运行中 (PID: $TESTAI_PID)"
  else
    echo "❌ TestAIServer 进程未运行"
    exit 1
  fi
fi

# 2. 检查 TestAIServer 端口
echo "[2/5] TestAIServer 端口..."
if [[ "$OSTYPE" == "msys" || "$OSTYPE" == "win32" ]]; then
  # Windows: 使用 netstat
  if netstat -an | grep ":8080 " | grep "LISTENING" > /dev/null; then
    echo "✅ 端口 8080 已监听"
  else
    echo "❌ 端口 8080 未监听"
    exit 1
  fi
else
  # Unix/Linux
  if netstat -an 2>/dev/null | grep ":8080 " > /dev/null || \
     ss -ln 2>/dev/null | grep ":8080 " > /dev/null; then
    echo "✅ 端口 8080 已监听"
  else
    echo "❌ 端口 8080 未监听"
    exit 1
  fi
fi

# 3. 检查 TestAIServer API
echo "[3/5] TestAIServer API..."
if curl -s http://127.0.0.1:8080/v1/models > /dev/null; then
  echo "✅ TestAIServer API 可访问"
else
  echo "❌ TestAIServer API 不可访问"
  exit 1
fi

# 4. 检查 NemesisBot 可执行文件
echo "[4/5] NemesisBot 可执行文件..."
if [ -f "./nemesisbot.exe" ]; then
  echo "✅ NemesisBot 可执行文件存在"
else
  echo "❌ NemesisBot 可执行文件不存在"
  exit 1
fi

# 5. 检查 PID 文件
echo "[5/5] PID 文件..."
if [ -f "./testaiserver.pid" ]; then
  echo "✅ TestAIServer PID 文件存在"
else
  echo "❌ TestAIServer PID 文件不存在"
  exit 1
fi

echo ""
echo "=== 环境验证完成，所有检查通过 ==="
echo ""
echo "📍 当前工作目录: $(pwd)"
echo "📁 测试工具位置: $(pwd)"
echo "   ├── testaiserver.exe"
echo "   ├── nemesisbot.exe"
echo "   ├── websocket_chat_client.exe"
echo "   └── testaiserver.pid"
echo ""
```

**预期输出**:
```
=== 环境验证 ===
📍 当前工作目录: /c/AI/NemesisBot/NemesisBot/test/autotest

检测到 Windows 环境

[1/5] TestAIServer 进程...
✅ TestAIServer 进程运行中 (PID: 12345)
[2/5] TestAIServer 端口...
✅ 端口 8080 已监听
[3/5] TestAIServer API...
✅ TestAIServer API 可访问
[4/5] NemesisBot 可执行文件...
✅ NemesisBot 可执行文件存在
[5/5] PID 文件...
✅ TestAIServer PID 文件存在

=== 环境验证完成，所有检查通过 ===

📍 当前工作目录: /c/AI/NemesisBot/NemesisBot/test/autotest
📁 测试工具位置: /c/AI/NemesisBot/NemesisBot/test/autotest
   ├── testaiserver.exe
   ├── nemesisbot.exe
   ├── websocket_chat_client.exe
   └── testaiserver.pid
```

---

## 目录结构说明

**本阶段完成后的目录结构**:
```
项目根目录/
├── nemesisbot/          # NemesisBot 源码（不变）
├── test/
│   ├── TestAIServer/    # TestAIServer 源码（不变）
│   ├── websocket_chat_client.go  # 客户端源码（不变）
│   └── autotest/        # 测试工作目录（新建）
│       ├── testaiserver.exe
│       ├── nemesisbot.exe
│       ├── websocket_chat_client.exe
│       └── testaiserver.pid
```

**⚠️ 重要**：
- 所有测试工具编译产物都在 `test/autotest/` 中
- 所有测试操作都在 `test/autotest/` 目录中执行
- 源码文件位置保持不变

---

## 故障排查

### 问题 1: 起始目录验证失败

**症状**: 提示"当前目录不是项目根目录"

**原因**: 从错误的目录开始执行流程

**解决方案**:
```bash
# 返回项目根目录
cd /c/AI/NemesisBot/NemesisBot

# 或使用相对路径
cd ../../  # 如果在 test/autotest/ 中
```

---

### 问题 2: TestAIServer 启动失败

**症状**: `testaiserver.exe` 进程未运行

**可能原因**:
- 端口 8080 被占用
- 编译错误
- 权限问题

**解决方案**:
```bash
# Windows: 检查端口占用
netstat -ano | findstr ":8080"

# 如果被占用，杀死占用进程
taskkill //F //PID <PID>

# Unix/Linux: 检查端口占用
netstat -tulpn | grep 8080
# 或
lsof -i :8080

# 杀死占用进程
kill -9 <PID>
```

---

### 问题 3: NemesisBot 编译失败

**症状**: `go build` 返回错误

**可能原因**:
- 依赖缺失
- 代码错误
- Go 版本不兼容

**解决方案**:
```bash
# 确保在 test/autotest/ 目录
pwd  # 应该显示 .../test/autotest

# 更新依赖（从项目根目录）
cd ../../
go mod tidy

# 返回测试目录
cd test/autotest

# 重新编译
go build -o nemesisbot.exe ../../nemesisbot
```

---

### 问题 4: TestAIServer API 不响应

**症状**: `curl` 返回错误

**可能原因**:
- 服务器未完全启动
- 防火墙阻止

**解决方案**:
```bash
# 检查服务器进程
# Windows
tasklist | findstr testaiserver

# Unix/Linux
ps aux | grep testaiserver

# 增加等待时间
sleep 5
```

---

## 检查点

**环境准备完成检查点**:

- [ ] 起始目录验证通过
- [ ] 测试工作目录 `test/autotest/` 已创建
- [ ] TestAIServer 编译成功（`test/autotest/testaiserver.exe`）
- [ ] NemesisBot 编译成功（`test/autotest/nemesisbot.exe`）
- [ ] WebSocket 客户端编译成功（`test/autotest/websocket_chat_client.exe`）
- [ ] TestAIServer 进程运行中
- [ ] **TestAIServer PID 已保存到 `test/autotest/testaiserver.pid`** ⚠️ **必须**
- [ ] TestAIServer 端口监听
- [ ] TestAIServer API 可访问
- [ ] **当前工作目录在 `test/autotest/`** ⚠️ **重要**

**状态**: ✅ 通过 / ❌ 失败

---

**下一步**: 阶段 3 - 本地环境初始化（继续在 `test/autotest/` 目录中执行）
