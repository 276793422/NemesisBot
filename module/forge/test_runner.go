package forge

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"time"
)

// TestRunner performs Stage 2 functional validation.
// On Windows, it does pure content checks without executing scripts.
type TestRunner struct {
	registry *Registry
}

// NewTestRunner creates a new TestRunner.
func NewTestRunner(registry *Registry) *TestRunner {
	return &TestRunner{registry: registry}
}

// RunTests performs functional validation on an artifact.
func (r *TestRunner) RunTests(ctx context.Context, artifact *Artifact) *FunctionalValidationResult {
	result := &FunctionalValidationResult{
		ValidationStage: ValidationStage{
			Timestamp: time.Now().UTC(),
		},
	}

	switch artifact.Type {
	case ArtifactSkill:
		r.validateSkillContent(artifact, result)
	case ArtifactScript:
		r.validateScriptTests(artifact, result)
	case ArtifactMCP:
		r.validateMCPTests(artifact, result)
	default:
		result.Errors = append(result.Errors, "未知的产物类型")
	}

	result.Passed = len(result.Errors) == 0
	return result
}

// --- Skill Validation (5 checks) ---

func (r *TestRunner) validateSkillContent(artifact *Artifact, result *FunctionalValidationResult) {
	content, err := os.ReadFile(artifact.Path)
	if err != nil {
		result.Errors = append(result.Errors, "无法读取 Skill 文件: "+err.Error())
		return
	}

	contentStr := string(content)
	result.TestsRun = 5

	// Check 1: Frontmatter parsing — extract name and description
	fm := extractFrontmatterFromContent(contentStr)
	var skillName, skillDesc string
	if fm != "" {
		// Try JSON first
		var jsonMeta struct {
			Name        string `json:"name"`
			Description string `json:"description"`
		}
		if err := json.Unmarshal([]byte(fm), &jsonMeta); err == nil {
			skillName = jsonMeta.Name
			skillDesc = jsonMeta.Description
		} else {
			// Fall back to simple YAML
			yamlMeta := parseSimpleYAML(fm)
			skillName = yamlMeta["name"]
			skillDesc = yamlMeta["description"]
		}
	}
	if skillName != "" && skillDesc != "" {
		result.TestsPassed++
	} else {
		result.Errors = append(result.Errors, "Skill 缺少有效的 frontmatter（需要 name 和 description）")
	}

	// Check 2: Name legality — pattern and length
	if skillName != "" {
		if isValidSkillName(skillName) {
			result.TestsPassed++
		} else {
			result.Errors = append(result.Errors, fmt.Sprintf("Skill name 不合法: %q（仅允许字母数字和连字符，长度 <= 64）", skillName))
		}
	} else {
		result.Errors = append(result.Errors, "Skill 缺少 name 字段")
	}

	// Check 3: Description non-empty and within length limit
	if skillDesc != "" {
		if len(skillDesc) <= 1024 {
			result.TestsPassed++
		} else {
			result.Errors = append(result.Errors, "Skill description 超过 1024 字符限制")
		}
	} else {
		result.Errors = append(result.Errors, "Skill 缺少 description 字段")
	}

	// Check 4: Body non-empty after stripping frontmatter
	body := stripFrontmatter(contentStr)
	if strings.TrimSpace(body) != "" {
		result.TestsPassed++
	} else {
		result.Errors = append(result.Errors, "Skill 正文为空（剥离 frontmatter 后无内容）")
	}

	// Check 5: Markdown structure — headings, unordered lists, or ordered lists
	hasHeadings := regexp.MustCompile(`(?m)^#{1,6}\s`).MatchString(body)
	hasUnorderedLists := regexp.MustCompile(`(?m)^[\-\*]\s`).MatchString(body)
	hasOrderedLists := regexp.MustCompile(`(?m)^\d+\.\s`).MatchString(body)
	if hasHeadings || hasUnorderedLists || hasOrderedLists {
		result.TestsPassed++
	} else {
		result.Errors = append(result.Errors, "Skill 正文缺少 Markdown 结构（标题或列表）")
	}
}

// --- MCP Validation (5 checks) ---

