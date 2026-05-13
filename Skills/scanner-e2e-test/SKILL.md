---
name: scanner-e2e-test
description: Scanner 杀毒引擎端到端自动化测试。验证 ClamAV 从配置、下载、安装、启动、扫描到拦截的完整生命周期。
---

# Scanner E2E Test

## 概述

验证 Scanner 模块完整生命周期：配置 → 下载安装 → 启动 → 扫描放行 → 病毒拦截。

**适用场景**: Scanner 模块代码变更后执行全量回归测试。

---

## 强制要求

1. **所有路径必须使用正斜杠** `C:/path/to/file`，禁止反斜杠。WebSocket 消息中的路径、命令行参数中的路径均如此。
2. **禁止在命令中使用项目绝对路径**: 所有 `cd`、编译输出路径等均使用相对路径。唯一允许的绝对路径是测试文件目标 `C:/Zoo/Temp/test`（与项目无关的外部路径）和 WebSocket 消息中的文件路径。
3. **工作目录流转**: Phase 1-2 在项目根目录，Phase 3-5 在 `test/autotest/`，Phase 5 末尾返回项目根目录。**每个阶段开始时执行 `pwd` 验证当前目录**。
4. **使用项目自带的 WebSocket 客户端**: `test/websocket_chat_client.go`，不使用 Python 脚本。
5. **编译 Bot 使用 `go build -a`**: 确保 `-a` 强制重编译，`-tags "production,powershell"` 启用 PowerShell 兼容。
6. **EICAR 测试必须使用 Go 脚本发送**: EICAR 字符串含 `\` 和 `!`，bash 命令行参数传递会导致 JSON 转义错误。用 `go run send_eicar.go` 替代 `websocket_chat_client.exe`。

---

## 前置条件

| 条件 | 说明 |
|------|------|
| Go 编译器 | 系统已安装 |
| 网络 | 需能访问 `database.clamav.net`（下载病毒库）；官网下载失败时回退本地 |
| 本地安装包（回退用） | `Skills/scanner-e2e-test/clamav-1.5.2.win.x64.zip` |

**端口**: 8080（测试 AI）、49001（Bot WebSocket）、3310（ClamAV）、9999（本地 HTTP 回退，按需）

---

## Phase 1: 环境准备

**每个 Step 开始前验证**: `pwd` 应在项目根目录（`NemesisBot/NemesisBot`）。

### Step 1: 编译测试 AI 服务

```bash
pwd
mkdir -p test/autotest
cd test/TestAIServer && go build -o ../../test/autotest/testaiserver.exe .
```

**验证**: `ls test/autotest/testaiserver.exe` 存在。

### Step 2: 编译无弹窗 Bot

```bash
pwd
go build -a -tags "production,powershell" -ldflags "-s -w" -o test/autotest/nemesisbot.exe ./nemesisbot/
```

`-a` 强制全部重编译，`-tags "production,powershell"` 启用 PowerShell 兼容。直接输出到 `test/autotest/`，无需额外复制。

### Step 3: 编译 WebSocket 测试客户端

```bash
pwd
go build -o test/autotest/websocket_chat_client.exe test/websocket_chat_client.go
```

### Step 4: 启动测试 AI 服务

```bash
cd test/autotest
pwd
./testaiserver.exe > testai.log 2>&1 &
```

**验证**: `curl.exe -s http://127.0.0.1:8080/v1/models` 返回模型列表包含 testai-5.0。

### Step 5: 配置本地 Bot

```bash
cd test/autotest
pwd
./nemesisbot.exe --local onboard default
./nemesisbot.exe --local model add --model test/testai-5.0 --base http://127.0.0.1:8080/v1 --key test-key --default
```

**验证**: `grep restrict_to_workspace .nemesisbot/config.json` 确认值为 `false`。

---

## Phase 2: Scanner 配置与安装

**每个 Step 开始前验证**: `pwd` 应在 `test/autotest/`。

### Step 6: 启用 clamav 引擎

```bash
pwd
./nemesisbot.exe --local security scanner enable clamav
```

### Step 7: 检查引擎环境

```bash
./nemesisbot.exe --local security scanner check
```

**预期**: `install=pending  db=missing`。

### Step 8: 安装引擎（官网 → 本地回退）

优先从 ClamAV 官网下载。若下载失败（超时、网络错误），自动回退到本地 HTTP 服务。

```bash
# 8a: 尝试从官网安装
./nemesisbot.exe --local security scanner add clamav --url https://www.clamav.net/downloads/production/clamav-1.5.2.win.x64.zip
./nemesisbot.exe --local security scanner install

# 8b: 若上述失败，执行本地回退
if [ $? -ne 0 ] || ! ./nemesisbot.exe --local security scanner check 2>&1 | grep -q "install=installed"; then
    echo "官方下载失败，启用本地回退"
    mkdir -p http-srv
    cp ../../Skills/scanner-e2e-test/clamav-1.5.2.win.x64.zip http-srv/
    python -m http.server 9999 --directory http-srv &
    ./nemesisbot.exe --local security scanner add clamav --url http://127.0.0.1:9999/clamav-1.5.2.win.x64.zip
    ./nemesisbot.exe --local security scanner install
fi
```

