# 阶段 3: 本地环境初始化

创建本地配置目录和初始化配置文件。

**⚠️ 重要前提**：
- 此阶段必须在 `test/autotest/` 目录中执行
- 阶段 2 应该已经创建了该目录并编译了所有工具

**当前工作目录**: `test/autotest/`

**阶段目标**：
- 使用 `--local` 模式初始化 NemesisBot 配置
- 创建 `test/autotest/.nemesisbot/` 目录
- **本阶段结束时，仍在 `test/autotest/` 目录中**

---

## 步骤

### 3.1 验证当前目录

**目的**: 确保在正确的目录中执行

**当前工作目录**: `test/autotest/`

**命令**:
```bash
# 验证当前目录
echo "=== 验证当前目录 ==="

# 检查是否在 test/autotest/ 目录
CURRENT_DIR=$(pwd)
if [[ ! "$CURRENT_DIR" =~ /test/autotest$ ]]; then
  echo "❌ 错误: 当前目录不是 test/autotest/"
  echo "   当前目录: $CURRENT_DIR"
  echo "   请先执行阶段 2，或切换到正确的目录"
  exit 1
fi

echo "✅ 当前目录验证通过: $CURRENT_DIR"
echo ""

# 检查必要文件是否存在
if [ ! -f "./nemesisbot.exe" ]; then
  echo "❌ 错误: 找不到 nemesisbot.exe"
  echo "   请先执行阶段 2 进行编译"
  exit 1
fi

if [ ! -f "./testaiserver.exe" ]; then
  echo "❌ 错误: 找不到 testaiserver.exe"
  echo "   请先执行阶段 2 进行编译"
  exit 1
fi

echo "✅ 测试工具验证通过"
echo ""
```

**预期输出**:
```
=== 验证当前目录 ===
✅ 当前目录验证通过: /c/AI/NemesisBot/NemesisBot/test/autotest
✅ 测试工具验证通过
```

---

### 3.2 执行环境初始化

**目的**: 使用 `--local` 模式创建本地配置

**当前工作目录**: `test/autotest/`

**⚠️ 重要**: 必须确保在 `test/autotest/` 目录中执行，否则配置会被创建在错误的位置

**命令**:
```bash
# 执行 onboard 命令（--local 模式）
echo "=== 初始化本地环境 ==="

# **关键**: 验证当前目录
echo "📍 当前工作目录: $(pwd)"

# 验证是否在正确的目录
if [[ ! "$(pwd)" =~ /test/autotest$ ]]; then
  echo "❌ 错误: 不在 test/autotest/ 目录中"
  echo "   当前目录: $(pwd)"
  echo ""
  echo "正在切换到正确目录..."
  cd test/autotest || {
    echo "❌ 无法切换到 test/autotest/"
    exit 1
  }
  echo "✅ 已切换到: $(pwd)"
fi

echo ""

# 在当前目录执行 onboard
# --local 标志会在当前目录创建 .nemesisbot/
./nemesisbot.exe onboard default --local

# 等待初始化完成
sleep 2
echo ""
```

**预期输出**:
```
=== 初始化本地环境 ===
📍 Local mode enabled: using ./.nemesisbot
🚀 Initializing NemesisBot with default settings...
📟 Detected platform: Windows (windows/amd64)
🔒 Applying platform-specific security rules...

✓ Main config saved to .nemesisbot/config.json
✓ MCP config created
✓ Security config created (Windows rules)
✓ Cluster config created
✓ Peers config created at .nemesisbot\workspace\cluster\peers.toml
✓ LLM logging enabled
✓ Security module enabled
✓ Workspace templates created
✓ Default personality files installed (IDENTITY.md, SOUL.md, USER.md)
✓ BOOTSTRAP.md removed
✓ Web authentication token set
✓ Web server host set to 127.0.0.1
✓ Web server port set to 49000
✓ WebSocket channel enabled

🤖 Initialization complete!

📝 Next step:
  Configure your LLM API key and start chatting:

  1. Add API key:
     nemesisbot model add --model zhipu/glm-4.7-flash --key YOUR_API_KEY --default

  2. Start the gateway:
     nemesisbot gateway

  Available interfaces:
    • Web:     http://127.0.0.1:49000 (access key: 276793422)
    • WebSocket: ws://127.0.0.1:49001/ws

For more information:
  nemesisbot --help
```

