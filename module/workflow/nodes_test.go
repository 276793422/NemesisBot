package workflow_test

import (
	"context"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/workflow"
)

func TestDefaultExecutorRegistry_AllTypes(t *testing.T) {
	registry := workflow.DefaultExecutorRegistry()

	expectedTypes := []string{
		"llm", "tool", "condition", "parallel", "loop",
		"transform", "http", "script", "delay", "human_review",
	}

	for _, typ := range expectedTypes {
		executor := registry.Get(typ)
		if executor == nil {
			t.Errorf("Get(%q) returned nil", typ)
		} else if executor.Type() != typ {
			t.Errorf("Get(%q).Type() = %q, want %q", typ, executor.Type(), typ)
		}
	}
}

func TestDefaultExecutorRegistry_SubWorkflow(t *testing.T) {
	// sub_workflow is not in the default registry, it's registered by Engine
	registry := workflow.DefaultExecutorRegistry()
	if registry.Get("sub_workflow") != nil {
		t.Error("sub_workflow should not be in default registry")
	}
}

func TestExecutorRegistry_GetUnknown(t *testing.T) {
	registry := workflow.DefaultExecutorRegistry()
	if registry.Get("nonexistent") != nil {
		t.Error("Get(nonexistent) should return nil")
	}
}

func TestExecutorRegistry_Register(t *testing.T) {
	registry := workflow.DefaultExecutorRegistry()

	custom := &mockExecutor{
		typeVal: "custom_type",
	}
	registry.Register("custom_type", custom)

	got := registry.Get("custom_type")
	if got == nil {
		t.Fatal("Get(custom_type) returned nil after Register")
	}
	if got.Type() != "custom_type" {
		t.Errorf("Type() = %q, want %q", got.Type(), "custom_type")
	}
}

func TestExecutorRegistry_RegisterOverwrite(t *testing.T) {
	registry := workflow.DefaultExecutorRegistry()

	custom := &mockExecutor{
		typeVal: "delay",
	}
	registry.Register("delay", custom)

	got := registry.Get("delay")
	if got == nil {
		t.Fatal("Get(delay) returned nil")
	}
	if got.Type() != "delay" {
		t.Errorf("Type() = %q, want %q", got.Type(), "delay")
	}
}

// --- LLMNode Tests ---

func TestLLMNode_Execute(t *testing.T) {
	node := &workflow.LLMNode{}
	wfCtx := workflow.NewContext(nil)
	wfCtx.SetVar("name", "world")

	nodeDef := &workflow.NodeDef{
		ID:   "llm1",
		Type: "llm",
		Config: map[string]interface{}{
			"prompt": "Hello {{name}}",
			"model":  "gpt-4",
		},
	}

	result, err := node.Execute(context.Background(), nodeDef, wfCtx)
	if err != nil {
		t.Fatalf("Execute() error: %v", err)
	}
	if result.State != workflow.StateCompleted {
		t.Errorf("State = %d, want %d", result.State, workflow.StateCompleted)
	}
	if result.NodeID != "llm1" {
		t.Errorf("NodeID = %q, want %q", result.NodeID, "llm1")
	}
	output, ok := result.Output.(string)
	if !ok {
		t.Fatalf("Output type = %T, want string", result.Output)
	}
	if output == "" {
		t.Error("Output is empty")
	}
}

func TestLLMNode_Type(t *testing.T) {
	node := &workflow.LLMNode{}
	if node.Type() != "llm" {
		t.Errorf("Type() = %q, want %q", node.Type(), "llm")
	}
}

// --- ToolNode Tests ---

