# 硬编码路径修复完成报告

## 执行日期
2026-03-04

---

## 修复概述

成功修复了所有硬编码的 `~/.nemesisbot` 路径，确保所有路径都尊重 `NEMESISBOT_HOME` 环境变量。

---

## 修复的文件

### 1. module/config/config.go ✅

#### 1.1 修复 DefaultConfig() 函数

**位置**: Line 545-693

**修改**:
```go
// 修改前
Logging: &LoggingConfig{
    LLMRequests: false,
    LogDir:      "~/.nemesisbot/workspace/logs/request_logs",  // ❌
    DetailLevel: "full",
},

// 修改后
Logging: &LoggingConfig{
    LLMRequests: false,
    LogDir:      filepath.Join(defaultWorkspace, "logs", "request_logs"),  // ✅
    DetailLevel: "full",
},
```

**效果**: DefaultConfig() 现在使用动态路径，尊重 NEMESISBOT_HOME

---

#### 1.2 新增 adjustPathsForEnvironment() 方法

**位置**: Line 1103-1135

**功能**:
- 检测硬编码的默认路径（`~/.nemesisbot/workspace` 和绝对路径形式）
- 将其替换为正确的动态路径（使用 path.NewPathManager()）
- 保留用户自定义的路径
- 跳过 LocalMode（由 SaveConfig 处理）

**代码**:
```go
func (c *Config) adjustPathsForEnvironment() {
    if path.LocalMode || path.DetectLocal() {
        return
    }

    pm := path.NewPathManager()
    expectedWorkspace := pm.Workspace()
    expectedLogDir := filepath.Join(expectedWorkspace, "logs", "request_logs")

    // 检测并修正 workspace
    userHome, _ := os.UserHomeDir()
    absoluteDefault := filepath.Join(userHome, ".nemesisbot", "workspace")

    isDefaultWorkspace := c.Agents.Defaults.Workspace == "~/.nemesisbot/workspace" ||
        c.Agents.Defaults.Workspace == filepath.Join("~", ".nemesisbot", "workspace") ||
        c.Agents.Defaults.Workspace == absoluteDefault

    if isDefaultWorkspace {
        c.Agents.Defaults.Workspace = expectedWorkspace
    }

    // 检测并修正 LogDir
    if c.Logging != nil {
        isDefaultLogDir := c.Logging.LogDir == "~/.nemesisbot/workspace/logs/request_logs" ||
            c.Logging.LogDir == filepath.Join("~", ".nemesisbot", "workspace", "logs", "request_logs") ||
            c.Logging.LogDir == filepath.Join(absoluteDefault, "logs", "request_logs")

        if isDefaultLogDir {
            c.Logging.LogDir = expectedLogDir
        }
    }
}
```

---

#### 1.3 在 LoadConfig() 中调用路径调整

**位置**: Line 733, 749

**修改**:
```go
// 添加调用
cfg.postProcessForCompatibility()
cfg.adjustPathsForEnvironment()  // ← 新增
```

**效果**: 所有从 LoadConfig() 加载的配置都会自动调整路径

---

#### 1.4 增强 SaveConfig 的 LocalMode 检查

**位置**: Line 778-799

**修改**:
- 增加对绝对路径形式的检测
- 同时检测 `~/.nemesisbot/workspace` 和 `{userHome}/.nemesisbot/workspace`
- 确保 LogDir 也被正确调整

---

### 2. nemesisbot/main.go ✅

#### 2.1 修复 onboard skip-all 命令

**位置**: Line 109-116

**修改**:
```go
// 修改前
if path.LocalMode || path.DetectLocal() {
    cfg.Agents.Defaults.Workspace = filepath.Join(".nemesisbot", "workspace")
}

// 修改后
if path.LocalMode || path.DetectLocal() {
    cfg.Agents.Defaults.Workspace = filepath.Join(".nemesisbot", "workspace")
    // Also adjust log directory to relative path
    if cfg.Logging != nil {
        cfg.Logging.LogDir = filepath.Join(".nemesisbot", "workspace", "logs", "request_logs")
    }
}
```

**效果**: LocalMode 下，LogDir 也使用相对路径

---

#### 2.2 修复 onboard default 命令

**位置**: Line 343-355

**修改**:
```go
// 修改前
logDir := "~/.nemesisbot/workspace/logs/request_logs"
if isLocalMode {
    logDir = filepath.Join(".nemesisbot", "workspace", "logs", "request_logs")
}

// 修改后
pm := path.NewPathManager()
logDir := filepath.Join(pm.Workspace(), "logs", "request_logs")
if isLocalMode {
    logDir = filepath.Join(".nemesisbot", "workspace", "logs", "request_logs")
}
```

**效果**: 使用 path 包获取动态路径

---

### 3. nemesisbot/command/log.go ✅

#### 3.1 添加 path 包导入

**位置**: Line 3-10

**修改**:
```go
import (
    "fmt"
    "os"
    "path/filepath"
    "strings"

    "github.com/276793422/NemesisBot/module/config"
    "github.com/276793422/NemesisBot/module/path"  // ← 新增
)
```

---

#### 3.2 修复 log enable 命令

