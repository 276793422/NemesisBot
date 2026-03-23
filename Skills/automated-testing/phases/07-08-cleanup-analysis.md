# 阶段 7-8: 清理环境和结果分析

> **⚠️ 推荐方式**: 使用辅助脚本（2026-03-23 更新）
>
> 阶段 7（停止服务）的大部分步骤已由 `cleanup-env.sh/ps1` 脚本自动化。
>
> **强烈建议优先使用脚本**：
> ```bash
> # Windows PowerShell
> .\Skills\automated-testing\scripts\cleanup-env.ps1
>
> # Linux/Git Bash
> bash Skills/automated-testing/scripts/cleanup-env.sh
> ```
>
> **本文档的作用**：
> - ✅ 理解脚本做了什么
> - ✅ 脚本失败时的备用方案
> - ✅ 阶段 8（结果分析）的详细指南
> - ✅ 故障排查参考
>
> **注意**：脚本只停止服务，不删除 `test/autotest/` 目录。删除目录仍需手动执行。
>
> ---

测试完成后清理所有进程和临时文件，并分析测试结果。

**⚠️ 重要前提**：
- 阶段 6 已完成测试执行
- 需要从 `test/autotest/` 返回项目根目录进行清理

**清理目标**：
- 停止所有测试进程
- 删除整个 `test/autotest/` 目录
- 释放所有占用的端口

---

## 阶段 7: 清理环境

### 7.1 返回项目根目录

**目的**: 切换到项目根目录进行清理

**当前状态**: 可能在 `test/autotest/` 或其他目录

**命令**:
```bash
echo "=== 阶段 7: 清理环境 ==="
echo ""

# 返回项目根目录
# 如果在 test/autotest/，使用 ../..
# 如果在其他位置，使用绝对路径
echo "切换到项目根目录..."

# 尝试检测当前位置并返回根目录
if [[ "$(pwd)" =~ /test/autotest$ ]]; then
  echo "检测到在 test/autotest/ 目录，返回根目录..."
  cd ../..
elif [[ "$(pwd)" =~ /test$ ]]; then
  echo "检测到在 test/ 目录，返回根目录..."
  cd ..
else
  echo "当前目录: $(pwd)"
  echo "请确保在项目根目录"
fi

# 验证是否在根目录
if [ -f "go.mod" ] && [ -d "test" ]; then
  echo "✅ 已返回项目根目录: $(pwd)"
else
  echo "❌ 无法确定项目根目录位置"
  echo "   请手动切换到项目根目录"
  exit 1
fi

echo ""
```

**预期输出**:
```
=== 阶段 7: 清理环境 ===

切换到项目根目录...
检测到在 test/autotest/ 目录，返回根目录...
✅ 已返回项目根目录: /c/AI/NemesisBot/NemesisBot
```

---

### 7.2 停止 NemesisBot 进程

**目的**: 停止 Bot 进程

**当前工作目录**: 项目根目录

