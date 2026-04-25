package workflow_test

import (
	"testing"

	"github.com/276793422/NemesisBot/module/workflow"
)

func TestTopologicalSort_SimpleDAG(t *testing.T) {
	nodes := []workflow.NodeDef{
		{ID: "a"},
		{ID: "b"},
		{ID: "c"},
	}
	edges := []workflow.Edge{
		{From: "a", To: "b"},
		{From: "b", To: "c"},
	}

	levels, err := workflow.TopologicalSort(nodes, edges)
	if err != nil {
		t.Fatalf("TopologicalSort() error: %v", err)
	}

	// Linear graph: a -> b -> c => [[a], [b], [c]]
	if len(levels) != 3 {
		t.Fatalf("expected 3 levels, got %d", len(levels))
	}
	assertLevel(t, levels, 0, []string{"a"})
	assertLevel(t, levels, 1, []string{"b"})
	assertLevel(t, levels, 2, []string{"c"})
}

func TestTopologicalSort_DiamondDAG(t *testing.T) {
	//     b
	//    / \
	//  a     d
	//    \ /
	//     c
	nodes := []workflow.NodeDef{
		{ID: "a"},
		{ID: "b"},
		{ID: "c"},
		{ID: "d"},
	}
	edges := []workflow.Edge{
		{From: "a", To: "b"},
		{From: "a", To: "c"},
		{From: "b", To: "d"},
		{From: "c", To: "d"},
	}

	levels, err := workflow.TopologicalSort(nodes, edges)
	if err != nil {
		t.Fatalf("TopologicalSort() error: %v", err)
	}

	if len(levels) != 3 {
		t.Fatalf("expected 3 levels, got %d", len(levels))
	}
	assertLevel(t, levels, 0, []string{"a"})
	assertLevelContains(t, levels, 1, []string{"b", "c"})
	assertLevel(t, levels, 2, []string{"d"})
}

func TestTopologicalSort_CyclicGraph(t *testing.T) {
	nodes := []workflow.NodeDef{
		{ID: "a"},
		{ID: "b"},
		{ID: "c"},
	}
	edges := []workflow.Edge{
		{From: "a", To: "b"},
		{From: "b", To: "c"},
		{From: "c", To: "a"},
	}

	_, err := workflow.TopologicalSort(nodes, edges)
	if err == nil {
		t.Fatal("expected error for cyclic graph, got nil")
	}
}

func TestTopologicalSort_EmptyGraph(t *testing.T) {
	levels, err := workflow.TopologicalSort(nil, nil)
	if err != nil {
		t.Fatalf("TopologicalSort() error: %v", err)
	}
	if len(levels) != 0 {
		t.Errorf("expected 0 levels for empty graph, got %d", len(levels))
	}
}

func TestTopologicalSort_SingleNode(t *testing.T) {
	nodes := []workflow.NodeDef{{ID: "only"}}
	levels, err := workflow.TopologicalSort(nodes, nil)
	if err != nil {
		t.Fatalf("TopologicalSort() error: %v", err)
	}
	if len(levels) != 1 {
		t.Fatalf("expected 1 level, got %d", len(levels))
	}
	assertLevel(t, levels, 0, []string{"only"})
}

func TestTopologicalSort_DisconnectedGraph(t *testing.T) {
	nodes := []workflow.NodeDef{
		{ID: "a"},
		{ID: "b"},
		{ID: "c"},
	}
	// Only a -> b, c is disconnected
	edges := []workflow.Edge{
		{From: "a", To: "b"},
	}

	levels, err := workflow.TopologicalSort(nodes, edges)
	if err != nil {
		t.Fatalf("TopologicalSort() error: %v", err)
	}

	// Level 0 should contain both "a" and "c" (no incoming edges)
	if len(levels) != 2 {
		t.Fatalf("expected 2 levels, got %d", len(levels))
	}
	assertLevelContains(t, levels, 0, []string{"a", "c"})
	assertLevel(t, levels, 1, []string{"b"})
}

