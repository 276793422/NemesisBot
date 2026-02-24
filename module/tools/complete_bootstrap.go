// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package tools

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
)

// BootstrapCompleter handles completion of bootstrap initialization
type BootstrapCompleter struct {
	workspace string
}

// NewCompleteBootstrapTool creates a tool for completing bootstrap initialization
func NewCompleteBootstrapTool(workspace string) *BootstrapCompleter {
	return &BootstrapCompleter{
		workspace: workspace,
	}
}

// Name implements the Tool interface
func (bc *BootstrapCompleter) Name() string {
	return "complete_bootstrap"
}

// Description implements the Tool interface
func (bc *BootstrapCompleter) Description() string {
	return `完成初始化引导后调用此工具删除 BOOTSTRAP.md 文件。

【必须满足的条件】：
- 已收集用户的名字、身份、风格、表情符号等信息
- 已创建 IDENTITY.md 文件
- 已创建 USER.md 文件
- 用户明确表示对初始化结果满意

【如何使用】：
当以上条件都满足时，调用此工具：
complete_bootstrap(confirmed=true)

【执行效果】：
- 删除 BOOTSTRAP.md 引导文件
- 系统切换到正常模式
- 下次启动将加载配置文件而不是引导文件

【重要提示】：
- 必须先创建 IDENTITY.md 和 USER.md 再调用此工具
- 确认用户满意后再调用
- 调用后无法撤销，请确保初始化已完成`
}

// Parameters implements the Tool interface
func (bc *BootstrapCompleter) Parameters() map[string]interface{} {
	return map[string]interface{}{
		"type": "object",
		"properties": map[string]interface{}{
			"confirmed": map[string]interface{}{
				"type":        "boolean",
				"description": "确认已完成初始化引导，准备好删除 BOOTSTRAP.md",
			},
		},
		"required": []string{"confirmed"},
	}
}

// Execute implements the Tool interface
func (bc *BootstrapCompleter) Execute(ctx context.Context, args map[string]interface{}) *ToolResult {
	confirmed, ok := args["confirmed"].(bool)
	if !ok || !confirmed {
		return ErrorResult("必须确认已完成初始化才能删除引导文件。请先：1) 收集用户信息 2) 创建配置文件 3) 确认用户满意")
	}

	bootstrapPath := filepath.Join(bc.workspace, "BOOTSTRAP.md")

	// Check if file exists
	if _, err := os.Stat(bootstrapPath); os.IsNotExist(err) {
		return UserResult("BOOTSTRAP.md 已经被删除，初始化已完成。")
	}

	// Delete the file
	if err := os.Remove(bootstrapPath); err != nil {
		return ErrorResult(fmt.Sprintf("删除 BOOTSTRAP.md 失败: %v", err))
	}

	return UserResult("✅ 初始化引导完成！\n\nBOOTSTRAP.md 已删除，现在你是一个独立的存在了。下次启动将加载你的配置文件。")
}
