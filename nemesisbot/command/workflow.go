package command

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"

	"github.com/276793422/NemesisBot/module/workflow"
)

// CmdWorkflow manages the workflow engine.
func CmdWorkflow() {
	if len(os.Args) < 3 {
		WorkflowHelp()
		return
	}

	subcommand := os.Args[2]

	switch subcommand {
	case "list":
		cmdWorkflowList()
	case "run":
		cmdWorkflowRun()
	case "status":
		cmdWorkflowStatus()
	case "template":
		cmdWorkflowTemplate()
	default:
		fmt.Printf("Unknown workflow command: %s\n", subcommand)
		WorkflowHelp()
	}
}

// WorkflowHelp prints workflow command help.
func WorkflowHelp() {
	fmt.Println("\nWorkflow commands (DAG workflow engine):")
	fmt.Println("  list                   List registered workflows")
	fmt.Println("  run <name> [input]     Run a workflow by name")
	fmt.Println("  status [id]            Show execution status")
	fmt.Println("  template               List available built-in templates")
	fmt.Println()
	fmt.Println("Examples:")
	fmt.Println("  nemesisbot workflow list                  # List all registered workflows")
	fmt.Println("  nemesisbot workflow run coder             # Run the coder workflow")
	fmt.Println("  nemesisbot workflow status                # Show all executions")
	fmt.Println("  nemesisbot workflow status <execution-id> # Show specific execution")
	fmt.Println("  nemesisbot workflow template              # List built-in templates")
	fmt.Println()
	fmt.Println("Configuration:")
	fmt.Println("  Workflow templates:  module/workflow/templates/")
	fmt.Println("  Execution storage:   workspace/workflow/executions/")
}

// cmdWorkflowList lists all registered workflows by scanning the workspace
// workflow directory and parsing any YAML definitions found.
func cmdWorkflowList() {
	cfg, err := LoadConfig()
	if err != nil {
		fmt.Printf("Error loading config: %v\n", err)
		os.Exit(1)
	}

	workspace := cfg.WorkspacePath()
	workflowDir := filepath.Join(workspace, "workflow")

	fmt.Println("\n=== Workflow Engine ===")
	fmt.Println()

	// Scan for workflow definitions in workspace
	workflows := scanWorkflowFiles(workflowDir)

	if len(workflows) == 0 {
		fmt.Println("  No workflows registered.")
		fmt.Println()
		fmt.Println("  Use 'nemesisbot workflow template' to see built-in templates.")
		return
	}

	fmt.Printf("  Registered Workflows (%d):\n\n", len(workflows))
	fmt.Println("  Name                 | Version | Description                           | Triggers")
	fmt.Println("  ---------------------|---------|---------------------------------------|----------")
	for _, wf := range workflows {
		triggers := "none"
		if len(wf.Triggers) > 0 {
			types := make([]string, 0, len(wf.Triggers))
			for _, t := range wf.Triggers {
				types = append(types, t.Type)
			}
			triggers = strings.Join(types, ", ")
		}

		desc := wf.Description
		if len(desc) > 39 {
			desc = desc[:36] + "..."
		}
		fmt.Printf("  %-20s | %-7s | %-37s | %s\n", wf.Name, wf.Version, desc, triggers)
	}

	// Show execution count
	execDir := filepath.Join(workflowDir, "executions")
	executions, err := workflow.ListExecutionsFromDisk(execDir, "")
	if err != nil {
		executions = nil
	}
	fmt.Printf("\n  Total executions: %d\n", len(executions))
}

