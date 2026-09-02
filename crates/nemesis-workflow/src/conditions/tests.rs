use super::*;

#[test]
fn test_evaluate_eq() {
    let vars = HashMap::new();
    assert!(evaluate("hello == hello", &vars).unwrap());
    assert!(!evaluate("hello == world", &vars).unwrap());
}

#[test]
fn test_evaluate_neq() {
    let vars = HashMap::new();
    assert!(evaluate("hello != world", &vars).unwrap());
    assert!(!evaluate("hello != hello", &vars).unwrap());
}

#[test]
fn test_evaluate_gt_numeric() {
    let vars = HashMap::new();
    assert!(evaluate("10 > 5", &vars).unwrap());
    assert!(!evaluate("5 > 10", &vars).unwrap());
}

#[test]
fn test_evaluate_lt_numeric() {
    let vars = HashMap::new();
    assert!(evaluate("5 < 10", &vars).unwrap());
    assert!(!evaluate("10 < 5", &vars).unwrap());
}

#[test]
fn test_evaluate_gte() {
    let vars = HashMap::new();
    assert!(evaluate("10 >= 10", &vars).unwrap());
    assert!(evaluate("10 >= 5", &vars).unwrap());
    assert!(!evaluate("5 >= 10", &vars).unwrap());
}

#[test]
fn test_evaluate_lte() {
    let vars = HashMap::new();
    assert!(evaluate("10 <= 10", &vars).unwrap());
    assert!(evaluate("5 <= 10", &vars).unwrap());
    assert!(!evaluate("10 <= 5", &vars).unwrap());
}

#[test]
fn test_evaluate_contains() {
    let vars = HashMap::new();
    assert!(evaluate("hello world contains world", &vars).unwrap());
    assert!(!evaluate("hello world contains xyz", &vars).unwrap());
}

#[test]
fn test_evaluate_starts_with() {
    let vars = HashMap::new();
    assert!(evaluate("hello world starts_with hello", &vars).unwrap());
    assert!(!evaluate("hello world starts_with world", &vars).unwrap());
}

#[test]
fn test_evaluate_ends_with() {
    let vars = HashMap::new();
    assert!(evaluate("hello world ends_with world", &vars).unwrap());
    assert!(!evaluate("hello world ends_with hello", &vars).unwrap());
}

#[test]
fn test_evaluate_matches() {
    let vars = HashMap::new();
    assert!(evaluate("hello123 matches h.*o\\d+", &vars).unwrap());
    assert!(!evaluate("hello matches \\d+", &vars).unwrap());
}

#[test]
fn test_evaluate_and() {
    let vars = HashMap::new();
    assert!(evaluate("true and true", &vars).unwrap());
    assert!(!evaluate("true and false", &vars).unwrap());
}

#[test]
fn test_evaluate_or() {
    let vars = HashMap::new();
    assert!(evaluate("false or true", &vars).unwrap());
    assert!(!evaluate("false or false", &vars).unwrap());
}

#[test]
fn test_evaluate_not() {
    let vars = HashMap::new();
    assert!(evaluate("not false", &vars).unwrap());
    assert!(!evaluate("not true", &vars).unwrap());
}

#[test]
fn test_evaluate_boolean_literals() {
    let vars = HashMap::new();
    assert!(evaluate("true", &vars).unwrap());
    assert!(evaluate("1", &vars).unwrap());
    assert!(evaluate("yes", &vars).unwrap());
    assert!(!evaluate("false", &vars).unwrap());
    assert!(!evaluate("0", &vars).unwrap());
    assert!(!evaluate("no", &vars).unwrap());
}

#[test]
fn test_evaluate_variable_resolution() {
    let mut vars = HashMap::new();
    vars.insert("status".to_string(), "ok".to_string());
    vars.insert("count".to_string(), "10".to_string());
    assert!(evaluate("{{status}} == ok", &vars).unwrap());
    assert!(evaluate("{{count}} > 5", &vars).unwrap());
    assert!(!evaluate("{{count}} < 5", &vars).unwrap());
}

#[test]
fn test_evaluate_complex_and_or() {
    let vars = HashMap::new();
    assert!(evaluate("1 == 1 and 2 == 2", &vars).unwrap());
    assert!(evaluate("1 == 2 or 2 == 2", &vars).unwrap());
    assert!(!evaluate("1 == 2 and 2 == 2", &vars).unwrap());
}

