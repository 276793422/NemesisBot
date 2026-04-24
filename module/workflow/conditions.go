package workflow

import (
	"fmt"
	"regexp"
	"strconv"
	"strings"
)

// Evaluate evaluates a condition expression against a set of variables.
//
// Supported operators:
//   - Comparison: ==, !=, >, <, >=, <=
//   - String: contains, starts_with, ends_with, matches
//   - Logical: and, or, not
//
// Variables in expressions are resolved via {{variable}} syntax.
func Evaluate(expr string, vars map[string]string) (bool, error) {
	expr = strings.TrimSpace(expr)
	if expr == "" {
		return false, fmt.Errorf("empty expression")
	}

	// Resolve {{variable}} references
	re := regexp.MustCompile(`\{\{([^}]+)\}\}`)
	expr = re.ReplaceAllStringFunc(expr, func(match string) string {
		key := strings.TrimSpace(match[2 : len(match)-2])
		if val, ok := vars[key]; ok {
			return val
		}
		return match
	})

	return evalExpression(expr)
}

// evalExpression handles the top-level expression parsing.
// It supports "and" and "or" logical operators with proper precedence:
//   - "or" has lower precedence
//   - "and" has higher precedence
func evalExpression(expr string) (bool, error) {
	// Handle parenthesized expressions
	if strings.HasPrefix(expr, "(") && findMatchingParen(expr, 0) == len(expr)-1 {
		inner := expr[1 : len(expr)-1]
		return evalExpression(inner)
	}

	// Handle "not" prefix
	if strings.HasPrefix(expr, "not ") {
		inner := strings.TrimPrefix(expr, "not ")
		result, err := evalExpression(inner)
		if err != nil {
			return false, err
		}
		return !result, nil
	}

	// Split by "or" at the top level (respecting parentheses)
	parts, err := splitLogical(expr, " or ")
	if err != nil {
		return false, err
	}
	if len(parts) > 1 {
		for _, part := range parts {
			result, evalErr := evalExpression(part)
			if evalErr != nil {
				return false, evalErr
			}
			if result {
				return true, nil
			}
		}
		return false, nil
	}

	// Split by "and" at the top level
	parts, err = splitLogical(expr, " and ")
	if err != nil {
		return false, err
	}
	if len(parts) > 1 {
		for _, part := range parts {
			result, evalErr := evalExpression(part)
			if evalErr != nil {
				return false, evalErr
			}
			if !result {
				return false, nil
			}
		}
		return true, nil
	}

	// Single comparison
	return evalComparison(strings.TrimSpace(expr))
}

// splitLogical splits an expression by a logical operator, respecting
// parentheses nesting.
func splitLogical(expr, op string) ([]string, error) {
	var parts []string
	depth := 0
	current := ""

	i := 0
	for i < len(expr) {
		if expr[i] == '(' {
			depth++
			current += string(expr[i])
			i++
		} else if expr[i] == ')' {
			depth--
			current += string(expr[i])
			i++
		} else if depth == 0 && strings.HasPrefix(expr[i:], op) {
			parts = append(parts, strings.TrimSpace(current))
			current = ""
			i += len(op)
		} else {
			current += string(expr[i])
			i++
		}
	}

	if depth != 0 {
		return nil, fmt.Errorf("unbalanced parentheses in expression")
	}

	if strings.TrimSpace(current) != "" {
		parts = append(parts, strings.TrimSpace(current))
	}

	return parts, nil
}