**位置**: Line 118-121

**修改**:
```go
// 修改前
if cfg.Logging.LogDir == "" {
    cfg.Logging.LogDir = "~/.nemesisbot/workspace/logs/request_logs"
}

// 修改后
if cfg.Logging.LogDir == "" {
    pm := path.NewPathManager()
    cfg.Logging.LogDir = filepath.Join(pm.Workspace(), "logs", "request_logs")
}
```

---

#### 3.3 修复 log status 命令

**位置**: Line 170-172, 249-251

**修改**:
```go
// 修改前
enabled := false
logDir := "~/.nemesisbot/workspace/logs/request_logs"
detailLevel := "full"

// 修改后
enabled := false
pm := path.NewPathManager()
logDir := filepath.Join(pm.Workspace(), "logs", "request_logs")
detailLevel := "full"
```

**效果**: log status 显示正确的默认路径

---

## 测试验证

### 单元测试 ✅

**module/path 包测试**:
```
TestResolveHomeDir_Default                    ✅ PASS
TestResolveHomeDir_WithNEMESISBOT_HOME        ✅ PASS
TestResolveHomeDir_LocalMode                  ✅ PASS
TestResolveHomeDir_NEMESISBOT_HOMETakesPrecedence ✅ PASS
TestResolveConfigPath_WithNEMESISBOT_HOME     ✅ PASS
TestWorkspacePath_Integration                 ✅ PASS
TestNEMESISBOT_HOME_DirectoryStructure        ✅ PASS
TestPathManager_Consistency                   ✅ PASS
TestExpandHome                                ✅ PASS
```
**结果**: 9/9 测试通过 (100%)

**test/unit/path 包测试**:
```
TestDetectLocal                               ✅ PASS
TestLocalModePriority                         ✅ PASS
TestAutoDetectionOrder                        ✅ PASS
... (共22个测试)
```
**结果**: 22/22 测试通过 (100%)

**test/path 集成测试**:
```
验证 NEMESISBOT_HOME 目录结构                ✅ PASS
验证 LocalMode 不受影响                        ✅ PASS
验证默认行为不受影响                          ✅ PASS
验证 NEMESISBOT_HOME 优先级                   ✅ PASS
```
**结果**: 4/4 测试通过 (100%)

---

### 功能验证 ✅

#### 测试1: NEMESISBOT_HOME 生效

```bash
$ NEMESISBOT_HOME=/tmp/test-hardcode ./nemesisbot.exe onboard --skip-all
```

**配置文件**:
```json
{
  "agents": {
    "defaults": {
      "workspace": "C:\\Users\\Zoo\\AppData\\Local\\Temp\\test-hardcode\\.nemesisbot\\workspace"
    }
  },
  "logging": {
    "log_dir": "C:\\Users\\Zoo\\AppData\\Local\\Temp\\test-hardcode\\.nemesisbot\\workspace\\logs\\request_logs"
  }
}
```

✅ **Workspace 和 LogDir 都使用了 NEMESISBOT_HOME 路径**

---

#### 测试2: log enable 命令

```bash
$ NEMESISBOT_HOME=/tmp/test-hardcode ./nemesisbot.exe log enable
✅ LLM request logging enabled
📁 Log directory: C:\Users\Zoo\AppData\Local\Temp\test-hardcode\.nemesisbot\workspace\logs\request_logs
```

✅ **使用正确的路径**

---

#### 测试3: log status 命令

```bash
$ NEMESISBOT_HOME=/tmp/test-hardcode ./nemesisbot.exe log status
Log Directory:  C:\Users\Zoo\AppData\Local\Temp\test-hardcode\.nemesisbot\workspace\logs\request_logs
```

✅ **显示正确的路径**

---

#### 测试4: 多实例部署

```bash
# 实例 A
$ NEMESISBOT_HOME=/tmp/instance-a ./nemesisbot.exe onboard --skip-all
# workspace: C:\Users\Zoo\AppData\Local\Temp\instance-a\.nemesisbot\workspace
# log_dir: C:\Users\Zoo\AppData\Local\Temp\instance-a\.nemesisbot\workspace\logs\request_logs

# 实例 B
$ NEMESISBOT_HOME=/tmp/instance-b ./nemesisbot.exe onboard --skip-all
# workspace: C:\Users\Zoo\AppData\Local\Temp\instance-b\.nemesisbot\workspace
# log_dir: C:\Users\Zoo\AppData\Local\Temp\instance-b\.nemesisbot\workspace\logs\request_logs
```

✅ **两个实例完全独立**

---

#### 测试5: LocalMode 不受影响

```bash
$ ./nemesisbot.exe --local onboard --skip-all
```

**配置文件**:
```json
{
  "workspace": ".nemesisbot\\workspace",
  "logging": {
    "log_dir": ".nemesisbot\\workspace\\logs\\request_logs"
  }
}
```

✅ **LocalMode 使用相对路径**

---

## 最终目录结构

### 设置 NEMESISBOT_HOME=/opt/custom 后：

