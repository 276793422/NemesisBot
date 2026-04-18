//go:build !cross_compile

package windows

import (
	"context"
	"fmt"
	"log"
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

	// 注册父进程通知处理器
	if w.wsClient != nil {
		// window.bring_to_front — 将窗口带到前台
		w.wsClient.RegisterNotificationHandler("window.bring_to_front", func(ctx context.Context, msg *websocket.Message) {
			fmt.Fprintf(os.Stderr, "[DashboardWindow-%s] Received bring_to_front\n", w.ID)
			if w.ctx != nil {
				wailsruntime.WindowShow(w.ctx)
				wailsruntime.WindowUnminimise(w.ctx)
			}
		})

		// window.minimize — 最小化窗口
		w.wsClient.RegisterNotificationHandler("window.minimize", func(ctx context.Context, msg *websocket.Message) {
			fmt.Fprintf(os.Stderr, "[DashboardWindow-%s] Received minimize\n", w.ID)
			if w.ctx != nil {
				wailsruntime.WindowMinimise(w.ctx)
			}
		})

		// state.service_status — 服务状态变更通知
		w.wsClient.RegisterNotificationHandler("state.service_status", func(ctx context.Context, msg *websocket.Message) {
			var status map[string]interface{}
			if err := msg.DecodeParams(&status); err != nil {
				log.Printf("[DashboardWindow-%s] Failed to decode service_status: %v", w.ID, err)
				return
			}
			log.Printf("[DashboardWindow-%s] Service status update: %+v", w.ID, status)
			// 前端可通过 Wails 事件系统接收
			wailsruntime.EventsEmit(w.ctx, "state:service_status", status)
		})

		// state.config_changed — 配置变更通知
		w.wsClient.RegisterNotificationHandler("state.config_changed", func(ctx context.Context, msg *websocket.Message) {
			var config map[string]interface{}
			if err := msg.DecodeParams(&config); err != nil {
				log.Printf("[DashboardWindow-%s] Failed to decode config_changed: %v", w.ID, err)
				return
			}
			log.Printf("[DashboardWindow-%s] Config changed: %+v", w.ID, config)
			wailsruntime.EventsEmit(w.ctx, "state:config_changed", config)
		})

		// state.cluster_event — 集群事件通知
		w.wsClient.RegisterNotificationHandler("state.cluster_event", func(ctx context.Context, msg *websocket.Message) {
			var event map[string]interface{}
			if err := msg.DecodeParams(&event); err != nil {
				log.Printf("[DashboardWindow-%s] Failed to decode cluster_event: %v", w.ID, err)
				return
			}
			log.Printf("[DashboardWindow-%s] Cluster event: %+v", w.ID, event)
			wailsruntime.EventsEmit(w.ctx, "state:cluster_event", event)
		})

		// dashboard.notification — 通知推送
		w.wsClient.RegisterNotificationHandler("dashboard.notification", func(ctx context.Context, msg *websocket.Message) {
			var notif map[string]interface{}
			if err := msg.DecodeParams(&notif); err != nil {
				log.Printf("[DashboardWindow-%s] Failed to decode notification: %v", w.ID, err)
				return
			}
			wailsruntime.EventsEmit(w.ctx, "dashboard:notification", notif)
		})

		// system.ping — 健康检查（Respond with pong）
		w.wsClient.RegisterHandler("system.ping", func(ctx context.Context, msg *websocket.Message) (*websocket.Message, error) {
			return websocket.NewResponse(msg.ID, map[string]string{"status": "ok"})
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
