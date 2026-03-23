//go:build ignore
// +build ignore

package main

import (
	"encoding/json"
	"fmt"
	"log"
	"strings"
	"time"

	"github.com/gorilla/websocket"
)

type ClientMessage struct {
	Type      string `json:"type"`
	Content   string `json:"content"`
	Timestamp string `json:"timestamp,omitempty"`
}

type ServerMessage struct {
	Type      string `json:"type"`
	Role      string `json:"role,omitempty"`
	Content   string `json:"content,omitempty"`
	Timestamp string `json:"timestamp,omitempty"`
	Error     string `json:"error,omitempty"`
}

func main() {
	// 连接 WebSocket
	wsURL := "ws://127.0.0.1:49001/ws"
	fmt.Printf("连接到 %s...\n", wsURL)

	conn, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err != nil {
		log.Fatal("连接失败:", err)
	}
	defer conn.Close()

	fmt.Println("✅ 已连接")

	// 设置读取超时
	conn.SetReadDeadline(time.Now().Add(30 * time.Second))

	// 发送测试消息
	testMsg := ClientMessage{
		Type:      "message",
		Content:   "你好，请简单介绍一下自己",
		Timestamp: time.Now().Format(time.RFC3339),
	}

	fmt.Printf("\n📤 发送消息: %s\n", testMsg.Content)

	jsonData, err := json.Marshal(testMsg)
	if err != nil {
		log.Fatal("JSON 编码失败:", err)
	}

	err = conn.WriteMessage(websocket.TextMessage, jsonData)
	if err != nil {
		log.Fatal("发送失败:", err)
	}

	fmt.Println("✅ 消息已发送\n")

	// 接收响应（可能收到多条消息）
	fmt.Println("⏳ 等待响应...")

	messageCount := 0
	maxMessages := 5 // 最多接收 5 条消息

	for messageCount < maxMessages {
		conn.SetReadDeadline(time.Now().Add(30 * time.Second))

		_, response, err := conn.ReadMessage()
		if err != nil {
			if messageCount == 0 {
				log.Fatal("接收失败:", err)
			}
			break // 已收到消息，退出循环
		}

		var serverMsg ServerMessage
		err = json.Unmarshal(response, &serverMsg)
		if err != nil {
			log.Printf("JSON 解码失败: %v\n", err)
			continue
		}

		messageCount++

		// 显示响应
		fmt.Printf("\n📥 收到第 %d 条消息:\n", messageCount)
		fmt.Printf("   类型: %s\n", serverMsg.Type)
		fmt.Printf("   角色: %s\n", serverMsg.Role)
		if serverMsg.Error != "" {
			fmt.Printf("   错误: %s\n", serverMsg.Error)
		}
		if serverMsg.Content != "" {
			content := serverMsg.Content
			if len(content) > 200 {
				content = content[:200] + "..."
			}
			fmt.Printf("   内容: %s\n", content)
		}

		// 如果收到 assistant 的响应，说明测试成功
		if serverMsg.Type == "message" && serverMsg.Role == "assistant" {
			fmt.Println("\n" + strings.Repeat("=", 60))
			fmt.Println("✅ 测试通过：消息收发功能正常")
			fmt.Printf("✅ 收到 AI 响应（%d 字符）\n", len(serverMsg.Content))
			fmt.Println(strings.Repeat("=", 60))
			return
		}
	}

	// 如果循环结束仍未收到 assistant 响应
	fmt.Println("\n" + strings.Repeat("=", 60))
	fmt.Println("⚠️  测试警告：未收到 AI 响应")
	fmt.Printf("ℹ️  共收到 %d 条系统消息\n", messageCount)
	fmt.Println(strings.Repeat("=", 60))
}
