//go:build !cross_compile

package windows

import (
	"context"
	"fmt"
	"os"
	"time"

	"github.com/276793422/NemesisBot/module/desktop/websocket"
	"github.com/276793422/NemesisBot/module/security/approval"
	"github.com/wailsapp/wails/v2/pkg/runtime"
)

// ApprovalWindowData 审批窗口数据
type ApprovalWindowData struct {
	RequestID      string            `json:"request_id"`
	Operation      string            `json:"operation"`
	OperationName  string            `json:"operation_name"`
	Target         string            `json:"target"`
	RiskLevel      string            `json:"risk_level"`
	Reason         string            `json:"reason"`
	TimeoutSeconds int               `json:"timeout_seconds"`
	Context        map[string]string `json:"context"`
	Timestamp      int64             `json:"timestamp"`
}

// Validate 验证数据
func (d *ApprovalWindowData) Validate() error {
	if d.RequestID == "" {
		return ErrInvalidData
	}
	if d.Operation == "" {
		return ErrInvalidData
	}
	return nil
}

// GetType 获取类型
func (d *ApprovalWindowData) GetType() string {
	return "approval"
}

// GetTimeout 获取超时时间
func (d *ApprovalWindowData) GetTimeout() int {
	return d.TimeoutSeconds
}

// ApprovalWindow 审批窗口
type ApprovalWindow struct {
	WindowBase
	ctx context.Context
}

// NewApprovalWindow 创建审批窗口
func NewApprovalWindow(windowID string, data *ApprovalWindowData, wsClient *websocket.WebSocketClient) *ApprovalWindow {
	base := NewWindowBase(windowID, "approval", data, wsClient)

	return &ApprovalWindow{
		WindowBase: *base,
	}
}

// Startup 启动窗口
func (w *ApprovalWindow) Startup(ctx context.Context) error {
	w.ctx = ctx // 保存 context 供后续使用

	if err := w.WindowBase.Startup(ctx); err != nil {
		return err
	}

	fmt.Fprintf(os.Stderr, "[ApprovalWindow-%s] Startup: setting up event listeners\n", w.ID)

	// 监听来自前端的提交事件
	runtime.EventsOn(ctx, "submit-approval", func(optionalData ...interface{}) {
		fmt.Fprintf(os.Stderr, "[ApprovalWindow-%s] === Received submit-approval event ===\n", w.ID)
		fmt.Fprintf(os.Stderr, "[ApprovalWindow-%s] Event data length: %d\n", w.ID, len(optionalData))
		for i, data := range optionalData {
			fmt.Fprintf(os.Stderr, "[ApprovalWindow-%s] Data[%d]: %+v (type: %T)\n", w.ID, i, data, data)
		}

		// 解析数据
		if len(optionalData) > 0 {
			if dataMap, ok := optionalData[0].(map[string]interface{}); ok {
				fmt.Fprintf(os.Stderr, "[ApprovalWindow-%s] Data is map[string]interface{}\n", w.ID)

				approved := false
				if v, exists := dataMap["approved"]; exists {
					fmt.Fprintf(os.Stderr, "[ApprovalWindow-%s] approved field: %+v (type: %T)\n", w.ID, v, v)
					if bv, ok := v.(bool); ok {
						approved = bv
						fmt.Fprintf(os.Stderr, "[ApprovalWindow-%s] Parsed approved as: %v\n", w.ID, approved)
					} else {
						fmt.Fprintf(os.Stderr, "[ApprovalWindow-%s] WARNING: approved is not bool\n", w.ID)
					}
				}

				reason := "未知原因"
				if v, exists := dataMap["reason"]; exists {
					if sv, ok := v.(string); ok {
						reason = sv
					}
				}

				fmt.Fprintf(os.Stderr, "[ApprovalWindow-%s] Calling SubmitApproval: approved=%v, reason=%s\n", w.ID, approved, reason)
				w.SubmitApproval(approved, reason)
			} else {
				fmt.Fprintf(os.Stderr, "[ApprovalWindow-%s] ERROR: Data is not map[string]interface{}, it's %T\n", w.ID, optionalData[0])
			}
		} else {
			fmt.Fprintf(os.Stderr, "[ApprovalWindow-%s] ERROR: No data in event\n", w.ID)
		}
	})

	// 监听来自前端的关闭窗口请求
	runtime.EventsOn(ctx, "request-window-close", func(...interface{}) {
		fmt.Fprintf(os.Stderr, "[ApprovalWindow-%s] Received window close request\n", w.ID)
		// Wails v2 中没有直接的 WindowClose，我们需要通过 quit 来关闭
		// 或者我们可以发送一个事件让主循环知道应该关闭
		// 暂时记录日志
	})

	// 发送初始数据到前端
	runtime.EventsEmit(ctx, "init-data", w.Data)
	fmt.Fprintf(os.Stderr, "[ApprovalWindow-%s] Init data sent to frontend\n", w.ID)

	return nil
}

