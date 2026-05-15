---
name: build-project
description: 定义 NemesisBot Rust 项目的构建流程，包括环境准备、版本信息收集、编译构建、结果验证等步骤。所有构建工作必须严格按照此流程执行。
---

# NemesisBot Rust 项目构建流程 (Build Project Process)

此 skill 定义了严格的构建流程，确保每次构建都能正确生成可执行文件，并包含完整的版本信息。

## 📋 流程概述

完整的构建流程包含以下阶段：

1. **准备阶段** - 环境和参数准备
2. **信息收集阶段** - 收集版本、Git 提交、构建时间等信息
3. **构建阶段** - 执行 Cargo 编译，生成可执行文件
4. **验证阶段** - 验证构建结果和文件大小
5. **报告阶段** - 生成构建报告

---

## 📖 详细流程说明

### 阶段 1: 准备阶段（环境准备）

**目标**: 确保构建环境正确，准备构建参数。

**执行步骤**:

1. **检查 Rust 环境**
   ```bash
   rustc --version
   cargo --version
   ```
   - 确认 Rust 工具链正确安装
   - 确保 cargo 在 PATH 中

2. **检查项目路径**
   - 当前目录必须在项目根目录 `C:\AI\NemesisBot\NemesisBot_Rust`
   - 确认 `Cargo.toml`（workspace 根配置）存在
   - 确认 `nemesisbot/` 目录存在

3. **确定输出文件名**
   - 默认: `nemesisbot.exe`
   - 可选: 用户指定自定义名称

**输出**: 构建环境就绪

---

### 阶段 2: 信息收集阶段（构建信息）

**目标**: 收集所有构建所需的版本信息。

**执行步骤**:

1. **获取版本号**
   ```bash
   git describe --tags --abbrev=0
   ```
   - 从 Git 标签获取版本号
   - 如果没有标签，使用默认值: `0.0.0.1`

2. **获取 Git 提交哈希**
   ```bash
   git rev-parse --short HEAD
   ```
   - 获取短格式的 Git 提交哈希
   - 用于追踪具体构建版本

3. **获取构建时间**
   ```bash
   powershell -Command "Get-Date -Format yyyy/MM/dd"
   ```
   - 格式: `2026/03/05`

4. **获取 Rust 版本**
   ```bash
   rustc --version
   ```
   - 提取版本号（如 `1.85.0`）

5. **显示构建信息**
   ```
   Build Information:
     - Version:     0.0.0.1
     - Git Commit:  abc1234
     - Build Time:  2026/03/05
     - Rust Version: 1.85.0
   ```

**输出**: 构建信息字典

---

### 阶段 3: 构建阶段（编译生成）

**目标**: 执行 Cargo 编译，生成可执行文件。

**执行步骤**:

1. **选择编译方式**

   **方式 A: 使用构建脚本（推荐，自动注入版本信息）**
   ```bash
   # Windows
   build.bat
   build.bat mybot.exe           # 指定输出文件名
   build.bat powershell          # 使用 PowerShell 编译

   # Linux/macOS
   ./build.sh
   ```

   构建脚本会自动：
   - 从 git tag 提取版本号（没有则用 `0.0.0.1`）
   - 提取 git commit hash
   - 通过环境变量注入版本信息
   - 编译并复制/重命名输出文件

   **方式 B: 使用 Cargo 直接编译**
   ```bash
   # Release 构建（推荐，用于发布）
   cargo build --release -p nemesisbot

   # 开发构建（更快编译，用于调试）
   cargo build -p nemesisbot
   ```

2. **检查编译结果**
   - 如果失败，立即报告错误
   - 如果成功，显示确认信息

3. **输出文件位置**
   - Cargo 输出: `target/release/nemesisbot.exe`
   - 构建脚本输出: 项目根目录下的 `nemesisbot.exe`（或自定义名称）

**输出**: 可执行文件（nemesisbot.exe 或自定义名称）

---

### 阶段 4: 验证阶段（结果确认）

**目标**: 验证构建结果是否正确。

**执行步骤**:

