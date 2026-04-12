//go:build !cross_compile

package windows

import (
	"context"
	"testing"

	"github.com/276793422/NemesisBot/module/desktop/websocket"
)

type testData struct{}

func (d *testData) Validate() error { return nil }
func (d *testData) GetType() string { return "test" }

func newTestWSClient() *websocket.WebSocketClient {
	wsKey := &websocket.WebSocketKey{Key: "test-key", Port: 12345, Path: "/test"}
	return websocket.NewWebSocketClient(wsKey)
}

func TestNewWindowBase(t *testing.T) {
	wsClient := newTestWSClient()
	wb := NewWindowBase("win-1", "approval", &testData{}, wsClient)

	if wb == nil {
		t.Fatal("NewWindowBase returned nil")
	}
	if wb.ID != "win-1" {
		t.Errorf("Expected ID 'win-1', got '%s'", wb.ID)
	}
	if wb.Type != "approval" {
		t.Errorf("Expected Type 'approval', got '%s'", wb.Type)
	}
	if wb.WSClient == nil {
		t.Error("WSClient should be set")
	}
	if wb.ResultCh == nil {
		t.Error("ResultCh should be initialized")
	}
}

func TestWindowBaseGetID(t *testing.T) {
	wb := NewWindowBase("win-42", "test", &testData{}, newTestWSClient())
	if wb.GetID() != "win-42" {
		t.Errorf("Expected GetID 'win-42', got '%s'", wb.GetID())
	}
}

func TestWindowBaseGetType(t *testing.T) {
	wb := NewWindowBase("win-1", "approval", &testData{}, newTestWSClient())
	if wb.GetType() != "approval" {
		t.Errorf("Expected GetType 'approval', got '%s'", wb.GetType())
	}
}

func TestWindowBaseGetData(t *testing.T) {
	data := &testData{}
	wb := NewWindowBase("win-1", "test", data, newTestWSClient())

	retrieved := wb.GetData()
	if retrieved != data {
		t.Error("GetData should return the same data instance")
	}
}

func TestWindowBaseSetData(t *testing.T) {
	wb := NewWindowBase("win-1", "test", &testData{}, newTestWSClient())

	newData := &testData{}
	err := wb.SetData(newData)
	if err != nil {
		t.Errorf("SetData failed: %v", err)
	}

	retrieved := wb.GetData()
	if retrieved != newData {
		t.Error("GetData should return the new data instance")
	}
}

func TestWindowBaseStartup(t *testing.T) {
	wb := NewWindowBase("win-1", "test", &testData{}, newTestWSClient())

	ctx := context.Background()
	err := wb.Startup(ctx)
	if err != nil {
		t.Errorf("Startup failed: %v", err)
	}
}

func TestWindowBaseShutdown(t *testing.T) {
	wb := NewWindowBase("win-1", "test", &testData{}, newTestWSClient())

	ctx := context.Background()
	wb.Shutdown(ctx)
	// Should not panic
}

func TestWindowBaseBind(t *testing.T) {
	wb := NewWindowBase("win-1", "test", &testData{}, newTestWSClient())

	bindings := wb.Bind()
	if len(bindings) != 1 {
		t.Errorf("Expected 1 binding, got %d", len(bindings))
	}
	if bindings[0] != wb {
		t.Error("Binding should be the WindowBase itself")
	}
}

func TestWindowBaseSendResult(t *testing.T) {
	wsClient := newTestWSClient()
	wb := NewWindowBase("win-1", "test", &testData{}, wsClient)

	result := map[string]interface{}{"approved": true, "reason": "ok"}
	wb.SendResult(result)

	// Result should be in the ResultCh
	select {
	case r := <-wb.ResultCh:
		rMap, ok := r.(map[string]interface{})
		if !ok {
			t.Fatalf("Expected map result, got %T", r)
		}
		if rMap["approved"] != true {
			t.Errorf("Expected approved=true, got %v", rMap["approved"])
		}
	default:
		t.Error("Expected result in ResultCh")
	}
}
