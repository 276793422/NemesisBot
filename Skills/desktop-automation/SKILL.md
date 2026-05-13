# Window Screenshot Skill

**MCP 服务器**: window-mcp 

自动查找浏览器窗口并截取屏幕保存为图片。

## 功能

1. 枚举系统中的所有可见窗口
2. 自动识别浏览器窗口（Chrome/Edge/Firefox）
3. 截取指定窗口区域并保存为 JPEG 图片

## 使用方法

### 步骤 1: 枚举窗口查找浏览器

使用 **window-mcp** 的 `enumerate_windows` 工具查找浏览器窗口：

```json
{
  "name": "enumerate_windows",
  "arguments": {
    "filter_visible": true,
    "title_contains": "Edge"
  }
}
```

- `filter_visible`: 只返回可见窗口（建议设为 true）
- `title_contains`: 可选，按窗口标题过滤（如 "Edge"、"Chrome"、"Firefox"）
- `class_contains`: 可选，按窗口类名过滤

### 步骤 2: 解析窗口信息

从返回结果中获取窗口位置信息：

```json
{
  "windows": [
    {
      "hwnd": "HWND(0x2520268)",
      "title": "网页标题 - Microsoft Edge",
      "class_name": "Chrome_WidgetWin_1",
      "rect": {
        "left": 1305,
        "top": 43,
        "width": 1009,
        "height": 859
      }
    }
  ]
}
```

浏览器类名特征：
- Chrome/Edge: `Chrome_WidgetWin_1`
- Firefox: `MozillaWindowClass`

### 步骤 3: 截图保存

使用 **window-mcp** 的 `capture_screenshot_to_file` 保存截图：

```json
{
  "name": "capture_screenshot_to_file",
  "arguments": {
    "file_path": "C:\\Code\\PPT\\screenshot.jpg",
    "x": 1305,
    "y": 43,
    "width": 1009,
    "height": 859
  }
}
```

参数说明：
- `file_path`: 保存路径（必须使用 `.jpg` 或 `.jpeg` 扩展名）
- `x`, `y`: 窗口左上角坐标
- `width`, `height`: 窗口宽高

## 完整工作流程示例

```
用户需求: 截取浏览器窗口保存到 C:\Code\PPT\temp2.jpg

1. 调用 window-mcp::enumerate_windows 查找浏览器
   → 获取窗口位置: x=1305, y=43, width=1009, height=859

2. 调用 window-mcp::capture_screenshot_to_file 截图
   → 保存到 C:\Code\PPT\temp2.jpg

3. 验证文件生成成功
```

## MCP 服务器信息

- **服务器名称**: window-mcp
- **可执行文件**: `Skills\desktop-automation\window-mcp.exe`
- **主要工具**:
  - `enumerate_windows` - 枚举系统窗口
  - `capture_screenshot_to_file` - 截图保存到文件

## 注意事项

1. 输出路径必须使用 `.jpg` 或 `.jpeg` 扩展名
2. 坐标系为屏幕绝对坐标
3. 如果找不到浏览器窗口，可使用第一个可见窗口作为备选
4. 文件保存格式为 JPEG，文件大小取决于截图内容复杂度