func (r *TestRunner) validateMCPTests(artifact *Artifact, result *FunctionalValidationResult) {
	artifactDir := filepath.Dir(artifact.Path)
	testCasesPath := filepath.Join(artifactDir, "tests", "test_cases.json")

	result.TestsRun = 5

	// Read main file first (needed for multiple checks)
	content, err := os.ReadFile(artifact.Path)
	if err != nil {
		result.Errors = append(result.Errors, "无法读取 MCP 主文件")
		return
	}
	contentStr := string(content)

	// Check 1: test_cases.json — exists, valid JSON array, each item has name+input+expected
	data, err := os.ReadFile(testCasesPath)
	if err != nil {
		result.Errors = append(result.Errors, "缺少测试用例文件: tests/test_cases.json")
	} else {
		var testCases []map[string]interface{}
		if err := json.Unmarshal(data, &testCases); err != nil {
			result.Errors = append(result.Errors, "test_cases.json 格式错误: "+err.Error())
		} else if len(testCases) == 0 {
			result.Errors = append(result.Errors, "test_cases.json 为空数组")
		} else {
			allValid := true
			for _, tc := range testCases {
				if _, ok := tc["name"]; !ok {
					allValid = false
					break
				}
				if _, ok := tc["input"]; !ok {
					allValid = false
					break
				}
				if _, ok := tc["expected"]; !ok {
					allValid = false
					break
				}
			}
			if allValid {
				result.TestsPassed++
			} else {
				result.Errors = append(result.Errors, "test_cases.json 每项必须包含 name、input 和 expected 字段")
			}
		}
	}

	// Check 2: Bracket balance
	if err := checkBracketBalance(contentStr); err != nil {
		result.Errors = append(result.Errors, "MCP 主文件括号不匹配: "+err.Error())
	} else {
		result.TestsPassed++
	}

	// Check 3: Project file completeness
	lang := detectMCPLanguage(contentStr)
	switch lang {
	case "python":
		if _, err := os.Stat(filepath.Join(artifactDir, "requirements.txt")); err != nil {
			result.Errors = append(result.Errors, "Python MCP 缺少 requirements.txt")
		} else {
			result.TestsPassed++
		}
	case "go":
		if _, err := os.Stat(filepath.Join(artifactDir, "go.mod")); err != nil {
			result.Errors = append(result.Errors, "Go MCP 缺少 go.mod")
		} else {
			result.TestsPassed++
		}
	default:
		result.Errors = append(result.Errors, "无法检测 MCP 代码语言（需要 Python 或 Go）")
	}

	// Check 4: MCP protocol structure
	if err := checkMCPServerStructure(contentStr, lang); err != nil {
		result.Errors = append(result.Errors, "MCP 协议结构检查失败: "+err.Error())
	} else {
		result.TestsPassed++
	}

	// Check 5: Function/method completeness
	if err := checkFunctionCompleteness(contentStr, lang); err != nil {
		result.Errors = append(result.Errors, "函数完整性检查失败: "+err.Error())
	} else {
		result.TestsPassed++
	}
}

// --- Script Validation (unchanged) ---

func (r *TestRunner) validateScriptTests(artifact *Artifact, result *FunctionalValidationResult) {
	artifactDir := filepath.Dir(artifact.Path)
	testCasesPath := filepath.Join(artifactDir, "tests", "test_cases.json")

	result.TestsRun = 2

	// Check 1: test_cases.json exists and is valid JSON array
	data, err := os.ReadFile(testCasesPath)
	if err != nil {
		result.Errors = append(result.Errors, "缺少测试用例文件: tests/test_cases.json")
		return
	}
	result.TestsPassed++

	// Check 2: Valid JSON array with structured test cases
	var testCases []map[string]interface{}
	if err := json.Unmarshal(data, &testCases); err != nil {
		result.Errors = append(result.Errors, "test_cases.json 格式错误: "+err.Error())
		return
	}

	if len(testCases) == 0 {
		result.Errors = append(result.Errors, "test_cases.json 为空数组")
		return
	}

	// Check each test case has basic structure
	for _, tc := range testCases {
		if _, ok := tc["name"]; !ok {
			if _, ok := tc["input"]; !ok {
				result.Errors = append(result.Errors, "测试用例缺少 name 或 input 字段")
				continue
			}
		}
	}
	result.TestsPassed++
}

