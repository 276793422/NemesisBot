//! Condition evaluation engine for workflow expressions.
//!
//! Supports comparison operators (==, !=, >, <, >=, <=), string operators
//! (contains, starts_with, ends_with, matches), and logical operators
//! (and, or, not). Mirrors the Go `conditions.go`.

use std::collections::HashMap;

/// Evaluate a condition expression against a set of variables.
///
/// Supported operators:
/// - Comparison: ==, !=, >, <, >=, <=
/// - String: contains, starts_with, ends_with, matches
/// - Logical: and, or, not
///
/// Variables in expressions are resolved via {{variable}} syntax.
pub fn evaluate(expr: &str, vars: &HashMap<String, String>) -> Result<bool, String> {
    let expr = expr.trim();
    if expr.is_empty() {
        return Err("empty expression".to_string());
    }

    // Resolve {{variable}} references
    let resolved = resolve_variables(expr, vars);

    eval_expression(&resolved)
}

/// Resolve {{variable}} references in an expression.
fn resolve_variables(expr: &str, vars: &HashMap<String, String>) -> String {
    let mut result = expr.to_string();
    let mut start = 0;

    while let Some(open) = result[start..].find("{{") {
        let open_abs = start + open;
        if let Some(close) = result[open_abs + 2..].find("}}") {
            let close_abs = open_abs + 2 + close;
            let key = result[open_abs + 2..close_abs].trim();

            if let Some(val) = vars.get(key) {
                result.replace_range(open_abs..close_abs + 2, val);
                // Don't advance start, as the replacement may contain {{ that need resolving
                start = open_abs + val.len();
            } else {
                start = close_abs + 2;
            }
        } else {
            break;
        }
    }

    result
}

/// Handle top-level expression parsing with logical operator precedence.
fn eval_expression(expr: &str) -> Result<bool, String> {
    let expr = expr.trim();

    // Handle parenthesized expressions
    if expr.starts_with('(') && find_matching_paren(expr, 0) == expr.len() - 1 {
        return eval_expression(&expr[1..expr.len() - 1]);
    }

    // Handle "not" prefix
    if expr.starts_with("not ") {
        let inner = &expr[4..];
        let result = eval_expression(inner)?;
        return Ok(!result);
    }

    // Split by "or" at the top level
    let parts = split_logical(expr, " or ");
    if parts.len() > 1 {
        for part in parts {
            if eval_expression(part)? {
                return Ok(true);
            }
        }
        return Ok(false);
    }

    // Split by "and" at the top level
    let parts = split_logical(expr, " and ");
    if parts.len() > 1 {
        for part in parts {
            if !eval_expression(part)? {
                return Ok(false);
            }
        }
        return Ok(true);
    }

    // Single comparison
    eval_comparison(expr.trim())
}

/// Split an expression by a logical operator, respecting parentheses.
fn split_logical<'a>(expr: &'a str, op: &str) -> Vec<&'a str> {
    let mut parts = Vec::new();
    let mut depth = 0;
    let mut current_start = 0;
    let mut in_quote = false;
    let chars: Vec<char> = expr.chars().collect();
    let op_chars: Vec<char> = op.chars().collect();

    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];

        if ch == '"' {
            in_quote = !in_quote;
            i += 1;
            continue;
        }

        if in_quote {
            i += 1;
            continue;
        }

        if ch == '(' {
            depth += 1;
        } else if ch == ')' {
            depth -= 1;
        } else if depth == 0 && i + op_chars.len() <= chars.len() {
            let slice: String = chars[i..i + op_chars.len()].iter().collect();
            if slice == op {
                parts.push(expr[current_start..i].trim());
                current_start = i + op_chars.len();
                i += op_chars.len();
                continue;
            }
        }

        i += 1;
    }

    let remainder = expr[current_start..].trim();
    if !remainder.is_empty() {
        parts.push(remainder);
    }

    parts
}

/// Evaluate a single comparison expression.
fn eval_comparison(expr: &str) -> Result<bool, String> {
    let expr = expr.trim();

    // Handle parenthesized
    if expr.starts_with('(') && find_matching_paren(expr, 0) == expr.len() - 1 {
        return eval_expression(&expr[1..expr.len() - 1]);
    }

    // Handle "not" prefix
    if expr.starts_with("not ") {
        return eval_expression(expr);
    }

    // Try operators in order of specificity (longest first)
    let operators: &[(&str, fn(&str, &str) -> Result<bool, String>)] = &[
        ("contains", eval_contains),
        ("starts_with", eval_starts_with),
        ("ends_with", eval_ends_with),
        ("matches", eval_matches),
        (">=", eval_gte),
        ("<=", eval_lte),
        ("!=", eval_neq),
        ("==", eval_eq),
        (">", eval_gt),
        ("<", eval_lt),
    ];

    for (op, eval_fn) in operators {
        if let Some(idx) = find_operator(expr, op) {
            let left = expr[..idx].trim();
            let right = expr[idx + op.len()..].trim();
            return eval_fn(left, right);
        }
    }

    // No operator found. Treat as a boolean value.
    match expr.to_lowercase().as_str() {
        "true" | "1" | "yes" => Ok(true),
        "false" | "0" | "no" | "" => Ok(false),
        _ => {
            // Check if it could be a variable reference
            if expr.contains("{{") {
                Ok(!expr.is_empty())
            } else {
                Err(format!("cannot evaluate expression {:?} as boolean", expr))
            }
        }
    }
}

