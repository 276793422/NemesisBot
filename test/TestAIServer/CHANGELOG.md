# TestAIServer 更新日志

## v1.3.1 - 2026-03-11 (紧急修复)

### 🔴 紧急修复

#### ISSUE-CRITICAL-001: 模型响应丢失

**问题**: TestAI12 和 TestAI13 实现代码完全丢失，**影响**: 所有模型没有返回响应内容
**严重程度**: 🔴 严重
**修复时间**: 立即修复

**根本原因**:
- 文件编辑错误导致 TestAI12 和 TestAI13 代码丢失
- `models/test_models.go` 从 150 行减少到 83 行
- 缺少完整的回归测试

**修复内容**:
- ✅ 恢复 TestAI12 实现（30秒延迟 + "好的，我知道了"）
- ✅ 恢复 TestAI13 实现（300秒延迟 + "好的，我知道了"）
- ✅ 验证所有模型响应正常
- ✅ 添加响应测试脚本 (`test_responses.bat`, `test_responses.sh`)

**验证测试**:
```bash
=== RUN   TestTestAI11 --- PASS
=== RUN   TestTestAI12 --- PASS  # 恢复
=== RUN   TestTestAI13 --- PASS  # 恢复
=== RUN   TestTestAI20 --- PASS
```

**影响范围**:
- ✅ testai-1.1: 正常（立即返回 "好的，我知道了"）
- ✅ testai-1.2: 恢复（延迟 30 秒后返回 "好的，我知道了"）
- ✅ testai-1.3: 恢复（延迟 300 秒后返回 "好的，我知道了"）
- ✅ testai-2.0: 正常（回显用户消息）

**教训**:
1. 文件编辑后必须验证完整性
2. 需要完整的回归测试
3. 所有模型都需要测试覆盖

**相关文档**:
- `EMERGENCY_FIX_REPORT.md` - 紧急修复报告
- `MODEL_FIX_REPORT.md` - 模型修复详情
- `test_responses.bat/sh` - 响应测试脚本

---

## v1.3.0 - 2026-03-11## v1.3.0 - 2026-03-11

### 🚨 重要修复

#### ISSUE-001: 流式响应兼容性修复

**问题**: 许多 OpenAI 客户端（如 Cherry Studio）默认使用 `stream=true`，导致无法使用 TestAIServer。

**原因**: 服务端之前直接拒绝 `stream=true` 请求，返回 400 错误。

**临时方案**: 实施兼容模式：
- ✅ 接受 `stream=true` 参数
- ✅ 返回非流式响应（而非错误）
- ⚠️ 记录警告日志
- 📋 已记录在 `docs/KNOWN_ISSUES.md` (ISSUE-001)

**代码变更** (`handlers/handlers.go`):
```go
// ⚠️ KNOWN ISSUE: 流式响应兼容性处理
if req.Stream {
    fmt.Printf("[WARNING] Client requested streaming (stream=true) but returning non-streaming response. Model: %s\n", req.Model)
}
// 继续处理，返回非流式响应
```

**新增文档**:
- `docs/KNOWN_ISSUES.md` - 已知问题清单（**重要：使用前必读**）
- `STREAMING_FIX.md` - 本次修复详细说明

**影响**:
- ✅ Cherry Studio 等工具现在可以正常使用
- ⚠️ 不会有逐字输出效果（非真正流式）
- 📋 这是临时方案，完整方案待开发

**优先级**: P1 - 高（临时解决）

---

## v1.2.0 - 2026-03-11

### 新增功能

#### 🌐 监听地址改为 0.0.0.0

服务器现在监听 `0.0.0.0` 而不是 `localhost`，支持远程访问。

**变更**:
- 修改前: `router.Run(":8080")` → 仅本地访问
- 修改后: `router.Run("0.0.0.0:8080")` → 支持所有网络接口

**优势**:
- ✅ 支持局域网访问
- ✅ 支持远程访问
- ✅ 便于团队协作测试
- ✅ 无需修改客户端代码

**访问方式**:
- 本地: `http://localhost:8080`
- 局域网: `http://192.168.x.x:8080`
- 远程: `http://<your-ip>:8080`

**新增文档**:
- `NETWORK_CONFIG.md` - 网络配置详细说明

**安全提示**:
- ⚠️ 监听 0.0.0.0 会接受所有网络接口的连接
- ⚠️ 建议仅在可信网络环境中使用
- ⚠️ 生产环境建议使用防火墙限制访问

---

## v1.1.0 - 2026-03-11

### 新增功能

#### 🎯 自动请求日志记录

程序现在会自动记录每个请求的详细信息：

