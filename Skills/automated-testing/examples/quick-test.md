# 快速测试示例

本文档提供了一个完整的测试执行示例，展示如何使用辅助脚本快速执行测试。

---

## 完整测试流程（推荐）

### Windows PowerShell

```powershell
# === 确保在项目根目录 ===
cd C:\AI\NemesisBot\NemesisBot_Rust

# === 阶段 2: 环境准备（脚本） ===
.\Skills\automated-testing\scripts\setup-env.ps1

# === 阶段 3: 本地初始化（AI） ===
cd test-tools\autotest
.\nemesisbot.exe onboard default --local

# === 阶段 4: 配置 AI（AI） ===
.\nemesisbot.exe model add `
    --model test/testai-1.1 `
    --base http://127.0.0.1:8080/v1 `
    --key test-key `
    --default

# === 阶段 5: 启动 Bot（AI） ===
.\nemesisbot.exe gateway

# === 阶段 6: 执行测试（AI - 另一个终端） ===
# 在新的 PowerShell 窗口中：
cd C:\AI\NemesisBot\NemesisBot_Rust\test-tools\autotest
.\websocket_chat_client.exe

# === 阶段 7: 清理环境（混合） ===
# 返回项目根目录
cd C:\AI\NemesisBot\NemesisBot_Rust

# 停止服务（脚本）
.\Skills\automated-testing\scripts\cleanup-env.ps1

# 删除测试目录（AI）
Remove-Item -Recurse -Force test-tools\autotest
```

### Git Bash / Linux

```bash
# === 确保在项目根目录 ===
cd /c/AI/NemesisBot/NemesisBot_Rust

# === 阶段 2: 环境准备（脚本） ===
bash Skills/automated-testing/scripts/setup-env.sh

# === 阶段 3: 本地初始化（AI） ===
cd test-tools/autotest
./nemesisbot.exe onboard default --local

# === 阶段 4: 配置 AI（AI） ===
./nemesisbot.exe model add \
    --model test/testai-1.1 \
    --base http://127.0.0.1:8080/v1 \
    --key test-key \
    --default

# === 阶段 5: 启动 Bot（AI） ===
./nemesisbot.exe gateway &

# === 阶段 6: 执行测试（AI） ===
./websocket_chat_client.exe

# === 阶段 7: 清理环境（混合） ===
# 返回项目根目录
cd /c/AI/NemesisBot/NemesisBot_Rust

# 停止服务（脚本）
bash Skills/automated-testing/scripts/cleanup-env.sh

# 删除测试目录（AI）
rm -rf test-tools/autotest
```

---

## 快速验证测试

如果只需要快速验证功能是否正常，可以使用以下简化流程：

```bash
# 1. 环境准备
bash Skills/automated-testing/scripts/setup-env.sh

# 2. 一行命令初始化、配置、启动
cd test-tools/autotest && \
  ./nemesisbot.exe onboard default --local && \
  ./nemesisbot.exe model add --model test/testai-1.1 --base http://127.0.0.1:8080/v1 --key test-key --default && \
  ./nemesisbot.exe gateway &

# 3. 等待启动后运行测试
sleep 5
./websocket_chat_client.exe

# 4. 清理
cd ../.. && \
  bash Skills/automated-testing/scripts/cleanup-env.sh && \
  rm -rf test-tools/autotest
```

---

## 预期输出

### setup-env.ps1 输出

```
SETUP_START
Creating test-tools\autotest directory...
Compiling test components...
[1/3] Compiling TestAIServer...
[2/3] Compiling NemesisBot...
[3/3] Compiling WebSocket client...
Compilation successful
Starting TestAIServer...
Waiting for TestAIServer to be ready...
SETUP_SUCCESS
TESTAI_PID=7256
TESTAI_PORT=8080
WORK_DIR=C:\AI\NemesisBot\NemesisBot_Rust\test-tools\autotest

Environment setup completed successfully!
TestAIServer is running with PID: 7256
TestAIServer endpoint: http://127.0.0.1:8080/v1
```

### websocket_chat_client.exe 输出

```
连接到 ws://127.0.0.1:49001/ws...
✅ 已连接

📤 发送消息: 你好，请简单介绍一下自己
✅ 消息已发送

⏳ 等待响应...

📥 收到第 1 条消息:
   类型: message
   角色: system
   内容: Connected to NemesisBot WebSocket channel. Client ID: client_xxx

📥 收到第 2 条消息:
   类型: message
   角色: assistant
   内容: 好的，我知道了

============================================================
✅ 测试通过：消息收发功能正常
============================================================
```

### cleanup-env.ps1 输出

```
CLEANUP_START
Stopping NemesisBot...
Stopping TestAIServer...
Stopping TestAIServer (PID: 7256)...
Waiting for file handles to be released...
All processes stopped successfully
CLEANUP_SUCCESS

Environment cleanup completed!
Note: test-tools\autotest\ directory was not removed (AI should handle this)
```

---

## 常见问题

### 1. 端口被占用

**错误**: `address already in use`

**解决**:
```bash
# 查找占用进程
netstat -ano | grep 8080
netstat -ano | grep 49001

# 停止进程或使用 cleanup-env 脚本
bash Skills/automated-testing/scripts/cleanup-env.sh
```

### 2. 编译失败

**错误**: `cargo: command not found`

**解决**: 确保 Rust 工具链（cargo）已安装并在 PATH 中

### 3. 权限错误

**错误**: `Access denied`

**解决**: 使用管理员权限运行 PowerShell

### 4. 进程未停止

**解决**: 手动停止进程
```powershell
Stop-Process -Name "nemesisbot" -Force
Stop-Process -Name "testaiserver" -Force
```

---

**更新日期**: 2026-05-15
**状态**: ✅ 已验证
