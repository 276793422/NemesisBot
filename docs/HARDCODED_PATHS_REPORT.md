# 硬编码路径问题报告

## 搜索日期
2026-03-04

---

## 问题概述

搜索代码发现多处硬编码的 `~/.nemesisbot` 路径，这些硬编码会导致：
1. 设置 NEMESISBOT_HOME 后，某些路径仍然指向 `~/.nemesisbot/`
2. 不支持多实例部署
3. 路径不一致，难以迁移

---

## 发现的硬编码位置

### 🔴 严重问题（需要修复）

#### 1. module/config/config.go:687

**位置**: `DefaultConfig()` 函数

```go
Logging: &LoggingConfig{
    LLMRequests: false,
    LogDir:      "~/.nemesisbot/workspace/logs/request_logs",  // ❌ 硬编码
    DetailLevel: "full",
},
```

**影响**:
- 当启用 LLM logging 时，日志目录硬编码为 `~/.nemesisbot/`
- 即使用户设置了 `NEMESISBOT_HOME`，日志仍会保存到用户目录
- 日志与项目数据分离

**优先级**: 🔴 高

---

#### 2. config/config.default.json:4

**位置**: 默认配置文件（嵌入到可执行文件中）

```json
{
  "agents": {
    "defaults": {
      "workspace": "~/.nemesisbot/workspace",  // ❌ 硬编码
      ...
    }
  }
}
```

**影响**:
- 当配置文件不存在时，使用嵌入的默认配置
- workspace 路径硬编码为 `~/.nemesisbot/`
- 优先级低于 `DefaultConfig()`，但仍可能被使用

**优先级**: 🔴 高

---

#### 3. nemesisbot/main.go:346

**位置**: `onboard default` 命令

```go
// Step 5: Enable LLM logging (optional enhancement for default mode)
if cfg.Logging == nil {
    // Determine log directory based on local mode
    logDir := "~/.nemesisbot/workspace/logs/request_logs"  // ❌ 硬编码
    if isLocalMode {
        logDir = filepath.Join(".nemesisbot", "workspace", "logs", "request_logs")
    }
    ...
}
```

**影响**:
- 在非 LocalMode 下，logDir 硬编码
- 会覆盖 DefaultConfig() 中的设置
- 即使 NEMESISBOT_HOME 已设置，仍使用错误的路径

**优先级**: 🔴 高

---

### 🟡 次要问题（建议修复）

#### 4. nemesisbot/command/log.go:119

**位置**: `log enable` 命令

```go
// Set defaults if not set
if cfg.Logging.LogDir == "" {
    cfg.Logging.LogDir = "~/.nemesisbot/workspace/logs/request_logs"  // ❌ 硬编码
}
```

**影响**:
- 当用户启用 logging 时，如果 logDir 为空，使用硬编码值
- 应该使用 path 包获取动态路径

**优先级**: 🟡 中

---

#### 5. nemesisbot/command/log.go:169, 248

**位置**: `log status` 命令

```go
logDir := "~/.nemesisbot/workspace/logs/request_logs"  // ❌ 仅作显示用
```

**影响**:
- 这些值仅用于显示，实际从 config 读取
- 如果 config 中没有设置，会显示错误的默认值
- 不影响实际功能，但会误导用户

**优先级**: 🟢 低

---

### ✅ 合理的硬编码（不需要修复）

#### 6. nemesisbot/main.go:115, 230

```go
if path.LocalMode || path.DetectLocal() {
    cfg.Agents.Defaults.Workspace = filepath.Join(".nemesisbot", "workspace")
}
```

**说明**: LocalMode 就是要使用当前目录，这是正确的

---

#### 7. module/path/paths.go:64

```go
const DefaultHomeDir = ".nemesisbot"
```

**说明**: 这是目录名常量，用于所有模式，这是正确的

---

#### 8. 测试文件中的路径

**位置**: `module/path/paths_test.go`, `test/unit/path/`, `test/path/integration.go`

**说明**: 测试代码中的硬编码是合理的，用于验证行为

---

#### 9. 文档中的路径

**位置**: 各种 `.md` 文档

**说明**: 文档中说明默认行为使用 `~/.nemesisbot/` 是正确的

---

## 根本原因

**核心问题**: `module/config/config.go` 的 `DefaultConfig()` 函数中，workspace 和 logDir 都是硬编码的

