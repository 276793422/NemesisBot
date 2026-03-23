# 阶段 2: 环境准备

准备测试所需的所有环境和组件。

---

## 步骤

### 2.1 编译 TestAIServer

**目的**: 编译测试用 AI 服务器

**命令**:
```bash
# 进入 TestAIServer 目录
cd test/TestAIServer

# 编译
go build -o testaiserver.exe

# 验证编译结果
if [ ! -f "testaiserver.exe" ]; then
  echo "❌ TestAIServer 编译失败"
  exit 1
fi

echo "✅ TestAIServer 编译成功"
ls -lh testaiserver.exe
```

**预期输出**:
```
✅ TestAIServer 编译成功
-rwxr-xr-x 1 user group 2.5M Mar 23 10:30 testaiserver.exe
```

---

### 2.2 启动 TestAIServer

**目的**: 在后台启动测试 AI 服务器

**命令**:
```bash
# 后台启动 TestAIServer
./testaiserver.exe &
TESTAI_PID=$!

echo "TestAIServer PID: $TESTAI_PID"

# 保存 PID 到文件（可选）
echo $TESTAI_PID > /tmp/testaiserver.pid

# 验证进程
ps -p $TESTAI_PID > /dev/null
if [ $? -ne 0 ]; then
  echo "❌ TestAIServer 进程未运行"
  exit 1
fi

echo "✅ TestAIServer 进程已启动"
```

**预期输出**:
```
TestAIServer PID: 12345
✅ TestAIServer 进程已启动
```

---

### 2.3 等待 TestAIServer 就绪

**目的**: 等待服务器完全启动并可以接受请求

**命令**:
```bash
# 等待 2 秒让服务器启动
sleep 2

# 测试服务器是否就绪
for i in {1..10}; do
  if curl -s http://127.0.0.1:8080/v1/models > /dev/null 2>&1; then
    echo "✅ TestAIServer 已就绪"
    break
  fi

  if [ $i -eq 10 ]; then
    echo "❌ TestAIServer 启动超时"
    exit 1
  fi

  echo "等待 TestAIServer 就绪... ($i/10)"
  sleep 1
done
```

**预期输出**:
```
✅ TestAIServer 已就绪
```

**验证测试**:
```bash
# 测试模型列表端点
curl http://127.0.0.1:8080/v1/models
```

**预期响应**:
```json
{
  "object": "list",
  "data": [
    {
      "id": "testai-1.1",
      "object": "model"
    },
    {
      "id": "testai-5.0",
      "object": "model"
    }
  ]
}
```

---

### 2.4 编译 NemesisBot

**目的**: 编译主程序

**命令**:
```bash
# 返回主目录
cd ../../

# 编译
go build -o nemesisbot.exe ./nemesisbot

# 验证编译结果
if [ ! -f "nemesisbot.exe" ]; then
  echo "❌ NemesisBot 编译失败"
  exit 1
fi

echo "✅ NemesisBot 编译成功"
ls -lh nemesisbot.exe
```

**预期输出**:
```
✅ NemesisBot 编译成功
-rwxr-xr-x 1 user group 15M Mar 23 10:32 nemesisbot.exe
```

---

### 2.5 环境验证

**目的**: 验证所有组件都已正确准备

**验证清单**:

```bash
echo "=== 环境验证 ==="

# 1. 检查 TestAIServer 进程
echo "[1/4] TestAIServer 进程..."
if ps -p $TESTAI_PID > /dev/null; then
  echo "✅ TestAIServer 进程运行中 (PID: $TESTAI_PID)"
else
  echo "❌ TestAIServer 进程未运行"
  exit 1
fi

# 2. 检查 TestAIServer 端口
echo "[2/4] TestAIServer 端口..."
if netstat -an | grep 8080 > /dev/null; then
  echo "✅ 端口 8080 已监听"
else
  echo "❌ 端口 8080 未监听"
  exit 1
fi

# 3. 检查 TestAIServer API
echo "[3/4] TestAIServer API..."
if curl -s http://127.0.0.1:8080/v1/models > /dev/null; then
  echo "✅ TestAIServer API 可访问"
else
  echo "❌ TestAIServer API 不可访问"
  exit 1
fi

# 4. 检查 NemesisBot 可执行文件
echo "[4/4] NemesisBot 可执行文件..."
if [ -x "./nemesisbot.exe" ]; then
  echo "✅ NemesisBot 可执行文件存在"
else
  echo "❌ NemesisBot 可执行文件不存在"
  exit 1
fi

echo ""
echo "=== 环境验证完成，所有检查通过 ==="
```