1. **检查文件存在性**
   ```bash
   ls -la nemesisbot.exe
   # 或（如果使用 Cargo 直接编译）
   ls -la target/release/nemesisbot.exe
   ```

2. **获取文件大小**
   ```bash
   # 使用 Bash
   stat -c %s nemesisbot.exe | awk '{printf "%.0f MB\n", $1/1048576}'
   ```

3. **显示构建摘要**
   ```
   ============================================
   Build Summary
   ============================================
   Output file: nemesisbot.exe
   File size: 15 MB

   Build Info:
     Version:     0.0.0.1
     Git Commit:  abc1234
     Build Time:  2026/03/05
     Rust Version: 1.85.0

   [SUCCESS] Build completed successfully!

   You can now run: .\nemesisbot.exe gateway
   ============================================
   ```

**输出**: 构建验证报告

---

### 阶段 5: 报告阶段（生成报告）

**目标**: 向用户汇报构建结果。

**执行步骤**:

1. **汇总构建信息**
   - 版本号
   - Git 提交哈希
   - 构建时间
   - Rust 版本
   - 文件大小

2. **确认构建状态**
   - ✅ 成功：可执行文件已生成
   - ❌ 失败：报告错误信息

3. **提供后续指导**
   ```
   构建成功！你可以运行：

   # 启动 Web 网关
   .\nemesisbot.exe gateway

   # 使用本地模式
   .\nemesisbot.exe --local gateway

   # 查看帮助
   .\nemesisbot.exe --help
   ```

**输出**: 构建报告 + 用户指导

---

## 🛠️ 命令参考

### 完整构建命令

**使用构建脚本（推荐）**:
```batch
build.bat
```

### 使用 Cargo 直接构建

```bash
# Release 构建
cargo build --release -p nemesisbot

# 开发构建
cargo build -p nemesisbot

# 构建 workspace 所有 crate
cargo build --workspace --release
```

### 指定输出文件名

```batch
build.bat my-bot.exe
```

---

## 📁 项目结构

```
NemesisBot_Rust/
├── Cargo.toml            # Workspace 根配置
├── build.bat             # 构建脚本（Windows，带版本注入）
├── build.sh              # 构建脚本（Linux/macOS）
├── crates/               # 核心模块（34 个 crate）
│   ├── nemesis-agent/    # Agent 引擎
│   ├── nemesis-bus/      # 消息总线
│   ├── nemesis-channels/ # 通道实现
│   ├── nemesis-cluster/  # 集群编排
│   ├── nemesis-security/ # 安全中间件
│   ├── nemesis-tools/    # 工具实现
│   └── ...               # 其他 crate
├── plugins/              # 插件
│   ├── plugin-onnx/      # ONNX 嵌入模型
│   └── plugin-ui/        # WebView UI 插件
├── nemesisbot/           # 主程序入口
│   ├── src/main.rs       # CLI 入口（clap）
│   └── src/commands/     # CLI 命令实现
├── test-tools/           # 测试工具
└── target/               # 编译输出（Cargo 自动生成）
    └── release/
        └── nemesisbot.exe
```

---

## ⚠️ 重要原则

### 必须遵守的原则

1. **环境检查**
   - 必须确保 Rust 工具链正确安装
   - 必须在项目根目录执行构建

2. **版本信息**
   - 推荐使用 build.bat/build.sh 自动注入版本信息
   - Git 提交哈希必须正确获取

3. **错误处理**
   - 构建失败必须立即报告
   - 不能忽略编译错误

4. **验证完整**
   - 必须验证输出文件存在
   - 必须检查文件大小合理

5. **用户友好**
   - 必须显示清晰的构建信息
   - 必须提供后续运行指导

### 禁止事项

❌ **禁止**在未配置环境的情况下构建
❌ **禁止**忽略编译错误继续执行
❌ **禁止**不验证输出文件就报告成功
❌ **禁止**在错误的目录执行构建

---

## 🧪 验证检查清单

在向用户报告构建成功前，确认以下事项：

