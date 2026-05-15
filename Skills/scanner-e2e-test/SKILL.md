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
3. **工作目录流转**: Phase 1-2 在项目根目录，Phase 3-5 在 `test-tools/autotest/`，Phase 5 末尾返回项目根目录。**每个阶段开始时执行 `pwd` 验证当前目录**。
4. **使用 `ws-send` 工具发送 WebSocket 消息**: `test-tools/ws-send/`（Rust 项目），编译后使用。禁止使用 Python 脚本。
5. **编译 Bot 使用 `cargo build --release`**: Release 模式编译。
6. **EICAR 测试使用 `ws-send`**: `ws-send` 内部处理 JSON 协议封装，bash 只需传原始消息内容。EICAR 字符串中的反斜杠通过单引号保护。

---

## 前置条件

| 条件 | 说明 |
|------|------|
| Rust 工具链 | 系统已安装（rustc + cargo） |
| Go 编译器 | TestAIServer 仍为 Go 项目，需要 Go 编译 |
| 网络 | 需能访问 `database.clamav.net`（下载病毒库）；官网下载失败时回退本地 |
| 本地安装包（回退用） | `Skills/scanner-e2e-test/clamav-1.5.2.win.x64.zip` |

**端口**: 8080（测试 AI）、49000（Bot Web + WebSocket）、3310（ClamAV）、9999（本地 HTTP 回退，按需）

---

## Phase 1: 环境准备

**每个 Step 开始前验证**: `pwd` 应在项目根目录（`NemesisBot/NemesisBot_Rust`）。

### Step 1: 编译测试 AI 服务

```bash
pwd
mkdir -p test-tools/autotest
cd test-tools/TestAIServer && go build -o ../../test-tools/autotest/testaiserver.exe .
cd ../../..
```

**注意**: TestAIServer 是 Go 项目，仍在自身模块目录中使用 `go build .` 编译。如果已有预编译的 `testaiserver.exe`，可直接复制：
```bash
cp test-tools/TestAIServer/testaiserver.exe test-tools/autotest/
```

**验证**: `ls test-tools/autotest/testaiserver.exe` 存在。

### Step 2: 编译 NemesisBot

```bash
pwd
cargo build --release -p nemesisbot
cp target/release/nemesisbot.exe test-tools/autotest/
```

Release 模式编译，复制到测试工作目录。

### Step 3: 编译 ws-send 工具

```bash
pwd
cargo build --release -p ws-send
cp target/release/ws-send.exe test-tools/autotest/
```

`ws-send` 是 one-shot WebSocket 消息发送工具，自动封装 NemesisBot 的三级协议格式 `{type:"message", module:"chat", cmd:"send", data:{content:"..."}}`。

### Step 4: 启动测试 AI 服务

```bash
cd test-tools/autotest
pwd
./testaiserver.exe > testai.log 2>&1 &
```

**验证**: `curl.exe -s http://127.0.0.1:8080/v1/models` 返回模型列表包含 testai-5.0。

### Step 5: 配置本地 Bot

```bash
cd test-tools/autotest
pwd
./nemesisbot.exe --local onboard default
./nemesisbot.exe --local model add --model test/testai-5.0 --base http://127.0.0.1:8080/v1 --key test-key --default
```

**验证**: `grep restrict_to_workspace .nemesisbot/config.json` 确认值为 `false`。

记录 `onboard` 输出中的 Auth Token（如 `276793422`），后续 `ws-send` 需要使用。

---

## Phase 2: Scanner 配置与安装

**每个 Step 开始前验证**: `pwd` 应在 `test-tools/autotest/`。

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
- 本地安装包路径使用相对路径 `../../Skills/scanner-e2e-test/clamav-1.5.2.win.x64.zip`（从 `test-tools/autotest` 出发）
- 本地回退使用 Python HTTP 服务（端口 9999），Phase 5 清理时需停止
- `scanner install` 会自动完成：下载 zip → 解压 → 递归检测 `clamd.exe` → 生成 freshclam.conf + clamd.conf → freshclam 下载病毒库 → 验证

