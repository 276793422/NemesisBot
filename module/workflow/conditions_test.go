package workflow_test

import (
	"testing"

	"github.com/276793422/NemesisBot/module/workflow"
)

func TestEvaluate_EmptyExpression(t *testing.T) {
	_, err := workflow.Evaluate("", nil)
	if err == nil {
		t.Fatal("expected error for empty expression")
	}
}

func TestEvaluate_BooleanLiterals(t *testing.T) {
	tests := []struct {
		expr     string
		expected bool
	}{
		{"true", true},
		{"false", false},
		{"1", true},
		{"0", false},
		{"yes", true},
		{"no", false},
		{"True", true},
		{"False", false},
	}

	for _, tt := range tests {
		t.Run(tt.expr, func(t *testing.T) {
			result, err := workflow.Evaluate(tt.expr, nil)
			if err != nil {
				t.Fatalf("Evaluate(%q) error: %v", tt.expr, err)
			}
			if result != tt.expected {
				t.Errorf("Evaluate(%q) = %v, want %v", tt.expr, result, tt.expected)
			}
		})
	}
}

func TestEvaluate_Equality(t *testing.T) {
	tests := []struct {
		name     string
		expr     string
		expected bool
	}{
		{"equal strings", `"hello" == "hello"`, true},
		{"unequal strings", `"hello" == "world"`, false},
		{"not equal strings", `"hello" != "world"`, true},
		{"not equal same strings", `"hello" != "hello"`, false},
		{"equal numbers", `42 == 42`, true},
		{"unequal numbers", `42 == 43`, false},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result, err := workflow.Evaluate(tt.expr, nil)
			if err != nil {
				t.Fatalf("Evaluate(%q) error: %v", tt.expr, err)
			}
			if result != tt.expected {
				t.Errorf("Evaluate(%q) = %v, want %v", tt.expr, result, tt.expected)
			}
		})
	}
}

func TestEvaluate_Comparison(t *testing.T) {
	tests := []struct {
		name     string
		expr     string
		expected bool
	}{
		{"gt true", `10 > 5`, true},
		{"gt false", `5 > 10`, false},
		{"lt true", `5 < 10`, true},
		{"lt false", `10 < 5`, false},
		{"gte equal", `5 >= 5`, true},
		{"gte greater", `10 >= 5`, true},
		{"gte false", `4 >= 5`, false},
		{"lte equal", `5 <= 5`, true},
		{"lte less", `4 <= 5`, true},
		{"lte false", `6 <= 5`, false},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result, err := workflow.Evaluate(tt.expr, nil)
			if err != nil {
				t.Fatalf("Evaluate(%q) error: %v", tt.expr, err)
			}
			if result != tt.expected {
				t.Errorf("Evaluate(%q) = %v, want %v", tt.expr, result, tt.expected)
			}
		})
	}
}

func TestEvaluate_StringOperations(t *testing.T) {
	tests := []struct {
		name     string
		expr     string
		expected bool
	}{
		{"contains true", `hello world contains world`, true},
		{"contains false", `hello world contains xyz`, false},
		{"starts_with true", `hello world starts_with hello`, true},
		{"starts_with false", `hello world starts_with world`, false},
		{"ends_with true", `hello world ends_with world`, true},
		{"ends_with false", `hello world ends_with hello`, false},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result, err := workflow.Evaluate(tt.expr, nil)
			if err != nil {
				t.Fatalf("Evaluate(%q) error: %v", tt.expr, err)
			}
			if result != tt.expected {
				t.Errorf("Evaluate(%q) = %v, want %v", tt.expr, result, tt.expected)
			}
		})
	}
}

func TestEvaluate_Regex(t *testing.T) {
	tests := []struct {
		name     string
		expr     string
		expected bool
	}{
		{"matches digit", `abc123 matches \d+`, true},
		{"matches full pattern", `hello matches ^h.*o$`, true},
		{"no match", `hello matches ^\d+$`, false},
		{"email pattern", `user@example.com matches .*@.*\.com$`, true},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result, err := workflow.Evaluate(tt.expr, nil)
			if err != nil {
				t.Fatalf("Evaluate(%q) error: %v", tt.expr, err)
			}
			if result != tt.expected {
				t.Errorf("Evaluate(%q) = %v, want %v", tt.expr, result, tt.expected)
			}
		})
	}
}

func TestEvaluate_InvalidRegex(t *testing.T) {
	_, err := workflow.Evaluate(`"test" matches "[invalid"`, nil)
	if err == nil {
		t.Fatal("expected error for invalid regex")
	}
}

func TestEvaluate_And(t *testing.T) {
	tests := []struct {
		name     string
		expr     string
		expected bool
	}{
		{"both true", `true and true`, true},
		{"first false", `false and true`, false},
		{"second false", `true and false`, false},
		{"both false", `false and false`, false},
		{"with comparisons", `5 > 3 and 10 > 7`, true},
		{"mixed", `5 > 3 and 10 < 7`, false},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result, err := workflow.Evaluate(tt.expr, nil)
			if err != nil {
				t.Fatalf("Evaluate(%q) error: %v", tt.expr, err)
			}
			if result != tt.expected {
				t.Errorf("Evaluate(%q) = %v, want %v", tt.expr, result, tt.expected)
			}
		})
	}
}

