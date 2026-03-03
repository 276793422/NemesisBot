# NEMESISBOT_HOME 环境变量使用指南

## 概述

`NEMESISBOT_HOME` 环境变量用于指定 NemesisBot 项目数据的根目录。设置后，所有项目相关文件（配置、工作空间等）都会组织在该目录下的 `.nemesisbot/` 子目录中。

---

## 目录结构

### 设置 NEMESISBOT_HOME 后的目录结构

```bash
export NEMESISBOT_HOME=/opt/nemesisbot

/opt/nemesisbot/                    # ← 您指定的根目录
└── .nemesisbot/                    # ← 项目目录（自动创建）
    ├── config.json                 # 主配置文件
    └── workspace/                  # 工作空间目录
        ├── cluster/               # 集群相关数据
        │   ├── peers.toml
        │   └── state.toml
        ├── agents/                # Agent 配置
        └── logs/                   # 日志文件
```

---

## 使用场景

### 场景 1: 生产环境部署

```bash
# 设置生产环境目录
export NEMESISBOT_HOME=/opt/nemesisbot

# 启动服务
/opt/nemesisbot/nemesisbot.exe gateway
```

**优点**：
- ✅ 所有数据集中管理
- ✅ 配置和工作空间在一起
- ✅ 迁移方便（只需复制 `.nemesisbot/` 目录）

### 场景 2: 开发环境

```bash
# 设置开发环境目录
export NEMESISBOT_HOME=~/projects/nemesisbot-dev

# 运行测试
./nemesisbot.exe test
```

**优点**：
- ✅ 与生产环境结构一致
- ✅ 便于切换不同环境

### 场景 3: 多实例部署

```bash
# 实例 1
export NEMESISBOT_HOME=/opt/instance1
./nemesisbot.exe daemon &

# 实例 2
export NEMESISBOT_HOME=/opt/instance2
./nemesisbot.exe daemon &
```

**优点**：
- ✅ 每个实例完全独立
- ✅ 互不干扰
- ✅ 管理清晰

### 场景 4: 便携模式（当前目录）

```bash
# 不设置环境变量，使用 --local 标志
./nemesisbot.exe --local gateway

# 或者当前目录有 .nemesisbot 目录时自动检测
cd /my/project
./nemesisbot.exe gateway
```

**优点**：
- ✅ 项目自包含
- ✅ 无需配置即可运行

---

## 环境变量解析优先级

当解析 `.nemesisbot` 目录位置时，按以下优先级（从高到低）：

### 1. LocalMode（--local 标志）
```bash
./nemesisbot.exe --local
# 使用: ./.nemesisbot/
```

### 2. NEMESISBOT_HOME 环境变量
```bash
export NEMESISBOT_HOME=/custom/path
# 使用: /custom/path/.nemesisbot/
```

### 3. 自动检测
```bash
# 如果当前目录存在 .nemesisbot/ 目录
# 使用: ./.nemesisbot/
```

### 4. 默认位置
```bash
# 使用: ~/.nemesisbot/
```

---

## 路径解析示例

### 示例 1: 设置 NEMESISBOT_HOME

```bash
$ export NEMESISBOT_HOME=/opt/nemesisbot
$ ./nemesisbot.exe gateway

# 实际使用的路径：
主目录: /opt/nemesisbot/.nemesisbot/
配置:   /opt/nemesisbot/.nemesisbot/config.json
工作区: /opt/nemesisbot/.nemesisbot/workspace/
```

### 示例 2: 使用 LocalMode

```bash
$ ./nemesisbot.exe --local gateway

# 实际使用的路径：
主目录: ./nemesisbot/
配置:   .nemesisbot/config.json
工作区: .nemesisbot/workspace/
```

### 示例 3: 默认行为

```bash
$ ./nemesisbot.exe gateway

# 实际使用的路径（假设没有 .nemesisbot 自动检测）：
主目录: ~/.nemesisbot/
配置:   ~/.nemesisbot/config.json
工作区: ~/.nemesisbot/workspace/
```

---

## 配置文件位置

### 主配置文件 (config.json)

| 场景 | 配置文件位置 |
|------|-------------|
| 设置 `NEMESISBOT_HOME=/opt/nemesisbot` | `/opt/nemesisbot/.nemesisbot/config.json` |
| 使用 `--local` 标志 | `./.nemesisbot/config.json` |
| 自动检测到 `.nemesisbot` | `./.nemesisbot/config.json` |
| 默认行为 | `~/.nemesisbot/config.json` |

### 覆盖配置文件位置

可以使用 `NEMESISBOT_CONFIG` 环境变量指定配置文件位置（高级用法）：

```bash
export NEMESISBOT_CONFIG=/etc/nemesisbot/config.json
```

---

## 工作空间 (workspace)

工作空间目录总是位于主目录（`.nemesisbot/`）下：

