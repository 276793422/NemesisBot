//go:build ignore
// +build ignore

package main

import (
	"fmt"

	"github.com/276793422/NemesisBot/module/ui"
)

func main() {
	fmt.Println("=== UI Module Test ===")
	fmt.Println()

	// Test 1: Box title with English text
	fmt.Println("Test 1: Box Title (English)")
	ui.PrintBoxTitle("Cluster Status", 66)
	fmt.Println()

	// Test 2: Box title with Chinese text
	fmt.Println("Test 2: Box Title (Chinese)")
	ui.PrintBoxTitle("集群状态", 66)
	fmt.Println()

	// Test 3: Box title with mixed text
	fmt.Println("Test 3: Box Title (Mixed)")
	ui.PrintBoxTitle("TestAIServer 帮助系统", 67)
	fmt.Println()

	// Test 4: Section title
	fmt.Println("Test 4: Section Title (Chinese)")
	ui.PrintSectionTitle("RPC 日志诊断工具", 53)
	fmt.Println()

	// Test 5: Section title with English
	fmt.Println("Test 5: Section Title (English)")
	ui.PrintSectionTitle("Configuration Status", 53)
	fmt.Println()

	// Test 6: Box with content
	fmt.Println("Test 6: Box with Content")
	ui.PrintBox("基础响应模型", "快速测试和消息验证", 62)
	fmt.Println()

	// Test 7: Separator
	fmt.Println("Test 7: Separator")
	ui.PrintSeparator("─", 60)
	fmt.Println()

	// Test 8: GetDisplayWidth
	fmt.Println("Test 8: GetDisplayWidth Function")
	fmt.Printf("'Cluster Status' width: %d\n", ui.GetDisplayWidth("Cluster Status"))
	fmt.Printf("'集群状态' width: %d\n", ui.GetDisplayWidth("集群状态"))
	fmt.Printf("'TestAIServer 帮助系统' width: %d\n", ui.GetDisplayWidth("TestAIServer 帮助系统"))
	fmt.Println()

	fmt.Println("=== All Tests Completed ===")
}