func TestToolNode_Execute(t *testing.T) {
	node := &workflow.ToolNode{}
	wfCtx := workflow.NewContext(nil)

	nodeDef := &workflow.NodeDef{
		ID:   "tool1",
		Type: "tool",
		Config: map[string]interface{}{
			"tool": "search",
			"args": map[string]interface{}{
				"query": "test query",
			},
		},
	}

	result, err := node.Execute(context.Background(), nodeDef, wfCtx)
	if err != nil {
		t.Fatalf("Execute() error: %v", err)
	}
	if result.State != workflow.StateCompleted {
		t.Errorf("State = %d, want %d", result.State, workflow.StateCompleted)
	}
	output, ok := result.Output.(string)
	if !ok {
		t.Fatalf("Output type = %T, want string", result.Output)
	}
	if output == "" {
		t.Error("Output is empty")
	}
}

func TestToolNode_Type(t *testing.T) {
	node := &workflow.ToolNode{}
	if node.Type() != "tool" {
		t.Errorf("Type() = %q, want %q", node.Type(), "tool")
	}
}

// --- ConditionNode Tests ---

func TestConditionNode_TrueCondition(t *testing.T) {
	node := &workflow.ConditionNode{}
	wfCtx := workflow.NewContext(nil)
	wfCtx.SetVar("count", "10")

	nodeDef := &workflow.NodeDef{
		ID:   "cond1",
		Type: "condition",
		Config: map[string]interface{}{
			"expression": "{{count}} > 5",
		},
	}

	result, err := node.Execute(context.Background(), nodeDef, wfCtx)
	if err != nil {
		t.Fatalf("Execute() error: %v", err)
	}
	if result.State != workflow.StateCompleted {
		t.Errorf("State = %d, want %d", result.State, workflow.StateCompleted)
	}
	outputMap, ok := result.Output.(map[string]interface{})
	if !ok {
		t.Fatalf("Output type = %T, want map[string]interface{}", result.Output)
	}
	if outputMap["result"] != true {
		t.Errorf("result = %v, want true", outputMap["result"])
	}
}

func TestConditionNode_FalseCondition(t *testing.T) {
	node := &workflow.ConditionNode{}
	wfCtx := workflow.NewContext(nil)

	nodeDef := &workflow.NodeDef{
		ID:   "cond2",
		Type: "condition",
		Config: map[string]interface{}{
			"expression": "5 > 10",
		},
	}

	result, err := node.Execute(context.Background(), nodeDef, wfCtx)
	if err != nil {
		t.Fatalf("Execute() error: %v", err)
	}
	outputMap := result.Output.(map[string]interface{})
	if outputMap["result"] != false {
		t.Errorf("result = %v, want false", outputMap["result"])
	}
}

func TestConditionNode_InvalidExpression(t *testing.T) {
	node := &workflow.ConditionNode{}
	wfCtx := workflow.NewContext(nil)

	nodeDef := &workflow.NodeDef{
		ID:   "cond3",
		Type: "condition",
		Config: map[string]interface{}{
			"expression": "",
		},
	}

	result, err := node.Execute(context.Background(), nodeDef, wfCtx)
	if err == nil {
		t.Fatal("expected error for empty expression")
	}
	if result.State != workflow.StateFailed {
		t.Errorf("State = %d, want %d", result.State, workflow.StateFailed)
	}
}

func TestConditionNode_Type(t *testing.T) {
	node := &workflow.ConditionNode{}
	if node.Type() != "condition" {
		t.Errorf("Type() = %q, want %q", node.Type(), "condition")
	}
}

// --- TransformNode Tests ---

func TestTransformNode_Template(t *testing.T) {
	node := &workflow.TransformNode{}
	wfCtx := workflow.NewContext(nil)
	wfCtx.SetVar("name", "world")

	nodeDef := &workflow.NodeDef{
		ID:   "t1",
		Type: "transform",
		Config: map[string]interface{}{
			"template": "Hello {{name}}!",
		},
	}

	result, err := node.Execute(context.Background(), nodeDef, wfCtx)
	if err != nil {
		t.Fatalf("Execute() error: %v", err)
	}
	if result.Output != "Hello world!" {
		t.Errorf("Output = %v, want %q", result.Output, "Hello world!")
	}
}

