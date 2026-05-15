# testai-5.0 安全测试模型使用指南

## 概述

`testai-5.0` 是专门为测试 NemesisBot 安全审批功能而设计的测试 AI 模型。它可以根据用户传入的参数，返回各种文件操作的工具调用，从而触发安全审批对话框。

## 功能特性

### 支持的文件操作

| 操作 | 说明 | 默认风险级别 | 用途 |
|------|------|-------------|------|
| `file_read` | 读取文件 | HIGH | 测试文件读取审批 |
| `file_write` | 写入文件 | HIGH | 测试文件写入审批 |
| `file_delete` | 删除文件 | CRITICAL | 测试文件删除审批 |
| `file_append` | 追加文件 | MEDIUM | 测试文件追加审批 |
| `dir_create` | 创建目录 | LOW | 测试目录创建审批 |
| `dir_delete` | 删除目录 | HIGH | 测试目录删除审批 |
| `dir_list` | 列出目录 | LOW | 测试目录列出 |

### 工作原理

1. **输入格式**：
   ```
   <FILE_OP>{"operation":"file_delete","path":"/etc/passwd","risk_level":"CRITICAL"}</FILE_OP>
   ```

2. **输出格式**：
   ```json
   {
     "tool_calls": [
       {
         "id": "call-xxx",
         "type": "function",
         "function": {
           "name": "file_delete",
           "arguments": "{\"path\":\"/etc/passwd\",\"risk_level\":\"CRITICAL\"}"
         }
       }
     ]
   }
   ```

3. **触发流程**：
   - NemesisBot 接收到工具调用
   - 检查操作类型和风险级别
   - 如果风险级别 >= MinRiskLevel，触发安全审批对话框
   - 用户批准/拒绝后，执行操作

## 快速开始

### 1. 启动 TestAIServer

```bash
cd test/TestAIServer
./testaiserver.exe
```

服务器将在 `http://localhost:8080` 启动。

### 2. 添加模型到 NemesisBot

```bash
nemesisbot model add \
  --model test/testai-5.0 \
  --base http://127.0.0.1:8080/v1 \
  --key test-key \
  --default
```

### 3. 配置安全策略

编辑 `.nemesisbot/config.json`：

```json
{
  "security": {
    "enabled": true,
    "defaultAction": "deny",
    "fileRules": {
      "delete": [
        {
          "pattern": "/etc/**",
          "action": "ask",
          "reason": "系统关键文件"
        },
        {
          "pattern": "/tmp/**",
          "action": "ask",
          "reason": "临时文件"
        }
      ],
      "write": [
        {
          "pattern": "/etc/**",
          "action": "ask",
          "reason": "系统配置文件"
        }
      ],
      "read": [
        {
          "pattern": "/etc/passwd",
          "action": "ask",
          "reason": "密码文件"
        }
      ]
    }
  }
}
```

### 4. 启动 NemesisBot

```bash
nemesisbot gateway
```

## 测试场景

### 场景 1：测试 CRITICAL 风险操作（删除密码文件）

**发送消息**：
```
<FILE_OP>{"operation":"file_delete","path":"/etc/passwd","risk_level":"CRITICAL"}</FILE_OP>
```

**预期行为**：
1. testai-5.0 返回 `file_delete` 工具调用
2. NemesisBot 检测到 CRITICAL 风险
3. 弹出安全审批对话框（550x480）
4. 用户批准后，执行删除操作

### 场景 2：测试 HIGH 风险操作（写入配置文件）

**发送消息**：
```
<FILE_OP>{"operation":"file_write","path":"/etc/config.conf","content":"new config","risk_level":"HIGH"}</FILE_OP>
```

**预期行为**：
1. testai-5.0 返回 `file_write` 工具调用
2. NemesisBot 检测到 HIGH 风险
3. 弹出安全审批对话框
4. 显示操作详情（文件路径、内容等）

### 场景 3：测试 LOW 风险操作（创建临时目录）

**发送消息**：
```
<FILE_OP>{"operation":"dir_create","path":"/tmp/mydir","risk_level":"LOW"}</FILE_OP>
```

