//go:build ignore

// +build ignore

// Web Error Response Test
// 测试 LLM 错误是否正确返回给 web 客户端

package main

import (
	"fmt"
	"time"
)

func main() {
	fmt.Println("╔════════════════════════════════════════════════════════════╗")
	fmt.Println("║       Web 客户端错误响应问题分析报告                        ║")
	fmt.Println("╚════════════════════════════════════════════════════════════╝")
	fmt.Println()

	fmt.Println("📋 问题描述")
	fmt.Println("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")
	fmt.Println()
	fmt.Println("当 LLM 调用失败时（如 429 速率限制错误），agent 日志显示：")
	fmt.Println()
	fmt.Println(`  [INFO] agent: Processing message from web:web:xxx: ...`)
	fmt.Println(`  [INFO] agent: Routed message {agent_id=main, ...}`)
	fmt.Println(`  [ERROR] agent: LLM call failed {..., error=API request failed: Status: 429}`)
	fmt.Println()
	fmt.Println("但 web 客户端没有收到任何错误消息。")
	fmt.Println()

	fmt.Println("🔍 代码流程分析")
	fmt.Println("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")
	fmt.Println()

	fmt.Println("1️⃣ 消息接收 (module/agent/loop.go:266-274)")
	fmt.Println("   ┌─────────────────────────────────────────────────────────┐")
	fmt.Println("   │ msg := al.bus.ConsumeInbound(ctx)                       │")
	fmt.Println("   │ response, err := al.processMessage(ctx, msg)            │")
	fmt.Println("   │ if err != nil {                                         │")
	fmt.Println("   │     response = fmt.Sprintf(\"Error processing message: %v\", err)│")
	fmt.Println("   │ }                                                       │")
	fmt.Println("   └─────────────────────────────────────────────────────────┘")
	fmt.Println()

	fmt.Println("2️⃣ 发送响应 (module/agent/loop.go:276-296)")
	fmt.Println("   ┌─────────────────────────────────────────────────────────┐")
	fmt.Println("   │ if response != \"\" {                                     │")
	fmt.Println("   │     al.bus.PublishOutbound(bus.OutboundMessage{        │")
	fmt.Println("   │         Channel: msg.Channel,  // \"web\"               │")
	fmt.Println("   │         ChatID:  msg.ChatID,    // \"web:session_id\"   │")
	fmt.Println("   │         Content: response,                              │")
	fmt.Println("   │     })                                                  │")
	fmt.Println("   │ }                                                       │")
	fmt.Println("   └─────────────────────────────────────────────────────────┘")
	fmt.Println()

	fmt.Println("3️⃣ LLM 错误处理 (module/agent/loop.go:643-658)")
	fmt.Println("   ┌─────────────────────────────────────────────────────────┐")
	fmt.Println("   │ finalContent, iteration, err := al.runLLMIteration(...)  │")
	fmt.Println("   │ if err != nil {                                         │")
	fmt.Println("   │     // Log error to request logger                      │")
	fmt.Println("   │     return \"\", err  // ⚠️ 返回空字符串和错误            │")
	fmt.Println("   │ }                                                       │")
	fmt.Println("   └─────────────────────────────────────────────────────────┘")
	fmt.Println()

	fmt.Println("✅ 理论上正确的流程")
	fmt.Println("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")
	fmt.Println()
	fmt.Println("1. processMessage 返回 (\"\", error)")
	fmt.Println("2. Run() 捕获错误，创建响应:")
	fmt.Println("   `Error processing message: LLM call failed after retries: ...`")
	fmt.Println("3. PublishOutbound 发送到 web 通道")
	fmt.Println("4. web.go 的 Send() 方法发送到 WebSocket")
	fmt.Println("5. 客户端收到错误消息")
	fmt.Println()

	fmt.Println("❓ 可能的问题")
	fmt.Println("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")
	fmt.Println()

	fmt.Println("问题 1: alreadySent 标志导致跳过发送")
	fmt.Println("   位置: module/agent/loop.go:280-288")
	fmt.Println("   ┌─────────────────────────────────────────────────────────┐")
	fmt.Println("   │ alreadySent := false                                    │")
	fmt.Println("   │ if mt, ok := tool.(*tools.MessageTool); ok {            │")
	fmt.Println("   │     alreadySent = mt.HasSentInRound()  // ← 如果为 true│")
	fmt.Println("   │ }                                                       │")
	fmt.Println("   │ if !alreadySent {  // ← 只有 alreadySent=false 才发送   │")
	fmt.Println("   │     al.bus.PublishOutbound(...)                        │")
	fmt.Println("   │ }                                                       │")
	fmt.Println("   └─────────────────────────────────────────────────────────┘")
	fmt.Println()

	fmt.Println("问题 2: WebSocket 会话断开")
	fmt.Println("   如果客户端与服务器之间的 WebSocket 连接断开，")
	fmt.Println("   服务器无法发送消息，但不会立即检测到。")
	fmt.Println()

	fmt.Println("问题 3: Session ID 不匹配")
	fmt.Println("   如果 peers.toml 重新生成，session ID 可能改变，")
	fmt.Println("   导致无法找到对应的 WebSocket 连接。")
	fmt.Println()

	fmt.Println("问题 4: 错误响应被过滤")
	fmt.Println("   某些中间件或处理层可能过滤了错误响应。")
	fmt.Println()

	fmt.Println("🔧 建议的调试步骤")
	fmt.Println("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")
	fmt.Println()

	fmt.Println("1. 检查 WebSocket 连接状态")
	fmt.Println("   在浏览器开发者工具的 Network 标签查看 WS 连接")
	fmt.Println()

	fmt.Println("2. 添加日志验证发送流程")
	fmt.Println("   在 module/agent/loop.go:291 之前添加日志：")
	fmt.Println("   `logger.InfoCF(\"agent\", \"Publishing outbound response\", ...)`")
	fmt.Println()

	fmt.Println("3. 检查 alreadySent 标志")
	fmt.Println("   在 module/agent/loop.go:285 之后添加日志：")
	fmt.Println("   `logger.DebugCF(\"agent\", \"alreadySent flag\", ...)`")
	fmt.Println()

	fmt.Println("4. 验证错误响应内容")
	fmt.Println("   确认 `response` 变量在发送前不为空")
	fmt.Println()

	fmt.Println("📝 总结")
	fmt.Println("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")
	fmt.Println()
	fmt.Println("代码逻辑上是正确的：错误应该被捕获并转换为响应消息，")
	fmt.Println("然后通过消息总线发送到 web 客户端。")
	fmt.Println()
	fmt.Println("但实际情况是 web 客户端没有收到消息，可能原因：")
	fmt.Println("  1. alreadySent 标志错误地设置为 true（最可能）")
	fmt.Println("  2. WebSocket 连接断开")
	fmt.Println("  3. Session ID 不匹配")
	fmt.Println("  4. 中间层过滤了错误响应")
	fmt.Println()
	fmt.Println("建议优先检查 alreadySent 标志的设置逻辑。")
	fmt.Println()

	time.Sleep(100 * time.Millisecond)
}
