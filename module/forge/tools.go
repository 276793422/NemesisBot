package forge

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/276793422/NemesisBot/module/tools"
)

// NewForgeTools creates all Forge tools for registration with the tool registry.
func NewForgeTools(f *Forge) []tools.Tool {
	return []tools.Tool{
		&forgeReflectTool{forge: f},
		&forgeCreateTool{forge: f},
		&forgeUpdateTool{forge: f},
		&forgeListTool{forge: f},
		&forgeEvaluateTool{forge: f},
		&forgeBuildMCPTool{forge: f},
		&forgeShareTool{forge: f},
		&forgeLearningStatusTool{forge: f}, // Phase 6
	}
}

// --- forge_reflect ---

type forgeReflectTool struct {
	forge *Forge
}

func (t *forgeReflectTool) Name() string {
	return "forge_reflect"
}

func (t *forgeReflectTool) Description() string {
	return "分析近期经验数据，识别可改进的模式并生成建议。使用 Forge 自学习系统进行反思。"
}

func (t *forgeReflectTool) Parameters() map[string]interface{} {
	return map[string]interface{}{
		"type": "object",
		"properties": map[string]interface{}{
			"period": map[string]interface{}{
				"type":        "string",
				"description": "分析时间段: 'today', 'week', 'all'",
				"default":     "today",
			},
			"focus": map[string]interface{}{
				"type":        "string",
				"description": "关注类型: 'skill', 'script', 'mcp', 'all'",
				"default":     "all",
			},
		},
	}
}

func (t *forgeReflectTool) Execute(ctx context.Context, args map[string]interface{}) *tools.ToolResult {
	period := "today"
	if p, ok := args["period"].(string); ok && p != "" {
		period = p
	}
	focus := "all"
	if f, ok := args["focus"].(string); ok && f != "" {
		focus = f
	}

	reportPath, err := t.forge.ReflectNow(ctx, period, focus)
	if err != nil {
		return tools.ErrorResult(fmt.Sprintf("反思失败: %v", err))
	}

	// Read and return the report content
	content, err := os.ReadFile(reportPath)
	if err != nil {
		return tools.NewToolResult(fmt.Sprintf("反思报告已生成: %s（但无法读取内容）", reportPath))
	}

	return tools.NewToolResult(fmt.Sprintf("反思报告已生成:\n\n%s", string(content)))
}

// --- forge_create ---

type forgeCreateTool struct {
	forge *Forge
}

func (t *forgeCreateTool) Name() string {
	return "forge_create"
}

func (t *forgeCreateTool) Description() string {
	return "创建新的自学习产物（Skill/脚本/MCP模块）。脚本和MCP类型必须附带test_cases。"
}

func (t *forgeCreateTool) Parameters() map[string]interface{} {
	return map[string]interface{}{
		"type": "object",
		"properties": map[string]interface{}{
			"type": map[string]interface{}{
				"type":        "string",
				"enum":        []string{"skill", "script", "mcp"},
				"description": "产物类型",
			},
			"name": map[string]interface{}{
				"type":        "string",
				"description": "产物名称（小写，用连字符分隔）",
			},
			"content": map[string]interface{}{
				"type":        "string",
				"description": "产物内容（Skill: SKILL.md 内容；脚本: 代码内容；MCP: 主文件内容）",
			},
			"description": map[string]interface{}{
				"type":        "string",
				"description": "产物描述",
			},
			"category": map[string]interface{}{
				"type":        "string",
				"description": "分类（用于脚本）",
			},
			"language": map[string]interface{}{
				"type":        "string",
				"description": "编程语言（用于脚本/MCP）",
			},
			"test_cases": map[string]interface{}{
				"type":        "array",
				"description": "测试用例（脚本/MCP必填）",
				"items": map[string]interface{}{
					"type": "object",
				},
			},
		},
		"required": []string{"type", "name", "content"},
	}
}