/// Find the index of an operator in an expression, respecting quoted strings
/// and parentheses.
fn find_operator(expr: &str, op: &str) -> Option<usize> {
    let mut depth = 0;
    let mut in_quote = false;
    let chars: Vec<char> = expr.chars().collect();
    let op_chars: Vec<char> = op.chars().collect();

    for i in 0..chars.len().saturating_sub(op_chars.len()) + 1 {
        let ch = chars[i];
        if ch == '"' {
            in_quote = !in_quote;
            continue;
        }
        if in_quote {
            continue;
        }
        if ch == '(' {
            depth += 1;
        } else if ch == ')' {
            depth -= 1;
        } else if depth == 0 && i + op_chars.len() <= chars.len() {
            let slice: String = chars[i..i + op_chars.len()].iter().collect();
            if slice == *op {
                // For symbolic operators, return immediately
                if is_symbolic_operator(op) {
                    return Some(i);
                }
                // For word operators, check word boundaries
                if i > 0 && !chars[i - 1].is_whitespace() {
                    continue;
                }
                if i + op_chars.len() < chars.len() && !chars[i + op_chars.len()].is_whitespace() {
                    continue;
                }
                return Some(i);
            }
        }
    }

    None
}

fn is_symbolic_operator(op: &str) -> bool {
    matches!(op, "==" | "!=" | ">=" | "<=" | ">" | "<")
}

// ---------------------------------------------------------------------------
// Operator evaluation functions
// ---------------------------------------------------------------------------

fn eval_eq(left: &str, right: &str) -> Result<bool, String> {
    Ok(left == right)
}

fn eval_neq(left: &str, right: &str) -> Result<bool, String> {
    Ok(left != right)
}

fn eval_gt(left: &str, right: &str) -> Result<bool, String> {
    let lf = left.parse::<f64>();
    let rf = right.parse::<f64>();
    if let (Ok(l), Ok(r)) = (lf, rf) {
        Ok(l > r)
    } else {
        Ok(left > right)
    }
}

fn eval_lt(left: &str, right: &str) -> Result<bool, String> {
    let lf = left.parse::<f64>();
    let rf = right.parse::<f64>();
    if let (Ok(l), Ok(r)) = (lf, rf) {
        Ok(l < r)
    } else {
        Ok(left < right)
    }
}

fn eval_gte(left: &str, right: &str) -> Result<bool, String> {
    let lf = left.parse::<f64>();
    let rf = right.parse::<f64>();
    if let (Ok(l), Ok(r)) = (lf, rf) {
        Ok(l >= r)
    } else {
        Ok(left >= right)
    }
}

fn eval_lte(left: &str, right: &str) -> Result<bool, String> {
    let lf = left.parse::<f64>();
    let rf = right.parse::<f64>();
    if let (Ok(l), Ok(r)) = (lf, rf) {
        Ok(l <= r)
    } else {
        Ok(left <= right)
    }
}

fn eval_contains(left: &str, right: &str) -> Result<bool, String> {
    Ok(left.contains(right))
}

fn eval_starts_with(left: &str, right: &str) -> Result<bool, String> {
    Ok(left.starts_with(right))
}

fn eval_ends_with(left: &str, right: &str) -> Result<bool, String> {
    Ok(left.ends_with(right))
}

fn eval_matches(left: &str, right: &str) -> Result<bool, String> {
    regex::Regex::new(right)
        .map(|re| re.is_match(left))
        .map_err(|e| format!("invalid regex {:?}: {}", right, e))
}

/// Find the closing paren that matches the opening paren at position start.
fn find_matching_paren(expr: &str, start: usize) -> usize {
    let mut depth = 0;
    for (i, ch) in expr.chars().enumerate().skip(start) {
        if ch == '(' {
            depth += 1;
        } else if ch == ')' {
            depth -= 1;
            if depth == 0 {
                return i;
            }
        }
    }
    usize::MAX // no matching paren
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
