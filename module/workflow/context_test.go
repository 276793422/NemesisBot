package workflow_test

import (
	"testing"

	"github.com/276793422/NemesisBot/module/workflow"
)

func TestNewContext(t *testing.T) {
	input := map[string]interface{}{"key": "value"}
	ctx := workflow.NewContext(input)

	if ctx == nil {
		t.Fatal("NewContext() returned nil")
	}
	if ctx.Input["key"] != "value" {
		t.Errorf("Input[key] = %v, want value", ctx.Input["key"])
	}
	if ctx.Variables == nil {
		t.Error("Variables map is nil")
	}
	if ctx.NodeResults == nil {
		t.Error("NodeResults map is nil")
	}
}

func TestNewContext_NilInput(t *testing.T) {
	ctx := workflow.NewContext(nil)
	if ctx.Input != nil {
		t.Errorf("Input = %v, want nil", ctx.Input)
	}
}

func TestContext_SetVarGetVar(t *testing.T) {
	ctx := workflow.NewContext(nil)

	ctx.SetVar("name", "alice")
	ctx.SetVar("count", "42")

	if v := ctx.GetVar("name"); v != "alice" {
		t.Errorf("GetVar(name) = %q, want %q", v, "alice")
	}
	if v := ctx.GetVar("count"); v != "42" {
		t.Errorf("GetVar(count) = %q, want %q", v, "42")
	}
	if v := ctx.GetVar("nonexistent"); v != "" {
		t.Errorf("GetVar(nonexistent) = %q, want empty", v)
	}
}

func TestContext_SetVarOverwrite(t *testing.T) {
	ctx := workflow.NewContext(nil)

	ctx.SetVar("key", "first")
	ctx.SetVar("key", "second")

	if v := ctx.GetVar("key"); v != "second" {
		t.Errorf("GetVar(key) = %q, want %q", v, "second")
	}
}

func TestContext_SetNodeResultGetNodeResult(t *testing.T) {
	ctx := workflow.NewContext(nil)

	result := &workflow.NodeResult{
		NodeID: "n1",
		Output: "test output",
		State:  workflow.StateCompleted,
	}
	ctx.SetNodeResult("n1", result)

	got := ctx.GetNodeResult("n1")
	if got == nil {
		t.Fatal("GetNodeResult(n1) returned nil")
	}
	if got.Output != "test output" {
		t.Errorf("Output = %v, want %q", got.Output, "test output")
	}

	if got := ctx.GetNodeResult("nonexistent"); got != nil {
		t.Errorf("GetNodeResult(nonexistent) = %+v, want nil", got)
	}
}

func TestContext_GetAllVariables(t *testing.T) {
	ctx := workflow.NewContext(nil)
	ctx.SetVar("a", "1")
	ctx.SetVar("b", "2")

	allVars := ctx.GetAllVariables()
	if len(allVars) != 2 {
		t.Fatalf("len(GetAllVariables()) = %d, want 2", len(allVars))
	}
	if allVars["a"] != "1" || allVars["b"] != "2" {
		t.Errorf("GetAllVariables() = %v, unexpected values", allVars)
	}

	// Verify it's a copy
	allVars["a"] = "modified"
	if ctx.GetVar("a") != "1" {
		t.Error("GetAllVariables() returned a reference, not a copy")
	}
}

func TestContext_Resolve_PlainVariable(t *testing.T) {
	ctx := workflow.NewContext(nil)
	ctx.SetVar("greeting", "hello")

	result := ctx.Resolve("{{greeting}}")
	if result != "hello" {
		t.Errorf("Resolve({{greeting}}) = %q, want %q", result, "hello")
	}
}

func TestContext_Resolve_InputVariable(t *testing.T) {
	ctx := workflow.NewContext(map[string]interface{}{
		"user": "alice",
	})

	result := ctx.Resolve("{{input.user}}")
	if result != "alice" {
		t.Errorf("Resolve({{input.user}}) = %q, want %q", result, "alice")
	}
}

func TestContext_Resolve_InputMissing(t *testing.T) {
	ctx := workflow.NewContext(nil)

	result := ctx.Resolve("{{input.missing}}")
	// Should return unresolved pattern
	if result != "{{input.missing}}" {
		t.Errorf("Resolve({{input.missing}}) = %q, want unresolved", result)
	}
}

func TestContext_Resolve_NodeOutput(t *testing.T) {
	ctx := workflow.NewContext(nil)
	ctx.SetNodeResult("n1", &workflow.NodeResult{
		NodeID: "n1",
		Output: "node result value",
		State:  workflow.StateCompleted,
	})

	result := ctx.Resolve("{{n1}}")
	if result != "node result value" {
		t.Errorf("Resolve({{n1}}) = %q, want %q", result, "node result value")
	}
}