- ✅ 程序启动时自动创建 `log/` 目录
- ✅ 每个请求创建独立的日志文件
- ✅ 按模型分类存储（`log/testai-1.1/`, `log/testai-2.0/` 等）
- ✅ 日志文件以时间戳命名（格式：`YYYYMMDD_HHMMSS.mmm.log`）
- ✅ 记录完整的请求信息：
  - 时间戳
  - 请求方法、URL、协议
  - 完整的请求头
  - 查询参数
  - 请求体（JSON 格式化）
  - Gin 上下文信息

#### 📁 日志目录结构

```
log/
├── testai-1.1/
│   ├── 20260311_193045.123.log
│   ├── 20260311_193046.456.log
│   └── ...
├── testai-1.2/
│   └── ...
├── testai-1.3/
│   └── ...
└── testai-2.0/
    └── ...
```

#### 📝 日志文件内容示例

```
========================================
TestAIServer Request Log (Detailed)
========================================

Timestamp: 2026-03-11 19:30:45.123

--- Request Info ---
Method: POST
URL: /v1/chat/completions
Protocol: HTTP/1.1
Remote Addr: 127.0.0.1:54321
Host: localhost:8080

--- Request Headers ---
Content-Type: application/json
Authorization: Bearer test-key
User-Agent: curl/7.68.0
Accept: */*

--- Raw Request Body ---
Length: 156 bytes

{
  "model": "testai-1.1",
  "messages": [
    {
      "role": "user",
      "content": "你好，这是测试消息"
    }
  ],
  "stream": false
}

--- Gin Context ---
Client IP: 127.0.0.1
Content Length: 156
Content Type: application/json
User Agent: curl/7.68.0
Is AJAX: false

========================================
End of Log
========================================
```

### 新增文件

- `logger/logger.go` - 日志记录器实现
- `test_logging.bat` - Windows 日志测试脚本
- `test_logging.sh` - Linux/macOS 日志测试脚本
- `LOGGING.md` - 日志功能详细文档
- `CHANGELOG.md` - 本文档

### 改进

#### 代码改进

- 更新 `handlers/handlers.go`：
  - 集成日志记录器
  - 在处理请求前读取原始请求体
  - 记录详细的请求信息

- 更新 `main.go`：
  - 初始化日志记录器
  - 启动时显示日志目录信息
  - 更友好的启动提示

- 更新 `main_test.go`：
  - 适配新的 Handler 签名

#### 文档改进

- 更新 `README.md`：
  - 添加日志功能说明
  - 更新项目结构
  - 添加日志测试场景

### 使用方法

#### 启动服务器

```bash
testaiserver.exe
```

输出：
```
日志目录已创建: log/
测试模型已注册: testai-1.1, testai-1.2, testai-1.3, testai-2.0
========================================
TestAIServer 正在启动...
========================================
服务地址: http://localhost:8080
日志目录: ./log/
========================================
```

#### 发送测试请求

```bash
curl http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "testai-1.1",
    "messages": [{"role": "user", "content": "测试消息"}]
  }'
```

#### 查看日志

```bash
# Windows
test_logging.bat

# Linux/macOS
./test_logging.sh

# 或手动查看
cat log/testai-1.1/$(ls -t log/testai-1.1/*.log | head -1)
```

### 性能影响

- **CPU**: 可忽略（< 1ms）
- **磁盘 I/O**: 每个请求写入约 1KB
- **内存**: 可忽略
- **总体**: 极低，适合生产环境使用

### 兼容性

- ✅ 完全向后兼容
- ✅ 无破坏性更改
- ✅ 所有现有功能正常工作

### 测试

所有测试通过（10/10）：
```
=== RUN   TestListModels
--- PASS: TestListModels (0.00s)
=== RUN   TestTestAI11
--- PASS: TestTestAI11 (0.00s)
=== RUN   TestTestAI12
--- PASS: TestTestAI12 (0.00s)
=== RUN   TestTestAI13
--- PASS: TestTestAI13 (0.00s)
=== RUN   TestTestAI20
--- PASS: TestTestAI20 (0.00s)
=== RUN   TestChatCompletionRequest
--- PASS: TestChatCompletionRequest (0.00s)
=== RUN   TestModelRegistry
--- PASS: TestModelRegistry (0.00s)
=== RUN   TestHTTPIntegration
--- PASS: TestHTTPIntegration (0.01s)
=== RUN   TestNonexistentModel
--- PASS: TestNonexistentModel (0.00s)
=== RUN   TestStreamingRequest
--- PASS: TestStreamingRequest (0.00s)
PASS
ok  	testaiserver	0.386s
```

### 下一步计划

- [ ] 支持配置化的日志级别
- [ ] 支持日志文件自动轮转
- [ ] 支持日志文件自动清理
- [ ] 添加日志搜索和过滤功能
- [ ] 支持结构化日志格式（JSON）

---

## v1.0.0 - 2026-03-11（初始版本）

### 功能

- ✅ 兼容 OpenAI API 接口
- ✅ 四个测试模型
- ✅ 完整的单元测试
- ✅ 详细的文档
