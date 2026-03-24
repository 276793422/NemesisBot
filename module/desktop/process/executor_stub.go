//go:build cross_compile

package process

import (
	"fmt"
	"time"
)

// CrossCompileChildProcess 跨平台编译的子进程（存根）
type CrossCompileChildProcess struct {
	ID       string
	PID      int
	CreateAt time.Time
}

// CrossCompileExecutor 跨平台编译执行器（存根）
type CrossCompileExecutor struct{}

// NewCrossCompileExecutor 创建跨平台编译执行器
func NewCrossCompileExecutor() *CrossCompileExecutor {
	return &CrossCompileExecutor{}
}

// SpawnChild 创建子进程（跨平台编译时不可用）
func (e *CrossCompileExecutor) SpawnChild(exePath string, args []string) (*ChildProcess, error) {
	return nil, fmt.Errorf("process creation not available in cross-compiled builds")
}

// TerminateChild 终止子进程
func (e *CrossCompileExecutor) TerminateChild(child *ChildProcess) error {
	return fmt.Errorf("process termination not available in cross-compiled builds")
}

// CreatePipes 创建管道
func (e *CrossCompileExecutor) CreatePipes(child *ChildProcess) error {
	return fmt.Errorf("pipe creation not available in cross-compiled builds")
}

// Cleanup 清理资源
func (e *CrossCompileExecutor) Cleanup(child *ChildProcess) error {
	return nil
}