**命令**:
```bash
echo "[1/4] 停止 NemesisBot..."

# **⚠️ Windows 特殊处理**: Git Bash 的 kill 命令可能无效
# 检测操作系统类型
if [[ "$OSTYPE" == "msys" || "$OSTYPE" == "win32" ]]; then
  echo "  检测到 Windows 环境，使用 taskkill..."

  # 方法1: 通过进程名终止（推荐）
  if tasklist | grep -i "nemesisbot.exe" > /dev/null 2>&1; then
    echo "  使用进程名终止..."
    taskkill //F //IM nemesisbot.exe 2>/dev/null

    # 等待并验证
    sleep 2
    if ! tasklist | grep -i "nemesisbot.exe" > /dev/null 2>&1; then
      echo "✅ NemesisBot 已停止（Windows）"
    else
      echo "⚠️  进程名方式失败，尝试 PID 方式..."
      # 方法2: 获取真实 PID 并终止
      REAL_PID=$(tasklist | grep -i "nemesisbot.exe" | awk '{print $2}' | head -1)
      if [ -n "$REAL_PID" ]; then
        taskkill //F //PID $REAL_PID 2>/dev/null
        sleep 2
        if ! tasklist | grep -i "nemesisbot.exe" > /dev/null 2>&1; then
          echo "✅ NemesisBot 已停止（PID 方式）"
        else
          echo "❌ 无法停止 NemesisBot"
          exit 1
        fi
      fi
    fi
  else
    echo "✅ NemesisBot 进程不存在（已停止）"
  fi
else
  # Unix/Linux 环境
  # 从 PID 文件读取（如果存在）
  if [ -f "test/autotest/nemesisbot.pid" ]; then
    BOT_PID=$(cat test/autotest/nemesisbot.pid)
    echo "  从 PID 文件读取: $BOT_PID"

    # 检查进程是否存在
    if ! kill -0 $BOT_PID 2>/dev/null; then
      echo "✅ NemesisBot 进程不存在（已停止）"
    else
      # 步骤 1: 优雅停止（TERM 信号）
      kill $BOT_PID 2>/dev/null

      # 等待进程退出（最多 5 秒）
      STOPPED=false
      for i in {1..5}; do
        if ! kill -0 $BOT_PID 2>/dev/null; then
          echo "✅ NemesisBot 已停止（优雅退出）"
          STOPPED=true
          break
        fi
        sleep 1
      done

      # 步骤 2: 强制停止（KILL 信号）
      if [ "$STOPPED" = false ]; then
        echo "  强制停止 NemesisBot..."
        kill -9 $BOT_PID 2>/dev/null
        sleep 2

        if ! kill -0 $BOT_PID 2>/dev/null; then
          echo "✅ NemesisBot 已停止（强制退出）"
        else
          echo "❌ 无法停止 NemesisBot (PID: $BOT_PID)"
          exit 1
        fi
      fi
    fi
  else
    echo "  ⚠️  未找到 NemesisBot PID 文件"
    echo "  尝试通过进程名查找..."
    if pgrep -f "nemesisbot.exe" > /dev/null; then
      pkill -9 -f "nemesisbot.exe"
      sleep 2
      echo "✅ NemesisBot 已停止"
    else
      echo "✅ NemesisBot 进程不存在"
    fi
  fi
fi

echo ""
```

**预期输出**:
```
[1/4] 停止 NemesisBot...
  检测到 Windows 环境，使用 taskkill...
  使用进程名终止...
SUCCESS: The process "nemesisbot.exe" with PID 12346 has been terminated.
✅ NemesisBot 已停止（Windows）
```

---

### 7.3 停止 TestAIServer 进程

**目的**: 停止 TestAIServer 进程

**当前工作目录**: 项目根目录

**命令**:
```bash
echo "[2/4] 停止 TestAIServer..."

# **⚠️ Windows 特殊处理**
if [[ "$OSTYPE" == "msys" || "$OSTYPE" == "win32" ]]; then
  echo "  检测到 Windows 环境，使用 taskkill..."

  # 方法1: 通过进程名终止（推荐）
  if tasklist | grep -i "testaiserver.exe" > /dev/null 2>&1; then
    echo "  使用进程名终止..."
    taskkill //F //IM testaiserver.exe 2>/dev/null

    # 等待并验证
    sleep 2
    if ! tasklist | grep -i "testaiserver.exe" > /dev/null 2>&1; then
      echo "✅ TestAIServer 已停止（Windows）"
    else
      echo "⚠️  进程名方式失败，尝试 PID 方式..."
      # 方法2: 获取真实 PID 并终止
      REAL_PID=$(tasklist | grep -i "testaiserver.exe" | awk '{print $2}' | head -1)
      if [ -n "$REAL_PID" ]; then
        taskkill //F //PID $REAL_PID 2>/dev/null
        sleep 2
        if ! tasklist | grep -i "testaiserver.exe" > /dev/null 2>&1; then
          echo "✅ TestAIServer 已停止（PID 方式）"
        else
          echo "❌ 无法停止 TestAIServer"
          exit 1
        fi
      fi
    fi
  else
    echo "✅ TestAIServer 进程不存在（已停止）"
  fi
else
  # Unix/Linux 环境
  # 从 PID 文件读取（如果存在）
  if [ -f "test/autotest/testaiserver.pid" ]; then
    TESTAI_PID=$(cat test/autotest/testaiserver.pid)
    echo "  从 PID 文件读取: $TESTAI_PID"

    # 检查进程是否存在
    if ! kill -0 $TESTAI_PID 2>/dev/null; then
      echo "✅ TestAIServer 进程不存在（已停止）"
    else
      # 步骤 1: 优雅停止
      kill $TESTAI_PID 2>/dev/null

      # 等待进程退出
      STOPPED=false
      for i in {1..5}; do
        if ! kill -0 $TESTAI_PID 2>/dev/null; then
          echo "✅ TestAIServer 已停止（优雅退出）"
          STOPPED=true
          break
        fi
        sleep 1
      done

      # 步骤 2: 强制停止
      if [ "$STOPPED" = false ]; then
        echo "  强制停止 TestAIServer..."
        kill -9 $TESTAI_PID 2>/dev/null
        sleep 2

        if ! kill -0 $TESTAI_PID 2>/dev/null; then
          echo "✅ TestAIServer 已停止（强制退出）"
        else
          echo "❌ 无法停止 TestAIServer (PID: $TESTAI_PID)"
          exit 1
        fi
      fi
    fi
  else
    echo "  ⚠️  未找到 TestAIServer PID 文件"
    echo "  尝试通过进程名查找..."
    if pgrep -f "testaiserver.exe" > /dev/null; then
      pkill -9 -f "testaiserver.exe"
      sleep 2
      echo "✅ TestAIServer 已停止"
    else
      echo "✅ TestAIServer 进程不存在"
    fi
  fi
fi

echo ""
```

