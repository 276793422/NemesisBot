//go:build cross_compile

package desktop

import (
	"context"
	"fmt"
)

// Config Desktop UI 配置
type Config struct {
	Enabled bool
	Debug   bool
}

// RunWithServiceManager is a stub for cross-compilation
// Desktop UI requires native compilation with platform-specific libraries
func RunWithServiceManager(cfg *Config, svcMgr interface{}) error {
	return fmt.Errorf("desktop UI is not available in cross-compiled builds")
}

// Run is a stub for cross-compilation
func Run() error {
	return fmt.Errorf("desktop UI is not available in cross-compiled builds")
}

// App is a stub for cross-compilation
type App struct{}

// NewApp is a stub for cross-compilation
func NewApp() *App {
	return &App{}
}

// Startup is a stub for cross-compilation
func (a *App) Startup(ctx context.Context) error {
	return nil
}

// Shutdown is a stub for cross-compilation
func (a *App) Shutdown(ctx context.Context) {
}

// domReady is a stub for cross-compilation
func (a *App) domReady(ctx context.Context) {
}

// bind is a stub for cross-compilation
func (a *App) bind(ctx context.Context, bindings []interface{}) []interface{} {
	return nil
}