func (t *forgeCreateTool) Execute(ctx context.Context, args map[string]interface{}) *tools.ToolResult {
	artifactType, _ := args["type"].(string)
	name, _ := args["name"].(string)
	content, _ := args["content"].(string)
	description, _ := args["description"].(string)
	category, _ := args["category"].(string)

	if artifactType == "" || name == "" || content == "" {
		return tools.ErrorResult("type, name, content 为必填字段")
	}

	// Validate: scripts and MCP require test_cases
	if artifactType == "script" || artifactType == "mcp" {
		testCases, hasTests := args["test_cases"]
		if !hasTests || testCases == nil {
			return tools.ErrorResult("脚本和MCP类型必须附带 test_cases。请提供测试用例后重试。")
		}
	}

	// Validate artifact type
	if artifactType != "skill" && artifactType != "script" && artifactType != "mcp" {
		return tools.ErrorResult("type 必须是 'skill', 'script', 或 'mcp'")
	}

	// Sanitize name
	name = strings.ToLower(strings.ReplaceAll(name, " ", "-"))

	// Use shared CreateSkill method for skill type (Phase 6 dedup)
	if artifactType == "skill" {
		artifact, err := t.forge.CreateSkill(ctx, name, content, description, nil)
		if err != nil {
			return tools.ErrorResult(fmt.Sprintf("创建失败: %v", err))
		}
		status := string(artifact.Status)
		var validationInfo string
		if artifact.Validation != nil {
			validationInfo = fmt.Sprintf("\n- 验证状态: Stage1=%v, Stage2=%v, Stage3=%v",
				artifact.Validation.Stage1Static != nil && artifact.Validation.Stage1Static.Passed,
				artifact.Validation.Stage2Functional != nil && artifact.Validation.Stage2Functional.Passed,
				artifact.Validation.Stage3Quality != nil && artifact.Validation.Stage3Quality.Passed)
			if artifact.Validation.Stage3Quality != nil {
				validationInfo += fmt.Sprintf(" (评分: %d)", artifact.Validation.Stage3Quality.Score)
			}
		}
		return tools.NewToolResult(fmt.Sprintf("Forge 产物已创建:\n- 类型: skill\n- 名称: %s\n- 路径: %s\n- 状态: %s\n- ID: %s%s",
			name, artifact.Path, status, artifact.ID, validationInfo))
	}

	// Non-skill types: inline creation logic (script/mcp)
	// Auto-generate frontmatter for Skills if missing
	if artifactType == "skill" && !strings.Contains(content, "---") {
		content = fmt.Sprintf("---\nname: %s\ndescription: %s\n---\n\n%s", name, description, content)
	}

	forgeDir := t.forge.GetWorkspace()
	var artifactPath string

	switch artifactType {
	case "skill":
		artifactPath = filepath.Join(forgeDir, "skills", name, "SKILL.md")
	case "script":
		if category == "" {
			category = "utils"
		}
		artifactPath = filepath.Join(forgeDir, "scripts", category, name)
	case "mcp":
		language, _ := args["language"].(string)
		if language == "" {
			language = "python"
		}
		ext := "py"
		entryFile := "server"
		if language == "go" {
			ext = "go"
			entryFile = "main"
		}
		artifactPath = filepath.Join(forgeDir, "mcp", name, fmt.Sprintf("%s.%s", entryFile, ext))
	}

	// Create directory
	if err := os.MkdirAll(filepath.Dir(artifactPath), 0755); err != nil {
		return tools.ErrorResult(fmt.Sprintf("创建目录失败: %v", err))
	}

	// Write content
	if err := os.WriteFile(artifactPath, []byte(content), 0644); err != nil {
		return tools.ErrorResult(fmt.Sprintf("写入文件失败: %v", err))
	}

	// Generate MCP project structure files
	if artifactType == "mcp" {
		mcpDir := filepath.Dir(artifactPath)
		language, _ := args["language"].(string)
		if language == "" {
			language = "python"
		}

		switch language {
		case "python":
			// requirements.txt with mcp SDK dependency
			requirements := "mcp>=1.0.0\n"
			os.WriteFile(filepath.Join(mcpDir, "requirements.txt"), []byte(requirements), 0644)
			// README.md
			readme := fmt.Sprintf("# %s\n\nForge-generated MCP server.\n\n## Usage\n\n```bash\nuv run server.py\n```\n", name)
			os.WriteFile(filepath.Join(mcpDir, "README.md"), []byte(readme), 0644)
		case "go":
			// go.mod for Go MCP server
			goMod := fmt.Sprintf("module forge-mcp-%s\n\ngo 1.21\n", name)
			os.WriteFile(filepath.Join(mcpDir, "go.mod"), []byte(goMod), 0644)
		}
	}

	// Write test cases if provided
	if testCases, ok := args["test_cases"]; ok {
		testData, err := json.MarshalIndent(testCases, "", "  ")
		if err == nil {
			testDir := filepath.Join(filepath.Dir(artifactPath), "tests")
			os.MkdirAll(testDir, 0755)
			os.WriteFile(filepath.Join(testDir, "test_cases.json"), testData, 0644)
		}
	}

	// Register in registry
	artifactID := fmt.Sprintf("%s-%s", artifactType, name)
	artifact := Artifact{
		ID:      artifactID,
		Type:    ArtifactType(artifactType),
		Name:    name,
		Version: "1.0",
		Status:  ArtifactStatus(t.forge.GetConfig().Artifacts.DefaultStatus),
		Path:    artifactPath,
		Evolution: []Evolution{
			{
				Version: "1.0",
				Date:    time.Now().UTC(),
				Change:  "初始创建",
			},
		},
	}

	if description != "" {
		artifact.Evolution[0].Change = fmt.Sprintf("初始创建: %s", description)
	}

	if err := t.forge.GetRegistry().Add(artifact); err != nil {
		return tools.ErrorResult(fmt.Sprintf("注册产物失败: %v", err))
	}

	// For skills, also copy to workspace/skills/ with -forge suffix
	if artifactType == "skill" {
		workspaceSkills := filepath.Join(t.forge.workspace, "skills", name+"-forge")
		if err := os.MkdirAll(workspaceSkills, 0755); err == nil {
			os.WriteFile(filepath.Join(workspaceSkills, "SKILL.md"), []byte(content), 0644)
		}
	}

	// Auto-validate if configured
	var validationInfo string
	if t.forge.GetConfig().Validation.AutoValidate {
		validation := t.forge.GetPipeline().RunFromContent(ctx, &artifact, content)
		newStatus := t.forge.GetPipeline().DetermineStatus(validation)
		t.forge.GetRegistry().Update(artifactID, func(a *Artifact) {
			a.Validation = validation
			a.Status = newStatus
		})
		validationInfo = fmt.Sprintf("\n- 验证状态: Stage1=%v, Stage2=%v, Stage3=%v",
			validation.Stage1Static != nil && validation.Stage1Static.Passed,
			validation.Stage2Functional != nil && validation.Stage2Functional.Passed,
			validation.Stage3Quality != nil && validation.Stage3Quality.Passed)
		if validation.Stage3Quality != nil {
			validationInfo += fmt.Sprintf(" (评分: %d)", validation.Stage3Quality.Score)
		}
	}

	status := t.forge.GetConfig().Artifacts.DefaultStatus
	// Get updated status after validation
	if updated, found := t.forge.GetRegistry().Get(artifactID); found {
		status = string(updated.Status)
	}

	// Auto-register MCP to config.mcp.json if active
	if artifactType == "mcp" && status == string(StatusActive) {
		inst := t.forge.GetMCPInstaller()
		if err := inst.Install(&artifact, filepath.Dir(artifactPath)); err != nil {
			validationInfo += fmt.Sprintf("\n- MCP 注册失败: %v（请手动注册）", err)
		} else {
			validationInfo += "\n- MCP 已自动注册到 config.mcp.json"
		}
	}

	return tools.NewToolResult(fmt.Sprintf("Forge 产物已创建:\n- 类型: %s\n- 名称: %s\n- 路径: %s\n- 状态: %s\n- ID: %s%s",
		artifactType, name, artifactPath, status, artifactID, validationInfo))
}

