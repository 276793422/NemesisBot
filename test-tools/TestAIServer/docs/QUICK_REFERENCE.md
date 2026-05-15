# TestAIServer 快速参考卡片

## 🚨 使用前必读

1. **已知问题清单**: `docs/KNOWN_ISSUES.md` ⚠️ **必读**
2. **流式响应修复**: `STREAMING_FIX.md` (v1.3.0)
3. **更新日志**: `CHANGELOG.md`

---

## 📋 已知问题快速索引

| 问题ID | 状态 | 描述 | 文档位置 |
|--------|------|------|----------|
| ISSUE-001 | 🟡 临时解决 | 不支持流式响应（兼容模式） | [KNOWN_ISSUES.md#ISSUE-001](docs/KNOWN_ISSUES.md#issue-001-不支持流式响应streaming-response) |
| ISSUE-002 | 🔴 未解决 | Token 计数不准确 | [KNOWN_ISSUES.md#ISSUE-002](docs/KNOWN_ISSUES.md#issue-002-token-计数不准确) |
| ISSUE-003 | 🔴 未解决 | 日志文件无自动清理 | [KNOWN_ISSUES.md#ISSUE-003](docs/KNOWN_ISSUES.md#issue-003-日志文件无自动清理) |

---

## 🔍 快速查找指南

### 查找已知问题

```bash
# 查看所有已知问题
cat docs/KNOWN_ISSUES.md

# 搜索特定问题
grep "ISSUE-001" docs/KNOWN_ISSUES.md
```

### 查找代码中的问题标记

```bash
# 查找所有已知问题标记
grep -r "KNOWN ISSUE" .
grep -r "⚠️" handlers/

# 查找流式响应相关代码
grep -A5 "stream" handlers/handlers.go
```

### 查看服务器日志

```bash
# 查看最新日志
ls -lt log/testai-1.1/ | head -1

# 查看警告日志（流式响应）
grep "WARNING" log/*/
```

---

## 🎯 版本历史快速参考

| 版本 | 主要变更 | 关键文档 |
|------|----------|----------|
| **v1.3.0** | 流式响应兼容模式 | `STREAMING_FIX.md` |
| v1.2.0 | 监听 0.0.0.0 | `NETWORK_CONFIG.md` |
| v1.1.0 | 自动日志记录 | `LOGGING.md` |
| v1.0.0 | 初始版本 | `README.md` |

---

## 📝 文档索引

### 核心文档
- `README.md` - 主文档
- `QUICKSTART.md` - 快速启动
- `CHANGELOG.md` - 更新日志

### 问题跟踪
- `docs/KNOWN_ISSUES.md` - ⚠️ 已知问题清单（必读）
- `STREAMING_FIX.md` - 流式响应修复说明
- `NETWORK_CONFIG.md` - 网络配置说明

### 功能文档
- `LOGGING.md` - 日志功能详细文档
- `PROJECT_SUMMARY.md` - 项目总结

### 更新记录
- `LOGGING_UPDATE.md` - 日志功能更新
- `NETWORK_UPDATE.md` - 网络配置更新
- `IMPLEMENTATION_COMPLETE.md` - 实现完成报告

---

## ⚡ 快速命令

### 构建和启动

```bash
# 构建
go build -o testaiserver.exe

# 启动
testaiserver.exe

# 查看日志
tail -f log/testai-1.1/*.log
```

### 测试

```bash
# 基础测试
curl http://localhost:8080/v1/models

# 流式响应测试（兼容模式）
curl http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"testai-1.1","messages":[{"role":"user","content":"测试"}],"stream":true}'
```

---

## 🚫 常见问题

### Q: Cherry Studio 无法使用？

**A**: v1.3.0 已修复。请更新到最新版本。

### Q: 为什么没有逐字输出？

**A**: 这是 ISSUE-001 的限制。当前使用兼容模式，不支持真正的流式响应。

详见: `docs/KNOWN_ISSUES.md#ISSUE-001`

### Q: 如何查找已知问题？

**A**: 查看 `docs/KNOWN_ISSUES.md`

### Q: 代码中哪里标记了问题？

**A**: 搜索 `// ⚠️ KNOWN ISSUE` 或 `// TODO`

---

## 📞 获取帮助

1. 查看 `docs/KNOWN_ISSUES.md`
2. 查看 `STREAMING_FIX.md`
3. 查看 `CHANGELOG.md`
4. 查看服务器日志 `log/*/`

---

**最后更新**: 2026-03-11
**版本**: v1.3.0