**说明**:
- 本地安装包路径使用相对路径 `../../Skills/scanner-e2e-test/clamav-1.5.2.win.x64.zip`（从 `test/autotest` 出发）
- 本地回退使用 Python HTTP 服务（端口 9999），Phase 5 清理时需停止
- `scanner install` 会自动完成：下载 zip → 解压 → 递归检测 `clamd.exe` → freshclam 下载病毒库 → 验证

### Step 9: 验证安装结果

```bash
./nemesisbot.exe --local security scanner check
```

**预期**: `install=installed  db=ready`。

---

## Phase 3: Bot 启动与验证

**每个 Step 开始前验证**: `pwd` 应在 `test/autotest/`。

### Step 10: 端口和进程预检查

```bash
tasklist | grep -i clamd && taskkill //F //IM clamd.exe
netstat -ano | grep ":3310 " | grep LISTEN
```

### Step 11: 启动 Bot

```bash
pwd
./nemesisbot.exe --local gateway > nemesisbot.log 2>&1 &
```

等待约 10 秒（首次启动需下载病毒库）。

### Step 12: 验证 clamd 进程

```bash
tasklist | grep -i clamd
netstat -ano | grep ":3310 " | grep LISTEN
```

**预期**: clamd.exe 进程存在，端口 3310 LISTENING。

**验证日志**:
```bash
grep "ClamAV daemon started and ready" nemesisbot.log
grep "Scanner chain initialized" nemesisbot.log
```

---

## Phase 4: 扫描功能验证

**每个 Step 开始前验证**: `pwd` 应在 `test/autotest/`。

### 前置: 创建测试目录

```bash
mkdir -p C:/Zoo/Temp/test
```

### Step 13: 干净文件放行

从 `nemesisbot.log` 中获取 Auth Token（`🔑 Auth Token: XXXXXXXXX`）。

```bash
pwd
./websocket_chat_client.exe ws://127.0.0.1:49001/ws '<FILE_OP>{"operation":"file_write","path":"C:/Zoo/Temp/test/clean.txt","content":"hello world"}</FILE_OP>'
```

**验证**:
```bash
cat C:/Zoo/Temp/test/clean.txt
# 预期: hello world

grep "virus detected" nemesisbot.log
# 预期: 无输出
```

### Step 14: 感染文件拦截

**注意**: EICAR 字符串包含 `\` 和 `!`，bash 命令行参数传递会导致 JSON 转义错误。必须使用 Go 脚本直接发送，确保 JSON 中 `\` 正确编码为 `\\`。

在 `test/autotest/` 下创建 `send_eicar.go`（固定内容，不需要命令行参数）：

```go
package main
import (
    "encoding/json"
    "fmt"
    "log"
    "time"
    "github.com/gorilla/websocket"
)
func main() {
    content := `<FILE_OP>{"operation":"file_write","path":"C:/Zoo/Temp/test/eicar.exe","content":"X5O!P%@AP[4\\PZX54(P^)7CC)7}$EICAR-STANDARD-ANTIVIRUS-TEST-FILE!$H+H*"}</FILE_OP>`
    conn, _, err := websocket.DefaultDialer.Dial("ws://127.0.0.1:49001/ws", nil)
    if err != nil { log.Fatal(err) }
    defer conn.Close()
    msg := map[string]string{"type": "message", "content": content, "timestamp": time.Now().Format(time.RFC3339)}
    data, _ := json.Marshal(msg)
    conn.WriteMessage(websocket.TextMessage, data)
    fmt.Println("EICAR sent")
    time.Sleep(15 * time.Second)
}
```

```bash
pwd
go run send_eicar.go
```

**验证**:
```bash
ls C:/Zoo/Temp/test/eicar.exe 2>/dev/null || echo "PASS: file not created"

grep "virus detected" nemesisbot.log
# 预期: 包含 Eicar-Test-Signature
```

### Step 15: 验证拦截反馈

```bash
grep "Tool execution failed" nemesisbot.log
# 预期: virus detected by clamav: C:/Zoo/Temp/test/eicar.exe (virus: Eicar-Test-Signature)
```

---

## Phase 5: 收尾

### Step 16: 清理环境

```bash
taskkill //F //IM nemesisbot.exe
taskkill //F //IM clamd.exe
taskkill //F //IM testaiserver.exe

# 若 Step 8 启用了本地 HTTP 回退，停止 Python 进程
taskkill //F //IM python.exe 2>/dev/null

# 先切出测试目录再删除（工作目录在 test/autotest 内时 Windows 会锁住目录）
cd ../..
pwd
rm -rf test/autotest
rm -rf C:/Zoo/Temp/test
```

**验证**: `ls test/autotest 2>/dev/null && echo "FAIL: 目录残留" || echo "PASS: 已清理"`

### Step 17: 分析日志

| 日志项 | 期望内容 |
|--------|---------|
| Bot 启动日志 | `ClamAV daemon started and ready` |
| 扫描放行日志 | clean file 无 virus detected |
| 扫描拦截日志 | `virus detected`, `Eicar-Test-Signature` |
| 错误日志 | 无 unexpected error |

### Step 18: 输出测试报告

测试完成后将报告保存到 `docs/REPORT/`，文件名格式 `YYYY-MM-DD_Scanner_E2E_Test_Report.md`。