### Step 9: 验证安装结果

```bash
./nemesisbot.exe --local security scanner check
```

**预期**: `install=installed  db=ready`。

---

## Phase 3: Bot 启动与验证

**每个 Step 开始前验证**: `pwd` 应在 `test-tools/autotest/`。

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

等待约 12 秒（ClamAV daemon 加载病毒库需要时间）。

### Step 12: 验证 clamd 进程

```bash
tasklist | grep -i clamd
netstat -ano | grep ":3310 " | grep LISTEN
```

**预期**: clamd.exe 进程存在，端口 3310 LISTENING。

**验证日志**:
```bash
grep -i "clamav\|scanner\|daemon" nemesisbot.log
```

---

## Phase 4: 扫描功能验证

**每个 Step 开始前验证**: `pwd` 应在 `test-tools/autotest/`。

### 前置: 获取 Auth Token

```bash
grep -oP 'Auth Token: \K\d+' nemesisbot.log || grep -o 'access key: [0-9]*' nemesisbot.log
```

记下 Token 值（如 `276793422`），以下命令中用 `TOKEN` 代替。

### 前置: 创建测试目录

```bash
mkdir -p C:/Zoo/Temp/test
```

### Step 13: 干净文件放行

```bash
pwd
./ws-send.exe --url ws://127.0.0.1:49000/ws --token TOKEN --msg '<FILE_OP>{"operation":"file_write","path":"C:/Zoo/Temp/test/clean.txt","content":"hello world"}</FILE_OP>'
```

**验证**:
```bash
cat C:/Zoo/Temp/test/clean.txt
# 预期: hello world

grep "virus detected" nemesisbot.log
# 预期: 无输出
```

### Step 14: 感染文件拦截

**注意**: EICAR 字符串中的反斜杠在 JSON 中需要双重转义（`\\P`），直接通过 bash 命令行传递会产生转义层级混乱。使用 `--file` 从文件读取消息内容，彻底避免 shell 转义问题。

```bash
pwd
# 写入消息文件（注意 content 值中的 \\P 是 JSON 转义，代表实际的 \P）
cat > eicar_msg.txt << 'ENDMSG'
<FILE_OP>{"operation":"file_write","path":"C:/Zoo/Temp/test/eicar.exe","content":"X5O!P%@AP[4\\PZX54(P^)7CC)7}$EICAR-STANDARD-ANTIVIRUS-TEST-FILE!$H+H*"}</FILE_OP>
ENDMSG

./ws-send.exe --url ws://127.0.0.1:49000/ws --token TOKEN --file eicar_msg.txt
```

**验证**:
```bash
ls C:/Zoo/Temp/test/eicar.exe 2>/dev/null || echo "PASS: file not created"

grep -i "eicar\|virus" nemesisbot.log
# 预期: 包含 Eicar-Test-Signature
```

### Step 15: 验证拦截反馈

```bash
grep "instream" nemesisbot.log
# 预期: instream(...): Eicar-Test-Signature FOUND
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

# 先切出测试目录再删除（工作目录在 test-tools/autotest 内时 Windows 会锁住目录）
cd ../..
pwd
rm -rf test-tools/autotest
rm -rf C:/Zoo/Temp/test
```

**验证**: `ls test-tools/autotest 2>/dev/null && echo "FAIL: 目录残留" || echo "PASS: 已清理"`

### Step 17: 分析日志

| 日志项 | 期望内容 |
|--------|---------|
| Bot 启动日志 | ClamAV daemon 进程启动，端口 3310 LISTENING |
| 扫描放行日志 | clean file 无 virus detected |
| 扫描拦截日志 | `instream: Eicar-Test-Signature FOUND` |
| 错误日志 | 无 unexpected error |

### Step 18: 输出测试报告

测试完成后将报告保存到 `docs/REPORT/`，文件名格式 `YYYY-MM-DD_Scanner_E2E_Test_Report.md`。
