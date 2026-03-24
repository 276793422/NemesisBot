//go:build !cross_compile

package windows

import (
	"context"
	"fmt"
	"os"
	stdruntime "runtime"
	"path/filepath"

	"github.com/276793422/NemesisBot/module/desktop/websocket"
	"github.com/wailsapp/wails/v2"
	"github.com/wailsapp/wails/v2/pkg/options"
	"github.com/wailsapp/wails/v2/pkg/options/assetserver"
	wailsruntime "github.com/wailsapp/wails/v2/pkg/runtime"
)

// ApprovalApp 审批窗口 Wails 应用
type ApprovalApp struct {
	window *ApprovalWindow
	ctx    context.Context
	data   *ApprovalWindowData
}

// NewApprovalApp 创建审批应用
func NewApprovalApp(window *ApprovalWindow) *ApprovalApp {
	return &ApprovalApp{
		window: window,
	}
}

// Startup 启动应用
func (a *ApprovalApp) Startup(ctx context.Context) error {
	fmt.Fprintf(os.Stderr, "[ApprovalApp] Startup called\n")
	a.ctx = ctx
	err := a.window.Startup(ctx)
	if err != nil {
		fmt.Fprintf(os.Stderr, "[ApprovalApp] Window startup failed: %v\n", err)
		return err
	}
	fmt.Fprintf(os.Stderr, "[ApprovalApp] Startup completed\n")
	return nil
}

// Shutdown 关闭应用
func (a *ApprovalApp) Shutdown(ctx context.Context) {
	fmt.Fprintf(os.Stderr, "[ApprovalApp] Shutdown called\n")
	a.window.Shutdown(ctx)
}

// DomReady DOM 准备完成
func (a *ApprovalApp) DomReady(ctx context.Context) {
	fmt.Fprintf(os.Stderr, "[ApprovalApp] DomReady called\n")
	// 发送数据到前端
	wailsruntime.EventsEmit(ctx, "init-data", a.data)
	fmt.Fprintf(os.Stderr, "[ApprovalApp] Init data emitted to frontend\n")
}

// Bind 返回绑定
func (a *ApprovalApp) Bind() []interface{} {
	return a.window.Bind()
}

// RunApprovalWindow 运行审批窗口（真正的 Wails 窗口实现）
func RunApprovalWindow(windowID string, data *ApprovalWindowData, wsClient *websocket.WebSocketClient) error {
	fmt.Fprintf(os.Stderr, "[RunApprovalWindow] Starting Wails window: %s\n", windowID)

	// 创建窗口
	window := NewApprovalWindow(windowID, data, wsClient)
	app := NewApprovalApp(window)

	// 设置应用数据
	app.data = data

	// 获取当前文件路径
	_, filename, _, _ := stdruntime.Caller(0)
	dir := filepath.Dir(filename)

	// 使用 index.html (现在是调试版本)
	htmlFile := filepath.Join(dir, "../frontend/index.html")
	fmt.Fprintf(os.Stderr, "[RunApprovalWindow] Using HTML file: %s\n", htmlFile)

	// 检查文件是否存在
	if _, err := os.Stat(htmlFile); os.IsNotExist(err) {
		fmt.Fprintf(os.Stderr, "[RunApprovalWindow] HTML file not found: %s\n", htmlFile)
		return err
	}

	fmt.Fprintf(os.Stderr, "[RunApprovalWindow] Calling wails.Run...\n")
	fmt.Fprintf(os.Stderr, "[RunApprovalWindow] Window ID: %s, Data: %+v\n", windowID, data)

	err := wails.Run(&options.App{
		Title:  "安全审批 - NemesisBot",
		Width:  750,
		Height: 700,
		AssetServer: &assetserver.Options{
			Assets: os.DirFS(filepath.Dir(htmlFile)),
		},
		Bind: []interface{}{
			app,
			&ApprovalBindings{window: window},
		},
		OnStartup: func(ctx context.Context) {
			fmt.Fprintf(os.Stderr, "[RunApprovalWindow] OnStartup: calling app.Startup...\n")
			if err := app.Startup(ctx); err != nil {
				fmt.Fprintf(os.Stderr, "[RunApprovalWindow] OnStartup: app.Startup failed: %v\n", err)
			}
		},
		OnDomReady: func(ctx context.Context) {
			fmt.Fprintf(os.Stderr, "[RunApprovalWindow] OnDomReady: calling app.DomReady...\n")
			app.DomReady(ctx)
		},
		OnShutdown: func(ctx context.Context) {
			fmt.Fprintf(os.Stderr, "[RunApprovalWindow] OnShutdown: calling app.Shutdown...\n")
			app.Shutdown(ctx)
		},
	})

	fmt.Fprintf(os.Stderr, "[RunApprovalWindow] wails.Run returned: err=%v\n", err)

	if err != nil {
		fmt.Fprintf(os.Stderr, "[RunApprovalWindow] Wails error: %v\n", err)
		// Wails 启动失败，发送拒绝结果
		window.SubmitApproval(false, "窗口启动失败: " + err.Error())
		return err
	}

	fmt.Fprintf(os.Stderr, "[RunApprovalWindow] Window completed: %s\n", windowID)
	return nil
}
