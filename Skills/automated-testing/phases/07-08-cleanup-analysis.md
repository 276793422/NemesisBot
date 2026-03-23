# 阶段 7-8: 清理环境和结果分析

---

## 阶段 7: 清理环境

测试完成后，需要清理所有进程和临时文件。

### 7.1 停止 Bot 进程

```bash
echo "=== 开始清理环境 ==="

# 从文件读取 PID（如果保存了）
if [ -f "/tmp/nemesisbot.pid" ]; then
  BOT_PID=$(cat /tmp/nemesisbot.pid)
  rm /tmp/nemesisbot.pid
fi

# 停止 Bot
if [ -n "$BOT_PID" ]; then
  echo "停止 Bot (PID: $BOT_PID)..."
  kill $BOT_PID 2>/dev/null

  # 等待进程结束（最多 5 秒）
  for i in {1..5}; do
    if ! ps -p $BOT_PID > /dev/null 2>&1; then
      echo "✅ Bot 进程已停止"
      break
    fi

    if [ $i -eq 5 ]; then
      echo "⚠️  Bot 进程未响应，强制终止"
      kill -9 $BOT_PID 2>/dev/null
    fi

    sleep 1
  done
fi
```

### 7.2 停止 TestAIServer 进程

```bash
# 从文件读取 PID（如果保存了）
if [ -f "/tmp/testaiserver.pid" ]; then
  TESTAI_PID=$(cat /tmp/testaiserver.pid)
  rm /tmp/testaiserver.pid
fi

# 停止 TestAIServer
if [ -n "$TESTAI_PID" ]; then
  echo "停止 TestAIServer (PID: $TESTAI_PID)..."
  kill $TESTAI_PID 2>/dev/null

  # 等待进程结束
  for i in {1..5}; do
    if ! ps -p $TESTAI_PID > /dev/null 2>&1; then
      echo "✅ TestAIServer 进程已停止"
      break
    fi

    if [ $i -eq 5 ]; then
      echo "⚠️  TestAIServer 进程未响应，强制终止"
      kill -9 $TESTAI_PID 2>/dev/null
    fi

    sleep 1
  done
fi
```

### 7.3 清理本地配置

```bash
# 删除本地配置目录
echo "清理本地配置..."
rm -rf ./.nemesisbot

# 验证清理
if [ -d "./.nemesisbot" ]; then
  echo "⚠️  警告: .nemesisbot 目录仍然存在"
  echo "尝试强制删除..."
  rm -rf ./.nemesisbot
  sleep 1

  if [ -d "./.nemesisbot" ]; then
    echo "❌ 无法删除 .nemesisbot 目录"
    echo "请手动检查并删除"
    exit 1
  fi
fi

echo "✅ 本地配置已清理"
```

### 7.4 清理验证

```bash
echo ""
echo "=== 清理验证 ==="

# 验证进程已停止
if [ -n "$BOT_PID" ]; then
  if ps -p $BOT_PID > /dev/null 2>&1; then
    echo "❌ Bot 进程仍在运行"
  else
    echo "✅ Bot 进程已停止"
  fi
fi

if [ -n "$TESTAI_PID" ]; then
  if ps -p $TESTAI_PID > /dev/null 2>&1; then
    echo "❌ TestAIServer 进程仍在运行"
  else
    echo "✅ TestAIServer 进程已停止"
  fi
fi

# 验证目录已清理
if [ -d "./.nemesisbot" ]; then
  echo "❌ .nemesisbot 目录仍然存在"
else
  echo "✅ .nemesisbot 目录已清理"
fi

# 验证端口已释放
if netstat -an 2>/dev/null | grep 8080 > /dev/null; then
  echo "⚠️  端口 8080 仍被占用"
else
  echo "✅ 端口 8080 已释放"
fi

echo ""
echo "=== 环境清理完成 ==="
```

---

## 阶段 8: 结果分析和迭代

### 8.1 测试结果分析

```yaml
结果评估:
  测试通过:
    标准:
      - 所有测试场景通过
      - 响应时间在可接受范围
      - 无错误或异常
      - 日志记录完整

    动作:
      1. 记录成功结果到开发报告
      2. 标记功能为已验证
      3. 可以提交代码或继续下一个功能

    报告内容:
      - 测试日期和时间
      - 测试环境信息
      - 测试场景和结果
      - 性能指标
      - 观察到的问题（如果有）
      - 结论和建议

  测试失败:
    分析步骤:
      1. 确定失败类型:
         - 功能错误（响应不符合预期）
         - 性能问题（响应超时）
         - 系统错误（崩溃、异常）
         - 配置问题（模型、参数）

      2. 收集诊断信息:
         - Bot 日志 (./.nemesisbot/workspace/logs/)
         - TestAIServer 日志 (test/TestAIServer/log/)
         - 错误消息和堆栈跟踪
         - 网络抓包（如需要）

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
      1. 记录失败详情到开发报告
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

```markdown
# [功能名称] 测试报告

