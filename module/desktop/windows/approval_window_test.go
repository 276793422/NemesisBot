//go:build !cross_compile

package windows

import (
	"errors"
	"testing"
)

func TestApprovalWindowDataValidate(t *testing.T) {
	tests := []struct {
		name    string
		data    *ApprovalWindowData
		wantErr bool
	}{
		{
			name: "valid data",
			data: &ApprovalWindowData{
				RequestID: "req-1",
				Operation: "file_write",
			},
			wantErr: false,
		},
		{
			name: "missing request ID",
			data: &ApprovalWindowData{
				Operation: "file_write",
			},
			wantErr: true,
		},
		{
			name: "missing operation",
			data: &ApprovalWindowData{
				RequestID: "req-1",
			},
			wantErr: true,
		},
		{
			name:    "both missing",
			data:    &ApprovalWindowData{},
			wantErr: true,
		},
		{
			name: "full data",
			data: &ApprovalWindowData{
				RequestID:      "req-1",
				Operation:      "file_write",
				OperationName:  "写入文件",
				Target:         "C:\\Temp\\test.txt",
				RiskLevel:      "HIGH",
				Reason:         "测试",
				TimeoutSeconds: 30,
				Context:        map[string]string{"key": "value"},
				Timestamp:      1710000000,
			},
			wantErr: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			err := tt.data.Validate()
			if (err != nil) != tt.wantErr {
				t.Errorf("Validate() error = %v, wantErr %v", err, tt.wantErr)
			}
			if tt.wantErr && !errors.Is(err, ErrInvalidData) {
				t.Errorf("Expected ErrInvalidData, got: %v", err)
			}
		})
	}
}

func TestApprovalWindowDataGetType(t *testing.T) {
	data := &ApprovalWindowData{}
	if data.GetType() != "approval" {
		t.Errorf("Expected GetType 'approval', got '%s'", data.GetType())
	}
}

func TestApprovalWindowDataGetTimeout(t *testing.T) {
	data := &ApprovalWindowData{TimeoutSeconds: 60}
	if data.GetTimeout() != 60 {
		t.Errorf("Expected GetTimeout 60, got %d", data.GetTimeout())
	}
}

func TestNewApprovalWindow(t *testing.T) {
	data := &ApprovalWindowData{
		RequestID: "req-1",
		Operation: "file_write",
	}
	wsClient := newTestWSClient()

	window := NewApprovalWindow("win-1", data, wsClient)
	if window == nil {
		t.Fatal("NewApprovalWindow returned nil")
	}
	if window.ID != "win-1" {
		t.Errorf("Expected ID 'win-1', got '%s'", window.ID)
	}
	if window.Type != "approval" {
		t.Errorf("Expected Type 'approval', got '%s'", window.Type)
	}
}

func TestApprovalBindingsGetRequestID(t *testing.T) {
	data := &ApprovalWindowData{RequestID: "req-123", Operation: "file_write"}
	wsClient := newTestWSClient()
	window := NewApprovalWindow("win-1", data, wsClient)
	bindings := &ApprovalBindings{window: window}

	if bindings.GetRequestID() != "req-123" {
		t.Errorf("Expected 'req-123', got '%s'", bindings.GetRequestID())
	}
}

func TestApprovalBindingsGetOperation(t *testing.T) {
	data := &ApprovalWindowData{RequestID: "req-1", Operation: "file_write"}
	wsClient := newTestWSClient()
	window := NewApprovalWindow("win-1", data, wsClient)
	bindings := &ApprovalBindings{window: window}

	if bindings.GetOperation() != "file_write" {
		t.Errorf("Expected 'file_write', got '%s'", bindings.GetOperation())
	}
}

func TestApprovalBindingsGetOperationName(t *testing.T) {
	data := &ApprovalWindowData{
		RequestID:     "req-1",
		Operation:     "file_write",
		OperationName: "写入文件",
	}
	wsClient := newTestWSClient()
	window := NewApprovalWindow("win-1", data, wsClient)
	bindings := &ApprovalBindings{window: window}

	if bindings.GetOperationName() != "写入文件" {
		t.Errorf("Expected '写入文件', got '%s'", bindings.GetOperationName())
	}
}