#[test]
fn test_evaluate_parentheses() {
    let vars = HashMap::new();
    assert!(evaluate("(1 == 1) and (2 == 2)", &vars).unwrap());
    assert!(evaluate("(false or true) and true", &vars).unwrap());
}

#[test]
fn test_evaluate_empty_expression() {
    let vars = HashMap::new();
    assert!(evaluate("", &vars).is_err());
}

#[test]
fn test_evaluate_whitespace_expression() {
    let vars = HashMap::new();
    assert!(evaluate("   ", &vars).is_err());
}

#[test]
fn test_evaluate_numeric_comparisons() {
    let vars = HashMap::new();
    assert!(evaluate("100 > 50", &vars).unwrap());
    assert!(evaluate("50 < 100", &vars).unwrap());
    assert!(evaluate("100 >= 100", &vars).unwrap());
    assert!(evaluate("50 <= 50", &vars).unwrap());
    assert!(evaluate("100 != 50", &vars).unwrap());
    assert!(evaluate("100 == 100", &vars).unwrap());
}

#[test]
fn test_evaluate_string_comparisons() {
    let vars = HashMap::new();
    assert!(evaluate("abc == abc", &vars).unwrap());
    assert!(!evaluate("abc == def", &vars).unwrap());
    assert!(evaluate("abc != def", &vars).unwrap());
}

#[test]
fn test_evaluate_complex_variable_resolution() {
    let mut vars = HashMap::new();
    vars.insert("env".to_string(), "production".to_string());
    vars.insert("region".to_string(), "us-east".to_string());

    assert!(evaluate("{{env}} == production", &vars).unwrap());
    assert!(!evaluate("{{env}} == staging", &vars).unwrap());
    assert!(evaluate("{{env}} == production and {{region}} == us-east", &vars).unwrap());
    assert!(evaluate("{{env}} == staging or {{region}} == us-east", &vars).unwrap());
}

#[test]
fn test_evaluate_nested_parentheses() {
    let vars = HashMap::new();
    assert!(evaluate("((true))", &vars).unwrap());
    assert!(evaluate("((1 == 1)) and (2 == 2)", &vars).unwrap());
    assert!(!evaluate("(true and false) or (false and false)", &vars).unwrap());
}

#[test]
fn test_evaluate_not_with_comparison() {
    let vars = HashMap::new();
    assert!(evaluate("not 1 == 2", &vars).unwrap());
    assert!(!evaluate("not 1 == 1", &vars).unwrap());
}

#[test]
fn test_evaluate_matches_various_patterns() {
    let vars = HashMap::new();
    assert!(evaluate("hello matches hel.*", &vars).unwrap());
    assert!(evaluate("test123 matches test\\d+", &vars).unwrap());
    assert!(!evaluate("hello matches ^\\d+$", &vars).unwrap());
}

#[test]
fn test_evaluate_matches_invalid_regex() {
    let vars = HashMap::new();
    assert!(evaluate("test matches [invalid", &vars).is_err());
}

#[test]
fn test_evaluate_boolean_literals_case_insensitive() {
    let vars = HashMap::new();
    assert!(evaluate("True", &vars).unwrap());
    assert!(!evaluate("False", &vars).unwrap());
}

#[test]
fn test_evaluate_multiple_and_conditions() {
    let vars = HashMap::new();
    assert!(evaluate("1 == 1 and 2 == 2 and 3 == 3", &vars).unwrap());
    assert!(!evaluate("1 == 1 and 2 == 2 and 3 == 4", &vars).unwrap());
}

#[test]
fn test_evaluate_multiple_or_conditions() {
    let vars = HashMap::new();
    assert!(evaluate("1 == 2 or 2 == 3 or 3 == 3", &vars).unwrap());
    assert!(!evaluate("1 == 2 or 2 == 3 or 3 == 4", &vars).unwrap());
}

#[test]
fn test_evaluate_string_contains_empty() {
    let vars = HashMap::new();
    // Empty string is always contained
    assert!(evaluate("hello contains ", &vars).unwrap());
}