## 测试信息

- **测试日期**: YYYY-MM-DD HH:MM:SS
- **测试人员**: [姓名/系统]
- **测试版本**: [Git commit hash]
- **测试环境**:
  - 操作系统: Windows 11
  - Go 版本: [版本]
  - TestAIServer: testai-X.X
  - NemesisBot: [版本]

## 测试目标

[描述测试的具体目标和验证内容]

## 测试场景

### 场景 1: [场景名称]

**目标**: [场景目标]

**输入**:
\`\`\`json
{
  "type": "message",
  "content": "[输入内容]",
  "timestamp": "2026-03-23T10:30:00Z"
}
\`\`\`

**预期输出**:
- [期望结果 1]
- [期望结果 2]

**实际输出**:
\`\`\`json
{
  "type": "message",
  "role": "assistant",
  "content": "[实际响应内容]",
  "timestamp": "2026-03-23T10:30:05Z"
}
\`\`\`

**结果**: ✅ 通过 / ❌ 失败

**响应时间**: X.X 秒

**备注**: [观察到的信息]

## 测试结果统计

| 场景 | 结果 | 响应时间 | 备注 |
|------|------|----------|------|
| 场景 1 | ✅ | 2.3s | - |
| 场景 2 | ❌ | 30.1s | 超时 |

**通过率**: X%

**平均响应时间**: X.X 秒

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

## 性能指标

- **平均响应时间**: X.X 秒
- **最大响应时间**: X.X 秒
- **超时次数**: X
- **错误率**: X%

## 改进建议

1. [建议 1]
2. [建议 2]

## 结论

[总体评价、测试是否通过、下一步行动]

---

**报告生成时间**: YYYY-MM-DD HH:MM:SS
**报告生成者**: [系统/手动]
```

---

### 8.3 迭代循环

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
```

---

## 清理脚本

```bash
#!/bin/bash
# cleanup.sh - 环境清理脚本

echo "=== 开始环境清理 ==="

# 停止 Bot
if [ -f "/tmp/nemesisbot.pid" ]; then
  BOT_PID=$(cat /tmp/nemesisbot.pid)
  echo "停止 Bot (PID: $BOT_PID)..."
  kill $BOT_PID 2>/dev/null
  wait $BOT_PID 2>/dev/null
  rm /tmp/nemesisbot.pid
  echo "✅ Bot 已停止"
fi

# 停止 TestAIServer
if [ -f "/tmp/testaiserver.pid" ]; then
  TESTAI_PID=$(cat /tmp/testaiserver.pid)
  echo "停止 TestAIServer (PID: $TESTAI_PID)..."
  kill $TESTAI_PID 2>/dev/null
  wait $TESTAI_PID 2>/dev/null
  rm /tmp/testaiserver.pid
  echo "✅ TestAIServer 已停止"
fi

# 清理本地配置
echo "清理本地配置..."
rm -rf ./.nemesisbot

# 验证
echo "验证清理..."
ps aux | grep -E "nemesisbot|testaiserver" | grep -v grep || echo "✅ 所有进程已停止"
[ -d "./.nemesisbot" ] && echo "❌ .nemesisbot 仍存在" || echo "✅ 环境已清理"

echo "=== 清理完成 ==="
```

---

## 检查点

**清理和结果分析完成检查点**:

- [ ] Bot 进程已停止
- [ ] TestAIServer 进程已停止
- [ ] .nemesisbot 目录已删除
- [ ] 端口 8080 已释放
- [ ] 测试结果已记录
- [ ] 测试报告已生成
- [ ] 问题已跟踪（如适用）

**状态**: ✅ 通过 / ❌ 失败

---

## 完整测试流程总结

```
预检查 → 环境准备 → 本地初始化 → 配置AI → 启动Bot → 执行测试 → 清理环境 → 结果分析
   ↓         ↓          ↓          ↓        ↓       ↓        ↓         ↓
  通过?    编译成功   配置创建   模型配置   进程运行  WebSocket  进程停止   记录报告
                     结构完整                    连接成功   目录删除   迭代修复
```

---

**测试流程完成！**