// --- forge_update ---

type forgeUpdateTool struct {
	forge *Forge
}

func (t *forgeUpdateTool) Name() string {
	return "forge_update"
}

func (t *forgeUpdateTool) Description() string {
	return "更新现有 Forge 产物，自动记录版本变更。支持版本快照和回滚。"
}

func (t *forgeUpdateTool) Parameters() map[string]interface{} {
	return map[string]interface{}{
		"type": "object",
		"properties": map[string]interface{}{
			"id": map[string]interface{}{
				"type":        "string",
				"description": "产物 ID（如 skill-batch-config-edit）",
			},
			"content": map[string]interface{}{
				"type":        "string",
				"description": "更新后的内容（与 rollback_version 二选一）",
			},
			"change_description": map[string]interface{}{
				"type":        "string",
				"description": "本次变更说明",
			},
			"rollback_version": map[string]interface{}{
				"type":        "string",
				"description": "回滚到指定版本（与 content 二选一）",
			},
		},
		"required": []string{"id"},
	}
}

func (t *forgeUpdateTool) Execute(ctx context.Context, args map[string]interface{}) *tools.ToolResult {
	id, _ := args["id"].(string)
	content, _ := args["content"].(string)
	changeDesc, _ := args["change_description"].(string)
	rollbackVersion, _ := args["rollback_version"].(string)

	if id == "" {
		return tools.ErrorResult("id 为必填字段")
	}

	artifact, found := t.forge.GetRegistry().Get(id)
	if !found {
		return tools.ErrorResult(fmt.Sprintf("产物 %s 不存在", id))
	}

	// Handle rollback
	if rollbackVersion != "" {
		snapshot, err := LoadVersionSnapshot(artifact.Path, rollbackVersion)
		if err != nil {
			return tools.ErrorResult(fmt.Sprintf("加载版本快照失败: %v", err))
		}
		content = snapshot
		changeDesc = fmt.Sprintf("回滚到版本 %s", rollbackVersion)
	}

	if content == "" {
		return tools.ErrorResult("content 或 rollback_version 为必填字段")
	}

	// Save version snapshot before updating
	if err := SaveVersionSnapshot(artifact.Path, artifact.Version); err == nil {
		// Snapshot saved successfully (best effort)
		_ = err
	}

	// Update file
	if err := os.WriteFile(artifact.Path, []byte(content), 0644); err != nil {
		return tools.ErrorResult(fmt.Sprintf("更新文件失败: %v", err))
	}

	// Update registry
	newVersion := incrementVersion(artifact.Version)
	_ = t.forge.GetRegistry().Update(id, func(a *Artifact) {
		a.Version = newVersion
		a.Evolution = append(a.Evolution, Evolution{
			Version: newVersion,
			Date:    time.Now().UTC(),
			Change:  changeDesc,
		})
		a.Status = ArtifactStatus(t.forge.GetConfig().Artifacts.DefaultStatus)
	})

	// Update skill copy if it's a skill type
	if artifact.Type == ArtifactSkill {
		skillsDir := filepath.Join(t.forge.workspace, "skills", artifact.Name+"-forge")
		os.MkdirAll(skillsDir, 0755)
		os.WriteFile(filepath.Join(skillsDir, "SKILL.md"), []byte(content), 0644)
	}

	// Re-register MCP if active after update
	if artifact.Type == ArtifactMCP {
		updated, found := t.forge.GetRegistry().Get(id)
		if found && updated.Status == StatusActive {
			inst := t.forge.GetMCPInstaller()
			inst.Install(&updated, filepath.Dir(artifact.Path))
		}
	}

	rollbackInfo := ""
	if rollbackVersion != "" {
		rollbackInfo = fmt.Sprintf(" (从 %s 回滚)", rollbackVersion)
	}

	return tools.NewToolResult(fmt.Sprintf("产物 %s 已更新到版本 %s%s: %s", id, newVersion, rollbackInfo, changeDesc))
}

