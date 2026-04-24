package workflow

import (
	"context"
	"fmt"
	"net/http"
	"strings"
	"time"
)

// --- LLMNode ---

// LLMNode invokes an LLM with a prompt. Actual LLM integration is done
// via callbacks injected through the Engine.
type LLMNode struct{}

func (n *LLMNode) Type() string { return "llm" }

func (n *LLMNode) Execute(ctx context.Context, node *NodeDef, wfCtx *Context) (*NodeResult, error) {
	start := time.Now()

	prompt := getConfigString(node.Config, "prompt", "")
	prompt = wfCtx.Resolve(prompt)

	model := getConfigString(node.Config, "model", "default")

	// Placeholder output. Real LLM integration replaces this via Engine callbacks.
	output := fmt.Sprintf("LLM execution (model=%s): %s", model, prompt)

	return &NodeResult{
		NodeID:    node.ID,
		Output:    output,
		State:     StateCompleted,
		StartedAt: start,
		EndedAt:   time.Now(),
	}, nil
}

// --- ToolNode ---

// ToolNode invokes a named tool. Actual tool execution is done via
// callbacks injected through the Engine.
type ToolNode struct{}

func (n *ToolNode) Type() string { return "tool" }

func (n *ToolNode) Execute(ctx context.Context, node *NodeDef, wfCtx *Context) (*NodeResult, error) {
	start := time.Now()

	tool := getConfigString(node.Config, "tool", "")
	args := getConfigMap(node.Config, "args")

	// Resolve args values
	resolvedArgs := make(map[string]interface{}, len(args))
	for k, v := range args {
		if s, ok := v.(string); ok {
			resolvedArgs[k] = wfCtx.Resolve(s)
		} else {
			resolvedArgs[k] = v
		}
	}

	// Placeholder output. Real tool execution replaces this via Engine callbacks.
	output := fmt.Sprintf("Tool execution (tool=%s): %v", tool, resolvedArgs)

	return &NodeResult{
		NodeID:    node.ID,
		Output:    output,
		State:     StateCompleted,
		StartedAt: start,
		EndedAt:   time.Now(),
	}, nil
}

// --- ConditionNode ---

// ConditionNode evaluates an expression and returns a boolean result.
// The output is stored as a map with "result" key (bool) for downstream routing.
type ConditionNode struct{}

func (n *ConditionNode) Type() string { return "condition" }

func (n *ConditionNode) Execute(ctx context.Context, node *NodeDef, wfCtx *Context) (*NodeResult, error) {
	start := time.Now()

	expr := getConfigString(node.Config, "expression", "")
	expr = wfCtx.Resolve(expr)

	result, err := Evaluate(expr, wfCtx.GetAllVariables())
	if err != nil {
		return &NodeResult{
			NodeID:    node.ID,
			Error:     fmt.Sprintf("condition evaluation error: %v", err),
			State:     StateFailed,
			StartedAt: start,
			EndedAt:   time.Now(),
		}, fmt.Errorf("condition evaluation error: %w", err)
	}

	return &NodeResult{
		NodeID: node.ID,
		Output: map[string]interface{}{
			"result": result,
		},
		State:     StateCompleted,
		StartedAt: start,
		EndedAt:   time.Now(),
	}, nil
}

// --- ParallelNode ---

// ParallelNode executes sub-nodes concurrently. Sub-nodes are defined
// in config.nodes as a list of NodeDef objects.
type ParallelNode struct{}

func (n *ParallelNode) Type() string { return "parallel" }

