//go:build !cross_compile

package windows

import (
	"context"
	"fmt"
	"os"

	"github.com/276793422/NemesisBot/module/desktop/websocket"
	wailsruntime "github.com/wailsapp/wails/v2/pkg/runtime"
)

// DashboardWindowData Dashboard 窗口数据
type DashboardWindowData struct {
	Token   string `json:"token"`
	WebPort int    `json:"web_port"`
	WebHost string `json:"web_host"`
}

// Validate 验证数据
func (d *DashboardWindowData) Validate() error {
	if d.Token == "" {
		return fmt.Errorf("token is required")
	}
	if d.WebPort <= 0 {
		return fmt.Errorf("invalid web port: %d", d.WebPort)
	}
	return nil
}

// GetType 获取类型
func (d *DashboardWindowData) GetType() string {
	return "dashboard"
}

// GetTimeout 获取超时时间（Dashboard 无超时）
func (d *DashboardWindowData) GetTimeout() int {
	return 0
}

// DashboardWindow Dashboard 窗口
type DashboardWindow struct {
	*WindowBase
	ctx      context.Context
	data     *DashboardWindowData
	wsClient *websocket.WebSocketClient
}

// NewDashboardWindow 创建 Dashboard 窗口
func NewDashboardWindow(windowID string, data *DashboardWindowData, wsClient *websocket.WebSocketClient) *DashboardWindow {
	base := NewWindowBase(windowID, "dashboard", data, wsClient)

	return &DashboardWindow{
		WindowBase: base,
		data:       data,
		wsClient:   wsClient,
	}
}

// Startup 启动窗口
func (w *DashboardWindow) Startup(ctx context.Context) error {
	w.ctx = ctx
	if err := w.WindowBase.Startup(ctx); err != nil {
		return err
	}

	// 注册父进程命令处理器（新协议 Dispatcher）
	if w.wsClient != nil {
		w.wsClient.RegisterNotificationHandler("window.bring_to_front", func(ctx context.Context, msg *websocket.Message) {
			fmt.Fprintf(os.Stderr, "[DashboardWindow-%s] Received bring_to_front command\n", w.ID)
			if w.ctx != nil {
				wailsruntime.WindowShow(w.ctx)
				wailsruntime.WindowUnminimise(w.ctx)
			}
		})
	}

	fmt.Fprintf(os.Stderr, "[DashboardWindow-%s] Startup: token=%s... web=%s:%d\n",
		w.ID, w.data.Token[:min(8, len(w.data.Token))], w.data.WebHost, w.data.WebPort)

	return nil
}

// Shutdown 关闭窗口
func (w *DashboardWindow) Shutdown(ctx context.Context) {
	fmt.Fprintf(os.Stderr, "[DashboardWindow-%s] Shutdown\n", w.ID)
	w.WindowBase.Shutdown(ctx)
}

// GetData 获取 Dashboard 数据
func (w *DashboardWindow) GetDashboardData() *DashboardWindowData {
	return w.data
}

// Bind 返回绑定结构
func (w *DashboardWindow) Bind() []interface{} {
	baseBindings := w.WindowBase.Bind()
	dashboardBindings := &DashboardBindings{window: w}
	return append(baseBindings, dashboardBindings)
}

// DashboardBindings Dashboard 窗口绑定
type DashboardBindings struct {
	window *DashboardWindow
}

// GetToken 获取 Token
func (b *DashboardBindings) GetToken() string {
	return b.window.data.Token
}

// GetWebPort 获取 Web 端口
func (b *DashboardBindings) GetWebPort() int {
	return b.window.data.WebPort
}

// GetWebHost 获取 Web 主机
func (b *DashboardBindings) GetWebHost() string {
	return b.window.data.WebHost
}

func min(a, b int) int {
	if a < b {
		return a
	}
	return b
}
