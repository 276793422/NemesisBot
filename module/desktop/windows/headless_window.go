package windows

import (
	"log"
	"time"

	"github.com/276793422/NemesisBot/module/desktop/websocket"
)

// RunHeadlessWindow 运行无窗口模式（用于测试通信流程）
func RunHeadlessWindow(windowID string, data *ApprovalWindowData, wsClient *websocket.WebSocketClient) error {
	log.Printf("[HeadlessWindow] Starting headless window: %s", windowID)

	// 模拟用户在 1 秒后自动批准
	go func() {
		log.Printf("[HeadlessWindow] Waiting 1 second before auto-approving...")
		time.Sleep(1 * time.Second)

		result := map[string]interface{}{
			"approved":   true,
			"reason":     "自动批准（测试模式）",
			"request_id": data.RequestID,
			"timestamp":  time.Now().Unix(),
		}

		log.Printf("[HeadlessWindow] Sending auto-approve result: %+v", result)

		// 通过新协议发送结果
		if err := wsClient.Notify("approval.submit", result); err != nil {
			log.Printf("[HeadlessWindow] Failed to send result: %v", err)
		}
	}()

	// 保持运行 5 秒让结果发送完成
	log.Printf("[HeadlessWindow] Keeping process alive for 5 seconds...")
	time.Sleep(5 * time.Second)

	log.Printf("[HeadlessWindow] Window completed: %s", windowID)
	return nil
}
