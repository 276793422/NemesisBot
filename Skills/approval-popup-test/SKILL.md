# 安全审批弹窗 UAT 测试 Skill

完整的端到端审批弹窗测试流程，通过 TestAIServer 触发安全审批弹窗，由用户手动点击批准/拒绝，验证全链路审批流程。

---

## 概述

此 Skill 用于测试 NemesisBot Rust 项目的**安全审批弹窗功能**。测试覆盖完整链路：

```
用户消息 → WebSocket → AgentLoop → LLM 工具调用 → 安全中间件拦截
→ ApprovalManager → ProcessManager 子进程 → plugin-ui DLL 弹窗
→ 用户点击 → WS 通知返回 → 安全中间件放行/拒绝 → 响应用户
```

### 适用场景

- ✅ 审批弹窗功能测试（Dashboard 和 Approval 窗口）
- ✅ 安全中间件 "ask" 规则验证
- ✅ ProcessManager → 子进程 → DLL 全链路验证
- ✅ 窗口去重逻辑验证（多次打开不弹新窗口）
- ❌ 不适用于纯后端功能测试（无 UI 交互）

### 项目路径

| 项目 | 路径 |
|------|------|
| Rust 项目 | `NemesisBot_Rust/` |
| Go 项目（TestAIServer） | `NemesisBot/test/TestAIServer/` |
| Skill 路径 | `Skills/approval-popup-test/` |

---

## 测试工作目录

**测试目录**: `{Rust项目}/test/uat-approval/`

```
NemesisBot_Rust/
├── test/
│   └── uat-approval/             # 测试工作目录（新建）
│       ├── nemesisbot.exe        # 编译产物（从 target/release/ 复制）
│       ├── plugins/
│       │   └── plugin_ui.dll     # 编译产物（从 plugins/plugin-ui/target/release/ 复制）
│       └── .nemesisbot/          # 运行时生成（onboard default --local）
│           ├── config.json
│           ├── workspace/
│           │   └── config/
│           │       └── config.security.json  # 需要修改（添加 ask 规则）
│           └── ...
├── target/release/nemesisbot.exe
└── plugins/plugin-ui/target/release/plugin_ui.dll
```

**清理方式**: 删除整个 `test/uat-approval/` 目录

---

## 流程概述

```
阶段 1: 编译 (nemesisbot + plugin-ui DLL)
阶段 2: 环境准备 (创建测试目录, 复制文件, 启动 TestAIServer)
阶段 3: 初始化 Bot (onboard default --local)
阶段 4: 配置模型 + 安全规则
阶段 5: 启动 Gateway
阶段 6: 触发审批弹窗 (WebSocket 发送消息)
阶段 7: 用户交互 (手动点击 Approve/Reject)
阶段 8: 验证结果 + 清理
```

---

## 详细流程

### 阶段 1: 编译

**目标**: 编译 nemesisbot 和 plugin-ui DLL

**命令**:
```bash
# 在 NemesisBot_Rust/ 根目录下执行
cargo build --release -p nemesisbot

# 编译 plugin-ui DLL（可并行）
cd plugins/plugin-ui && cargo build --release
```

**验证**:
```bash
ls target/release/nemesisbot.exe
ls plugins/plugin-ui/target/release/plugin_ui.dll
```

**产物**:
- `target/release/nemesisbot.exe`
- `plugins/plugin-ui/target/release/plugin_ui.dll`

---

### 阶段 2: 环境准备

**目标**: 创建测试目录、复制可执行文件、启动 TestAIServer

**步骤**:

1. **创建测试目录并复制文件**:
```bash
# 在 NemesisBot_Rust/ 根目录下执行
mkdir -p test/uat-approval/plugins
cp target/release/nemesisbot.exe test/uat-approval/
cp plugins/plugin-ui/target/release/plugin_ui.dll test/uat-approval/plugins/
```

2. **启动 Go TestAIServer**:
```bash
# 在 NemesisBot/test/TestAIServer/ 目录下执行
./testaiserver.exe &   # 使用 run_in_background: true
```

