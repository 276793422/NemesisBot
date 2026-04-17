//go:build !cross_compile

package systray

import (
	"fmt"
	"log"
	"os/exec"
	"runtime"
	"sync"
	"time"

	"github.com/276793422/NemesisBot/module/desktop/process"
	"fyne.io/systray"
)

// SystemTray 系统托盘
type SystemTray struct {
	mu         sync.RWMutex
	running    bool
	quitCh     chan struct{}
	menuItems  map[string]*systray.MenuItem
	webUIURL   string
	chatURL    string

	// Dashboard 子进程支持
	procMgr   *process.ProcessManager
	authToken string
	webPort   int
	webHost   string

	// 回调函数
	onStartFunc     func()
	onStopFunc      func()
	onOpenWebUIFunc func()
	onQuitFunc      func()

	// 左键双击检测
	leftClickMu    sync.Mutex
	leftClickTimer *time.Timer
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

	// 左键单击无操作，左键双击打开 WebUI
	systray.SetOnTapped(s.handleLeftClick)

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
	mWebUI := systray.AddMenuItem("打开 Dashboard", "打开管理面板（子进程模式）")
	mChat := systray.AddMenuItem("打开聊天", "打开独立聊天界面")
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
	s.menuItems["chat"] = mChat
	s.menuItems["quit"] = mQuit

	// 启动事件监听
	go s.handleMenuEvents(mStart, mStop, mWebUI, mChat, mQuit)
}

// handleLeftClick 处理左键单击（双击检测：单击无操作，双击打开 WebUI）
func (s *SystemTray) handleLeftClick() {
	s.leftClickMu.Lock()
	defer s.leftClickMu.Unlock()

	if s.leftClickTimer != nil {
		// 定时器内第二次点击 → 双击
		s.leftClickTimer.Stop()
		s.leftClickTimer = nil
		log.Printf("[SysTray] Left double click: opening WebUI")
		go func() {
			defer func() {
				if r := recover(); r != nil {
					log.Printf("[SysTray] Recovered from panic in double click handler: %v", r)
				}
			}()
			s.openWebUI()
		}()
	} else {
		// 第一次点击 → 启动定时器，超时则忽略
		s.leftClickTimer = time.AfterFunc(400*time.Millisecond, func() {
			s.leftClickMu.Lock()
			s.leftClickTimer = nil
			s.leftClickMu.Unlock()
		})
	}
}

// openWebUI 打开 WebUI（供左键单击和菜单项共用）
func (s *SystemTray) openWebUI() {
	s.mu.RLock()
	fn := s.onOpenWebUIFunc
	url := s.webUIURL
	procMgr := s.procMgr
	token := s.authToken
	webPort := s.webPort
	webHost := s.webHost
	s.mu.RUnlock()

	if fn != nil {
		fn()
	} else if procMgr != nil {
		data := map[string]interface{}{
			"token":    token,
			"web_port": webPort,
			"web_host": webHost,
		}
		childID, _, err := procMgr.SpawnChild("dashboard", data)
		if err != nil {
			log.Printf("[SysTray] Failed to spawn Dashboard: %v", err)
		} else {
			log.Printf("[SysTray] Dashboard spawned: %s", childID)
		}
	} else {
		if url == "" {
			url = "http://127.0.0.1:49000"
		}
		s.openBrowser(url)
	}
}

// handleMenuEvents 处理菜单点击事件
func (s *SystemTray) handleMenuEvents(
	mStart, mStop, mWebUI, mChat, mQuit *systray.MenuItem,
) {
	for {
		select {
		case <-mStart.ClickedCh:
			log.Printf("[SysTray] Menu: Start clicked")
			s.mu.RLock()
			fn := s.onStartFunc
			s.mu.RUnlock()
			if fn != nil {
				go func() {
					defer func() {
						if r := recover(); r != nil {
							log.Printf("[SysTray] Recovered from panic in Start handler: %v", r)
						}
					}()
					fn()
				}()
			}

		case <-mStop.ClickedCh:
			log.Printf("[SysTray] Menu: Stop clicked")
			s.mu.RLock()
			fn := s.onStopFunc
			s.mu.RUnlock()
			if fn != nil {
				go func() {
					defer func() {
						if r := recover(); r != nil {
							log.Printf("[SysTray] Recovered from panic in Stop handler: %v", r)
						}
					}()
					fn()
				}()
			}

		case <-mWebUI.ClickedCh:
			log.Printf("[SysTray] Menu: Dashboard clicked")
			go func() {
				defer func() {
					if r := recover(); r != nil {
						log.Printf("[SysTray] Recovered from panic in Dashboard handler: %v", r)
					}
				}()
				s.openWebUI()
			}()

		case <-mChat.ClickedCh:
			log.Printf("[SysTray] Menu: Chat clicked")
			s.mu.RLock()
			url := s.chatURL
			s.mu.RUnlock()
			if url == "" {
				url = "http://127.0.0.1:49000/chat/"
			}
			go func() {
				defer func() {
					if r := recover(); r != nil {
						log.Printf("[SysTray] Recovered from panic in Chat browser open: %v", r)
					}
				}()
				s.openBrowser(url)
			}()

		case <-mQuit.ClickedCh:
			log.Printf("[SysTray] Menu: Quit clicked")
			s.mu.RLock()
			fn := s.onQuitFunc
			s.mu.RUnlock()
			if fn != nil {
				go func() {
					defer func() {
						if r := recover(); r != nil {
							log.Printf("[SysTray] Recovered from panic in Quit handler: %v", r)
						}
					}()
					fn()
				}()
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

// SetChatURL 设置独立聊天界面地址
func (s *SystemTray) SetChatURL(url string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.chatURL = url
}

// SetProcessManager 设置进程管理器（用于 Dashboard 子进程启动）
func (s *SystemTray) SetProcessManager(pm *process.ProcessManager) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.procMgr = pm
}

// SetAuthToken 设置认证 Token
func (s *SystemTray) SetAuthToken(token string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.authToken = token
}

// SetWebPort 设置 Web 端口
func (s *SystemTray) SetWebPort(port int) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.webPort = port
}

// SetWebHost 设置 Web 主机地址
func (s *SystemTray) SetWebHost(host string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.webHost = host
}

// openBrowser 打开浏览器
func (s *SystemTray) openBrowser(url string) {
	var cmd string
	var args []string

	switch runtime.GOOS {
	case "windows":
		cmd = "powershell"
		args = []string{"-WindowStyle", "Hidden", "-Command", "Start-Process", url}
	case "darwin":
		cmd = "open"
		args = []string{url}
	default: // linux
		cmd = "xdg-open"
		args = []string{url}
	}

	log.Printf("[SysTray] Opening browser: %s", url)

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
