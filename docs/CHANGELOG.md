# 修改日志 (Changelog)

本文记录 NemesisBot 项目的所有重要更改和升级。

格式基于 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.0.0/)，
版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

---

## [未发布]

### 2026-02-25 (第二次更新)

#### ✨ 新增 (Added)

##### 命令行 Web 认证管理

- **新增 `channel web` 子命令**
  - `nemesisbot channel web auth` - 交互式设置 Web 认证 token（安全模式）
  - `nemesisbot channel web auth set <token>` - 命令行直接设置 token（便捷模式）
  - `nemesisbot channel web auth get` - 查看当前设置的 token
  - `nemesisbot channel web status` - 查看 Web 认证状态
  - `nemesisbot channel web clear` - 清除 Web 认证 token
  - `nemesisbot channel web config` - 显示详细 Web 配置

- **两种设置方式**
  - **交互式模式**（推荐手动使用）
    - Token 不在命令行参数中暴露
    - 避免在进程列表和终端历史中泄露
    - 适合日常手动配置

  - **命令行模式**（推荐脚本/自动化）
    - 直接通过参数传递 token
    - 方便脚本和自动化场景
    - 有安全提示警告风险

- **安全特性**
  - Token 长度验证（建议至少 8 字符）
  - 二次确认机制，防止误操作
  - 命令行模式显示安全警告
  - 状态显示中隐藏 token 内容（除了 auth get）

#### 🔧 技术细节

##### 新增的文件

```
nemesisbot/command/channel_web.go  # Web 子命令实现
```

##### 修改的文件

```
nemesisbot/command/channel.go       # 添加 web 子命令路由
docs/CHANGELOG.md                    # 更新修改日志
README.md                             # 更新使用说明
```

#### 🎯 使用方式

##### 交互式模式（推荐）

```bash
# 安全地设置 token
nemesisbot channel web auth
```

##### 命令行模式（方便）

```bash
# 快速设置 token
nemesisbot channel web auth set my-secret-token
nemesisbot channel web auth set 276793422

# 查看当前 token
nemesisbot channel web auth get
```

##### 其他命令

```bash
# 查看认证状态
nemesisbot channel web status

# 查看详细配置
nemesisbot channel web config

# 清除 token
nemesisbot channel web clear
```

#### 🔒 安全说明

- **交互式模式**：Token 不在命令行参数中，更安全
- **命令行模式**：Token 在参数中可见，适合脚本但需谨慎
- **存储安全**：Token 保存在配置文件中，建议设置文件权限 0600
- **状态保护**：Token 不在日志和常规状态输出中显示

---

### 2026-02-25 (第一次更新)

#### ✨ 新增 (Added)

##### Web 认证功能

- **前端登录界面**
  - 添加用户友好的登录界面，包含密钥输入框
  - 支持"记住我"选项，密钥存储在浏览器 localStorage
  - 添加登录错误提示和连接状态反馈
  - 新增注销功能，可清除本地存储的密钥

- **认证逻辑实现**
  - 实现 `AuthManager` 类处理密钥的存储、读取和清除
  - 修改 `WebSocketManager` 支持在连接时携带 auth token
  - 更新 `UIController` 实现完整的登录流程：
    - 自动检测本地存储的密钥
    - 密钥存在则自动登录
    - 密钥不存在则显示登录界面

- **安全增强**
  - 密钥通过 URL 查询参数传递 (`?token=xxx`)
  - 服务端验证密钥，错误密钥返回 401 Unauthorized
  - 密钥存储在 localStorage，不在 URL 或浏览器历史中暴露
  - 连接超时保护（5 秒）

- **配置支持**
  - 配置文件 `channels.web.auth_token` 字段（已存在）
  - 环境变量 `NEMESISBOT_CHANNELS_WEB_AUTH_TOKEN` 支持

- **样式优化**
  - 新增登录界面样式（约 100 行 CSS）
  - 响应式设计，支持移动端访问
  - 渐变背景和动画效果

##### 测试覆盖

- **单元测试** (11 个测试)
  - 服务器配置测试
  - 会话管理器测试
  - Token 提取逻辑测试
  - 服务器生命周期测试
  - WebSocket 消息类型测试

- **集成测试** (12 个测试)
  - 完整服务器生命周期测试（带认证和不带认证）
  - 多服务器实例测试
  - 并发服务器创建测试
  - 会话超时配置测试
  - 上下文取消处理测试
  - 不同配置变体测试

#### 📝 文档 (Documentation)

- 更新 `README.md`，新增 "Web 认证配置" 章节
  - 配置方法说明（JSON 和环境变量）
  - 使用流程说明
  - 安全建议（强密钥、HTTPS、定期更换）

#### 🔧 技术细节

##### 修改的文件

```
module/web/static/
├── index.html          # 添加登录界面和注销按钮
├── app.js              # 实现 AuthManager 和登录流程
└── style.css           # 添加登录界面样式

README.md                # 添加 Web 认证配置说明文档
```

##### 新增的文件

```
test/unit/web/
└── websocket_test.go                    # 11 个单元测试

test/integration/web/
└── websocket_integration_test.go        # 12 个集成测试

docs/CHANGELOG.md                        # 本文件（修改日志）
```

##### 测试结果

```
✅ 单元测试: 11/11 通过
✅ 集成测试: 12/12 通过
总测试数: 23 个测试
```

#### 🎯 使用方式

##### 配置文件方式

```json
{
  "channels": {
    "web": {
      "enabled": true,
      "host": "0.0.0.0",
      "port": 8080,
      "path": "/ws",
      "auth_token": "your-secret-key-here",  // 设置密钥后启用认证
      "session_timeout": 3600
    }
  }
}
```

##### 环境变量方式

```bash
export NEMESISBOT_CHANNELS_WEB_AUTH_TOKEN="your-secret-key-here"
```

#### 🔒 安全说明

- **默认行为**：`auth_token` 为空时不启用认证，任何人都可以访问
- **启用认证**：设置 `auth_token` 后，用户必须输入正确密钥才能访问
- **密钥存储**：密钥存储在浏览器 localStorage 中，关闭浏览器后仍然保留
- **注销功能**：用户可以点击"退出"按钮清除本地密钥

#### 📊 代码统计

- 新增代码：约 800 行（前端 + 测试）
- 修改代码：约 200 行
- 新增测试：23 个测试用例
- 新增文档：1 个修改日志文件

---

## 版本历史

### v0.0.0.1 (2026-02-23)

详见 `README.md` 的"更新日志"章节。

---

## 贡献指南

### 修改日志格式

每次重大改动都应该记录在本文件中，包括：

- **新增** (Added): 新功能
- **变更** (Changed): 现有功能的变更
- **弃用** (Deprecated): 即将移除的功能
- **移除** (Removed): 已移除的功能
- **修复** (Fixed): Bug 修复
- **安全** (Security): 安全相关的改动

### 格式示例

```markdown
### [版本号] (日期)

#### ✨ 新增 (Added)
- 功能描述

#### 🔄 变更 (Changed)
- 变更描述

#### 🐛 修复 (Fixed)
- 修复描述
```

---

## 链接

- [项目主页](https://github.com/276793422/NemesisBot)
- [问题反馈](https://github.com/276793422/NemesisBot/issues)
- [README](../README.md)
