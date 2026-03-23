# NemesisBot UI 开发者文档

**版本**: 1.0.0
**更新日期**: 2026-03-24
**技术栈**: Wails v2 + React v18 + Go 1.25

---

## 📖 目录

1. [项目结构](#项目结构)
2. [开发环境](#开发环境)
3. [构建流程](#构建流程)
4. [架构设计](#架构设计)
5. [API 参考](#api-参考)
6. [组件开发](#组件开发)
7. [样式系统](#样式系统)
8. [事件系统](#事件系统)
9. [测试指南](#测试指南)
10. [部署指南](#部署指南)

---

## 📁 项目结构

```
wails-ui/
├── main.go                      # 后端入口（~800 行）
├── go.mod                       # Go 模块定义
├── go.sum                       # Go 依赖锁定
├── wails.json                   # Wails 配置
├── README.md                    # 项目说明
├── USER_GUIDE.md                # 用户指南
├── DEVELOPER_GUIDE.md           # 开发者文档（本文件）
│
├── build/
│   └── bin/
│       └── nemesisbot-ui.exe    # 构建的可执行文件
│
└── frontend/                    # 前端源码
    ├── package.json             # 前端依赖
    ├── vite.config.js           # Vite 配置
    ├── index.html               # HTML 入口
    │
    └── src/
        ├── main.jsx             # React 入口
        ├── App.jsx              # 主应用组件（~500 行）
        ├── App.css              # 主应用样式（~1000 行）
        ├── index.css            # 全局样式
        │
        ├── components/
        │   ├── ApprovalDialog.jsx    # 审批对话框组件
        │   └── ApprovalDialog.css    # 审批对话框样式
        │
        └── wailsjs/             # Wails 自动生成
            ├── go/
            │   └── main/
            │       ├── App.js    # Go API 绑定
            │       └── App.d.ts  # TypeScript 类型定义
            └── runtime/
                ├── runtime.js   # Wails 运行时
                └── runtime.d.ts # TypeScript 类型定义
```

---

## 🛠️ 开发环境

### 必需软件

1. **Go** 1.25.7+
   ```bash
   go version
   ```

2. **Node.js** 24.13.0+
   ```bash
   node --version
   npm --version
   ```

3. **Wails CLI** v2.11.0+
   ```bash
   go install github.com/wailsapp/wails/v2/cmd/wails@latest
   wails version
   ```

### 环境变量

```bash
# 添加 Go bin 到 PATH
export PATH=$PATH:$(go env GOPATH)/bin
```

---

## 🔨 构建流程

### 开发模式（支持热重载）

```bash
cd wails-ui
wails dev
```

特点：
- 前端修改自动热重载
- 后端修改自动重启
- 实时日志输出
- 开发者工具集成

### 生产构建

```bash
cd wails-ui
wails build
```

输出：`build/bin/nemesisbot-ui.exe`

构建时间：~3 秒

### 清理构建

```bash
wails build -clean
```

---

## 🏗️ 架构设计

### 整体架构

```
┌─────────────────────────────────────┐
│         Wails Desktop App           │
├─────────────────────────────────────┤
│                                     │
│  ┌──────────────┐  ┌─────────────┐ │
│  │   Frontend   │  │   Backend   │ │
│  │   (React)    │◄─┤    (Go)     │ │
│  │              │  │             │ │
│  │  • Pages     │  │  • App      │ │
│  │  • Components│  │  • APIs     │ │
│  │  • Styles    │  │  • Managers │ │
│  └──────────────┘  └─────────────┘ │
│         ▲                   ▲      │
│         │                   │      │
│    Wails Runtime (WebView2 Bridge) │
└─────────────────────────────────────┘
```

### 通信机制

1. **Go → React (单向)**
   - 函数调用：React 调用 Go 导出的函数
   - 事件发送：Go 发送事件到 React

2. **React → Go (单向)**
   - 函数调用：通过 Wails 绑定
   - 事件监听：通过 `EventsOn` 监听

### 事件系统

**Go 端发送事件**:
```go
runtime.EventsEmit(a.ctx, "event-name", data)
```

**React 端监听事件**:
```jsx
EventsOn("event-name", (data) => {
    // 处理事件
})
```

---

## 📡 API 参考

### Desktop API

#### `GetDesktopInfo() → DesktopInfo`

获取桌面信息。

**返回**:
```go
type DesktopInfo struct {
    Version     string
    Environment string
    BotState    string
    Uptime      string
}
```

#### `StartBot() → error`

启动 Bot。

#### `StopBot() → error`

停止 Bot。

---

### Approval API

#### `ShowApproval(req: ApprovalRequest) → error`

显示审批对话框（已弃用，使用事件驱动）。

#### `SubmitApproval(response: ApprovalResponse) → error`

提交审批决定。

**参数**:
```go
type ApprovalResponse struct {
    RequestID       string
    Approved        bool
    TimedOut        bool
    DurationSeconds float64
    ResponseTime    int64
}
```

#### `GetApprovalHistory(limit: int) → []ApprovalHistoryItem`

获取审批历史。

#### `GetApprovalStats() → ApprovalStats`

获取审批统计。

#### `SimulateApproval(operation: string) → error`

模拟审批请求（事件驱动）。

---

### Chat API

#### `SendMessage(message: string) → string`

发送消息并获得响应。

#### `GetChatHistory(limit: int) → []ChatMessage`

获取聊天历史。

#### `ClearChatHistory() → error`

清空聊天历史。

---

### Logs API

#### `GetLogs(level: string, module: string, limit: int) → []LogEntry`

获取日志（支持过滤）。

#### `GetLogModules() → []string`

获取所有日志模块。

---

### Settings API

#### `GetSettings() → []Setting`

获取所有设置。

#### `UpdateSetting(key: string, value: string) → error`

更新设置值。

#### `GetThemeConfig() → ThemeConfig`

获取主题配置。

#### `SetTheme(theme: string, auto: bool) → error`

设置主题。

---

### System API

#### `GetSystemStatus() → SystemStatus`

获取系统状态。

---

## 🧩 组件开发

### 创建新组件

1. **创建组件文件**:
```jsx
// frontend/src/components/MyComponent.jsx
import { useState } from 'react'
import './MyComponent.css'

function MyComponent({ prop1, prop2 }) {
  const [state, setState] = useState(null)

  return (
    <div className="my-component">
      {/* 组件内容 */}
    </div>
  )
}

export default MyComponent
```

2. **创建样式文件**:
```css
/* frontend/src/components/MyComponent.css */
.my-component {
  /* 样式定义 */
}
```

3. **在 App.jsx 中使用**:
```jsx
import MyComponent from './components/MyComponent'

function App() {
  return (
    <div>
      <MyComponent prop1="value" />
    </div>
  )
}
```

---

## 🎨 样式系统

### CSS 变量

支持深色/浅色主题切换：

```css
:root {
  --bg-primary: #0f172a;
  --bg-secondary: #1e293b;
  --text-primary: #f1f5f9;
  --text-secondary: #94a3b8;
}

body.light-theme {
  --bg-primary: #f8fafc;
  --bg-secondary: #e2e8f0;
  --text-primary: #1e293b;
  --text-secondary: #64748b;
}
```

### 动画效果

内置动画：

| 名称 | 描述 | 用途 |
|------|------|------|
| fadeIn | 淡入 | 页面加载 |
| fadeOut | 淡出 | 页面卸载 |
| slideIn | 滑入 | 侧边栏 |
| slideOut | 滑出 | 侧边栏 |
| pulse | 脉冲 | 强调元素 |
| bounce | 弹跳 | 按钮点击 |
| shake | 抖动 | 错误提示 |
| spin | 旋转 | 加载指示 |

---

## 🔄 事件系统

### Go 端发送事件

```go
// 发送事件到前端
runtime.EventsEmit(a.ctx, "event-name", data)
```

### React 端监听事件

```jsx
import { EventsOn } from '../wailsjs/runtime/runtime'

useEffect(() => {
  const unlisten = EventsOn("event-name", (data) => {
    console.log('Received:', data)
    // 处理事件
  })

  return () => {
    if (unlisten) unlisten()
  }
}, [])
```

---

## 🧪 测试指南

### 单元测试

**Go 测试**:
```go
// main_test.go
package main

import "testing"

func TestGetDesktopInfo(t *testing.T) {
    app := NewApp()
    info := app.GetDesktopInfo()

    if info.Version == "" {
        t.Error("Version should not be empty")
    }
}
```

运行测试：
```bash
go test ./...
```

### 集成测试

手动测试流程：

1. **审批流程测试**
   - 点击模拟请求按钮
   - 验证对话框弹出
   - 测试允许/拒绝按钮
   - 验证历史记录更新

2. **Chat 功能测试**
   - 发送消息
   - 验证响应
   - 清空历史

3. **Logs 功能测试**
   - 查看日志
   - 测试过滤功能

4. **Settings 功能测试**
   - 切换主题
   - 修改设置
   - 验证快捷键

---

## 📦 部署指南

### 构建生产版本

```bash
# 清理并重新构建
wails build -clean

# 输出位置
build/bin/nemesisbot-ui.exe
```

### 分发

1. **独立分发**
   - 直接分发 `nemesisbot-ui.exe`
   - 无需额外依赖
   - 用户双击运行

2. **打包成安装程序**（可选）
   ```bash
   # 使用 Inno Setup 或 NSIS
   # 创建安装程序
   ```

3. **签名**（可选）
   ```bash
   # 使用代码签名证书
   signtool sign /f certificate.pfx nemesisbot-ui.exe
   ```

---

## 🔧 调试技巧

### 启用详细日志

```go
// main.go
log.SetFlags(log.LstdFlags | log.Lshortfile)
```

### 前端调试

1. 打开开发者工具
2. 查看控制台日志
3. 检查网络请求
4. 分析性能

### 后端调试

1. 查看终端输出
2. 使用 `log.Printf` 打印日志
3. 检查错误返回

---

## 📚 相关资源

### 官方文档

- [Wails 文档](https://wails.io/docs/introduction)
- [React 文档](https://react.dev/)
- [Go 文档](https://go.dev/doc/)

### 项目文档

- [用户指南](USER_GUIDE.md)
- [执行报告](../../docs/REPORT/FINAL_EXECUTION_SUMMARY_2026-03-24.md)
- [开发计划](../../docs/PLAN/UI_MIGRATION_DEVELOPMENT_PLAN_2026-03-24.md)

---

## 🤝 贡献指南

### 代码规范

1. **Go 代码**
   - 遵循 `gofmt` 格式
   - 添加注释
   - 错误处理

2. **React 代码**
   - 使用函数组件
   - Hooks 优先
   - PropTypes 或 TypeScript

3. **CSS 代码**
   - 使用 BEM 命名
   - 避免内联样式
   - 支持主题切换

### 提交代码

1. Fork 项目
2. 创建特性分支
3. 提交变更
4. 推送到分支
5. 创建 Pull Request

---

## 📝 版本历史

### v1.0.0 (2026-03-24)

**新增**:
- 完整的桌面应用框架
- 审批中心、Chat、Logs、Settings 页面
- 主题切换功能
- 键盘快捷键支持
- 事件驱动的审批流程

**技术**:
- Wails v2.11.0
- React v18
- Go 1.25.7
- 构建时间 ~3 秒
- 包大小 ~25 MB

---

**祝你开发顺利！** 🚀
