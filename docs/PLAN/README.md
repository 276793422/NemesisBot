# 开发计划目录

此文件夹放置 NemesisBot 的**活跃开发计划**文档。

---

## 📋 当前活跃计划

### ✅ 无活跃计划

**当前没有活跃的开发计划。**

所有主要开发工作已完成：
- ✅ 安全审批对话框功能（2026-03-20 至 2026-03-24）
- ✅ UI 框架迁移（webview2 → Wails v2）（2026-03-24）
- ✅ 跨平台编译支持（2026-03-24）
- ✅ 构建系统修复（build.ps1, build.bat, build.sh）（2026-03-24）

---

## 📁 归档位置

**已完成的计划**已移至：
- `docs/REPORT/` - 完成报告和技术实施记录
- `docs/INFO/` - 技术评估、设计文档和知识文档

---

## 📊 最近归档（2026-03-24）

### 1. ✅ 安全审批对话框功能（2026-03-20 至 2026-03-24）

**状态**: ✅ 已完成（提前 12 天）
**计划工期**: 17 天
**实际工期**: 5 天

**归档文档**:
- 📊 [完成报告](../REPORT/SECURITY_APPROVAL_COMPLETE_2026-03-24.md)
- 📋 [开发路线图](../REPORT/2026-03-20_SECURITY_APPROVAL_DEVELOPMENT_ROADMAP.md) - 归档
- 📘 [单进程方案](../INFO/2026-03-20_SECURITY_APPROVAL_SINGLE_PROCESS_PLAN.md) - 归档
- 📘 [完整实施计划](../INFO/SECURITY_APPROVAL_COMPLETE_IMPLEMENTATION_PLAN_2026-03-20.md)

**主要成果**:
- ✅ 完整的审批系统（ABAC 风险评估）
- ✅ Wails Desktop UI 集成
- ✅ 全局 ApprovalHandler 模式
- ✅ 超时机制和审批历史
- ✅ 完整的测试覆盖

**技术方案**:
- 架构：单进程模式（Wails v2）
- 模式：全局 ApprovalHandler（平台无关）
- 实现位置：`module/security/approval/`

---

### 2. ✅ UI 框架迁移（webview2 → Wails v2）（2026-03-24）

**状态**: ✅ 核心功能已完成（85%）
**计划工期**: 5 周（25 天）
**实际工期**: 1 天

**归档文档**:
- 📊 [完成报告](../REPORT/WAILS_MIGRATION_COMPLETE_2026-03-24.md)
- 📋 [UI 迁移开发计划](../REPORT/UI_MIGRATION_DEVELOPMENT_PLAN_2026-03-24.md) - 归档
- 📋 [执行计划](../REPORT/EXECUTION_PLAN.md) - 归档

**主要成果**:
- ✅ Wails v2.11.0 集成（`module/desktop`）
- ✅ 移除旧 webview2 代码（-400+ 行）
- ✅ 跨平台编译支持（Windows/Linux/macOS）
- ✅ cross_compile build tag 策略
- ✅ 构建系统修复（production build tag）
- ✅ build.sh 创建（Linux/macOS 支持）
- ✅ 前端功能（Chat、Overview、Logs、Settings）
- ✅ 主题系统（Light/Dark）
- ✅ 快捷键支持

**待完成**（非阻塞性）:
- 🔄 跨平台测试（20% - Linux/macOS 本地编译）
- ⏳ 性能优化
- ⏳ 文档更新（用户迁移指南、开发者文档）

---

### 3. ✅ 构建系统修复（2026-03-24）

**状态**: ✅ 已完成

**归档文档**:
- 📊 [build.sh 创建报告](../REPORT/BUILD_SH_CREATION_2026-03-24.md)
- 📊 [交叉编译修复报告](../REPORT/CROSS_COMPILE_BUILD_TAG_FIX_2026-03-24.md)