```bash
$NEMESISBOT_HOME=/opt/nemesisbot
# 工作空间: /opt/nemesisbot/.nemesisbot/workspace/

~/.nemesisbot/
# 工作空间: ~/.nemesisbot/workspace/
```

### 工作空间包含

```
workspace/
├── cluster/           # 集群配置和状态
│   ├── peers.toml     # 对等节点配置
│   └── state.toml     # 集群状态
├── config/            # 各种子配置
│   ├── config.mcp.json
│   ├── config.security.json
│   └── config.cluster.json
├── agents/            # Agent 配置和状态
│   └── default/
└── logs/               # 运行日志
    ├── cluster/
    ├── security/
    └── ...
```

---

## 迁移和备份

### 完整项目备份

```bash
# 设置环境变量
export NEMESISBOT_HOME=/opt/nemesisbot

# 备份整个项目目录
tar -czf nemesisbot-backup-$(date +%Y%m%d).tar.gz /opt/nemesisbot/.nemesisbot/

# 恢复
tar -xzf nemesisbot-backup-20250304.tar.gz -C /opt/nemesisbot/
```

### 迁移到新机器

```bash
# 1. 在新机器上创建目录
mkdir -p /opt/nemesisbot

# 2. 复制整个 .nemesisbot 目录
scp -r oldmachine:/opt/nemesisbot/.nemesisbot/ \
          newmachine:/opt/nemesisbot/

# 3. 在新机器上设置环境变量
export NEMESISBOT_HOME=/opt/nemesisbot

# 4. 启动
./nemesisbot.exe gateway
```

---

## 常见问题

### Q1: 如何查看当前使用的主目录？

```bash
# 查看环境变量
echo $NEMESISBOT_HOME

# 使用 nemesisbot 命令查看
./nemesisbot.exe status
```

### Q2: NEMESISBOT_HOME 和 NEMESISBOT_CONFIG 有什么区别？

- **NEMESISBOT_HOME**: 指定项目根目录，config.json 和 workspace 都在其中
- **NEMESISBOT_CONFIG**: 指定配置文件的精确位置（会覆盖默认查找）

**推荐**: 使用 `NEMESISBOT_HOME`，让系统自动管理路径。

### Q3: 可以在同一个 NEMESISBOT_HOME 下运行多个实例吗？

不推荐。每个实例应该有自己独立的 `.nemesisbot/` 目录：

```bash
# 实例 1
export NEMESISBOT_HOME=/opt/instance1
./nemesisbot.exe &

# 实例 2
export NEMESISBOT_HOME=/opt/instance2
./nemesisbot.exe &
```

### Q4: nemesisbot.exe 必须和 .nemesisbot 在同一目录吗？

**不需要**。exe 可以在任何位置，只要正确设置 `NEMESISBOT_HOME`：

```bash
/opt/nemesisbot/bin/nemesisbot.exe
# 设置: export NEMESISBOT_HOME=/data/nemesisbot
# 项目: /data/nemesisbot/.nemesisbot/
```

### Q5: 如何切换不同的项目环境？

```bash
# 项目 A
export NEMESISBOT_HOME=~/project-a
./nemesisbot.exe gateway

# 项目 B
export NEMESISBOT_HOME=~/project-b
./nemesisbot.exe gateway
```

---

## 最佳实践

### 1. 生产环境
```bash
# 使用固定目录
export NEMESISBOT_HOME=/var/lib/nemesisbot
mkdir -p $NEMESISBOT_HOME/.nemesisbot

# 配置权限
chmod 755 $NEMESISBOT_HOME/.nemesisbot
chmod 644 $NEMESISBOT_HOME/.nemesisbot/config.json
```

### 2. 开发环境
```bash
# 每个开发项目独立目录
export NEMESISBOT_HOME=~/dev/nemesisbot-project
```

### 3. 测试环境
```bash
# 使用临时目录
export NEMESISBOT_HOME=/tmp/nemesisbot-test
```

### 4. 多实例部署
```bash
# 每个实例独立根目录
export NEMESISBOT_HOME=/opt/nemesisbot/instance-1
export NEMESISBOT_HOME=/opt/nemesisbot/instance-2
export NEMESISBOT_HOME=/opt/nemesisbot/instance-3
```

---

## 总结

**关键设计原则**：
1. **项目完整性**: `.nemesisbot/` 包含所有项目数据
2. **易于迁移**: 复制一个目录即可
3. **多实例支持**: 每个 `.nemesisbot/` 独立运行
4. **向后兼容**: 默认行为和 LocalMode 不受影响

**目录结构**：
```
$NEMESISBOT_HOME/
└── .nemesisbot/        ← 一个完整的项目
    ├── config.json
    └── workspace/
```

**使用建议**：
- 生产环境：设置明确的 `NEMESISBOT_HOME`
- 开发环境：使用 `--local` 或默认位置
- 多实例：每个实例独立的 `NEMESISBOT_HOME`
