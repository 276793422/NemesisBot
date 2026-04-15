# 开发计划目录

此文件夹放置 NemesisBot 的**当前待办任务**文档。

---

## 当前任务列表

**更新日期**: 2026-04-15

| 任务 | 类型 | 优先级 | 状态 |
|------|------|--------|------|
| [跨平台测试](2026-03-24_CROSS_PLATFORM_TEST.md) | 测试 | P2 | 需要平台环境 |
| [性能优化](2026-03-24_PERFORMANCE_OPTIMIZATION.md) | 优化 | P3 | 待开始 |
| [文档完善](2026-03-24_DOCUMENTATION.md) | 文档 | P3 | 待开始 |

---

## 已归档的计划

| 计划 | 归档日期 | 归档位置 |
|------|---------|---------|
| 集群 RPC 全异步改造（Phase 2 续行快照） | 2026-04-15 | `docs/REPORT/2026-04-13_ASYNC_RPC_PHASE2_CONTINUATION_SNAPSHOT.md` |
| ClamAV 杀毒扫描集成 | 2026-04-15 | `docs/REPORT/2026-04-15_CLAMAV_INTEGRATION.md` |
| ClamAV SecurityPlugin 集成 | 2026-04-15 | `docs/REPORT/2026-04-15_CLAMAV_SECURITY_PLUGIN_INTEGRATION.md` |
| Scanner 杀毒引擎集成 | 2026-04-15 | `docs/REPORT/2026-04-15_SCANNER_ENGINE_INTEGRATION.md` |
| Scanner E2E 测试流程 | 2026-04-15 | `docs/REPORT/2026-04-15_Scanner_E2E_Test_Flow.md` |
| 模型间通信方式改进（部分完成） | 2026-04-15 | `docs/REPORT/2026-04-12_INTER_MODEL_COMMUNICATION_IMPROVEMENT.md` |

---

## 任务详情

### 1. 跨平台测试（P2）

**文档**: [2026-03-24_CROSS_PLATFORM_TEST.md](2026-03-24_CROSS_PLATFORM_TEST.md)

**目标**: Linux/macOS 兼容性验证

**依赖**: 需要访问 Linux/macOS 环境

---

### 2. 性能优化（P3）

**文档**: [2026-03-24_PERFORMANCE_OPTIMIZATION.md](2026-03-24_PERFORMANCE_OPTIMIZATION.md)

**目标**:
- 启动时间 < 0.5s
- 内存占用 < 50MB
- UI 响应速度提升

---

### 3. 文档完善（P3）

**文档**: [2026-03-24_DOCUMENTATION.md](2026-03-24_DOCUMENTATION.md)

**内容**: README、用户指南、开发者文档、API 文档

---

## 建议执行顺序

```
阶段 1（按需）：
  ├─ 跨平台测试（需要 Linux/macOS 环境）
  ├─ 性能优化
  └─ 文档完善
```

---

## 如何创建新任务

1. 在 `docs/PLAN/` 目录创建文件，命名规范：`YYYY-MM-DD_任务名称.md`
2. 文档包含：任务概述、类型、优先级、实施步骤、验收标准
3. 更新本 README

---

**最后更新**: 2026-04-15
