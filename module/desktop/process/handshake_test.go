//go:build !cross_compile

package process

import (
	"encoding/json"
	"io"
	"testing"
	"time"
)

// setupPipes creates connected pipe pairs using io.Pipe for in-process communication.
// reader/writer fields are nil since handshake functions only use Encode/Decode, not Close.
func setupPipes(t *testing.T) (*ChildProcess, *ReadCloser, *WriteCloser) {
	t.Helper()

	// parent → child: parent writes, child reads
	p2cR, p2cW := io.Pipe()
	// child → parent: child writes, parent reads
	c2pR, c2pW := io.Pipe()

	child := &ChildProcess{
		ID:     "test-child",
		Stdin:  &WriteCloser{Encoder: json.NewEncoder(p2cW), writer: nil},
		Stdout: &ReadCloser{Decoder: json.NewDecoder(c2pR), reader: nil},
	}

	childIn := &ReadCloser{Decoder: json.NewDecoder(p2cR), reader: nil}
	childOut := &WriteCloser{Encoder: json.NewEncoder(c2pW), writer: nil}

	return child, childIn, childOut
}

func TestParentChildHandshake(t *testing.T) {
	child, childIn, childOut := setupPipes(t)

	parentResultCh := make(chan error, 1)
	go func() {
		_, err := ParentHandshake(child)
		parentResultCh <- err
	}()

	childResultCh := make(chan error, 1)
	go func() {
		_, err := ChildHandshake(childIn, childOut)
		childResultCh <- err
	}()

	select {
	case err := <-parentResultCh:
		if err != nil {
			t.Errorf("ParentHandshake failed: %v", err)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("ParentHandshake timed out")
	}

	select {
	case err := <-childResultCh:
		if err != nil {
			t.Errorf("ChildHandshake failed: %v", err)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("ChildHandshake timed out")
	}
}

func TestParentHandshakeInvalidACK(t *testing.T) {
	p2cR, p2cW := io.Pipe()
	c2pR, c2pW := io.Pipe()

	child := &ChildProcess{
		ID:     "test-child",
		Stdin:  &WriteCloser{Encoder: json.NewEncoder(p2cW), writer: nil},
		Stdout: &ReadCloser{Decoder: json.NewDecoder(c2pR), reader: nil},
	}

	// Simulate a child that sends a non-ack message
	go func() {
		// Read the handshake message (discard)
		var msg PipeMessage
		json.NewDecoder(p2cR).Decode(&msg)

		// Send invalid response
		invalid := &PipeMessage{Type: "error", Version: "1.0"}
		json.NewEncoder(c2pW).Encode(invalid)
	}()

	_, err := ParentHandshake(child)
	if err == nil {
		t.Error("Expected error for non-ack response")
	}
}

func TestSendReceiveWSKey(t *testing.T) {
	child, childIn, childOut := setupPipes(t)

	sendErrCh := make(chan error, 1)
	go func() {
		sendErrCh <- SendWSKey(child, "test-ws-key", 8080, "/ws")
	}()

	key, port, path, err := ReceiveWSKey(childIn, childOut)

	select {
	case sendErr := <-sendErrCh:
		if sendErr != nil {
			t.Errorf("SendWSKey failed: %v", sendErr)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("SendWSKey timed out")
	}

	if err != nil {
		t.Fatalf("ReceiveWSKey failed: %v", err)
	}
	if key != "test-ws-key" {
		t.Errorf("Expected key 'test-ws-key', got '%s'", key)
	}
	if port != 8080 {
		t.Errorf("Expected port 8080, got %d", port)
	}
	if path != "/ws" {
		t.Errorf("Expected path '/ws', got '%s'", path)
	}
}

func TestSendReceiveWindowData(t *testing.T) {
	child, childIn, childOut := setupPipes(t)

	testData := map[string]interface{}{
		"request_id": "req-123",
		"operation":  "file_write",
		"risk_level": "HIGH",
	}

	sendErrCh := make(chan error, 1)
	go func() {
		sendErrCh <- SendWindowData(child, testData)
	}()

	receivedData, err := ReceiveWindowData(childIn, childOut)

	select {
	case sendErr := <-sendErrCh:
		if sendErr != nil {
			t.Errorf("SendWindowData failed: %v", sendErr)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("SendWindowData timed out")
	}

	if err != nil {
		t.Fatalf("ReceiveWindowData failed: %v", err)
	}

	dataMap, ok := receivedData.(map[string]interface{})
	if !ok {
		t.Fatalf("Expected map[string]interface{}, got %T", receivedData)
	}
	if dataMap["request_id"] != "req-123" {
		t.Errorf("Expected request_id 'req-123', got '%v'", dataMap["request_id"])
	}
	if dataMap["operation"] != "file_write" {
		t.Errorf("Expected operation 'file_write', got '%v'", dataMap["operation"])
	}
}

func TestReceiveWSKeyInvalidType(t *testing.T) {
	p2cR, p2cW := io.Pipe()
	_, c2pW := io.Pipe()

	childIn := &ReadCloser{Decoder: json.NewDecoder(p2cR), reader: nil}
	childOut := &WriteCloser{Encoder: json.NewEncoder(c2pW), writer: nil}

	// Send wrong message type
	go func() {
		msg := &PipeMessage{Type: "handshake", Version: "1.0", Data: map[string]interface{}{}}
		json.NewEncoder(p2cW).Encode(msg)
	}()

	_, _, _, err := ReceiveWSKey(childIn, childOut)
	if err == nil {
		t.Error("Expected error for non-ws_key message type")
	}
}

func TestProtocolConstants(t *testing.T) {
	if ProtocolName != "anon-pipe-v1" {
		t.Errorf("Expected ProtocolName 'anon-pipe-v1', got '%s'", ProtocolName)
	}
	if ProtocolVersion != "1.0" {
		t.Errorf("Expected ProtocolVersion '1.0', got '%s'", ProtocolVersion)
	}
	if HandshakeTimeout != 3*time.Second {
		t.Errorf("Expected HandshakeTimeout 3s, got %v", HandshakeTimeout)
	}
	if AckTimeout != 5*time.Second {
		t.Errorf("Expected AckTimeout 5s, got %v", AckTimeout)
	}
}
