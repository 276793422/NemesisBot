package workflow_test

import (
	"context"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/workflow"
)

// newTestEngine creates a new workflow engine without persistence.
func newTestEngine(t *testing.T) *workflow.Engine {
	t.Helper()
	return workflow.NewEngine("")
}

func TestEngine_RegisterAndList(t *testing.T) {
	engine := newTestEngine(t)

	wf1 := &workflow.Workflow{
		Name:        "test-wf-1",
		Description: "First test workflow",
		Nodes: []workflow.NodeDef{
			{ID: "n1", Type: "transform", Config: map[string]interface{}{"template": "hello"}},
		},
	}
	wf2 := &workflow.Workflow{
		Name:        "test-wf-2",
		Description: "Second test workflow",
		Nodes: []workflow.NodeDef{
			{ID: "n1", Type: "transform", Config: map[string]interface{}{"template": "world"}},
		},
	}

	if err := engine.Register(wf1); err != nil {
		t.Fatalf("Register wf1: %v", err)
	}
	if err := engine.Register(wf2); err != nil {
		t.Fatalf("Register wf2: %v", err)
	}

	list := engine.ListWorkflows()
	if len(list) != 2 {
		t.Errorf("expected 2 registered workflows, got %d", len(list))
	}

	// Verify GetWorkflow.
	got, ok := engine.GetWorkflow("test-wf-1")
	if !ok {
		t.Error("GetWorkflow test-wf-1: not found")
	}
	if got.Name != "test-wf-1" {
		t.Errorf("GetWorkflow name: got %q", got.Name)
	}
}

