# 🚀 快速开始 - Wails 安全审批对话框

## 一键运行

### 方法 1: 直接运行（已构建）

```bash
# 进入项目目录
cd C:\AI\NemesisBot\NemesisBot\test\wails-approval-dialog

# 运行已构建的程序
start "" "build\bin\wails-approval-dialog.exe"
```

### 方法 2: 开发模式（支持热重载）

```bash
# 1. 进入项目目录
cd C:\AI\NemesisBot\NemesisBot\test\wails-approval-dialog

# 2. 设置 PATH
export PATH=$PATH:$(go env GOPATH)/bin

# 3. 启动开发服务器
wails dev
```

### 方法 3: 重新构建

```bash
# 1. 进入项目目录
cd C:\AI\NemesisBot\NemesisBot\test\wails-approval-dialog

# 2. 构建应用
export PATH=$PATH:$(go env GOPATH)/bin
wails build

# 3. 运行
start "" "build\bin\wails-approval-dialog.exe"
```

## 📂 项目结构

```
wails-approval-dialog/
├── main.go                 # 主入口
├── app.go                  # 后端逻辑
├── frontend/               # 前端代码
│   ├── src/
│   │   ├── App.jsx         # React 组件
│   │   └── App.css         # 样式文件
│   └── package.json
├── build/
│   └── bin/
│       └── wails-approval-dialog.exe  # 构建输出
├── README.md               # 项目说明
├── DEMO_GUIDE.md           # 使用指南
├── PROJECT_SUMMARY.md      # 项目总结
└── wails.json              # Wails 配置
```

## 🎯 功能展示

### 主界面
- 📋 待审批请求列表（4个预设请求）
- 📊 实时统计（已批准/已拒绝）
- 🔄 模拟新请求按钮

### 审批界面
- ⚠️ 警告消息（橙色高亮）
- 📝 操作详情展示
- 🎨 风险等级标识
- ⏰ 倒计时进度条
- ✅ 允许/拒绝按钮
- 💫 流畅动画效果

### 风险等级
- 🟢 LOW - 低风险
- 🟡 MEDIUM - 中风险
- 🟠 HIGH - 高风险
- 🔴 CRITICAL - 严重风险（带脉冲动画）

## 💻 快速命令

```bash
# 安装 Wails CLI（如果未安装）
go install github.com/wailsapp/wails/v2/cmd/wails@latest

# 安装前端依赖
cd frontend
npm install
cd ..

# 开发模式
wails dev

# 构建生产版本
wails build

# 清理构建文件
wails build -clean
```

## 🎨 自定义配置

### 修改窗口大小

编辑 `main.go`:
```go
Width:  680,   // 修改宽度
Height: 620,   // 修改高度
```

### 修改超时时间

编辑 `app.go`:
```go
TimeoutSeconds: 30,  // 修改超时秒数
```

### 修改风险等级

编辑 `app.go`:
```go
operations := map[string][]string{
    "LOW":       {"file_read", "读取配置文件"},
    "MEDIUM":    {"file_write", "写入文件"},
    "HIGH":      {"file_delete", "删除文件"},
    "CRITICAL":  {"process_exec", "执行系统命令"},
}
```

## 🔧 常见问题

### Q1: wails 命令找不到？

```bash
# 添加到 PATH
export PATH=$PATH:$(go env GOPATH)/bin

# 验证安装
wails version
```

### Q2: 前端依赖安装失败？

```bash
# 清理缓存
cd frontend
rm -rf node_modules package-lock.json
npm install
```

### Q3: 构建失败？

```bash
# 清理构建文件
wails build -clean

# 重新构建
wails build
```

### Q4: 应用无法启动？

- 检查 WebView2 是否已安装（Windows 10+ 自带）
- 检查防火墙设置
- 查看控制台错误信息

## 📊 性能数据

| 指标 | 数值 |
|------|------|
| 应用大小 | ~20-40 MB |
| 内存占用 | ~30-50 MB |
| 启动时间 | <0.5 秒 |
| 构建时间 | ~10 秒 |

## 🎯 下一步

1. **修改界面**: 编辑 `frontend/src/App.jsx` 和 `App.css`
2. **添加功能**: 在 `app.go` 中添加新的导出方法
3. **调整配置**: 修改 `main.go` 中的应用设置
4. **集成到项目**: 将代码复制到 NemesisBot 项目

## 📚 相关文档

- [README.md](./README.md) - 项目说明
- [DEMO_GUIDE.md](./DEMO_GUIDE.md) - 详细使用指南
- [PROJECT_SUMMARY.md](./PROJECT_SUMMARY.md) - 项目总结
- [Wails 官方文档](https://wails.io/docs/introduction)

## 🎉 开始使用

```bash
# 一键启动
cd C:\AI\NemesisBot\NemesisBot\test\wails-approval-dialog
start "" "build\bin\wails-approval-dialog.exe"
```

**享受你的 Wails 安全审批对话框！** 🚀
