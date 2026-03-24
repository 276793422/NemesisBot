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

// SetOnShow 设置显示回调（stub）
func (s *SystemTray) SetOnShow(fn func()) {}

// SetOnHide 设置隐藏回调（stub）
func (s *SystemTray) SetOnHide(fn func()) {}

// SetOnStart 设置启动服务回调（stub）
func (s *SystemTray) SetOnStart(fn func()) {}

// SetOnStop 设置停止服务回调（stub）
func (s *SystemTray) SetOnStop(fn func()) {}

// SetOnOpenWebUI 设置打开 Web UI 回调（stub）
func (s *SystemTray) SetOnOpenWebUI(fn func()) {}

// SetOnQuit 设置退出回调（stub）
func (s *SystemTray) SetOnQuit(fn func()) {}

// Notify 显示通知（stub）
func (s *SystemTray) Notify(title, message string) {}
