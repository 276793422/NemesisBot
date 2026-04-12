//go:build !cross_compile

package process

import (
	"encoding/json"
	"os"
	"os/exec"
	"syscall"
	"time"
)

// WindowsSpecific Windows 平台特定数据
type WindowsSpecific struct {
	JobObject syscall.Handle
	Done      chan struct{} // 进程退出后关闭，用于 IsProcessAlive 检测
}

// WindowsExecutor Windows 平台执行器
type WindowsExecutor struct {
	config ExecutorConfig
}

// NewWindowsExecutor 创建 Windows 执行器
func NewWindowsExecutor(config *ExecutorConfig) *WindowsExecutor {
	if config == nil {
		config = &ExecutorConfig{HideWindow: true}
	}
	return &WindowsExecutor{config: *config}
}

// SpawnChild 创建子进程
func (e *WindowsExecutor) SpawnChild(exePath string, args []string) (*ChildProcess, error) {
	cmd := exec.Command(exePath, args...)

	// 设置 Windows 特定属性
	cmd.SysProcAttr = &syscall.SysProcAttr{}

	// 检查是否是 GUI 进程（通过 --window-type 参数判断）
	isGUIProcess := false
	for i, arg := range args {
		if arg == "--window-type" && i+1 < len(args) {
			isGUIProcess = true
			break
		}
	}

	// 只有非 GUI 进程才隐藏窗口
	if e.config.HideWindow && !isGUIProcess {
		// CREATE_NO_WINDOW = 0x08000000
		cmd.SysProcAttr.CreationFlags = 0x08000000
	}

	// 创建管道
	stdin, err := cmd.StdinPipe()
	if err != nil {
		return nil, err
	}

	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return nil, err
	}

	stderr, err := cmd.StderrPipe()
	if err != nil {
		return nil, err
	}

	// 启动进程
	if err := cmd.Start(); err != nil {
		return nil, err
	}

	// 启动 goroutine 等待进程退出，关闭 Done channel 通知
	done := make(chan struct{})
	go func() {
		cmd.Process.Wait()
		close(done)
	}()

	return &ChildProcess{
		Cmd:       cmd,
		PID:       cmd.Process.Pid,
		Stdin:     &WriteCloser{Encoder: json.NewEncoder(stdin), writer: stdin.(*os.File)},
		Stdout:    &ReadCloser{Decoder: json.NewDecoder(stdout), reader: stdout.(*os.File)},
		Stderr:    &ReadCloser{Decoder: json.NewDecoder(stderr), reader: stderr.(*os.File)},
		Platform:  &WindowsSpecific{JobObject: 0, Done: done},
		CreatedAt: time.Now(),
	}, nil
}

// TerminateChild 终止子进程
func (e *WindowsExecutor) TerminateChild(child *ChildProcess) error {
	if child.Cmd.Process == nil {
		return nil
	}

	// 尝试优雅退出
	if err := child.Cmd.Process.Signal(syscall.SIGTERM); err != nil {
		// 如果优雅退出失败，强制终止
		return child.Cmd.Process.Kill()
	}

	// 等待进程退出（通过 SpawnChild 启动的 Wait goroutine）
	spec, ok := child.Platform.(*WindowsSpecific)
	if !ok || spec.Done == nil {
		return nil
	}

	select {
	case <-spec.Done:
		return nil
	case <-time.After(5 * time.Second):
		// 超时后强制终止
		return child.Cmd.Process.Kill()
	}
}

// CreatePipes 创建管道（已在 SpawnChild 中完成）
func (e *WindowsExecutor) CreatePipes(child *ChildProcess) error {
	// 管道已在 SpawnChild 中创建
	return nil
}

// Cleanup 清理资源
func (e *WindowsExecutor) Cleanup(child *ChildProcess) error {
	// 关闭管道
	if child.Stdin != nil {
		child.Stdin.Close()
	}
	if child.Stdout != nil {
		child.Stdout.Close()
	}
	if child.Stderr != nil {
		child.Stderr.Close()
	}

	// 清理 Windows 特定资源
	if spec, ok := child.Platform.(*WindowsSpecific); ok {
		if spec.JobObject != 0 {
			syscall.CloseHandle(spec.JobObject)
		}
	}

	return nil
}

// IsProcessAlive 检查子进程是否仍在运行
func (e *WindowsExecutor) IsProcessAlive(child *ChildProcess) bool {
	spec, ok := child.Platform.(*WindowsSpecific)
	if !ok || spec.Done == nil {
		return false
	}
	select {
	case <-spec.Done:
		return false
	default:
		return true
	}
}

// Windows API 常量
const (
	CREATE_NO_WINDOW = 0x08000000
)

// Windows API 函数
var (
	modkernel32                  = syscall.NewLazyDLL("kernel32.dll")
	procCreateJobObject          = modkernel32.NewProc("CreateJobObject")
	procAssignProcessToJobObject = modkernel32.NewProc("AssignProcessToJobObject")
)

// createJobObject 创建作业对象（用于进程组管理）
func createJobObject(pid int) (syscall.Handle, error) {
	// 简化实现，返回空句柄
	// 完整实现需要调用 CreateJobObject 和 AssignProcessToJobObject
	return 0, nil
}
