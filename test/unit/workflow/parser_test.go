package workflow_test

import (
	"os"
	"testing"

	"github.com/276793422/NemesisBot/module/workflow"
)

func TestParseYAML_ValidWorkflow(t *testing.T) {
	yamlData := []byte(`
name: test-workflow
description: A test workflow
version: "1.0"
nodes:
  - id: step1
    type: transform
    config:
      template: "Hello {{input.name}}"
  - id: step2
    type: delay
    config:
      duration: "1ms"
edges:
  - from: step1
    to: step2
variables:
  greeting: hello
`)

	wf, err := workflow.ParseYAML(yamlData)
	if err != nil {
		t.Fatalf("ParseYAML: %v", err)
	}
	if wf.Name != "test-workflow" {
		t.Errorf("Name: got %q", wf.Name)
	}
	if len(wf.Nodes) != 2 {
		t.Errorf("Nodes: expected 2, got %d", len(wf.Nodes))
	}
	if wf.Nodes[0].ID != "step1" {
		t.Errorf("Node[0].ID: got %q", wf.Nodes[0].ID)
	}
	if wf.Nodes[0].Type != "transform" {
		t.Errorf("Node[0].Type: got %q", wf.Nodes[0].Type)
	}
	if len(wf.Edges) != 1 {
		t.Errorf("Edges: expected 1, got %d", len(wf.Edges))
	}
	if wf.Edges[0].From != "step1" {
		t.Errorf("Edge[0].From: got %q", wf.Edges[0].From)
	}
	if wf.Variables["greeting"] != "hello" {
		t.Errorf("Variables[greeting]: got %q", wf.Variables["greeting"])
	}
}

func TestParseYAML_InvalidYAML(t *testing.T) {
	yamlData := []byte(`
name: broken
nodes:
  - id: step1
    type: transform
  invalid yaml content here: [unclosed
`)

	_, err := workflow.ParseYAML(yamlData)
	if err == nil {
		t.Error("expected error for malformed YAML")
	}
}

func TestParseYAML_WithTriggers(t *testing.T) {
	yamlData := []byte(`
name: triggered-workflow
description: Workflow with triggers
nodes:
  - id: n1
    type: transform
    config:
      template: "triggered"
triggers:
  - type: cron
    config:
      expression: "*/5 * * * *"
  - type: webhook
    config:
      path: "/api/trigger"
  - type: event
    config:
      event_type: "user.created"
  - type: message
    config:
      pattern: "deploy*"
`)

	wf, err := workflow.ParseYAML(yamlData)
	if err != nil {
		t.Fatalf("ParseYAML: %v", err)
	}
	if len(wf.Triggers) != 4 {
		t.Fatalf("expected 4 triggers, got %d", len(wf.Triggers))
	}
	if wf.Triggers[0].Type != "cron" {
		t.Errorf("Trigger[0].Type: got %q", wf.Triggers[0].Type)
	}
	if wf.Triggers[1].Type != "webhook" {
		t.Errorf("Trigger[1].Type: got %q", wf.Triggers[1].Type)
	}
	if wf.Triggers[2].Type != "event" {
		t.Errorf("Trigger[2].Type: got %q", wf.Triggers[2].Type)
	}
	if wf.Triggers[3].Type != "message" {
		t.Errorf("Trigger[3].Type: got %q", wf.Triggers[3].Type)
	}

	// Validate should pass with these trigger types.
	if err := workflow.Validate(wf); err != nil {
		t.Fatalf("Validate: %v", err)
	}
}

func TestValidate_NoEdges(t *testing.T) {
	wf := &workflow.Workflow{
		Name: "no-edges",
		Nodes: []workflow.NodeDef{
			{ID: "a", Type: "transform", Config: map[string]interface{}{"template": "x"}},
		},
	}

	if err := workflow.Validate(wf); err != nil {
		t.Fatalf("Validate workflow without edges: %v", err)
	}
}

func TestValidate_Cycle(t *testing.T) {
	wf := &workflow.Workflow{
		Name: "cyclic",
		Nodes: []workflow.NodeDef{
			{ID: "a", Type: "transform"},
			{ID: "b", Type: "transform"},
		},
		Edges: []workflow.Edge{
			{From: "a", To: "b"},
			{From: "b", To: "a"},
		},
	}

	err := workflow.Validate(wf)
	if err == nil {
		t.Fatal("expected validation error for cyclic workflow")
	}
}

