package approval

// WebViewAdapter WebView 适配器接口，屏蔽平台差异
//
// 这个接口定义了所有平台必须实现的方法，使得上层代码可以使用统一的 API
// 而不需要关心底层是使用 webview2 (Windows) 还是 webview (Unix)。
type WebViewAdapter interface {
	// Create 创建 WebView 窗口
	// 参数:
	//   - title: 窗口标题
	//   - width: 窗口宽度（像素）
	//   - height: 窗口高度（像素）
	// 返回:
	//   - error: 创建失败时返回错误
	Create(title string, width, height int) error

	// Destroy 销毁 WebView 窗口并释放资源
	Destroy()

	// SetHTML 设置窗口的 HTML 内容
	// 参数:
	//   - html: HTML 内容字符串
	// 返回:
	//   - error: 设置失败时返回错误
	SetHTML(html string) error

	// Eval 执行 JavaScript 代码
	// 参数:
	//   - js: JavaScript 代码字符串
	// 返回:
	//   - error: 执行失败时返回错误
	Eval(js string) error

	// Bind 绑定 Go 函数到 JavaScript，使 JavaScript 可以调用 Go 代码
	// 参数:
	//   - name: JavaScript 中调用的函数名
	//   - fn: Go 函数，可以是任意签名的函数
	// 返回:
	//   - error: 绑定失败时返回错误
	Bind(name string, fn interface{}) error

	// Run 启动 WebView（阻塞直到窗口关闭）
	// 这个方法会阻塞当前 goroutine，直到用户关闭窗口
	Run()

	// Terminate 终止 WebView（非阻塞）
	// 立即关闭窗口，不等待用户操作
	Terminate()
}

// NewWebViewAdapter 创建平台特定的适配器
//
// 这个函数会根据编译时的平台标签自动选择正确的实现：
//   - Windows: 返回 WindowsAdapter (使用 webview2)
//   - macOS/Linux: 返回 UnixAdapter (使用 webview)
//
// 返回:
//   - WebViewAdapter: 平台特定的适配器实例
func NewWebViewAdapter() WebViewAdapter {
	return newPlatformAdapter()
}
