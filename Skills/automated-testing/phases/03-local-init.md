# 阶段 3: 本地环境初始化

创建本地配置目录和初始化配置文件。

---

## 步骤

### 3.1 创建本地配置目录

**目的**: 使用 `--local` 标志在当前目录创建独立的配置

**命令**:
```bash
# 执行 onboard 命令
./nemesisbot.exe onboard default --local

# 等待初始化完成
sleep 2
```

**预期输出**:
```
📟 Detected platform: windows
🔒 Applying platform-specific security rules...
✓ Local mode enabled: using ./.nemesisbot

Configuration initialized successfully.
Config file: ./.nemesisbot/config.json
Workspace: ./.nemesisbot/workspace/

✓ Identity file created: ./.nemesisbot/IDENTITY.md
✓ Soul file created: ./.nemesisbot/SOUL.md
✓ User file created: ./.nemesisbot/USER.md
```

---

### 3.2 验证目录结构

**目的**: 确认所有必要的文件和目录都已创建

**验证命令**:
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
ls -la ./.nemesisbot/

# 预期输出:
# ./.nemesisbot/
# ├── config.json
# ├── IDENTITY.md
# ├── SOUL.md
# ├── USER.md
# └── workspace/
#     ├── agents/
#     ├── cluster/
#     └── logs/
```

---

### 3.3 验证配置文件

**目的**: 检查配置文件内容是否正确

**命令**:
```bash
# 检查 config.json
echo "=== 检查配置文件 ==="
cat ./.nemesisbot/config.json | jq '.'
```

**预期内容**:
```json
{
  "model": "",
  "modelList": [],
  "agents": {
    "defaults": {
      "llm": "",
      "workspace": "./workspace",
      "restrict_to_workspace": true,
      "enable_cron": true
    }
  },
  "channels": {
    "web": {
      "enabled": true,
      "host": "0.0.0.0",
      "port": 8080
    }
  },
  "logging": {
    "general": {
      "enabled": true,
      "level": "INFO"
    }
  },
  "security": {
    "enabled": true,
    "min_risk_level": "MEDIUM"
  }
}
```

---

### 3.4 验证工作空间目录

**目的**: 确认工作空间子目录已创建

**命令**:
```bash
echo "=== 检查工作空间目录 ==="

# 检查必需的子目录
required_dirs=(
  ".nemesisbot/workspace"
  ".nemesisbot/workspace/agents"
  ".nemesisbot/workspace/cluster"
  ".nemesisbot/workspace/logs"
)

for dir in "${required_dirs[@]}"; do
  if [ -d "./.$dir" ]; then
    echo "✅ $dir"
  else
    echo "❌ $dir 缺失"
    exit 1
  fi
done
```

---

### 3.5 验证配置文件

**目的**: 检查 AI 身份相关文件

**命令**:
```bash
echo "=== 检查 AI 身份文件 ==="

# 检查文件是否存在
identity_files=(
  ".nemesisbot/IDENTITY.md"
  ".nemesisbot/SOUL.md"
  ".nemesisbot/USER.md"
)

for file in "${identity_files[@]}"; do
  if [ -f "./$file" ]; then
    echo "✅ $file"
  else
    echo "❌ $file 缺失"
    exit 1
  fi
done
```

---

### 3.6 配置验证清单

```yaml
本地环境初始化清单:
  目录创建:
    - [✅] .nemesisbot 目录
    - [✅] workspace 目录
    - [✅] workspace/agents 目录
    - [✅] workspace/cluster 目录
    - [✅] workspace/logs 目录

  配置文件:
    - [✅] config.json
    - [✅] IDENTITY.md
    - [✅] SOUL.md
    - [✅] USER.md

  配置验证:
    - [✅] 工作空间路径正确
    - [✅] 安全配置已应用
    - [✅] 日志配置已设置
    - [✅] 通道配置已启用
```

---

## 故障排查

### 问题 1: onboard 命令失败

**症状**: `onboard default --local` 返回错误

**可能原因**:
- 权限不足
- 磁盘空间不足
- 依赖文件缺失

**解决方案**:
```bash
# 检查权限
# 确保当前用户有写入权限

# 检查磁盘空间
df -h .

# 检查是否是嵌套的 .nemesisbot 目录
# 如果在主 .nemesisbot 目录中运行，可能冲突
```

---

### 问题 2: 配置文件缺失

**症状**: 某些配置文件未创建

**可能原因**:
- onboard 流程中断
- 文件创建权限问题

**解决方案**:
```bash
# 手动创建缺失文件
# 或重新运行 onboard
rm -rf ./.nemesisbot
./nemesisbot.exe onboard default --local
```

---

### 问题 3: 配置内容不正确

**症状**: config.json 内容不符合预期

**可能原因**:
- 模板文件问题
- 平台特定配置问题

**解决方案**:
```bash
# 检查配置文件
cat ./.nemesisbot/config.json

# 如果需要，手动修正配置
# 使用 jq 或文本编辑器
```

---

## 环境隔离说明

**为什么使用 --local**:
- ✅ 完全隔离，不影响主配置（`~/.nemesisbot`）
- ✅ 每次测试都是干净的环境
- ✅ 测试完成后可以完全清理
- ✅ 避免配置冲突

**目录结构对比**:
```
主配置: ~/.nemesisbot/
本地配置: ./.nemesisbot/
```

---

## 初始化脚本

```bash
#!/bin/bash
# init_local_env.sh - 本地环境初始化脚本

set -e

echo "=== 开始本地环境初始化 ==="

# 1. 执行 onboard
echo "[1/3] 创建本地配置..."
./nemesisbot.exe onboard default --local

# 2. 等待完成
echo "[2/3] 等待初始化完成..."
sleep 2

# 3. 验证目录
echo "[3/3] 验证目录结构..."
[ -d "./.nemesisbot" ] || { echo "❌ 目录创建失败"; exit 1; }
[ -d "./.nemesisbot/workspace" ] || { echo "❌ workspace 缺失"; exit 1; }
[ -f "./.nemesisbot/config.json" ] || { echo "❌ config.json 缺失"; exit 1; }

# 列出目录
echo ""
echo "目录结构:"
tree -L 2 ./.nemesisbot/ 2>/dev/null || ls -laR ./.nemesisbot/

echo ""
echo "=== 本地环境初始化完成 ==="
echo "配置位置: ./.nemesisbot/"
```

---

## 检查点

**本地环境初始化完成检查点**:

- [ ] .nemesisbot 目录已创建
- [ ] workspace 目录结构完整
- [ ] config.json 文件存在
- [ ] IDENTITY.md 文件存在
- [ ] SOUL.md 文件存在
- [ ] USER.md 文件存在
- [ ] 配置内容正确

**状态**: ✅ 通过 / ❌ 失败

---

**下一步**: 阶段 4 - 配置测试 AI
