package main

import (
	"fmt"
	"strings"
)

// getDisplayWidth 计算字符串的显示宽度（中文=2，英文=1）
func getDisplayWidth(s string) int {
	width := 0
	for _, r := range s {
		if r < 128 {
			// ASCII字符（英文、数字、符号）
			width += 1
		} else {
			// 非ASCII字符（中文等）
			width += 2
		}
	}
	return width
}

// printHelpHeader 打印居中的帮助标题
func printHelpHeader() {
	const boxWidth = 67
	title := "TestAIServer 帮助系统"

	// 上边框
	fmt.Printf("╔%s╗\n", strings.Repeat("═", boxWidth-2))

	// 标题行 - 计算居中位置
	titleWidth := getDisplayWidth(title)
	leftPadding := (boxWidth - 2 - titleWidth) / 2
	rightPadding := boxWidth - 2 - titleWidth - leftPadding

	fmt.Printf("║%s%s%s║\n",
		strings.Repeat(" ", leftPadding),
		title,
		strings.Repeat(" ", rightPadding))

	// 下边框
	fmt.Printf("╚%s╝\n", strings.Repeat("═", boxWidth-2))
}

// HelpCategory 帮助分类
type HelpCategory struct {
	Name        string
	Description string
	ModelCount  int
}

var categories = []HelpCategory{
	{
		Name:        "基础响应模型",
		Description: "快速测试和消息验证",
		ModelCount:  2,
	},
	{
		Name:        "延迟测试模型",
		Description: "超时和长延迟场景测试",
		ModelCount:  2,
	},
	{
		Name:        "工具调用模型",
		Description: "工具执行和 RPC 通信",
		ModelCount:  3,
	},
	{
		Name:        "安全测试模型",
		Description: "文件操作和安全审批",
		ModelCount:  1,
	},
}

// ModelInfo 模型信息
type ModelInfo struct {
	ID          string
	Name        string
	Category    string
	Description string
	Features    []string
	Usage       string
}

var modelInfos = []ModelInfo{
	{
		ID:       "testai-1.1",
		Name:     "快速响应模型",
		Category: "基础响应模型",
		Description: "立即返回固定响应，用于基础功能测试",
		Features: []string{
			"0秒延迟",
			"固定响应: \"好的，我知道了\"",
			"测试基本消息处理流程",
		},
		Usage: "curl http://localhost:8080/v1/chat/completions -H \"Content-Type: application/json\" -d '{\"model\":\"testai-1.1\",\"messages\":[{\"role\":\"user\",\"content\":\"你好\"}]}'",
	},
	{
		ID:       "testai-2.0",
		Name:     "回显模型",
		Category: "基础响应模型",
		Description: "原样返回用户消息，用于验证消息传递",
		Features: []string{
			"0秒延迟",
			"返回用户最后一条消息",
			"测试消息格式和内容完整性",
		},
		Usage: "curl http://localhost:8080/v1/chat/completions -H \"Content-Type: application/json\" -d '{\"model\":\"testai-2.0\",\"messages\":[{\"role\":\"user\",\"content\":\"回显测试\"}]}'",
	},
	{
		ID:       "testai-1.2",
		Name:     "中等延迟模型",
		Category: "延迟测试模型",
		Description: "30秒延迟，用于测试中等超时场景",
		Features: []string{
			"30秒延迟",
			"固定响应: \"好的，我知道了\"",
			"测试 30 秒超时配置",
		},
		Usage: "curl http://localhost:8080/v1/chat/completions -H \"Content-Type: application/json\" -d '{\"model\":\"testai-1.2\",\"messages\":[{\"role\":\"user\",\"content\":\"测试延迟\"}]}'",
	},
	{
		ID:       "testai-1.3",
		Name:     "长延迟模型",
		Category: "延迟测试模型",
		Description: "300秒延迟（5分钟），用于测试长延迟和超时处理",
		Features: []string{
			"300秒延迟（5分钟）",
			"固定响应: \"好的，我知道了\"",
			"测试超时机制",
		},
		Usage: "curl http://localhost:8080/v1/chat/completions -H \"Content-Type: application/json\" -d '{\"model\":\"testai-1.3\",\"messages\":[{\"role\":\"user\",\"content\":\"测试超长延迟\"}]}'",
	},
	{
		ID:       "testai-3.0",
		Name:     "集群通信模型",
		Category: "工具调用模型",
		Description: "检测 PEER_CHAT 标记，返回 cluster_rpc 工具调用",
		Features: []string{
			"0秒延迟",
			"触发 peer_chat 工具",
			"测试集群间通信",
		},
		Usage: "发送消息: <PEER_CHAT>{\"peer_id\":\"agent-b\",\"content\":\"测试消息\"}</PEER_CHAT>",
	},
	{
		ID:       "testai-4.2",
		Name:     "客户端休眠模型（30秒）",
		Category: "工具调用模型",
		Description: "返回 sleep 工具调用（30秒），测试工具执行流程",
		Features: []string{
			"第一轮: 返回 sleep(30秒)",
			"第二轮: 返回 \"工作完成\"",
			"测试客户端工具调用",
		},
		Usage: "发送消息: \"执行一个30秒的任务\"",
	},
	{
		ID:       "testai-4.3",
		Name:     "客户端休眠模型（300秒）",
		Category: "工具调用模型",
		Description: "返回 sleep 工具调用（300秒），测试长时间工具执行",
		Features: []string{
			"第一轮: 返回 sleep(300秒)",
			"第二轮: 返回 \"工作完成\"",
			"测试 10 分钟 HTTP 超时配置",
		},
		Usage: "发送消息: \"执行一个5分钟的任务\"",
	},
	{
		ID:       "testai-5.0",
		Name:     "安全文件操作模型",
		Category: "安全测试模型",
		Description: "返回文件操作工具调用，用于测试安全审批功能",
		Features: []string{
			"支持 7 种文件操作:",
			"  • file_read    - 读取文件（HIGH）",
			"  • file_write   - 写入文件（HIGH）",
			"  • file_delete  - 删除文件（CRITICAL）",
			"  • file_append  - 追加文件（MEDIUM）",
			"  • dir_create   - 创建目录（LOW）",
			"  • dir_delete   - 删除目录（HIGH）",
			"  • dir_list     - 列出目录（LOW）",
			"格式: <FILE_OP>{\"operation\":\"file_delete\",\"path\":\"/etc/passwd\",\"risk_level\":\"CRITICAL\"}</FILE_OP>",
			"触发安全审批对话框",
		},
		Usage: "发送: <FILE_OP>{\"operation\":\"file_delete\",\"path\":\"/etc/passwd\",\"risk_level\":\"CRITICAL\"}</FILE_OP>",
	},
}

