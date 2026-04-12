//go:build !cross_compile

package process

// PlatformExecutor 平台执行器接口
type PlatformExecutor interface {
	// SpawnChild 创建子进程
	SpawnChild(exePath string, args []string) (*ChildProcess, error)

	// TerminateChild 终止子进程
	TerminateChild(child *ChildProcess) error

	// CreatePipes 创建管道
	CreatePipes(child *ChildProcess) error

	// Cleanup 清理资源
	Cleanup(child *ChildProcess) error

	// IsProcessAlive 检查子进程是否仍在运行
	IsProcessAlive(child *ChildProcess) bool
}

// ExecutorConfig 执行器配置
type ExecutorConfig struct {
	HideWindow bool // 是否隐藏窗口
}
