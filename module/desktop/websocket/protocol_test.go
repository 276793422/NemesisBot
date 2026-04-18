//go:build !cross_compile

package websocket

import (
	"encoding/json"
	"testing"
)

func TestNewRequest(t *testing.T) {
	params := map[string]string{"key": "value"}
	msg, err := NewRequest("window.bring_to_front", params)
	if err != nil {
		t.Fatalf("NewRequest: %v", err)
	}

	if msg.JSONRPC != Version {
		t.Errorf("JSONRPC = %q, want %q", msg.JSONRPC, Version)
	}
	if msg.ID == "" {
		t.Error("ID should not be empty for Request")
	}
	if msg.Method != "window.bring_to_front" {
		t.Errorf("Method = %q, want %q", msg.Method, "window.bring_to_front")
	}
	if msg.Params == nil {
		t.Error("Params should not be nil")
	}
	if msg.Result != nil {
		t.Error("Result should be nil for Request")
	}
	if msg.Error != nil {
		t.Error("Error should be nil for Request")
	}
	if !msg.IsRequest() {
		t.Error("IsRequest() should be true")
	}
	if msg.IsNotification() {
		t.Error("IsNotification() should be false")
	}
	if msg.IsResponse() {
		t.Error("IsResponse() should be false")
	}
}

func TestNewRequestWithNilParams(t *testing.T) {
	msg, err := NewRequest("system.ping", nil)
	if err != nil {
		t.Fatalf("NewRequest: %v", err)
	}
	if msg.Params != nil {
		t.Error("Params should be nil when nil is passed")
	}
}

func TestNewNotification(t *testing.T) {
	params := map[string]int{"port": 8080}
	msg, err := NewNotification("state.service_status", params)
	if err != nil {
		t.Fatalf("NewNotification: %v", err)
	}

	if msg.ID != "" {
		t.Errorf("ID = %q, want empty", msg.ID)
	}
	if msg.Method != "state.service_status" {
		t.Errorf("Method = %q, want %q", msg.Method, "state.service_status")
	}
	if !msg.IsNotification() {
		t.Error("IsNotification() should be true")
	}
	if msg.IsRequest() {
		t.Error("IsRequest() should be false")
	}
	if msg.IsResponse() {
		t.Error("IsResponse() should be false")
	}
}

func TestNewResponse(t *testing.T) {
	result := map[string]string{"status": "ok"}
	msg, err := NewResponse("test-id-123", result)
	if err != nil {
		t.Fatalf("NewResponse: %v", err)
	}

	if msg.ID != "test-id-123" {
		t.Errorf("ID = %q, want %q", msg.ID, "test-id-123")
	}
	if msg.Method != "" {
		t.Errorf("Method = %q, want empty", msg.Method)
	}
	if msg.Result == nil {
		t.Error("Result should not be nil")
	}
	if msg.Error != nil {
		t.Error("Error should be nil for success response")
	}
	if !msg.IsSuccessResponse() {
		t.Error("IsSuccessResponse() should be true")
	}
	if msg.IsErrorResponse() {
		t.Error("IsErrorResponse() should be false")
	}
}

func TestNewResponseNilResult(t *testing.T) {
	msg, err := NewResponse("test-id", nil)
	if err != nil {
		t.Fatalf("NewResponse: %v", err)
	}
	if msg.Result != nil {
		t.Error("Result should be nil when nil is passed")
	}
	if !msg.IsSuccessResponse() {
		t.Error("IsSuccessResponse() should be true even with nil result")
	}
}

func TestNewErrorResponse(t *testing.T) {
	msg, err := NewErrorResponse("test-id-456", ErrMethodNotFound, "method not found", map[string]string{"method": "foo"})
	if err != nil {
		t.Fatalf("NewErrorResponse: %v", err)
	}

	if msg.ID != "test-id-456" {
		t.Errorf("ID = %q, want %q", msg.ID, "test-id-456")
	}
	if msg.Error == nil {
		t.Fatal("Error should not be nil")
	}
	if msg.Error.Code != ErrMethodNotFound {
		t.Errorf("Error.Code = %d, want %d", msg.Error.Code, ErrMethodNotFound)
	}
	if msg.Error.Message != "method not found" {
		t.Errorf("Error.Message = %q, want %q", msg.Error.Message, "method not found")
	}
	if msg.Error.Data == nil {
		t.Error("Error.Data should not be nil")
	}
	if !msg.IsErrorResponse() {
		t.Error("IsErrorResponse() should be true")
	}
	if msg.IsSuccessResponse() {
		t.Error("IsSuccessResponse() should be false")
	}
}

