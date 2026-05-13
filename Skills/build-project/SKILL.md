---
name: build-project
description: 定义 NemesisBot 项目的构建流程，包括环境准备、版本信息收集、编译构建、结果验证等步骤。所有构建工作必须严格按照此流程执行。
---

# NemesisBot 项目构建流程 (Build Project Process)

此 skill 定义了严格的构建流程，确保每次构建都能正确生成可执行文件，并包含完整的版本信息。

## 📋 流程概述

完整的构建流程包含以下阶段：

1. **准备阶段** - 环境和参数准备
2. **信息收集阶段** - 收集版本、Git 提交、构建时间等信息
3. **构建阶段** - 执行 Go 编译，生成可执行文件
4. **验证阶段** - 验证构建结果和文件大小
5. **报告阶段** - 生成构建报告

---

## 📖 详细流程说明

### 阶段 1: 准备阶段（环境准备）

**目标**: 确保构建环境正确，准备构建参数。

**执行步骤**:

1. **检查 Go 环境**
   ```bash
   go version
   ```
   - 确认 Go 版本正确（项目使用 go1.25.7）
   - 确保 GOPATH 和 GOROOT 配置正确

2. **检查项目路径**
   - 当前目录必须在项目根目录 `C:\AI\NemesisBot\NemesisBot`
   - 确认 nemesisbot/ 目录存在

3. **确定输出文件名**
   - 默认: `nemesisbot.exe`
   - 可选: 用户指定自定义名称

4. **设置 PATH 环境变量**
   - 确保 go.exe 存在，若不存在则寻找 go.exe
   - 找到了 go.exe 的路径后，设置环境变量
   ```batch
   SET PATH=C:\AI\golang\go1.25.7\bin;%PATH%
   ```
   - 确保 Go 编译器在 PATH 中

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

4. **获取 Go 版本**
   ```bash
   go version
   ```
   - 提取版本号（如 `go1.25.7`）

5. **显示构建信息**
   ```
   Build Information:
     - Version:    0.0.0.1
     - Git Commit: abc1234
     - Build Time: 2026/03/05
     - Go Version: go1.25.7
   ```

**输出**: 构建信息字典

---

### 阶段 3: 构建阶段（编译生成）

**目标**: 执行 Go 编译，生成可执行文件。

**执行步骤**:

1. **设置编译参数（ldflags）**
   ```bash
   -ldflags "-X main.version=%VERSION%
             -X main.gitCommit=%GIT_COMMIT%
             -X main.buildTime=%BUILD_TIME%
             -X main.goVersion=%GO_VERSION%
             -s -w"
   ```
   - `-s`: 去除符号表
   - `-w`: 去除调试信息
   - `-X`: 注入版本信息到 main 包

2. **执行编译命令**
   ```bash
   go build -ldflags "..." -o %OUTPUT_NAME% .\nemesisbot\
   ```
   - 默认输出: `nemesisbot.exe`
   - 可自定义输出文件名

3. **检查编译结果**
   - 如果失败，立即报告错误
   - 如果成功，显示确认信息

**输出**: 可执行文件（nemesisbot.exe 或自定义名称）

---

### 阶段 4: 验证阶段（结果确认）

**目标**: 验证构建结果是否正确。

**执行步骤**:

1. **检查文件存在性**
   ```batch
   if exist "%OUTPUT_NAME%" (
       echo 文件存在
   ) else (
       echo 文件不存在
   )
   ```

2. **获取文件大小**
   ```batch
   for %%A in ("%OUTPUT_NAME%") do (
       set size=%%~zA
       set /a sizeMB=!size! / 1048576
       echo 文件大小: !sizeMB! MB
   )
   ```

3. **显示构建摘要**
   ```
   ============================================
   Build Summary
   ============================================
   Output file: nemesisbot.exe
   File size: 15 MB

   Build Info:
     Version:    0.0.0.1
     Git Commit: abc1234
     Build Time: 2026/03/05
     Go Version: go1.25.7

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
   - Go 版本
   - 文件大小

2. **确认构建状态**
   - ✅ 成功：可执行文件已生成
   - ❌ 失败：报告错误信息

3. **提供后续指导**
   ```
   构建成功！你可以运行：

   # 启动 Web 网关
   .\nemesisbot.exe gateway

   # 查看帮助
   .\nemesisbot.exe --help

   # 启动特定 agent
   .\nemesisbot.exe agent
   ```

**输出**: 构建报告 + 用户指导

---

## 🛠️ 命令参考

### 完整构建命令

```batch
@echo off
setlocal enabledelayedexpansion

SET PATH=C:\AI\golang\go1.25.7\bin;%PATH%

# 默认输出文件名
if "%~1"=="" (
    set OUTPUT_NAME=nemesisbot.exe
) else (
    set OUTPUT_NAME=%~1
)

# 获取构建信息
set VERSION=0.0.0.1
for /f "tokens=*" %%i in ('git describe --tags --abbrev=0 2^>nul') do set VERSION=%%i

set GIT_COMMIT=unknown
for /f "tokens=*" %%i in ('git rev-parse --short HEAD 2^>nul') do set GIT_COMMIT=%%i

for /f "delims=" %%i in ('powershell -Command "Get-Date -Format yyyy/MM/dd"') do set BUILD_TIME=%%i

for /f "tokens=3" %%i in ('go version') do set GO_VERSION_RAW=%%i
set GO_VERSION=%GO_VERSION_RAW:go=%