// --- forge_list ---

type forgeListTool struct {
	forge *Forge
}

func (t *forgeListTool) Name() string {
	return "forge_list"
}

func (t *forgeListTool) Description() string {
	return "列出所有 Forge 产物及其状态。"
}

func (t *forgeListTool) Parameters() map[string]interface{} {
	return map[string]interface{}{
		"type": "object",
		"properties": map[string]interface{}{
			"type": map[string]interface{}{
				"type":        "string",
				"description": "筛选类型: 'skill', 'script', 'mcp', 'all'",
				"default":     "all",
			},
			"status": map[string]interface{}{
				"type":        "string",
				"description": "筛选状态: 'draft', 'testing', 'active', 'deprecated'",
			},
		},
	}
}

func (t *forgeListTool) Execute(ctx context.Context, args map[string]interface{}) *tools.ToolResult {
	artifactType, _ := args["type"].(string)
	status, _ := args["status"].(string)

	var artifacts []Artifact
	if artifactType == "" || artifactType == "all" {
		artifacts = t.forge.GetRegistry().ListAll()
	} else {
		artifacts = t.forge.GetRegistry().List(ArtifactType(artifactType), ArtifactStatus(status))
	}

	if len(artifacts) == 0 {
		return tools.NewToolResult("暂无 Forge 产物")
	}

	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("共 %d 个 Forge 产物:\n\n", len(artifacts)))
	sb.WriteString("| ID | 类型 | 名称 | 版本 | 状态 | 使用次数 | 成功率 |\n")
	sb.WriteString("|-----|------|------|------|------|----------|--------|\n")
	for _, a := range artifacts {
		sb.WriteString(fmt.Sprintf("| %s | %s | %s | %s | %s | %d | %.0f%% |\n",
			a.ID, a.Type, a.Name, a.Version, a.Status, a.UsageCount, a.SuccessRate*100))
	}

	return tools.NewToolResult(sb.String())
}

