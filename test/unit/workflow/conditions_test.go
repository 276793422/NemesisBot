package workflow_test

import (
	"testing"

	"github.com/276793422/NemesisBot/module/workflow"
)

func TestEvaluate_Equal(t *testing.T) {
	result, err := workflow.Evaluate("hello == hello", nil)
	if err != nil {
		t.Fatalf("Evaluate: %v", err)
	}
	if !result {
		t.Error("expected true for 'hello == hello'")
	}

	result2, err := workflow.Evaluate("hello == world", nil)
	if err != nil {
		t.Fatalf("Evaluate: %v", err)
	}
	if result2 {
		t.Error("expected false for 'hello == world'")
	}
}

func TestEvaluate_NotEqual(t *testing.T) {
	result, err := workflow.Evaluate("hello != world", nil)
	if err != nil {
		t.Fatalf("Evaluate: %v", err)
	}
	if !result {
		t.Error("expected true for 'hello != world'")
	}

	result2, err := workflow.Evaluate("hello != hello", nil)
	if err != nil {
		t.Fatalf("Evaluate: %v", err)
	}
	if result2 {
		t.Error("expected false for 'hello != hello'")
	}
}

func TestEvaluate_GreaterThan(t *testing.T) {
	result, err := workflow.Evaluate("5 > 3", nil)
	if err != nil {
		t.Fatalf("Evaluate: %v", err)
	}
	if !result {
		t.Error("expected true for '5 > 3'")
	}

	result2, err := workflow.Evaluate("3 > 5", nil)
	if err != nil {
		t.Fatalf("Evaluate: %v", err)
	}
	if result2 {
		t.Error("expected false for '3 > 5'")
	}

	// Equal values.
	result3, err := workflow.Evaluate("5 > 5", nil)
	if err != nil {
		t.Fatalf("Evaluate: %v", err)
	}
	if result3 {
		t.Error("expected false for '5 > 5'")
	}
}

func TestEvaluate_LessThan(t *testing.T) {
	result, err := workflow.Evaluate("3 < 5", nil)
	if err != nil {
		t.Fatalf("Evaluate: %v", err)
	}
	if !result {
		t.Error("expected true for '3 < 5'")
	}
}

func TestEvaluate_GreaterThanOrEqual(t *testing.T) {
	result, err := workflow.Evaluate("5 >= 5", nil)
	if err != nil {
		t.Fatalf("Evaluate: %v", err)
	}
	if !result {
		t.Error("expected true for '5 >= 5'")
	}

	result2, err := workflow.Evaluate("5 >= 3", nil)
	if err != nil {
		t.Fatalf("Evaluate: %v", err)
	}
	if !result2 {
		t.Error("expected true for '5 >= 3'")
	}
}

func TestEvaluate_LessThanOrEqual(t *testing.T) {
	result, err := workflow.Evaluate("3 <= 5", nil)
	if err != nil {
		t.Fatalf("Evaluate: %v", err)
	}
	if !result {
		t.Error("expected true for '3 <= 5'")
	}

	result2, err := workflow.Evaluate("5 <= 5", nil)
	if err != nil {
		t.Fatalf("Evaluate: %v", err)
	}
	if !result2 {
		t.Error("expected true for '5 <= 5'")
	}
}

func TestEvaluate_Contains(t *testing.T) {
	result, err := workflow.Evaluate("hello contains ell", nil)
	if err != nil {
		t.Fatalf("Evaluate: %v", err)
	}
	if !result {
		t.Error("expected true for 'hello contains ell'")
	}

	result2, err := workflow.Evaluate("hello contains xyz", nil)
	if err != nil {
		t.Fatalf("Evaluate: %v", err)
	}
	if result2 {
		t.Error("expected false for 'hello contains xyz'")
	}
}

func TestEvaluate_StartsWith(t *testing.T) {
	result, err := workflow.Evaluate("hello starts_with hel", nil)
	if err != nil {
		t.Fatalf("Evaluate: %v", err)
	}
	if !result {
		t.Error("expected true for 'hello starts_with hel'")
	}

	result2, err := workflow.Evaluate("hello starts_with llo", nil)
	if err != nil {
		t.Fatalf("Evaluate: %v", err)
	}
	if result2 {
		t.Error("expected false for 'hello starts_with llo'")
	}
}

func TestEvaluate_EndsWith(t *testing.T) {
	result, err := workflow.Evaluate("hello ends_with llo", nil)
	if err != nil {
		t.Fatalf("Evaluate: %v", err)
	}
	if !result {
		t.Error("expected true for 'hello ends_with llo'")
	}
}

func TestEvaluate_Matches(t *testing.T) {
	result, err := workflow.Evaluate("hello matches ^he.*", nil)
	if err != nil {
		t.Fatalf("Evaluate: %v", err)
	}
	if !result {
		t.Error("expected true for 'hello matches ^he.*'")
	}

	result2, err := workflow.Evaluate("hello matches ^xyz.*", nil)
	if err != nil {
		t.Fatalf("Evaluate: %v", err)
	}
	if result2 {
		t.Error("expected false for 'hello matches ^xyz.*'")
	}
}