func (n *ParallelNode) Execute(ctx context.Context, node *NodeDef, wfCtx *Context) (*NodeResult, error) {
	start := time.Now()

	subNodes := getConfigNodeList(node.Config, "nodes")
	if len(subNodes) == 0 {
		return &NodeResult{
			NodeID:    node.ID,
			Output:    map[string]interface{}{"results": []interface{}{}},
			State:     StateCompleted,
			StartedAt: start,
			EndedAt:   time.Now(),
		}, nil
	}

	type parallelResult struct {
		nodeID string
		result *NodeResult
		err    error
	}

	ch := make(chan parallelResult, len(subNodes))

	for _, subDef := range subNodes {
		go func(def NodeDef) {
			executor := DefaultExecutorRegistry().Get(def.Type)
			if executor == nil {
				ch <- parallelResult{
					nodeID: def.ID,
					err:    fmt.Errorf("unknown node type %q in parallel block", def.Type),
				}
				return
			}

			subCtx := wfCtx.Clone()
			result, err := executor.Execute(ctx, &def, subCtx)
			ch <- parallelResult{nodeID: def.ID, result: result, err: err}
		}(subDef)
	}

	results := make(map[string]interface{})
	var firstErr error
	for i := 0; i < len(subNodes); i++ {
		r := <-ch
		if r.err != nil {
			if firstErr == nil {
				firstErr = r.err
			}
			results[r.nodeID] = map[string]interface{}{"error": r.err.Error()}
		} else {
			results[r.nodeID] = r.result.Output
			wfCtx.SetNodeResult(r.nodeID, r.result)
		}
	}

	state := StateCompleted
	if firstErr != nil {
		state = StateFailed
	}

	return &NodeResult{
		NodeID:    node.ID,
		Output:    results,
		State:     state,
		StartedAt: start,
		EndedAt:   time.Now(),
	}, firstErr
}

// --- LoopNode ---

// LoopNode executes a sub-node repeatedly until a condition is met or
// max_iterations is reached.
type LoopNode struct{}

func (n *LoopNode) Type() string { return "loop" }

func (n *LoopNode) Execute(ctx context.Context, node *NodeDef, wfCtx *Context) (*NodeResult, error) {
	start := time.Now()

	maxIter := getConfigInt(node.Config, "max_iterations", 10)
	condExpr := getConfigString(node.Config, "condition", "")

	subNodes := getConfigNodeList(node.Config, "nodes")
	if len(subNodes) == 0 {
		return &NodeResult{
			NodeID:    node.ID,
			Output:    map[string]interface{}{"iterations": 0},
			State:     StateCompleted,
			StartedAt: start,
			EndedAt:   time.Now(),
		}, nil
	}

	var lastOutput interface{}
	var iterationErr error

	for i := 0; i < maxIter; i++ {
		// Check loop condition (if provided)
		if condExpr != "" && i > 0 {
			resolved := wfCtx.Resolve(condExpr)
			condResult, err := Evaluate(resolved, wfCtx.GetAllVariables())
			if err != nil {
				iterationErr = fmt.Errorf("loop condition error at iteration %d: %w", i, err)
				break
			}
			if !condResult {
				break
			}
		}

		// Check context cancellation
		select {
		case <-ctx.Done():
			return &NodeResult{
				NodeID:    node.ID,
				Output:    map[string]interface{}{"iterations": i, "cancelled": true},
				State:     StateCancelled,
				StartedAt: start,
				EndedAt:   time.Now(),
			}, ctx.Err()
		default:
		}

		// Execute sub-nodes sequentially within the loop body
		for _, subDef := range subNodes {
			executor := DefaultExecutorRegistry().Get(subDef.Type)
			if executor == nil {
				iterationErr = fmt.Errorf("unknown node type %q in loop body", subDef.Type)
				break
			}

			result, err := executor.Execute(ctx, &subDef, wfCtx)
			if err != nil {
				iterationErr = fmt.Errorf("loop iteration %d node %q: %w", i, subDef.ID, err)
				break
			}
			wfCtx.SetNodeResult(subDef.ID, result)
			lastOutput = result.Output
		}

		if iterationErr != nil {
			break
		}

		wfCtx.SetVar("loop_index", fmt.Sprintf("%d", i))
	}

	return &NodeResult{
		NodeID: node.ID,
		Output: map[string]interface{}{
			"iterations":   maxIter,
			"last_output":  lastOutput,
		},
		State:     StateCompleted,
		StartedAt: start,
		EndedAt:   time.Now(),
	}, iterationErr
}

// --- SubWorkflowNode ---

