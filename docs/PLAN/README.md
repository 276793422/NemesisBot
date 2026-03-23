# 开发计划目录

此文件夹放置 NemesisBot 的**活跃开发计划**文档。

---

## 📋 当前活跃计划

### 🔴 正在进行

#### 1. 安全审批对话框功能（2026-03-20 至 2026-04-09）

**状态**: 🔄 开发中（Week 1）
**预计工期**: 17 天（2.5 周）
**当前进度**: 约 20%

**主要文档**:
- 📋 [开发路线图](./SECURITY_APPROVAL_DEVELOPMENT_ROADMAP.md) - 总体时间线和里程碑
- 📋 [单进程方案](./SECURITY_APPROVAL_SINGLE_PROCESS_PLAN.md) - 技术实施细节

**技术方案**:
- 架构：单进程模式（webview 库）
- 模式：适配器模式（平台无关接口 + 平台特定实现）
- Windows: `github.com/shmspace/webview2`
- Unix: `github.com/webview/webview`

**关键里程碑**:
- Week 1 Day 7 (2026-03-27): Windows 平台 Alpha 版本
- Week 2 Day 3 (2026-03-30): Unix 平台补充完成
- Week 3 Day 1 (2026-04-05): Beta 版本发布
- Week 3 Day 5 (2026-04-09): 正式版本发布

**实施进展**:
- ✅ 架构设计和接口定义
- ✅ Windows/Unix 适配器实现
- ✅ 核心逻辑代码
- ✅ 测试用例编写
- 🔄 UI 模块开发
- ⏳ WebView 集成
- ⏳ 系统集成

---

### 🟡 未开始

#### 2. 日志配置文件支持（暂停）

**状态**: 📋 计划中（暂停）
**优先级**: 🟡 中等
**预计工时**: 2-3 天

**文档**: [日志配置开发计划](./DEV_PLAN_LOGGING_CONFIG.md)

**功能描述**:
扩展日志配置系统，支持通过配置文件设置常规日志级别。

**暂停原因**:
安全审批对话框功能优先级更高，此计划暂缓。

---

## 📁 归档位置

**已完成的计划**已移至：
- `docs/REPORT/` - 完成报告和改进记录
- `docs/INFO/` - 技术评估和知识文档
- `docs/INFO/ARCHIVE/` - 早期设计文档（已过时）

**查看归档**:
- [INFO 目录](../INFO/) - 技术文档和评估报告
- [REPORT 目录](../REPORT/) - 完成报告
- [INFO/ARCHIVE 目录](../INFO/ARCHIVE/) - 早期设计文档

---

## 🔗 相关文档

**活跃计划**:
- [安全审批开发路线图](./SECURITY_APPROVAL_DEVELOPMENT_ROADMAP.md)
- [安全审批单进程方案](./SECURITY_APPROVAL_SINGLE_PROCESS_PLAN.md)
- [日志配置开发计划](./DEV_PLAN_LOGGING_CONFIG.md)

**项目状态**:
- [当前计划状态](../INFO/PLAN_STATUS_2026-03-23.md)
- [安全审批完整实施计划](../INFO/SECURITY_APPROVAL_COMPLETE_IMPLEMENTATION_PLAN_2026-03-20.md)

---

## 📊 统计信息

```
活跃计划:    2 个
  - 正在进行:  1 个（安全审批对话框）
  - 未开始:    1 个（日志配置）

已完成计划:  8 个（已移至 REPORT/INFO）
```

---

**最后更新**: 2026-03-23