func TestTopologicalSort_WideGraph(t *testing.T) {
	// All nodes independent: a, b, c, d, e
	nodes := []workflow.NodeDef{
		{ID: "a"},
		{ID: "b"},
		{ID: "c"},
		{ID: "d"},
		{ID: "e"},
	}

	levels, err := workflow.TopologicalSort(nodes, nil)
	if err != nil {
		t.Fatalf("TopologicalSort() error: %v", err)
	}

	if len(levels) != 1 {
		t.Fatalf("expected 1 level for all-independent nodes, got %d", len(levels))
	}
	assertLevelContains(t, levels, 0, []string{"a", "b", "c", "d", "e"})
}

func TestTopologicalSort_DependsOn(t *testing.T) {
	nodes := []workflow.NodeDef{
		{ID: "a"},
		{ID: "b", DependsOn: []string{"a"}},
		{ID: "c", DependsOn: []string{"b"}},
	}

	levels, err := workflow.TopologicalSort(nodes, nil)
	if err != nil {
		t.Fatalf("TopologicalSort() error: %v", err)
	}

	if len(levels) != 3 {
		t.Fatalf("expected 3 levels, got %d", len(levels))
	}
	assertLevel(t, levels, 0, []string{"a"})
	assertLevel(t, levels, 1, []string{"b"})
	assertLevel(t, levels, 2, []string{"c"})
}

func TestTopologicalSort_SelfLoop(t *testing.T) {
	nodes := []workflow.NodeDef{{ID: "a"}}
	edges := []workflow.Edge{{From: "a", To: "a"}}

	_, err := workflow.TopologicalSort(nodes, edges)
	if err == nil {
		t.Fatal("expected error for self-loop, got nil")
	}
}

func TestSchedule_SimpleLinear(t *testing.T) {
	ctx := newTestContext(t)
	defer ctx.cancel()

	nodes := []workflow.NodeDef{
		{ID: "n1", Type: "delay", Config: map[string]interface{}{"duration": "10ms"}},
		{ID: "n2", Type: "delay", Config: map[string]interface{}{"duration": "10ms"}},
	}
	edges := []workflow.Edge{
		{From: "n1", To: "n2"},
	}

	wfCtx := workflow.NewContext(nil)
	registry := workflow.DefaultExecutorRegistry()

	err := workflow.Schedule(ctx.ctx, nodes, edges, registry, wfCtx)
	if err != nil {
		t.Fatalf("Schedule() error: %v", err)
	}

	r1 := wfCtx.GetNodeResult("n1")
	r2 := wfCtx.GetNodeResult("n2")

	if r1 == nil || r1.State != workflow.StateCompleted {
		t.Errorf("n1 result: %+v", r1)
	}
	if r2 == nil || r2.State != workflow.StateCompleted {
		t.Errorf("n2 result: %+v", r2)
	}
}

func TestSchedule_ParallelExecution(t *testing.T) {
	ctx := newTestContext(t)
	defer ctx.cancel()

	nodes := []workflow.NodeDef{
		{ID: "n1", Type: "delay", Config: map[string]interface{}{"duration": "10ms"}},
		{ID: "n2", Type: "delay", Config: map[string]interface{}{"duration": "10ms"}},
		{ID: "n3", Type: "delay", Config: map[string]interface{}{"duration": "10ms"}},
	}
	// No edges: all should run in parallel
	edges := []workflow.Edge{}

	wfCtx := workflow.NewContext(nil)
	registry := workflow.DefaultExecutorRegistry()

	err := workflow.Schedule(ctx.ctx, nodes, edges, registry, wfCtx)
	if err != nil {
		t.Fatalf("Schedule() error: %v", err)
	}

	for _, id := range []string{"n1", "n2", "n3"} {
		r := wfCtx.GetNodeResult(id)
		if r == nil || r.State != workflow.StateCompleted {
			t.Errorf("node %s result: %+v", id, r)
		}
	}
}