// cmdWorkflowRun runs a workflow by name.
func cmdWorkflowRun() {
	if len(os.Args) < 4 {
		fmt.Println("Usage: nemesisbot workflow run <name> [key=value ...]")
		fmt.Println()
		fmt.Println("Example:")
		fmt.Println("  nemesisbot workflow run coder")
		fmt.Println("  nemesisbot workflow run translator target_language=English")
		return
	}

	name := os.Args[3]

	// Parse key=value input arguments
	input := make(map[string]interface{})
	for i := 4; i < len(os.Args); i++ {
		parts := strings.SplitN(os.Args[i], "=", 2)
		if len(parts) == 2 {
			input[parts[0]] = parts[1]
		} else {
			// If no key provided, use "input" as the key
			if _, exists := input["input"]; !exists {
				input["input"] = parts[0]
			}
		}
	}

	cfg, err := LoadConfig()
	if err != nil {
		fmt.Printf("Error loading config: %v\n", err)
		os.Exit(1)
	}

	workspace := cfg.WorkspacePath()
	workflowDir := filepath.Join(workspace, "workflow")
	execDir := filepath.Join(workflowDir, "executions")

	// Create the engine
	engine := workflow.NewEngine(execDir)

	// Register workflows from workspace
	workflows := scanWorkflowFiles(workflowDir)

	// Check if it's a built-in template name
	found := false
	for _, wf := range workflows {
		if wf.Name == name {
			if err := engine.Register(wf); err != nil {
				fmt.Printf("Error registering workflow %q: %v\n", name, err)
				os.Exit(1)
			}
			found = true
			break
		}
	}

	if !found {
		// Try loading from built-in templates
		templateDir := getWorkflowTemplateDir()
		templatePath := filepath.Join(templateDir, name+".yaml")
		wf, err := workflow.ParseFile(templatePath)
		if err != nil {
			fmt.Printf("Workflow %q not found.\n", name)
			fmt.Println("Use 'nemesisbot workflow list' to see registered workflows.")
			fmt.Println("Use 'nemesisbot workflow template' to see built-in templates.")
			os.Exit(1)
		}
		if err := engine.Register(wf); err != nil {
			fmt.Printf("Error registering workflow %q: %v\n", name, err)
			os.Exit(1)
		}
	}

	fmt.Printf("Running workflow: %s\n", name)
	if len(input) > 0 {
		fmt.Printf("  Input: %v\n", input)
	}

	exec, err := engine.Run(context.Background(), name, input)
	if err != nil {
		fmt.Printf("Workflow execution failed: %v\n", err)
		os.Exit(1)
	}

	fmt.Println()
	fmt.Printf("  Execution ID:    %s\n", exec.ID)
	fmt.Printf("  State:           %s\n", exec.State)
	fmt.Printf("  Started:         %s\n", exec.StartedAt.Format("2006-01-02 15:04:05"))
	if !exec.EndedAt.IsZero() {
		fmt.Printf("  Ended:           %s\n", exec.EndedAt.Format("2006-01-02 15:04:05"))
		duration := exec.EndedAt.Sub(exec.StartedAt)
		fmt.Printf("  Duration:        %s\n", duration.Round(time.Millisecond))
	}
	if exec.Error != "" {
		fmt.Printf("  Error:           %s\n", exec.Error)
	}

	// Show node results
	if len(exec.NodeResults) > 0 {
		fmt.Println()
		fmt.Println("  Node Results:")
		for nodeID, result := range exec.NodeResults {
			fmt.Printf("    [%s] %s", nodeID, result.State)
			if result.Error != "" {
				fmt.Printf(" - %s", result.Error)
			}
			fmt.Println()
		}
	}
}

// cmdWorkflowStatus shows execution status.
func cmdWorkflowStatus() {
	cfg, err := LoadConfig()
	if err != nil {
		fmt.Printf("Error loading config: %v\n", err)
		os.Exit(1)
	}

	workspace := cfg.WorkspacePath()
	execDir := filepath.Join(workspace, "workflow", "executions")

	// Show specific execution
	if len(os.Args) >= 4 {
		execID := os.Args[3]
		exec, err := workflow.LoadExecutionByID(execDir, execID)
		if err != nil {
			fmt.Printf("Execution %q not found: %v\n", execID, err)
			os.Exit(1)
		}
		printExecutionDetail(exec)
		return
	}

	// List all executions
	executions, err := workflow.ListExecutionsFromDisk(execDir, "")
	if err != nil || len(executions) == 0 {
		fmt.Println("\n  No workflow executions found.")
		return
	}

	// Sort by started time (newest first)
	sort.Slice(executions, func(i, j int) bool {
		return executions[i].StartedAt.After(executions[j].StartedAt)
	})

	fmt.Printf("\nWorkflow Executions (%d):\n\n", len(executions))
	fmt.Println("  ID                                 | Workflow     | State     | Started")
	fmt.Println("  -----------------------------------|--------------|-----------|-------------------")
	for _, exec := range executions {
		shortID := exec.ID
		if len(shortID) > 36 {
			shortID = shortID[:36]
		}
		fmt.Printf("  %-36s | %-12s | %-9s | %s\n",
			shortID,
			exec.WorkflowName,
			exec.State,
			exec.StartedAt.Format("2006-01-02 15:04:05"))
	}
}

// cmdWorkflowTemplate lists available built-in workflow templates.
func cmdWorkflowTemplate() {
	templateDir := getWorkflowTemplateDir()

	fmt.Println("\n=== Workflow Templates ===")
	fmt.Println()

	// List template files from the templates directory
	entries, err := os.ReadDir(templateDir)
	if err != nil {
		fmt.Println("  No templates found.")
		fmt.Printf("  Template directory: %s\n", templateDir)
		return
	}

	templates := make([]*workflow.Workflow, 0, len(entries))
	for _, entry := range entries {
		if entry.IsDir() || !strings.HasSuffix(entry.Name(), ".yaml") {
			continue
		}
		path := filepath.Join(templateDir, entry.Name())
		wf, err := workflow.ParseFile(path)
		if err != nil {
			fmt.Printf("  Warning: Failed to parse %s: %v\n", entry.Name(), err)
			continue
		}
		templates = append(templates, wf)
	}

	if len(templates) == 0 {
		fmt.Println("  No templates available.")
		return
	}

	fmt.Printf("  Available Templates (%d):\n\n", len(templates))
	fmt.Println("  Name                 | Version | Nodes | Triggers | Description")
	fmt.Println("  ---------------------|---------|-------|----------|-------------------------------------------")
	for _, wf := range templates {
		triggerCount := len(wf.Triggers)
		triggers := "-"
		if triggerCount > 0 {
			triggers = fmt.Sprintf("%d", triggerCount)
		}

		desc := wf.Description
		if len(desc) > 43 {
			desc = desc[:40] + "..."
		}
		fmt.Printf("  %-20s | %-7s | %-5d | %-8s | %s\n",
			wf.Name, wf.Version, len(wf.Nodes), triggers, desc)
	}

	fmt.Println()
	fmt.Println("  Usage:")
	fmt.Println("    nemesisbot workflow run <template-name>")
	fmt.Println()
	fmt.Println("  Example:")
	fmt.Println("    nemesisbot workflow run coder")
}