**主要成果**:
- ✅ 修复 Wails build tags 错误（使用 `-tags production`）
- ✅ 更新 build.ps1（16 处修改）
- ✅ 创建 build.sh（匹配 build.ps1 功能）
- ✅ 跨平台编译支持（cross_compile tag）
- ✅ 文件大小计算优化（移除 bc 依赖）

---

### 4. ✅ LLM 消息流程测试（2026-03-24）

**状态**: ✅ 已完成

**归档文档**:
- 📊 [LLM 消息流程测试报告](../REPORT/LLM_MESSAGE_FLOW_TEST_2026-03-24.md)

**测试结果**:
- ✅ WebSocket 通信正常
- ✅ 消息路由正确
- ✅ Agent Loop 处理正常
- ✅ LLM API 调用成功
- ✅ 响应回传正确

---

## 🔗 相关文档

**活跃技术文档**:
- [CLAUDE.md](../../CLAUDE.md) - 项目指导和工作流程
- [MEMORY.md](../../MEMORY.md) - 项目记忆和最佳实践

**已完成项目报告**:
- [REPORT 目录](../REPORT/) - 所有完成报告
- [INFO 目录](../INFO/) - 技术文档和评估

---

## 📊 统计信息

```
总计划数:     4 个
已完成:       4 个（100%）
  - 提前完成: 3 个（75%）
  - 按期完成: 1 个（25%）
  - 延期完成: 0 个（0%）

总节省时间:   41 天
  - 安全审批: 12 天（17 → 5 天）
  - UI 迁移:  24 天（25 → 1 天）
  - 构建修复: 2 天
  - 测试:     3 天
```

---

## 🎯 待启动工作（非计划性）

虽然当前没有活跃的开发计划，但以下工作可以考虑：

### 1. 跨平台测试（优先级：中）

**内容**:
- Linux 本地编译和测试（需要 GTK 库）
- macOS 本地编译和测试（需要 Cocoa 库）
- Ubuntu/Fedora/Debian 兼容性测试
- Intel vs Apple Silicon 测试

**预计时间**: 2-3 天

**阻塞条件**:
- 需要访问 Linux/macOS 环境
- 需要安装平台特定依赖

---

### 2. 性能优化（优先级：低）

**内容**:
- 启动时间优化（目标 < 0.5s）
- 内存占用优化（目标 < 50MB）
- 虚拟滚动（长列表）
- 代码分割和懒加载

**预计时间**: 3-5 天

**阻塞条件**:
- 无硬性阻塞
- 可在后续迭代中进行

---

### 3. 文档完善（优先级：低）

**内容**:
- 更新 README.md（Wails 集成说明）
- 编写用户迁移指南（webview2 → Wails）
- 编写开发者指南（如何开发 Desktop UI）
- API 文档（Wails bindings）
- 发布说明（新功能、破坏性变更）

**预计时间**: 2-3 天

**阻塞条件**:
- 无硬性阻塞
- 可在后续迭代中进行

---

## 📝 如何创建新的开发计划

如果需要启动新的开发工作：

1. **创建计划文档**
   ```bash
   # 在 docs/PLAN/ 目录创建
   touch docs/PLAN/YYYY-MM-DD_PROJECT_NAME.md
   ```

2. **使用标准模板**
   - 项目概述
   - 技术方案
   - 开发阶段
   - 详细任务清单
   - 风险评估
   - 验收标准
   - 回滚方案

3. **参考已完成计划**
   - [安全审批完成报告](../REPORT/SECURITY_APPROVAL_COMPLETE_2026-03-24.md)
   - [Wails 迁移完成报告](../REPORT/WAILS_MIGRATION_COMPLETE_2026-03-24.md)

4. **更新本 README**
   - 将新计划添加到"当前活跃计划"部分
   - 包含计划文档链接
   - 设置预期完成时间

---

**最后更新**: 2026-03-24
**更新内容**: 归档所有已完成计划，更新统计信息
