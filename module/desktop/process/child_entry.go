//go:build !cross_compile

package process

import (
	"encoding/json"
	"fmt"
	"os"
	"time"

	"github.com/276793422/NemesisBot/module/desktop/websocket"
	"github.com/276793422/NemesisBot/module/desktop/windows"
)

// childLog 写日志到 stderr，避免干扰管道通信
func childLog(format string, args ...interface{}) {
	fmt.Fprintf(os.Stderr, "[Child] "+format+"\n", args...)
}

// HasChildModeFlag 检查是否有 --multiple 参数
func HasChildModeFlag() bool {
	for _, arg := range os.Args {
		if arg == "--multiple" {
			return true
		}
	}
	return false
}

// GetChildID 获取 --child-id 参数
func GetChildID() string {
	for i, arg := range os.Args {
		if arg == "--child-id" && i+1 < len(os.Args) {
			return os.Args[i+1]
		}
	}
	return ""
}

// GetWindowType 获取 --window-type 参数
func GetWindowType() string {
	for i, arg := range os.Args {
		if arg == "--window-type" && i+1 < len(os.Args) {
			return os.Args[i+1]
		}
	}
	return ""
}

// RunChildMode 运行子进程模式
func RunChildMode() error {
	// 注意：不要使用 log.Printf，因为它会干扰管道的 JSON 通信
	// 只在出错时写到 stderr

	// 1. 检查参数
	if !HasChildModeFlag() {
		return fmt.Errorf("not in child mode")
	}

	childID := GetChildID()
	if childID == "" {
		return fmt.Errorf("child-id not specified")
	}

	windowType := GetWindowType()
	if windowType == "" {
		return fmt.Errorf("window-type not specified")
	}

	// Allow forcing headless mode via environment variable (for testing)
	if os.Getenv("NEMESISBOT_FORCE_HEADLESS") == "1" && windowType == "approval" {
		windowType = "headless"
		childLog("Forced headless mode via NEMESISBOT_FORCE_HEADLESS=1")
	}

	childLog("Child ID: %s, Window Type: %s", childID, windowType)

	// 2. 创建标准输入输出包装器
	// 子进程从 stdin 读取父进程发送的数据，通过 stdout 写入响应
	stdin := &ReadCloser{Decoder: json.NewDecoder(os.Stdin), reader: os.Stdin}
	stdout := &WriteCloser{Encoder: json.NewEncoder(os.Stdout), writer: os.Stdout}

	// 3. 等待握手
	childLog("Waiting for handshake...")
	result, err := ChildHandshake(stdin, stdout)
	if err != nil {
		childLog("Handshake failed: %v", err)
		return err
	}

	if !result.Success {
		return fmt.Errorf("handshake failed")
	}

	childLog("Handshake completed")

	// 4. 接收 WebSocket 密钥
	childLog("Waiting for WebSocket key...")
	key, port, path, err := ReceiveWSKey(stdin, stdout)
	if err != nil {
		childLog("Receive WS key failed: %v", err)
		return err
	}

	childLog("WS key received: key=%s, port=%d, path=%s", key, port, path)

	// 5. 创建 WebSocket 客户端
	wsKey := &websocket.WebSocketKey{
		Key:  key,
		Port: port,
		Path: path,
	}

	wsClient := websocket.NewWebSocketClient(wsKey)

	// 6. 连接 WebSocket
	childLog("Connecting to WebSocket...")

	if err := wsClient.Connect(); err != nil {
		childLog("WS connect failed: %v", err)
		return err
	}

	childLog("WebSocket connected")

	// 7. 接收窗口数据
	childLog("Waiting for window data...")
	windowData, err := ReceiveWindowData(stdin, stdout)
	if err != nil {
		childLog("Receive window data failed: %v", err)
		return err
	}

	childLog("Window data received")

	// 8. 启动 Wails 窗口
	return runWailsWindow(childID, windowType, windowData, wsClient)
}

// runWailsWindow 运行 Wails 窗口
func runWailsWindow(childID, windowType string, windowData interface{}, wsClient *websocket.WebSocketClient) error {
	childLog("Starting Wails window: type=%s", windowType)

	switch windowType {
	case "approval":
		// 转换数据为 ApprovalWindowData
		dataMap, ok := windowData.(map[string]interface{})
		if !ok {
			return fmt.Errorf("invalid window data type")
		}

		data := &windows.ApprovalWindowData{
			RequestID:      dataMap["request_id"].(string),
			Operation:      dataMap["operation"].(string),
			OperationName:  dataMap["operation_name"].(string),
			Target:         dataMap["target"].(string),
			RiskLevel:      dataMap["risk_level"].(string),
			Reason:         dataMap["reason"].(string),
			TimeoutSeconds: int(dataMap["timeout_seconds"].(float64)),
			Context:        make(map[string]string),
			Timestamp:      int64(dataMap["timestamp"].(float64)),
		}

		// 复制 context
		if ctxMap, ok := dataMap["context"].(map[string]interface{}); ok {
			for k, v := range ctxMap {
				if str, ok := v.(string); ok {
					data.Context[k] = str
				}
			}
		}

		// 运行审批窗口（使用真正的 Wails 窗口）
		childLog("Starting Wails GUI window")
		return windows.RunApprovalWindow(childID, data, wsClient)

	case "headless":
		// 转换数据为 ApprovalWindowData（与 approval 相同的数据格式）
		dataMap, ok := windowData.(map[string]interface{})
		if !ok {
			return fmt.Errorf("invalid window data type")
		}

		data := &windows.ApprovalWindowData{
			RequestID:      dataMap["request_id"].(string),
			Operation:      dataMap["operation"].(string),
			OperationName:  dataMap["operation_name"].(string),
			Target:         dataMap["target"].(string),
			RiskLevel:      dataMap["risk_level"].(string),
			Reason:         dataMap["reason"].(string),
			TimeoutSeconds: int(dataMap["timeout_seconds"].(float64)),
			Context:        make(map[string]string),
			Timestamp:      int64(dataMap["timestamp"].(float64)),
		}

		if ctxMap, ok := dataMap["context"].(map[string]interface{}); ok {
			for k, v := range ctxMap {
				if str, ok := v.(string); ok {
					data.Context[k] = str
				}
			}
		}

		childLog("Starting headless window (auto-approve)")
		return windows.RunHeadlessWindow(childID, data, wsClient)

	default:
		return fmt.Errorf("unknown window type: %s", windowType)
	}
}

// runWindow 运行窗口（旧版本，已弃用）
func runWindow(childID, windowType string, wsClient *websocket.WebSocketClient) error {
	childLog("Starting window: type=%s", windowType)

	switch windowType {
	case "approval":
		// 创建审批窗口数据
		data := &windows.ApprovalWindowData{
			RequestID:      childID,
			Operation:      "test_operation",
			OperationName:  "测试操作",
			Target:         "C:\\Temp\\test.txt",
			RiskLevel:      "HIGH",
			Reason:         "测试审批流程",
			TimeoutSeconds: 30,
			Context:        make(map[string]string),
			Timestamp:      time.Now().Unix(),
		}

		// 运行审批窗口
		return windows.RunApprovalWindow(childID, data, wsClient)

	default:
		return fmt.Errorf("unknown window type: %s", windowType)
	}
}