func TestEvaluate_Matches_InvalidRegex(t *testing.T) {
	_, err := workflow.Evaluate("hello matches [invalid", nil)
	if err == nil {
		t.Error("expected error for invalid regex")
	}
}

func TestEvaluate_AndOr(t *testing.T) {
	// and: both true.
	result, err := workflow.Evaluate("1 == 1 and 2 == 2", nil)
	if err != nil {
		t.Fatalf("Evaluate and: %v", err)
	}
	if !result {
		t.Error("expected true for '1 == 1 and 2 == 2'")
	}

	// and: one false.
	result2, err := workflow.Evaluate("1 == 1 and 2 == 3", nil)
	if err != nil {
		t.Fatalf("Evaluate and false: %v", err)
	}
	if result2 {
		t.Error("expected false for '1 == 1 and 2 == 3'")
	}

	// or: one true.
	result3, err := workflow.Evaluate("1 == 2 or 2 == 2", nil)
	if err != nil {
		t.Fatalf("Evaluate or: %v", err)
	}
	if !result3 {
		t.Error("expected true for '1 == 2 or 2 == 2'")
	}

	// or: both false.
	result4, err := workflow.Evaluate("1 == 2 or 3 == 4", nil)
	if err != nil {
		t.Fatalf("Evaluate or false: %v", err)
	}
	if result4 {
		t.Error("expected false for '1 == 2 or 3 == 4'")
	}
}

func TestEvaluate_VariableResolution(t *testing.T) {
	vars := map[string]string{
		"var": "expected",
	}

	result, err := workflow.Evaluate("{{var}} == expected", vars)
	if err != nil {
		t.Fatalf("Evaluate: %v", err)
	}
	if !result {
		t.Error("expected true when {{var}} resolves to 'expected'")
	}

	// Unresolved variable.
	result2, err := workflow.Evaluate("{{unknown}} == expected", vars)
	if err != nil {
		t.Fatalf("Evaluate unresolved: %v", err)
	}
	if result2 {
		t.Error("expected false for unresolved variable")
	}
}

func TestEvaluate_ComplexExpression(t *testing.T) {
	vars := map[string]string{
		"status": "200",
		"ready":  "true",
	}

	// Nested and/or.
	result, err := workflow.Evaluate("{{status}} == 200 and ({{ready}} == true or 1 == 0)", vars)
	if err != nil {
		t.Fatalf("Evaluate complex: %v", err)
	}
	if !result {
		t.Error("expected true for complex expression")
	}
}

func TestEvaluate_Not(t *testing.T) {
	result, err := workflow.Evaluate("not false", nil)
	if err != nil {
		t.Fatalf("Evaluate: %v", err)
	}
	if !result {
		t.Error("expected true for 'not false'")
	}

	result2, err := workflow.Evaluate("not true", nil)
	if err != nil {
		t.Fatalf("Evaluate: %v", err)
	}
	if result2 {
		t.Error("expected false for 'not true'")
	}
}

func TestEvaluate_BooleanLiterals(t *testing.T) {
	tests := []struct {
		expr string
		want bool
	}{
		{"true", true},
		{"false", false},
		{"yes", true},
		{"no", false},
		{"1", true},
		{"0", false},
	}
	for _, tt := range tests {
		result, err := workflow.Evaluate(tt.expr, nil)
		if err != nil {
			t.Errorf("Evaluate(%q): %v", tt.expr, err)
			continue
		}
		if result != tt.want {
			t.Errorf("Evaluate(%q) = %v, want %v", tt.expr, result, tt.want)
		}
	}
}

func TestEvaluate_EmptyExpression(t *testing.T) {
	_, err := workflow.Evaluate("", nil)
	if err == nil {
		t.Error("expected error for empty expression")
	}
}

func TestEvaluate_ParenthesizedExpression(t *testing.T) {
	result, err := workflow.Evaluate("(1 == 1)", nil)
	if err != nil {
		t.Fatalf("Evaluate: %v", err)
	}
	if !result {
		t.Error("expected true for '(1 == 1)'")
	}

	// Nested parentheses.
	result2, err := workflow.Evaluate("((1 == 1))", nil)
	if err != nil {
		t.Fatalf("Evaluate: %v", err)
	}
	if !result2 {
		t.Error("expected true for '((1 == 1))'")
	}
}

func TestEvaluate_UnbalancedParens(t *testing.T) {
	_, err := workflow.Evaluate("(1 == 1", nil)
	if err == nil {
		t.Error("expected error for unbalanced parentheses")
	}
}

func TestEvaluate_NumericComparison(t *testing.T) {
	result, err := workflow.Evaluate("10.5 > 9.9", nil)
	if err != nil {
		t.Fatalf("Evaluate: %v", err)
	}
	if !result {
		t.Error("expected true for '10.5 > 9.9'")
	}

	result2, err := workflow.Evaluate("-1 < 0", nil)
	if err != nil {
		t.Fatalf("Evaluate: %v", err)
	}
	if !result2 {
		t.Error("expected true for '-1 < 0'")
	}
}