func TestNewErrorResponseNilData(t *testing.T) {
	msg, err := NewErrorResponse("test-id", ErrInternal, "internal error", nil)
	if err != nil {
		t.Fatalf("NewErrorResponse: %v", err)
	}
	if msg.Error.Data != nil {
		t.Error("Error.Data should be nil when nil is passed")
	}
}

func TestMessageJSONRoundTrip(t *testing.T) {
	orig, err := NewRequest("window.bring_to_front", map[string]string{"window": "main"})
	if err != nil {
		t.Fatalf("NewRequest: %v", err)
	}

	data, err := json.Marshal(orig)
	if err != nil {
		t.Fatalf("Marshal: %v", err)
	}

	var decoded Message
	if err := json.Unmarshal(data, &decoded); err != nil {
		t.Fatalf("Unmarshal: %v", err)
	}

	if decoded.JSONRPC != Version {
		t.Errorf("JSONRPC = %q, want %q", decoded.JSONRPC, Version)
	}
	if decoded.ID != orig.ID {
		t.Errorf("ID = %q, want %q", decoded.ID, orig.ID)
	}
	if decoded.Method != orig.Method {
		t.Errorf("Method = %q, want %q", decoded.Method, orig.Method)
	}
	if !decoded.IsRequest() {
		t.Error("Decoded message should be a Request")
	}
}

func TestDecodeParams(t *testing.T) {
	msg, err := NewRequest("test.method", map[string]string{"foo": "bar"})
	if err != nil {
		t.Fatalf("NewRequest: %v", err)
	}

	var params map[string]string
	if err := msg.DecodeParams(&params); err != nil {
		t.Fatalf("DecodeParams: %v", err)
	}
	if params["foo"] != "bar" {
		t.Errorf("params[\"foo\"] = %q, want %q", params["foo"], "bar")
	}
}

func TestDecodeParamsNil(t *testing.T) {
	msg := &Message{JSONRPC: Version}
	var params map[string]string
	if err := msg.DecodeParams(&params); err != nil {
		t.Fatalf("DecodeParams on nil params: %v", err)
	}
}

func TestDecodeResult(t *testing.T) {
	msg, err := NewResponse("id", map[string]int{"count": 42})
	if err != nil {
		t.Fatalf("NewResponse: %v", err)
	}

	var result map[string]int
	if err := msg.DecodeResult(&result); err != nil {
		t.Fatalf("DecodeResult: %v", err)
	}
	if result["count"] != 42 {
		t.Errorf("result[\"count\"] = %d, want 42", result["count"])
	}
}

func TestDecodeErrorData(t *testing.T) {
	msg, err := NewErrorResponse("id", ErrInvalidParams, "bad params", map[string]string{"field": "x"})
	if err != nil {
		t.Fatalf("NewErrorResponse: %v", err)
	}

	var data map[string]string
	if err := msg.DecodeErrorData(&data); err != nil {
		t.Fatalf("DecodeErrorData: %v", err)
	}
	if data["field"] != "x" {
		t.Errorf("data[\"field\"] = %q, want %q", data["field"], "x")
	}
}

func TestDecodeErrorDataNil(t *testing.T) {
	msg := &Message{JSONRPC: Version}
	var data map[string]string
	if err := msg.DecodeErrorData(&data); err != nil {
		t.Fatalf("DecodeErrorData on nil error: %v", err)
	}
}

func TestNotificationJSONOmitsID(t *testing.T) {
	msg, err := NewNotification("state.update", map[string]string{"status": "running"})
	if err != nil {
		t.Fatalf("NewNotification: %v", err)
	}

	data, err := json.Marshal(msg)
	if err != nil {
		t.Fatalf("Marshal: %v", err)
	}

	var raw map[string]interface{}
	if err := json.Unmarshal(data, &raw); err != nil {
		t.Fatalf("Unmarshal to raw: %v", err)
	}

	if _, hasID := raw["id"]; hasID {
		t.Error("Notification should not have \"id\" in JSON output")
	}
}
