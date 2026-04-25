package workflow_test

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/276793422/NemesisBot/module/workflow"
)

const validWorkflowYAML = `
name: test-workflow
description: A test workflow
version: "1.0"
nodes:
  - id: start
    type: llm
    config:
      prompt: "Hello {{name}}"
  - id: end
    type: tool
    config:
      tool: search
    depends_on:
      - start
edges:
  - from: start
    to: end
variables:
  name: world
metadata:
  author: tester
`

const cyclicWorkflowYAML = `
name: cyclic-workflow
nodes:
  - id: a
    type: llm
    config: {}
  - id: b
    type: llm
    config: {}
  - id: c
    type: llm
    config: {}
edges:
  - from: a
    to: b
  - from: b
    to: c
  - from: c
    to: a
`

const noNameWorkflowYAML = `
description: missing name
nodes:
  - id: n1
    type: llm
    config: {}
`

const emptyNodesYAML = `
name: empty-nodes
nodes: []
`

const duplicateNodesYAML = `
name: duplicate-nodes
nodes:
  - id: dup
    type: llm
    config: {}
  - id: dup
    type: tool
    config: {}
`

const missingNodeIDYAML = `
name: missing-id
nodes:
  - type: llm
    config: {}
`

const invalidEdgeYAML = `
name: bad-edge
nodes:
  - id: n1
    type: llm
    config: {}
edges:
  - from: n1
    to: nonexistent
`

const invalidDependsOnYAML = `
name: bad-depends
nodes:
  - id: n1
    type: llm
    config: {}
  - id: n2
    type: llm
    config: {}
    depends_on:
      - nonexistent
`

const withTriggersYAML = `
name: triggered-workflow
nodes:
  - id: n1
    type: llm
    config:
      prompt: "test"
triggers:
  - type: cron
    config:
      expression: "*/5 * * * *"
  - type: webhook
    config:
      path: /hook
  - type: event
    config:
      event_type: deploy
  - type: message
    config:
      pattern: "hello*"
`

const invalidTriggerTypeYAML = `
name: bad-trigger
nodes:
  - id: n1
    type: llm
    config: {}
triggers:
  - type: invalid_type
    config: {}
`

func TestParseYAML_Valid(t *testing.T) {
	wf, err := workflow.ParseYAML([]byte(validWorkflowYAML))
	if err != nil {
		t.Fatalf("ParseYAML() error: %v", err)
	}
	if wf.Name != "test-workflow" {
		t.Errorf("Name = %q, want %q", wf.Name, "test-workflow")
	}
	if wf.Description != "A test workflow" {
		t.Errorf("Description = %q, want %q", wf.Description, "A test workflow")
	}
	if wf.Version != "1.0" {
		t.Errorf("Version = %q, want %q", wf.Version, "1.0")
	}
	if len(wf.Nodes) != 2 {
		t.Errorf("len(Nodes) = %d, want 2", len(wf.Nodes))
	}
	if len(wf.Edges) != 1 {
		t.Errorf("len(Edges) = %d, want 1", len(wf.Edges))
	}
	if wf.Variables["name"] != "world" {
		t.Errorf("Variables[name] = %q, want %q", wf.Variables["name"], "world")
	}
	if wf.Metadata["author"] != "tester" {
		t.Errorf("Metadata[author] = %q, want %q", wf.Metadata["author"], "tester")
	}
}

func TestParseYAML_InvalidYAML(t *testing.T) {
	_, err := workflow.ParseYAML([]byte("::invalid::yaml::["))
	if err == nil {
		t.Fatal("expected error for invalid YAML")
	}
}

func TestParseYAML_NodeTypes(t *testing.T) {
	yaml := `
name: node-types
nodes:
  - id: n1
    type: llm
    config:
      prompt: "test"
  - id: n2
    type: tool
    config:
      tool: search
      args:
        q: "query"
  - id: n3
    type: condition
    config:
      expression: "5 > 3"
  - id: n4
    type: transform
    config:
      template: "{{n1}}"
  - id: n5
    type: delay
    config:
      duration: "5s"
  - id: n6
    type: script
    config:
      language: bash
      script: "echo hello"
  - id: n7
    type: human_review
    config:
      message: "Review this"
`
	wf, err := workflow.ParseYAML([]byte(yaml))
	if err != nil {
		t.Fatalf("ParseYAML() error: %v", err)
	}

	expectedTypes := map[string]string{
		"n1": "llm",
		"n2": "tool",
		"n3": "condition",
		"n4": "transform",
		"n5": "delay",
		"n6": "script",
		"n7": "human_review",
	}

	for _, node := range wf.Nodes {
		expected, ok := expectedTypes[node.ID]
		if !ok {
			t.Errorf("unexpected node ID %q", node.ID)
			continue
		}
		if node.Type != expected {
			t.Errorf("node %q type = %q, want %q", node.ID, node.Type, expected)
		}
	}
}

