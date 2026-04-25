// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package agent

import (
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"time"

	"github.com/276793422/NemesisBot/module/logger"
	"github.com/276793422/NemesisBot/module/path"
	"github.com/276793422/NemesisBot/module/providers"
	"github.com/276793422/NemesisBot/module/skills"
	"github.com/276793422/NemesisBot/module/tools"
)

type ContextBuilder struct {
	workspace    string
	skillsLoader *skills.SkillsLoader
	memory       *MemoryStore
	tools        *tools.ToolRegistry // Direct reference to tool registry
}

func getGlobalConfigDir() string {
	return path.DefaultPathManager().HomeDir()
}

func NewContextBuilder(workspace string) *ContextBuilder {
	// global skills: ~/.nemesisbot/workspace/skills/
	// builtin skills: (currently unused, reserved for future embedded skills)
	globalSkillsDir := filepath.Join(getGlobalConfigDir(), "workspace", "skills")
	builtinSkillsDir := "" // Reserved for embedded skills in the future

	sl := skills.NewSkillsLoader(workspace, globalSkillsDir, builtinSkillsDir)
	sl.EnableSecurity()

	return &ContextBuilder{
		workspace:    workspace,
		skillsLoader: sl,
		memory:       NewMemoryStore(workspace),
	}
}

// SetToolsRegistry sets the tools registry for dynamic tool summary generation.
func (cb *ContextBuilder) SetToolsRegistry(registry *tools.ToolRegistry) {
	cb.tools = registry
}

func (cb *ContextBuilder) getIdentity() string {
	now := time.Now().Format("2006-01-02 15:04 (Monday)")
	workspacePath, _ := filepath.Abs(filepath.Join(cb.workspace))
	runtime := fmt.Sprintf("%s %s, Go %s", runtime.GOOS, runtime.GOARCH, runtime.Version())

	// Build tools section dynamically
	toolsSection := cb.buildToolsSection()

	return fmt.Sprintf(`# 当前时间
%s

## 运行环境
%s

## 工作区
你的工作区位于: %s
- 记忆: %s/memory/MEMORY.md
- 每日笔记: %s/memory/YYYYMM/YYYYMMDD.md
- 技能: %s/skills/{skill-name}/SKILL.md

%s

## 重要规则

1. **务必使用工具** - 当你需要执行操作（安排提醒、发送消息、执行命令等）时，你必须调用相应的工具。不要只说你会做，也不要假装执行。

2. **提供帮助且准确** - 使用工具时，简要说明你在做什么。

3. **记忆** - 当需要记住某事时，写入到 %s/memory/MEMORY.md`,
		now, runtime, workspacePath, workspacePath, workspacePath, workspacePath, toolsSection, workspacePath)
}

func (cb *ContextBuilder) buildToolsSection() string {
	if cb.tools == nil {
		return ""
	}

	summaries := cb.tools.GetSummaries()
	if len(summaries) == 0 {
		return ""
	}

	var sb strings.Builder
	sb.WriteString("## 可用工具\n\n")
	sb.WriteString("**重要提示**: 你必须使用工具来执行操作。不要假装执行命令或安排任务。\n\n")
	sb.WriteString("你可以访问以下工具:\n\n")
	for _, s := range summaries {
		sb.WriteString(s)
		sb.WriteString("\n")
	}

	return sb.String()
}

func (cb *ContextBuilder) BuildSystemPrompt(skipBootstrap bool) string {
	parts := []string{}

	// Core identity section
	parts = append(parts, cb.getIdentity())

	// Bootstrap files (with parameter)
	bootstrapContent := cb.LoadBootstrapFiles(skipBootstrap)
	if bootstrapContent != "" {
		parts = append(parts, bootstrapContent)
	}

	// Skills - show summary, AI can read full content with read_file tool
	skillsSummary := cb.skillsLoader.BuildSkillsSummary()
	if skillsSummary != "" {
		parts = append(parts, fmt.Sprintf(`# Skills 技能

以下技能扩展了你的能力。要使用某个技能，请使用 read_file 工具读取其 SKILL.md 文件。

%s`, skillsSummary))
	}

	// Memory context
	memoryContext := cb.memory.GetMemoryContext()
	if memoryContext != "" {
		parts = append(parts, "# Memory\n\n"+memoryContext)
	}

	// Join with "---" separator
	return strings.Join(parts, "\n\n---\n\n")
}

