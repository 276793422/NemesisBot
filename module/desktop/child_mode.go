//go:build !cross_compile

package desktop

import (
	"fmt"

	"github.com/276793422/NemesisBot/module/desktop/process"
)

var (
	// childModeHandler 子进程模式处理器
	childModeHandler func() error
)

// init 初始化
func init() {
	// 注册子进程模式处理器
	childModeHandler = process.RunChildMode
}

// RunChildMode 运行子进程模式（公开接口）
func RunChildMode() error {
	// 不要使用 log.Printf，会干扰管道通信

	if childModeHandler == nil {
		return fmt.Errorf("child mode handler not initialized")
	}

	return childModeHandler()
}

// HasChildModeFlag 检查是否有 --multiple 参数
func HasChildModeFlag() bool {
	return process.HasChildModeFlag()
}
