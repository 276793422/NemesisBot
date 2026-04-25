package workflow_test

import (
	"encoding/json"
	"testing"

	"github.com/276793422/NemesisBot/module/workflow"
)

func TestExecutionStateString(t *testing.T) {
	tests := []struct {
		state    workflow.ExecutionState
		expected string
	}{
		{workflow.StatePending, "pending"},
		{workflow.StateRunning, "running"},
		{workflow.StateCompleted, "completed"},
		{workflow.StateFailed, "failed"},
		{workflow.StateCancelled, "cancelled"},
		{workflow.StateWaiting, "waiting"},
		{workflow.ExecutionState(99), "unknown"},
	}

	for _, tt := range tests {
		t.Run(tt.expected, func(t *testing.T) {
			got := tt.state.String()
			if got != tt.expected {
				t.Errorf("ExecutionState(%d).String() = %q, want %q", tt.state, got, tt.expected)
			}
		})
	}
}

func TestExecutionStateMarshalJSON(t *testing.T) {
	tests := []struct {
		name     string
		state    workflow.ExecutionState
		expected string
	}{
		{"pending", workflow.StatePending, `"pending"`},
		{"running", workflow.StateRunning, `"running"`},
		{"completed", workflow.StateCompleted, `"completed"`},
		{"failed", workflow.StateFailed, `"failed"`},
		{"cancelled", workflow.StateCancelled, `"cancelled"`},
		{"waiting", workflow.StateWaiting, `"waiting"`},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			data, err := json.Marshal(tt.state)
			if err != nil {
				t.Fatalf("MarshalJSON() error: %v", err)
			}
			if string(data) != tt.expected {
				t.Errorf("MarshalJSON() = %s, want %s", data, tt.expected)
			}
		})
	}
}

func TestExecutionStateUnmarshalJSON(t *testing.T) {
	tests := []struct {
		name     string
		jsonStr  string
		expected workflow.ExecutionState
	}{
		{"pending", `"pending"`, workflow.StatePending},
		{"running", `"running"`, workflow.StateRunning},
		{"completed", `"completed"`, workflow.StateCompleted},
		{"failed", `"failed"`, workflow.StateFailed},
		{"cancelled", `"cancelled"`, workflow.StateCancelled},
		{"waiting", `"waiting"`, workflow.StateWaiting},
		{"unknown_defaults_to_pending", `"unknown_state"`, workflow.StatePending},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			var state workflow.ExecutionState
			err := json.Unmarshal([]byte(tt.jsonStr), &state)
			if err != nil {
				t.Fatalf("UnmarshalJSON() error: %v", err)
			}
			if state != tt.expected {
				t.Errorf("UnmarshalJSON(%s) = %d, want %d", tt.jsonStr, state, tt.expected)
			}
		})
	}
}

func TestExecutionStateJSONRoundTrip(t *testing.T) {
	states := []workflow.ExecutionState{
		workflow.StatePending,
		workflow.StateRunning,
		workflow.StateCompleted,
		workflow.StateFailed,
		workflow.StateCancelled,
		workflow.StateWaiting,
	}

	for _, original := range states {
		t.Run(original.String(), func(t *testing.T) {
			data, err := json.Marshal(original)
			if err != nil {
				t.Fatalf("Marshal error: %v", err)
			}

			var decoded workflow.ExecutionState
			if err := json.Unmarshal(data, &decoded); err != nil {
				t.Fatalf("Unmarshal error: %v", err)
			}

			if decoded != original {
				t.Errorf("round-trip: got %d, want %d", decoded, original)
			}
		})
	}
}

func TestWorkflowStructCreation(t *testing.T) {
	wf := &workflow.Workflow{
		Name:        "test-workflow",
		Description: "A test workflow",
		Version:     "1.0.0",
		Nodes: []workflow.NodeDef{
			{ID: "node1", Type: "llm", Config: map[string]interface{}{"prompt": "hello"}},
			{ID: "node2", Type: "tool", Config: map[string]interface{}{"tool": "search"}},
		},
		Edges: []workflow.Edge{
			{From: "node1", To: "node2"},
		},
		Variables: map[string]string{"key": "value"},
		Metadata:  map[string]string{"author": "test"},
	}

	if wf.Name != "test-workflow" {
		t.Errorf("Name = %q, want %q", wf.Name, "test-workflow")
	}
	if len(wf.Nodes) != 2 {
		t.Errorf("len(Nodes) = %d, want 2", len(wf.Nodes))
	}
	if len(wf.Edges) != 1 {
		t.Errorf("len(Edges) = %d, want 1", len(wf.Edges))
	}
	if wf.Variables["key"] != "value" {
		t.Errorf("Variables[key] = %q, want %q", wf.Variables["key"], "value")
	}
	if wf.Metadata["author"] != "test" {
		t.Errorf("Metadata[author] = %q, want %q", wf.Metadata["author"], "test")
	}
}

