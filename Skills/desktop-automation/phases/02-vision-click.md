# 功能 2: 视觉识别点击

通过 AI 图像识别找到元素并点击。

## 流程

```
1. window-mcp: find_window(title)
2. window-mcp: set_foreground_window(handle)
3. window-mcp: get_window_rect(handle) → {left, top, right, bottom}
4. window-mcp: capture_rect(...) → 保存到 screenshot.png
5. AI: Read(screenshot.png) → 视觉识别 → 返回元素相对坐标 {x, y}
6. 计算: absoluteX = windowLeft + elementX, absoluteY = windowTop + elementY
7. window-mcp: click_at(absoluteX, absoluteY)
```

## 关键点

- ✅ AI 通过 Read 工具读取截图
- ✅ AI 自己视觉分析识别元素
- ❌ 不使用 window-mcp 的 analyze 函数
- ❌ 不使用任何 MCP 分析工具

## 示例

```yaml
输入:
  窗口标题: "安全审批"
  元素描述: "绿色的'允许'按钮"
  提示: "返回按钮中心坐标 {x, y}"

执行:
  1. 截图保存到 screenshot.png
  2. AI 读取并分析 → {x: 450, y: 520}
  3. 窗口坐标: (100, 200)
  4. 绝对坐标: (550, 720)
  5. 点击 (550, 720)

结果:
  ✓ 点击成功
  ✓ 窗口关闭
```
