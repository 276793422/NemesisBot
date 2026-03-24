//go:build !cross_compile

package process

import (
	"runtime"
)

// Platform 平台类型
type Platform int

const (
	PlatformWindows Platform = iota
	PlatformLinux
	PlatformmacOS
)

// GetPlatform 获取当前平台
func GetPlatform() Platform {
	switch runtime.GOOS {
	case "windows":
		return PlatformWindows
	case "linux":
		return PlatformLinux
	case "darwin":
		return PlatformmacOS
	default:
		return PlatformLinux // 默认
	}
}

// GetPlatformExecutor 获取平台执行器
func GetPlatformExecutor(config *ExecutorConfig) PlatformExecutor {
	switch GetPlatform() {
	case PlatformWindows:
		return NewWindowsExecutor(config)
	case PlatformLinux, PlatformmacOS:
		// TODO: 实现 UnixExecutor
		return NewWindowsExecutor(config) // 临时使用 Windows 执行器
	default:
		return NewWindowsExecutor(config)
	}
}
