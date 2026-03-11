# TestAIServer 配置更新

## 更新内容

已将服务器监听地址从 `localhost` 改为 `0.0.0.0`。

---

## 变更详情

### 修改前

```go
router.Run(":8080")
```

监听地址: `localhost:8080`（仅本地访问）

### 修改后

```go
router.Run("0.0.0.0:8080")
```

监听地址: `0.0.0.0:8080`（所有网络接口）

---

## 影响说明

### ✅ 新增功能

1. **局域网访问**
   - 同一局域网内的其他设备可以访问
   - 例如: `http://192.168.1.100:8080`

2. **远程访问**
   - 可以从其他机器访问服务器
   - 适合测试和开发场景

3. **灵活性**
   - 支持本地和远程访问
   - 无需修改客户端代码

### 📋 访问方式

#### 本地访问

```bash
curl http://localhost:8080/v1/models
curl http://127.0.0.1:8080/v1/models
```

#### 局域网访问

```bash
# 假设服务器 IP 是 192.168.1.100
curl http://192.168.1.100:8080/v1/models
```

#### 远程访问

```bash
# 假设服务器公网 IP 是 203.0.113.50
curl http://203.0.113.50:8080/v1/models
```

---

## 使用示例

### 启动服务器

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
服务地址: http://0.0.0.0:8080
日志目录: ./log/
========================================

[GIN-debug] Listening and serving HTTP on 0.0.0.0:8080
```

### 本地测试

```bash
curl http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model": "testai-1.1", "messages": [{"role": "user", "content": "测试"}]}'
```

### 局域网测试

从另一台机器访问：

```bash
# 查看服务器 IP（在服务器上运行）
# Windows: ipconfig
# Linux/macOS: ifconfig 或 ip addr

# 假设服务器 IP 是 192.168.1.100
curl http://192.168.1.100:8080/v1/models
```

---

## 与 NemesisBot 集成

### 本地集成

```bash
nemesisbot model add \
  --model testai-1.1 \
  --base-url http://localhost:8080/v1 \
  --key test-key
```

### 远程集成

如果 NemesisBot 在另一台机器上：

```bash
nemesisbot model add \
  --model testai-1.1 \
  --base-url http://192.168.1.100:8080/v1 \
  --key test-key
```

---

## 安全注意事项

### ⚠️ 安全警告

监听 `0.0.0.0` 意味着服务器接受来自所有网络接口的连接，请注意：

1. **仅用于开发和测试**
   - 不应该在生产环境直接暴露
   - 建议使用防火墙限制访问

2. **局域网安全**
   - 确保局域网是可信的
   - 避免在公共 WiFi 上使用

3. **防火墙配置**
   ```bash
   # Windows: 允许特定端口
   netsh advfirewall firewall add rule name="TestAIServer" dir=in action=allow protocol=tcp localport=8080

   # Linux: 使用 ufw
   sudo ufw allow 8080/tcp

   # 仅允许特定 IP 访问
   sudo ufw allow from 192.168.1.0/24 to any port 8080
   ```

4. **生产环境建议**
   - 使用反向代理（如 Nginx）
   - 添加身份认证
   - 使用 HTTPS
   - 限制访问 IP

---

## 防火墙配置示例

### Windows 防火墙

```powershell
# 允许 8080 端口入站
netsh advfirewall firewall add rule name="TestAIServer 8080" dir=in action=allow protocol=tcp localport=8080

# 查看规则
netsh advfirewall firewall show rule name="TestAIServer 8080"

# 删除规则
netsh advfirewall firewall delete rule name="TestAIServer 8080"
```

### Linux 防火墙 (ufw)

```bash
# 允许 8080 端口
sudo ufw allow 8080/tcp

# 仅允许局域网访问
sudo ufw allow from 192.168.1.0/24 to any port 8080

# 查看状态
sudo ufw status

# 删除规则
sudo ufw delete allow 8080/tcp
```

### Linux 防火墙 (firewalld)

```bash
# 允许 8080 端口
sudo firewall-cmd --add-port=8080/tcp --permanent
sudo firewall-cmd --reload

# 查看规则
sudo firewall-cmd --list-ports

# 删除规则
sudo firewall-cmd --remove-port=8080/tcp --permanent
sudo firewall-cmd --reload
```

---

## 网络配置

### 查看本机 IP

**Windows**:
```bash
ipconfig
# 或
ipconfig | findstr "IPv4"
```

**Linux/macOS**:
```bash
ifconfig
# 或
ip addr show
# 或
hostname -I
```

### 测试连通性

**从客户端机器测试**:
```bash
# 测试端口是否开放
telnet 192.168.1.100 8080

# 或使用 nc (netcat)
nc -zv 192.168.1.100 8080

# 测试 HTTP 请求
curl http://192.168.1.100:8080/v1/models
```

---

## 故障排查

### 问题 1: 无法从其他机器访问

**可能原因**:
1. 防火墙阻止了端口
2. 服务器未正确绑定 0.0.0.0
3. 网络不通

**解决方案**:
```bash
# 1. 检查服务器是否在运行
netstat -an | grep 8080

# 2. 检查防火墙
# Windows: 检查防火墙规则
netsh advfirewall firewall show rule name=all | findstr 8080

# Linux: 检查防火墙状态
sudo ufw status

# 3. 测试本地访问
curl http://localhost:8080/v1/models

# 4. 测试远程访问
curl http://<server-ip>:8080/v1/models
```

### 问题 2: 服务器启动失败

**错误信息**: `bind: address already in use`

**解决方案**:
```bash
# 查找占用端口的进程
# Windows
netstat -ano | findstr :8080

# Linux/macOS
lsof -i :8080

# 结束占用端口的进程
# Windows: 使用任务管理器或
taskkill /PID <pid> /F

# Linux/macOS
kill -9 <pid>
```

---

## 高级配置

### 绑定特定 IP

如果只想绑定特定网络接口：

```go
// 仅绑定到特定 IP
router.Run("192.168.1.100:8080")
```

### 动态配置

使用环境变量配置：

```go
host := os.Getenv("HOST")
if host == "" {
    host = "0.0.0.0"
}
port := os.Getenv("PORT")
if port == "" {
    port = "8080"
}
router.Run(host + ":" + port)
```

运行：
```bash
# Windows
set HOST=0.0.0.0
set PORT=8080
testaiserver.exe

# Linux/macOS
HOST=0.0.0.0 PORT=8080 ./testaiserver
```

---

## 性能优化

### 高并发配置

```go
// 在 main.go 中
router := gin.Default()

// 配置 HTTP 服务器
srv := &http.Server{
    Addr:         "0.0.0.0:8080",
    Handler:      router,
    ReadTimeout:  10 * time.Second,
    WriteTimeout: 10 * time.Second,
}

srv.ListenAndServe()
```

---

## 总结

### ✅ 优势

1. **灵活访问** - 支持本地和远程访问
2. **测试便利** - 可以在多台机器间测试
3. **开发友好** - 适合团队协作开发

### ⚠️ 注意事项

1. **安全风险** - 需要适当的安全措施
2. **防火墙** - 确保防火墙配置正确
3. **网络环境** - 确保网络环境可信

### 📋 建议

- ✅ 开发环境: 使用 `0.0.0.0`
- ✅ 测试环境: 使用 `0.0.0.0` + 防火墙
- ⚠️ 生产环境: 使用反向代理 + 认证

---

**更新日期**: 2026-03-11
**版本**: v1.2.0
**变更**: 监听地址从 localhost 改为 0.0.0.0