// --- forge_evaluate ---

type forgeEvaluateTool struct {
	forge *Forge
}

func (t *forgeEvaluateTool) Name() string {
	return "forge_evaluate"
}

func (t *forgeEvaluateTool) Description() string {
	return "评估 Forge 产物质量，运行三阶段验证管线并给出评分。"
}

func (t *forgeEvaluateTool) Parameters() map[string]interface{} {
	return map[string]interface{}{
		"type": "object",
		"properties": map[string]interface{}{
			"id": map[string]interface{}{
				"type":        "string",
				"description": "要评估的产物 ID",
			},
		},
		"required": []string{"id"},
	}
}

func (t *forgeEvaluateTool) Execute(ctx context.Context, args map[string]interface{}) *tools.ToolResult {
	id, _ := args["id"].(string)
	if id == "" {
		return tools.ErrorResult("id 为必填字段")
	}

	artifact, found := t.forge.GetRegistry().Get(id)
	if !found {
		return tools.ErrorResult(fmt.Sprintf("产物 %s 不存在", id))
	}

	// Read artifact content
	content, err := os.ReadFile(artifact.Path)
	if err != nil {
		return tools.ErrorResult(fmt.Sprintf("读取产物文件失败: %v", err))
	}

	// Run full validation pipeline
	validation := t.forge.GetPipeline().RunFromContent(ctx, &artifact, string(content))
	newStatus := t.forge.GetPipeline().DetermineStatus(validation)

	// Update registry with validation results and new status
	t.forge.GetRegistry().Update(id, func(a *Artifact) {
		a.Validation = validation
		a.Status = newStatus
	})

	// Format results
	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("## Forge 产物评估: %s\n\n", id))
	sb.WriteString(fmt.Sprintf("**旧状态: %s → 新状态: %s**\n\n", artifact.Status, newStatus))

	// Stage 1
	sb.WriteString("### Stage 1: 静态验证\n")
	if validation.Stage1Static != nil {
		if validation.Stage1Static.Passed {
			sb.WriteString("- **通过** ✓\n")
		} else {
			sb.WriteString("- **未通过** ✗\n")
			for _, e := range validation.Stage1Static.Errors {
				sb.WriteString(fmt.Sprintf("  - 错误: %s\n", e))
			}
		}
		for _, w := range validation.Stage1Static.Warnings {
			sb.WriteString(fmt.Sprintf("  - 警告: %s\n", w))
		}
	}

	// Stage 2
	sb.WriteString("\n### Stage 2: 功能验证\n")
	if validation.Stage2Functional != nil {
		if validation.Stage2Functional.Passed {
			sb.WriteString(fmt.Sprintf("- **通过** ✓ (%d/%d 测试)\n",
				validation.Stage2Functional.TestsPassed, validation.Stage2Functional.TestsRun))
		} else {
			sb.WriteString("- **未通过** ✗\n")
			for _, e := range validation.Stage2Functional.Errors {
				sb.WriteString(fmt.Sprintf("  - 错误: %s\n", e))
			}
		}
	} else {
		sb.WriteString("- 跳过（Stage 1 未通过）\n")
	}

	// Stage 3
	sb.WriteString("\n### Stage 3: 质量评估\n")
	if validation.Stage3Quality != nil {
		sb.WriteString(fmt.Sprintf("- **评分: %d/100**\n", validation.Stage3Quality.Score))
		if validation.Stage3Quality.Notes != "" {
			sb.WriteString(fmt.Sprintf("- 备注: %s\n", validation.Stage3Quality.Notes))
		}
		if len(validation.Stage3Quality.Dimensions) > 0 {
			sb.WriteString("- 维度评分:\n")
			for dim, score := range validation.Stage3Quality.Dimensions {
				sb.WriteString(fmt.Sprintf("  - %s: %d\n", dim, score))
			}
		}
	} else {
		sb.WriteString("- 跳过（前置阶段未通过）\n")
	}

	return tools.NewToolResult(sb.String())
}

