# 快速入门指南 - 外部通道

## 您已经完成的步骤 ✅

1. ✅ 执行 `onboard default` - 初始化配置
2. ✅ 设置 LLM - 配置 AI 模型

## 下一步：启用外部通道

### 最简单的方式（5 分钟完成）

---

## 步骤 1: 准备测试程序

我们提供了现成的示例程序，位于：
- `examples\external\input.bat` - 输入程序
- `examples\external\output.bat` - 输出程序

**这些文件已经为您准备好了！**

---

## 步骤 2: 使用命令行配置

### 打开命令提示符，执行：

```bash
nemesisbot channel external setup
```

### 配置向导会问您：

#### 问题 1: 输入程序路径
```
Enter path to input executable (or press Enter to skip):
```

**输入**（请根据实际路径调整）：
```
C:\AI\NemesisBot\NemesisBot\examples\external\input.bat
```

#### 问题 2: 输出程序路径
```
Enter path to output executable (or press Enter to skip):
```

**输入**（请根据实际路径调整）：
```
C:\AI\NemesisBot\NemesisBot\examples\external\output.bat
```

#### 问题 3: Chat ID
```
Enter new chat ID (or press Enter to keep current):
```

**直接按 Enter** 使用默认值 `external:main`

#### 问题 4: Web 同步
```
Enable web sync? (Y/n):
```

**输入 `Y`** 或直接按 Enter 启用 Web 同步

---

## 步骤 3: 启用外部通道

```bash
nemesisbot channel enable external
```

应该看到：
```
⚠️  External channel requires additional configuration
   Use: nemesisbot channel external setup
✅ External channel enabled (not configured yet)
```

---

## 步骤 4: 验证配置

```bash
nemesisbot channel external config
```

应该看到：
```
External Channel Configuration
===============================

Enabled:         ✅ Yes
Input EXE:       C:\AI\NemesisBot\NemesisBot\examples\external\input.bat
Output EXE:      C:\AI\NemesisBot\NemesisBot\examples\external\output.bat
Chat ID:         external:main
Sync to Web:     ✅ Yes

✅ External channel is ready to use
```

---

## 步骤 5: 启动网关

```bash
nemesisbot gateway
```

启动后，您应该看到类似的日志：
```
[INFO] channels: Attempting to initialize External channel
[INFO] channels: External channel enabled successfully
[INFO] channels: Starting all channels
[INFO] channels: Starting channel - external
[INFO] external: Starting external channel
[INFO] external: Input EXE started
[INFO] external: Output EXE started
[INFO] external: External channel started successfully
```

---

## 步骤 6: 开始使用

### 方式 A: 通过 Web 界面（推荐）

1. 打开浏览器访问：`http://localhost:8080`
2. 输入消息并发送
3. 您会看到：
   - Web 界面显示对话
   - 输出程序窗口显示 AI 响应

### 方式 B: 通过输入程序

如果您的输入程序支持交互：
1. 在输入程序窗口输入文本
2. NemesisBot 会处理并发送到 LLM
3. 响应会显示在输出程序和 Web 界面

---

## 验证清单

完成以上步骤后，检查以下几点：

- [ ] `nemesisbot channel list` 显示 external 为 ✅ Enabled
- [ ] `nemesisbot channel external config` 显示配置正确
- [ ] `nemesisbot gateway` 启动没有报错
- [ ] 打开 `http://localhost:8080` 可以看到 Web 界面
- [ ] 输入程序窗口可以输入文本
- [ ] 输出程序窗口显示 AI 响应

---

## 常见问题

### Q: 程序无法启动？

**A**: 确保使用绝对路径：
```
C:\AI\NemesisBot\NemesisBot\examples\external\input.bat
```

### Q: Web 界面看不到消息？

**A**: 检查配置：
```bash
nemesisbot channel external config
```

确保 `Sync to Web: ✅ Yes`

### Q: 如何停止？

**A**: 在 `nemesisbot gateway` 窗口按 `Ctrl+C`

---

## 下一步

### 自定义程序

您可以根据需求修改示例程序：

1. **输入程序** (`input.bat`)
   - 添加语音识别
   - 添加格式转换
   - 添加输入验证

2. **输出程序** (`output.bat`)
   - 添加语音播放
   - 添加桌面通知
   - 添加日志记录

### 参考文档

详细文档：[docs/EXTERNAL_CHANNEL_GUIDE.md](EXTERNAL_CHANNEL_GUIDE.md)

---

## 恭喜！🎉

您已经成功启用了 NemesisBot 的外部通道！

现在您可以通过自定义程序与 AI 交互，同时享受 Web 界面的便利。

---

**需要帮助？**

```bash
# 查看帮助
nemesisbot channel external

# 查看配置
nemesisbot channel external config

# 测试程序
nemesisbot channel external test
```