// scanWorkflowFiles scans a directory recursively for YAML workflow definitions.
func scanWorkflowFiles(dir string) []*workflow.Workflow {
	var results []*workflow.Workflow

	entries, err := os.ReadDir(dir)
	if err != nil {
		return results
	}

	for _, entry := range entries {
	 fullPath := filepath.Join(dir, entry.Name())

		if entry.IsDir() {
			// Skip executions directory
			if entry.Name() == "executions" {
				continue
			}
			results = append(results, scanWorkflowFiles(fullPath)...)
			continue
		}

		if !strings.HasSuffix(entry.Name(), ".yaml") && !strings.HasSuffix(entry.Name(), ".yml") {
			continue
		}

		wf, err := workflow.ParseFile(fullPath)
		if err != nil {
			continue // skip invalid files
		}
		results = append(results, wf)
	}

	return results
}

// getWorkflowTemplateDir returns the path to the built-in workflow templates directory.
func getWorkflowTemplateDir() string {
	// Resolve relative to the executable's location
	// In development, templates are at module/workflow/templates/
	// Try multiple paths
	candidates := []string{
		filepath.Join("module", "workflow", "templates"),
		filepath.Join("..", "module", "workflow", "templates"),
	}

	// Get executable directory
	if exePath, err := os.Executable(); err == nil {
		exeDir := filepath.Dir(exePath)
		candidates = append([]string{
			filepath.Join(exeDir, "module", "workflow", "templates"),
		}, candidates...)
	}

	// Also try relative to working directory
	for _, dir := range candidates {
		if _, err := os.Stat(dir); err == nil {
			abs, _ := filepath.Abs(dir)
			return abs
		}
	}

	// Fallback: return the first candidate as absolute path
	abs, _ := filepath.Abs(candidates[0])
	return abs
}

// printExecutionDetail prints detailed information about a single execution.
func printExecutionDetail(exec *workflow.Execution) {
	fmt.Println()
	fmt.Println("=== Workflow Execution Detail ===")
	fmt.Println()
	fmt.Printf("  Execution ID:    %s\n", exec.ID)
	fmt.Printf("  Workflow:        %s\n", exec.WorkflowName)
	fmt.Printf("  State:           %s\n", exec.State)
	fmt.Printf("  Started:         %s\n", exec.StartedAt.Format("2006-01-02 15:04:05"))
	if !exec.EndedAt.IsZero() {
		fmt.Printf("  Ended:           %s\n", exec.EndedAt.Format("2006-01-02 15:04:05"))
		duration := exec.EndedAt.Sub(exec.StartedAt)
		fmt.Printf("  Duration:        %s\n", duration.Round(time.Millisecond))
	}

	if exec.Error != "" {
		fmt.Printf("  Error:           %s\n", exec.Error)
	}

	// Show input
	if len(exec.Input) > 0 {
		fmt.Println()
		fmt.Println("  Input:")
		for k, v := range exec.Input {
			fmt.Printf("    %s: %v\n", k, v)
		}
	}

	// Show variables
	if len(exec.Variables) > 0 {
		fmt.Println()
		fmt.Println("  Variables:")
		for k, v := range exec.Variables {
			fmt.Printf("    %s: %s\n", k, v)
		}
	}

	// Show node results
	if len(exec.NodeResults) > 0 {
		fmt.Println()
		fmt.Println("  Node Results:")
		for nodeID, result := range exec.NodeResults {
			fmt.Printf("    [%s] %s\n", nodeID, result.State)
			fmt.Printf("      Started: %s", result.StartedAt.Format("15:04:05"))
			if !result.EndedAt.IsZero() {
				fmt.Printf("  Ended: %s", result.EndedAt.Format("15:04:05"))
				fmt.Printf("  (%s)", result.EndedAt.Sub(result.StartedAt).Round(time.Millisecond))
			}
			fmt.Println()
			if result.Error != "" {
				fmt.Printf("      Error: %s\n", result.Error)
			}
			if result.Output != nil {
				outputStr := fmt.Sprintf("%v", result.Output)
				if len(outputStr) > 200 {
					outputStr = outputStr[:197] + "..."
				}
				fmt.Printf("      Output: %s\n", outputStr)
			}
		}
	}
}