func TestSchedule_UnknownNodeType(t *testing.T) {
	ctx := newTestContext(t)
	defer ctx.cancel()

	nodes := []workflow.NodeDef{
		{ID: "n1", Type: "nonexistent_type", Config: map[string]interface{}{}},
	}

	wfCtx := workflow.NewContext(nil)
	registry := workflow.DefaultExecutorRegistry()

	err := workflow.Schedule(ctx.ctx, nodes, nil, registry, wfCtx)
	if err == nil {
		t.Fatal("expected error for unknown node type, got nil")
	}
}

func TestSchedule_ContextCancellation(t *testing.T) {
	ctx := newTestContext(t)
	// Cancel immediately
	ctx.cancel()

	nodes := []workflow.NodeDef{
		{ID: "n1", Type: "delay", Config: map[string]interface{}{"duration": "10s"}},
	}

	wfCtx := workflow.NewContext(nil)
	registry := workflow.DefaultExecutorRegistry()

	err := workflow.Schedule(ctx.ctx, nodes, nil, registry, wfCtx)
	if err == nil {
		t.Fatal("expected error from cancelled context, got nil")
	}
}

func TestSchedule_NodeTimeout(t *testing.T) {
	ctx := newTestContext(t)
	defer ctx.cancel()

	nodes := []workflow.NodeDef{
		{ID: "n1", Type: "delay", Config: map[string]interface{}{"duration": "10s"}, Timeout: "50ms"},
	}

	wfCtx := workflow.NewContext(nil)
	registry := workflow.DefaultExecutorRegistry()

	err := workflow.Schedule(ctx.ctx, nodes, nil, registry, wfCtx)
	if err == nil {
		t.Fatal("expected error from node timeout, got nil")
	}
}

func TestSchedule_WithRetry(t *testing.T) {
	ctx := newTestContext(t)
	defer ctx.cancel()

	callCount := 0
	mockExec := &mockExecutor{
		executeFunc: func() (*workflow.NodeResult, error) {
			callCount++
			if callCount < 3 {
				return &workflow.NodeResult{
					NodeID: "n1",
					State:  workflow.StateFailed,
					Error:  "transient error",
				}, nil
			}
			return &workflow.NodeResult{
				NodeID: "n1",
				State:  workflow.StateCompleted,
				Output: "success",
			}, nil
		},
	}

	registry := workflow.DefaultExecutorRegistry()
	registry.Register("mock", mockExec)

	nodes := []workflow.NodeDef{
		{ID: "n1", Type: "mock", Config: map[string]interface{}{}, RetryCount: 3},
	}

	wfCtx := workflow.NewContext(nil)
	err := workflow.Schedule(ctx.ctx, nodes, nil, registry, wfCtx)
	if err != nil {
		t.Fatalf("Schedule() error: %v", err)
	}

	r := wfCtx.GetNodeResult("n1")
	if r == nil || r.State != workflow.StateCompleted {
		t.Errorf("expected completed, got: %+v", r)
	}
}

// --- helpers ---

func assertLevel(t *testing.T, levels [][]string, idx int, expected []string) {
	t.Helper()
	if idx >= len(levels) {
		t.Fatalf("level %d out of range (only %d levels)", idx, len(levels))
	}
	if len(levels[idx]) != len(expected) {
		t.Fatalf("level %d: got %v, want %v", idx, levels[idx], expected)
	}
	for i, id := range expected {
		if levels[idx][i] != id {
			t.Errorf("level %d[%d]: got %q, want %q", idx, i, levels[idx][i], id)
		}
	}
}

func assertLevelContains(t *testing.T, levels [][]string, idx int, expected []string) {
	t.Helper()
	if idx >= len(levels) {
		t.Fatalf("level %d out of range (only %d levels)", idx, len(levels))
	}
	set := make(map[string]bool, len(levels[idx]))
	for _, id := range levels[idx] {
		set[id] = true
	}
	for _, id := range expected {
		if !set[id] {
			t.Errorf("level %d: missing expected node %q, got %v", idx, id, levels[idx])
		}
	}
}
