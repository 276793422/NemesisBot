# Wails 安全审批对话框 Demo - 使用说明

## 📋 项目信息

**位置**: `C:\AI\NemesisBot\NemesisBot\test\wails-approval-dialog`

**技术栈**:
- 后端: Go 1.25.7
- 前端: React + Vite
- 框架: Wails v2.11.0
- 构建时间: 10.5 秒
- 输出大小: ~20-40 MB

## 🎯 功能演示

### 1. 主界面 - 待审批请求列表

应用启动后会显示：
- **待审批请求列表**: 4 个预设的审批请求
- **统计信息**: 已批准/已拒绝数量
- **模拟新请求按钮**: 生成随机审批请求

### 2. 审批对话框界面

选择任意请求后进入审批界面：

#### 🎨 界面元素

1. **警告消息**
   - 橙色高亮显示
   - 抖动动画吸引注意

2. **操作名称**
   - 大字体显示操作类型（如"删除文件"）
   - 代码字体显示目标路径

3. **操作详情**
   - 操作类型: file_delete / process_exec / registry_write 等
   - 操作目标: 具体路径或命令
   - 危险等级: 🟢 LOW / 🟡 MEDIUM / 🟠 HIGH / 🔴 CRITICAL
   - 原因: AI Agent 的操作理由

4. **倒计时**
   - ⏰ 图标 + 秒数显示
   - 进度条显示剩余时间
   - 剩余 10 秒时变红并放大

5. **操作按钮**
   - ✅ 允许执行（绿色）
   - ✗ 拒绝操作（红色）
   - 按钮悬停有波纹效果

### 3. 风险等级展示

| 等级 | 颜色 | 动画效果 | 示例操作 |
|------|------|---------|---------|
| **LOW** | 🟢 绿色 | 无 | 读取配置文件 |
| **MEDIUM** | 🟡 黄色 | 无 | 写入文件、网络下载 |
| **HIGH** | 🟠 橙色 | 无 | 删除文件 |
| **CRITICAL** | 🔴 红色 | 脉冲闪烁 | 执行系统命令 |

### 4. 动画效果

- ✨ **淡入动画**: 界面切换时平滑淡入
- 🔄 **脉冲动画**: 头部背景光晕效果
- 🎯 **滑入动画**: 请求卡片依次滑入
- ⏰ **倒计时动画**: 时钟图标左右摆动
- 🚨 **警告抖动**: 警告消息快速抖动
- 💫 **按钮波纹**: 按钮悬停时水波扩散
- 📊 **进度条**: 倒计时进度条平滑缩减

### 5. 模拟新请求

点击"模拟新请求"后：
1. 显示 4 个风险等级选择按钮
2. 点击任意等级生成随机请求
3. 自动进入审批界面
4. 开始倒计时

## 🚀 快速命令

### 开发模式（支持热重载）
```bash
cd C:\AI\NemesisBot\NemesisBot\test\wails-approval-dialog
export PATH=$PATH:$(go env GOPATH)/bin
wails dev
```

### 构建生产版本
```bash
wails build
```

### 运行构建的程序
```bash
# Windows
start "" "build\bin\wails-approval-dialog.exe"

# 或直接双击
build\bin\wails-approval-dialog.exe
```

## 📊 代码结构

### 后端 (app.go)

```go
// 审批请求结构
type ApprovalRequest struct {
    RequestID      string
    Operation      string
    OperationName  string
    Target         string
    RiskLevel      string
    Reason         string
    TimeoutSeconds int
    Context        map[string]string
}

// 导出的方法（前端可调用）
func (a *App) GetDemoRequests() []ApprovalRequest
func (a *App) SubmitApproval(response ApprovalResponse) error
func (a *App) SimulateBackendRequest(riskLevel string) ApprovalRequest
func (a *App) GetSystemInfo() map[string]interface{}
```

### 前端 (App.jsx)

```jsx
// 调用后端方法
import { GetDemoRequests, SubmitApproval } from '../wailsjs/go/main/App'

// React Hooks
const [currentRequest, setCurrentRequest] = useState(null)
const [countdown, setCountdown] = useState(30)
const [approvedCount, setApprovedCount] = useState(0)
const [deniedCount, setDeniedCount] = useState(0)

// 主要功能
- selectRequest(request): 选择请求并启动倒计时
- handleApprove(): 批准请求
- handleDeny(): 拒绝请求
- handleTimeout(): 超时自动拒绝
- simulateNewRequest(): 模拟新请求
```

## 🎨 CSS 亮点

### 渐变背景
```css
background: linear-gradient(135deg, #0f172a 0%, #1e293b 100%);
```

### 毛玻璃效果
```css
background: rgba(30, 41, 59, 0.95);
backdrop-filter: blur(10px);
```

### 脉冲动画（Critical 等级）
```css
@keyframes criticalPulse {
    0%, 100% { box-shadow: 0 0 0 0 rgba(239, 68, 68, 0.4); }
    50% { box-shadow: 0 0 0 10px rgba(239, 68, 68, 0); }
}
```

### 按钮波纹效果
```css
.btn::before {
    content: '';
    position: absolute;
    width: 0;
    height: 0;
    border-radius: 50%;
    background: rgba(255, 255, 255, 0.2);
    transition: width 0.6s, height 0.6s;
}
.btn:hover::before {
    width: 300px;
    height: 300px;
}
```

## 🔧 自定义配置

### 修改窗口大小 (main.go)
```go
Width:  680,
Height: 620,
```

### 修改超时时间 (app.go)
```go
TimeoutSeconds: 30,
```

### 添加新的风险等级 (app.go)
```go
operations := map[string][]string{
    "EXTREME": {"system_shutdown", "关闭系统"},
}
```

## 💡 使用场景

这个 demo 展示了：
1. ✅ Wails 的跨平台 UI 能力
2. ✅ React + Go 的前后端通信
3. ✅ CSS 动画和特效实现
4. ✅ 响应式状态管理
5. ✅ 倒计时和定时器处理
6. ✅ 模态对话框交互

## 🎯 适用项目类型

- AI Agent 安全审批系统
- 系统管理工具
- 权限管理界面
- 操作确认对话框
- 风险控制面板
- 监控告警界面

## 📝 下一步可以扩展

- [ ] 系统托盘集成
- [ ] 系统通知
- [ ] 全局快捷键
- [ ] 审批历史记录
- [ ] Web 管理界面
- [ ] 主题切换（亮/暗）
- [ ] 多语言支持
- [ ] 声音提示
- [ ] 操作日志导出

## 🔗 相关文档

- [Wails 官方文档](https://wails.io/docs/introduction)
- [React 官方文档](https://react.dev/)
- [NemesisBot 项目](https://github.com/276793422/NemesisBot)

---

**演示完成！** 🎉

你现在应该能看到一个运行中的安全审批对话框应用，具有流畅的动画效果和完整的交互功能。
