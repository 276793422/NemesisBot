# Test Tools

本目录包含用于测试的辅助工具和服务器。

## 工具列表

### 1. test_http_server

通用的HTTP测试服务器，用于测试OAuth流程、HTTP客户端和Webhook。

**构建**:
```bash
cd test_http_server
go build -o test_http_server.exe
```

**运行**:
```bash
# 默认端口 8081
./test_http_server.exe

# 自定义端口
./test_http_server.exe 9999
```

**端点**:
- `GET /` - 服务器状态
- `POST /echo` - 回显请求（包括headers、body、query）
- `GET /delay/{seconds}` - 延迟响应（用于测试超时）
- `GET /status/{code}` - 返回指定HTTP状态码
- `GET /oauth/callback` - OAuth回调端点
- `POST /oauth/device` - 设备码端点
- `POST /oauth/token` - Token交换端点
- `POST /webhook` - 通用Webhook端点
- `GET /api/tools/list` - MCP工具列表
- `GET /api/resources/list` - MCP资源列表

### 2. auth_test_server

专门用于测试OAuth认证流程的HTTP服务器。

**构建**:
```bash
cd auth_test_server
go build -o auth_test_server.exe
```

**运行**:
```bash
# 默认端口 8082
./auth_test_server.exe

# 自定义端口
./auth_test_server.exe 9000
```

**用途**:
- 测试device code流程
- 测试token交换
- 模拟OAuth提供商响应

### 3. channel_webhook_server

模拟各种消息通道的Webhook端点。

**构建**:
```bash
cd channel_webhook_server
go build -o channel_webhook_server.exe
```

**运行**:
```bash
# 默认端口 8083
./channel_webhook_server.exe

# 自定义端口
./channel_webhook_server.exe 8080
```

**支持的通道**:
- `POST /telegram/webhook` - Telegram Bot API
- `POST /line/webhook` - LINE Messaging API
- `POST /slack/events` - Slack Events API
- `POST /discord/webhook` - Discord webhook
- `GET /onebot/ws` - OneBot WebSocket (升级)
- `GET /files/download?file=test.txt` - 文件下载测试

---

## 使用示例

### 测试OAuth流程

```bash
# 1. 启动auth测试服务器
cd test/tools/auth_test_server
./auth_test_server.exe &

# 2. 运行auth测试
cd ../../../
go test ./module/auth -v -run TestOAuth
```

### 测试通道Webhook

```bash
# 1. 启动webhook服务器
cd test/tools/channel_webhook_server
./channel_webhook_server.exe &

# 2. 运行channels测试
cd ../../../
go test ./module/channels -v -run TestWebhook
```

### 集成测试

```bash
# 同时启动所有测试服务器
cd test/tools

# 启动HTTP服务器
cd test_http_server && ./test_http_server.exe &

# 启动Auth服务器
cd ../auth_test_server && ./auth_test_server.exe &

# 启动Webhook服务器
cd ../channel_webhook_server && ./channel_webhook_server.exe &

# 运行所有测试
cd ../../..
go test ./module/... -v
```

---

## 端口分配

| 工具 | 默认端口 | 用途 |
|------|---------|------|
| test_http_server | 8081 | 通用HTTP测试 |
| auth_test_server | 8082 | OAuth认证测试 |
| channel_webhook_server | 8083 | 通道Webhook测试 |

---

## 故障排除

### 端口已被占用

如果看到类似错误：
```
listen tcp :8081: bind: address already in use
```

解决方法：
```bash
# Windows
netstat -ano | findstr :8081
taskkill /PID <PID> /F

# 或使用不同的端口
./test_http_server.exe 9999
```

### 服务器无法启动

检查防火墙设置，确保端口未被阻止。

### 测试超时

某些测试可能需要较长时间。可以增加测试超时时间：
```bash
go test -timeout 5m ./module/auth
```