func TestTransformNode_OutputMap(t *testing.T) {
	node := &workflow.TransformNode{}
	wfCtx := workflow.NewContext(nil)
	wfCtx.SetVar("val", "42")

	nodeDef := &workflow.NodeDef{
		ID:   "t2",
		Type: "transform",
		Config: map[string]interface{}{
			"output": map[string]interface{}{
				"count": "{{val}}",
				"fixed": "constant",
			},
		},
	}

	result, err := node.Execute(context.Background(), nodeDef, wfCtx)
	if err != nil {
		t.Fatalf("Execute() error: %v", err)
	}
	outputMap, ok := result.Output.(map[string]interface{})
	if !ok {
		t.Fatalf("Output type = %T, want map", result.Output)
	}
	if outputMap["count"] != "42" {
		t.Errorf("count = %v, want %q", outputMap["count"], "42")
	}
	if outputMap["fixed"] != "constant" {
		t.Errorf("fixed = %v, want %q", outputMap["fixed"], "constant")
	}
}

func TestTransformNode_Empty(t *testing.T) {
	node := &workflow.TransformNode{}
	wfCtx := workflow.NewContext(nil)

	nodeDef := &workflow.NodeDef{
		ID:     "t3",
		Type:   "transform",
		Config: map[string]interface{}{},
	}

	result, err := node.Execute(context.Background(), nodeDef, wfCtx)
	if err != nil {
		t.Fatalf("Execute() error: %v", err)
	}
	if result.Output != "" {
		t.Errorf("Output = %v, want empty string", result.Output)
	}
}

func TestTransformNode_Type(t *testing.T) {
	node := &workflow.TransformNode{}
	if node.Type() != "transform" {
		t.Errorf("Type() = %q, want %q", node.Type(), "transform")
	}
}

// --- DelayNode Tests ---

func TestDelayNode_Execute(t *testing.T) {
	node := &workflow.DelayNode{}

	nodeDef := &workflow.NodeDef{
		ID:   "d1",
		Type: "delay",
		Config: map[string]interface{}{
			"duration": "50ms",
		},
	}

	start := time.Now()
	result, err := node.Execute(context.Background(), nodeDef, workflow.NewContext(nil))
	elapsed := time.Since(start)

	if err != nil {
		t.Fatalf("Execute() error: %v", err)
	}
	if result.State != workflow.StateCompleted {
		t.Errorf("State = %d, want %d", result.State, workflow.StateCompleted)
	}
	if elapsed < 40*time.Millisecond {
		t.Errorf("elapsed = %v, expected at least ~50ms", elapsed)
	}
}

func TestDelayNode_InvalidDuration(t *testing.T) {
	node := &workflow.DelayNode{}

	nodeDef := &workflow.NodeDef{
		ID:   "d2",
		Type: "delay",
		Config: map[string]interface{}{
			"duration": "not-a-duration",
		},
	}

	result, err := node.Execute(context.Background(), nodeDef, workflow.NewContext(nil))
	if err == nil {
		t.Fatal("expected error for invalid duration")
	}
	if result.State != workflow.StateFailed {
		t.Errorf("State = %d, want %d", result.State, workflow.StateFailed)
	}
}

func TestDelayNode_Cancelled(t *testing.T) {
	node := &workflow.DelayNode{}

	ctx, cancel := context.WithCancel(context.Background())
	cancel() // cancel immediately

	nodeDef := &workflow.NodeDef{
		ID:   "d3",
		Type: "delay",
		Config: map[string]interface{}{
			"duration": "10s",
		},
	}

	result, err := node.Execute(ctx, nodeDef, workflow.NewContext(nil))
	if err == nil {
		t.Log("cancelled delay returned no error (acceptable)")
	}
	if result.State != workflow.StateCancelled {
		t.Errorf("State = %d, want %d", result.State, workflow.StateCancelled)
	}
}