// SubWorkflowNode recursively executes another registered workflow.
type SubWorkflowNode struct {
	// Engine reference is set via the executor registry at registration time.
	Engine *Engine
}

func (n *SubWorkflowNode) Type() string { return "sub_workflow" }

func (n *SubWorkflowNode) Execute(ctx context.Context, node *NodeDef, wfCtx *Context) (*NodeResult, error) {
	start := time.Now()

	workflowName := getConfigString(node.Config, "workflow", "")
	if workflowName == "" {
		return &NodeResult{
			NodeID:    node.ID,
			Error:     "sub_workflow requires 'workflow' config",
			State:     StateFailed,
			StartedAt: start,
			EndedAt:   time.Now(),
		}, fmt.Errorf("sub_workflow node %q requires 'workflow' config", node.ID)
	}

	// Build sub-workflow input from config
	subInput := getConfigMap(node.Config, "input")
	resolvedInput := make(map[string]interface{}, len(subInput))
	for k, v := range subInput {
		if s, ok := v.(string); ok {
			resolvedInput[k] = wfCtx.Resolve(s)
		} else {
			resolvedInput[k] = v
		}
	}

	if n.Engine == nil {
		return &NodeResult{
			NodeID:    node.ID,
			Error:     "sub_workflow engine not configured",
			State:     StateFailed,
			StartedAt: start,
			EndedAt:   time.Now(),
		}, fmt.Errorf("sub_workflow node %q: engine not configured", node.ID)
	}

	exec, err := n.Engine.Run(ctx, workflowName, resolvedInput)
	if err != nil {
		return &NodeResult{
			NodeID:    node.ID,
			Error:     err.Error(),
			State:     StateFailed,
			StartedAt: start,
			EndedAt:   time.Now(),
		}, err
	}

	return &NodeResult{
		NodeID:    node.ID,
		Output:    exec.NodeResults,
		State:     exec.State,
		StartedAt: start,
		EndedAt:   time.Now(),
		Metadata: map[string]interface{}{
			"execution_id": exec.ID,
		},
	}, nil
}

// --- TransformNode ---

// TransformNode applies template transformations to produce output data.
type TransformNode struct{}

func (n *TransformNode) Type() string { return "transform" }

func (n *TransformNode) Execute(ctx context.Context, node *NodeDef, wfCtx *Context) (*NodeResult, error) {
	start := time.Now()

	// Output is a map of key -> template expression
	outputDefs := getConfigMap(node.Config, "output")
	if len(outputDefs) == 0 {
		// Try single template
		template := getConfigString(node.Config, "template", "")
		if template == "" {
			return &NodeResult{
				NodeID:    node.ID,
				Output:    "",
				State:     StateCompleted,
				StartedAt: start,
				EndedAt:   time.Now(),
			}, nil
		}
		resolved := wfCtx.Resolve(template)
		return &NodeResult{
			NodeID:    node.ID,
			Output:    resolved,
			State:     StateCompleted,
			StartedAt: start,
			EndedAt:   time.Now(),
		}, nil
	}

	output := make(map[string]interface{}, len(outputDefs))
	for k, v := range outputDefs {
		if s, ok := v.(string); ok {
			output[k] = wfCtx.Resolve(s)
		} else {
			output[k] = v
		}
	}

	return &NodeResult{
		NodeID:    node.ID,
		Output:    output,
		State:     StateCompleted,
		StartedAt: start,
		EndedAt:   time.Now(),
	}, nil
}

// --- HTTPNode ---

// HTTPNode makes an HTTP request and returns the response.
type HTTPNode struct {
	Client *http.Client
}

func (n *HTTPNode) Type() string { return "http" }