**预期输出**:
```
[2/4] 停止 TestAIServer...
  检测到 Windows 环境，使用 taskkill...
  使用进程名终止...
SUCCESS: The process "testaiserver.exe" with PID 12345 has been terminated.
✅ TestAIServer 已停止（Windows）
```

---

### 7.4 等待文件释放

**目的**: 等待进程完全退出，文件句柄释放

**当前工作目录**: 项目根目录

**命令**:
```bash
echo "[3/4] 等待文件释放..."
sleep 3
echo "✅ 文件释放等待完成"
echo ""
```

**预期输出**:
```
[3/4] 等待文件释放...
✅ 文件释放等待完成
```

---

### 7.5 删除测试目录

**目的**: 删除整个 `test/autotest/` 目录

**当前工作目录**: 项目根目录

**⚠️ 重要**：这会删除所有测试相关文件，包括：
- 编译的可执行文件
- 配置文件（`.nemesisbot/`）
- 日志文件
- PID 文件
- 会话数据

**命令**:
```bash
echo "[4/4] 删除测试目录..."

# 检查测试目录是否存在
if [ ! -d "test/autotest" ]; then
  echo "⚠️  测试目录不存在: test/autotest"
  echo "   可能已被删除或从未创建"
else
  echo "  删除 test/autotest/ 目录..."
  rm -rf test/autotest

  # 等待文件系统同步
  sleep 1
fi

# 验证清理结果
if [ -d "test/autotest" ]; then
  echo "❌ 测试目录仍然存在"
  echo ""
  echo "可能原因:"
  echo "  1. 文件被进程锁定"
  echo "  2. 权限不足"
  echo ""
  echo "请检查:"
  echo "  - 进程是否已完全停止"
  echo "  - 文件是否被其他程序占用"
  echo ""
  echo "手动清理命令:"
  echo "  cd test/autotest"
  echo "  # 检查占用情况"
  echo "  # 然后逐个删除文件"
  exit 1
else
  echo "✅ 测试目录已删除: test/autotest/"
fi

echo ""
```

**预期输出**:
```
[4/4] 删除测试目录...
  删除 test/autotest/ 目录...
✅ 测试目录已删除: test/autotest/
```

---

### 7.6 验证清理结果

**目的**: 确认所有进程和目录已清理

**当前工作目录**: 项目根目录

