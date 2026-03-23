# 阶段 1: 预检查

在开始测试之前，需要进行一系列预检查，确保测试可以安全执行。

---

## 检查项

### 1.1 UI 依赖检查

**目的**: 确认测试功能不需要 UI 交互

**检查方法**:
```bash
# 检查功能描述中是否包含以下关键词
keywords=("对话框" "dialog" "window" "窗口" "approval" "审批")

# 如果包含任一关键词，则不适用此测试流程
```

**决策树**:
```
功能需要 UI?
├─ 是 → 使用其他测试方法（手动测试、UI 自动化测试）
└─ 否 → 继续下一步检查
```

**通过条件**: 无 UI 依赖

---

### 1.2 AI 支持检查

**目的**: 确认功能需要 LLM 调用

**检查方法**:
```go
// 检查工具定义中是否有以下特征：
// 1. 工具会发送到 LLM
// 2. 需要调用 model.Complete()
// 3. 属于 Agent 执行流程

// 示例：文件操作工具
type FileOperationTool struct {
    // 如果实现了 Tool 接口，需要 AI 支持
}
```

**决策树**:
```
功能需要 AI?
├─ 否 → 不需要此测试流程（直接单元测试）
└─ 是 → 继续下一步检查
```

**通过条件**: 需要 AI 处理

---

### 1.3 TestAIServer 能力检查

**目的**: 确认 TestAIServer 有支持的测试模型

**检查方法**:
```bash
# 查看 TestAIServer README
cat test/TestAIServer/README.md

# 或者运行帮助命令
cd test/TestAIServer
./testaiserver.exe --help
```

**可用模型列表**:

| 模型 | 名称 | 用途 | 特殊标记 |
|------|------|------|----------|
| testai-1.1 | 快速响应模型 | 基础功能测试 | 无 |
| testai-1.2 | 中等延迟模型 | 超时测试(30s) | 无 |
| testai-1.3 | 长延迟模型 | 超时测试(300s) | 无 |
| testai-2.0 | 回显模型 | 消息传递验证 | 无 |
| testai-3.0 | 集群通信模型 | peer_chat 测试 | `<PEER_CHAT>{}` |
| testai-4.2 | 客户端休眠模型 | 工具调用(30s) | sleep 工具 |
| testai-4.3 | 客户端休眠模型 | 工具调用(300s) | sleep 工具 |
| testai-5.0 | 安全文件操作模型 | 文件操作测试 | `<FILE_OP>{}` |

**模型选择指南**:

```yaml
文件操作测试:
  模型: testai-5.0
  支持操作:
    - file_read
    - file_write
    - file_delete
    - file_append
    - dir_create
    - dir_delete
    - dir_list
  输入格式: <FILE_OP>{"operation":"file_read","path":"test.txt"}</FILE_OP>

集群通信测试:
  模型: testai-3.0
  支持操作: peer_chat
  输入格式: <PEER_CHAT>{"peer_id":"agent-b","content":"test"}</PEER_CHAT>

工具调用测试:
  模型: testai-4.2 或 testai-4.3
  支持操作: sleep 工具调用
  延迟: 30秒 或 300秒

基础功能测试:
  模型: testai-1.1 或 testai-2.0
  用途: 快速响应和消息验证
```

**决策树**:
```
TestAIServer 支持所需功能?
├─ 是 → 继续测试
└─ 否 →
    ├─ 扩展 TestAIServer 功能
    └─ 或使用其他测试方法
```

**通过条件**: 找到匹配的测试模型

---

## 预检查脚本示例

```bash
#!/bin/bash
# pre_check.sh - 预检查脚本

echo "=== 测试预检查 ==="

# 1. UI 依赖检查
echo "[1/3] 检查 UI 依赖..."
if grep -q "对话框\|dialog\|approval" <<<$TEST_DESCRIPTION; then
  echo "❌ 功能需要 UI，不适用此测试流程"
  exit 1
fi
echo "✅ 无 UI 依赖"

# 2. AI 支持检查
echo "[2/3] 检查 AI 支持需求..."
# 这里可以根据实际功能检查
echo "✅ 需要 AI 支持"

# 3. TestAIServer 能力检查
echo "[3/3] 检查 TestAIServer 能力..."
cd test/TestAIServer
if ./testaiserver.exe --help | grep -q "testai-5.0"; then
  echo "✅ TestAIServer 支持所需模型"
else
  echo "❌ TestAIServer 不支持所需模型"
  exit 1
fi

echo ""
echo "=== 预检查完成，可以开始测试 ==="
```

---

## 检查结果记录

```yaml
预检查记录:
  日期: YYYY-MM-DD HH:MM:SS
  功能: [功能名称]
  检查结果:
    UI 依赖: ✅ 通过 / ❌ 不通过
    AI 支持: ✅ 通过 / ❌ 不通过
    TestAIServer: ✅ 通过 / ❌ 不通过
    选择模型: testai-X.X
  决策: 继续 / 终止 / 调整方案
  备注: [其他说明]
```

---

**下一步**: 阶段 2 - 环境准备
