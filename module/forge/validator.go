package forge

import (
	"regexp"
	"strings"
	"time"
)

// ValidationStage is the common base for all validation result stages.
type ValidationStage struct {
	Passed    bool      `json:"passed"`
	Timestamp time.Time `json:"timestamp"`
	Errors    []string  `json:"errors,omitempty"`
}

// StaticValidationResult holds the result of Stage 1 (static validation).
type StaticValidationResult struct {
	ValidationStage
	Warnings []string `json:"warnings,omitempty"`
}

// FunctionalValidationResult holds the result of Stage 2 (functional testing).
type FunctionalValidationResult struct {
	ValidationStage
	TestsRun    int `json:"tests_run,omitempty"`
	TestsPassed int `json:"tests_passed,omitempty"`
}

// QualityValidationResult holds the result of Stage 3 (LLM quality evaluation).
type QualityValidationResult struct {
	ValidationStage
	Score      int            `json:"score,omitempty"`
	Notes      string         `json:"notes,omitempty"`
	Dimensions map[string]int `json:"dimensions,omitempty"`
}

// ArtifactValidation aggregates all three validation stages.
type ArtifactValidation struct {
	Stage1Static     *StaticValidationResult     `json:"stage1_static,omitempty"`
	Stage2Functional *FunctionalValidationResult `json:"stage2_functional,omitempty"`
	Stage3Quality    *QualityValidationResult    `json:"stage3_quality,omitempty"`
	LastValidated    time.Time                   `json:"last_validated,omitempty"`
}

// StaticValidator performs Stage 1 static content validation.
type StaticValidator struct {
	registry *Registry
}

// NewStaticValidator creates a new StaticValidator.
func NewStaticValidator(registry *Registry) *StaticValidator {
	return &StaticValidator{registry: registry}
}

// Validate performs static validation on artifact content.
func (v *StaticValidator) Validate(artifactType ArtifactType, name string, content string) *StaticValidationResult {
	result := &StaticValidationResult{
		ValidationStage: ValidationStage{
			Timestamp: time.Now().UTC(),
		},
	}

	switch artifactType {
	case ArtifactSkill:
		v.validateSkill(content, result)
	case ArtifactScript:
		v.validateScript(content, result)
	case ArtifactMCP:
		v.validateMCP(content, result)
	default:
		result.Errors = append(result.Errors, "未知的产物类型")
	}

	// Common checks
	v.checkSecurity(content, result)
	v.checkDuplicates(artifactType, name, result)

	result.Passed = len(result.Errors) == 0
	return result
}

func (v *StaticValidator) validateSkill(content string, result *StaticValidationResult) {
	// Check frontmatter
	if !strings.Contains(content, "---") {
		result.Warnings = append(result.Warnings, "Skill 缺少 frontmatter（--- 分隔符），建议添加元数据")
	} else {
		// Check frontmatter has name and description
		parts := strings.SplitN(content, "---", 3)
		if len(parts) >= 3 {
			frontmatter := parts[1]
			if !strings.Contains(frontmatter, "name") && !strings.Contains(frontmatter, "名称") {
				result.Errors = append(result.Errors, "Skill frontmatter 缺少 name 字段")
			}
			if !strings.Contains(frontmatter, "description") && !strings.Contains(frontmatter, "描述") {
				result.Warnings = append(result.Warnings, "Skill frontmatter 建议包含 description 字段")
			}
		}
	}

	// Check content length
	if len(content) < 50 {
		result.Errors = append(result.Errors, "Skill 内容过短（少于 50 字符）")
	} else if len(content) > 5000 {
		result.Warnings = append(result.Warnings, "Skill 内容较长（超过 5000 字符），建议精简")
	}
}

func (v *StaticValidator) validateScript(content string, result *StaticValidationResult) {
	if strings.TrimSpace(content) == "" {
		result.Errors = append(result.Errors, "脚本内容不能为空")
		return
	}

	// Check for dangerous patterns
	dangerousPatterns := []struct {
		pattern string
		desc    string
	}{
		{`rm\s+-rf\s+/`, "包含危险命令: rm -rf /"},
		{"curl.*\\|.*bash", "包含危险模式: curl | bash"},
		{"curl.*\\|.*sh", "包含危险模式: curl | sh"},
	}

	for _, dp := range dangerousPatterns {
		matched, _ := regexp.MatchString(dp.pattern, content)
		if matched {
			result.Errors = append(result.Errors, dp.desc)
		}
	}
}

func (v *StaticValidator) validateMCP(content string, result *StaticValidationResult) {
	if strings.TrimSpace(content) == "" {
		result.Errors = append(result.Errors, "MCP 内容不能为空")
		return
	}

	// Detect language
	isPython := strings.Contains(content, "import ") && (strings.Contains(content, "def ") || strings.Contains(content, "class ")) ||
		strings.Contains(content, "from mcp")
	isGo := strings.Contains(content, "package ") || strings.Contains(content, "func ")

	if !isPython && !isGo {
		result.Warnings = append(result.Warnings, "MCP 内容缺少基本的代码结构（未检测到 package/func/def/class/import）")
	}

	// Python MCP-specific checks
	if isPython {
		// Check for MCP SDK import
		hasMCPSDK := strings.Contains(content, "from mcp.server") ||
			strings.Contains(content, "from mcp import") ||
			strings.Contains(content, "import mcp")
		if !hasMCPSDK {
			result.Warnings = append(result.Warnings, "Python MCP 未检测到 mcp SDK 导入（from mcp.server / import mcp）")
		}

		// Check for tool decorator registration
		hasToolDecorator := strings.Contains(content, "@server.tool") ||
			strings.Contains(content, "@mcp.tool") ||
			strings.Contains(content, "@server.list_tools") ||
			strings.Contains(content, "@mcp.list_tools")
		if !hasToolDecorator {
			result.Warnings = append(result.Warnings, "Python MCP 未检测到工具注册装饰器（@server.tool / @mcp.tool）")
		}

		// Check for function definitions
		if !strings.Contains(content, "def ") {
			result.Errors = append(result.Errors, "Python MCP 缺少函数定义（def）")
		}
	}
}

func (v *StaticValidator) checkSecurity(content string, result *StaticValidationResult) {
	// Check for keys/tokens in content
	secretPatterns := []struct {
		pattern string
		desc    string
	}{
		{`(?i)api[_-]?key\s*[:=]\s*['"][^'"]{8,}`, "包含疑似 API Key"},
		{`(?i)secret[_-]?key\s*[:=]\s*['"][^'"]{8,}`, "包含疑似 Secret Key"},
		{`(?i)token\s*[:=]\s*['"][^'"]{8,}`, "包含疑似 Token"},
	}

	for _, sp := range secretPatterns {
		matched, _ := regexp.MatchString(sp.pattern, content)
		if matched {
			result.Errors = append(result.Errors, sp.desc)
		}
	}
}

func (v *StaticValidator) checkDuplicates(artifactType ArtifactType, name string, result *StaticValidationResult) {
	if v.registry == nil {
		return
	}
	artifacts := v.registry.ListAll()
	for _, a := range artifacts {
		if a.Type == artifactType && a.Name == name {
			result.Warnings = append(result.Warnings, "已存在同名产物: "+name)
			break
		}
	}
}
