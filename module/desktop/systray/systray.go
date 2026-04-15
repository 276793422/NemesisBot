//go:build !cross_compile

package systray

import (
	"fmt"
	"log"
	"os/exec"
	"runtime"
	"sync"

	"github.com/getlantern/systray"
)

// SystemTray 系统托盘
type SystemTray struct {
	mu         sync.RWMutex
	running    bool
	quitCh     chan struct{}
	menuItems  map[string]*systray.MenuItem
	webUIURL   string

	// 回调函数
	onStartFunc     func()
	onStopFunc      func()
	onOpenWebUIFunc func()
	onQuitFunc      func()
}

// NewSystemTray 创建系统托盘实例
func NewSystemTray() *SystemTray {
	return &SystemTray{
		running:   false,
		quitCh:    make(chan struct{}),
		menuItems: make(map[string]*systray.MenuItem),
	}
}

// Run 运行系统托盘（阻塞，应在 goroutine 中调用）
func (s *SystemTray) Run() error {
	s.mu.Lock()
	if s.running {
		s.mu.Unlock()
		return fmt.Errorf("system tray already running")
	}
	s.running = true
	s.mu.Unlock()

	log.Printf("[SysTray] Starting system tray")

	// systray.Run 会阻塞
	systray.Run(s.onReady, s.onExit)

	return nil
}

// Stop 停止系统托盘
func (s *SystemTray) Stop() {
	s.mu.Lock()
	defer s.mu.Unlock()

	if !s.running {
		return
	}

	log.Printf("[SysTray] Stopping system tray")

	s.running = false
	systray.Quit()
}

// onReady 系统托盘就绪回调
func (s *SystemTray) onReady() {
	log.Printf("[SysTray] System tray ready")

	// 设置图标和提示
	systray.SetIcon(getIcon())
	systray.SetTitle("NemesisBot")
	systray.SetTooltip("NemesisBot - AI Agent")

	// 设置菜单
	s.setupMenu()

	log.Printf("[SysTray] Menu setup complete")
}

// onExit 系统托盘退出回调
func (s *SystemTray) onExit() {
	log.Printf("[SysTray] System tray exited")
	close(s.quitCh)
}

// setupMenu 设置右键菜单
func (s *SystemTray) setupMenu() {
	// 服务控制
	mStart := systray.AddMenuItem("启动服务", "启动 Bot 服务")
	mStop := systray.AddMenuItem("停止服务", "停止 Bot 服务")
	systray.AddSeparator()

	// 快捷访问
	mWebUI := systray.AddMenuItem("打开 Web UI", "打开 Web 界面")
	systray.AddSeparator()

	// 信息
	mVersion := systray.AddMenuItem(fmt.Sprintf("NemesisBot (%s)", runtime.GOOS), "版本信息")
	mVersion.Disable()
	systray.AddSeparator()

	// 退出
	mQuit := systray.AddMenuItem("退出", "退出程序")

	// 保存菜单项引用
	s.menuItems["start"] = mStart
	s.menuItems["stop"] = mStop
	s.menuItems["webui"] = mWebUI
	s.menuItems["quit"] = mQuit

	// 启动事件监听
	go s.handleMenuEvents(mStart, mStop, mWebUI, mQuit)
}

// handleMenuEvents 处理菜单点击事件
func (s *SystemTray) handleMenuEvents(
	mStart, mStop, mWebUI, mQuit *systray.MenuItem,
) {
	for {
		select {
		case <-mStart.ClickedCh:
			log.Printf("[SysTray] Menu: Start clicked")
			s.mu.RLock()
			fn := s.onStartFunc
			s.mu.RUnlock()
			if fn != nil {
				go fn()
			}

		case <-mStop.ClickedCh:
			log.Printf("[SysTray] Menu: Stop clicked")
			s.mu.RLock()
			fn := s.onStopFunc
			s.mu.RUnlock()
			if fn != nil {
				go fn()
			}

		case <-mWebUI.ClickedCh:
			log.Printf("[SysTray] Menu: Web UI clicked")
			s.mu.RLock()
			fn := s.onOpenWebUIFunc
			url := s.webUIURL
			s.mu.RUnlock()
			if fn != nil {
				go fn()
			} else {
				if url == "" {
					url = "http://127.0.0.1:49000"
				}
				go s.openBrowser(url)
			}

		case <-mQuit.ClickedCh:
			log.Printf("[SysTray] Menu: Quit clicked")
			s.mu.RLock()
			fn := s.onQuitFunc
			s.mu.RUnlock()
			if fn != nil {
				go fn()
			}

		case <-s.quitCh:
			log.Printf("[SysTray] Menu handler exiting")
			return
		}
	}
}

// SetOnStart 设置启动服务回调
func (s *SystemTray) SetOnStart(fn func()) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.onStartFunc = fn
}

// SetOnStop 设置停止服务回调
func (s *SystemTray) SetOnStop(fn func()) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.onStopFunc = fn
}

// SetOnOpenWebUI 设置打开 Web UI 回调
func (s *SystemTray) SetOnOpenWebUI(fn func()) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.onOpenWebUIFunc = fn
}

// SetOnQuit 设置退出回调
func (s *SystemTray) SetOnQuit(fn func()) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.onQuitFunc = fn
}

// SetWebUIURL 设置 Web UI 地址
func (s *SystemTray) SetWebUIURL(url string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.webUIURL = url
}

// openBrowser 打开浏览器
func (s *SystemTray) openBrowser(url string) {
	var cmd string
	var args []string

	switch runtime.GOOS {
	case "windows":
		cmd = "cmd"
		args = []string{"/c", "start"}
	case "darwin":
		cmd = "open"
	default: // linux
		cmd = "xdg-open"
	}
	args = append(args, url)

	log.Printf("[SysTray] Opening browser: %s", url)

	// Start the browser command
	if err := exec.Command(cmd, args...).Start(); err != nil {
		log.Printf("[SysTray] Failed to open browser: %v", err)
	}
}

// Notify 显示通知（使用系统通知）
func (s *SystemTray) Notify(title, message string) {
	// systray 库本身不提供通知功能
	// 可以使用其他库（如 go-toast）来实现
	log.Printf("[SysTray] Notification: %s - %s", title, message)
}
