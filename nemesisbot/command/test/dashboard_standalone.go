//go:build !cross_compile

package test

import (
	"context"
	"fmt"
	"net/http"
	"os"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/desktop/windows"
	"github.com/276793422/NemesisBot/module/web"
)

// CmdDashboardStandalone 启动独立 Dashboard 窗口（无需 ProcessManager/子进程）
// 用法: nemesisbot test dashboard-standalone [--token TOKEN] [--port PORT] [--host HOST]
func CmdDashboardStandalone() {
	fmt.Println("=== Dashboard Standalone Test ===")

	// 默认参数
	token := "276793422"
	port := 49000
	host := "127.0.0.1"

	// 从命令行参数读取（跳过 "nemesisbot" "test" "dashboard-standalone"）
	args := os.Args
	for i := 3; i < len(args); i++ {
		switch args[i] {
		case "--token":
			if i+1 < len(args) {
				token = args[i+1]
				i++
			}
		case "--port":
			if i+1 < len(args) {
				fmt.Sscanf(args[i+1], "%d", &port)
				i++
			}
		case "--host":
			if i+1 < len(args) {
				host = args[i+1]
				i++
			}
		}
	}

	fmt.Printf("参数: host=%s port=%d token=%s\n", host, port, token)

	// 1. 创建 minimal web server 依赖
	msgBus := bus.NewMessageBus()
	sessionMgr := web.NewSessionManager(30 * time.Minute)

	server := web.NewServer(web.ServerConfig{
		Host:       host,
		Port:       port,
		WSPath:     "/ws",
		AuthToken:  token,
		SessionMgr: sessionMgr,
		Bus:        msgBus,
		Version:    "standalone-test",
	})

	// 2. 在 goroutine 中启动 server
	serverCtx, serverCancel := context.WithCancel(context.Background())
	serverErrCh := make(chan error, 1)
	go func() {
		if err := server.Start(serverCtx); err != nil {
			serverErrCh <- err
		}
	}()

	// 3. Health check 等待 server 就绪
	healthURL := fmt.Sprintf("http://%s:%d/health", host, port)
	fmt.Printf("等待 server 就绪 (%s)...\n", healthURL)

	if !waitForServer(healthURL, 10*time.Second) {
		fmt.Println("Server 未能在超时时间内就绪")
		serverCancel()
		os.Exit(1)
	}

	fmt.Println("Server 已就绪")

	// 4. 直接启动 Dashboard 窗口（无 ProcessManager、无子进程）
	data := &windows.DashboardWindowData{
		Token:   token,
		WebPort: port,
		WebHost: host,
	}

	fmt.Println("正在启动 Dashboard 窗口...")
	err := windows.RunDashboardWindow("standalone-test", data, nil)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Dashboard 窗口错误: %v\n", err)
	}

	// 5. 清理
	fmt.Println("Dashboard 窗口已关闭，正在停止 server...")
	serverCancel()
	msgBus.Close()
	sessionMgr.Shutdown()

	select {
	case err := <-serverErrCh:
		if err != nil {
			fmt.Fprintf(os.Stderr, "Server 关闭警告: %v\n", err)
		}
	default:
	}

	fmt.Println("=== 测试完成 ===")
}

// waitForServer 轮询 health check 直到 server 就绪或超时
func waitForServer(healthURL string, timeout time.Duration) bool {
	deadline := time.Now().Add(timeout)
	client := &http.Client{Timeout: 2 * time.Second}

	for time.Now().Before(deadline) {
		resp, err := client.Get(healthURL)
		if err == nil {
			resp.Body.Close()
			if resp.StatusCode == http.StatusOK {
				return true
			}
		}
		time.Sleep(200 * time.Millisecond)
	}
	return false
}