```
/opt/custom/                     # ← 您指定的根目录
└── .nemesisbot/                 # ← 项目目录（自动创建）
    ├── config.json              # ← 主配置 ✅
    └── workspace/               # ← 工作区 ✅
        ├── AGENT.md
        ├── config/
        │   ├── config.mcp.json         # ← MCP配置 ✅
        │   └── config.security.json    # ← Security配置 ✅
        └── logs/
            └── request_logs/           # ← LLM日志 ✅
```

**所有路径统一**：
- Config: `/opt/custom/.nemesisbot/config.json`
- Workspace: `/opt/custom/.nemesisbot/workspace`
- MCP Config: `/opt/custom/.nemesisbot/workspace/config/config.mcp.json`
- Security Config: `/opt/custom/.nemesisbot/workspace/config/config.security.json`
- Log Directory: `/opt/custom/.nemesisbot/workspace/logs/request_logs`

---

## 核心改进总结

### 1. 统一路径解析 ✅
```
DefaultConfig() → 使用 path.NewPathManager()
adjustPathsForEnvironment() → 检测并修正硬编码
LoadConfig() → 自动调用路径调整
```

### 2. 配置完整性 ✅
```
所有项目数据都在 .nemesisbot/ 目录下：
- config.json (配置)
- workspace/ (数据、日志、技能等)
- logs/request_logs/ (LLM请求日志)
```

### 3. 多实例支持 ✅
```bash
# 每个实例完全独立
export NEMESISBOT_HOME=/opt/instance1
export NEMESISBOT_HOME=/opt/instance2
export NEMESISBOT_HOME=/opt/instance3
```

### 4. 向后兼容 ✅
```
- 默认行为: ~/.nemesisbot/ (自动修正为动态路径)
- LocalMode: ./.nemesisbot/ (相对路径)
- 自动检测: ./.nemesisbot/ (相对路径)
- 用户自定义路径: 保留不变
```

### 5. LocalMode 优化 ✅
```
- Workspace: .nemesisbot/workspace (相对路径)
- LogDir: .nemesisbot/workspace/logs/request_logs (相对路径)
- 完全自包含，可移植
```

---

## 修复前后对比

### 场景：设置 NEMESISBOT_HOME=/opt/nemesisbot

| 配置项 | 修复前 | 修复后 |
|--------|--------|--------|
| Config | `~/.nemesisbot/config.json` ❌ | `/opt/nemesisbot/.nemesisbot/config.json` ✅ |
| Workspace | `~/.nemesisbot/workspace` ❌ | `/opt/nemesisbot/.nemesisbot/workspace` ✅ |
| LogDir | `~/.nemesisbot/workspace/logs/` ❌ | `/opt/nemesisbot/.nemesisbot/workspace/logs/` ✅ |
| MCP Config | `~/.nemesisbot/config.mcp.json` ❌ | `/opt/nemesisbot/.nemesisbot/workspace/config/config.mcp.json` ✅ |
| Security Config | `~/.nemesisbot/config.security.json` ❌ | `/opt/nemesisbot/.nemesisbot/workspace/config/config.security.json` ✅ |

**修复前**: 路径分散，混乱，难以迁移
**修复后**: 统一在 NEMESISBOT_HOME/.nemesisbot/ 下

---

## 编译验证

```bash
$ go build -o nemesisbot.exe ./nemesisbot
✅ 编译成功 - 27MB 可执行文件
```

---

## 文件清单

### 修改的文件 (3个)
- ✅ `module/config/config.go` - 核心配置逻辑
- ✅ `nemesisbot/main.go` - 主程序入口
- ✅ `nemesisbot/command/log.go` - 日志命令

### 未修改但相关的文件
- `config/config.default.json` - 保持不变，由 adjustPathsForEnvironment() 处理

---

## 验证状态

✅ **所有测试通过**
- 单元测试: 31/31 通过
- 集成测试: 4/4 通过
- **总通过率: 100%**

✅ **编译成功**
- nemesisbot.exe (27MB)

✅ **功能验证**
- onboard 命令正常
- log enable/status 命令正常
- 所有路径使用正确的 NEMESISBOT_HOME
- 多实例部署正常
- LocalMode 不受影响

---

## 总结

### 核心成就

1. ✅ **完全消除了硬编码路径**
   - 所有路径都使用 path.NewPathManager() 动态解析
   - Workspace 和 LogDir 都尊重 NEMESISBOT_HOME

2. ✅ **实现了路径自动修正**
   - 新增 adjustPathsForEnvironment() 方法
   - 在 LoadConfig() 中自动调用
   - 检测并替换所有硬编码的默认路径

3. ✅ **保持了向后兼容**
   - 用户自定义的路径不会被修改
   - LocalMode 正常工作
   - 默认行为不受影响

4. ✅ **支持多实例部署**
   - 每个实例完全独立
   - 所有数据在各自的 .nemesisbot/ 目录下

5. ✅ **项目完整性**
   - Config、Workspace、Logs 都在同一个 .nemesisbot/ 目录
   - 易于迁移和备份

---

**修复完成时间**: 2026-03-04
**验证状态**: ✅ 所有测试通过
**编译状态**: ✅ 成功
**项目状态**: ✅ 可以投入使用

**所有硬编码路径问题已完全修复！** 🎉
