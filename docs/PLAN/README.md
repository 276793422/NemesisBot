# 开发计划目录

此文件夹放置 NemesisBot 的**当前待办任务**文档。

---

## 当前任务列表

**更新日期**: 2026-04-22

| 任务 | 类型 | 优先级 | 状态 |
|------|------|--------|------|
| [集群消息角色切换](2026-04-22_CLUSTER_IDENTITY_SWITCHING_PLAN.md) | 功能 | P2 | 待实施 |
| [跨平台测试](2026-03-24_CROSS_PLATFORM_TEST.md) | 测试 | P2 | 需要平台环境 |
| [性能优化](2026-03-24_PERFORMANCE_OPTIMIZATION.md) | 优化 | P3 | 待开始 |
| [文档完善](2026-03-24_DOCUMENTATION.md) | 文档 | P3 | 待开始 |

---

## 任务详情

### 1. 集群消息角色切换（P2，待实施）

**文档**: [2026-04-22_CLUSTER_IDENTITY_SWITCHING_PLAN.md](2026-04-22_CLUSTER_IDENTITY_SWITCHING_PLAN.md)

**目标**: 集群 P2P 消息处理时使用差异化身份

**状态**: 计划已重新规划，方案已简化

---

### 2. 跨平台测试（P2）

**文档**: [2026-03-24_CROSS_PLATFORM_TEST.md](2026-03-24_CROSS_PLATFORM_TEST.md)

**目标**: Linux/macOS 兼容性验证

**依赖**: 需要访问 Linux/macOS 环境

---

### 3. 性能优化（P3）

**文档**: [2026-03-24_PERFORMANCE_OPTIMIZATION.md](2026-03-24_PERFORMANCE_OPTIMIZATION.md)

**目标**:
- 启动时间 < 0.5s
- 内存占用 < 50MB
- UI 响应速度提升

---

### 4. 文档完善（P3）

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

**最后更新**: 2026-04-22