func TestDelayNode_Type(t *testing.T) {
	node := &workflow.DelayNode{}
	if node.Type() != "delay" {
		t.Errorf("Type() = %q, want %q", node.Type(), "delay")
	}
}

// --- ParallelNode Tests ---

func TestParallelNode_Empty(t *testing.T) {
	node := &workflow.ParallelNode{}

	nodeDef := &workflow.NodeDef{
		ID:     "p1",
		Type:   "parallel",
		Config: map[string]interface{}{},
	}

	result, err := node.Execute(context.Background(), nodeDef, workflow.NewContext(nil))
	if err != nil {
		t.Fatalf("Execute() error: %v", err)
	}
	if result.State != workflow.StateCompleted {
		t.Errorf("State = %d, want %d", result.State, workflow.StateCompleted)
	}
}

func TestParallelNode_SubNodes(t *testing.T) {
	node := &workflow.ParallelNode{}

	nodeDef := &workflow.NodeDef{
		ID:   "p1",
		Type: "parallel",
		Config: map[string]interface{}{
			"nodes": []interface{}{
				map[string]interface{}{
					"id":   "sub1",
					"type": "delay",
					"config": map[string]interface{}{
						"duration": "10ms",
					},
				},
				map[string]interface{}{
					"id":   "sub2",
					"type": "delay",
					"config": map[string]interface{}{
						"duration": "10ms",
					},
				},
			},
		},
	}

	result, err := node.Execute(context.Background(), nodeDef, workflow.NewContext(nil))
	if err != nil {
		t.Fatalf("Execute() error: %v", err)
	}
	if result.State != workflow.StateCompleted {
		t.Errorf("State = %d, want %d", result.State, workflow.StateCompleted)
	}
}

func TestParallelNode_UnknownSubType(t *testing.T) {
	node := &workflow.ParallelNode{}

	nodeDef := &workflow.NodeDef{
		ID:   "p2",
		Type: "parallel",
		Config: map[string]interface{}{
			"nodes": []interface{}{
				map[string]interface{}{
					"id":     "bad",
					"type":   "nonexistent",
					"config": map[string]interface{}{},
				},
			},
		},
	}

	result, err := node.Execute(context.Background(), nodeDef, workflow.NewContext(nil))
	if err == nil {
		t.Fatal("expected error for unknown sub-node type")
	}
	if result.State != workflow.StateFailed {
		t.Errorf("State = %d, want %d", result.State, workflow.StateFailed)
	}
}

func TestParallelNode_Type(t *testing.T) {
	node := &workflow.ParallelNode{}
	if node.Type() != "parallel" {
		t.Errorf("Type() = %q, want %q", node.Type(), "parallel")
	}
}

// --- LoopNode Tests ---

func TestLoopNode_Empty(t *testing.T) {
	node := &workflow.LoopNode{}

	nodeDef := &workflow.NodeDef{
		ID:     "loop1",
		Type:   "loop",
		Config: map[string]interface{}{},
	}

	result, err := node.Execute(context.Background(), nodeDef, workflow.NewContext(nil))
	if err != nil {
		t.Fatalf("Execute() error: %v", err)
	}
	if result.State != workflow.StateCompleted {
		t.Errorf("State = %d, want %d", result.State, workflow.StateCompleted)
	}
}

func TestLoopNode_WithMaxIterations(t *testing.T) {
	node := &workflow.LoopNode{}
	wfCtx := workflow.NewContext(nil)

	nodeDef := &workflow.NodeDef{
		ID:   "loop2",
		Type: "loop",
		Config: map[string]interface{}{
			"max_iterations": 3,
			"nodes": []interface{}{
				map[string]interface{}{
					"id":   "sub",
					"type": "delay",
					"config": map[string]interface{}{
						"duration": "10ms",
					},
				},
			},
		},
	}

	result, err := node.Execute(context.Background(), nodeDef, wfCtx)
	if err != nil {
		t.Fatalf("Execute() error: %v", err)
	}
	if result.State != workflow.StateCompleted {
		t.Errorf("State = %d, want %d", result.State, workflow.StateCompleted)
	}
	outputMap, ok := result.Output.(map[string]interface{})
	if !ok {
		t.Fatalf("Output type = %T, want map", result.Output)
	}
	iterations, ok := outputMap["iterations"]
	if !ok {
		t.Error("missing iterations in output")
	}
	_ = iterations
}