// evalComparison evaluates a single comparison expression.
func evalComparison(expr string) (bool, error) {
	// Handle parenthesized
	if strings.HasPrefix(expr, "(") && findMatchingParen(expr, 0) == len(expr)-1 {
		return evalExpression(expr[1 : len(expr)-1])
	}

	// Check for "not" prefix
	if strings.HasPrefix(expr, "not ") {
		return evalExpression(expr)
	}

	// Try operators in order of specificity (longest first)
	operators := []struct {
		op   string
		eval func(left, right string) (bool, error)
	}{
		{"contains", evalContains},
		{"starts_with", evalStartsWith},
		{"ends_with", evalEndsWith},
		{"matches", evalMatches},
		{">=", evalGTE},
		{"<=", evalLTE},
		{"!=", evalNEQ},
		{"==", evalEQ},
		{">", evalGT},
		{"<", evalLT},
	}

	for _, op := range operators {
		idx := findOperator(expr, op.op)
		if idx >= 0 {
			left := strings.TrimSpace(expr[:idx])
			right := strings.TrimSpace(expr[idx+len(op.op):])
			return op.eval(left, right)
		}
	}

	// No operator found. Treat as a boolean value.
	switch strings.ToLower(expr) {
	case "true", "1", "yes":
		return true, nil
	case "false", "0", "no", "":
		return false, nil
	default:
		// Non-empty string that is not a recognized boolean
		// Check if it could be a variable reference
		if strings.Contains(expr, "{{") {
			return expr != "", nil
		}
		return false, fmt.Errorf("cannot evaluate expression %q as boolean", expr)
	}
}

// findOperator finds the index of an operator in an expression,
// respecting quoted strings and parentheses.
func findOperator(expr, op string) int {
	depth := 0
	inQuote := false

	for i := 0; i <= len(expr)-len(op); i++ {
		ch := expr[i]
		if ch == '"' {
			inQuote = !inQuote
			continue
		}
		if inQuote {
			continue
		}
		if ch == '(' {
			depth++
		} else if ch == ')' {
			depth--
		} else if depth == 0 && expr[i:i+len(op)] == op {
			// For symbolic operators (==, !=, >=, <=, >, <), ensure we don't
			// confuse them with word operators.
			if isSymbolicOperator(op) {
				return i
			}
			// For word operators, check word boundaries
			if i > 0 && expr[i-1] != ' ' {
				continue
			}
			if i+len(op) < len(expr) && expr[i+len(op)] != ' ' {
				continue
			}
			return i
		}
	}

	return -1
}

func isSymbolicOperator(op string) bool {
	return op == "==" || op == "!=" || op == ">=" || op == "<=" || op == ">" || op == "<"
}

// --- Operator evaluation functions ---

func evalEQ(left, right string) (bool, error) {
	return left == right, nil
}

func evalNEQ(left, right string) (bool, error) {
	return left != right, nil
}

func evalGT(left, right string) (bool, error) {
	lf, errL := strconv.ParseFloat(left, 64)
	rf, errR := strconv.ParseFloat(right, 64)
	if errL == nil && errR == nil {
		return lf > rf, nil
	}
	return left > right, nil
}

func evalLT(left, right string) (bool, error) {
	lf, errL := strconv.ParseFloat(left, 64)
	rf, errR := strconv.ParseFloat(right, 64)
	if errL == nil && errR == nil {
		return lf < rf, nil
	}
	return left < right, nil
}

func evalGTE(left, right string) (bool, error) {
	lf, errL := strconv.ParseFloat(left, 64)
	rf, errR := strconv.ParseFloat(right, 64)
	if errL == nil && errR == nil {
		return lf >= rf, nil
	}
	return left >= right, nil
}

func evalLTE(left, right string) (bool, error) {
	lf, errL := strconv.ParseFloat(left, 64)
	rf, errR := strconv.ParseFloat(right, 64)
	if errL == nil && errR == nil {
		return lf <= rf, nil
	}
	return left <= right, nil
}

func evalContains(left, right string) (bool, error) {
	return strings.Contains(left, right), nil
}

func evalStartsWith(left, right string) (bool, error) {
	return strings.HasPrefix(left, right), nil
}

func evalEndsWith(left, right string) (bool, error) {
	return strings.HasSuffix(left, right), nil
}

func evalMatches(left, right string) (bool, error) {
	re, err := regexp.Compile(right)
	if err != nil {
		return false, fmt.Errorf("invalid regex %q: %w", right, err)
	}
	return re.MatchString(left), nil
}

// findMatchingParen finds the index of the closing paren that matches
// the opening paren at position start.
func findMatchingParen(expr string, start int) int {
	depth := 0
	for i := start; i < len(expr); i++ {
		if expr[i] == '(' {
			depth++
		} else if expr[i] == ')' {
			depth--
			if depth == 0 {
				return i
			}
		}
	}
	return -1
}