3. **验证 TestAIServer**:
```bash
curl -s http://127.0.0.1:8080/v1/models
# 确认返回包含 testai-5.0
```

**注意**: TestAIServer 是 Go 项目，已有预编译的 `testaiserver.exe`。如果没有，需要先编译：
```bash
# 在 NemesisBot/test/TestAIServer/ 目录下执行
go build -o testaiserver.exe .
```

---

### 阶段 3: 初始化 Bot

**目标**: 在测试目录初始化本地 Bot 配置

```bash
# 在 NemesisBot_Rust/test/uat-approval/ 目录下执行
./nemesisbot.exe onboard default --local
```

**验证**: 确认输出 `Initialization complete!` 并显示端口和 Token 信息。

**产物**: `.nemesisbot/config.json` 及相关配置文件

---

### 阶段 4: 配置模型 + 安全规则

**目标**: 配置 testai-5.0 模型，修改安全规则添加 "ask" 触发点

**步骤 1: 添加测试模型**:
```bash
# 在 NemesisBot_Rust/test/uat-approval/ 目录下执行
./nemesisbot.exe model add --model test/testai-5.0 --base http://127.0.0.1:8080/v1 --key test-key --default --local
```

**步骤 2: 修改安全配置添加 "ask" 规则**:

需要编辑 `.nemesisbot/workspace/config/config.security.json`，在 `file_rules.write` 数组最前面插入 ask 规则：

```python
python3 -c "
import json
with open('.nemesisbot/workspace/config/config.security.json') as f:
    d = json.load(f)
# 在 file_rules.write 最前面添加 ask 规则
d['file_rules']['write'].insert(0, {
    'pattern': '/tmp/**',
    'action': 'ask'
})
# 也可以添加通配规则（所有文件写入都需要审批）
# d['file_rules']['write'].append({'pattern': '*', 'action': 'ask'})
with open('.nemesisbot/workspace/config/config.security.json', 'w') as f:
    json.dump(d, f, indent=4, ensure_ascii=False)
print('Security config updated')
"
```

**验证**: 检查规则已添加：
```bash
cat .nemesisbot/workspace/config/config.security.json | python3 -c "
import sys, json
d = json.load(sys.stdin)
for r in d.get('file_rules', {}).get('write', []):
    print(f'  {r[\"pattern\"]} -> {r[\"action\"]}')
"
```

---

### 阶段 5: 启动 Gateway

**目标**: 启动 NemesisBot Gateway，确认所有组件就绪

```bash
# 在 NemesisBot_Rust/test/uat-approval/ 目录下执行
RUST_LOG=info ./nemesisbot.exe --local gateway   # 使用 run_in_background: true
```

**等待启动完成后检查日志**，确认以下关键日志行：

| 日志 | 含义 |
|------|------|
| `Security default_action: allow` | 安全配置已加载 |
| `Security file_rules loaded` | 文件规则已加载 |
| `Security config loaded from ...config.security.json` | 安全配置文件路径正确 |
| `Approval manager wired (popup via ProcessManager)` | 审批管理器已接线 |
| `ProcessManager started (WS server on port XXXXX)` | 子进程管理器已启动 |
| `Web server is listening` | Web 服务已就绪 |

**如果缺少 `Approval manager wired`**: gateway.rs 的 ApprovalPopupAdapter 未正确接线。
**如果缺少 `Security file_rules loaded`**: config.security.json 未被加载。

---

### 阶段 6: 触发审批弹窗

**目标**: 通过 WebSocket 发送消息触发文件写入工具调用 → 安全审批

**WebSocket 协议** (Rust 版本三级协议):
```
ws://127.0.0.1:49000/ws?token={auth_token}
```

**发送消息格式**:
```json
{
  "type": "message",
  "module": "chat",
  "cmd": "send",
  "data": {
    "content": "<FILE_OP>{\"operation\":\"file_write\",\"path\":\"/tmp/test.txt\",\"risk_level\":\"HIGH\"}</FILE_OP>"
  }
}
```

**auth_token**: 在 `onboard default --local` 的输出中获取（例如 `276793422`）