// PrintMainHelp 打印主帮助（概览）
func PrintMainHelp() {
	printHelpHeader()
	fmt.Println()
	fmt.Println("📊 模型分类总览")
	fmt.Println("─────────────────────────────────────────────────────────")

	for i, cat := range categories {
		fmt.Printf("%d. %s\n", i+1, cat.Name)
		fmt.Printf("   %s\n", cat.Description)
		fmt.Printf("   模型数量: %d\n", cat.ModelCount)
		fmt.Println()
	}

	fmt.Println("─────────────────────────────────────────────────────────")
	fmt.Println("💡 使用提示:")
	fmt.Println("   ./testaiserver.exe help          - 显示此概览")
	fmt.Println("   ./testaiserver.exe help categories  - 显示分类详情")
	fmt.Println("   ./testaiserver.exe models        - 显示所有模型列表")
	fmt.Println("   ./testaiserver.exe help <模型名>  - 显示特定模型帮助")
	fmt.Println("   ./testaiserver.exe api           - 显示 API 使用说明")
	fmt.Println()
	fmt.Println("📖 示例1:")
	fmt.Println("   ./testaiserver.exe help testai-5.0    # 查看安全模型帮助")
	fmt.Println("   ./testaiserver.exe help categories     # 查看分类详情")
	fmt.Println()
	fmt.Println("📖 示例2:")
	fmt.Println("   nemesisbot model add --model testai/testai-1.1 --key YOUR_API_KEY --base http://localhost:8080/v1 --default    # 模型测试用，回显内容")
	fmt.Println()
}

// PrintCategoriesHelp 打印分类详情
func PrintCategoriesHelp() {
	printHelpHeader()
	fmt.Println()
	fmt.Println("📁 模型分类详情")
	fmt.Println("═════════════════════════════════════════════════════════════════════════════")
	fmt.Println()

	const boxWidth = 62

	for _, cat := range categories {
		// 上边框
		fmt.Printf("┌─ %s", cat.Name)
		padding := boxWidth - 3 - getDisplayWidth(cat.Name) - 1
		fmt.Printf("%s┐\n", strings.Repeat("─", padding))

		// 描述
		fmt.Printf("│ %s", cat.Description)
		padding = boxWidth - 2 - getDisplayWidth(cat.Description) - 1
		fmt.Printf("%s│\n", strings.Repeat(" ", padding))

		// 模型数量
		modelCountText := fmt.Sprintf("模型: %d 个", cat.ModelCount)
		fmt.Printf("│ %s", modelCountText)
		padding = boxWidth - 2 - getDisplayWidth(modelCountText) - 1
		fmt.Printf("%s│\n", strings.Repeat(" ", padding))

		// 下边框
		fmt.Printf("└%s┘\n", strings.Repeat("─", boxWidth-2))
		fmt.Println()

		// 显示该分类下的模型
		for _, model := range modelInfos {
			if model.Category == cat.Name {
				fmt.Printf("   • %s - %s\n", model.ID, model.Name)
				fmt.Printf("     └─ %s\n", model.Description)
			}
		}
		fmt.Println()
	}
}

