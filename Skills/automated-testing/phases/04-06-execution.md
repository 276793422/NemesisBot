# 阶段 4-6: 配置测试 AI、启动 Bot、执行测试

---

## 阶段 4: 配置测试 AI

### 4.1 添加测试模型

```bash
# 添加 testai-5.0 (文件操作测试)
./nemesisbot.exe model add \
  --model test/testai-5.0 \
  --base http://127.0.0.1:8080/v1 \
  --key test-key \
  --default

# 预期输出:
# ✓ Model 'test-5.0' added successfully
# ✓ Default LLM set to: test/testai-5.0
```

### 4.2 验证模型配置

```bash
./nemesisbot.exe model list

# 预期输出包含:
# test/testai-5.0
#   Base URL: http://127.0.0.1:8080/v1
#   Default: yes
```

---

## 阶段 5: 启动 Bot

### 5.1 启动 Bot 进程

```bash
# 后台启动
./nemesisbot.exe agent &
BOT_PID=$!

echo "Bot PID: $BOT_PID"

# 保存 PID
echo $BOT_PID > /tmp/nemesisbot.pid

# 等待启动
sleep 3
```

### 5.2 验证 Bot 进程

```bash
ps -p $BOT_PID > /dev/null
if [ $? -ne 0 ]; then
  echo "❌ Bot 进程未运行"
  exit 1
fi

echo "✅ Bot 进程已启动 (PID: $BOT_PID)"
```

---

## 阶段 6: 执行测试

### 测试执行框架

```yaml
测试执行:
  前置条件:
    - TestAIServer 运行中 (PID: $TESTAI_PID)
    - Bot 运行中 (PID: $BOT_PID)
    - 测试模型已配置

  测试步骤:
    1. 连接 WebSocket:
       URL: ws://127.0.0.1:8080/ws
       超时: 10 秒

    2. 发送测试消息:
       格式: JSON
       内容: 根据测试场景

    3. 接收响应:
       超时: 30 秒
       验证: 响应格式和内容

    4. 记录结果:
       日志: 保存到文件
       状态: 通过/失败
```

### WebSocket 客户端示例

```go
// 简化的 WebSocket 测试客户端
package main

import (
    "encoding/json"
    "fmt"
    "log"
    "time"

    "github.com/gorilla/websocket"
)

type ClientMessage struct {
    Type      string `json:"type"`
    Content   string `json:"content"`
    Timestamp string `json:"timestamp"`
}

type ServerMessage struct {
    Type      string `json:"type"`
    Role      string `json:"role,omitempty"`
    Content   string `json:"content,omitempty"`
    Timestamp string `json:"timestamp,omitempty"`
}

func main() {
    // 连接 WebSocket
    wsURL := "ws://127.0.0.1:8080/ws"
    conn, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
    if err != nil {
        log.Fatal("连接失败:", err)
    }
    defer conn.Close()

    fmt.Println("✅ 已连接到", wsURL)

    // 发送测试消息
    msg := ClientMessage{
        Type:      "message",
        Content:   "<FILE_OP>{\"operation\":\"file_read\",\"path\":\"test.txt\"}</FILE_OP>",
        Timestamp: time.Now().Format(time.RFC3339),
    }

    jsonData, _ := json.Marshal(msg)
    err = conn.WriteMessage(websocket.TextMessage, jsonData)
    if err != nil {
        log.Fatal("发送失败:", err)
    }

    fmt.Println("📤 已发送:", msg.Content)

    // 接收响应
    conn.SetReadDeadline(time.Now().Add(30 * time.Second))
    _, response, err := conn.ReadMessage()
    if err != nil {
        log.Fatal("接收失败:", err)
    }

    var serverMsg ServerMessage
    json.Unmarshal(response, &serverMsg)

    fmt.Println("📥 收到响应:")
    fmt.Println("   Type:", serverMsg.Type)
    fmt.Println("   Role:", serverMsg.Role)
    fmt.Println("   Content:", serverMsg.Content)
}
```

---

## 测试场景示例

### 场景 1: 文件读取操作测试

```yaml
名称: 文件读取操作
模型: testai-5.0

输入:
  content: '<FILE_OP>{"operation":"file_read","path":"test.txt"}</FILE_OP>'

预期:
  type: "message"
  role: "assistant"
  content_contains: "file_read" 或 "test.txt"

验证:
  - 响应时间 < 10 秒
  - 无错误消息
  - 工具调用正确

结果记录:
  - 实际响应: [记录完整响应]
  - 响应时间: X.X 秒
  - 测试状态: ✅ 通过 / ❌ 失败
  - 失败原因: [如适用]
```

---

## 检查点

**测试执行完成检查点**:

- [ ] WebSocket 连接成功
- [ ] 测试消息已发送
- [ ] 收到 Bot 响应
- [ ] 响应格式正确
- [ ] 响应内容符合预期
- [ ] 响应时间在可接受范围
- [ ] 测试结果已记录

---

**下一步**: 阶段 7 - 清理环境