func TestNodeDefStruct(t *testing.T) {
	node := workflow.NodeDef{
		ID:         "n1",
		Type:       "llm",
		Config:     map[string]interface{}{"prompt": "test"},
		DependsOn:  []string{"n0"},
		RetryCount: 3,
		Timeout:    "30s",
	}

	if node.ID != "n1" {
		t.Errorf("ID = %q, want %q", node.ID, "n1")
	}
	if node.Type != "llm" {
		t.Errorf("Type = %q, want %q", node.Type, "llm")
	}
	if len(node.DependsOn) != 1 || node.DependsOn[0] != "n0" {
		t.Errorf("DependsOn = %v, want [n0]", node.DependsOn)
	}
	if node.RetryCount != 3 {
		t.Errorf("RetryCount = %d, want 3", node.RetryCount)
	}
	if node.Timeout != "30s" {
		t.Errorf("Timeout = %q, want %q", node.Timeout, "30s")
	}
}

func TestEdgeStruct(t *testing.T) {
	edge := workflow.Edge{
		From:      "a",
		To:        "b",
		Condition: "x == y",
	}

	if edge.From != "a" || edge.To != "b" || edge.Condition != "x == y" {
		t.Errorf("Edge = %+v, unexpected values", edge)
	}
}

func TestTriggerConfigStruct(t *testing.T) {
	trigger := workflow.TriggerConfig{
		Type:   "cron",
		Config: map[string]interface{}{"expression": "*/5 * * * *"},
	}

	if trigger.Type != "cron" {
		t.Errorf("Type = %q, want %q", trigger.Type, "cron")
	}
}

func TestNodeResultStruct(t *testing.T) {
	now := getTimeNow()
	result := &workflow.NodeResult{
		NodeID: "node1",
		Output: "result data",
		State:  workflow.StateCompleted,
		Metadata: map[string]interface{}{
			"key": "value",
		},
		StartedAt: now,
		EndedAt:   now,
	}

	if result.NodeID != "node1" {
		t.Errorf("NodeID = %q, want %q", result.NodeID, "node1")
	}
	if result.State != workflow.StateCompleted {
		t.Errorf("State = %d, want %d", result.State, workflow.StateCompleted)
	}
	if result.Error != "" {
		t.Errorf("Error = %q, want empty", result.Error)
	}
}

func TestExecutionStruct(t *testing.T) {
	exec := &workflow.Execution{
		ID:           "exec-1",
		WorkflowName: "test-workflow",
		State:        workflow.StateRunning,
		Input:        map[string]interface{}{"query": "hello"},
		NodeResults:  make(map[string]*workflow.NodeResult),
		Variables:    map[string]string{"var1": "val1"},
	}

	if exec.ID != "exec-1" {
		t.Errorf("ID = %q, want %q", exec.ID, "exec-1")
	}
	if exec.WorkflowName != "test-workflow" {
		t.Errorf("WorkflowName = %q, want %q", exec.WorkflowName, "test-workflow")
	}
	if exec.State != workflow.StateRunning {
		t.Errorf("State = %d, want %d", exec.State, workflow.StateRunning)
	}
	if exec.Input["query"] != "hello" {
		t.Errorf("Input[query] = %v, want hello", exec.Input["query"])
	}
}

func TestExecutionStateConstants(t *testing.T) {
	states := []workflow.ExecutionState{
		workflow.StatePending,
		workflow.StateRunning,
		workflow.StateCompleted,
		workflow.StateFailed,
		workflow.StateCancelled,
		workflow.StateWaiting,
	}

	// Verify all states are distinct
	seen := make(map[workflow.ExecutionState]bool)
	for _, s := range states {
		if seen[s] {
			t.Errorf("duplicate state value: %d", s)
		}
		seen[s] = true
	}

	// Verify they are iota-ordered starting from 0
	for i, s := range states {
		if int(s) != i {
			t.Errorf("states[%d] = %d, want %d", i, s, i)
		}
	}
}
