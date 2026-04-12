# TestAIServer

一个兼容 OpenAI API 的测试服务器，提供八个硬编码的测试模型用于测试目的。

## ⚠️ 重要提示

**使用前请务必阅读**:
- 📋 [已知问题清单](docs/KNOWN_ISSUES.md) - 了解当前限制
- 📖 [帮助系统](#使用帮助系统) - 快速查看模型信息

### 当前功能

- ✅ **完整支持流式响应**（Server-Sent Events）
- ✅ **支持工具调用**（OpenAI function calling）
- ✅ 自动请求日志记录

---

## NemesisBot 测试命令

nemesisbot model add --model test/testai-1.1 --base http://127.0.0.1:8080/v1 --key test-key --default

---

## 本地双模型交互测试

1. 编译`test\TestAIServer`项目后启动

2. 编译当前项目，在两台设备上部署，然后开启设置模型，
    - A 设备上设置 
        - `nemesisbot model add --model test/testai-3.0 --base http://127.0.0.1:8080/v1 --key test-key --default`
    - B 设备上设置
        - `nemesisbot model add --model test/testai-1.1 --base http://127.0.0.1:8080/v1 --key test-key --default`

3. 两台设备同时开启集群功能 `nemesisbot cluster enable`

4. 两个服务端全部启动

5. 查看对端设备 id ，在本地记录的 id

6. 使用如下命令发送一条信息
    - `<PEER_CHAT>{"peer_id":"bot-CloudServer-20260312-164732","content":"测试"}</PEER_CHAT>`
    - 可以看到本端返回了对端发来的信息：`{"response":"好的，我知道了","status":"success"}`
    - 逻辑是这样的
        - 本端将信息发给本地模拟的模型 testai-3.0 
        - testai-3.0 模型检测到内容包含 `<PEER_CHAT></PEER_CHAT>` 则取其中内容，组合 tool_call
        - 模型返回给本地调用工具执行 peer_chat 发送信息给远端设备
        - 远端设备收到信息之后执行正常流程调用 testai-1.1
        - 剩下的流程就是正常返回。


## 服务器收到的信息如下：
```
[GIN] 2026/03/12 - 16:51:18 | 200 |       2.062ms |       127.0.0.1 | POST     "/v1/chat/completions"
[GIN] 2026/03/12 - 16:51:18 | 200 |      3.5631ms | 192.168.236.128 | POST     "/v1/chat/completions"
[GIN] 2026/03/12 - 16:51:18 | 200 |      2.1887ms |       127.0.0.1 | POST     "/v1/chat/completions"
```

1. 第一行是本地第一次调用 testai-3.0 记录的内容，
2. 第二行是远端调用 testai-1.1 的请求。
3. 第三行是远端返回后本地再次调用 testai-3.0 的请求。

---

## 功能特性

- ✅ 完全兼容 OpenAI API 接口
- ✅ 支持 `/v1/chat/completions` 端点
- ✅ 支持 `/v1/models` 端点
- ✅ **八个预定义的测试模型**
- ✅ **完整支持流式响应**（SSE - Server-Sent Events）
- ✅ **支持工具调用**（OpenAI function calling 格式）
- ✅ 支持延迟响应测试
- ✅ 自动请求日志记录
- ✅ 分层帮助系统
- ✅ 简单易用，零配置

## 测试模型

### 1. testai-1.1
- **功能**: 立即返回固定响应
- **响应**: "好的，我知道了"
- **延迟**: 0 秒
- **用途**: 测试正常的即时响应

### 2. testai-1.2
- **功能**: 延迟 30 秒后返回固定响应
- **响应**: "好的，我知道了"
- **延迟**: 30 秒
- **用途**: 测试中等延迟场景

### 3. testai-1.3
- **功能**: 延迟 300 秒后返回固定响应
- **响应**: "好的，我知道了"
- **延迟**: 300 秒（5 分钟）
- **用途**: 测试超长延迟和超时处理

### 4. testai-2.0
- **功能**: 原样返回用户消息
- **响应**: 用户输入的最后一条消息
- **延迟**: 0 秒
- **用途**: 测试消息传递和验证

### 5. testai-3.0
- **功能**: Peer Chat 触发模型
- **响应**: 返回 `cluster_rpc` 工具调用
- **延迟**: 0 秒
- **用途**: 测试集群间 peer_chat 通信

### 6. testai-4.2
- **功能**: Sleep 工具调用（30秒）
- **响应**: 返回 `sleep(30秒)` 工具调用 → "工作完成"
- **延迟**: 0 秒
- **用途**: 测试工具调用和超时处理

### 7. testai-4.3
- **功能**: Sleep 工具调用（300秒）
- **响应**: 返回 `sleep(300秒)` 工具调用 → "工作完成"
- **延迟**: 0 秒
- **用途**: 测试长时间工具调用

### 8. testai-5.0 ⭐ 安全测试模型
- **功能**: 返回文件操作工具调用
- **响应**: 根据参数返回对应的文件操作工具调用
- **延迟**: 0 秒
- **用途**: 测试文件操作和安全审批功能
- **支持的操作**:
  - `file_read` - 读取文件
  - `file_write` - 写入文件
  - `file_delete` - 删除文件
  - `file_append` - 追加文件
  - `dir_create` - 创建目录
  - `dir_delete` - 删除目录
  - `dir_list` - 列出目录


## 快速开始

### 构建服务器

```bash
cd test/TestAIServer
go build -o testaiserver.exe
```

### 运行服务器

```bash
./testaiserver.exe
```

服务器将在 `http://0.0.0.0:8080` 启动，可以从本地或其他机器访问。

**本地访问**: `http://localhost:8080`
**远程访问**: `http://<your-ip>:8080`

### 使用帮助系统

TestAIServer 提供了分层帮助系统，方便快速查看模型信息：

```bash
# 显示帮助概览（推荐新手）
./testaiserver.exe --help

# 显示分类详情
./testaiserver.exe --help categories

# 显示所有模型列表
./testaiserver.exe models

# 显示特定模型的详细帮助
./testaiserver.exe --help testai-5.0

# 显示 API 使用说明
./testaiserver.exe --help api

# 快速参考
./testaiserver.exe --help quick
```

**帮助系统层级结构**：
1. **概览** (`--help`) - 按分类组织，快速了解所有模型
2. **分类详情** (`--help categories`) - 深入了解每个分类的模型
3. **模型详情** (`--help <模型名>`) - 特定模型的详细说明和使用示例
4. **API 文档** (`--help api`) - API 接口和 curl 示例
5. **快速参考** (`--help quick`) - 常用命令和模型速查表

### 使用环境变量配置端口

```bash
# Windows
set PORT=9090
testaiserver.exe

# Linux/macOS
PORT=9090 ./testaiserver
```

## API 使用示例

### 列出所有模型

```bash
curl http://localhost:8080/v1/models
```

响应示例：
```json
{
  "object": "list",
  "data": [
    {
      "id": "testai-1.1",
      "object": "model",
      "created": 1700000000,
      "owned_by": "test-ai-server"
    },
    {
      "id": "testai-1.2",
      "object": "model",
      "created": 1700000000,
      "owned_by": "test-ai-server"
    },
    ...
  ]
}
```

### 发送聊天请求

```bash
curl http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "testai-1.1",
    "messages": [
      {"role": "user", "content": "你好"}
    ]
  }'
```

响应示例：
```json
{
  "id": "chatcmpl-1700000000",
  "object": "chat.completion",
  "created": 1700000000,
  "model": "testai-1.1",
  "choices": [
    {
      "index": 0,
      "message": {
        "role": "assistant",
        "content": "好的，我知道了"
      },
      "finish_reason": "stop"
    }
  ],
  "usage": {
    "prompt_tokens": 2,
    "completion_tokens": 7,
    "total_tokens": 9
  }
}
```

### 测试延迟模型

```bash
# 30 秒延迟
curl http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "testai-1.2",
    "messages": [{"role": "user", "content": "测试延迟"}]
  }'

# 300 秒延迟（5 分钟）
curl http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "testai-1.3",
    "messages": [{"role": "user", "content": "测试超长延迟"}]
  }'
```

### 测试回显模型

```bash
curl http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "testai-2.0",
    "messages": [
      {"role": "user", "content": "这是测试消息"}
    ]
  }'
```

响应示例：
```json
{
  "id": "chatcmpl-1700000000",
  "object": "chat.completion",
  "created": 1700000000,
  "model": "testai-2.0",
  "choices": [
    {
      "index": 0,
      "message": {
        "role": "assistant",
        "content": "这是测试消息"
      },
      "finish_reason": "stop"
    }
  ],
  "usage": {
    "prompt_tokens": 6,
    "completion_tokens": 6,
    "total_tokens": 12
  }
}
```

### 测试安全文件操作模型（testai-5.0）⭐

```bash
curl http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "testai-5.0",
    "messages": [
      {"role": "user", "content": "<FILE_OP>{\"operation\":\"file_delete\",\"path\":\"/etc/passwd\",\"risk_level\":\"CRITICAL\"}</FILE_OP>"}
    ]
  }'
```

响应示例（返回工具调用）：
```json
{
  "id": "chatcmpl-1700000000",
  "object": "chat.completion",
  "created": 1700000000,
  "model": "testai-5.0",
  "choices": [
    {
      "index": 0,
      "message": {
        "role": "assistant",
        "content": "",
        "tool_calls": [
          {
            "id": "call-1234567890",
            "type": "function",
            "function": {
              "name": "file_delete",
              "arguments": "{\"path\":\"/etc/passwd\",\"risk_level\":\"CRITICAL\"}"
            }
          }
        ]
      },
      "finish_reason": "tool_calls"
    }
  ]
}
```

**testai-5.0 支持的文件操作**：

1. **file_read** - 读取文件
```json
<FILE_OP>{"operation":"file_read","path":"/etc/passwd","risk_level":"CRITICAL"}</FILE_OP>
```

2. **file_write** - 写入文件
```json
<FILE_OP>{"operation":"file_write","path":"/tmp/test.txt","content":"Hello World","risk_level":"HIGH"}</FILE_OP>
```

3. **file_delete** - 删除文件
```json
<FILE_OP>{"operation":"file_delete","path":"/tmp/test.txt","risk_level":"CRITICAL"}</FILE_OP>
```

4. **file_append** - 追加文件
```json
<FILE_OP>{"operation":"file_append","path":"/tmp/test.txt","content":"Append this","risk_level":"MEDIUM"}</FILE_OP>
```

5. **dir_create** - 创建目录
```json
<FILE_OP>{"operation":"dir_create","path":"/tmp/newdir","risk_level":"LOW"}</FILE_OP>
```

6. **dir_delete** - 删除目录
```json
<FILE_OP>{"operation":"dir_delete","path":"/tmp/olddir","risk_level":"HIGH"}</FILE_OP>
```

7. **dir_list** - 列出目录
```json
<FILE_OP>{"operation":"dir_list","path":"/tmp"}</FILE_OP>
```

### 测试流式响应 ⭐

所有模型都支持流式响应（SSE 格式）：

```bash
# 使用 curl 测试流式响应（需要 -N 参数禁用缓冲）
curl -N http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "testai-2.0",
    "messages": [
      {"role": "user", "content": "你好"}
    ],
    "stream": true
  }'
```

**流式响应格式**（SSE - Server-Sent Events）：
```
data: {"id":"chatcmpl-xxx","object":"chat.completion.chunk","created":1694268190,"model":"testai-2.0","choices":[{"index":0,"delta":{"role":"assistant"},"finish_reason":null}]}

data: {"id":"chatcmpl-xxx","object":"chat.completion.chunk","created":1694268190,"model":"testai-2.0","choices":[{"index":0,"delta":{"content":"你"},"finish_reason":null}]}

data: {"id":"chatcmpl-xxx","object":"chat.completion.chunk","created":1694268190,"model":"testai-2.0","choices":[{"index":0,"delta":{"content":"好"},"finish_reason":null}]}

data: {"id":"chatcmpl-xxx","object":"chat.completion.chunk","created":1694268190,"model":"testai-2.0","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}

data: [DONE]
```

**流式响应特点**：
- 逐字符发送，每字符间隔 10ms（模拟打字效果）
- 支持 `Content-Type: text/event-stream`
- 完整的 OpenAI SSE 格式兼容
- 所有 8 个模型均支持流式输出

## 在 NemesisBot 中使用

### 在 NemesisBot 中使用

```bash
# 添加测试模型（使用 --base 参数）
nemesisbot model add --model test/testai-1.1 --base http://127.0.0.1:8080/v1 --key test-key --default
```

## 测试场景

### 1. 基础功能测试
使用 `testai-1.1` 测试基本的消息处理流程。

### 2. 超时处理测试
使用 `testai-1.2` (30秒) 和 `testai-1.3` (300秒) 测试超时机制：
- 验证 30 秒超时配置
- 验证 60 秒超时配置
- 测试超时后的错误处理

### 3. 消息验证测试
使用 `testai-2.0` 验证消息是否正确传递：
- 验证消息格式
- 验证消息内容完整性
- 验证多轮对话

### 4. 并发测试
同时发送多个请求到不同模型，测试并发处理能力。

### 5. 压力测试
使用 `testai-1.1` 进行高频请求测试。

### 6. 日志记录测试 ⭐ NEW
测试请求日志记录功能：
- 验证日志目录自动创建
- 验证日志文件格式
- 验证请求信息完整性

```bash
# Windows
test_logging.bat

# Linux/macOS
./test_logging.sh
```

详细说明请查看 `LOGGING.md` 文档。

## 项目结构

```
TestAIServer/
├── main.go              # 主程序入口
├── main_test.go         # 单元测试和集成测试
├── help_system.go       # 分层帮助系统
├── middleware.go         # HTTP 中间件
├── go.mod               # Go 模块定义
├── go.sum               # 依赖校验
├── README.md            # 本文档
├── test_security.bat    # 安全测试脚本
├── docs/
│   ├── KNOWN_ISSUES.md  # 已知问题清单
│   ├── PROJECT_SUMMARY.md
│   ├── DOCUMENTATION_INDEX.md
│   ├── QUICKSTART.md
│   ├── QUICK_REFERENCE.md
│   ├── CHANGELOG.md
│   ├── LOGGING.md
│   └── ...              # 其他文档
├── models/
│   ├── types.go         # 类型定义
│   └── test_models.go   # 八个测试模型实现
├── handlers/
│   └── handlers.go      # HTTP 请求处理器（含流式响应）
└── logger/
    └── logger.go        # 日志记录器
```

## 注意事项

1. **延迟模型**: `testai-1.3` 会阻塞 300 秒，仅用于测试超时处理
2. **Token 计数**: 使用简单的字符计数，非真实的 tokenizer
3. **并发限制**: 没有并发限制，所有请求都会被处理
4. **流式响应**: 完整支持，使用 `stream: true` 启用

## 开发说明

### 添加新模型

1. 在 `models/test_models.go` 中实现 `Model` 接口：
```go
type NewModel struct{}

func (m *NewModel) Name() string {
    return "new-model"
}

func (m *NewModel) Process(messages []Message) string {
    // 实现逻辑
}

func (m *NewModel) Delay() time.Duration {
    return 0
}
```

2. 在 `main.go` 中注册：
```go
registry.Register(models.NewNewModel())
```

### 自定义配置

修改 `main.go` 中的路由配置：
```go
// 修改端口
router.Run(":9090")

// 或使用环境变量
port := os.Getenv("PORT")
if port == "" {
    port = "8080"
}
router.Run(":" + port)
```

## 许可证

内部测试工具，仅供 NemesisBot 项目测试使用。
