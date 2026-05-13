# 开发计划目录

此文件夹放置 NemesisBot 的**当前待办任务**文档。

---

## 当前任务列表

**更新日期**: 2026-05-01

| 任务 | 类型 | 优先级 | 状态 |
|------|------|--------|------|
| ~~Go→Rust 1:1 对比与补全~~ | 架构 | P0 | **已完成，文档已迁移至 Rust 项目** |
| ~~Go→Rust 全面迁移计划~~ | 架构 | P0 | **已完成，文档已迁移至 Rust 项目** |
| [剩余改进任务总规划](2026-04-26_COMPETITIVE_REMAINING_TASKS.md) | 综合 | P1-P3 | 规划中 |
| [Wails DLL 插件系统](2026-04-26_WAILS_DLL_PLUGIN.md) | 功能 | P2 | 待实施（若 Rust 迁移推进则取消） |
| [集群消息角色切换](2026-04-22_CLUSTER_IDENTITY_SWITCHING_PLAN.md) | 功能 | P2 | 待实施 |
| [跨平台测试](2026-03-24_CROSS_PLATFORM_TEST.md) | 测试 | P2 | 需要平台环境 |
| [性能优化](2026-03-24_PERFORMANCE_OPTIMIZATION.md) | 优化 | P3 | 待开始 |
| [文档完善](2026-03-24_DOCUMENTATION.md) | 文档 | P3 | 待开始 |

---

## 任务详情

### 0a. Go→Rust 1:1 对比与补全（P0，已完成）

**状态**: ✅ 5 批次全部完成，1,514+ 测试通过，workspace 编译通过

> 所有相关文档已迁移至 Rust 项目 `NemesisBot_Rust/docs/`

**补全内容**:
- 批次 1 (核心): agent loop 增强、plugin 工具系统、cluster 5 个新 Action、services 心跳
- 批次 2 (基础设施): OutboundMessage.type 字段 + 37 个构造点修复
- 批次 3 (业务): WebSocket 消息总线修复、CORSManager、workflow 真实节点执行器、forge 生命周期、图存储持久化
- 批次 4 (支撑): skills lint 增强、desktop 协议辅助、优雅进程终止、WS 客户端分发
- 批次 5 (CLI): version/test 命令、status 版本信息

---

### 0. Go→Rust 全面迁移计划（P0，已完成）

**状态**: ✅ 四轮分析+预案全部完成。Rust 项目已创建并完成 1:1 对比补全。

> 所有相关文档已迁移至 Rust 项目 `NemesisBot_Rust/docs/`

---

### 1. 剩余改进任务（P1-P3，规划中）

**文档**: [2026-04-26_COMPETITIVE_REMAINING_TASKS.md](2026-04-26_COMPETITIVE_REMAINING_TASKS.md)

**目标**: 汇总所有剩余改进任务（1 P1 + 5 P2 + 13 P3 = 19 项）

**状态**: P0 已全部关闭，剩余任务已规划归档

---

### 2. 集群消息角色切换（P2，待实施）

**文档**: [2026-04-22_CLUSTER_IDENTITY_SWITCHING_PLAN.md](2026-04-22_CLUSTER_IDENTITY_SWITCHING_PLAN.md)

**目标**: 集群 P2P 消息处理时使用差异化身份

---

### 3. 跨平台测试（P2）

**文档**: [2026-03-24_CROSS_PLATFORM_TEST.md](2026-03-24_CROSS_PLATFORM_TEST.md)

**目标**: Linux/macOS 兼容性验证

**依赖**: 需要访问 Linux/macOS 环境

---

### 4. 性能优化（P3）

**文档**: [2026-03-24_PERFORMANCE_OPTIMIZATION.md](2026-03-24_PERFORMANCE_OPTIMIZATION.md)

**目标**: 启动时间 < 0.5s，内存占用 < 50MB，UI 响应速度提升

---

### 5. 文档完善（P3）

**文档**: [2026-03-24_DOCUMENTATION.md](2026-03-24_DOCUMENTATION.md)

**目标**: README、用户指南、开发者文档、API 文档

---

## 建议执行顺序

```
已完成（P0）:
  └─ Go→Rust 1:1 对比与补全 — ✅ 5 批次全部完成
     └─ 所有文档已迁移至 NemesisBot_Rust/docs/

下一步:
  ├─ ✅ 功能验证: Rust Gateway 全面测试已完成 (3,384 单元测试 + 14 集成测试)
  │   详见: NemesisBot_Rust/docs/REPORT/2026-05-08_RUST_FULL_TEST_REPORT.md
  ├─ GUI 框架选型: Tauri / iced 替代 Wails
  ├─ 跨平台测试（需要 Linux/macOS 环境）
  └─ 性能优化

按需排期:
  ├─ 集群消息角色切换
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

**最后更新**: 2026-05-08