func TestApprovalBindingsGetOperationNameEmpty(t *testing.T) {
	data := &ApprovalWindowData{
		RequestID: "req-1",
		Operation: "file_write",
	}
	wsClient := newTestWSClient()
	window := NewApprovalWindow("win-1", data, wsClient)
	bindings := &ApprovalBindings{window: window}

	// When OperationName is empty, falls back to GetOperationDisplayName
	name := bindings.GetOperationName()
	if name == "" {
		t.Error("Expected non-empty display name")
	}
}

func TestApprovalBindingsGetTarget(t *testing.T) {
	data := &ApprovalWindowData{
		RequestID: "req-1",
		Operation: "file_write",
		Target:    "C:\\Temp\\test.txt",
	}
	wsClient := newTestWSClient()
	window := NewApprovalWindow("win-1", data, wsClient)
	bindings := &ApprovalBindings{window: window}

	if bindings.GetTarget() != "C:\\Temp\\test.txt" {
		t.Errorf("Expected 'C:\\Temp\\test.txt', got '%s'", bindings.GetTarget())
	}
}

func TestApprovalBindingsGetRiskLevel(t *testing.T) {
	data := &ApprovalWindowData{
		RequestID: "req-1",
		Operation: "file_write",
		RiskLevel: "HIGH",
	}
	wsClient := newTestWSClient()
	window := NewApprovalWindow("win-1", data, wsClient)
	bindings := &ApprovalBindings{window: window}

	if bindings.GetRiskLevel() != "HIGH" {
		t.Errorf("Expected 'HIGH', got '%s'", bindings.GetRiskLevel())
	}
}

func TestApprovalBindingsGetReason(t *testing.T) {
	data := &ApprovalWindowData{
		RequestID: "req-1",
		Operation: "file_write",
		Reason:    "安全测试",
	}
	wsClient := newTestWSClient()
	window := NewApprovalWindow("win-1", data, wsClient)
	bindings := &ApprovalBindings{window: window}

	if bindings.GetReason() != "安全测试" {
		t.Errorf("Expected '安全测试', got '%s'", bindings.GetReason())
	}
}

func TestApprovalBindingsGetTimeout(t *testing.T) {
	data := &ApprovalWindowData{
		RequestID:      "req-1",
		Operation:      "file_write",
		TimeoutSeconds: 45,
	}
	wsClient := newTestWSClient()
	window := NewApprovalWindow("win-1", data, wsClient)
	bindings := &ApprovalBindings{window: window}

	if bindings.GetTimeout() != 45 {
		t.Errorf("Expected 45, got %d", bindings.GetTimeout())
	}
}

func TestApprovalBindingsGetContext(t *testing.T) {
	data := &ApprovalWindowData{
		RequestID: "req-1",
		Operation: "file_write",
		Context:   map[string]string{"source": "rpc", "user": "admin"},
	}
	wsClient := newTestWSClient()
	window := NewApprovalWindow("win-1", data, wsClient)
	bindings := &ApprovalBindings{window: window}

	ctx := bindings.GetContext()
	if ctx["source"] != "rpc" {
		t.Errorf("Expected source='rpc', got '%s'", ctx["source"])
	}
	if ctx["user"] != "admin" {
		t.Errorf("Expected user='admin', got '%s'", ctx["user"])
	}
}

func TestApprovalWindowBind(t *testing.T) {
	data := &ApprovalWindowData{
		RequestID: "req-1",
		Operation: "file_write",
	}
	wsClient := newTestWSClient()
	window := NewApprovalWindow("win-1", data, wsClient)

	bindings := window.Bind()
	if len(bindings) < 2 {
		t.Errorf("Expected at least 2 bindings (base + approval), got %d", len(bindings))
	}
}

func TestWindowError(t *testing.T) {
	err := &WindowError{Code: "TEST", Message: "test error"}
	if err.Error() != "test error" {
		t.Errorf("Expected 'test error', got '%s'", err.Error())
	}
}
