# `onboard default` 配置生成流程分析报告

## 执行日期
2026-02-25

---

## 一、问题概述

用户询问：运行 `nemesisbot onboard default` 后，`~/.nemesisbot/config.json` 是如何生成的？
它与 `config/config.default.json` 是否一致？是否存在问题？

---

## 二、配置生成流程

### 2.1 命令执行流程

```
用户执行: nemesisbot onboard default
    ↓
[nemesisbot/main.go:52] 检测到 "onboard" 参数
    ↓
[nemesisbot/main.go:53] 检查是否有 "default" 子参数
    ↓
[nemesisbot/main.go:177] 调用 onboardDefault() 函数
    ↓
[module/config/config.go:189] 调用 config.DefaultConfig()
    ↓
[main.go:190] 调用 config.SaveConfig() 保存到 ~/.nemesisbot/config.json
```

### 2.2 配置来源

**关键发现**：存在**两套默认配置**：

| 配置来源 | 位置 | 使用时机 |
|---------|------|---------|
| **config/config.default.json** | 源文件 | `LoadConfig()` 函数优先使用 |
| **config.DefaultConfig()** | Go 代码硬编码 | `onboardDefault` 命令使用 |

---

## 三、配置差异分析

### 3.1 `config/config.default.json` 内容

```json
{
  "agents": {
    "defaults": {
      "restrict_to_workspace": true,    ← 设置为 true
      ...
    }
  },
  "channels": {
    "web": {
      "auth_token": "",                   ← 空字符串
      ...
    }
  },
  "security": {
    "enabled": false                    ← 禁用安全模块
  },
  "logging": null                        ← null 或不存在
}
```

### 3.2 `onboardDefault` 生成的配置（`config.DefaultConfig()` + 修改）

```go
// 第 1 步：基础配置（config.DefaultConfig()）
{
  "agents": {
    "defaults": {
      "restrict_to_workspace": true,
      ...
    }
  },
  "channels": {
    "web": {
      "enabled": true,
      "auth_token": "",
      ...
    }
  },
  "security": {
    "enabled": false                   ← 初始状态 false
  },
  "logging": null                        ← 初始状态 null
}

// 第 2 步：onboardDefault() 额外修改
{
  "logging": {                          ← 新增字段
    "llm_requests": true,                ← 启用 LLM 日志记录
    "log_dir": "~/.nemesisbot/workspace/logs/request_logs",
    "detail_level": "full"
  }

// 第 3 步：再次修改
{
  "security": {
    "enabled": true                     ← 修改为启用！
  },
  "agents": {
    "defaults": {
      "restrict_to_workspace": false    ← 修改为 false！
    }
  }
}
```

---

## 四、发现的问题

### ❌ 问题 1：两套配置不一致

**问题**：
- `config/config.default.json` 中 `security.enabled = false`
- `onboardDefault()` 生成的配置中 `security.enabled = true`

**影响**：
- 用户查看 `config/config.default.json` 时会看到错误的默认值
- 文档说明与实际行为不符

### ❌ 问题 2：配置行为不符合预期

**问题**：
- `onboardDefault` 模式下会**修改安全配置**
  - `security.enabled` 从 `false` 变为 `true`
  - `restrict_to_workspace` 从 `true` 变为 `false`

**影响**：
- 用户运行 `onboard default` 期望得到"默认配置"
- 实际得到的是"修改过的配置"
- 这违反了"默认"的语义

### ❌ 问题 3：文档误导

**问题**：
- `config/config.default.json` 作为默认配置参考
- 但 `onboard default` 命令使用的不是它

**影响**：
- 用户根据 `config/config.default.json` 修改配置
- 但运行 `onboard default` 后被覆盖

---

## 五、根本原因分析

### 5.1 架构设计问题

```go
// LoadConfig() 函数（第578行）
// 优先级：文件 > embedded default > DefaultConfig()

// onboardDefault() 函数（第189行）
// 直接调用 DefaultConfig()，跳过 embedded default
cfg := config.DefaultConfig()  // ❌ 不使用 config/config.default.json
```

### 5.2 配置初始化优先级混乱

```
1. config/config.default.json（源文件）
2. //go:embed config（嵌入到二进制）
3. embeddedDefaults.config（运行时读取）
4. config.DefaultConfig()（硬编码 Go 结构体）

┌─────────────────────────────────────────┐
│  当前优先级                                │
│  LoadConfig()：     1 > 2 > 4            │
│  onboardDefault():  4（跳过 1,2,3）      │
└─────────────────────────────────────────┘
```