// --- forge_build_mcp ---

type forgeBuildMCPTool struct {
	forge *Forge
}

func (t *forgeBuildMCPTool) Name() string {
	return "forge_build_mcp"
}

func (t *forgeBuildMCPTool) Description() string {
	return "构建/验证 MCP 服务器并注册到配置。验证通过后自动启用。"
}

func (t *forgeBuildMCPTool) Parameters() map[string]interface{} {
	return map[string]interface{}{
		"type": "object",
		"properties": map[string]interface{}{
			"id": map[string]interface{}{
				"type":        "string",
				"description": "MCP 产物 ID（如 mcp-json-validator）",
			},
			"action": map[string]interface{}{
				"type":        "string",
				"enum":        []string{"build", "install", "uninstall"},
				"description": "操作: build=验证, install=注册到配置, uninstall=从配置移除",
				"default":     "build",
			},
		},
		"required": []string{"id"},
	}
}

func (t *forgeBuildMCPTool) Execute(ctx context.Context, args map[string]interface{}) *tools.ToolResult {
	id, _ := args["id"].(string)
	action, _ := args["action"].(string)
	if action == "" {
		action = "build"
	}

	if id == "" {
		return tools.ErrorResult("id 为必填字段")
	}

	artifact, found := t.forge.GetRegistry().Get(id)
	if !found {
		return tools.ErrorResult(fmt.Sprintf("产物 %s 不存在", id))
	}

	if artifact.Type != ArtifactMCP {
		return tools.ErrorResult(fmt.Sprintf("产物 %s 不是 MCP 类型", id))
	}

	inst := t.forge.GetMCPInstaller()
	mcpDir := filepath.Dir(artifact.Path)

	switch action {
	case "install":
		if err := inst.Install(&artifact, mcpDir); err != nil {
			return tools.ErrorResult(fmt.Sprintf("MCP 注册失败: %v", err))
		}
		return tools.NewToolResult(fmt.Sprintf("MCP 服务器 '%s' 已注册到 config.mcp.json", artifact.Name))

	case "uninstall":
		if err := inst.Uninstall(artifact.Name); err != nil {
			return tools.ErrorResult(fmt.Sprintf("MCP 卸载失败: %v", err))
		}
		return tools.NewToolResult(fmt.Sprintf("MCP 服务器 '%s' 已从 config.mcp.json 移除", artifact.Name))

	case "build":
		// Read and validate content
		content, err := os.ReadFile(artifact.Path)
		if err != nil {
			return tools.ErrorResult(fmt.Sprintf("读取产物文件失败: %v", err))
		}

		validation := t.forge.GetPipeline().RunFromContent(ctx, &artifact, string(content))
		newStatus := t.forge.GetPipeline().DetermineStatus(validation)

		t.forge.GetRegistry().Update(id, func(a *Artifact) {
			a.Validation = validation
			a.Status = newStatus
		})

		var sb strings.Builder
		sb.WriteString(fmt.Sprintf("MCP 构建验证: %s\n", id))
		sb.WriteString(fmt.Sprintf("状态: %s → %s\n", artifact.Status, newStatus))

		if validation.Stage1Static != nil {
			sb.WriteString(fmt.Sprintf("- 静态验证: %v\n", validation.Stage1Static.Passed))
		}

		// Auto-install if active
		if newStatus == StatusActive {
			if err := inst.Install(&artifact, mcpDir); err != nil {
				sb.WriteString(fmt.Sprintf("- 自动注册失败: %v\n", err))
			} else {
				sb.WriteString("- 已自动注册到 config.mcp.json\n")
			}
		}

		return tools.NewToolResult(sb.String())

	default:
		return tools.ErrorResult(fmt.Sprintf("未知操作: %s（支持: build, install, uninstall）", action))
	}
}