#[test]
fn test_evaluate_starts_ends_with() {
    let vars = HashMap::new();
    assert!(evaluate("hello world starts_with hello", &vars).unwrap());
    assert!(!evaluate("hello world starts_with world", &vars).unwrap());
    assert!(evaluate("hello world ends_with world", &vars).unwrap());
    assert!(!evaluate("hello world ends_with hello", &vars).unwrap());
}

#[test]
fn test_evaluate_unresolved_variable() {
    let vars = HashMap::new();
    // Missing variable - the {{}} stays in place and evaluation tries to handle it
    let result = evaluate("{{missing}} == test", &vars);
    // The variable won't be resolved, so comparison fails
    assert!(result.is_ok());
    assert!(!result.unwrap());
}

#[test]
fn test_evaluate_gt_lexicographic() {
    let vars = HashMap::new();
    // Non-numeric values compared lexicographically
    assert!(evaluate("zebra > apple", &vars).unwrap());
    assert!(!evaluate("apple > zebra", &vars).unwrap());
}

#[test]
fn test_evaluate_unparseable_expression() {
    let vars = HashMap::new();
    let result = evaluate("something_unparseable", &vars);
    // Should return an error for things that can't be evaluated
    assert!(result.is_err());
}

#[test]
fn test_evaluate_eq_numeric_strings() {
    let vars = HashMap::new();
    assert!(evaluate("100 == 100", &vars).unwrap());
    assert!(!evaluate("100 == 200", &vars).unwrap());
}

#[test]
fn test_evaluate_neq_different_types() {
    let vars = HashMap::new();
    assert!(evaluate("hello != 123", &vars).unwrap());
}

#[test]
fn test_evaluate_gt_float() {
    let vars = HashMap::new();
    assert!(evaluate("3.14 > 2.71", &vars).unwrap());
    assert!(!evaluate("2.71 > 3.14", &vars).unwrap());
}

#[test]
fn test_evaluate_lt_float() {
    let vars = HashMap::new();
    assert!(evaluate("2.71 < 3.14", &vars).unwrap());
}

#[test]
fn test_evaluate_gte_float() {
    let vars = HashMap::new();
    assert!(evaluate("3.14 >= 3.14", &vars).unwrap());
    assert!(evaluate("3.15 >= 3.14", &vars).unwrap());
}

#[test]
fn test_evaluate_lte_float() {
    let vars = HashMap::new();
    assert!(evaluate("3.14 <= 3.14", &vars).unwrap());
    assert!(evaluate("3.13 <= 3.14", &vars).unwrap());
}

#[test]
fn test_evaluate_contains_substring() {
    let vars = HashMap::new();
    assert!(evaluate("hello world contains orl", &vars).unwrap());
    assert!(!evaluate("hello world contains xyz", &vars).unwrap());
}

#[test]
fn test_evaluate_starts_with_prefix() {
    let vars = HashMap::new();
    assert!(evaluate("hello starts_with h", &vars).unwrap());
    assert!(!evaluate("hello starts_with ello", &vars).unwrap());
}

#[test]
fn test_evaluate_ends_with_suffix() {
    let vars = HashMap::new();
    assert!(evaluate("hello ends_with llo", &vars).unwrap());
    assert!(!evaluate("hello ends_with hel", &vars).unwrap());
}

#[test]
fn test_evaluate_matches_email() {
    let vars = HashMap::new();
    assert!(evaluate("user@example.com matches .*@.*\\.com", &vars).unwrap());
    assert!(!evaluate("not-an-email matches .*@.*\\.com", &vars).unwrap());
}

#[test]
fn test_evaluate_not_true() {
    let vars = HashMap::new();
    assert!(!evaluate("not true", &vars).unwrap());
}

#[test]
fn test_evaluate_not_false() {
    let vars = HashMap::new();
    assert!(evaluate("not false", &vars).unwrap());
}

#[test]
fn test_evaluate_and_short_circuit() {
    let vars = HashMap::new();
    assert!(!evaluate("false and 1 == 1", &vars).unwrap());
}

#[test]
fn test_evaluate_or_short_circuit() {
    let vars = HashMap::new();
    assert!(evaluate("true or 1 == 2", &vars).unwrap());
}

#[test]
fn test_evaluate_nested_not() {
    let vars = HashMap::new();
    assert!(evaluate("not false", &vars).unwrap());
    assert!(!evaluate("not true", &vars).unwrap());
}