### 5.3 设计意图分析

根据 `onboardDefault()` 的代码注释（第177行）：

```go
// onboardDefault initializes NemesisBot with default settings for quick start
```

**设计意图**：
- `onboard default` 应该提供"快速启动"的配置
- 启用 LLM 日志、安全模块等功能
- 与"最小默认配置"区分开

**问题**：
- 命名 `onboard default` 暗示使用"默认配置"
- 实际使用的是"优化后的配置"
- 语义不清晰

---

## 六、对比表格

### 6.1 配置差异对比

| 配置项 | config.default.json | DefaultConfig() | onboardDefault() 后 |
|--------|-------------------|-----------------|-------------------|
| `restrict_to_workspace` | true | true | **false** ❗ |
| `security.enabled` | false | false | **true** ❗ |
| `logging.llm_requests` | N/A | false | **true** ❗ |
| `logging.log_dir` | N/A | N/A | ~/.nemesisbot/workspace/logs/request_logs |
| `logging.detail_level` | N/A | N/A | full |
| `web.auth_token` | "" | "" | "" |

❗ = 与 config/config.default.json 不一致

### 6.2 命令对比

| 命令 | 配置来源 | 安全模块 | 工作区限制 |
|------|---------|---------|-----------|
| `onboard` | 使用 embedded default | 禁用 | 启用 |
| `onboard default` | 使用 DefaultConfig() | **启用** | **禁用** ⚠️ |
| 手动创建 config.json | 使用 embedded default | 禁用 | 启用 |

---

## 七、潜在影响

### 7.1 安全影响

**危险**：`onboard default` 禁用了 `restrict_to_workspace`

```
restrict_to_workspace: false
```

这意味着：
- ❌ Agent 可以访问整个文件系统
- ❌ 没有"工作目录隔离"保护
- ⚠️  依赖 `security.enabled` 的规则来保护

### 7.2 用户体验影响

**困惑**：用户期望"默认" = "最小/安全"，实际得到"优化/宽松"

### 7.3 维护性问题

**双重默认**：
- `config/config.default.json` 需要维护
- `config.DefaultConfig()` 硬编码需要维护
- 容易出现不同步

---

## 八、建议的解决方案

### 方案 A：统一为嵌入式配置（推荐）

**目标**：所有命令使用同一套配置

**修改**：
```go
func onboardDefault() {
    // 使用 embedded default 而不是硬编码
    cfg, err := config.LoadConfig(configPath)
    if err != nil {
        if os.IsNotExist(err) {
            cfg = config.LoadEmbeddedConfig() // 新函数
        } else {
            fmt.Printf("Error loading config: %v\n", err)
            os.Exit(1)
        }
    }
    // 然后应用 "default" 模式的额外配置...
}
```

**优点**：
- ✅ 单一配置源
- ✅ `config/config.default.json` 成为唯一的默认配置
- ✅ 符合用户预期

### 方案 B：重命名命令（语义化）

**修改**：
- `onboard default` → `onboard optimized` 或 `onboard quickstart`
- `onboard` → `onboard minimal` 或 `onboard secure`

**优点**：
- ✅ 命令名称反映实际行为
- ✅ 避免"默认"的歧义

### 方案 C：添加配置验证

**修改**：
- 启动时验证 `config.default.json` 与 `DefaultConfig()` 的一致性
- 如果不一致，显示警告或错误

---

## 九、总结

### 核心问题

**`onboard default` 命令生成的配置与 `config/config.default.json` 不一致**

### 具体差异

1. **安全模块**：`onboard default` 启用了，config.default.json 禁用
2. **工作区限制**：`onboard default` 禁用了，config.default.json 启用
3. **LLM 日志**：`onboard default` 启用了，config.default.json 没有

### 风险等级

🟡 **中等风险**

- `restrict_to_workspace: false` 可能导致安全风险
- 配置不一致导致用户困惑
- 文档与实际行为不符

### 建议

**立即**：
1. 文档中明确说明 `onboard default` 的特殊行为
2. 在 `onboard default` 输出中添加警告提示

**长期**：
1. 统一配置源（推荐方案 A）
2. 重命名命令以反映实际行为（方案 B）
3. 添加配置一致性检查（方案 C）

---

**报告人**：Claude Sonnet 4.5
**分析日期**：2026-02-25
