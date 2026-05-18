# 记忆系统 UAT 测试 Skill

记忆系统的用户验收测试，验证增强记忆（enhanced memory）在真实场景下的完整工作流。

---

## 概述

此 Skill 提供记忆系统的用户验收测试（UAT），覆盖从基本记忆到 ONNX 插件增强记忆的完整生命周期。测试通过 TestAIServer 的 `testai-6.0` 记忆模型驱动，验证 memory tools 的 store/search/list/forget 功能。

### 测试范围

- ✅ 基本记忆模式启动和工具可用性
- ✅ ONNX 插件增强记忆自动检测和初始化
- ✅ 语义搜索质量验证（跨语义间隙）
- ✅ 降级场景（移除 plugin 后回退到基本记忆）
- ✅ 持久化场景（重启后数据存活）
- ❌ 不测试 API 嵌入 tier（需要真实 API key）
- ❌ 不测试集群记忆（需要多节点部署）

### 测试工作目录

```
test-tools/
├── TestAIServer/testaiserver.exe   # AI 服务器
├── memory-uat-workdir/             # UAT 工作目录（临时）
│   ├── nemesisbot.exe              # 从 target/release/ 复制
│   ├── testaiserver.exe            # 从 TestAIServer/ 复制
│   ├── plugins/plugin_onnx.dll     # 可选，ONNX 插件
│   └── .nemesisbot/                # 运行时配置
```

---

## 前置条件

### 必须存在

| 组件 | 位置 | 构建命令 |
|------|------|----------|
| nemesisbot.exe | `target/release/` | `cargo build --release -p nemesisbot` |
| testaiserver.exe | `test-tools/TestAIServer/` | `cd test-tools/TestAIServer && go build` |
| plugin_onnx.dll | `target/release/plugins/` | `cd plugins/plugin-onnx && cargo build --release` |
| ONNX 模型文件 | `test-tools/plugin-onnx-test/test-data/` | `bash test-tools/plugin-onnx-test/scripts/setup-test.sh` |

### 端口分配

| 端口 | 用途 |
|------|------|
| 8080 | TestAIServer |
| 49000 | Gateway Web/WS |
| 18790 | Health check |

---

## 测试流程（7 阶段）

### Stage 1: 预检

验证所有必需的二进制文件和资源文件存在。

**检查项**:
1. `nemesisbot.exe` 存在且可执行
2. `testaiserver.exe` 存在且可执行
3. `plugin_onnx.dll` 存在（如果不存在，跳过 plugin 相关场景）
4. 端口 8080、49000、18790 未被占用

**操作**:
```bash
# 检查二进制
ls -la target/release/nemesisbot.exe
ls -la test-tools/TestAIServer/testaiserver.exe
ls -la target/release/plugins/plugin_onnx.dll || echo "WARN: plugin DLL not found"

# 检查端口
netstat -ano | findstr "LISTENING" | findstr ":8080" && echo "Port 8080 in use" || echo "Port 8080 free"
netstat -ano | findstr "LISTENING" | findstr ":49000" && echo "Port 49000 in use" || echo "Port 49000 free"
```

### Stage 2: 环境准备

创建隔离的测试工作目录，准备所需文件。

**操作**:
```bash
bash Skills/memory-uat/scripts/setup-env.sh
```

脚本执行：
1. 创建 `test-tools/memory-uat-workdir/` 目录
2. 复制 `nemesisbot.exe` 和 `testaiserver.exe`
3. 如果存在，复制 `plugin_onnx.dll` 到 `plugins/` 子目录
4. 启动 TestAIServer（端口 8080）

### Stage 3: 基本记忆验证

**场景**: `uat_basic_memory`

验证基本记忆模式（无增强）下 memory tools 可用。

1. 运行 `nemesisbot.exe --local onboard default`
2. 添加 testai-6.0 模型
3. 启动 gateway
4. 通过 WebSocket 发送 "列出记忆"
5. 验证收到正常响应（包含 "Memory Store Status"）
6. 发送 "记住：测试事实"
7. 验证 store 成功
8. 发送 "关于 测试 你知道什么"
9. 验证搜索返回结果
10. 停止 gateway

**预期结果**: 所有 memory tools 正常工作，无错误。

### Stage 4: ONNX 增强记忆验证

**场景**: `uat_plugin_auto_detection`

验证 ONNX plugin 自动检测和向量存储初始化。

1. 确保 `plugins/plugin_onnx.dll` 存在
2. 配置 `config.enhanced_memory.json`（enabled=true, tier="auto"）
3. 启动 gateway
4. 检查日志确认 "Vector store initialized"
5. 通过 WebSocket 执行 store/search
6. 验证功能正常
7. 停止 gateway

**预期结果**: 向量存储初始化成功，语义搜索正常。

**如果 plugin DLL 不存在**: 跳过此阶段，记录 SKIP。

### Stage 5: 语义搜索质量验证

**场景**: `uat_semantic_search_quality`

验证语义搜索能跨越关键词间隙匹配。

1. 配置增强记忆（plugin tier）
2. 启动 gateway
3. 存储事实："猫是哺乳动物，喜欢追逐激光笔"
4. 存储事实："Python 是一种解释型编程语言"
5. 搜索："猫科动物宠物"
6. 验证返回关于"猫"的事实（非"Python"）
7. 停止 gateway

**预期结果**: 语义搜索能匹配到语义相关内容，即使搜索词不含原文关键词。

**如果 plugin DLL 不存在**: 跳过此阶段，使用本地 hash tier 进行近似验证。

### Stage 6: 降级验证

**场景**: `uat_degradation_remove_plugin`

验证移除 plugin 后系统降级到基本记忆，不崩溃。

1. 配置增强记忆（plugin tier）并运行
2. 停止 gateway
3. 删除 `plugin_onnx.dll`
4. 重新启动 gateway
5. 验证 bot 正常启动（降级到基本记忆）
6. 执行基本 memory store/search
7. 验证功能正常

**预期结果**: 系统优雅降级，基本记忆功能可用。

### Stage 7: 报告

生成 UAT 测试报告。

**操作**:
1. 收集所有场景结果
2. 生成报告到 `docs/REPORT/YYYY-MM-DD_memory-uat.md`
3. 清理环境

```bash
bash Skills/memory-uat/scripts/cleanup-env.sh
```

---

## 执行方式

### 手动执行

按照 Stage 1-7 的步骤逐步执行。

### 半自动执行

```bash
# 准备环境
bash Skills/memory-uat/scripts/setup-env.sh

# 执行测试（手动/半自动）
# ... 按 Stage 3-6 的步骤操作 ...

# 清理
bash Skills/memory-uat/scripts/cleanup-env.sh
```

---

## 故障排除

### TestAIServer 启动失败

检查 8080 端口是否被占用：
```bash
netstat -ano | findstr ":8080"
taskkill /F /PID <pid>
```

### Plugin DLL 加载失败

确保 DLL 架构匹配（x86_64）：
```bash
file target/release/plugins/plugin_onnx.dll
```

### 向量存储初始化失败

检查 embedding.toml 和模型文件：
```bash
ls -la test-tools/plugin-onnx-test/test-data/model.onnx
ls -la test-tools/plugin-onnx-test/test-data/tokenizer.json
```

### Bot 无法启动

查看日志：
```bash
cat test-tools/memory-uat-workdir/.nemesisbot/workspace/logs/*.log
```
