//go:build darwin || linux

package approval

import (
	"fmt"
	"github.com/webview/webview"
)

// UnixAdapter Unix 平台的 WebView 适配器
//
// 使用系统默认的 WebView 引擎：
//   - macOS: WKWebView
//   - Linux: WebKitGTK
type UnixAdapter struct {
	w webview.WebView
}

// newPlatformAdapter 创建 Unix 适配器
//
// 返回 Unix 适配器实例
func newPlatformAdapter() WebViewAdapter {
	return &UnixAdapter{}
}

// Create 创建 WebView 窗口
func (a *UnixAdapter) Create(title string, width, height int) error {
	if width <= 0 || height <= 0 {
		return fmt.Errorf("invalid window size: %dx%d", width, height)
	}

	a.w = webview.New(webview.Settings{
		Title:  title,
		Width:  width,
		Height: height,
	})

	if a.w == nil {
		return fmt.Errorf("failed to create webview")
	}

	return nil
}

// Destroy 销毁 WebView
func (a *UnixAdapter) Destroy() {
	if a.w != nil {
		a.w.Destroy()
		a.w = nil
	}
}

// SetHTML 设置 HTML 内容
func (a *UnixAdapter) SetHTML(html string) error {
	if a.w == nil {
		return fmt.Errorf("webview not created")
	}
	a.w.SetHTML(html)
	return nil
}

// Eval 执行 JavaScript 代码
func (a *UnixAdapter) Eval(js string) error {
	if a.w == nil {
		return fmt.Errorf("webview not created")
	}
	a.w.Eval(js)
	return nil
}

// Bind 绑定 Go 函数到 JavaScript
func (a *UnixAdapter) Bind(name string, fn interface{}) error {
	if a.w == nil {
		return fmt.Errorf("webview not created")
	}
	a.w.Bind(name, fn)
	return nil
}

// Run 启动 WebView（阻塞）
func (a *UnixAdapter) Run() {
	if a.w != nil {
		a.w.Run()
	}
}

// Terminate 终止 WebView（非阻塞）
func (a *UnixAdapter) Terminate() {
	if a.w != nil {
		a.w.Terminate()
	}
}
