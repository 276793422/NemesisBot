//go:build !cross_compile

package windows

import (
	"errors"
	"testing"
)

// --- DashboardWindowData tests ---

func TestDashboardWindowData_Validate_Valid(t *testing.T) {
	data := &DashboardWindowData{
		Token:   "test-token",
		WebPort: 8080,
		WebHost: "127.0.0.1",
	}
	if err := data.Validate(); err != nil {
		t.Errorf("Validate() should succeed for valid data: %v", err)
	}
}

func TestDashboardWindowData_Validate_MissingToken(t *testing.T) {
	data := &DashboardWindowData{
		WebPort: 8080,
		WebHost: "127.0.0.1",
	}
	if err := data.Validate(); err == nil {
		t.Error("Expected error for missing token")
	}
}

func TestDashboardWindowData_Validate_InvalidPort(t *testing.T) {
	data := &DashboardWindowData{
		Token:   "test-token",
		WebPort: 0,
		WebHost: "127.0.0.1",
	}
	if err := data.Validate(); err == nil {
		t.Error("Expected error for invalid port")
	}
}

func TestDashboardWindowData_Validate_NegativePort(t *testing.T) {
	data := &DashboardWindowData{
		Token:   "test-token",
		WebPort: -1,
		WebHost: "127.0.0.1",
	}
	if err := data.Validate(); err == nil {
		t.Error("Expected error for negative port")
	}
}

func TestDashboardWindowData_GetType(t *testing.T) {
	data := &DashboardWindowData{}
	if data.GetType() != "dashboard" {
		t.Errorf("GetType() = %q, want 'dashboard'", data.GetType())
	}
}

func TestDashboardWindowData_GetTimeout(t *testing.T) {
	data := &DashboardWindowData{}
	if data.GetTimeout() != 0 {
		t.Errorf("GetTimeout() = %d, want 0 (dashboard has no timeout)", data.GetTimeout())
	}
}

// --- DashboardWindow tests ---

func TestNewDashboardWindow(t *testing.T) {
	data := &DashboardWindowData{
		Token:   "test-token",
		WebPort: 8080,
		WebHost: "127.0.0.1",
	}
	wsClient := newTestWSClient()

	window := NewDashboardWindow("win-dash-1", data, wsClient)
	if window == nil {
		t.Fatal("NewDashboardWindow returned nil")
	}
	if window.ID != "win-dash-1" {
		t.Errorf("ID = %q, want 'win-dash-1'", window.ID)
	}
	if window.Type != "dashboard" {
		t.Errorf("Type = %q, want 'dashboard'", window.Type)
	}
	if window.data != data {
		t.Error("Data should be set")
	}
	if window.wsClient != wsClient {
		t.Error("WSClient should be set")
	}
}

func TestDashboardWindow_GetDashboardData(t *testing.T) {
	data := &DashboardWindowData{
		Token:   "abc123",
		WebPort: 9090,
		WebHost: "localhost",
	}
	wsClient := newTestWSClient()

	window := NewDashboardWindow("win-1", data, wsClient)
	retrieved := window.GetDashboardData()
	if retrieved != data {
		t.Error("GetDashboardData should return the same data instance")
	}
	if retrieved.Token != "abc123" {
		t.Errorf("Token = %q, want 'abc123'", retrieved.Token)
	}
}

func TestDashboardWindow_Bind(t *testing.T) {
	data := &DashboardWindowData{
		Token:   "test",
		WebPort: 8080,
		WebHost: "localhost",
	}
	wsClient := newTestWSClient()

	window := NewDashboardWindow("win-1", data, wsClient)
	bindings := window.Bind()
	if len(bindings) < 2 {
		t.Errorf("Expected at least 2 bindings (base + dashboard), got %d", len(bindings))
	}
}

// --- DashboardBindings tests ---

func TestDashboardBindings_GetToken(t *testing.T) {
	data := &DashboardWindowData{
		Token:   "my-secret-token",
		WebPort: 8080,
		WebHost: "localhost",
	}
	wsClient := newTestWSClient()
	window := NewDashboardWindow("win-1", data, wsClient)
	bindings := &DashboardBindings{window: window}

	if bindings.GetToken() != "my-secret-token" {
		t.Errorf("GetToken() = %q, want 'my-secret-token'", bindings.GetToken())
	}
}