**Python 客户端脚本**:
```python
import asyncio
import websockets
import json

async def trigger_approval():
    # auth_token 从 onboard 输出获取
    uri = 'ws://127.0.0.1:49000/ws?token=276793422'
    async with websockets.connect(uri) as ws:
        msg = {
            'type': 'message',
            'module': 'chat',
            'cmd': 'send',
            'data': {
                'content': '<FILE_OP>{"operation":"file_write","path":"/tmp/test.txt","risk_level":"HIGH"}</FILE_OP>'
            }
        }
        await ws.send(json.dumps(msg))
        print('SENT: file_write request', flush=True)
        print('>>> Approval popup should appear - please click Approve or Reject <<<', flush=True)

        for i in range(15):
            try:
                response = await asyncio.wait_for(ws.recv(), timeout=15)
                data = json.loads(response)
                content = data.get('data', {}).get('content', '')
                safe = content.encode('ascii', 'replace').decode('ascii')[:300]
                print(f'RECV: {safe}', flush=True)
            except asyncio.TimeoutError:
                print(f'Waiting... ({i+1})', flush=True)

asyncio.run(trigger_approval())
```

**关键**: 需要设置 `PYTHONIOENCODING=utf-8` 避免编码错误。

---

### 阶段 7: 用户交互

**目标**: 用户在弹出的审批窗口中点击 Approve 或 Reject

当阶段 6 执行后，屏幕上应该弹出一个标题为 **"Security Approval - NemesisBot"** 的窗口，显示：
- 操作类型（如 file_write）
- 目标路径（如 /tmp/test.txt）
- 风险等级（如 HIGH）
- Approve / Reject 按钮

**用户操作**: 点击 **Approve** 或 **Reject**。

---

### 阶段 8: 验证结果 + 清理

**目标**: 验证审批结果正确传递，清理测试环境

**步骤 1: 检查 Gateway 日志**

审批成功的日志链路：
```
Requesting approval popup: operation=file_write, target=/tmp/test.txt, risk=HIGH
ProcessManager: Spawning child child-0 (type: approval)
Handshake completed with child child-0
WS key generated for child child-0
WebSocketServer: Connection registered
approval.submit handler fired: action=approved/rejected, request_id=xxx
Approval result: action=approved/rejected for request_id=xxx
Security blocked tool write_file: operation denied by user  ← 如果 rejected
```

**如果 Approved**: 工具调用继续执行（但 write_file 工具不存在会返回错误，这是正常的）
**如果 Rejected**: 安全中间件阻止操作，返回 "operation denied by user" 消息

**步骤 2: 清理**
```bash
# 停止进程
taskkill //F //IM nemesisbot.exe
taskkill //F //IM testaiserver.exe

# 删除测试目录（在 NemesisBot_Rust/ 根目录下执行）
rm -rf test/uat-approval
```

---

## 验证检查清单

测试完成后，确认以下所有项：

### 编译阶段
- [ ] `nemesisbot.exe` 编译成功
- [ ] `plugin_ui.dll` 编译成功
- [ ] DLL 放置在 `plugins/` 子目录下

### 环境阶段
- [ ] TestAIServer 启动并响应 `/v1/models`
- [ ] `testai-5.0` 模型已注册
- [ ] 本地 Bot 初始化成功（`onboard default --local`）
- [ ] 模型添加成功（`model add`）
- [ ] 安全配置包含 "ask" 规则

### Gateway 阶段
- [ ] Gateway 启动成功
- [ ] `Security config loaded from config.security.json` 日志出现
- [ ] `Approval manager wired (popup via ProcessManager)` 日志出现
- [ ] ProcessManager 启动成功
- [ ] Web server 监听正常

### 审批阶段
- [ ] WebSocket 消息发送成功
- [ ] testai-5.0 返回 `write_file` 工具调用
- [ ] 安全中间件匹配 "ask" 规则
- [ ] `Requesting approval popup` 日志出现
- [ ] 子进程创建成功（`Spawning child`）
- [ ] 管道握手完成（`Handshake completed`）
- [ ] WS 连接建立（`Connection registered`）
- [ ] **审批弹窗在屏幕上弹出**
- [ ] 用户点击后 `approval.submit handler fired` 日志出现
- [ ] 审批结果传回（`Approval result: action=...`）
- [ ] 安全中间件根据结果放行或阻止操作
- [ ] 用户收到响应消息