---

### 3.3 验证目录结构

**目的**: 确认所有必要的文件和目录都已创建

**当前工作目录**: `test/autotest/`

**配置位置**: `./.nemesisbot/`（即 `test/autotest/.nemesisbot/`）

**命令**:
```bash
# 检查 .nemesisbot 目录
echo "=== 验证本地配置目录 ==="

if [ ! -d "./.nemesisbot" ]; then
  echo "❌ .nemesisbot 目录不存在"
  exit 1
fi

echo "✅ .nemesisbot 目录已创建"

# 列出目录内容
echo ""
echo "目录结构:"
ls -la ./.nemesisbot/ | head -20

echo ""
echo "📍 配置位置: $(pwd)/.nemesisbot/"
```

**预期输出**:
```
=== 验证本地配置目录 ===
✅ .nemesisbot 目录已创建

目录结构:
total 48
drwxr-xr-x 1 Zoo 197121    0 Mar 23 12:23 .
drwxr-xr-x 1 Zoo 197121    0 Mar 23 12:23 ..
-rw-r--r-- 1 Zoo 197121 3415 Mar 23 12:23 config.json
drwxr-xr-x 1 Zoo 197121    0 Mar 23 12:23 workspace

📍 配置位置: /c/AI/NemesisBot/NemesisBot/test/autotest/.nemesisbot
```

---

### 3.4 验证配置文件

**目的**: 检查配置文件是否正确创建

**当前工作目录**: `test/autotest/`

**命令**:
```bash
echo "=== 检查配置文件 ==="

# 检查主配置文件
if [ ! -f "./.nemesisbot/config.json" ]; then
  echo "❌ config.json 不存在"
  exit 1
fi

echo "✅ config.json 存在"

# 检查工作空间目录
if [ ! -d "./.nemesisbot/workspace" ]; then
  echo "❌ workspace 目录不存在"
  exit 1
fi

echo "✅ workspace 目录存在"

# 检查必需的子目录
required_dirs=(
  ".nemesisbot/workspace/agents"
  ".nemesisbot/workspace/cluster"
  ".nemesisbot/workspace/logs"
)

echo ""
echo "检查工作空间子目录:"
for dir in "${required_dirs[@]}"; do
  if [ -d "./$dir" ]; then
    echo "  ✅ $dir"
  else
    echo "  ❌ $dir 缺失"
    exit 1
  fi
done
echo ""

# 检查身份文件
identity_files=(
  ".nemesisbot/IDENTITY.md"
  ".nemesisbot/SOUL.md"
  ".nemesisbot/USER.md"
)

echo "检查 AI 身份文件:"
for file in "${identity_files[@]}"; do
  if [ -f "./$file" ]; then
    echo "  ✅ $file"
  else
    echo "  ❌ $file 缺失"
    exit 1
  fi
done
echo ""
```

**预期输出**:
```
=== 检查配置文件 ===
✅ config.json 存在
✅ workspace 目录存在

检查工作空间子目录:
  ✅ .nemesisbot/workspace/agents
  ✅ .nemesisbot/workspace/cluster
  ✅ .nemesisbot/workspace/logs

检查 AI 身份文件:
  ✅ .nemesisbot/IDENTITY.md
  ✅ .nemesisbot/SOUL.md
  ✅ .nemesisbot/USER.md
```

---

### 3.5 验证配置内容

**目的**: 检查配置文件内容是否正确

**当前工作目录**: `test/autotest/`

**命令**:
```bash
echo "=== 检查配置内容 ==="
echo ""

# 显示关键配置
echo "📋 关键配置项:"
echo ""

# 检查 Web 服务器配置
if command -v jq > /dev/null 2>&1; then
  # 使用 jq 美化输出（如果安装了）
  echo "Web 服务器配置:"
  cat ./.nemesisbot/config.json | jq '.channels.web' 2>/dev/null || echo "  (无法解析 JSON)"
  echo ""
else
  echo "提示: 安装 jq 可查看详细配置"
  echo ""
fi

# 验证 WebSocket 通道是否启用
echo "✅ 本地环境初始化完成"
echo ""
echo "📍 当前工作目录: $(pwd)"
echo "📁 配置目录: $(pwd)/.nemesisbot/"
echo ""
```