func TestValidate_MissingNode(t *testing.T) {
	wf := &workflow.Workflow{
		Name: "missing-node-ref",
		Nodes: []workflow.NodeDef{
			{ID: "a", Type: "transform"},
		},
		Edges: []workflow.Edge{
			{From: "a", To: "nonexistent"},
		},
	}

	err := workflow.Validate(wf)
	if err == nil {
		t.Fatal("expected validation error for edge referencing missing node")
	}
}

func TestValidate_NoName(t *testing.T) {
	wf := &workflow.Workflow{
		Nodes: []workflow.NodeDef{
			{ID: "a", Type: "transform"},
		},
	}

	err := workflow.Validate(wf)
	if err == nil {
		t.Fatal("expected validation error for workflow without name")
	}
}

func TestValidate_NoNodes(t *testing.T) {
	wf := &workflow.Workflow{
		Name: "empty",
	}

	err := workflow.Validate(wf)
	if err == nil {
		t.Fatal("expected validation error for workflow without nodes")
	}
}

func TestValidate_DuplicateNodeID(t *testing.T) {
	wf := &workflow.Workflow{
		Name: "dup-nodes",
		Nodes: []workflow.NodeDef{
			{ID: "a", Type: "transform"},
			{ID: "a", Type: "transform"},
		},
	}

	err := workflow.Validate(wf)
	if err == nil {
		t.Fatal("expected validation error for duplicate node IDs")
	}
}

func TestValidate_NodeMissingID(t *testing.T) {
	wf := &workflow.Workflow{
		Name: "node-no-id",
		Nodes: []workflow.NodeDef{
			{ID: "", Type: "transform"},
		},
	}

	err := workflow.Validate(wf)
	if err == nil {
		t.Fatal("expected validation error for node without ID")
	}
}

func TestValidate_DependsOnMissingNode(t *testing.T) {
	wf := &workflow.Workflow{
		Name: "missing-dep",
		Nodes: []workflow.NodeDef{
			{ID: "a", Type: "transform"},
			{ID: "b", Type: "transform", DependsOn: []string{"nonexistent"}},
		},
	}

	err := workflow.Validate(wf)
	if err == nil {
		t.Fatal("expected validation error for DependsOn referencing missing node")
	}
}

func TestValidate_UnknownTriggerType(t *testing.T) {
	wf := &workflow.Workflow{
		Name: "bad-trigger",
		Nodes: []workflow.NodeDef{
			{ID: "a", Type: "transform"},
		},
		Triggers: []workflow.TriggerConfig{
			{Type: "unknown_type"},
		},
	}

	err := workflow.Validate(wf)
	if err == nil {
		t.Fatal("expected validation error for unknown trigger type")
	}
}

func TestParseFile(t *testing.T) {
	// Create a temporary YAML file.
	tmp := t.TempDir()
	path := tmp + "/test_workflow.yaml"
	content := []byte(`
name: file-workflow
nodes:
  - id: n1
    type: transform
    config:
      template: "from file"
`)
	if err := writeFile(path, content); err != nil {
		t.Fatalf("write test file: %v", err)
	}

	wf, err := workflow.ParseFile(path)
	if err != nil {
		t.Fatalf("ParseFile: %v", err)
	}
	if wf.Name != "file-workflow" {
		t.Errorf("Name: got %q", wf.Name)
	}
}

func TestParseFile_NotFound(t *testing.T) {
	_, err := workflow.ParseFile("/nonexistent/path/workflow.yaml")
	if err == nil {
		t.Error("expected error for non-existent file")
	}
}

func TestValidate_ValidComplex(t *testing.T) {
	wf := &workflow.Workflow{
		Name:    "complex-valid",
		Version: "1.0",
		Nodes: []workflow.NodeDef{
			{ID: "start", Type: "transform"},
			{ID: "check", Type: "condition", Config: map[string]interface{}{"expression": "1 == 1"}},
			{ID: "yes", Type: "transform"},
			{ID: "no", Type: "transform"},
			{ID: "end", Type: "transform"},
		},
		Edges: []workflow.Edge{
			{From: "start", To: "check"},
			{From: "check", To: "yes", Condition: "true"},
			{From: "check", To: "no", Condition: "false"},
			{From: "yes", To: "end"},
			{From: "no", To: "end"},
		},
		Variables: map[string]string{"key": "value"},
		Metadata:  map[string]string{"author": "test"},
	}

	if err := workflow.Validate(wf); err != nil {
		t.Fatalf("Validate: %v", err)
	}
}

// writeFile is a small helper to write test data to a file.
func writeFile(path string, data []byte) error {
	return os.WriteFile(path, data, 0644)
}