---

## TestAIServer testai-5.0 模型说明

testai-5.0 是专门用于安全测试的模型，它检测消息中的 `<FILE_OP>` 标签并返回对应的工具调用。

**支持的文件操作**:
- `file_write` → 返回 `write_file` 工具调用
- `file_delete` → 返回 `delete_file` 工具调用
- `file_read` → 返回 `read_file` 工具调用

**消息格式**:
```
<FILE_OP>{"operation":"file_write","path":"/tmp/test.txt","risk_level":"HIGH"}</FILE_OP>
```

**注意**: 路径中不要使用 Windows 反斜杠（`\\`），会导致 JSON 解析错误。使用 `/` 代替。

---

## 常见问题排查

### 问题 1: 审批弹窗没有弹出

**症状**: 消息发出后直接收到拒绝/允许响应，没有弹窗

**排查**:
1. 检查 Gateway 日志是否有 `Approval manager wired`
2. 检查安全配置是否有 "ask" 规则（`cat .nemesisbot/workspace/config/config.security.json`）
3. 检查 `plugin_ui.dll` 是否在 `plugins/` 目录下
4. 检查 Gateway 日志是否显示 `Security config loaded from config.security.json`

### 问题 2: 安全规则未加载

**症状**: 日志显示 `no rules configured, using default action`

**原因**: `config.security.json` 的规则没有被 `SecurityPlugin` 加载

**解决**: 确认 `gateway.rs` 的 `load_security_rules()` 被调用，检查安全配置文件路径是否正确

### 问题 3: 弹窗出现但子进程不退出

**症状**: 用户点击后弹窗关闭，但子进程还在运行

**排查**:
1. 确认 `plugin-ui DLL` 使用的是 `run_return()` 而非 `run()`（`run()` 返回 never type，会调用 `std::process::exit(0)`）
2. 确认事件循环使用 `WaitUntil(100ms)` 而非 `Wait`
3. 检查 `WsHandle` 的 `AtomicBool` shutdown 标志是否正常工作

### 问题 4: 弹窗点击后 ^C 出现在测试输出

**原因**: `ProcessManager.stop()` 调用 `GenerateConsoleCtrlEvent(CTRL_C_EVENT)` 广播到整个 console group

**解决**: 子进程使用 `CREATE_NEW_PROCESS_GROUP` 标志创建（已在 executor.rs 中实现）

### 问题 5: WebSocket 连接被拒绝 (401)

**原因**: 需要带 auth_token 查询参数

**解决**: WebSocket URL 格式为 `ws://127.0.0.1:49000/ws?token={auth_token}`

### 问题 6: Python UnicodeEncodeError

**原因**: Windows 默认终端编码不是 UTF-8

**解决**: 使用 `PYTHONIOENCODING=utf-8` 或在 print 时使用 `.encode('ascii', 'replace')`

---

## 窗口去重测试（可选扩展）

审批弹窗基础测试通过后，可以额外测试窗口去重功能：

### Dashboard 去重测试

1. 点击系统托盘图标 → "打开 Dashboard"
2. 确认 Dashboard 窗口弹出
3. 再次点击系统托盘图标 → "打开 Dashboard"
4. **预期**: 不弹出新窗口，已有窗口被前置到前台

**验证日志**:
```
Plugin window 'dashboard' already running (child_id: xxx), sending bring_to_front
Sent bring_to_front notification to child xxx
```

---

## 安全规则参考

| 操作类型 | 风险级别 | 可用 action |
|---------|---------|------------|
| file_write | HIGH | allow / deny / ask |
| file_delete | HIGH | allow / deny / ask |
| file_read | LOW | allow / deny / ask |
| process_exec | CRITICAL | allow / deny / ask |
| dir_create | LOW | allow / deny / ask |

**default_action**: 全局默认策略（allow / deny / ask）

---

**最后更新**: 2026-05-13