func TestParseFile(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "workflow.yaml")
	if err := os.WriteFile(path, []byte(validWorkflowYAML), 0644); err != nil {
		t.Fatalf("WriteFile() error: %v", err)
	}

	wf, err := workflow.ParseFile(path)
	if err != nil {
		t.Fatalf("ParseFile() error: %v", err)
	}
	if wf.Name != "test-workflow" {
		t.Errorf("Name = %q, want %q", wf.Name, "test-workflow")
	}
}

func TestParseFile_NotFound(t *testing.T) {
	_, err := workflow.ParseFile("/nonexistent/path/workflow.yaml")
	if err == nil {
		t.Fatal("expected error for missing file")
	}
}

func TestValidate_ValidWorkflow(t *testing.T) {
	wf := &workflow.Workflow{
		Name: "valid",
		Nodes: []workflow.NodeDef{
			{ID: "a", Type: "llm", Config: map[string]interface{}{"prompt": "test"}},
			{ID: "b", Type: "llm", Config: map[string]interface{}{"prompt": "test"}},
		},
		Edges: []workflow.Edge{
			{From: "a", To: "b"},
		},
	}

	if err := workflow.Validate(wf); err != nil {
		t.Errorf("Validate() error: %v", err)
	}
}

func TestValidate_NoName(t *testing.T) {
	wf := &workflow.Workflow{
		Nodes: []workflow.NodeDef{{ID: "a", Type: "llm"}},
	}
	err := workflow.Validate(wf)
	if err == nil {
		t.Fatal("expected error for no name")
	}
}

func TestValidate_NoNodes(t *testing.T) {
	wf := &workflow.Workflow{
		Name:  "no-nodes",
		Nodes: []workflow.NodeDef{},
	}
	err := workflow.Validate(wf)
	if err == nil {
		t.Fatal("expected error for empty nodes")
	}
}

func TestValidate_DuplicateNodeIDs(t *testing.T) {
	wf := &workflow.Workflow{
		Name: "dup-ids",
		Nodes: []workflow.NodeDef{
			{ID: "dup", Type: "llm"},
			{ID: "dup", Type: "tool"},
		},
	}
	err := workflow.Validate(wf)
	if err == nil {
		t.Fatal("expected error for duplicate node IDs")
	}
}

func TestValidate_MissingNodeID(t *testing.T) {
	wf := &workflow.Workflow{
		Name:  "missing-id",
		Nodes: []workflow.NodeDef{{Type: "llm"}},
	}
	err := workflow.Validate(wf)
	if err == nil {
		t.Fatal("expected error for missing node ID")
	}
}

func TestValidate_InvalidEdgeFrom(t *testing.T) {
	wf := &workflow.Workflow{
		Name: "bad-edge-from",
		Nodes: []workflow.NodeDef{
			{ID: "a", Type: "llm"},
		},
		Edges: []workflow.Edge{
			{From: "nonexistent", To: "a"},
		},
	}
	err := workflow.Validate(wf)
	if err == nil {
		t.Fatal("expected error for invalid edge 'from'")
	}
}

func TestValidate_InvalidEdgeTo(t *testing.T) {
	wf := &workflow.Workflow{
		Name: "bad-edge-to",
		Nodes: []workflow.NodeDef{
			{ID: "a", Type: "llm"},
		},
		Edges: []workflow.Edge{
			{From: "a", To: "nonexistent"},
		},
	}
	err := workflow.Validate(wf)
	if err == nil {
		t.Fatal("expected error for invalid edge 'to'")
	}
}

func TestValidate_Cyclic(t *testing.T) {
	wf, err := workflow.ParseYAML([]byte(cyclicWorkflowYAML))
	if err != nil {
		t.Fatalf("ParseYAML() error: %v", err)
	}
	err = workflow.Validate(wf)
	if err == nil {
		t.Fatal("expected error for cyclic graph")
	}
}

