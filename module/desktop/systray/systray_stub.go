//go:build cross_compile

package systray

// SystemTray 系统托盘（cross_compile stub）
type SystemTray struct{}

// NewSystemTray 创建系统托盘实例（stub）
func NewSystemTray() *SystemTray {
	return &SystemTray{}
}

// Run 运行系统托盘（stub）
func (s *SystemTray) Run() error {
	// cross_compile 模式不支持系统托盘
	return nil
}

// Stop 停止系统托盘（stub）
func (s *SystemTray) Stop() {}

// SetOnStart 设置启动服务回调（stub）
func (s *SystemTray) SetOnStart(fn func()) {}

// SetOnStop 设置停止服务回调（stub）
func (s *SystemTray) SetOnStop(fn func()) {}

// SetOnOpenWebUI 设置打开 Web UI 回调（stub）
func (s *SystemTray) SetOnOpenWebUI(fn func()) {}

// SetOnQuit 设置退出回调（stub）
func (s *SystemTray) SetOnQuit(fn func()) {}

// SetWebUIURL 设置 Web UI 地址（stub）
func (s *SystemTray) SetWebUIURL(url string) {}

// SetChatURL 设置独立聊天界面地址（stub）
func (s *SystemTray) SetChatURL(url string) {}

// SetProcessManager 设置进程管理器（stub）
func (s *SystemTray) SetProcessManager(pm interface{}) {}

// SetAuthToken 设置认证 Token（stub）
func (s *SystemTray) SetAuthToken(token string) {}

// SetWebPort 设置 Web 端口（stub）
func (s *SystemTray) SetWebPort(port int) {}

// SetWebHost 设置 Web 主机地址（stub）
func (s *SystemTray) SetWebHost(host string) {}

// Notify 显示通知（stub）
func (s *SystemTray) Notify(title, message string) {}
