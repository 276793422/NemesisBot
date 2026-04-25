package systray

import (
	"testing"
)

func TestNewSystemTray(t *testing.T) {
	s := NewSystemTray()
	if s == nil {
		t.Fatal("NewSystemTray returned nil")
	}
}

func TestSystemTray_SetOnStart(t *testing.T) {
	s := NewSystemTray()
	called := false
	s.SetOnStart(func() { called = true })
	if called {
		t.Error("Callback should not be called on Set")
	}
}

func TestSystemTray_SetOnStop(t *testing.T) {
	s := NewSystemTray()
	s.SetOnStop(func() {})
}

func TestSystemTray_SetOnOpenWebUI(t *testing.T) {
	s := NewSystemTray()
	s.SetOnOpenWebUI(func() {})
}

func TestSystemTray_SetOnQuit(t *testing.T) {
	s := NewSystemTray()
	s.SetOnQuit(func() {})
}

func TestSystemTray_SetWebUIURL(t *testing.T) {
	s := NewSystemTray()
	s.SetWebUIURL("http://localhost:8080")
}

func TestSystemTray_SetChatURL(t *testing.T) {
	s := NewSystemTray()
	s.SetChatURL("http://localhost:49000/chat/")
}

func TestSystemTray_SetAuthToken(t *testing.T) {
	s := NewSystemTray()
	s.SetAuthToken("test-token")
}

func TestSystemTray_SetWebPort(t *testing.T) {
	s := NewSystemTray()
	s.SetWebPort(8080)
}

func TestSystemTray_SetWebHost(t *testing.T) {
	s := NewSystemTray()
	s.SetWebHost("localhost")
}

func TestSystemTray_SetProcessManager_Nil(t *testing.T) {
	s := NewSystemTray()
	s.SetProcessManager(nil)
}

func TestSystemTray_Notify(t *testing.T) {
	s := NewSystemTray()
	s.Notify("Test Title", "Test Message")
}

func TestSystemTray_Run_AlreadyRunning(t *testing.T) {
	s := NewSystemTray()
	s.running = true
	err := s.Run()
	if err == nil {
		t.Error("Expected error when already running")
	}
}

func TestSystemTray_Stop_NotRunning(t *testing.T) {
	s := NewSystemTray()
	// Should not panic when not running
	s.Stop()
}

func TestSystemTray_ConcurrentSetters(t *testing.T) {
	s := NewSystemTray()

	// Test that setters are goroutine-safe
	done := make(chan bool)
	for i := 0; i < 10; i++ {
		go func(n int) {
			s.SetOnStart(func() {})
			s.SetOnStop(func() {})
			s.SetOnQuit(func() {})
			s.SetWebUIURL("url")
			s.SetChatURL("url")
			s.SetAuthToken("token")
			s.SetWebPort(n)
			s.SetWebHost("host")
			done <- true
		}(i)
	}

	for i := 0; i < 10; i++ {
		<-done
	}
}