func (n *HTTPNode) Execute(ctx context.Context, node *NodeDef, wfCtx *Context) (*NodeResult, error) {
	start := time.Now()

	url := wfCtx.Resolve(getConfigString(node.Config, "url", ""))
	method := strings.ToUpper(getConfigString(node.Config, "method", "GET"))
	body := wfCtx.Resolve(getConfigString(node.Config, "body", ""))

	if url == "" {
		return &NodeResult{
			NodeID:    node.ID,
			Error:     "http node requires 'url' config",
			State:     StateFailed,
			StartedAt: start,
			EndedAt:   time.Now(),
		}, fmt.Errorf("http node %q requires 'url' config", node.ID)
	}

	client := n.Client
	if client == nil {
		client = http.DefaultClient
	}

	var bodyReader *strings.Reader
	if body != "" {
		bodyReader = strings.NewReader(body)
	}

	var req *http.Request
	var err error
	if bodyReader != nil {
		req, err = http.NewRequestWithContext(ctx, method, url, bodyReader)
	} else {
		req, err = http.NewRequestWithContext(ctx, method, url, nil)
	}
	if err != nil {
		return &NodeResult{
			NodeID:    node.ID,
			Error:     err.Error(),
			State:     StateFailed,
			StartedAt: start,
			EndedAt:   time.Now(),
		}, err
	}

	// Set headers
	headers := getConfigMap(node.Config, "headers")
	for k, v := range headers {
		if s, ok := v.(string); ok {
			req.Header.Set(k, wfCtx.Resolve(s))
		}
	}

	resp, err := client.Do(req)
	if err != nil {
		return &NodeResult{
			NodeID:    node.ID,
			Error:     err.Error(),
			State:     StateFailed,
			StartedAt: start,
			EndedAt:   time.Now(),
		}, err
	}
	defer resp.Body.Close()

	// Read response body (limit to 1MB)
	buf := make([]byte, 1024*1024)
	nRead, _ := resp.Body.Read(buf)
	responseBody := string(buf[:nRead])

	return &NodeResult{
		NodeID: node.ID,
		Output: map[string]interface{}{
			"status_code": resp.StatusCode,
			"headers":     resp.Header,
			"body":        responseBody,
		},
		State:     StateCompleted,
		StartedAt: start,
		EndedAt:   time.Now(),
	}, nil
}

// --- ScriptNode ---

// ScriptNode executes a script. This is a placeholder that returns
// the script content as output. Real script execution is done via
// Engine callbacks or OS-level execution.
type ScriptNode struct{}

func (n *ScriptNode) Type() string { return "script" }

func (n *ScriptNode) Execute(ctx context.Context, node *NodeDef, wfCtx *Context) (*NodeResult, error) {
	start := time.Now()

	script := getConfigString(node.Config, "script", "")
	language := getConfigString(node.Config, "language", "bash")

	script = wfCtx.Resolve(script)

	// Placeholder: actual script execution via Engine callbacks.
	output := fmt.Sprintf("Script execution (%s): %s", language, script)

	return &NodeResult{
		NodeID:    node.ID,
		Output:    output,
		State:     StateCompleted,
		StartedAt: start,
		EndedAt:   time.Now(),
	}, nil
}

// --- DelayNode ---

// DelayNode pauses execution for a configured duration.
type DelayNode struct{}

func (n *DelayNode) Type() string { return "delay" }

func (n *DelayNode) Execute(ctx context.Context, node *NodeDef, wfCtx *Context) (*NodeResult, error) {
	start := time.Now()

	durationStr := getConfigString(node.Config, "duration", "1s")
	d, err := time.ParseDuration(durationStr)
	if err != nil {
		return &NodeResult{
			NodeID:    node.ID,
			Error:     fmt.Sprintf("invalid duration %q: %v", durationStr, err),
			State:     StateFailed,
			StartedAt: start,
			EndedAt:   time.Now(),
		}, fmt.Errorf("invalid duration %q: %w", durationStr, err)
	}

	select {
	case <-time.After(d):
		return &NodeResult{
			NodeID:    node.ID,
			Output:    map[string]interface{}{"delayed": durationStr},
			State:     StateCompleted,
			StartedAt: start,
			EndedAt:   time.Now(),
		}, nil
	case <-ctx.Done():
		return &NodeResult{
			NodeID:    node.ID,
			Output:    map[string]interface{}{"delayed": "cancelled"},
			State:     StateCancelled,
			StartedAt: start,
			EndedAt:   time.Now(),
		}, ctx.Err()
	}
}

