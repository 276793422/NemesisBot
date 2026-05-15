# TestAIServer 快速启动指南

## 快速开始（3 步）

### 1. 构建

```bash
cd test/TestAIServer
build.bat
```

或手动构建：
```bash
go build -o testaiserver.exe
```

### 2. 启动服务器

```bash
testaiserver.exe
```

服务器将在 `http://0.0.0.0:8080` 启动。

输出：
```
[GIN-debug] GET    /v1/models                --> testaiserver/handlers.(*Handler).ListModels (3 handlers)
[GIN-debug] POST   /v1/chat/completions      --> testaiserver/handlers.(*Handler).ChatCompletions (3 handlers)
[GIN-debug] Listening and serving HTTP on 0.0.0.0:8080
```

**访问方式**:
- 本地访问: `http://localhost:8080`
- 远程访问: `http://<your-ip>:8080`

### 3. 测试

#### Windows:
```bash
test_api.bat
```

#### Linux/macOS:
```bash
chmod +x test_api.sh
./test_api.sh
```

## 快速测试命令

### 列出所有模型

```bash
curl http://localhost:8080/v1/models
```

### 测试 testai-1.1（立即响应）

```bash
curl http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d "{\"model\": \"testai-1.1\", \"messages\": [{\"role\": \"user\", \"content\": \"你好\"}]}"
```

预期响应：
```json
{
  "id": "chatcmpl-...",
  "object": "chat.completion",
  "created": 1700000000,
  "model": "testai-1.1",
  "choices": [{
    "index": 0,
    "message": {
      "role": "assistant",
      "content": "好的，我知道了"
    },
    "finish_reason": "stop"
  }],
  "usage": {...}
}
```

### 测试 testai-2.0（回显消息）

```bash
curl http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d "{\"model\": \"testai-2.0\", \"messages\": [{\"role\": \"user\", \"content\": \"这是测试消息\"}]}"
```

预期响应：返回 "这是测试消息"

### 测试 testai-1.2（30秒延迟）

```bash
curl http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d "{\"model\": \"testai-1.2\", \"messages\": [{\"role\": \"user\", \"content\": \"测试延迟\"}]}"
```

注意：此命令将等待 30 秒后才返回响应。

### 测试 testai-1.3（300秒延迟）

```bash
curl http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d "{\"model\": \"testai-1.3\", \"messages\": [{\"role\": \"user\", \"content\": \"测试超长延迟\"}]}"
```

注意：此命令将等待 300 秒（5 分钟）后才返回响应。建议在后台运行。

## 运行测试

### 运行所有单元测试

```bash
go test -v
```

### 运行基准测试

```bash
go test -bench=.
```

### 运行特定测试

```bash
go test -v -run TestTestAI11
go test -v -run TestHTTPIntegration
```

## 与 NemesisBot 集成

### 1. 启动 TestAIServer

```bash
cd test/TestAIServer
testaiserver.exe
```

### 2. 在 NemesisBot 中添加测试模型

```bash
# 添加 testai-1.1
nemesisbot model add --model testai-1.1 --base-url http://localhost:8080/v1 --key test-key

# 添加 testai-2.0
nemesisbot model add --model testai-2.0 --base-url http://localhost:8080/v1 --key test-key
```

### 3. 使用测试模型

```bash
nemesisbot chat --model testai-1.1 "你好"
```

## 测试场景

### 场景 1: 测试基本功能

使用 `testai-1.1` 测试消息处理流程。

### 场景 2: 测试超时处理

使用 `testai-1.2` (30秒) 测试超时配置是否生效：
- 如果超时设置 < 30 秒，应该触发超时错误
- 如果超时设置 > 30 秒，应该正常返回

### 场景 3: 测试消息传递

使用 `testai-2.0` 验证消息是否正确传递：
- 发送消息
- 验证返回的消息与发送的一致

### 场景 4: 测试并发

同时发送多个请求：
```bash
# 终端 1
curl http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d "{\"model\": \"testai-1.1\", \"messages\": [{\"role\": \"user\", \"content\": \"请求1\"}]}"

# 终端 2
curl http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d "{\"model\": \"testai-1.1\", \"messages\": [{\"role\": \"user\", \"content\": \"请求2\"}]}"
```

### 场景 5: 测试错误处理

请求不存在的模型：
```bash
curl http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d "{\"model\": \"nonexistent\", \"messages\": [{\"role\": \"user\", \"content\": \"测试\"}]}"
```

预期：返回 404 错误。

## 端口配置

### 修改端口

编辑 `main.go`：

```go
func main() {
    // ... 其他代码 ...

    // 修改为其他端口
    router.Run("0.0.0.0:9090")
}
```

或使用环境变量：

```go
port := os.Getenv("PORT")
if port == "" {
    port = "8080"
}
router.Run("0.0.0.0:" + port)
```

然后运行：
```bash
# Windows
set PORT=9090
testaiserver.exe

# Linux/macOS
PORT=9090 ./testaiserver
```

### 绑定地址

默认绑定 `0.0.0.0`，允许所有网络接口访问：
- 本地: `http://localhost:8080`
- 局域网: `http://192.168.x.x:8080`
- 所有接口: `http://0.0.0.0:8080`

如需仅本地访问，修改为：
```go
router.Run("127.0.0.1:8080")
```

## 常见问题

### Q1: 端口被占用怎么办？

**A**: 修改端口号或关闭占用端口的程序。

Windows 查看端口占用：
```bash
netstat -ano | findstr :8080
```

### Q2: 如何停止服务器？

**A**: 按 `Ctrl+C` 停止服务器。

### Q3: 请求一直不返回怎么办？

**A**:
- 检查是否使用了 `testai-1.2` 或 `testai-1.3`（有延迟）
- 检查客户端超时设置
- 使用 `testai-1.1` 或 `testai-2.0` 进行快速测试

### Q4: 如何查看日志？

**A**: Gin 框架会自动输出请求日志到控制台。

### Q5: 支持 HTTPS 吗？

**A**: 当前版本仅支持 HTTP。如需 HTTPS，可以修改 `main.go`：

```go
router.RunTLS(":8443", "cert.pem", "key.pem")
```

## 项目文件说明

```
TestAIServer/
├── main.go              # 主程序，启动 HTTP 服务器
├── main_test.go         # 单元测试和集成测试
├── go.mod               # Go 模块定义
├── go.sum               # 依赖校验文件
├── build.bat            # Windows 构建脚本
├── test_api.bat         # Windows 测试脚本
├── test_api.sh          # Linux/macOS 测试脚本
├── README.md            # 详细文档
├── QUICKSTART.md        # 本文档
├── models/
│   ├── types.go         # 类型定义和模型注册表
│   └── test_models.go   # 四个测试模型的实现
└── handlers/
    └── handlers.go      # HTTP 请求处理器
```

## 下一步

1. ✅ 启动服务器并运行测试脚本
2. ✅ 在 NemesisBot 中添加测试模型
3. ✅ 编写自动化测试用例
4. ✅ 集成到 CI/CD 流程

## 技术支持

如有问题，请查看：
- `README.md` - 详细文档
- `main_test.go` - 测试用例示例
- NemesisBot 项目文档