func TestContext_Resolve_NodeField(t *testing.T) {
	ctx := workflow.NewContext(nil)
	ctx.SetNodeResult("n1", &workflow.NodeResult{
		NodeID: "n1",
		Output: map[string]interface{}{
			"name":   "alice",
			"status": "active",
		},
		State: workflow.StateCompleted,
	})

	tests := []struct {
		template string
		expected string
	}{
		{"{{n1.name}}", "alice"},
		{"{{n1.status}}", "active"},
	}

	for _, tt := range tests {
		t.Run(tt.template, func(t *testing.T) {
			result := ctx.Resolve(tt.template)
			if result != tt.expected {
				t.Errorf("Resolve(%q) = %q, want %q", tt.template, result, tt.expected)
			}
		})
	}
}

func TestContext_Resolve_NodeFieldMissing(t *testing.T) {
	ctx := workflow.NewContext(nil)
	ctx.SetNodeResult("n1", &workflow.NodeResult{
		NodeID: "n1",
		Output: map[string]interface{}{"name": "alice"},
		State:  workflow.StateCompleted,
	})

	result := ctx.Resolve("{{n1.nonexistent}}")
	if result != "{{n1.nonexistent}}" {
		t.Errorf("Resolve({{n1.nonexistent}}) = %q, want unresolved", result)
	}
}

func TestContext_Resolve_NodeNotMap(t *testing.T) {
	ctx := workflow.NewContext(nil)
	ctx.SetNodeResult("n1", &workflow.NodeResult{
		NodeID: "n1",
		Output: "string output",
		State:  workflow.StateCompleted,
	})

	// n1.field where n1 output is string should not resolve
	result := ctx.Resolve("{{n1.field}}")
	if result != "{{n1.field}}" {
		t.Errorf("Resolve({{n1.field}}) = %q, want unresolved", result)
	}
}

func TestContext_Resolve_MultipleTemplates(t *testing.T) {
	ctx := workflow.NewContext(nil)
	ctx.SetVar("first", "hello")
	ctx.SetVar("second", "world")

	result := ctx.Resolve("{{first}} {{second}}")
	if result != "hello world" {
		t.Errorf("Resolve() = %q, want %q", result, "hello world")
	}
}

func TestContext_Resolve_UnresolvedVariable(t *testing.T) {
	ctx := workflow.NewContext(nil)
	result := ctx.Resolve("{{unknown}}")
	if result != "{{unknown}}" {
		t.Errorf("Resolve({{unknown}}) = %q, want %q", result, "{{unknown}}")
	}
}

func TestContext_Resolve_NoTemplate(t *testing.T) {
	ctx := workflow.NewContext(nil)
	result := ctx.Resolve("plain text")
	if result != "plain text" {
		t.Errorf("Resolve(plain text) = %q, want %q", result, "plain text")
	}
}

func TestContext_Resolve_EmptyString(t *testing.T) {
	ctx := workflow.NewContext(nil)
	result := ctx.Resolve("")
	if result != "" {
		t.Errorf("Resolve('') = %q, want empty", result)
	}
}

func TestContext_Clone(t *testing.T) {
	ctx := workflow.NewContext(map[string]interface{}{"input_key": "input_val"})
	ctx.SetVar("var1", "original")
	ctx.SetNodeResult("n1", &workflow.NodeResult{
		NodeID: "n1",
		Output: "result1",
		State:  workflow.StateCompleted,
	})

	clone := ctx.Clone()

	// Verify clone has same values
	if clone.GetVar("var1") != "original" {
		t.Errorf("clone GetVar(var1) = %q, want %q", clone.GetVar("var1"), "original")
	}
	if clone.GetNodeResult("n1").Output != "result1" {
		t.Errorf("clone GetNodeResult(n1).Output = %v, want result1", clone.GetNodeResult("n1").Output)
	}

	// Modify clone and verify independence
	clone.SetVar("var1", "modified")
	clone.SetVar("var2", "new")

	if ctx.GetVar("var1") != "original" {
		t.Error("modifying clone affected original Variables")
	}
	if ctx.GetVar("var2") != "" {
		t.Error("adding to clone affected original Variables")
	}
}

func TestContext_Clone_InputIndependence(t *testing.T) {
	ctx := workflow.NewContext(map[string]interface{}{"key": "value"})
	clone := ctx.Clone()

	clone.Input["key"] = "modified"
	if ctx.Input["key"] != "value" {
		t.Error("modifying clone Input affected original")
	}
}

func TestContext_Clone_NodeResults(t *testing.T) {
	ctx := workflow.NewContext(nil)
	ctx.SetNodeResult("n1", &workflow.NodeResult{
		NodeID: "n1",
		Output: "original",
		State:  workflow.StateCompleted,
	})

	clone := ctx.Clone()

	// Modify clone's node result
	clone.SetNodeResult("n1", &workflow.NodeResult{
		NodeID: "n1",
		Output: "modified",
		State:  workflow.StateFailed,
	})

	// Original should be unaffected (map entries are pointers, so we check the value)
	if ctx.GetNodeResult("n1").Output != "original" {
		t.Errorf("original NodeResult was affected by clone modification")
	}
}