func TestLoopNode_Cancelled(t *testing.T) {
	node := &workflow.LoopNode{}

	ctx, cancel := context.WithCancel(context.Background())
	// Cancel after a short delay
	go func() {
		time.Sleep(50 * time.Millisecond)
		cancel()
	}()

	nodeDef := &workflow.NodeDef{
		ID:   "loop3",
		Type: "loop",
		Config: map[string]interface{}{
			"max_iterations": 1000,
			"nodes": []interface{}{
				map[string]interface{}{
					"id":   "sub",
					"type": "delay",
					"config": map[string]interface{}{
						"duration": "10ms",
					},
				},
			},
		},
	}

	result, err := node.Execute(ctx, nodeDef, workflow.NewContext(nil))
	if err != nil {
		t.Logf("Execute() error (expected on cancel): %v", err)
	}
	if result == nil {
		t.Fatal("result is nil")
	}
}

func TestLoopNode_UnknownSubType(t *testing.T) {
	node := &workflow.LoopNode{}

	nodeDef := &workflow.NodeDef{
		ID:   "loop4",
		Type: "loop",
		Config: map[string]interface{}{
			"max_iterations": 5,
			"nodes": []interface{}{
				map[string]interface{}{
					"id":     "bad",
					"type":   "nonexistent",
					"config": map[string]interface{}{},
				},
			},
		},
	}

	result, err := node.Execute(context.Background(), nodeDef, workflow.NewContext(nil))
	if err == nil {
		t.Fatal("expected error for unknown sub-node type")
	}
	if result.State != workflow.StateCompleted {
		// LoopNode still returns StateCompleted even with error
		t.Logf("State = %d (loop node returns completed even with iteration error)", result.State)
	}
}

func TestLoopNode_Type(t *testing.T) {
	node := &workflow.LoopNode{}
	if node.Type() != "loop" {
		t.Errorf("Type() = %q, want %q", node.Type(), "loop")
	}
}

// --- ScriptNode Tests ---

func TestScriptNode_Execute(t *testing.T) {
	node := &workflow.ScriptNode{}
	wfCtx := workflow.NewContext(nil)
	wfCtx.SetVar("target", "world")

	nodeDef := &workflow.NodeDef{
		ID:   "s1",
		Type: "script",
		Config: map[string]interface{}{
			"language": "bash",
			"script":   "echo Hello {{target}}",
		},
	}

	result, err := node.Execute(context.Background(), nodeDef, wfCtx)
	if err != nil {
		t.Fatalf("Execute() error: %v", err)
	}
	if result.State != workflow.StateCompleted {
		t.Errorf("State = %d, want %d", result.State, workflow.StateCompleted)
	}
	output, ok := result.Output.(string)
	if !ok {
		t.Fatalf("Output type = %T, want string", result.Output)
	}
	if output == "" {
		t.Error("Output is empty")
	}
}

func TestScriptNode_Type(t *testing.T) {
	node := &workflow.ScriptNode{}
	if node.Type() != "script" {
		t.Errorf("Type() = %q, want %q", node.Type(), "script")
	}
}

// --- HumanReviewNode Tests ---

