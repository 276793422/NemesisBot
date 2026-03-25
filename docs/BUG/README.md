# BUG 目录

此文件夹放置 NemesisBot 的**当前存在的 BUG**文档。

---

## 📋 当前 BUG 列表

**更新日期**: 2026-03-25

| BUG | 严重程度 | 状态 | 影响范围 |
|-----|---------|------|---------|
| [LLM 优先使用外部 Python 脚本而非内置网络工具](2026-03-11_LLM_PREFERS_PYTHON_OVER_BUILTIN_TOOLS.md) | 中等 | ❌ 未修复 | 网络请求操作 |

**当前 BUG 总数**: 1 个

---

## 🔴 当前 BUG 详情

### LLM 优先使用外部 Python 脚本而非内置网络工具

**发现日期**: 2026-03-11
**严重程度**: 中等
**状态**: ❌ 未修复

**问题描述**:
LLM 在处理网络请求时优先使用外部 Python 脚本，而不是内置的 `web_fetch` 工具，导致：
- Windows 环境下路径处理失败
- Token 消耗增加 70%+
- 用户体验差

**复现条件**:
- 使用 skill 进行网络 API 调用
- LLM 模型：智谱 GLM-4.5-air / GLM-4.7-flash
- 操作系统：Windows 11

**推荐修复方案**:
1. 更新 `module/tools/web.go` 中的 `web_fetch` 工具描述
2. 在 IDENTITY.md 添加工具使用优先级说明

**预计修复时间**: 30 分钟

**相关文档**: [2026-03-11_LLM_PREFERS_PYTHON_OVER_BUILTIN_TOOLS.md](2026-03-11_LLM_PREFERS_PYTHON_OVER_BUILTIN_TOOLS.md)

---

## 📊 统计信息

```
当前 BUG:     1 个（中等严重）
按状态:
  - 未修复: 1 个

按严重程度:
  - 高:   0 个
  - 中:   1 个
  - 低:   0 个
```

---

## ✅ 已修复 BUG

以下 BUG 已修复并移至 `docs/REPORT/`：

### Peer Chat 阻塞问题
- **修复日期**: 2026-03-24
- **Git 提交**: 80ec110
- **修复方式**: 超时配置优化
- **归档位置**: docs/REPORT/2026-03-06_PEER_CHAT_BLOCKING_INVESTIGATION.md 及相关分析文档

### Windows 命令执行问题
- **修复日期**: 2026-03-25
- **修复方式**: 实现 `normalizeWindowsPaths()` 函数
- **归档位置**: docs/REPORT/2026-03-11_WINDOWS_COMMAND_ANALYSIS_DETAILED.md

### 测试环境残留问题
- **修复日期**: 2026-03-23
- **修复方式**: 清理流程已更新
- **归档位置**: docs/REPORT/2026-03-23_nemesisbot-residual-analysis.md

---

## 🎯 建议修复顺序

```
优先级 1（立即处理，30 分钟）：
  └─ 修复 LLM 工具选择问题
      ├─ 更新 web_fetch 工具描述
      └─ 在 IDENTITY.md 添加工具优先级说明
```

---

## 🔗 相关文档

**项目管理**:
- [../PLAN/README.md](../PLAN/README.md) - 开发计划
- [CLAUDE.md](../../CLAUDE.md) - 项目指导

**已完成报告**:
- [../REPORT/](../REPORT/) - 已修复 BUG 和完成任务的报告

---

## 📝 如何创建新 BUG 文档

### 1. 创建 BUG 文档

```bash
# 在 docs/BUG/ 目录创建
# 文件命名规范：YYYY-MM-DD_BUG简述.md
touch docs/BUG/2026-03-25_NEW_BUG.md
```

### 2. BUG 文档模板

BUG 文档应包含：
- **问题描述** - 清晰描述 BUG 现象
- **复现步骤** - 如何重现问题
- **影响范围** - 影响的功能和用户
- **严重程度** - 高/中/低
- **推荐修复方案** - 如何修复
- **测试验证** - 如何验证修复

### 3. 更新本 README

- 将新 BUG 添加到"当前 BUG 列表"
- 设置正确的严重程度和状态

---

**最后更新**: 2026-03-25
**说明**: 本 README 只记录当前存在的 BUG，已修复的 BUG 已移至 docs/REPORT/