// --- Helper functions for Skill validation ---

// skillNamePattern matches the same pattern as skills.SkillsLoader's namePattern.
// Kept as a package-level var to avoid recompilation.
var skillNamePattern = regexp.MustCompile(`^[a-zA-Z0-9]+(-[a-zA-Z0-9]+)*$`)

// isValidSkillName checks that a skill name matches the allowed pattern and length.
func isValidSkillName(name string) bool {
	return len(name) <= 64 && skillNamePattern.MatchString(name)
}

// frontmatterRe matches ---\n...\n--- blocks at start of content.
var frontmatterRe = regexp.MustCompile(`(?s)^---\r?\n(.*?)\r?\n---\r?\n*`)

// extractFrontmatterFromContent replicates skills.SkillsLoader.extractFrontmatter.
func extractFrontmatterFromContent(content string) string {
	match := frontmatterRe.FindStringSubmatch(content)
	if len(match) > 1 {
		return match[1]
	}
	return ""
}

// stripFrontmatter removes the frontmatter block from content.
func stripFrontmatter(content string) string {
	return frontmatterRe.ReplaceAllString(content, "")
}

// parseSimpleYAML parses simple key: value format (same logic as skills.SkillsLoader).
func parseSimpleYAML(content string) map[string]string {
	result := make(map[string]string)
	normalized := strings.ReplaceAll(content, "\r\n", "\n")
	normalized = strings.ReplaceAll(normalized, "\r", "\n")

	for _, line := range strings.Split(normalized, "\n") {
		line = strings.TrimSpace(line)
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		parts := strings.SplitN(line, ":", 2)
		if len(parts) == 2 {
			key := strings.TrimSpace(parts[0])
			value := strings.TrimSpace(parts[1])
			value = strings.Trim(value, "\"'")
			result[key] = value
		}
	}
	return result
}

// --- Helper functions for MCP validation ---

// checkBracketBalance verifies that parentheses, brackets, and braces are balanced.
func checkBracketBalance(code string) error {
	paren, bracket, brace := 0, 0, 0
	inString := false
	stringChar := byte(0)
	escaped := false

	for i := 0; i < len(code); i++ {
		ch := code[i]

		if escaped {
			escaped = false
			continue
		}
		if ch == '\\' && inString {
			escaped = true
			continue
		}

		if !inString {
			if ch == '"' || ch == '\'' {
				inString = true
				stringChar = ch
				continue
			}
			// Skip triple-quoted strings (Python)
			if ch == '`' || (ch == '"' && i+2 < len(code) && code[i+1] == '"' && code[i+2] == '"') {
				inString = true
				stringChar = ch
				continue
			}
			switch ch {
			case '(':
				paren++
			case ')':
				paren--
			case '[':
				bracket++
			case ']':
				bracket--
			case '{':
				brace++
			case '}':
				brace--
			}
		} else {
			if ch == stringChar {
				inString = false
			}
		}
	}

	var errs []string
	if paren < 0 {
		errs = append(errs, "多余的右括号 )")
	} else if paren > 0 {
		errs = append(errs, fmt.Sprintf("缺少 %d 个右括号 )", paren))
	}
	if bracket < 0 {
		errs = append(errs, "多余的右方括号 ]")
	} else if bracket > 0 {
		errs = append(errs, fmt.Sprintf("缺少 %d 个右方括号 ]", bracket))
	}
	if brace < 0 {
		errs = append(errs, "多余的右花括号 }")
	} else if brace > 0 {
		errs = append(errs, fmt.Sprintf("缺少 %d 个右花括号 }", brace))
	}

	if len(errs) > 0 {
		return fmt.Errorf("%s", strings.Join(errs, "; "))
	}
	return nil
}