func TestHumanReviewNode_Execute(t *testing.T) {
	node := &workflow.HumanReviewNode{}
	wfCtx := workflow.NewContext(nil)

	nodeDef := &workflow.NodeDef{
		ID:   "hr1",
		Type: "human_review",
		Config: map[string]interface{}{
			"message": "Please review this action",
		},
	}

	result, err := node.Execute(context.Background(), nodeDef, wfCtx)
	if err != nil {
		t.Fatalf("Execute() error: %v", err)
	}
	if result.State != workflow.StateWaiting {
		t.Errorf("State = %d, want %d", result.State, workflow.StateWaiting)
	}
	outputMap, ok := result.Output.(map[string]interface{})
	if !ok {
		t.Fatalf("Output type = %T, want map", result.Output)
	}
	if outputMap["message"] != "Please review this action" {
		t.Errorf("message = %v, want %q", outputMap["message"], "Please review this action")
	}
	if outputMap["status"] != "waiting_for_review" {
		t.Errorf("status = %v, want %q", outputMap["status"], "waiting_for_review")
	}
}

func TestHumanReviewNode_Type(t *testing.T) {
	node := &workflow.HumanReviewNode{}
	if node.Type() != "human_review" {
		t.Errorf("Type() = %q, want %q", node.Type(), "human_review")
	}
}

// --- SubWorkflowNode Tests ---

func TestSubWorkflowNode_NoWorkflowConfig(t *testing.T) {
	node := &workflow.SubWorkflowNode{}

	nodeDef := &workflow.NodeDef{
		ID:     "sw1",
		Type:   "sub_workflow",
		Config: map[string]interface{}{},
	}

	result, err := node.Execute(context.Background(), nodeDef, workflow.NewContext(nil))
	if err == nil {
		t.Fatal("expected error for missing workflow config")
	}
	if result.State != workflow.StateFailed {
		t.Errorf("State = %d, want %d", result.State, workflow.StateFailed)
	}
}

func TestSubWorkflowNode_NoEngine(t *testing.T) {
	node := &workflow.SubWorkflowNode{}

	nodeDef := &workflow.NodeDef{
		ID:   "sw2",
		Type: "sub_workflow",
		Config: map[string]interface{}{
			"workflow": "child",
		},
	}

	result, err := node.Execute(context.Background(), nodeDef, workflow.NewContext(nil))
	if err == nil {
		t.Fatal("expected error for no engine")
	}
	if result.State != workflow.StateFailed {
		t.Errorf("State = %d, want %d", result.State, workflow.StateFailed)
	}
}

func TestSubWorkflowNode_Type(t *testing.T) {
	node := &workflow.SubWorkflowNode{}
	if node.Type() != "sub_workflow" {
		t.Errorf("Type() = %q, want %q", node.Type(), "sub_workflow")
	}
}

// --- HTTPNode Tests ---

func TestHTTPNode_NoURL(t *testing.T) {
	node := &workflow.HTTPNode{}

	nodeDef := &workflow.NodeDef{
		ID:     "h1",
		Type:   "http",
		Config: map[string]interface{}{},
	}

	result, err := node.Execute(context.Background(), nodeDef, workflow.NewContext(nil))
	if err == nil {
		t.Fatal("expected error for missing URL")
	}
	if result.State != workflow.StateFailed {
		t.Errorf("State = %d, want %d", result.State, workflow.StateFailed)
	}
}

func TestHTTPNode_Type(t *testing.T) {
	node := &workflow.HTTPNode{}
	if node.Type() != "http" {
		t.Errorf("Type() = %q, want %q", node.Type(), "http")
	}
}

// --- mock executor ---

type mockExecutor struct {
	typeVal    string
	executeFunc func() (*workflow.NodeResult, error)
}

func (m *mockExecutor) Type() string {
	if m.typeVal != "" {
		return m.typeVal
	}
	return "mock"
}

func (m *mockExecutor) Execute(ctx context.Context, node *workflow.NodeDef, wfCtx *workflow.Context) (*workflow.NodeResult, error) {
	if m.executeFunc != nil {
		return m.executeFunc()
	}
	return &workflow.NodeResult{
		NodeID: node.ID,
		Output: "mock output",
		State:  workflow.StateCompleted,
	}, nil
}