func TestEngine_SimplePipeline(t *testing.T) {
	engine := newTestEngine(t)

	wf := &workflow.Workflow{
		Name: "simple-pipeline",
		Nodes: []workflow.NodeDef{
			{ID: "step1", Type: "transform", Config: map[string]interface{}{"template": "hello world"}},
			{ID: "step2", Type: "delay", Config: map[string]interface{}{"duration": "1ms"}},
			{ID: "step3", Type: "transform", Config: map[string]interface{}{"template": "done"}},
		},
		Edges: []workflow.Edge{
			{From: "step1", To: "step2"},
			{From: "step2", To: "step3"},
		},
	}

	if err := engine.Register(wf); err != nil {
		t.Fatalf("Register: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	exec, err := engine.Run(ctx, "simple-pipeline", nil)
	if err != nil {
		t.Fatalf("Run: %v", err)
	}
	if exec.State != workflow.StateCompleted {
		t.Errorf("expected StateCompleted, got %s (error: %s)", exec.State, exec.Error)
	}
	if len(exec.NodeResults) != 3 {
		t.Errorf("expected 3 node results, got %d", len(exec.NodeResults))
	}
}

func TestEngine_ConditionalBranch(t *testing.T) {
	engine := newTestEngine(t)

	wf := &workflow.Workflow{
		Name: "conditional-test",
		Nodes: []workflow.NodeDef{
			{ID: "check", Type: "condition", Config: map[string]interface{}{"expression": "1 == 1"}},
			{ID: "true_branch", Type: "transform", Config: map[string]interface{}{"template": "yes"}},
			{ID: "false_branch", Type: "transform", Config: map[string]interface{}{"template": "no"}},
		},
		Edges: []workflow.Edge{
			{From: "check", To: "true_branch", Condition: "true"},
			{From: "check", To: "false_branch", Condition: "false"},
		},
	}

	if err := engine.Register(wf); err != nil {
		t.Fatalf("Register: %v", err)
	}

	ctx := context.Background()
	exec, err := engine.Run(ctx, "conditional-test", nil)
	if err != nil {
		t.Fatalf("Run: %v", err)
	}
	if exec.State != workflow.StateCompleted {
		t.Errorf("expected StateCompleted, got %s", exec.State)
	}
}

func TestEngine_ParallelExecution(t *testing.T) {
	engine := newTestEngine(t)

	wf := &workflow.Workflow{
		Name: "parallel-test",
		Nodes: []workflow.NodeDef{
			{ID: "parallel1", Type: "parallel", Config: map[string]interface{}{
				"nodes": []interface{}{
					map[string]interface{}{
						"id":   "p1",
						"type": "transform",
						"config": map[string]interface{}{
							"template": "parallel-A",
						},
					},
					map[string]interface{}{
						"id":   "p2",
						"type": "transform",
						"config": map[string]interface{}{
							"template": "parallel-B",
						},
					},
				},
			}},
		},
	}

	if err := engine.Register(wf); err != nil {
		t.Fatalf("Register: %v", err)
	}

	ctx := context.Background()
	exec, err := engine.Run(ctx, "parallel-test", nil)
	if err != nil {
		t.Fatalf("Run: %v", err)
	}
	if exec.State != workflow.StateCompleted {
		t.Errorf("expected StateCompleted, got %s (error: %s)", exec.State, exec.Error)
	}
}

func TestEngine_LoopExecution(t *testing.T) {
	engine := newTestEngine(t)

	wf := &workflow.Workflow{
		Name: "loop-test",
		Nodes: []workflow.NodeDef{
			{ID: "loop1", Type: "loop", Config: map[string]interface{}{
				"max_iterations": 3,
				"nodes": []interface{}{
					map[string]interface{}{
						"id":   "body",
						"type": "transform",
						"config": map[string]interface{}{
							"template": "iteration-{{loop_index}}",
						},
					},
				},
			}},
		},
	}

	if err := engine.Register(wf); err != nil {
		t.Fatalf("Register: %v", err)
	}

	ctx := context.Background()
	exec, err := engine.Run(ctx, "loop-test", nil)
	if err != nil {
		t.Fatalf("Run: %v", err)
	}
	if exec.State != workflow.StateCompleted {
		t.Errorf("expected StateCompleted, got %s", exec.State)
	}
}

func TestEngine_VariablePassing(t *testing.T) {
	engine := newTestEngine(t)

	wf := &workflow.Workflow{
		Name: "variable-test",
		Nodes: []workflow.NodeDef{
			{ID: "set_var", Type: "transform", Config: map[string]interface{}{
				"output": map[string]interface{}{
					"greeting": "Hello, {{input.name}}!",
				},
			}},
			{ID: "use_var", Type: "transform", Config: map[string]interface{}{
				"template": "Got: {{set_var.greeting}}",
			}},
		},
		Edges: []workflow.Edge{
			{From: "set_var", To: "use_var"},
		},
	}

	if err := engine.Register(wf); err != nil {
		t.Fatalf("Register: %v", err)
	}

	ctx := context.Background()
	exec, err := engine.Run(ctx, "variable-test", map[string]interface{}{
		"name": "World",
	})
	if err != nil {
		t.Fatalf("Run: %v", err)
	}
	if exec.State != workflow.StateCompleted {
		t.Errorf("expected StateCompleted, got %s", exec.State)
	}
}

func TestEngine_DAGExecution(t *testing.T) {
	engine := newTestEngine(t)

	// Diamond DAG: start -> left, right -> end
	wf := &workflow.Workflow{
		Name: "dag-test",
		Nodes: []workflow.NodeDef{
			{ID: "start", Type: "transform", Config: map[string]interface{}{"template": "begin"}},
			{ID: "left", Type: "transform", Config: map[string]interface{}{"template": "left"}},
			{ID: "right", Type: "transform", Config: map[string]interface{}{"template": "right"}},
			{ID: "end", Type: "transform", Config: map[string]interface{}{"template": "end"}},
		},
		Edges: []workflow.Edge{
			{From: "start", To: "left"},
			{From: "start", To: "right"},
			{From: "left", To: "end"},
			{From: "right", To: "end"},
		},
	}

	if err := engine.Register(wf); err != nil {
		t.Fatalf("Register: %v", err)
	}

	ctx := context.Background()
	exec, err := engine.Run(ctx, "dag-test", nil)
	if err != nil {
		t.Fatalf("Run: %v", err)
	}
	if exec.State != workflow.StateCompleted {
		t.Errorf("expected StateCompleted, got %s (error: %s)", exec.State, exec.Error)
	}
	if len(exec.NodeResults) != 4 {
		t.Errorf("expected 4 node results, got %d", len(exec.NodeResults))
	}
}

func TestEngine_InvalidWorkflow(t *testing.T) {
	engine := newTestEngine(t)

	// Cycle: A -> B -> A.
	cyclicWF := &workflow.Workflow{
		Name: "cyclic",
		Nodes: []workflow.NodeDef{
			{ID: "a", Type: "transform", Config: map[string]interface{}{"template": "a"}},
			{ID: "b", Type: "transform", Config: map[string]interface{}{"template": "b"}},
		},
		Edges: []workflow.Edge{
			{From: "a", To: "b"},
			{From: "b", To: "a"},
		},
	}
	if err := engine.Register(cyclicWF); err == nil {
		t.Error("expected error when registering cyclic workflow")
	}

	// Missing node reference.
	missingNodeWF := &workflow.Workflow{
		Name: "missing-node",
		Nodes: []workflow.NodeDef{
			{ID: "a", Type: "transform", Config: map[string]interface{}{"template": "a"}},
		},
		Edges: []workflow.Edge{
			{From: "a", To: "nonexistent"},
		},
	}
	if err := engine.Register(missingNodeWF); err == nil {
		t.Error("expected error when registering workflow with missing node reference")
	}

	// No name.
	noNameWF := &workflow.Workflow{
		Nodes: []workflow.NodeDef{
			{ID: "a", Type: "transform", Config: map[string]interface{}{"template": "a"}},
		},
	}
	if err := engine.Register(noNameWF); err == nil {
		t.Error("expected error for workflow without name")
	}

	// No nodes.
	noNodesWF := &workflow.Workflow{
		Name: "no-nodes",
	}
	if err := engine.Register(noNodesWF); err == nil {
		t.Error("expected error for workflow without nodes")
	}
}

func TestEngine_CancelExecution(t *testing.T) {
	engine := newTestEngine(t)

	wf := &workflow.Workflow{
		Name: "cancel-test",
		Nodes: []workflow.NodeDef{
			{ID: "long_delay", Type: "delay", Config: map[string]interface{}{"duration": "10s"}},
		},
	}

	if err := engine.Register(wf); err != nil {
		t.Fatalf("Register: %v", err)
	}

	ctx, cancel := context.WithCancel(context.Background())

	// Run in a goroutine and cancel shortly after.
	done := make(chan struct{})
	go func() {
		defer close(done)
		exec, err := engine.Run(ctx, "cancel-test", nil)
		if err == nil {
			if exec.State != workflow.StateCancelled {
				t.Errorf("expected StateCancelled, got %s", exec.State)
			}
		}
	}()

	// Give the workflow a moment to start, then cancel.
	time.Sleep(50 * time.Millisecond)
	cancel()
	<-done
}

func TestEngine_RunNotFound(t *testing.T) {
	engine := newTestEngine(t)
	ctx := context.Background()

	_, err := engine.Run(ctx, "nonexistent", nil)
	if err == nil {
		t.Error("expected error for non-existent workflow")
	}
}

func TestEngine_Unregister(t *testing.T) {
	engine := newTestEngine(t)

	wf := &workflow.Workflow{
		Name: "to-remove",
		Nodes: []workflow.NodeDef{
			{ID: "n1", Type: "transform", Config: map[string]interface{}{"template": "x"}},
		},
	}
	if err := engine.Register(wf); err != nil {
		t.Fatalf("Register: %v", err)
	}

	engine.Unregister("to-remove")

	if list := engine.ListWorkflows(); len(list) != 0 {
		t.Errorf("expected 0 workflows after unregister, got %d", len(list))
	}
}

func TestEngine_ExecutionPersistence(t *testing.T) {
	tmp := t.TempDir()
	engine := workflow.NewEngine(tmp)
	defer engine.Close()

	wf := &workflow.Workflow{
		Name: "persist-test",
		Nodes: []workflow.NodeDef{
			{ID: "n1", Type: "transform", Config: map[string]interface{}{"template": "persist me"}},
		},
	}
	if err := engine.Register(wf); err != nil {
		t.Fatalf("Register: %v", err)
	}

	ctx := context.Background()
	exec, err := engine.Run(ctx, "persist-test", nil)
	if err != nil {
		t.Fatalf("Run: %v", err)
	}

	// Load the execution from persistence.
	loaded, err := engine.GetExecution(exec.ID)
	if err != nil {
		t.Fatalf("GetExecution: %v", err)
	}
	if loaded.WorkflowName != "persist-test" {
		t.Errorf("WorkflowName: got %q", loaded.WorkflowName)
	}
}

func TestEngine_ListExecutions(t *testing.T) {
	engine := newTestEngine(t)

	wf := &workflow.Workflow{
		Name: "list-exec-test",
		Nodes: []workflow.NodeDef{
			{ID: "n1", Type: "transform", Config: map[string]interface{}{"template": "x"}},
		},
	}
	_ = engine.Register(wf)

	ctx := context.Background()
	_, _ = engine.Run(ctx, "list-exec-test", nil)
	_, _ = engine.Run(ctx, "list-exec-test", nil)

	all := engine.ListExecutions("")
	if len(all) != 2 {
		t.Errorf("expected 2 executions, got %d", len(all))
	}

	filtered := engine.ListExecutions("list-exec-test")
	if len(filtered) != 2 {
		t.Errorf("expected 2 filtered executions, got %d", len(filtered))
	}

	other := engine.ListExecutions("nonexistent")
	if len(other) != 0 {
		t.Errorf("expected 0 for non-existent workflow, got %d", len(other))
	}
}
