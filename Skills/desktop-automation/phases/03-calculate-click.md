# 功能 3: 代码计算点击

基于窗口布局规律计算元素位置并点击。

## 流程

```
1. window-mcp: find_window(title)
2. window-mcp: set_foreground_window(handle)
3. window-mcp: get_window_rect(handle) → {left, top, right, bottom}
4. 计算: 窗口尺寸 = (width, height)
5. 计算: 元素位置:
   - 如果是百分比: elementX = width × percentX
   - 如果是像素: elementX = givenX
6. 计算: absoluteX = windowLeft + elementX, absoluteY = windowTop + elementY
7. window-mcp: click_at(absoluteX, absoluteY)
```

## 关键点

- ✅ 快速（无需图像分析）
- ✅ 可靠（基于规律）
- ❌ 需要先验知识（布局规律）

## 常用坐标

```javascript
// 窗口中心
{x: 0.5, y: 0.5, usePercentage: true}

// 对话框底部按钮（左/右）
{x: 0.3, y: 0.85, usePercentage: true}  // 左按钮
{x: 0.7, y: 0.85, usePercentage: true}  // 右按钮
```

## 示例

```yaml
输入:
  窗口标题: "安全审批"
  坐标: {x: 0.75, y: 0.85, usePercentage: true}

执行:
  1. 窗口坐标: (100, 200, 700, 780)
  2. 窗口尺寸: 600 × 580
  3. 计算: (600×0.75, 580×0.85) = (450, 493)
  4. 绝对坐标: (550, 693)
  5. 点击 (550, 693)

结果:
  ✓ 点击成功（< 100ms）
  ✓ 窗口关闭
```