**预期行为**：
- 如果 MinRiskLevel = MEDIUM，不会弹出对话框（LOW < MEDIUM）
- 如果 MinRiskLevel = LOW，会弹出对话框

### 场景 4：测试多轮对话（工具调用后返回结果）

**第一轮**：
```
用户: <FILE_OP>{"operation":"file_read","path":"/etc/passwd"}</FILE_OP>
AI: [返回 file_read 工具调用]
```

**第二轮**：
```
工具: {"result":"success","content":"root:x:0:0:root:/root:/bin/bash"}
AI: 文件读取成功。内容为：root:x:0:0:root:/root:/bin/bash
```

## 高级用法

### 批量测试文件操作

创建测试脚本：

```bash
# test_multiple_operations.sh
#!/bin/bash

operations=(
  '{"operation":"file_read","path":"/etc/passwd"}'
  '{"operation":"file_write","path":"/tmp/test.txt","content":"test"}'
  '{"operation":"file_delete","path":"/tmp/test.txt"}'
  '{"operation":"dir_create","path":"/tmp/testdir"}'
  '{"operation":"dir_delete","path":"/tmp/testdir"}'
)

for op in "${operations[@]}"; do
  echo "Testing: $op"
  curl -s -X POST http://localhost:8080/v1/chat/completions \
    -H "Content-Type: application/json" \
    -d "{\"model\":\"testai-5.0\",\"messages\":[{\"role\":\"user\",\"content\":\"<FILE_OP>$op</FILE_OP>\"}]}"
  echo ""
  sleep 1
done
```

### 测试不同风险级别的审批策略

```bash
# 测试所有风险级别
risk_levels=("LOW" "MEDIUM" "HIGH" "CRITICAL")

for level in "${risk_levels[@]}"; do
  echo "Testing risk level: $level"
  curl -s -X POST http://localhost:8080/v1/chat/completions \
    -H "Content-Type: application/json" \
    -d "{\"model\":\"testai-5.0\",\"messages\":[{\"role\":\"user\",\"content\":\"<FILE_OP>{\\\"operation\\\":\\\"file_read\\\",\\\"path\\\":\\\"/etc/passwd\\\",\\\"risk_level\\\":\\\"$level\\\"}</FILE_OP>\"}]}"
  echo ""
  sleep 1
done
```

## 验证安全审批功能

### 检查清单

- [ ] 服务器启动成功
- [ ] 模型添加到 NemesisBot
- [ ] 安全策略配置正确
- [ ] 发送文件操作请求
- [ ] 弹出安全审批对话框
- [ ] 对话框显示正确的操作信息
- [ ] 点击 Allow/Deny 按钮
- [ ] 操作正确执行或阻止

### 调试技巧

**查看请求日志**：
```bash
# TestAIServer 日志位置
cd test/TestAIServer/log/testai-5.0/
ls -lt
cat *.json
```

**查看 NemesisBot 日志**：
```bash
# NemesisBot 日志
tail -f ~/.nemesisbot/logs/nemesisbot.log
```

**检查安全策略**：
```bash
nemesisbot security status
```

## 常见问题

### Q1: 没有弹出安全对话框？

**检查项**：
1. 安全模块是否启用？`nemesisbot security status`
2. MinRiskLevel 配置？如果设为 CRITICAL，LOW/MEDIUM 操作不会触发
3. 操作是否匹配规则？检查 `pattern` 是否匹配文件路径

### Q2: 工具调用返回错误？

**检查项**：
1. JSON 格式是否正确？
2. operation 拼写是否正确？
3. path 是否存在？

### Q3: 如何模拟真实的 AI 回复？

**技巧**：可以组合使用 `testai-2.0` 和 `testai-5.0`：
- 第一轮：用户发送消息，testai-5.0 返回工具调用
- 工具执行后，NemesisBot 发送第二轮
- 如果需要 AI 解释结果，可以切换到其他模型

## 总结

`testai-5.0` 是一个强大的安全测试工具，可以：
- ✅ 测试所有文件操作类型
- ✅ 模拟不同风险级别
- ✅ 触发安全审批对话框
- ✅ 验证安全策略配置
- ✅ 测试审批流程完整性

通过这个模型，您可以完整测试 NemesisBot 的安全审批功能！
