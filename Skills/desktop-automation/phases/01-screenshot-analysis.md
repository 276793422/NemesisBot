# 功能 1: 智能截图解析

对指定窗口进行截图，然后 AI 自己分析截图内容。

## 流程

```
1. window-mcp: find_window(title)
2. window-mcp: set_foreground_window(handle)
3. window-mcp: get_window_rect(handle) → {left, top, right, bottom}
4. window-mcp: capture_rect(...) → 保存到 screenshot.png
5. AI: Read(screenshot.png) → 视觉分析 → 返回报告
```

## 关键点

- ✅ 截图保存为文件
- ✅ AI 使用 Read 工具读取
- ✅ AI 自己分析（视觉识别）
- ❌ 不使用任何 MCP 分析功能

## 示例

```yaml
输入:
  窗口标题: "安全审批"
  分析提示: "列出所有UI元素"

输出:
  窗口位置: (100, 200, 700, 780)
  元素列表:
    - 按钮"允许" (绿色, 右下)
    - 按钮"拒绝" (红色, 左下)
    - 倒计时: 28 秒
```