# 显示构建信息
echo Build Information:
echo   - Version:    %VERSION%
echo   - Git Commit: %GIT_COMMIT%
echo   - Build Time: %BUILD_TIME%
echo   - Go Version: %GO_VERSION%

# 执行编译
echo.
echo [Step 2/3] Building %OUTPUT_NAME%...
go build -ldflags "-X main.version=%VERSION% -X main.gitCommit=%GIT_COMMIT% -X main.buildTime=%BUILD_TIME% -X main.goVersion=%GO_VERSION% -s -w" -o %OUTPUT_NAME% .\nemesisbot\

if errorlevel 1 (
    echo [ERROR] Build failed!
    pause
    exit /b 1
)

echo [OK] Build completed successfully

# 显示文件大小
for %%A in ("%OUTPUT_NAME%") do (
    set size=%%~zA
    set /a sizeMB=!size! / 1048576
    echo File size: !sizeMB! MB
)

echo.
echo [SUCCESS] Build completed successfully!
echo You can now run: .\%OUTPUT_NAME% gateway
```

### 快速构建（使用默认参数）

```batch
build.bat
```

### 指定输出文件名

```batch
build.bat my-bot.exe
```

---

## 📁 项目结构

```
NemesisBot/
├── build.bat              # 构建脚本
├── nemesisbot/            # 主程序目录
│   ├── main.go           # 入口文件
│   ├── command/          # 命令处理
│   └── ...
├── module/               # 核心模块
│   ├── agent/
│   ├── channels/
│   ├── cluster/
│   ├── config/
│   ├── providers/
│   ├── security/
│   └── tools/
└── nemesisbot.exe        # 构建输出（运行后生成）
```

---

## ⚠️ 重要原则

### 必须遵守的原则

1. **环境检查**
   - 必须确保 Go 环境正确配置
   - 必须在项目根目录执行构建

2. **版本信息**
   - 必须注入正确的版本信息
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
❌ **禁止**使用错误的 ldflags 参数
❌ **禁止**在错误的目录执行构建

---

## 🧪 验证检查清单

在向用户报告构建成功前，确认以下事项：

- [ ] Go 环境配置正确
- [ ] 项目路径正确
- [ ] 版本信息已正确收集
- [ ] Git 提交哈希已获取
- [ ] 构建时间已获取
- [ ] Go 版本已获取
- [ ] 编译命令执行成功
- [ ] 输出文件已生成
- [ ] 文件大小在合理范围（10-50 MB）
- [ ] 构建摘要已显示
- [ ] 后续运行指导已提供

**只有所有检查项都完成，构建才算真正成功！**

---

## 📚 相关资源

- 项目根目录：`C:\AI\NemesisBot\NemesisBot`
- 构建脚本：`C:\AI\NemesisBot\NemesisBot\build.bat`
- Go 环境路径：`C:\AI\golang\go1.25.7\bin`
- 主程序目录：`C:\AI\NemesisBot\NemesisBot\nemesisbot`

---

## 🎯 使用指南

### 当用户要求构建项目时

1. **第一步**: 确认当前目录是否在项目根目录
2. **第二步**: 检查 Go 环境是否配置正确
3. **第三步**: 收集构建所需的版本信息
4. **第四步**: 执行编译命令
5. **第五步**: 验证构建结果
6. **第六步**: 生成构建报告并汇报

### 示例对话

```
用户: 帮我构建项目

助手: 收到。我将开始构建流程。

📋 阶段 1: 准备阶段
检查 Go 环境...
go version: go version go1.25.7 windows/amd64
✅ 环境检查通过

📋 阶段 2: 信息收集阶段
收集构建信息...
  - Version:    0.0.0.1
  - Git Commit: abc1234
  - Build Time: 2026/03/05
  - Go Version: go1.25.7
✅ 信息收集完成

📋 阶段 3: 构建阶段
执行编译命令...
go build -ldflags "-X main.version=0.0.0.1 -X main.gitCommit=abc1234 ..." -o nemesisbot.exe .\nemesisbot\
✅ 编译成功

📋 阶段 4: 验证阶段
验证构建结果...
  - 输出文件: nemesisbot.exe
  - 文件大小: 15 MB
✅ 验证通过

📋 阶段 5: 报告阶段
构建成功！你可以运行：
  .\nemesisbot.exe gateway
```

---

## 🔧 版本注入说明

构建过程中注入的版本信息可在程序中访问：

```go
package main

var (
    version    string  // 版本号
    gitCommit  string  // Git 提交哈希
    buildTime  string  // 构建时间
    goVersion  string  // Go 版本
)
```

这些变量可以在程序运行时显示，帮助用户了解当前使用的具体版本。

---

## ✅ 快速构建命令

```batch
# 默认构建
build.bat

# 指定输出文件名
build.bat my-custom-name.exe
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
  - Version:    0.0.0.1
  - Git Commit: abc1234
  - Build Time: 2026/03/05
  - Go Version: go1.25.7

[Step 1/3] Nothing

[Step 2/3] Building nemesisbot.exe...

[OK] Build completed successfully

[Step 3/3] Nothing

============================================
Build Summary
============================================
Output file: nemesisbot.exe
File size: 15 MB

Build Info:
  Version:    0.0.0.1
  Git Commit: abc1234
  Build Time: 2026/03/05
  Go Version: go1.25.7

[SUCCESS] Build completed successfully!

You can now run: .\nemesisbot.exe gateway
============================================
```

---

构建流程定义完成！所有构建工作必须严格按照此流程执行，确保每次构建都包含完整的版本信息，并且输出文件可正常使用。