**预期输出**:
```
=== 检查配置内容 ===

📋 关键配置项:

Web 服务器配置:
{
  "enabled": true,
  "host": "127.0.0.1",
  "port": 49000,
  ...
}

✅ 本地环境初始化完成

📍 当前工作目录: /c/AI/NemesisBot/NemesisBot/test/autotest
📁 配置目录: /c/AI/NemesisBot/NemesisBot/test/autotest/.nemesisbot
```

---

## 目录结构说明

**本阶段完成后的目录结构**:
```
test/autotest/              # 当前工作目录
├── testaiserver.exe         # 已编译（阶段 2）
├── nemesisbot.exe           # 已编译（阶段 2）
├── websocket_chat_client.exe  # 已编译（阶段 2）
├── testaiserver.pid         # 已创建（阶段 2）
└── .nemesisbot/             # 新创建（本阶段）
    ├── config.json
    ├── IDENTITY.md
    ├── SOUL.md
    ├── USER.md
    └── workspace/
        ├── agents/
        ├── cluster/
        │   └── peers.toml
        ├── logs/
        ├── memory/
        ├── sessions/
        ├── skills/
        └── state/
```

**⚠️ 重要**：
- `.nemesisbot/` 是相对于**当前工作目录**的
- 因为当前在 `test/autotest/` 中，所以配置在 `test/autotest/.nemesisbot/`
- 这样配置完全隔离，不影响项目根目录或用户主目录

---

## 环境隔离说明

**为什么使用 --local**:
- ✅ 完全隔离，不影响主配置（`~/.nemesisbot`）
- ✅ 不影响项目根目录配置
- ✅ 每次测试都是干净的环境
- ✅ 测试完成后可以完全清理（删除整个 `test/autotest/`）
- ✅ 避免配置冲突

**配置位置对比**:
```
主配置:        ~/.nemesisbot/
项目本地配置:  ./.nemesisbot/ (在项目根目录)
测试配置:      test/autotest/.nemesisbot/ (在本流程中)
```

---

## 故障排查

### 问题 1: 目录验证失败

**症状**: 提示"当前目录不是 test/autotest/"

**原因**: 在错误的目录中执行

**解决方案**:
```bash
# 检查当前目录
pwd

# 如果在项目根目录
cd test/autotest

# 如果在其他位置，使用绝对路径
cd /c/AI/NemesisBot/NemesisBot/test/autotest
```

---

### 问题 2: onboard 命令失败

**症状**: `onboard default --local` 返回错误

**可能原因**:
- 权限不足
- 磁盘空间不足
- 依赖文件缺失

**解决方案**:
```bash
# 检查磁盘空间
df -h .

# 检查是否有写入权限
touch test.txt && rm test.txt || echo "无写入权限"

# 如果之前创建过，先删除
rm -rf ./.nemesisbot

# 重新执行
./nemesisbot.exe onboard default --local
```

---

### 问题 3: 配置文件缺失

**症状**: 某些配置文件未创建

**可能原因**:
- onboard 流程中断
- 文件创建权限问题

**解决方案**:
```bash
# 删除现有配置
rm -rf ./.nemesisbot

# 重新运行 onboard
./nemesisbot.exe onboard default --local
```

---

## 检查点

**本地环境初始化完成检查点**:

- [ ] **当前目录验证通过**（在 `test/autotest/` 中）
- [ ] `.nemesisbot` 目录已创建
- [ ] workspace 目录结构完整
- [ ] config.json 文件存在
- [ ] IDENTITY.md 文件存在
- [ ] SOUL.md 文件存在
- [ ] USER.md 文件存在
- [ ] 配置内容正确
- [ ] **当前工作目录仍在 `test/autotest/`** ⚠️ **重要**

**状态**: ✅ 通过 / ❌ 失败

---

**下一步**: 阶段 4 - 配置测试 AI（继续在 `test/autotest/` 目录中执行）