- [ ] Rust 工具链配置正确
- [ ] 项目路径正确
- [ ] 版本信息已正确收集
- [ ] Git 提交哈希已获取
- [ ] 构建时间已获取
- [ ] Rust 版本已获取
- [ ] 编译命令执行成功
- [ ] 输出文件已生成
- [ ] 文件大小在合理范围（10-50 MB）
- [ ] 构建摘要已显示
- [ ] 后续运行指导已提供

**只有所有检查项都完成，构建才算真正成功！**

---

## 📚 相关资源

- 项目根目录：`C:\AI\NemesisBot\NemesisBot_Rust`
- 构建脚本：`C:\AI\NemesisBot\NemesisBot_Rust\build.bat`
- Workspace 配置：`C:\AI\NemesisBot\NemesisBot_Rust\Cargo.toml`
- 主程序目录：`C:\AI\NemesisBot\NemesisBot_Rust\nemesisbot`

---

## 🎯 使用指南

### 当用户要求构建项目时

1. **第一步**: 确认当前目录是否在项目根目录
2. **第二步**: 检查 Rust 环境是否配置正确
3. **第三步**: 收集构建所需的版本信息
4. **第四步**: 执行编译命令（推荐使用 build.bat）
5. **第五步**: 验证构建结果
6. **第六步**: 生成构建报告并汇报

### 示例对话

```
用户: 帮我构建项目

助手: 收到。我将开始构建流程。

📋 阶段 1: 准备阶段
检查 Rust 环境...
rustc: 1.85.0
cargo: 1.85.0
✅ 环境检查通过

📋 阶段 2: 信息收集阶段
收集构建信息...
  - Version:     0.0.0.1
  - Git Commit:  abc1234
  - Build Time:  2026/03/05
  - Rust Version: 1.85.0
✅ 信息收集完成

📋 阶段 3: 构建阶段
执行编译命令...
cargo build --release -p nemesisbot
✅ 编译成功

📋 阶段 4: 验证阶段
验证构建结果...
  - 输出文件: target/release/nemesisbot.exe
  - 文件大小: 15 MB
✅ 验证通过

📋 阶段 5: 报告阶段
构建成功！你可以运行：
 .\target\release\nemesisbot.exe gateway
```

---

## 🔧 版本注入说明

构建脚本（build.bat/build.sh）通过环境变量注入版本信息：

- `NEMESISBOT_VERSION`: 版本号（从 git tag 提取）
- `NEMESISBOT_GIT_COMMIT`: Git 提交哈希
- `NEMESISBOT_BUILD_TIME`: 构建时间

这些环境变量在编译时通过 `option_env!()` 宏在 `nemesisbot/src/main.rs` 中读取，并嵌入到二进制文件中。

---

## ✅ 快速构建命令

```batch
# 默认构建（推荐，带版本注入）
build.bat

# 指定输出文件名
build.bat my-custom-name.exe

# 使用 PowerShell 编译
build.bat powershell

# 使用 Cargo 直接编译（无版本注入）
cargo build --release -p nemesisbot
```

---

## 📝 构建日志示例

```
============================================
NemesisBot Build Script
============================================

[INFO] Output filename specified: nemesisbot.exe

[INFO] Gathering build information...

Build Information:
  - Version:     0.0.0.1
  - Git Commit:  abc1234
  - Build Time:  2026/03/05
  - Rust Version: 1.85.0

[Step 1/3] Nothing

[Step 2/3] Building nemesisbot.exe...
   Compiling nemesisbot v0.0.0.1 (...)

[OK] Build completed successfully

[Step 3/3] Nothing

============================================
Build Summary
============================================
Output file: nemesisbot.exe
File size: 15 MB

Build Info:
  Version:     0.0.0.1
  Git Commit:  abc1234
  Build Time:  2026/03/05
  Rust Version: 1.85.0

[SUCCESS] Build completed successfully!

You can now run: .\nemesisbot.exe gateway
============================================
```

---

构建流程定义完成！所有构建工作必须严格按照此流程执行，确保每次构建都包含完整的版本信息，并且输出文件可正常使用。