#[test]
fn test_evaluate_variable_in_and_condition() {
    let mut vars = HashMap::new();
    vars.insert("x".to_string(), "10".to_string());
    vars.insert("y".to_string(), "20".to_string());
    assert!(evaluate("{{x}} > 5 and {{y}} > 15", &vars).unwrap());
    assert!(!evaluate("{{x}} > 15 and {{y}} > 15", &vars).unwrap());
}

#[test]
fn test_evaluate_variable_in_or_condition() {
    let mut vars = HashMap::new();
    vars.insert("status".to_string(), "error".to_string());
    assert!(evaluate("{{status}} == ok or {{status}} == error", &vars).unwrap());
    assert!(!evaluate("{{status}} == ok or {{status}} == pending", &vars).unwrap());
}

#[test]
fn test_evaluate_complex_parenthesized() {
    let vars = HashMap::new();
    assert!(evaluate("(1 == 1 or 2 == 3) and 3 == 3", &vars).unwrap());
    assert!(!evaluate("(1 == 2 or 3 == 4) and 5 == 5", &vars).unwrap());
}

#[test]
fn test_evaluate_comparison_with_spaces() {
    let vars = HashMap::new();
    assert!(evaluate("  10   >   5  ", &vars).unwrap());
}

#[test]
fn test_evaluate_variable_missing() {
    let vars = HashMap::new();
    let result = evaluate("{{missing}} == test", &vars);
    assert!(result.is_ok());
}

#[test]
fn test_evaluate_zero_vs_one() {
    let vars = HashMap::new();
    assert!(!evaluate("0", &vars).unwrap());
    assert!(evaluate("1", &vars).unwrap());
}

#[test]
fn test_evaluate_yes_no() {
    let vars = HashMap::new();
    assert!(evaluate("yes", &vars).unwrap());
    assert!(!evaluate("no", &vars).unwrap());
}

#[test]
fn test_evaluate_gt_with_mixed_types() {
    let vars = HashMap::new();
    // One numeric, one string: string comparison
    let result = evaluate("10 > abc", &vars);
    // Since 10 is numeric but abc isn't, it falls back to lexicographic
    assert!(result.is_ok());
}

// ---------------------------------------------------------------------------
// W4a coverage gap closure (uncovered: resolve_variables break, split_logical
// quote arms, find_operator quote/depth/word-boundary guards, string-fallback
// lt/gte/lte, {{-truthy arm, find_matching_paren unbalanced)
// ---------------------------------------------------------------------------

#[test]
fn w4a_resolve_variables_unclosed_template_breaks_in_place() {
    let vars = HashMap::new();
    // No closing }} -> loop breaks, template text stays verbatim (line 48)
    assert_eq!(resolve_variables("{{unclosed", &vars), "{{unclosed");
    assert_eq!(resolve_variables("x == {{open", &vars), "x == {{open");
    // via evaluate: unresolved {{ stays, comparison falls back to string eq
    assert_eq!(evaluate("{{open == 1", &vars), Ok(false));
}

#[test]
fn w4a_resolve_variables_value_templates_are_not_rescanned() {
    // The `start` cursor advances past the inserted value, so a replacement
    // value that itself contains {{key}} is NOT resolved recursively.
    let mut vars = HashMap::new();
    vars.insert("a".to_string(), "{{b}}".to_string());
    vars.insert("b".to_string(), "done".to_string());
    assert_eq!(resolve_variables("{{a}}", &vars), "{{b}}");
}

#[test]
fn w4a_resolve_variables_skips_unknown_keys_forward() {
    let mut vars = HashMap::new();
    vars.insert("x".to_string(), "1".to_string());
    // Unknown key: cursor jumps past it (stays verbatim), later keys resolve
    assert_eq!(
        resolve_variables("{{miss}} == {{x}}", &vars),
        "{{miss}} == 1"
    );
}