func TestDashboardBindings_GetWebPort(t *testing.T) {
	data := &DashboardWindowData{
		Token:   "token",
		WebPort: 49000,
		WebHost: "localhost",
	}
	wsClient := newTestWSClient()
	window := NewDashboardWindow("win-1", data, wsClient)
	bindings := &DashboardBindings{window: window}

	if bindings.GetWebPort() != 49000 {
		t.Errorf("GetWebPort() = %d, want 49000", bindings.GetWebPort())
	}
}

func TestDashboardBindings_GetWebHost(t *testing.T) {
	data := &DashboardWindowData{
		Token:   "token",
		WebPort: 8080,
		WebHost: "192.168.1.100",
	}
	wsClient := newTestWSClient()
	window := NewDashboardWindow("win-1", data, wsClient)
	bindings := &DashboardBindings{window: window}

	if bindings.GetWebHost() != "192.168.1.100" {
		t.Errorf("GetWebHost() = %q, want '192.168.1.100'", bindings.GetWebHost())
	}
}

// --- ApprovalWindowData additional tests ---

func TestApprovalWindowData_GetType(t *testing.T) {
	data := &ApprovalWindowData{}
	if data.GetType() != "approval" {
		t.Errorf("GetType() = %q, want 'approval'", data.GetType())
	}
}

func TestApprovalWindowData_GetTimeout(t *testing.T) {
	data := &ApprovalWindowData{TimeoutSeconds: 45}
	if data.GetTimeout() != 45 {
		t.Errorf("GetTimeout() = %d, want 45", data.GetTimeout())
	}
}

func TestApprovalWindowData_GetTimeout_Zero(t *testing.T) {
	data := &ApprovalWindowData{}
	if data.GetTimeout() != 0 {
		t.Errorf("GetTimeout() = %d, want 0", data.GetTimeout())
	}
}

// --- WindowError tests ---

func TestWindowError_Error(t *testing.T) {
	tests := []struct {
		code    string
		message string
	}{
		{"INVALID_DATA", "Invalid window data"},
		{"TEST_ERROR", "test error message"},
		{"", ""},
	}
	for _, tt := range tests {
		err := &WindowError{Code: tt.code, Message: tt.message}
		if err.Error() != tt.message {
			t.Errorf("Error() = %q, want %q", err.Error(), tt.message)
		}
	}
}

func TestErrInvalidData(t *testing.T) {
	if ErrInvalidData == nil {
		t.Fatal("ErrInvalidData should not be nil")
	}
	if ErrInvalidData.Code != "INVALID_DATA" {
		t.Errorf("Code = %q, want 'INVALID_DATA'", ErrInvalidData.Code)
	}
	if !errors.Is(ErrInvalidData, ErrInvalidData) {
		t.Error("ErrInvalidData should match itself via errors.Is")
	}
}

// --- min helper function test ---

func TestMin(t *testing.T) {
	tests := []struct {
		a, b, expected int
	}{
		{3, 5, 3},
		{5, 3, 3},
		{0, 0, 0},
		{-1, 1, -1},
		{100, 100, 100},
	}
	for _, tt := range tests {
		if got := min(tt.a, tt.b); got != tt.expected {
			t.Errorf("min(%d, %d) = %d, want %d", tt.a, tt.b, got, tt.expected)
		}
	}
}

// --- ApprovalApp tests ---

func TestNewApprovalApp(t *testing.T) {
	data := &ApprovalWindowData{
		RequestID: "req-1",
		Operation: "file_write",
	}
	wsClient := newTestWSClient()
	window := NewApprovalWindow("win-1", data, wsClient)

	app := NewApprovalApp(window)
	if app == nil {
		t.Fatal("NewApprovalApp returned nil")
	}
	if app.window != window {
		t.Error("Window should be set")
	}
}

// --- ApprovalBindings additional tests ---

func TestApprovalBindings_CloseWindow(t *testing.T) {
	data := &ApprovalWindowData{
		RequestID: "req-1",
		Operation: "file_write",
	}
	wsClient := newTestWSClient()
	window := NewApprovalWindow("win-1", data, wsClient)
	bindings := &ApprovalBindings{window: window}

	// Should not panic
	bindings.CloseWindow()
}