// Bind 返回绑定结构
func (w *ApprovalWindow) Bind() []interface{} {
	baseBindings := w.WindowBase.Bind()
	approvalBindings := &ApprovalBindings{window: w}
	return append(baseBindings, approvalBindings)
}

// SubmitApproval 提交审批决定
func (w *ApprovalWindow) SubmitApproval(approved bool, reason string) error {
	fmt.Fprintf(os.Stderr, "[ApprovalWindow-%s] SubmitApproval: approved=%v, reason=%s\n", w.ID, approved, reason)

	data := w.GetData().(*ApprovalWindowData)

	result := map[string]interface{}{
		"approved": approved,
		"reason":   reason,
		"request_id": data.RequestID,
		"timestamp": time.Now().Unix(),
	}

	w.SendResult(result)

	fmt.Fprintf(os.Stderr, "[ApprovalWindow-%s] Result sent via WebSocket\n", w.ID)

	// 关闭窗口/退出应用
	if w.ctx != nil {
		fmt.Fprintf(os.Stderr, "[ApprovalWindow-%s] Quitting application...\n", w.ID)
		runtime.Quit(w.ctx)
	}

	return nil
}

// ApprovalBindings 审批窗口绑定
type ApprovalBindings struct {
	window *ApprovalWindow
}

// GetRequestID 获取请求 ID
func (b *ApprovalBindings) GetRequestID() string {
	data := b.window.GetData().(*ApprovalWindowData)
	return data.RequestID
}

// GetOperation 获取操作类型
func (b *ApprovalBindings) GetOperation() string {
	data := b.window.GetData().(*ApprovalWindowData)
	return data.Operation
}

// GetOperationName 获取操作显示名称
func (b *ApprovalBindings) GetOperationName() string {
	data := b.window.GetData().(*ApprovalWindowData)
	if data.OperationName != "" {
		return data.OperationName
	}
	return approval.GetOperationDisplayName(data.Operation)
}

// GetTarget 获取目标
func (b *ApprovalBindings) GetTarget() string {
	data := b.window.GetData().(*ApprovalWindowData)
	return data.Target
}

// GetRiskLevel 获取风险级别
func (b *ApprovalBindings) GetRiskLevel() string {
	data := b.window.GetData().(*ApprovalWindowData)
	return data.RiskLevel
}

// GetReason 获取原因
func (b *ApprovalBindings) GetReason() string {
	data := b.window.GetData().(*ApprovalWindowData)
	return data.Reason
}

// GetTimeout 获取超时时间
func (b *ApprovalBindings) GetTimeout() int {
	data := b.window.GetData().(*ApprovalWindowData)
	return data.TimeoutSeconds
}

// GetContext 获取上下文
func (b *ApprovalBindings) GetContext() map[string]string {
	data := b.window.GetData().(*ApprovalWindowData)
	return data.Context
}

// CloseWindow 关闭窗口
func (b *ApprovalBindings) CloseWindow() {
	fmt.Fprintf(os.Stderr, "[ApprovalBindings-%s] CloseWindow called - Note: Wails will close on return from wails.Run\n", b.window.ID)
	// 在 Wails v2 中，当 wails.Run 返回时窗口会自动关闭
	// 我们无法从 Go 代码中直接关闭窗口，只能通过返回让 Wails 知道应该关闭
}

// Errors
var (
	ErrInvalidData = &WindowError{Code: "INVALID_DATA", Message: "Invalid window data"}
)

// WindowError 窗口错误
type WindowError struct {
	Code    string
	Message string
}

func (e *WindowError) Error() string {
	return e.Message
}