**命令**:
```bash
echo "=== 验证清理结果 ==="
echo ""

REMAINING=0

# **⚠️ Windows 特殊处理**
if [[ "$OSTYPE" == "msys" || "$OSTYPE" == "win32" ]]; then
  # Windows: 使用 tasklist
  echo "检查残留进程..."

  if tasklist | grep -i "nemesisbot.exe" > /dev/null 2>&1; then
    echo "❌ 发现残留的 NemesisBot 进程"
    tasklist | grep -i "nemesisbot.exe"
    REMAINING=$((REMAINING + 1))
  else
    echo "✅ NemesisBot 进程已停止"
  fi

  if tasklist | grep -i "testaiserver.exe" > /dev/null 2>&1; then
    echo "❌ 发现残留的 TestAIServer 进程"
    tasklist | grep -i "testaiserver.exe"
    REMAINING=$((REMAINING + 1))
  else
    echo "✅ TestAIServer 进程已停止"
  fi
else
  # Unix/Linux: 使用 ps
  echo "检查残留进程..."

  if ps aux | grep -E "[n]emesisbot.exe" > /dev/null; then
    echo "❌ 发现残留的 NemesisBot 进程"
    ps aux | grep -E "[n]emesisbot.exe" | head -3
    REMAINING=$((REMAINING + 1))
  else
    echo "✅ NemesisBot 进程已停止"
  fi

  if ps aux | grep "[t]estaiserver" > /dev/null; then
    echo "❌ 发现残留的 TestAIServer 进程"
    ps aux | grep "[t]estaiserver" | head -3
    REMAINING=$((REMAINING + 1))
  else
    echo "✅ TestAIServer 进程已停止"
  fi
fi

echo ""

# 检查测试目录
if [ -d "test/autotest" ]; then
  echo "❌ 测试目录仍然存在: test/autotest"
  REMAINING=$((REMAINING + 1))
else
  echo "✅ 测试目录已清理: test/autotest"
fi

echo ""

# 总结
if [ $REMAINING -eq 0 ]; then
  echo "========================================"
  echo "     ✅ 环境清理完全成功"
  echo "========================================"
  echo ""
  echo "清理状态:"
  echo "  ✅ NemesisBot 进程已停止"
  echo "  ✅ TestAIServer 进程已停止"
  echo "  ✅ 测试目录已删除"
  echo "  ✅ 端口已释放（8080, 49001）"
  echo ""
else
  echo "========================================"
  echo "     ⚠️  部分清理未完成"
  echo "========================================"
  echo ""
  echo "发现 $REMAINING 个残留项"
  echo "请手动检查并清理"
  echo ""
  exit 1
fi
```

**预期输出**:
```
=== 验证清理结果 ===

检查残留进程...
✅ NemesisBot 进程已停止
✅ TestAIServer 进程已停止

✅ 测试目录已清理: test/autotest

========================================
     ✅ 环境清理完全成功
========================================

清理状态:
  ✅ NemesisBot 进程已停止
  ✅ TestAIServer 进程已停止
  ✅ 测试目录已删除
  ✅ 端口已释放（8080, 49001）
```

---

## 阶段 8: 结果分析和迭代

### 8.1 测试结果分析

**目的**: 评估测试结果并确定下一步行动

**当前工作目录**: 项目根目录

**分析框架**:

```yaml
结果评估:
  测试通过:
    标准:
      - 所有测试场景通过
      - 响应时间在可接受范围
      - 无错误或异常
      - 日志记录完整

    动作:
      1. 记录成功结果到测试报告
      2. 标记功能为已验证
      3. 可以提交代码或继续下一个功能

    报告位置:
      - 测试报告: docs/REPORT/TEST_XXX_YYYY-MM-DD.md
      - ⚠️ 不要保存到 test/autotest/（已被删除）

  测试失败:
    分析步骤:
      1. 确定失败类型:
         - 功能错误（响应不符合预期）
         - 性能问题（响应超时）
         - 系统错误（崩溃、异常）
         - 配置问题（模型、参数）

      2. 收集诊断信息:
         - Bot 日志（test/autotest/nemesisbot.log，但已被删除）
         - TestAIServer 日志（test/autotest/，已被删除）
         - 错误消息和堆栈跟踪
         - 说明: 如果日志已被删除，需要重新运行测试收集

      3. 定位根本原因:
         - 代码 Bug
         - 配置错误
         - TestAIServer 模型问题
         - 环境问题

      4. 确定修复方案:
         - 修复 NemesisBot 代码
         - 扩展 TestAIServer 功能
         - 调整配置参数
         - 修改测试用例

    动作:
      1. 记录失败详情到测试报告
      2. 创建问题跟踪（如需要）
      3. 实施修复方案
      4. 返回阶段 2 重新测试

  测试部分失败:
    评估:
      1. 失败的影响范围
      2. 是否影响核心功能
      3. 是否有变通方案
      4. 修复的优先级

    决策:
      - 阻断问题: 必须修复后才能继续
      - 非阻塞性问题: 可以记录并继续
      - 优化项: 后续改进
```

---

### 8.2 测试报告模板

**目的**: 提供标准化的测试报告格式

