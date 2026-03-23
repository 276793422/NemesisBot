# Wails 安全审批对话框 Demo

这是一个使用 **Wails v2 + React** 实现的安全审批对话框演示项目，展示如何用 Golang 构建跨平台的桌面 UI。

## 🎯 功能特性

- ✅ 完整的安全审批对话框界面
- ✅ 4 种风险等级（LOW/MEDIUM/HIGH/CRITICAL）
- ✅ 倒计时自动拒绝机制
- ✅ 流畅的 CSS 动画和过渡效果
- ✅ 模拟后端审批请求
- ✅ 审批统计和记录
- ✅ 响应式设计
- ✅ 跨平台支持（Windows/macOS/Linux）

## 🚀 快速开始

### 前置要求

1. **Go 1.18+**
2. **Node.js 16+**
3. **Wails CLI**（已安装 v2.11.0）

### 开发模式运行

```bash
cd C:\AI\NemesisBot\NemesisBot\test\wails-approval-dialog

# 安装前端依赖
cd frontend
npm install
cd ..

# 启动开发服务器（支持热重载）
export PATH=$PATH:$(go env GOPATH)/bin
wails dev
```

### 构建生产版本

```bash
wails build
```

## 🎨 技术亮点

### 后端（Go）
- 审批请求结构定义
- 模拟后端 API
- 审批响应处理

### 前端（React）
- 响应式 UI 组件
- 实时倒计时
- 动画效果

### CSS 动画
- 淡入淡出、脉冲动画
- 倒计时进度条
- 按钮波纹效果
- 风险等级动画

## 📊 性能数据

- **应用大小**: ~20-40 MB
- **内存占用**: ~30-50 MB
- **启动时间**: <0.5 秒

## 🔗 相关资源

- [Wails 官方文档](https://wails.io/docs/introduction)
- [NemesisBot 项目](https://github.com/276793422/NemesisBot)

---

**创建日期**: 2026-03-24
**框架版本**: Wails v2.11.0