func TestEvaluate_Or(t *testing.T) {
	tests := []struct {
		name     string
		expr     string
		expected bool
	}{
		{"both true", `true or true`, true},
		{"first true", `true or false`, true},
		{"second true", `false or true`, true},
		{"both false", `false or false`, false},
		{"with comparisons", `5 > 10 or 10 > 7`, true},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result, err := workflow.Evaluate(tt.expr, nil)
			if err != nil {
				t.Fatalf("Evaluate(%q) error: %v", tt.expr, err)
			}
			if result != tt.expected {
				t.Errorf("Evaluate(%q) = %v, want %v", tt.expr, result, tt.expected)
			}
		})
	}
}

func TestEvaluate_Not(t *testing.T) {
	tests := []struct {
		name     string
		expr     string
		expected bool
	}{
		{"not true", `not true`, false},
		{"not false", `not false`, true},
		{"not 1", `not 1`, false},
		{"not 0", `not 0`, true},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result, err := workflow.Evaluate(tt.expr, nil)
			if err != nil {
				t.Fatalf("Evaluate(%q) error: %v", tt.expr, err)
			}
			if result != tt.expected {
				t.Errorf("Evaluate(%q) = %v, want %v", tt.expr, result, tt.expected)
			}
		})
	}
}

func TestEvaluate_Parenthesized(t *testing.T) {
	tests := []struct {
		name     string
		expr     string
		expected bool
	}{
		{"parens true", `(true)`, true},
		{"parens false", `(false)`, false},
		{"parens comparison", `(5 > 3)`, true},
		{"complex", `(true or false) and (false or true)`, true},
		{"complex false", `(false or false) and true`, false},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result, err := workflow.Evaluate(tt.expr, nil)
			if err != nil {
				t.Fatalf("Evaluate(%q) error: %v", tt.expr, err)
			}
			if result != tt.expected {
				t.Errorf("Evaluate(%q) = %v, want %v", tt.expr, result, tt.expected)
			}
		})
	}
}

func TestEvaluate_VariableResolution(t *testing.T) {
	vars := map[string]string{
		"name":  "world",
		"count": "42",
		"score": "95.5",
	}

	tests := []struct {
		name     string
		expr     string
		expected bool
	}{
		{"variable equality", `{{name}} == world`, true},
		{"variable inequality", `{{name}} != hello`, true},
		{"variable numeric gt", `{{count}} > 10`, true},
		{"variable numeric lt", `{{count}} < 100`, true},
		{"variable float", `{{score}} >= 90`, true},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result, err := workflow.Evaluate(tt.expr, vars)
			if err != nil {
				t.Fatalf("Evaluate(%q) error: %v", tt.expr, err)
			}
			if result != tt.expected {
				t.Errorf("Evaluate(%q) = %v, want %v", tt.expr, result, tt.expected)
			}
		})
	}
}

func TestEvaluate_MissingVariable(t *testing.T) {
	// Unresolved variable should remain as {{missing}}
	result, err := workflow.Evaluate("{{missing}} == value", nil)
	if err != nil {
		// This may error since {{missing}} stays as literal
		t.Logf("Evaluate with missing var error (expected): %v", err)
	}
	// The {{missing}} won't be resolved, so the comparison fails
	if result {
		t.Error("expected false for unresolved variable")
	}
}

func TestEvaluate_NumericComparisonWithStringFallback(t *testing.T) {
	tests := []struct {
		name     string
		expr     string
		expected bool
	}{
		{"string gt", `"abc" > "abb"`, true},
		{"string lt", `"abb" < "abc"`, true},
		{"string gte", `"abc" >= "abc"`, true},
		{"string lte", `"abc" <= "abd"`, true},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result, err := workflow.Evaluate(tt.expr, nil)
			if err != nil {
				t.Fatalf("Evaluate(%q) error: %v", tt.expr, err)
			}
			if result != tt.expected {
				t.Errorf("Evaluate(%q) = %v, want %v", tt.expr, result, tt.expected)
			}
		})
	}
}

func TestEvaluate_UnbalancedParens(t *testing.T) {
	_, err := workflow.Evaluate("(true and false", nil)
	if err == nil {
		t.Fatal("expected error for unbalanced parentheses")
	}
}

func TestEvaluate_ComplexExpressions(t *testing.T) {
	tests := []struct {
		name     string
		expr     string
		expected bool
	}{
		{"three and", `true and true and true`, true},
		{"three and one false", `true and false and true`, false},
		{"three or", `false or false or true`, true},
		{"three or all false", `false or false or false`, false},
		{"mixed precedence", `true or false and false`, true}, // or has lower precedence, but left-to-right
		{"not and", `not false and true`, true},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result, err := workflow.Evaluate(tt.expr, nil)
			if err != nil {
				t.Fatalf("Evaluate(%q) error: %v", tt.expr, err)
			}
			if result != tt.expected {
				t.Errorf("Evaluate(%q) = %v, want %v", tt.expr, result, tt.expected)
			}
		})
	}
}
