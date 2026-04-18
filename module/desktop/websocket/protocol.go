//go:build !cross_compile

package websocket

import (
	"encoding/json"
	"fmt"

	"github.com/google/uuid"
)

const Version = "2.0"

// Message 线路消息信封（JSON-RPC 2.0 兼容）
type Message struct {
	JSONRPC string          `json:"jsonrpc"`
	ID      string          `json:"id,omitempty"`
	Method  string          `json:"method,omitempty"`
	Params  json.RawMessage `json:"params,omitempty"`
	Result  json.RawMessage `json:"result,omitempty"`
	Error   *ErrorPayload   `json:"error,omitempty"`
}

// ErrorPayload 错误载荷
type ErrorPayload struct {
	Code    int             `json:"code"`
	Message string          `json:"message"`
	Data    json.RawMessage `json:"data,omitempty"`
}

// 标准错误码
const (
	ErrParseError     = -32700
	ErrInvalidRequest = -32600
	ErrMethodNotFound = -32601
	ErrInvalidParams  = -32602
	ErrInternal       = -32603
)

// 应用错误码
const (
	ErrTimeout       = -32001
	ErrNotReady      = -32002
	ErrWindowNotFound = -32003
)

// NewRequest 创建一个 Request 消息（带 ID，等响应）
func NewRequest(method string, params interface{}) (*Message, error) {
	id := uuid.New().String()
	return newMessageWithID(id, method, params)
}

// NewRequestWithID 创建一个指定 ID 的 Request 消息
func NewRequestWithID(id, method string, params interface{}) (*Message, error) {
	return newMessageWithID(id, method, params)
}

func newMessageWithID(id, method string, params interface{}) (*Message, error) {
	m := &Message{
		JSONRPC: Version,
		ID:      id,
		Method:  method,
	}
	if params != nil {
		raw, err := json.Marshal(params)
		if err != nil {
			return nil, fmt.Errorf("marshal params: %w", err)
		}
		m.Params = raw
	}
	return m, nil
}

// NewNotification 创建一个 Notification 消息（无 ID，不等响应）
func NewNotification(method string, params interface{}) (*Message, error) {
	m := &Message{
		JSONRPC: Version,
		Method:  method,
	}
	if params != nil {
		raw, err := json.Marshal(params)
		if err != nil {
			return nil, fmt.Errorf("marshal params: %w", err)
		}
		m.Params = raw
	}
	return m, nil
}

// NewResponse 创建一个成功响应
func NewResponse(id string, result interface{}) (*Message, error) {
	m := &Message{
		JSONRPC: Version,
		ID:      id,
	}
	if result != nil {
		raw, err := json.Marshal(result)
		if err != nil {
			return nil, fmt.Errorf("marshal result: %w", err)
		}
		m.Result = raw
	}
	return m, nil
}

// NewErrorResponse 创建一个错误响应
func NewErrorResponse(id string, code int, msg string, data interface{}) (*Message, error) {
	m := &Message{
		JSONRPC: Version,
		ID:      id,
		Error: &ErrorPayload{
			Code:    code,
			Message: msg,
		},
	}
	if data != nil {
		raw, err := json.Marshal(data)
		if err != nil {
			return nil, fmt.Errorf("marshal error data: %w", err)
		}
		m.Error.Data = raw
	}
	return m, nil
}

// IsRequest 判断是否为 Request（有 ID 且有 Method）
func (m *Message) IsRequest() bool {
	return m.ID != "" && m.Method != ""
}

// IsNotification 判断是否为 Notification（无 ID 但有 Method）
func (m *Message) IsNotification() bool {
	return m.ID == "" && m.Method != ""
}

// IsResponse 判断是否为 Response（有 ID 且无 Method）
func (m *Message) IsResponse() bool {
	return m.ID != "" && m.Method == ""
}

// IsSuccessResponse 判断是否为成功响应
func (m *Message) IsSuccessResponse() bool {
	return m.IsResponse() && m.Error == nil
}

// IsErrorResponse 判断是否为错误响应
func (m *Message) IsErrorResponse() bool {
	return m.IsResponse() && m.Error != nil
}

// DecodeParams 解码 params 到目标结构
func (m *Message) DecodeParams(v interface{}) error {
	if m.Params == nil {
		return nil
	}
	return json.Unmarshal(m.Params, v)
}

// DecodeResult 解码 result 到目标结构
func (m *Message) DecodeResult(v interface{}) error {
	if m.Result == nil {
		return nil
	}
	return json.Unmarshal(m.Result, v)
}

// DecodeErrorData 解码 error.data 到目标结构
func (m *Message) DecodeErrorData(v interface{}) error {
	if m.Error == nil || m.Error.Data == nil {
		return nil
	}
	return json.Unmarshal(m.Error.Data, v)
}
