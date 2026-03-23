# 自动化测试流程 Skill

完整的自动化测试流程，用于测试需要 AI 支持的功能，使用 TestAIServer 作为模拟后端。

---

## 概述

此 Skill 提供了一个完整的自动化测试流程，用于测试 NemesisBot 的各种功能。它使用 TestAIServer 作为模拟 AI 后端，通过 WebSocket API 与 Bot 通信，执行测试用例并验证结果。

### 适用场景

- ✅ 需要调用 LLM 的功能测试
- ✅ 工具调用测试（文件操作、集群通信等）
- ✅ 消息处理和响应验证
- ✅ 回归测试（代码修改后重新测试）
- ❌ 不适用于需要 UI 交互的功能（如安全审批对话框）
- ❌ 不适用于不需要 LLM 的功能测试（在不需要 LLM 的功能测试中，可通过 UT/IT/ST 保证功能有效）

---

## WebSocket 接口规范

### 连接信息

```
协议: ws://
地址: 127.0.0.1
端口: 8080
路径: /ws
完整 URL: ws://127.0.0.1:8080/ws
```

### 客户端消息格式

```json
{
  "type": "message",
  "content": "用户消息内容",
  "timestamp": "2026-03-23T10:30:00Z"
}
```

**字段说明**:
- `type`: 消息类型，固定为 "message"
- `content`: 消息内容（可以是特殊标记，如 `<FILE_OP>...</FILE_OP>`）
- `timestamp`: 可选的时间戳（RFC3339 格式）

### 服务端消息格式

```json
{
  "type": "message",
  "role": "assistant",
  "content": "AI 响应内容",
  "timestamp": "2026-03-23T10:30:05Z"
}
```

**字段说明**:
- `type`: 消息类型
  - "message": 正常消息
  - "error": 错误消息
  - "pong": 心跳响应
- `role`: 角色
  - "assistant": AI 助手响应
  - "user": 用户消息（回显）
- `content`: 消息内容
- `timestamp`: 时间戳

---

## 测试阶段

### 阶段 1: 预检查

```yaml
检查项:
  - UI 依赖检查:
      描述: 确认测试功能不需要 UI 交互
      方法: 检查功能描述，确认不涉及对话框、窗口等
      通过条件: 无 UI 依赖

  - AI 支持检查:
      描述: 确认功能需要 LLM 调用
      方法: 检查工具定义、Agent 处理逻辑
      通过条件: 需要 AI 处理

  - TestAIServer 能力检查:
      描述: 确认 TestAIServer 有支持的测试模型
      方法: 查看 test/TestAIServer/README.md
      可用模型:
        - testai-5.0: 文件操作测试
        - testai-3.0: 集群通信测试
        - testai-4.2/4.3: 工具调用测试（sleep）
        - testai-1.1: 基础响应测试
        - testai-2.0: 消息回显测试
      通过条件: 找到匹配的测试模型
```

---

### 阶段 2: 环境准备

```bash
# 2.1 编译 TestAIServer
cd test/TestAIServer
go build -o testaiserver.exe

# 2.2 启动 TestAIServer (后台)
./testaiserver.exe &
TESTAI_PID=$!
echo "TestAIServer PID: $TESTAI_PID"

# 2.3 等待 TestAIServer 就绪
sleep 2
curl http://127.0.0.1:8080/v1/models

# 2.4 编译 NemesisBot
cd ../../
go build -o nemesisbot.exe ./nemesisbot

# 验证
if [ ! -f "./testaiserver.exe" ]; then
  echo "❌ TestAIServer 编译失败"
  exit 1
fi

if [ ! -f "./nemesisbot.exe" ]; then
  echo "❌ NemesisBot 编译失败"
  exit 1
fi
```

---

### 阶段 3: 本地环境初始化

```bash
# 3.1 创建本地配置目录
./nemesisbot.exe onboard default --local

# 验证配置创建
if [ ! -d "./.nemesisbot" ]; then
  echo "❌ 本地配置目录创建失败"
  exit 1
fi

# 3.2 检查配置文件
ls -la ./.nemesisbot/
# 预期输出:
# config.json
# workspace/
# workspace/agents/
# workspace/cluster/
# workspace/logs/
```

---

### 阶段 4: 配置测试 AI

```bash
# 4.1 添加测试 AI 模型
./nemesisbot.exe model add \
  --model test/testai-5.0 \
  --base http://127.0.0.1:8080/v1 \
  --key test-key \
  --default

# 4.2 验证模型配置
./nemesisbot.exe model list

# 预期输出包含:
# test/testai-5.0
# base: http://127.0.0.1:8080/v1
# default: true
```

---

### 阶段 5: 启动 Bot

```bash
# 5.1 启动 Bot (后台)
./nemesisbot.exe agent &
BOT_PID=$!
echo "Bot PID: $BOT_PID"

# 5.2 等待 Bot 就绪
sleep 3

# 5.3 验证 Bot 进程
ps -p $BOT_PID > /dev/null
if [ $? -ne 0 ]; then
  echo "❌ Bot 进程未运行"
  exit 1
fi
```

---

### 阶段 6: 执行测试

#### 测试场景框架

此 Skill 是通用测试框架，测试场景根据开发目标动态确定。以下是测试场景的通用结构：

```yaml
测试场景结构:
  名称: 功能名称
  目标: 明确的测试目标
  前置条件:
    - TestAIServer 运行中
    - Bot 运行中
    - 测试模型已配置

  测试步骤:
    - 步骤 1: 描述
      输入: 消息内容
      预期: 期望结果
    - 步骤 2: 描述
      输入: ...
      预期: ...

  验证标准:
    - 功能正确性: 响应符合预期
    - 无错误: 无异常或错误消息
    - 性能: 响应时间合理（< 30秒）
    - 日志: 日志记录完整

  清理:
    - 断开 WebSocket
    - 记录测试结果
```

