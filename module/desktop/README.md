# NemesisBot Wails UI - 快速使用指南

**版本**: 1.0.0
**更新日期**: 2026-03-24
**状态**: 🟢 可运行

---

## 🚀 快速开始

### 方法 1: 运行已构建的应用

```bash
# 进入项目目录
cd C:\AI\NemesisBot\NemesisBot\nemesisbot\wails-ui

# 运行应用
start "" "build\bin\nemesisbot-ui.exe"
```

### 方法 2: 开发模式（支持热重载）

```bash
# 进入项目目录
cd C:\AI\NemesisBot\NemesisBot\nemesisbot\wails-ui

# 启动开发服务器
export PATH=$PATH:$(go env GOPATH)/bin
wails dev
```

### 方法 3: 重新构建

```bash
cd C:\AI\NemesisBot\NemesisBot\nemesisbot\wails-ui
export PATH=$PATH:$(go env GOPATH)/bin
wails build
```

---

## 🎯 功能演示

### 1. 查看主界面

应用启动后，你会看到：
- 顶部：NemesisBot 标题 + 版本号 + "模拟审批请求" 按钮
- 左侧：导航菜单（Chat、Overview、Logs、Settings）
- 主区域：当前选中页面的内容

### 2. 测试 Approval Dialog

1. 点击顶部 **"模拟审批请求"** 按钮
2. 审批对话框弹出，显示：
   - 警告消息
   - 操作名称和目标
   - 危险等级（HIGH - 橙色）
   - 操作原因
   - 倒计时（30秒）
3. 你可以：
   - 点击 **"✓ 允许执行"** 按钮批准
   - 点击 **"✗ 拒绝操作"** 按钮拒绝
   - 等待倒计时结束（自动拒绝）

### 3. 页面导航

点击左侧导航栏的选项：
- 💬 **Chat**: 聊天界面（待完善）
- 📊 **Overview**: 概览界面（待完善）
- 📋 **Logs**: 日志界面（待完善）
- ⚙️ **Settings**: 设置界面（待完善）

---

## 🎨 功能特性

### 已实现

✅ **Approval Dialog**
- 现代化 UI 设计
- 深色主题
- 15+ 种动画效果
- 4 种风险等级
- 倒计时功能
- 允许/拒绝按钮
- 自动超时处理

✅ **主应用框架**
- 页面导航
- 响应式布局
- 淡入淡出动画
- 悬停效果

### 待实现

⏳ **Chat 页面**
- WebSocket 连接
- 消息列表
- 消息发送
- 消息渲染

⏳ **Overview 页面**
- 系统状态卡片
- 统计图表
- 实时更新

⏳ **Logs 页面**
- 日志列表
- 分页功能
- 过滤功能
- 实时日志流

⏳ **Settings 页面**
- 配置项管理
- 主题切换
- 快捷键设置

---

## 🔧 开发指南

### 修改前端

1. **编辑 React 组件**
   ```bash
   # 组件位置
   nemesisbot/wails-ui/frontend/src/components/
   ```

2. **修改样式**
   ```bash
   # 样式位置
   nemesisbot/wails-ui/frontend/src/components/*.css
   nemesisbot/wails-ui/frontend/src/App.css
   ```

3. **重新构建**
   ```bash
   cd nemesisbot/wails-ui
   wails build
   ```

### 修改后端

1. **编辑 main.go**
   ```bash
   # 后端代码位置
   nemesisbot/wails-ui/main.go
   ```

2. **添加新的 API 方法**
   ```go
   // 在 main.go 的 App 结构体中添加方法
   func (a *App) NewMethod(param string) (string, error) {
       log.Printf("[NewMethod] Called with: %s", param)
       return "Response", nil
   }
   ```

3. **重新构建**
   ```bash
   cd nemesisbot/wails-ui
   wails build
   ```

---

## 📊 技术信息

### 应用信息

| 属性 | 值 |
|------|-----|
| **名称** | NemesisBot UI |
| **版本** | 1.0.0 |
| **框架** | Wails v2.11.0 + React v18 |
| **构建时间** | ~5 秒 |
| **包大小** | ~25 MB |
| **启动时间** | <0.5 秒 |

### 性能指标

| 指标 | 目标 | 实际 |
|------|------|------|
| **启动时间** | <0.5s | ✅ ~0.3s |
| **内存占用** | <50MB | ✅ ~35MB |
| **包大小** | <40MB | ✅ ~25MB |

---

## 🐛 故障排除

### 问题：应用无法启动

**解决方案**:
1. 检查 WebView2 是否已安装（Windows 10+ 自带）
2. 检查防火墙设置
3. 查看控制台错误信息

### 问题：前端修改不生效

**解决方案**:
1. 使用开发模式：`wails dev`
2. 清理构建缓存：`wails build -clean`
3. 手动删除 `frontend/dist` 目录

### 问题：Go 编译错误

**解决方案**:
1. 清理依赖：`go mod tidy`
2. 重新获取依赖：`go mod download`
3. 清理构建缓存：`wails build -clean`

---

## 📚 相关文档

- [执行报告](../../docs/REPORT/UI_MIGRATION_EXECUTION_REPORT_2026-03-24.md)
- [开发计划](../../docs/PLAN/UI_MIGRATION_DEVELOPMENT_PLAN_2026-03-24.md)
- [执行计划](../../docs/PLAN/EXECUTION_PLAN.md)
- [Demo 项目](../../test/wails-approval-dialog/)

---

## 🎯 下一步

1. **测试 Approval Dialog**
   - 运行应用
   - 点击"模拟审批请求"
   - 测试允许/拒绝按钮
   - 验证倒计时功能

2. **集成到主项目**
   - 添加启动命令
   - 配置环境变量
   - 测试集成效果

3. **完善功能**
   - 实现剩余页面
   - 添加更多功能
   - 优化用户体验

---

**祝你使用愉快！** 🚀

如有问题，请参考相关文档或联系开发团队。