func TestValidate_InvalidDependsOn(t *testing.T) {
	wf := &workflow.Workflow{
		Name: "bad-dep",
		Nodes: []workflow.NodeDef{
			{ID: "a", Type: "llm"},
			{ID: "b", Type: "llm", DependsOn: []string{"nonexistent"}},
		},
	}
	err := workflow.Validate(wf)
	if err == nil {
		t.Fatal("expected error for invalid depends_on")
	}
}

func TestValidate_WithTriggers(t *testing.T) {
	wf, err := workflow.ParseYAML([]byte(withTriggersYAML))
	if err != nil {
		t.Fatalf("ParseYAML() error: %v", err)
	}
	if err := workflow.Validate(wf); err != nil {
		t.Errorf("Validate() error: %v", err)
	}
	if len(wf.Triggers) != 4 {
		t.Errorf("len(Triggers) = %d, want 4", len(wf.Triggers))
	}
}

func TestValidate_InvalidTriggerType(t *testing.T) {
	wf, err := workflow.ParseYAML([]byte(invalidTriggerTypeYAML))
	if err != nil {
		t.Fatalf("ParseYAML() error: %v", err)
	}
	err = workflow.Validate(wf)
	if err == nil {
		t.Fatal("expected error for invalid trigger type")
	}
}

func TestValidate_SingleNodeNoEdges(t *testing.T) {
	wf := &workflow.Workflow{
		Name: "single",
		Nodes: []workflow.NodeDef{
			{ID: "only", Type: "llm"},
		},
	}
	if err := workflow.Validate(wf); err != nil {
		t.Errorf("Validate() error: %v", err)
	}
}

func TestValidate_DisconnectedGraph(t *testing.T) {
	wf := &workflow.Workflow{
		Name: "disconnected",
		Nodes: []workflow.NodeDef{
			{ID: "a", Type: "llm"},
			{ID: "b", Type: "llm"},
			{ID: "c", Type: "llm"},
		},
		Edges: []workflow.Edge{
			{From: "a", To: "b"},
		},
	}
	if err := workflow.Validate(wf); err != nil {
		t.Errorf("Validate() error: %v", err)
	}
}

func TestParseYAML_ComplexWorkflow(t *testing.T) {
	yaml := `
name: complex
description: Complex workflow
version: "2.0"
nodes:
  - id: step1
    type: llm
    config:
      prompt: "Analyze {{input}}"
    retry_count: 3
    timeout: 60s
  - id: step2
    type: condition
    config:
      expression: '{{step1.result}} == yes'
    depends_on:
      - step1
  - id: step3a
    type: transform
    config:
      output:
        data: "{{step1.output}}"
    depends_on:
      - step2
  - id: step3b
    type: tool
    config:
      tool: search
    depends_on:
      - step2
  - id: step4
    type: delay
    config:
      duration: 5s
edges:
  - from: step1
    to: step2
  - from: step2
    to: step3a
    condition: "{{step2.result}} == true"
  - from: step2
    to: step3b
    condition: "{{step2.result}} == false"
  - from: step3a
    to: step4
variables:
  input: "test data"
  mode: "auto"
`
	wf, err := workflow.ParseYAML([]byte(yaml))
	if err != nil {
		t.Fatalf("ParseYAML() error: %v", err)
	}

	if wf.Name != "complex" {
		t.Errorf("Name = %q, want %q", wf.Name, "complex")
	}
	if len(wf.Nodes) != 5 {
		t.Errorf("len(Nodes) = %d, want 5", len(wf.Nodes))
	}
	if len(wf.Edges) != 4 {
		t.Errorf("len(Edges) = %d, want 4", len(wf.Edges))
	}

	// Verify node with retry and timeout
	step1 := wf.Nodes[0]
	if step1.RetryCount != 3 {
		t.Errorf("step1.RetryCount = %d, want 3", step1.RetryCount)
	}
	if step1.Timeout != "60s" {
		t.Errorf("step1.Timeout = %q, want %q", step1.Timeout, "60s")
	}

	// Validate the workflow
	if err := workflow.Validate(wf); err != nil {
		t.Errorf("Validate() error: %v", err)
	}
}