#[test]
fn w4a_split_logical_skips_quoted_operator_and_parens() {
    // " or " inside double quotes is not a split point (quote arms 111-118)
    let parts = split_logical(r#""a or b" == "c or d""#, " or ");
    assert_eq!(parts.len(), 1);
    // split outside quotes still works
    let parts = split_logical(r#""a or b" == c or 1 == 1"#, " or ");
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0], r#""a or b" == c"#);
    assert_eq!(parts[1], "1 == 1");
    // " or " inside parens is not a split point (depth guard)
    let parts = split_logical("(a or b) and c", " or ");
    assert_eq!(parts.len(), 1);
}

#[test]
fn w4a_quoted_logical_operators_evaluate_as_strings() {
    let vars = HashMap::new();
    // The quoted " or " must survive splitting; both sides compare equal
    assert_eq!(evaluate(r#""a or b" == "a or b""#, &vars), Ok(true));
    assert_eq!(evaluate(r#""a or b" == "a b""#, &vars), Ok(false));
    // quoted side vs literal: quotes are part of the operand (no stripping)
    assert_eq!(evaluate(r#""a or b" == "a or b""#, &vars), Ok(true));
    assert_eq!(
        evaluate(r#""a or b" == a b"#, &vars),
        Ok(false) // left `"a or b"` != right `a b`
    );
    // top-level split on " or " with bare identifier part -> that part errors
    assert!(evaluate(r#""a or b" == a or b"#, &vars).is_err());
}

#[test]
fn w4a_find_operator_quote_and_depth_guards() {
    // == inside quotes not matched; top-level one found (208-209, 212)
    assert_eq!(find_operator(r#""a == b" == "c""#, "=="), Some(9));
    // == inside parens skipped at depth>0 (215, 217); top-level == found
    assert_eq!(find_operator("(1 == 1) == true", "=="), Some(9));
    // word operators need whitespace on both sides (227, 230)
    assert_eq!(find_operator("xcontains y", "contains"), None);
    assert_eq!(find_operator("a containsx", "contains"), None);
    assert_eq!(find_operator("a contains b", "contains"), Some(2));
}

#[test]
fn w4a_word_boundary_glued_operators_are_not_operators() {
    let vars = HashMap::new();
    // glued "contains" doesn't match any operator -> boolean fallback -> Err
    assert!(evaluate("xcontains y", &vars).is_err());
    assert!(evaluate("a containsx", &vars).is_err());
}

#[test]
fn w4a_comparison_string_fallbacks_lt_gte_lte() {
    let vars = HashMap::new();
    // Non-numeric operands fall back to lexicographic ordering (272/282/292)
    assert_eq!(evaluate("apple < banana", &vars), Ok(true));
    assert_eq!(evaluate("banana < apple", &vars), Ok(false));
    assert_eq!(evaluate("banana >= apple", &vars), Ok(true));
    assert_eq!(evaluate("apple >= banana", &vars), Ok(false));
    assert_eq!(evaluate("apple <= banana", &vars), Ok(true));
    assert_eq!(evaluate("banana <= apple", &vars), Ok(false));
    // equal non-numeric strings satisfy both >= and <=
    assert_eq!(evaluate("apple <= apple", &vars), Ok(true));
    assert_eq!(evaluate("apple >= apple", &vars), Ok(true));
}

#[test]
fn w4a_find_matching_paren_unbalanced_returns_max() {
    // unbalanced / unclosed parens (line 327)
    assert_eq!(find_matching_paren("(a == 1", 0), usize::MAX);
    assert_eq!(find_matching_paren("((a)", 0), usize::MAX);
    assert_eq!(find_matching_paren("no parens", 0), usize::MAX);
    // balanced
    assert_eq!(find_matching_paren("(a)", 0), 2);
    // unbalanced full expression errors out via boolean fallback
    assert!(evaluate("(a == 1", &HashMap::new()).is_err());
}

#[test]
fn w4a_unresolved_template_alone_is_truthy() {
    let vars = HashMap::new();
    // Unresolved {{...}} with no operator hits the {{-contains truthy arm (189)
    assert_eq!(evaluate("{{missing}}", &vars), Ok(true));
}

// ===========================================================================
// S12b batch：整段括号包裹表达式的递归臂（eval_comparison ~152 行：
// "( expr )" → eval_expression(inner)）。
// ===========================================================================

#[test]
fn s12b_fully_parenthesized_expression_recurses_into_inner() {
    let mut vars = HashMap::new();
    vars.insert("a".to_string(), "1".to_string());
    assert!(evaluate("( {{a}} == 1 )", &vars).unwrap());
    assert!(!evaluate("( {{a}} == 2 )", &vars).unwrap());
    // 嵌套括号包裹的完整表达式同样先剥壳再求值
    assert!(evaluate("(( true ))", &vars).unwrap());
}
