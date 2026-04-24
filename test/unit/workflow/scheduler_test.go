package workflow_test

import (
	"testing"

	"github.com/276793422/NemesisBot/module/workflow"
)

func TestTopologicalSort_Linear(t *testing.T) {
	nodes := []workflow.NodeDef{
		{ID: "a", Type: "transform"},
		{ID: "b", Type: "transform"},
		{ID: "c", Type: "transform"},
	}
	edges := []workflow.Edge{
		{From: "a", To: "b"},
		{From: "b", To: "c"},
	}

	levels, err := workflow.TopologicalSort(nodes, edges)
	if err != nil {
		t.Fatalf("TopologicalSort: %v", err)
	}
	if len(levels) != 3 {
		t.Fatalf("expected 3 levels for linear chain, got %d", len(levels))
	}
	if levels[0][0] != "a" {
		t.Errorf("level 0: expected [a], got %v", levels[0])
	}
	if levels[1][0] != "b" {
		t.Errorf("level 1: expected [b], got %v", levels[1])
	}
	if levels[2][0] != "c" {
		t.Errorf("level 2: expected [c], got %v", levels[2])
	}
}

func TestTopologicalSort_Diamond(t *testing.T) {
	// Diamond: A -> B, A -> C, B -> D, C -> D
	nodes := []workflow.NodeDef{
		{ID: "a", Type: "transform"},
		{ID: "b", Type: "transform"},
		{ID: "c", Type: "transform"},
		{ID: "d", Type: "transform"},
	}
	edges := []workflow.Edge{
		{From: "a", To: "b"},
		{From: "a", To: "c"},
		{From: "b", To: "d"},
		{From: "c", To: "d"},
	}

	levels, err := workflow.TopologicalSort(nodes, edges)
	if err != nil {
		t.Fatalf("TopologicalSort: %v", err)
	}

	// Expected: [a], [b, c], [d]
	if len(levels) != 3 {
		t.Fatalf("expected 3 levels for diamond, got %d", len(levels))
	}
	if len(levels[0]) != 1 || levels[0][0] != "a" {
		t.Errorf("level 0: expected [a], got %v", levels[0])
	}
	if len(levels[2]) != 1 || levels[2][0] != "d" {
		t.Errorf("level 2: expected [d], got %v", levels[2])
	}
	// Level 1 should contain both b and c.
	level1 := make(map[string]bool)
	for _, id := range levels[1] {
		level1[id] = true
	}
	if !level1["b"] || !level1["c"] {
		t.Errorf("level 1: expected [b,c], got %v", levels[1])
	}
}

func TestTopologicalSort_Parallel(t *testing.T) {
	// Independent nodes with no edges -- all in level 0.
	nodes := []workflow.NodeDef{
		{ID: "a", Type: "transform"},
		{ID: "b", Type: "transform"},
		{ID: "c", Type: "transform"},
	}

	levels, err := workflow.TopologicalSort(nodes, nil)
	if err != nil {
		t.Fatalf("TopologicalSort: %v", err)
	}
	if len(levels) != 1 {
		t.Fatalf("expected 1 level for independent nodes, got %d", len(levels))
	}
	if len(levels[0]) != 3 {
		t.Errorf("expected 3 nodes in single level, got %d", len(levels[0]))
	}
}

func TestTopologicalSort_CycleDetection(t *testing.T) {
	nodes := []workflow.NodeDef{
		{ID: "a", Type: "transform"},
		{ID: "b", Type: "transform"},
		{ID: "c", Type: "transform"},
	}
	edges := []workflow.Edge{
		{From: "a", To: "b"},
		{From: "b", To: "c"},
		{From: "c", To: "a"},
	}

	_, err := workflow.TopologicalSort(nodes, edges)
	if err == nil {
		t.Fatal("expected error for cyclic graph")
	}
}

func TestTopologicalSort_Complex(t *testing.T) {
	// Complex DAG:
	//   n1 -> n2, n3
	//   n2 -> n4
	//   n3 -> n4, n5
	//   n4 -> n6
	//   n5 -> n6
	nodes := []workflow.NodeDef{
		{ID: "n1", Type: "transform"},
		{ID: "n2", Type: "transform"},
		{ID: "n3", Type: "transform"},
		{ID: "n4", Type: "transform"},
		{ID: "n5", Type: "transform"},
		{ID: "n6", Type: "transform"},
	}
	edges := []workflow.Edge{
		{From: "n1", To: "n2"},
		{From: "n1", To: "n3"},
		{From: "n2", To: "n4"},
		{From: "n3", To: "n4"},
		{From: "n3", To: "n5"},
		{From: "n4", To: "n6"},
		{From: "n5", To: "n6"},
	}

	levels, err := workflow.TopologicalSort(nodes, edges)
	if err != nil {
		t.Fatalf("TopologicalSort: %v", err)
	}

	// Expected levels:
	// Level 0: [n1]
	// Level 1: [n2, n3]
	// Level 2: [n4, n5]
	// Level 3: [n6]
	if len(levels) != 4 {
		t.Fatalf("expected 4 levels, got %d", len(levels))
	}

	// Verify n1 is first, n6 is last.
	if levels[0][0] != "n1" {
		t.Errorf("level 0: expected n1, got %v", levels[0])
	}
	lastLevel := levels[len(levels)-1]
	if len(lastLevel) != 1 || lastLevel[0] != "n6" {
		t.Errorf("last level: expected [n6], got %v", lastLevel)
	}
}

func TestTopologicalSort_DependsOn(t *testing.T) {
	// Use DependsOn instead of Edges.
	nodes := []workflow.NodeDef{
		{ID: "a", Type: "transform"},
		{ID: "b", Type: "transform", DependsOn: []string{"a"}},
		{ID: "c", Type: "transform", DependsOn: []string{"a"}},
		{ID: "d", Type: "transform", DependsOn: []string{"b", "c"}},
	}

	levels, err := workflow.TopologicalSort(nodes, nil)
	if err != nil {
		t.Fatalf("TopologicalSort with DependsOn: %v", err)
	}

	if len(levels) != 3 {
		t.Fatalf("expected 3 levels, got %d", len(levels))
	}
	if levels[0][0] != "a" {
		t.Errorf("level 0: expected [a], got %v", levels[0])
	}
}