// --- HumanReviewNode ---

// HumanReviewNode pauses workflow execution until a human reviews and
// approves/rejects the current state.
type HumanReviewNode struct{}

func (n *HumanReviewNode) Type() string { return "human_review" }

func (n *HumanReviewNode) Execute(ctx context.Context, node *NodeDef, wfCtx *Context) (*NodeResult, error) {
	start := time.Now()

	message := getConfigString(node.Config, "message", "Human review required")
	message = wfCtx.Resolve(message)

	// Return waiting state. The engine is responsible for pausing the
	// workflow and resuming when a human review is submitted.
	return &NodeResult{
		NodeID: node.ID,
		Output: map[string]interface{}{
			"message": message,
			"status":  "waiting_for_review",
		},
		State:     StateWaiting,
		StartedAt: start,
		EndedAt:   time.Now(),
	}, nil
}

// --- ExecutorRegistry ---

// ExecutorRegistry maps node types to NodeExecutor implementations.
type ExecutorRegistry struct {
	executors map[string]NodeExecutor
}

// DefaultExecutorRegistry returns a registry pre-loaded with all built-in node types.
func DefaultExecutorRegistry() *ExecutorRegistry {
	return &ExecutorRegistry{
		executors: map[string]NodeExecutor{
			"llm":           &LLMNode{},
			"tool":          &ToolNode{},
			"condition":     &ConditionNode{},
			"parallel":      &ParallelNode{},
			"loop":          &LoopNode{},
			"transform":     &TransformNode{},
			"http":          &HTTPNode{},
			"script":        &ScriptNode{},
			"delay":         &DelayNode{},
			"human_review":  &HumanReviewNode{},
			// sub_workflow is registered dynamically by Engine
		},
	}
}

// Get returns the executor for a given node type, or nil.
func (r *ExecutorRegistry) Get(nodeType string) NodeExecutor {
	return r.executors[nodeType]
}

// Register adds or replaces an executor for a node type.
func (r *ExecutorRegistry) Register(nodeType string, executor NodeExecutor) {
	r.executors[nodeType] = executor
}

// --- Config helpers ---

func getConfigString(config map[string]interface{}, key, defaultVal string) string {
	if v, ok := config[key]; ok {
		if s, ok := v.(string); ok {
			return s
		}
		return fmt.Sprintf("%v", v)
	}
	return defaultVal
}

func getConfigInt(config map[string]interface{}, key string, defaultVal int) int {
	if v, ok := config[key]; ok {
		switch n := v.(type) {
		case int:
			return n
		case int64:
			return int(n)
		case float64:
			return int(n)
		}
	}
	return defaultVal
}

func getConfigMap(config map[string]interface{}, key string) map[string]interface{} {
	if v, ok := config[key]; ok {
		if m, ok := v.(map[string]interface{}); ok {
			return m
		}
	}
	return nil
}

func getConfigNodeList(config map[string]interface{}, key string) []NodeDef {
	if v, ok := config[key]; ok {
		// YAML unmarshals to []interface{}
		if list, ok := v.([]interface{}); ok {
			var nodes []NodeDef
			for _, item := range list {
				if m, ok := item.(map[string]interface{}); ok {
					nd := NodeDef{
						ID:     getStringFromMap(m, "id"),
						Type:   getStringFromMap(m, "type"),
						Config: getMapFromMap(m, "config"),
					}
					nodes = append(nodes, nd)
				}
			}
			return nodes
		}
	}
	return nil
}

func getStringFromMap(m map[string]interface{}, key string) string {
	if v, ok := m[key]; ok {
		if s, ok := v.(string); ok {
			return s
		}
		return fmt.Sprintf("%v", v)
	}
	return ""
}

func getMapFromMap(m map[string]interface{}, key string) map[string]interface{} {
	if v, ok := m[key]; ok {
		if sub, ok := v.(map[string]interface{}); ok {
			return sub
		}
	}
	return nil
}