// PrintModelHelp 打印特定模型的详细帮助
func PrintModelHelp(modelID string) {
	printHelpHeader()
	fmt.Println()

	// 查找模型
	var found *ModelInfo
	for _, m := range modelInfos {
		if m.ID == modelID {
			found = &m
			break
		}
	}

	if found == nil {
		fmt.Printf("❌ 未找到模型: %s\n\n", modelID)
		fmt.Println("💡 可用模型列表:")
		for _, m := range modelInfos {
			fmt.Printf("   - %s\n", m.ID)
		}
		fmt.Println()
		PrintAllModelsBrief()
		return
	}

	// 显示模型详细信息
	fmt.Printf("🤖 %s - %s\n", found.ID, found.Name)
	fmt.Println("═════════════════════════════════════════════════════════════════════════════")
	fmt.Println()
	fmt.Printf("📂 分类: %s\n", found.Category)
	fmt.Printf("📝 描述: %s\n", found.Description)
	fmt.Println()
	fmt.Println("✨ 功能特性:")
	for _, feature := range found.Features {
		fmt.Printf("   • %s\n", feature)
	}
	fmt.Println()

	if found.Usage != "" {
		fmt.Println("📖 使用示例:")
		fmt.Printf("   %s\n", found.Usage)
		fmt.Println()
	}

	// 检查是否需要特殊说明
	if found.ID == "testai-5.0" {
		fmt.Println("🔐 安全测试说明:")
		fmt.Println("   此模型专门用于测试 NemesisBot 的安全审批功能。")
		fmt.Println("   当操作的风险级别 >= MinRiskLevel 时，会触发安全审批对话框。")
		fmt.Println()
		fmt.Println("   支持的文件操作及默认风险级别:")
		fmt.Println("   • file_read    - HIGH     (需要审批)")
		fmt.Println("   • file_write   - HIGH     (需要审批)")
		fmt.Println("   • file_delete  - CRITICAL (需要审批)")
		fmt.Println("   • file_append  - MEDIUM   (可能需要审批)")
		fmt.Println("   • dir_create   - LOW      (可能需要审批)")
		fmt.Println("   • dir_delete   - HIGH     (需要审批)")
		fmt.Println("   • dir_list     - LOW      (通常无需审批)")
		fmt.Println()
		fmt.Println("   配置示例:")
		fmt.Println("   nemesisbot model add --model test/testai-5.0 --base http://127.0.0.1:8080/v1 --key test-key")
		fmt.Println()
	} else if found.ID == "testai-3.0" {
		fmt.Println("🌐 集群通信说明:")
		fmt.Println("   此模型用于测试 NemesisBot 集群间的 peer_chat 功能。")
		fmt.Println("   通过检测 <PEER_CHAT> 标记，自动触发 cluster_rpc 工具调用。")
		fmt.Println()
		fmt.Println("   使用流程:")
		fmt.Println("   1. 本地 Agent A 发送包含 PEER_CHAT 标记的消息")
		fmt.Println("   2. testai-3.0 返回 cluster_rpc 工具调用")
		fmt.Println("   3. Agent A 执行工具，发送 peer_chat 给远端 Agent B")
		fmt.Println("   4. Agent B 处理请求并返回响应")
		fmt.Println()
	}

	fmt.Println("═══════════════════════════════════════════════════════════════════════════")
	fmt.Println()
}

// PrintAllModelsBrief 打印所有模型简要列表
func PrintAllModelsBrief() {
	fmt.Println("📋 所有模型列表")
	fmt.Println("─────────────────────────────────────────────────────────")
	fmt.Println()

	for i, model := range modelInfos {
		fmt.Printf("%2d. %-12s  %s\n", i+1, model.ID, model.Name)
		fmt.Printf("    分类: %s\n", model.Category)
		fmt.Println()
	}
}