#### 示例测试场景：文件操作测试

```yaml
名称: 文件读取操作测试
目标: 验证 Bot 能正确处理文件读取工具调用

测试步骤:
  - 步骤 1: 连接 WebSocket
    动作: 建立 ws://127.0.0.1:8080/ws 连接
    预期: 连接成功

  - 步骤 2: 发送文件读取请求
    输入: "<FILE_OP>{\"operation\":\"file_read\",\"path\":\"test.txt\"}</FILE_OP>"
    预期: Bot 返回工具调用响应

  - 步骤 3: 验证响应
    检查:
      - 响应类型是 "message"
      - role 是 "assistant"
      - content 包含工具调用结果或错误信息

  - 步骤 4: 检查日志
    检查: ./.nemesisbot/workspace/logs/ 中的日志
    预期: 记录了文件读取操作

验证标准:
  - 功能正确性: 10 分
  - 无错误: 5 分
  - 性能: 3 分
  - 日志完整: 2 分
  总分: 20 分，通过: >= 15 分
```

---

### 阶段 7: 清理环境

```bash
# 7.1 停止 Bot
echo "停止 Bot (PID: $BOT_PID)..."
kill $BOT_PID
wait $BOT_PID 2>/dev/null

# 7.2 停止 TestAIServer
echo "停止 TestAIServer (PID: $TESTAI_PID)..."
kill $TESTAI_PID
wait $TESTAI_PID 2>/dev/null

# 7.3 清理本地配置
echo "清理本地配置..."
rm -rf ./.nemesisbot

# 7.4 验证清理
if [ -d "./.nemesisbot" ]; then
  echo "⚠️  警告: .nemesisbot 目录未完全删除"
  rm -rf ./.nemesisbot
fi

echo "✅ 环境清理完成"
```

---

### 阶段 8: 结果分析和迭代

```yaml
结果分析:
  测试通过:
    动作: 记录成功结果到开发报告
    下一步: 测试完成，可以提交代码

  测试失败:
    动作:
      1. 分析失败原因
      2. 定位问题代码
      3. 修复问题
      4. 可能需要扩展 TestAIServer 功能
    下一步: 返回阶段 2，重新测试

  测试部分失败:
    动作:
      1. 评估失败影响范围
      2. 确定是否需要修复
      3. 更新测试用例
    下一步: 根据评估结果决定是否重新测试
```

---

## 测试日志记录

### 日志位置

```
docs/REPORT/
  └── TEST_<功能名称>_<日期>.md
```

### 日志模板

```markdown
# <功能名称> 测试报告

**测试日期**: YYYY-MM-DD
**测试人员**: [姓名/系统]
**测试版本**: [Git commit hash]

---

## 测试目标

[描述测试的具体目标]

---

## 测试环境

- **操作系统**: Windows 11
- **TestAIServer 版本**: testai-X.X
- **NemesisBot 版本**: [version]
- **测试模型**: test/testai-X.X

---

## 测试场景

### 场景 1: [场景名称]

**目标**: [场景目标]

**步骤**:
1. [步骤 1]
2. [步骤 2]

**输入**:
\`\`\`json
{
  "type": "message",
  "content": "[输入内容]"
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
  "content": "[实际响应]"
}
\`\`\`

**结果**: ✅ 通过 / ❌ 失败

**备注**: [任何观察到的信息]

---

## 测试结果统计

| 场景 | 结果 | 响应时间 | 备注 |
|------|------|----------|------|
| 场景 1 | ✅ | 2.3s | - |
| 场景 2 | ❌ | 30.1s | 超时 |

**通过率**: X%

---

## 问题记录

### 问题 1: [问题描述]

**现象**: [具体表现]
**原因**: [根因分析]
**解决方案**: [如何修复]
**状态**: 已修复 / 待修复

---

## 改进建议

1. [建议 1]
2. [建议 2]

---

## 结论

[总体评价和下一步行动]
```

---

## 快速参考

### 常用命令

```bash
# 编译
cd test/TestAIServer && go build -o testaiserver.exe
cd ../../ && go build -o nemesisbot.exe ./nemesisbot

# 启动测试 AI
./testaiserver.exe &

# 初始化本地环境
./nemesisbot.exe onboard default --local

# 配置测试模型
./nemesisbot.exe model add --model test/testai-5.0 --base http://127.0.0.1:8080/v1 --key test-key --default

# 启动 Bot
./nemesisbot.exe agent &

# 停止所有进程
kill $BOT_PID $TESTAI_PID

# 清理
rm -rf ./.nemesisbot
```

### TestAIServer 模型快速参考

| 模型 | 用途 | 特殊标记 |
|------|------|----------|
| testai-1.1 | 基础响应测试 | 无 |
| testai-2.0 | 消息回显 | 无 |
| testai-3.0 | 集群通信 | `<PEER_CHAT>{}</PEER_CHAT>` |
| testai-4.2 | 工具调用(30s) | 返回 sleep 工具 |
| testai-4.3 | 工具调用(300s) | 返回 sleep 工具 |
| testai-5.0 | 文件操作 | `<FILE_OP>{}</FILE_OP>` |

---

## 注意事项

1. **进程管理**: 确保在测试结束后清理所有后台进程
2. **环境隔离**: 使用 `--local` 标志确保不影响主配置
3. **端口冲突**: 确保 8080 端口未被占用
4. **日志备份**: 测试日志应保存到 docs/REPORT/ 目录
5. **错误处理**: 每个阶段都应有错误检查和处理
6. **超时设置**: WebSocket 消息响应超时设为 30 秒

---

**最后更新**: 2026-03-23
