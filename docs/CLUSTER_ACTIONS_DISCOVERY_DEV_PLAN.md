# 集群 Actions 发现功能 - 开发计划

**功能名称**: 集群 Actions 列表与 Schema 定义
**创建日期**: 2026-03-05
**开发人员**: Claude Sonnet
**版本**: 1.0

---

## 📋 功能概述

为 NemesisBot 集群添加 actions 发现功能，使外部节点能够：
1. 动态查询节点的可用 actions
2. 获取每个 action 的详细说明
3. 了解 action 的参数和返回值格式
4. 通过 cluster skill 学习如何使用集群功能

---

## 🎯 开发目标

### 主要目标

1. ✅ 实现 `list_actions` RPC handler
2. ✅ 定义所有 actions 的 schema
3. ✅ 创建 cluster skill 文档
4. ✅ 编写完整测试
5. ✅ 更新相关文档

### 非目标

- ❌ 不修改现有 handler 的功能
- ❌ 不改变 RPC 通信协议
- ❌ 不实现设备接入功能（留待后续）

---

## 📝 开发步骤

### 步骤 1: 创建 actions schema 定义文件

**文件**: `module/cluster/actions_schema.go`

**操作**:
1. 创建新文件定义 actions schema 结构
2. 为每个 action 定义完整的 schema
3. 包含：名称、描述、参数、返回值

**验收标准**:
- [x] 文件创建成功
- [x] 所有 actions 都有完整定义
- [x] 格式符合 JSON Schema 规范
- [x] 编译无错误

---

### 步骤 2: 实现 `list_actions` handler

**文件**: `module/cluster/handlers/default.go`

**操作**:
1. 在 `RegisterDefaultHandlers` 函数中添加 `list_actions` handler
2. 调用 `actions_schema.go` 中的 schema 定义
3. 返回完整的 actions 列表

**验收标准**:
- [x] `list_actions` handler 注册成功
- [x] 可以被 RPC 调用
- [x] 返回格式正确
- [x] 包含所有 actions 的完整信息

---

### 步骤 3: 创建 cluster skill 文档

**文件**: `workspace/skills/cluster/SKILL.md`

**操作**:
1. 创建 cluster skill 目录
2. 编写 skill 文档，包含：
   - 集群功能概述
   - 可用的 actions 说明
   - 使用示例
   - 注意事项
3. 遵循 skill 文档格式规范

**验收标准**:
- [x] skill 文件创建成功
- [x] 内容完整且清晰
- [x] 包含使用示例
- [x] 符合 skill 格式要求

---

### 步骤 4: 编写单元测试

**文件**: `test/unit/cluster/handlers/default_test.go`

**操作**:
1. 测试 `list_actions` handler
2. 验证返回的数据格式
3. 测试边界情况

**验收标准**:
- [x] 测试文件创建成功
- [x] 所有测试用例通过
- [x] 测试覆盖率 > 80%

---

### 步骤 5: 编写集成测试

**文件**: `test/integration/cluster/list_actions_test.go`

**操作**:
1. 测试完整的 RPC 调用流程
2. 验证集群间通信
3. 测试实际场景

**验收标准**:
- [x] 集成测试创建成功
- [x] 所有集成测试通过
- [x] 覆盖主要使用场景

---

### 步骤 6: 编译验证

**操作**:
1. 运行项目编译命令
2. 检查所有模块编译状态
3. 验证无编译错误

**验收标准**:
- [x] 项目整体编译成功
- [x] 无编译错误
- [x] 无编译警告
- [x] 所有模块正常

---

### 步骤 7: 更新文档

**文件**: `docs/CLUSTER_ACTIONS_DISCOVERY.md`

**操作**:
1. 创建功能说明文档
2. 更新 RPC API 文档
3. 记录使用示例

**验收标准**:
- [x] 文档创建成功
- [x] 内容准确完整
- [x] 包含使用示例

---

## 🧪 测试计划

### 单元测试

```go
// test/unit/cluster/handlers/default_test.go
func TestListActionsHandler(t *testing.T)
func TestListActionsResponseFormat(t *testing.T)
func TestListActionsCompleteness(t *testing.T)
```

### 集成测试

```go
// test/integration/cluster/list_actions_test.go
func TestListActionsRPCFlow(t *testing.T)
func TestListActionsAcrossNodes(t *testing.T)
```

### 测试执行命令

```bash
# 单元测试
go test ./test/unit/cluster/handlers/

# 集成测试
go test ./test/integration/cluster/

# 回归测试
go test ./...
```

---

## ✅ 验收标准

### 功能验收

- [ ] `list_actions` handler 正常工作
- [ ] 所有 actions 都有完整 schema 定义
- [ ] cluster skill 可以被 LLM 理解和使用
- [ ] 外部节点可以查询 action 列表

### 质量验收

- [ ] 所有单元测试通过
- [ ] 所有集成测试通过
- [ ] 测试覆盖率达到要求
- [ ] 项目整体编译成功
- [ ] 无编译错误和警告

### 文档验收

- [ ] 开发整理文档完成
- [ ] 功能说明文档完成
- [ ] 使用示例文档完整
- [ ] 代码注释充分

---

## 📊 风险评估

### 技术风险

| 风险 | 影响 | 应对策略 |
|------|------|---------|
| RPC 序列化问题 | 中 | 使用现有序列化机制 |
| Schema 格式变化 | 低 | 使用标准 JSON Schema |
| 性能影响 | 低 | action 列表很小 |

### 兼容性风险

| 风险 | 影响 | 应对策略 |
|------|------|---------|
| 现有 handler 冲突 | 无 | 新 handler，无冲突 |
| API 变更影响 | 无 | 新增接口，不修改现有 |

---

## 📅 时间估算

| 步骤 | 预计时间 |
|------|---------|
| 步骤 1: 创建 schema 定义 | 30 分钟 |
| 步骤 2: 实现 list_actions handler | 20 分钟 |
| 步骤 3: 创建 cluster skill | 40 分钟 |
| 步骤 4: 单元测试 | 30 分钟 |
| 步骤 5: 集成测试 | 30 分钟 |
| 步骤 6: 编译验证 | 10 分钟 |
| 步骤 7: 更新文档 | 20 分钟 |
| **总计** | **约 3 小时** |

---

## 🔗 相关资源

- **现有代码**: `module/cluster/handlers/`
- **文档目录**: `docs/`
- **测试目录**: `test/unit/cluster/`, `test/integration/cluster/`
- **Skill 目录**: `workspace/skills/cluster/`

---

## 📝 开发日志

**开始时间**: 2026-03-05

**开发记录**:
- (待填写)

---

## ✅ 完成状态

- [x] 步骤 1: 创建 actions schema 定义
- [x] 步骤 2: 实现 list_actions handler
- [x] 步骤 3: 创建 cluster skill
- [x] 步骤 4: 编写单元测试
- [x] 步骤 5: 编写集成测试
- [x] 步骤 6: 编译验证
- [x] 步骤 7: 更新文档

**当前状态**: ✅ 全部完成！

---

## 🎉 项目总结

**实际开发时间**: 约 3 小时
**完成情况**: 所有 7 个步骤全部完成
**测试结果**: 所有单元测试和集成测试通过
**编译状态**: 项目编译成功，无错误

**关键成果**:
1. ✅ 实现了完整的 `list_actions` RPC handler
2. ✅ 定义了所有默认 actions 的 schema
3. ✅ 创建了 cluster skill 供 LLM 使用
4. ✅ 编写了完整的单元测试和集成测试
5. ✅ 通过了编译验证
6. ✅ 编写了完整的技术文档

---

**开发计划执行完成！** 🎉
