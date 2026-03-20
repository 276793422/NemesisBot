//go:build windows

package approval

import (
	"fmt"

	webview "github.com/shmspace/webview2"
)

// WindowsAdapter Windows 平台的 WebView 适配器
//
// 使用 Microsoft Edge WebView2 (基于 Chromium) 来渲染网页内容
// 提供最佳的性能和最新的 Web 标准支持
type WindowsAdapter struct {
	w webview.WebView
}

// newPlatformAdapter 创建 Windows 适配器
//
// 返回 Windows 适配器实例
func newPlatformAdapter() WebViewAdapter {
	return &WindowsAdapter{}
}

// Create 创建 WebView2 窗口
func (a *WindowsAdapter) Create(title string, width, height int) error {
	if width <= 0 || height <= 0 {
		return fmt.Errorf("invalid window size: %dx%d", width, height)
	}

	a.w = webview.NewWithOptions(webview.WebViewOptions{
		Debug:     false,
		AutoFocus: true,
		WindowOptions: webview.WindowOptions{
			Title:  title,
			Width:  uint(width),
			Height: uint(height),
			Center: true,
		},
	})

	if a.w == nil {
		return fmt.Errorf("failed to create webview2 window (WebView2 Runtime may not be installed)")
	}

	return nil
}

// Destroy 销毁 WebView2 窗口
func (a *WindowsAdapter) Destroy() {
	if a.w != nil {
		a.w.Destroy()
		a.w = nil
	}
}

// SetHTML 设置 HTML 内容
//
// 注意：webview2 使用 SetHtml (小写 h)
func (a *WindowsAdapter) SetHTML(html string) error {
	if a.w == nil {
		return fmt.Errorf("webview not created")
	}
	a.w.SetHtml(html)
	return nil
}

// Eval 执行 JavaScript 代码
func (a *WindowsAdapter) Eval(js string) error {
	if a.w == nil {
		return fmt.Errorf("webview not created")
	}
	a.w.Eval(js)
	return nil
}

// Bind 绑定 Go 函数到 JavaScript
func (a *WindowsAdapter) Bind(name string, fn interface{}) error {
	if a.w == nil {
		return fmt.Errorf("webview not created")
	}
	a.w.Bind(name, fn)
	return nil
}

// Run 启动 WebView2（阻塞）
func (a *WindowsAdapter) Run() {
	if a.w != nil {
		a.w.Run()
	}
}

// Terminate 终止 WebView2（非阻塞）
func (a *WindowsAdapter) Terminate() {
	if a.w != nil {
		a.w.Terminate()
	}
}