// --- forge_share ---

type forgeShareTool struct {
	forge *Forge
}

func (t *forgeShareTool) Name() string {
	return "forge_share"
}

func (t *forgeShareTool) Description() string {
	return "将最新的反思报告分享给集群中的其他在线节点。需要集群模式已启用。"
}

func (t *forgeShareTool) Parameters() map[string]interface{} {
	return map[string]interface{}{
		"type": "object",
		"properties": map[string]interface{}{
			"report_path": map[string]interface{}{
				"type":        "string",
				"description": "要分享的报告路径（留空则分享最新报告）",
			},
		},
	}
}

func (t *forgeShareTool) Execute(ctx context.Context, args map[string]interface{}) *tools.ToolResult {
	syncer := t.forge.GetSyncer()
	if syncer == nil || !syncer.IsEnabled() {
		return tools.NewToolResult("Forge 集群共享未启用。请确保集群模式已开启且桥接已配置。")
	}

	// Determine which report to share
	reportPath, _ := args["report_path"].(string)
	if reportPath == "" {
		// Find the latest report
		latest, err := t.forge.GetReflector().GetLatestReport()
		if err != nil {
			return tools.ErrorResult(fmt.Sprintf("未找到反思报告: %v", err))
		}
		reportPath = latest
	} else {
		// Security: validate report_path is within the reflections directory
		reflectionsDir := filepath.Join(t.forge.GetWorkspace(), "reflections")
		absPath, err := filepath.Abs(reportPath)
		if err != nil {
			return tools.ErrorResult(fmt.Sprintf("无效路径: %v", err))
		}
		absDir, err := filepath.Abs(reflectionsDir)
		if err != nil {
			return tools.ErrorResult(fmt.Sprintf("无效目录: %v", err))
		}
		if !strings.HasPrefix(absPath, absDir+string(filepath.Separator)) && absPath != absDir {
			return tools.ErrorResult("report_path 必须在 forge reflections 目录内")
		}
	}

	if err := syncer.ShareReflection(ctx, reportPath); err != nil {
		return tools.ErrorResult(fmt.Sprintf("分享失败: %v", err))
	}

	return tools.NewToolResult(fmt.Sprintf("反思报告已分享: %s", filepath.Base(reportPath)))
}

// --- forge_learning_status (Phase 6) ---

type forgeLearningStatusTool struct {
	forge *Forge
}

func (t *forgeLearningStatusTool) Name() string {
	return "forge_learning_status"
}

func (t *forgeLearningStatusTool) Description() string {
	return "查看 Forge 闭环学习状态，包括最近学习周期摘要和活跃产物效果。"
}

func (t *forgeLearningStatusTool) Parameters() map[string]interface{} {
	return map[string]interface{}{
		"type": "object",
		"properties": map[string]interface{}{},
	}
}