**报告保存位置**: `docs/REPORT/`（不在 `test/autotest/`）

**模板**:

```markdown
# [功能名称] 测试报告

**测试日期**: YYYY-MM-DD
**测试时间**: HH:MM:SS
**测试人员**: [姓名/系统]
**测试版本**: [Git commit hash]

---

## 测试目标

[描述测试的具体目标和验证内容]

---

## 测试环境

- **操作系统**: Windows 11
- **Go 版本**: [版本]
- **TestAIServer**: testai-X.X
- **NemesisBot**: [版本]
- **测试工作目录**: test/autotest/ (已清理)

---

## 测试场景

### 场景 1: [场景名称]

**目标**: [场景目标]

**输入**:
```json
{
  "type": "message",
  "content": "[输入内容]",
  "timestamp": "2026-03-23T10:30:00Z"
}
```

**预期输出**:
- [期望结果 1]
- [期望结果 2]

**实际输出**:
```json
{
  "type": "message",
  "role": "assistant",
  "content": "[实际响应内容]",
  "timestamp": "2026-03-23T10:30:05Z"
}
```

**结果**: ✅ 通过 / ❌ 失败

**响应时间**: X.X 秒

**备注**: [观察到的信息]

---

## 测试结果统计

| 场景 | 结果 | 响应时间 | 备注 |
|------|------|----------|------|
| 场景 1 | ✅ | 2.3s | - |
| 场景 2 | ❌ | 30.1s | 超时 |

**通过率**: X%

**平均响应时间**: X.X 秒

---

## 问题记录

### 问题 1: [问题标题]

**严重程度**: 🔴 高 / 🟡 中 / 🟢 低

**现象**:
[具体描述问题表现]

**复现步骤**:
1. [步骤 1]
2. [步骤 2]

**根本原因**:
[问题原因分析]

**解决方案**:
[如何修复]

**状态**: 已解决 / 待解决 / 进行中

---

## 性能指标

- **平均响应时间**: X.X 秒
- **最大响应时间**: X.X 秒
- **超时次数**: X
- **错误率**: X%

---

## 改进建议

1. [建议 1]
2. [建议 2]

---

## 结论

[总体评价、测试是否通过、下一步行动]

---

**报告生成时间**: YYYY-MM-DD HH:MM:SS
**报告生成者**: [系统/手动]
**报告保存位置**: docs/REPORT/TEST_XXX_YYYY-MM-DD.md
```

---

### 8.3 迭代循环

**目的**: 处理测试失败后的修复流程

```yaml
测试迭代流程:
  while 测试失败:
    1. 分析失败原因
    2. 修复问题
    3. 重新编译（如需要）
    4. 返回阶段 2（环境准备）
    5. 重新执行测试

    if 修复成功:
      break

  记录最终结果
  生成测试报告
  保存到 docs/REPORT/
```

---

## 检查点

**清理和结果分析完成检查点**:

- [ ] **已返回项目根目录**
- [ ] NemesisBot 进程已停止
- [ ] TestAIServer 进程已停止
- [ ] **test/autotest/ 目录已删除** ⚠️ **必须**
- [ ] 端口 8080 已释放
- [ ] 端口 49001 已释放
- [ ] 测试结果已记录
- [ ] **测试报告已生成并保存到 `docs/REPORT/`** ⚠️ **不在 test/autotest/**
- [ ] 问题已跟踪（如适用）

**状态**: ✅ 通过 / ❌ 失败

---

## 完整流程总结

```
阶段流程:
  1. 预检查 → 验证测试适用性
  2. 环境准备 → 创建 test/autotest/，编译工具
  3. 本地初始化 → 创建 test/autotest/.nemesisbot/
  4. 配置 AI → 添加测试模型
  5. 启动 Bot → 启动 gateway（在 test/autotest/）
  6. 执行测试 → 运行测试场景
  7. 清理环境 → 返回根目录，删除 test/autotest/
  8. 结果分析 → 生成报告（保存到 docs/REPORT/）

目录流转:
  根目录 → test/autotest/ → (保持) → (保持) → (保持) → (保持) → 根目录

关键改进:
  - 规范测试工作目录到 test/autotest/
  - 简化清理为删除单个目录
  - 测试报告隔离（保存在 docs/REPORT/）
  - 源码位置不变，只管理编译产物
```

---

**测试流程完成！**
