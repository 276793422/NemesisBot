# TestAIServer 网络配置更新

## ✅ 更新完成

已成功将服务器监听地址从 `localhost` 改为 `0.0.0.0`。

---

## 🔄 变更内容

### 修改前

```go
router.Run(":8080")
```
- 监听: `localhost:8080`
- 访问: 仅本地

### 修改后

```go
router.Run("0.0.0.0:8080")
```
- 监听: `0.0.0.0:8080`
- 访问: 所有网络接口（本地 + 远程）

---

## 📋 访问方式

### 本地访问

```bash
# 方式 1: localhost
curl http://localhost:8080/v1/models

# 方式 2: 127.0.0.1
curl http://127.0.0.1:8080/v1/models
```

### 局域网访问

```bash
# 假设服务器 IP 是 192.168.1.100
curl http://192.168.1.100:8080/v1/models
```

### 远程访问

```bash
# 假设服务器公网 IP 是 203.0.113.50
curl http://203.0.113.50:8080/v1/models
```

---

## 🚀 启动服务器

```bash
testaiserver.exe
```

**输出**:
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

---

## 💡 使用场景

### 1. 本地开发测试

```bash
# 在同一台机器上
curl http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model": "testai-1.1", "messages": [{"role": "user", "content": "测试"}]}'
```

### 2. 局域网测试

```bash
# 在另一台机器上访问
# 假设服务器 IP: 192.168.1.100
curl http://192.168.1.100:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model": "testai-1.1", "messages": [{"role": "user", "content": "测试"}]}'
```

### 3. 与 NemesisBot 集成

**本地集成**:
```bash
nemesisbot model add \
  --model testai-1.1 \
  --base-url http://localhost:8080/v1 \
  --key test-key
```

**远程集成**:
```bash
# NemesisBot 在另一台机器上
nemesisbot model add \
  --model testai-1.1 \
  --base-url http://192.168.1.100:8080/v1 \
  --key test-key
```

---

## ⚠️ 安全注意事项

### 重要提醒

监听 `0.0.0.0` 意味着接受来自所有网络接口的连接，请注意安全！

### 建议措施

1. **仅用于开发测试**
   - ✅ 本地开发环境
   - ✅ 可信的局域网
   - ⚠️ 公网环境需谨慎

2. **防火墙配置**
   ```bash
   # Windows: 允许 8080 端口
   netsh advfirewall firewall add rule name="TestAIServer" dir=in action=allow protocol=tcp localport=8080

   # Linux (ufw): 允许局域网访问
   sudo ufw allow from 192.168.1.0/24 to any port 8080
   ```

3. **网络安全**
   - 确保在可信网络环境中运行
   - 不要在公共 WiFi 上使用
   - 生产环境使用反向代理

---

## 🔍 验证测试

### 测试 1: 本地访问

```bash
curl http://localhost:8080/v1/models
```

**预期结果**: 返回模型列表

### 测试 2: 检查监听地址

```bash
# Windows
netstat -an | findstr 8080

# Linux/macOS
netstat -an | grep 8080
```

**预期结果**: 显示 `0.0.0.0:8080` 或 `:::8080`

### 测试 3: 局域网访问

从另一台机器：
```bash
curl http://<server-ip>:8080/v1/models
```

**预期结果**: 返回模型列表

---

## 📚 相关文档

| 文档 | 说明 |
|------|------|
| README.md | 主文档（已更新） |
| QUICKSTART.md | 快速启动（已更新） |
| NETWORK_CONFIG.md | 网络配置详细说明（新增） |
| CHANGELOG.md | 更新日志（已更新） |

---

## 🛠️ 故障排查

### 问题: 无法从其他机器访问

**检查步骤**:

1. **确认服务器正在运行**
   ```bash
   netstat -an | grep 8080
   ```

2. **测试本地访问**
   ```bash
   curl http://localhost:8080/v1/models
   ```

3. **检查防火墙**
   ```bash
   # Windows
   netsh advfirewall firewall show rule name=all | findstr 8080

   # Linux
   sudo ufw status
   ```

4. **测试网络连通性**
   ```bash
   # 从客户端机器
   ping <server-ip>
   telnet <server-ip> 8080
   ```

### 问题: 端口被占用

```bash
# 查找占用进程
# Windows
netstat -ano | findstr :8080

# Linux/macOS
lsof -i :8080
```

---

## ✅ 测试结果

### 单元测试

```
=== RUN   TestListModels
--- PASS: TestListModels (0.00s)
...
PASS
ok  	testaiserver	0.386s
```

**状态**: ✅ 全部通过（10/10）

### 构建结果

**状态**: ✅ 成功

---

## 📊 更新摘要

| 项目 | 状态 |
|------|------|
| 代码修改 | ✅ 完成 |
| 文档更新 | ✅ 完成 |
| 测试验证 | ✅ 通过 |
| 构建验证 | ✅ 成功 |

---

## 🎉 总结

服务器现在监听 `0.0.0.0:8080`，支持从任何网络接口访问。

**使用建议**:
- ✅ 本地开发测试
- ✅ 局域网团队协作
- ⚠️ 注意网络安全

**下一步**:
1. 启动服务器
2. 从本地测试访问
3. 从局域网其他机器测试
4. 集成到 NemesisBot

---

**更新日期**: 2026-03-11
**版本**: v1.2.0
**变更**: 监听地址 localhost → 0.0.0.0
**状态**: ✅ 完成并测试通过
