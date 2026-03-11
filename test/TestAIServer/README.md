# TestAIServer

一个兼容 OpenAI API 的测试服务器，提供四个硬编码的测试模型用于测试目的。

## ⚠️ 重要提示

**使用前请务必阅读**:
- 📋 [已知问题清单](docs/KNOWN_ISSUES.md) - **必读！**
- 🔧 [流式响应修复说明](STREAMING_FIX.md) - v1.3.0 重要更新

### 当前限制

- ⚠️ **不支持真正的流式响应**（已实施兼容模式）
- ⚠️ 详见 [ISSUE-001](docs/KNOWN_ISSUES.md#issue-001-不支持流式响应streaming-response)

---

## NemesisBot 测试命令

nemesisbot model add --model test/testai-1.1 --base http://127.0.0.1:8080/v1 --key test-key --default

---

## 功能特性

- ✅ 完全兼容 OpenAI API 接口
- ✅ 支持 `/v1/chat/completions` 端点
- ✅ 支持 `/v1/models` 端点
- ✅ 四个预定义的测试模型
- ✅ 支持延迟响应测试
- ✅ **自动请求日志记录** ⭐ NEW
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

## 在 NemesisBot 中使用

### 添加测试模型

```bash
# 添加 testai-1.1
nemesisbot model add \
  --model testai-1.1 \
  --base-url http://localhost:8080/v1 \
  --key test-key

# 添加 testai-1.2
nemesisbot model add \
  --model testai-1.2 \
  --base-url http://localhost:8080/v1 \
  --key test-key

# 添加 testai-1.3
nemesisbot model add \
  --model testai-1.3 \
  --base-url http://localhost:8080/v1 \
  --key test-key

# 添加 testai-2.0
nemesisbot model add \
  --model testai-2.0 \
  --base-url http://localhost:8080/v1 \
  --key test-key
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
├── go.mod               # Go 模块定义
├── go.sum               # 依赖校验
├── README.md            # 本文档
├── STREAMING_FIX.md     # 流式响应修复说明 ⭐ v1.3.0
├── LOGGING.md           # 日志功能详细文档
├── QUICKSTART.md        # 快速启动指南
├── CHANGELOG.md         # 更新日志
├── docs/
│   └── KNOWN_ISSUES.md  # ⚠️ 已知问题清单（必读）
├── models/
│   ├── types.go         # 类型定义
│   └── test_models.go   # 测试模型实现
├── handlers/
│   └── handlers.go      # HTTP 请求处理器
├── logger/
│   └── logger.go        # 日志记录器
└── log/                 # 日志目录（运行时自动创建）
    ├── testai-1.1/
    ├── testai-1.2/
    ├── testai-1.3/
    └── testai-2.0/
```

## 注意事项

1. **延迟模型**: `testai-1.3` 会阻塞 300 秒，仅用于测试超时处理
2. **流式响应**: 当前版本不支持流式响应（stream=true）
3. **Token 计数**: 使用简单的字符计数，非真实的 tokenizer
4. **并发限制**: 没有并发限制，所有请求都会被处理

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