// PrintAPIHelp 打印 API 使用说明
func PrintAPIHelp() {
	printHelpHeader()
	fmt.Println()
	fmt.Println("🌐 API 接口说明")
	fmt.Println("═════════════════════════════════════════════════════════════════════════════")
	fmt.Println()

	fmt.Println("📍 基础端点:")
	fmt.Println()
	fmt.Println("1. GET  /v1/models")
	fmt.Println("   功能: 列出所有可用模型")
	fmt.Println("   响应: 模型列表（OpenAI 格式）")
	fmt.Println()
	fmt.Println("2. POST /v1/chat/completions")
	fmt.Println("   功能: 聊天补全接口")
	fmt.Println("   支持流式响应: stream=true")
	fmt.Println("   请求体示例:")
	fmt.Println("   {")
	fmt.Println("     \"model\": \"testai-1.1\",")
	fmt.Println("     \"messages\": [")
	fmt.Println("       {\"role\": \"user\", \"content\": \"你好\"}")
	fmt.Println("     ],")
	fmt.Println("     \"stream\": false  // true 启用流式响应")
	fmt.Println("   }")
	fmt.Println()
	fmt.Println("3. GET  /v1/help")
	fmt.Println("   功能: 获取帮助信息")
	fmt.Println()

	fmt.Println("─────────────────────────────────────────────────────────")
	fmt.Println("💡 快速测试:")
	fmt.Println()
	fmt.Println("# 列出所有模型")
	fmt.Println("curl http://localhost:8080/v1/models")
	fmt.Println()
	fmt.Println("# 发送聊天请求（非流式）")
	fmt.Println("curl http://localhost:8080/v1/chat/completions \\")
	fmt.Println("  -H \"Content-Type: application/json\" \\")
	fmt.Println("  -d '{\"model\":\"testai-1.1\",\"messages\":[{\"role\":\"user\",\"content\":\"你好\"}]}'")
	fmt.Println()
	fmt.Println("# 发送聊天请求（流式响应）⭐")
	fmt.Println("curl -N http://localhost:8080/v1/chat/completions \\")
	fmt.Println("  -H \"Content-Type: application/json\" \\")
	fmt.Println("  -d '{\"model\":\"testai-2.0\",\"messages\":[{\"role\":\"user\",\"content\":\"你好\"}],\"stream\":true}'")
	fmt.Println()

	fmt.Println("═════════════════════════════════════════════════════════════════════════════")
	fmt.Println()
}

// PrintQuickReference 打印快速参考
func PrintQuickReference() {
	printHelpHeader()
	fmt.Println()
	fmt.Println("⚡ 快速参考")
	fmt.Println("─────────────────────────────────────────────────────────")
	fmt.Println()

	fmt.Println("🚀 快速开始:")
	fmt.Println("   1. 启动服务器: ./testaiserver.exe")
	fmt.Println("   2. 查看帮助:     ./testaiserver.exe --help")
	fmt.Println("   3. 查看模型:     ./testaiserver.exe models")
	fmt.Println("   4. 模型帮助:     ./testaiserver.exe help testai-5.0")
	fmt.Println()

	fmt.Println("📦 常用模型:")
	fmt.Println("   • testai-1.1  - 基础测试（快速响应）")
	fmt.Println("   • testai-2.0  - 消息回显（验证传递）")
	fmt.Println("   • testai-3.0  - 集群通信（peer_chat）")
	fmt.Println("   • testai-5.0  - 安全测试（文件操作）⭐")
	fmt.Println()

	fmt.Println("🔧 高级功能:")
	fmt.Println("   • testai-1.2/1.3  - 延迟测试（30秒/300秒）")
	fmt.Println("   • testai-4.2/4.3  - 工具调用（sleep 30秒/300秒）")
	fmt.Println()

	fmt.Println("─────────────────────────────────────────────────────────")
	fmt.Println()
}

// ShowHelp 根据 args 显示不同层级的帮助
func ShowHelp(args []string) {
	if len(args) == 0 {
		// 无参数，显示主帮助
		PrintMainHelp()
	} else if len(args) == 1 {
		arg := strings.ToLower(args[0])

		switch arg {
		case "categories":
			PrintCategoriesHelp()
		case "models":
			fmt.Println()
			PrintAllModelsBrief()
		case "api":
			PrintAPIHelp()
		case "quick", "q", "reference", "ref":
			PrintQuickReference()
		default:
			// 尝试作为模型 ID
			PrintModelHelp(arg)
		}
	} else {
		// 有多个参数，第一个作为模型 ID
		PrintModelHelp(args[0])
	}
}
