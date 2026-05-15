# 日志记录功能说明

## 概述

TestAIServer 现在支持完整的请求日志记录功能。每次收到请求时，会自动记录请求的详细信息到日志文件中。

## 功能特性

### ✅ 自动日志记录

- 程序启动时自动在当前目录创建 `log/` 目录
- 每个请求自动创建独立的日志文件
- 按模型分类存储日志文件
- 日志文件以时间戳命名，精确到毫秒

### 📁 目录结构

```
log/
├── testai-1.1/
│   ├── 20260311_193045.123.log
│   ├── 20260311_193046.456.log
│   └── ...
├── testai-1.2/
│   ├── 20260311_193050.789.log
│   └── ...
├── testai-1.3/
│   └── ...
└── testai-2.0/
    └── ...
```

### 📄 日志文件格式

日志文件名格式：`YYYYMMDD_HHMMSS.mmm.log`

- `YYYYMMDD`: 日期（年月日）
- `HHMMSS`: 时间（时分秒）
- `mmm`: 毫秒

示例：`20260311_193045.123.log`

## 日志内容

每个日志文件包含以下信息：

### 1. 时间戳

```
Timestamp: 2026-03-11 19:30:45.123
```

### 2. 请求信息

```
--- Request Info ---
Method: POST
URL: /v1/chat/completions
Protocol: HTTP/1.1
Remote Addr: 127.0.0.1:54321
Host: localhost:8080
```

### 3. 请求头

```
--- Request Headers ---
Content-Type: application/json
Authorization: Bearer test-key
User-Agent: curl/7.68.0
Accept: *//
Content-Length: 156
X-Custom-Header: custom-value
```

### 4. 查询参数（如果有）

```
--- Query Parameters ---
version: v1
debug: true
```

### 5. 请求体（JSON 格式）

```
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
```

### 6. Gin 上下文信息

```
--- Gin Context ---
Client IP: 127.0.0.1
Content Length: 156
Content Type: application/json
User Agent: curl/7.68.0
Is AJAX: false
```

## 使用方法

### 1. 启动服务器

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

[GIN-debug] Listening and serving HTTP on :8080
```

### 2. 发送测试请求

```bash
curl http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "testai-1.1",
    "messages": [{"role": "user", "content": "测试消息"}]
  }'
```

### 3. 查看日志

```bash
# 查看最新的日志文件
cat log/testai-1.1/$(ls -t log/testai-1.1/*.log | head -1)

# 或者使用测试脚本
./test_logging.bat  # Windows
./test_logging.sh   # Linux/macOS
```

## 日志文件示例

### 完整日志示例

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
Authorization: Bearer sk-test-key-12345
User-Agent: NemesisBot/1.0
Accept: application/json
Content-Length: 234

--- Raw Request Body ---
Length: 234 bytes

{
  "model": "testai-1.1",
  "messages": [
    {
      "role": "system",
      "content": "你是一个AI助手"
    },
    {
      "role": "user",
      "content": "请帮我测试一下"
    }
  ],
  "stream": false
}

--- Gin Context ---
Client IP: 127.0.0.1
Content Length: 234
Content Type: application/json
User Agent: NemesisBot/1.0
Is AJAX: false

========================================
End of Log
========================================
```

## 测试脚本

### Windows

```bash
test_logging.bat
```

功能：
- 发送多个测试请求到不同模型
- 检查 log 目录结构
- 显示最新的日志文件内容

### Linux/macOS

```bash
chmod +x test_logging.sh
./test_logging.sh
```

## 日志管理

### 日志文件大小

- 每个日志文件大小：约 500-2000 字节（取决于请求内容）
- 无自动清理机制
- 建议定期清理旧日志

### 清理日志

```bash
# 删除所有日志
rm -rf log/

# 删除特定模型的日志
rm -rf log/testai-1.1/

# 删除 7 天前的日志（Linux/macOS）
find log/ -name "*.log" -mtime +7 -delete
```

### 日志轮转

当前版本不支持自动日志轮转，建议：
1. 定期手动清理
2. 使用外部工具（如 logrotate）
3. 在 CI/CD 中自动清理

## 调试和故障排查

### 查看特定模型的所有请求

```bash
# 查看所有 testai-1.1 的请求
ls -lt log/testai-1.1/

# 统计请求数量
ls log/testai-1.1/*.log | wc -l
```

### 搜索特定内容

```bash
# 搜索包含特定消息的请求
grep -r "测试消息" log/

# 搜索特定 User-Agent
grep -r "NemesisBot" log/

# 搜索特定时间段的请求
find log/ -name "20260311_19*.log"
```

### 监控实时日志

```bash
# 监控新创建的日志文件
watch -n 1 'ls -lt log/testai-1.1/ | head -5'

# 或者使用 tail 监控最新日志
tail -f $(ls -t log/testai-1.1/*.log | head -1)
```

## 性能影响

### 日志记录开销

- **CPU**: 可忽略（< 1ms）
- **磁盘 I/O**: 每个请求写入约 1KB
- **内存**: 可忽略

### 建议

- ✅ 开发和测试环境：始终启用
- ✅ 生产环境：可以启用（影响很小）
- ⚠️ 高并发场景：考虑定期清理

## 隐私和安全

### 敏感信息

日志中可能包含：
- ✅ 请求头（包括 Authorization）
- ✅ 完整的请求体
- ⚠️ API Keys 和敏感数据

### 建议

1. **不要提交日志到版本控制**
   ```bash
   # 添加到 .gitignore
   echo "log/" >> .gitignore
   ```

2. **限制日志目录访问权限**
   ```bash
   chmod 700 log/
   ```

3. **定期清理敏感日志**
   ```bash
   # 清理包含敏感信息的日志
   grep -l "Bearer sk-" log/*/*.log | xargs rm
   ```

## 故障排查

### 问题：日志目录未创建

**原因**: 权限不足或磁盘空间不足

**解决方案**:
```bash
# 手动创建
mkdir -p log

# 检查权限
ls -ld log/
```

### 问题：日志文件为空

**原因**: 请求体读取失败

**解决方案**: 检查请求格式是否正确

### 问题：日志文件过多

**原因**: 长时间运行未清理

**解决方案**:
```bash
# 删除旧日志
find log/ -name "*.log" -mtime +30 -delete
```

## 技术实现

### 核心组件

- **logger/logger.go**: 日志记录器实现
- **handlers/handlers.go**: 集成日志记录
- **main.go**: 初始化日志记录器

### 关键函数

```go
// 创建日志记录器
func NewLogger() (*Logger, error)

// 记录请求详细信息
func (l *Logger) LogRequestDetails(c *gin.Context, modelName string, rawBody []byte) error
```

### 实现细节

1. 在处理请求前读取原始请求体
2. 创建模型专属目录
3. 生成时间戳文件名
4. 格式化并写入日志
5. 恢复请求体以供后续处理

## 最佳实践

### 1. 定期清理

```bash
# 每周清理一次
0 0 * * 0 find log/ -name "*.log" -mtime +7 -delete
```

### 2. 日志分析

```bash
# 统计每个模型的请求数
for model in log/*/; do
    echo "$(basename $model): $(ls $model*.log 2>/dev/null | wc -l) requests"
done
```

### 3. 监控异常请求

```bash
# 查找错误请求
grep -r "error" log/
```

## 总结

TestAIServer 的日志记录功能提供了：

- ✅ 完整的请求追踪
- ✅ 详细的调试信息
- ✅ 按模型分类存储
- ✅ 零配置使用
- ✅ 极低的性能影响

这使得它成为测试和调试 AI 应用的理想工具。