func (t *forgeLearningStatusTool) Execute(ctx context.Context, args map[string]interface{}) *tools.ToolResult {
	cfg := t.forge.GetConfig()
	if !cfg.Learning.Enabled {
		return tools.NewToolResult("Forge 闭环学习未启用。在 forge.json 中设置 learning.enabled = true 以启用。")
	}

	var sb strings.Builder
	sb.WriteString("## Forge 闭环学习状态\n\n")
	sb.WriteString(fmt.Sprintf("- 学习引擎: 已启用\n"))
	sb.WriteString(fmt.Sprintf("- 最小模式频次: %d\n", cfg.Learning.MinPatternFrequency))
	sb.WriteString(fmt.Sprintf("- 高置信度阈值: %.2f\n", cfg.Learning.HighConfThreshold))
	sb.WriteString(fmt.Sprintf("- 每轮最大自动创建: %d\n", cfg.Learning.MaxAutoCreates))

	// Latest cycle info
	le := t.forge.GetLearningEngine()
	if le != nil {
		cycle := le.GetLatestCycle()
		if cycle != nil {
			sb.WriteString(fmt.Sprintf("\n### 最近学习周期\n"))
			sb.WriteString(fmt.Sprintf("- ID: %s\n", cycle.ID))
			sb.WriteString(fmt.Sprintf("- 开始时间: %s\n", cycle.StartedAt.Format("2006-01-02 15:04")))
			if cycle.CompletedAt != nil {
				sb.WriteString(fmt.Sprintf("- 完成时间: %s\n", cycle.CompletedAt.Format("2006-01-02 15:04")))
			}
			sb.WriteString(fmt.Sprintf("- 检测模式: %d\n", cycle.PatternsFound))
			sb.WriteString(fmt.Sprintf("- 创建行动: %d\n", cycle.ActionsCreated))
			sb.WriteString(fmt.Sprintf("- 已执行: %d, 已跳过: %d\n", cycle.ActionsExecuted, cycle.ActionsSkipped))
		} else {
			sb.WriteString("\n暂无学习周期记录。\n")
		}
	}

	// Active forge skills with effect
	registry := t.forge.GetRegistry()
	artifacts := registry.List(ArtifactSkill, StatusActive)
	forgeSkills := make([]Artifact, 0)
	for _, a := range artifacts {
		if len(a.ToolSignature) > 0 {
			forgeSkills = append(forgeSkills, a)
		}
	}
	if len(forgeSkills) > 0 {
		sb.WriteString("\n### 活跃学习产物\n\n")
		sb.WriteString("| ID | 名称 | 工具签名 | 使用次数 | 成功率 |\n")
		sb.WriteString("|-----|------|----------|----------|--------|\n")
		for _, a := range forgeSkills {
			sig := strings.Join(a.ToolSignature, "→")
			sb.WriteString(fmt.Sprintf("| %s | %s | %s | %d | %.0f%% |\n",
				a.ID, a.Name, truncate(sig, 30), a.UsageCount, a.SuccessRate*100))
		}
	}

	return tools.NewToolResult(sb.String())
}

// incrementVersion increments a semver-like version string.
func incrementVersion(v string) string {
	parts := strings.Split(v, ".")
	if len(parts) < 2 {
		return v + ".1"
	}
	minor := 0
	fmt.Sscanf(parts[len(parts)-1], "%d", &minor)
	parts[len(parts)-1] = fmt.Sprintf("%d", minor+1)
	return strings.Join(parts, ".")
}

// IncrementVersionForTest exposes incrementVersion for testing.
func IncrementVersionForTest(v string) string {
	return incrementVersion(v)
}

// SaveVersionSnapshot saves a version backup of the artifact file.
func SaveVersionSnapshot(artifactPath, version string) error {
	versionsDir := filepath.Join(filepath.Dir(artifactPath), ".versions")
	if err := os.MkdirAll(versionsDir, 0755); err != nil {
		return err
	}

	content, err := os.ReadFile(artifactPath)
	if err != nil {
		return err
	}

	snapshotPath := filepath.Join(versionsDir, version+".bak")
	return os.WriteFile(snapshotPath, content, 0644)
}

// LoadVersionSnapshot loads a version backup from the .versions directory.
func LoadVersionSnapshot(artifactPath, version string) (string, error) {
	snapshotPath := filepath.Join(filepath.Dir(artifactPath), ".versions", version+".bak")
	content, err := os.ReadFile(snapshotPath)
	if err != nil {
		return "", fmt.Errorf("版本快照 %s 不存在: %w", version, err)
	}
	return string(content), nil
}