---

## 故障排查

### 问题 1: TestAIServer 启动失败

**症状**: `testaiserver.exe` 进程未运行

**可能原因**:
- 端口 8080 被占用
- 编译错误
- 权限问题

**解决方案**:
```bash
# 检查端口占用
netstat -ano | grep 8080

# 如果被占用，杀死占用进程
kill -9 <PID>

# 或者使用其他端口
# 需要修改 TestAIServer 代码或配置
```

---

### 问题 2: NemesisBot 编译失败

**症状**: `go build` 返回错误

**可能原因**:
- 依赖缺失
- 代码错误
- Go 版本不兼容

**解决方案**:
```bash
# 更新依赖
go mod tidy

# 检查 Go 版本
go version

# 清理并重新编译
go clean -cache
go build -o nemesisbot.exe ./nemesisbot
```

---

### 问题 3: TestAIServer API 不响应

**症状**: `curl` 返回错误

**可能原因**:
- 服务器未完全启动
- 防火墙阻止
- 路径配置错误

**解决方案**:
```bash
# 检查服务器日志
# 如果 TestAIServer 有日志文件

# 检查防火墙
# Windows: 检查 Windows Defender 防火墙
# Linux: check iptables

# 增加等待时间
sleep 5
```

---

## 环境准备脚本

```bash
#!/bin/bash
# prepare_env.sh - 环境准备脚本

set -e  # 遇到错误立即退出

echo "=== 开始环境准备 ==="

# 1. 编译 TestAIServer
echo "[1/5] 编译 TestAIServer..."
cd test/TestAIServer
go build -o testaiserver.exe
echo "✅ TestAIServer 编译完成"

# 2. 启动 TestAIServer
echo "[2/5] 启动 TestAIServer..."
./testaiserver.exe &
TESTAI_PID=$!
echo "TestAIServer PID: $TESTAI_PID"

# 3. 等待就绪
echo "[3/5] 等待 TestAIServer 就绪..."
sleep 2
for i in {1..10}; do
  if curl -s http://127.0.0.1:8080/v1/models > /dev/null 2>&1; then
    echo "✅ TestAIServer 已就绪"
    break
  fi
  [ $i -eq 10 ] && { echo "❌ 启动超时"; exit 1; }
  sleep 1
done

# 4. 编译 NemesisBot
echo "[4/5] 编译 NemesisBot..."
cd ../../
go build -o nemesisbot.exe ./nemesisbot
echo "✅ NemesisBot 编译完成"

# 5. 验证环境
echo "[5/5] 验证环境..."
ps -p $TESTAI_PID > /dev/null || { echo "❌ TestAIServer 未运行"; exit 1; }
[ -x "./nemesisbot.exe" ] || { echo "❌ NemesisBot 不存在"; exit 1; }

echo ""
echo "=== 环境准备完成 ==="
echo "TestAIServer PID: $TESTAI_PID"
echo "NemesisBot: ./nemesisbot.exe"

# 保存 PID
echo $TESTAI_PID > /tmp/testaiserver.pid
```

---

## 检查点

**环境准备完成检查点**:

- [ ] TestAIServer 编译成功
- [ ] TestAIServer 进程运行中
- [ ] TestAIServer 端口监听
- [ ] TestAIServer API 可访问
- [ ] NemesisBot 编译成功
- [ ] NemesisBot 可执行文件存在
- [ ] PID 已保存（用于后续清理）

**状态**: ✅ 通过 / ❌ 失败

---

**下一步**: 阶段 3 - 本地环境初始化