**影响链**:
```
DefaultConfig() 硬编码
    ↓
config.default.json 硬编码（用于嵌入）
    ↓
onboard 命令覆盖 logDir（硬编码）
    ↓
log enable 命令使用硬编码默认值
    ↓
最终结果：路径混乱
```

---

## 修复方案

### 修复顺序

1. **修复 module/config/config.go** ⭐ 最关键
   - 修改 `DefaultConfig()` 函数
   - 使用 `path.NewPathManager()` 获取动态路径
   - Workspace 和 LogDir 都使用动态路径

2. **更新 config/config.default.json**
   - 更新默认 workspace 路径
   - 保持与 DefaultConfig() 一致

3. **修复 nemesisbot/main.go**
   - 删除硬编码的 logDir
   - 使用 DefaultConfig() 中的值或 path 包

4. **修复 nemesisbot/command/log.go**
   - 使用 path 包获取动态默认路径
   - 更新显示的默认值

---

## 修复后的预期行为

### 场景 1: 设置 NEMESISBOT_HOME

```bash
$ export NEMESISBOT_HOME=/opt/nemesisbot
$ ./nemesisbot.exe onboard --skip-all
```

**当前行为** ❌:
- Config: `/opt/nemesisbot/.nemesisbot/config.json` ✅
- Workspace: `/opt/nemesisbot/.nemesisbot/workspace` ✅
- LogDir: `~/.nemesisbot/workspace/logs/request_logs` ❌

**修复后** ✅:
- Config: `/opt/nemesisbot/.nemesisbot/config.json`
- Workspace: `/opt/nemesisbot/.nemesisbot/workspace`
- LogDir: `/opt/nemesisbot/.nemesisbot/workspace/logs/request_logs`

### 场景 2: 多实例部署

```bash
# 实例 1
$ export NEMESISBOT_HOME=/opt/instance1
$ ./nemesisbot.exe log enable

# 实例 2
$ export NEMESISBOT_HOME=/opt/instance2
$ ./nemesisbot.exe log enable
```

**修复后** ✅:
- 实例1 日志: `/opt/instance1/.nemesisbot/workspace/logs/`
- 实例2 日志: `/opt/instance2/.nemesisbot/workspace/logs/`
- 完全隔离，互不干扰

---

## 测试验证计划

### 单元测试
1. 测试 `DefaultConfig()` 返回正确的路径
2. 测试 LogDir 使用 path 包
3. 测试 NEMESISBOT_HOME 被正确使用

### 集成测试
1. 测试 `onboard` 命令创建正确的配置
2. 测试 `log enable` 命令使用正确的路径
3. 测试 `log status` 显示正确的路径
4. 测试多实例部署日志隔离

### 功能测试
1. 启用 logging 后检查日志文件位置
2. 验证 NEMESISBOT_HOME 生效
3. 验证 LocalMode 不受影响

---

## 优先级总结

| 文件 | 行号 | 问题 | 优先级 | 影响范围 |
|------|------|------|--------|----------|
| module/config/config.go | 687 | LogDir 硬编码 | 🔴 高 | 所有使用默认配置的用户 |
| config/config.default.json | 4 | workspace 硬编码 | 🔴 高 | 嵌入配置被使用时 |
| nemesisbot/main.go | 346 | logDir 硬编码 | 🔴 高 | onboard default 命令 |
| nemesisbot/command/log.go | 119 | LogDir 默认值 | 🟡 中 | log enable 命令 |
| nemesisbot/command/log.go | 169, 248 | logDir 显示 | 🟢 低 | log status 显示 |

---

## 建议

**立即修复** 🔴:
1. module/config/config.go - LogDir 硬编码
2. config/config.default.json - workspace 硬编码
3. nemesisbot/main.go - logDir 硬编码

**后续修复** 🟡:
4. nemesisbot/command/log.go - 默认值使用 path 包

**可选** 🟢:
5. 更新 log status 的默认显示值

---

## 总结

发现了 **5 个需要修复的硬编码位置**，其中 **3 个是高优先级**。

**核心问题**: `DefaultConfig()` 函数中的硬编码导致整个路径系统不一致。

**修复策略**: 从源头（DefaultConfig）修复，然后更新依赖它的代码。

**预期收益**:
- ✅ 所有路径统一使用 NEMESISBOT_HOME
- ✅ 支持多实例部署
- ✅ 项目完整性（所有数据在 .nemesisbot/ 下）
- ✅ 易于迁移和备份

---

**报告生成时间**: 2026-03-04
**建议**: 立即修复高优先级问题