func (cb *ContextBuilder) LoadBootstrapFiles(skipBootstrap bool) string {
	// Boundary condition: heartbeat requests should not trigger bootstrap
	if skipBootstrap {
		// Heartbeat mode: only load config files, do not trigger initialization
		bootstrapFiles := []string{
			"AGENT.md",
			"IDENTITY.md",
			"SOUL.md",
			"USER.md",
			"MCP.md",
		}

		var result string
		for _, filename := range bootstrapFiles {
			filePath := filepath.Join(cb.workspace, filename)
			if data, err := os.ReadFile(filePath); err == nil {
				result += fmt.Sprintf("## %s\n\n%s\n\n", filename, string(data))
			}
		}
		return result
	}

	// Normal mode: check for BOOTSTRAP.md
	bootstrapPath := filepath.Join(cb.workspace, "BOOTSTRAP.md")
	if data, err := os.ReadFile(bootstrapPath); err == nil {
		// BOOTSTRAP.md exists = initialization mode
		return fmt.Sprintf(`## ⚠️ 初始化引导模式

BOOTSTRAP.md 文件存在，说明这是首次启动或需要重新初始化。

**重要指令 - 必须遵守**:
1. 主动发起对话，按照 BOOTSTRAP.md 内容与用户完成初始化
2. 初始化完成后必须调用 complete_bootstrap 工具（confirmed=true）删除 BOOTSTRAP.md
3. 不要用其他方式删除文件，必须调用工具
4. 完成标准：已修正 IDENTITY.md 和 USER.md，且用户确认满意

**工具调用示例**:
complete_bootstrap(confirmed=true)

## BOOTSTRAP.md

%s`, string(data))
	}

	// BOOTSTRAP.md does not exist = normal mode
	bootstrapFiles := []string{
		"AGENT.md",
		"IDENTITY.md",
		"SOUL.md",
		"USER.md",
		"MCP.md",
	}

	var result string
	for _, filename := range bootstrapFiles {
		filePath := filepath.Join(cb.workspace, filename)
		if data, err := os.ReadFile(filePath); err == nil {
			result += fmt.Sprintf("## %s\n\n%s\n\n", filename, string(data))
		}
	}

	return result
}

func (cb *ContextBuilder) BuildMessages(history []providers.Message, summary string, currentMessage string, media []string, channel, chatID string, skipBootstrap bool) []providers.Message {
	messages := []providers.Message{}

	// Pass skipBootstrap parameter
	systemPrompt := cb.BuildSystemPrompt(skipBootstrap)

	// Add Current Session info if provided
	if channel != "" && chatID != "" {
		systemPrompt += fmt.Sprintf("\n\n## Current Session\nChannel: %s\nChat ID: %s", channel, chatID)
	}

	// Log system prompt summary for debugging (debug mode only)
	logger.DebugCF("agent", "System prompt built",
		map[string]interface{}{
			"total_chars":   len(systemPrompt),
			"total_lines":   strings.Count(systemPrompt, "\n") + 1,
			"section_count": strings.Count(systemPrompt, "\n\n---\n\n") + 1,
		})

	// Log preview of system prompt (avoid logging huge content)
	preview := systemPrompt
	if len(preview) > 500 {
		preview = preview[:500] + "... (truncated)"
	}
	logger.DebugCF("agent", "System prompt preview",
		map[string]interface{}{
			"preview": preview,
		})

	if summary != "" {
		systemPrompt += "\n\n## Summary of Previous Conversation\n\n" + summary
	}

	//This fix prevents the session memory from LLM failure due to elimination of toolu_IDs required from LLM
	// --- INICIO DEL FIX ---
	//Diegox-17
	for len(history) > 0 && (history[0].Role == "tool") {
		logger.DebugCF("agent", "Removing orphaned tool message from history to prevent LLM error",
			map[string]interface{}{"role": history[0].Role})
		history = history[1:]
	}
	//Diegox-17
	// --- FIN DEL FIX ---

	messages = append(messages, providers.Message{
		Role:    "system",
		Content: systemPrompt,
	})

	messages = append(messages, history...)

	messages = append(messages, providers.Message{
		Role:    "user",
		Content: currentMessage,
	})

	return messages
}

func (cb *ContextBuilder) AddToolResult(messages []providers.Message, toolCallID, toolName, result string) []providers.Message {
	messages = append(messages, providers.Message{
		Role:       "tool",
		Content:    result,
		ToolCallID: toolCallID,
	})
	return messages
}

func (cb *ContextBuilder) AddAssistantMessage(messages []providers.Message, content string, toolCalls []map[string]interface{}) []providers.Message {
	msg := providers.Message{
		Role:    "assistant",
		Content: content,
	}
	// Always add assistant message, whether or not it has tool calls
	messages = append(messages, msg)
	return messages
}

func (cb *ContextBuilder) loadSkills() string {
	allSkills := cb.skillsLoader.ListSkills()
	if len(allSkills) == 0 {
		return ""
	}

	var skillNames []string
	for _, s := range allSkills {
		skillNames = append(skillNames, s.Name)
	}

	content := cb.skillsLoader.LoadSkillsForContext(skillNames)
	if content == "" {
		return ""
	}

	return "# Skill Definitions\n\n" + content
}

// GetSkillsInfo returns information about loaded skills.
func (cb *ContextBuilder) GetSkillsInfo() map[string]interface{} {
	allSkills := cb.skillsLoader.ListSkills()
	skillNames := make([]string, 0, len(allSkills))
	for _, s := range allSkills {
		skillNames = append(skillNames, s.Name)
	}
	return map[string]interface{}{
		"total":     len(allSkills),
		"available": len(allSkills),
		"names":     skillNames,
	}
}