// detectMCPLanguage detects whether the MCP code is Python or Go.
func detectMCPLanguage(code string) string {
	if strings.Contains(code, "package ") && strings.Contains(code, "func ") {
		return "go"
	}
	if strings.Contains(code, "def ") || strings.Contains(code, "import ") && strings.Contains(code, "from ") {
		return "python"
	}
	// Check for Python shebang or common Python patterns
	if strings.HasPrefix(strings.TrimSpace(code), "#!") && strings.Contains(code, "python") {
		return "python"
	}
	if strings.Contains(code, "class ") && (strings.Contains(code, "def ") || strings.Contains(code, "async def ")) {
		return "python"
	}
	if strings.Contains(code, "async def ") || regexp.MustCompile(`(?m)^\s*def\s`).MatchString(code) {
		return "python"
	}
	return ""
}

// checkMCPServerStructure verifies the MCP server has proper protocol structure.
// Python: Server/FastMCP init + tool registration + run entry.
// Go: main function + MCP-related imports.
func checkMCPServerStructure(code string, lang string) error {
	switch lang {
	case "python":
		// Must have Server/FastMCP initialization
		hasServerInit := strings.Contains(code, "Server(") ||
			strings.Contains(code, "FastMCP(") ||
			strings.Contains(code, "MCPServer(")
		if !hasServerInit {
			return fmt.Errorf("Python MCP 缺少 Server/FastMCP 初始化")
		}

		// Must have tool registration
		hasToolReg := strings.Contains(code, "@server.tool") ||
			strings.Contains(code, "@mcp.tool") ||
			strings.Contains(code, "server.tool(") ||
			strings.Contains(code, "mcp.tool(")
		if !hasToolReg {
			return fmt.Errorf("Python MCP 缺少工具注册（@server.tool / @mcp.tool / server.tool()）")
		}

		// Must have run/serve entry or __main__
		hasRunEntry := strings.Contains(code, ".run(") ||
			strings.Contains(code, ".serve(") ||
			strings.Contains(code, "__main__")
		if !hasRunEntry {
			return fmt.Errorf("Python MCP 缺少运行入口（.run() / .serve() / __main__）")
		}
		return nil

	case "go":
		// Must have func main
		if !regexp.MustCompile(`(?m)^func\s+main\s*\(`).MatchString(code) {
			return fmt.Errorf("Go MCP 缺少 func main()")
		}
		return nil

	default:
		return fmt.Errorf("未知语言类型: %q", lang)
	}
}

// checkFunctionCompleteness verifies that function definitions have bodies.
// Python: def/async def followed by indented lines.
// Go: func followed by {.
func checkFunctionCompleteness(code string, lang string) error {
	switch lang {
	case "python":
		re := regexp.MustCompile(`(?m)^(?:async\s+)?def\s+\w+.*:\s*$`)
		matches := re.FindAllStringIndex(code, -1)
		for _, loc := range matches {
			// Find next non-empty line and check indentation
			after := code[loc[1]:]
			lines := strings.SplitN(after, "\n", 3)
			// First line after def should be indented (non-empty, starts with space/tab)
			if len(lines) < 2 {
				return fmt.Errorf("Python 函数定义后缺少函数体")
			}
			nextLine := lines[1]
			if strings.TrimSpace(nextLine) == "" {
				return fmt.Errorf("Python 函数定义后缺少函数体（空行）")
			}
			if !strings.HasPrefix(nextLine, " ") && !strings.HasPrefix(nextLine, "\t") {
				return fmt.Errorf("Python 函数体缺少缩进")
			}
		}
		return nil

	case "go":
		// Check that func definitions have opening brace
		re := regexp.MustCompile(`(?m)^func\s+`)
		for _, line := range strings.Split(code, "\n") {
			if re.MatchString(line) && !strings.Contains(line, "{") {
				// Function signature without brace on same line — could be on next line
				// Just check the pattern is reasonable
				stripped := strings.TrimSpace(line)
				if strings.HasSuffix(stripped, ")") || strings.HasSuffix(stripped, "type") {
					// Might have brace on next line — this is OK for Go
					continue
				}
				// Missing opening brace entirely in function definition
				return fmt.Errorf("Go 函数定义缺少左花括号 {")
			}
		}
		return nil

	default:
		return fmt.Errorf("未知语言类型: %q", lang)
	}
}
