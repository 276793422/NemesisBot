//! Tool-argument validation against a tool's JSON-Schema `parameters()`.
//!
//! Part of the small-model-tool-robustness plan (Phase 2). Catches B-class
//! failures — wrong field name, wrong type, missing required field, bad enum
//! value — *before* dispatch, so the model gets a structured, actionable error
//! it can self-correct from on the next round instead of a confusing runtime
//! failure. When a field name is a high-confidence typo of a real schema field
//! (edit distance ≤ 2, unambiguous), it is auto-corrected rather than bounced.
//!
//! Industry-standard pattern: OpenAI Function Calling cookbook, Anthropic
//! tool_use docs, LangChain `ToolValidation`, Instructor/DSPy retry loops.
//!
//! Scope is deliberately narrow (no `oneOf`/`allOf`/deep-nested `$ref`):
//! it is fail-open — anything it cannot confidently assess is allowed through,
//! so it never blocks a legitimate call.

use serde_json::Value;

/// Outcome of checking one tool call's arguments against its schema.
#[derive(Debug, Clone)]
pub enum Outcome {
    /// Args conform to the schema (or the tool declares no object schema).
    Valid,
    /// Args had near-miss field names that were all confidently remapped.
    /// Carries the fixed arguments as a JSON string to feed to the tool.
    Fixed(String),
    /// Args violate the schema. `class` is the failure class for telemetry:
    /// `"A"` = args are not valid JSON, `"B"` = schema violation.
    Invalid {
        message: String,
        class: &'static str,
    },
}

/// A single schema violation, with enough structure to render a precise hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Violation {
    UnknownField {
        field: String,
        suggestion: Option<String>,
    },
    MissingRequired {
        field: String,
    },
    WrongType {
        field: String,
        expected: String,
        got: String,
    },
    NotInEnum {
        field: String,
        allowed: Vec<String>,
    },
}

impl Violation {
    pub fn message(&self) -> String {
        match self {
            Violation::UnknownField { field, suggestion } => match suggestion {
                Some(s) => format!("unknown field '{}' (did you mean '{}'?)", field, s),
                None => format!("unknown field '{}'", field),
            },
            Violation::MissingRequired { field } => {
                format!("missing required field '{}'", field)
            }
            Violation::WrongType {
                field,
                expected,
                got,
            } => format!("field '{}' should be {}, got {}", field, expected, got),
            Violation::NotInEnum { field, allowed } => format!(
                "field '{}' must be one of: {}",
                field,
                allowed.join(" | ")
            ),
        }
    }
}

/// Check `args_json` (the raw JSON string from the model) against `schema`
/// (the tool's `parameters()`). This is the main entry point used by the
/// agent loop before dispatching a tool call.
pub fn check(schema: &Value, args_json: &str) -> Outcome {
    let args: Value = match serde_json::from_str(args_json) {
        Ok(v) => v,
        Err(e) => {
            return Outcome::Invalid {
                message: format!(
                    "Arguments are not valid JSON ({}). Please resend valid JSON arguments.",
                    e
                ),
                class: "A",
            }
        }
    };

    let violations = validate(schema, &args);
    if violations.is_empty() {
        return Outcome::Valid;
    }

    // Try to auto-fix near-miss field names; accept only if the result is clean.
    if let Some(fixed) = try_autofix(schema, &args) {
        if validate(schema, &fixed).is_empty() {
            if let Ok(s) = serde_json::to_string(&fixed) {
                return Outcome::Fixed(s);
            }
        }
    }

    Outcome::Invalid {
        message: format_violations(schema, &violations),
        class: "B",
    }
}

/// Validate `args` against `schema`. Empty result = valid.
pub fn validate(schema: &Value, args: &Value) -> Vec<Violation> {
    let mut out = Vec::new();

    // No `properties` (or empty) ⇒ the tool declares no object schema to
    // enforce. The default `Tool::parameters()` returns `{"type":"object",
    // "properties":{}}`, so this is the common "undeclared schema" case — fail
    // open rather than rejecting every field as unknown.
    let props = match schema.get("properties").and_then(|p| p.as_object()) {
        Some(p) if !p.is_empty() => p,
        _ => return out,
    };

    let required: Vec<&str> = schema
        .get("required")
        .and_then(|r| r.as_array())
        .map(|arr| arr.iter().filter_map(|x| x.as_str()).collect())
        .unwrap_or_default();

    let args_obj = match args.as_object() {
        Some(o) => o,
        None => {
            if schema.get("type").and_then(|t| t.as_str()) == Some("object") {
                out.push(Violation::WrongType {
                    field: "<root>".to_string(),
                    expected: "object".to_string(),
                    got: type_name(args).to_string(),
                });
            }
            return out;
        }
    };

    for (key, val) in args_obj {
        match props.get(key) {
            None => {
                // Only flag as a likely-typo violation if a real field is within
                // autofix range (≤2) so `check()` can fix it. Truly extra fields
                // (no close neighbour) are silently ignored — tools skip
                // undeclared keys anyway, and bouncing on them would
                // false-positive on extra context that models sometimes add
                // (e.g. an "encoding" field on a read_file call). This matches
                // the JSON Schema default (additionalProperties: true); a strong
                // model passing a helpful-but-undeclared field is no longer
                // penalised.
                if let Some(suggestion) = nearest_field(props.keys(), key, 2) {
                    out.push(Violation::UnknownField {
                        field: key.clone(),
                        suggestion: Some(suggestion),
                    });
                }
            }
            Some(prop_schema) => {
                if let Some(exp) = prop_schema.get("type").and_then(|t| t.as_str()) {
                    if !type_matches(exp, val) {
                        out.push(Violation::WrongType {
                            field: key.clone(),
                            expected: exp.to_string(),
                            got: type_name(val).to_string(),
                        });
                    }
                }
                if let Some(allowed) = prop_schema.get("enum").and_then(|e| e.as_array()) {
                    if !allowed.iter().any(|a| a == val) {
                        let allowed_str: Vec<String> =
                            allowed.iter().filter_map(|a| a.as_str().map(String::from)).collect();
                        out.push(Violation::NotInEnum {
                            field: key.clone(),
                            allowed: allowed_str,
                        });
                    }
                }
            }
        }
    }

    for req in required {
        if !args_obj.contains_key(req) {
            out.push(Violation::MissingRequired {
                field: req.to_string(),
            });
        }
    }

    out
}

/// Attempt to auto-fix near-miss field names. Returns fixed args only if EVERY
/// unknown field maps unambiguously to exactly one schema property within edit
/// distance 2 (and no two unknowns collapse onto the same target). Otherwise
/// returns `None` and the caller falls back to bouncing a structured error.
pub fn try_autofix(schema: &Value, args: &Value) -> Option<Value> {
    let props = schema.get("properties")?.as_object()?;
    let args_obj = args.as_object()?;

    let mut rename: Vec<(String, String)> = Vec::new();
    let mut used_targets: std::collections::HashSet<String> = std::collections::HashSet::new();

    for (key, _val) in args_obj {
        if props.contains_key(key) {
            continue;
        }
        // Ranked candidates within distance 2.
        let mut candidates: Vec<(&String, usize)> = props
            .keys()
            .map(|p| (p, edit_distance(p.as_str(), key.as_str())))
            .filter(|(_, d)| *d <= 2)
            .collect();
        candidates.sort_by_key(|(_, d)| *d);

        if candidates.is_empty() {
            return None;
        }
        let min_d = candidates[0].1;
        let tied: usize = candidates.iter().filter(|(_, d)| *d == min_d).count();
        if tied > 1 {
            return None; // ambiguous — don't guess
        }
        let target = candidates[0].0;
        if used_targets.contains(target) {
            return None; // two unknowns would collide on the same field
        }
        used_targets.insert(target.clone());
        rename.push((key.clone(), target.clone()));
    }

    if rename.is_empty() {
        return None;
    }

    let mut fixed = serde_json::Map::new();
    for (key, val) in args_obj {
        let new_key = rename
            .iter()
            .find(|(from, _)| from == key)
            .map(|(_, to)| to.clone())
            .unwrap_or_else(|| key.clone());
        fixed.insert(new_key, val.clone());
    }
    Some(Value::Object(fixed))
}

/// Levenshtein edit distance over Unicode chars.
pub fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (n, m) = (a.len(), b.len());
    if n == 0 {
        return m;
    }
    if m == 0 {
        return n;
    }
    let mut prev: Vec<usize> = (0..=m).collect();
    let mut cur: Vec<usize> = vec![0; m + 1];
    for i in 1..=n {
        cur[0] = i;
        for j in 1..=m {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[m]
}

/// Find the closest property name to `field` within `max_dist`, case-insensitive.
fn nearest_field<'a, I>(names: I, field: &str, max_dist: usize) -> Option<String>
where
    I: Iterator<Item = &'a String>,
{
    let lower = field.to_lowercase();
    let mut best: Option<(&String, usize)> = None;
    for n in names {
        let d = edit_distance(&n.to_lowercase(), &lower);
        if d <= max_dist && best.map_or(true, |(_, bd)| d < bd) {
            best = Some((n, d));
        }
    }
    best.map(|(n, _)| n.clone())
}

fn type_matches(expected: &str, val: &Value) -> bool {
    match expected {
        "string" => val.is_string(),
        "integer" => val.is_i64() || val.is_u64(),
        "number" => val.is_number(),
        "boolean" => val.is_boolean(),
        "array" => val.is_array(),
        "object" => val.is_object(),
        "null" => val.is_null(),
        _ => true, // unknown type spec — fail open
    }
}

fn type_name(val: &Value) -> &'static str {
    match val {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn format_violations(schema: &Value, vs: &[Violation]) -> String {
    let mut parts: Vec<String> = vs.iter().map(|v| v.message()).collect();
    parts.sort();
    let mut msg = format!(
        "Tool argument validation failed: {}. ",
        parts.join("; ")
    );
    // Append the legal field list once if there's at least one UnknownField, so
    // the model can see exactly what keys are accepted.
    if vs.iter().any(|v| matches!(v, Violation::UnknownField { .. })) {
        if let Some(props) = schema.get("properties").and_then(|p| p.as_object()) {
            let names: Vec<&str> = props.keys().map(|s| s.as_str()).collect();
            if !names.is_empty() {
                msg.push_str(&format!("Accepted fields: {}.", names.join(", ")));
            }
        }
    }
    msg
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "content": {"type": "string"},
                "timeout": {"type": "integer"},
                "action": {"type": "string", "enum": ["create", "delete", "list"]}
            },
            "required": ["path"]
        })
    }

    #[test]
    fn valid_args_pass() {
        let s = sample_schema();
        assert!(matches!(check(&s, r#"{"path":"a.txt"}"#), Outcome::Valid));
        assert!(matches!(
            check(&s, r#"{"path":"a.txt","timeout":30}"#),
            Outcome::Valid
        ));
    }

    #[test]
    fn missing_required_is_invalid() {
        let s = sample_schema();
        match check(&s, r#"{"content":"hi"}"#) {
            Outcome::Invalid { message, class } => {
                assert_eq!(class, "B");
                assert!(message.contains("required"), "{}", message);
                assert!(message.contains("path"), "{}", message);
            }
            other => panic!("expected Invalid, got {:?}", other),
        }
    }

    #[test]
    fn wrong_type_is_invalid() {
        let s = sample_schema();
        match check(&s, r#"{"path":123}"#) {
            Outcome::Invalid { message, class } => {
                assert_eq!(class, "B");
                assert!(message.contains("path"), "{}", message);
            }
            other => panic!("expected Invalid, got {:?}", other),
        }
    }

    #[test]
    fn bad_enum_is_invalid() {
        let s = sample_schema();
        match check(&s, r#"{"path":"a","action":"nope"}"#) {
            Outcome::Invalid { message, class } => {
                assert_eq!(class, "B");
                assert!(message.contains("one of"), "{}", message);
                assert!(message.contains("create"), "{}", message);
            }
            other => panic!("expected Invalid, got {:?}", other),
        }
    }

    #[test]
    fn unknown_field_typo_is_autofixed() {
        // "patch" is edit-distance 1 from "path" — should autofix.
        let s = sample_schema();
        match check(&s, r#"{"patch":"a.txt"}"#) {
            Outcome::Fixed(fixed) => {
                let v: Value = serde_json::from_str(&fixed).unwrap();
                assert_eq!(v["path"], "a.txt");
                assert!(v.get("patch").is_none());
            }
            other => panic!("expected Fixed, got {:?}", other),
        }
    }

    #[test]
    fn extra_field_with_no_close_neighbor_is_ignored() {
        // A clearly-extra field (no near-miss with a real field) is IGNORED —
        // tools skip undeclared keys, and bouncing would false-positive on
        // helpful extras that strong models sometimes add (e.g. "encoding",
        // "verbose"). The valid field still executes normally. The validator is
        // now lenient about extras (JSON Schema default), while still catching
        // typos (unknown_field_typo_is_autofixed) and unambiguous errors
        // (missing required / wrong type / bad enum).
        let s = sample_schema();
        assert!(matches!(check(&s, r#"{"path":"a","zzzzzz":"x"}"#), Outcome::Valid));
        assert!(matches!(check(&s, r#"{"path":"a","encoding":"utf-8"}"#), Outcome::Valid));
    }

    #[test]
    fn invalid_json_is_class_a() {
        let s = sample_schema();
        match check(&s, r#"{"path":"a", broken}"#) {
            Outcome::Invalid { message, class } => {
                assert_eq!(class, "A");
                assert!(message.contains("not valid JSON"), "{}", message);
            }
            other => panic!("expected Invalid, got {:?}", other),
        }
    }

    #[test]
    fn ambiguous_typo_not_autofixed() {
        // Schema with two fields equally close to the typo. "xat" is distance 1
        // from both "bat" and "cat" → ambiguous → must bounce, not guess.
        let s = json!({
            "type": "object",
            "properties": {
                "bat": {"type": "string"},
                "cat": {"type": "string"},
                "content": {"type": "string"}
            },
            "required": []
        });
        match check(&s, r#"{"xat":"a"}"#) {
            Outcome::Invalid { .. } => {}
            other => panic!("expected Invalid (ambiguous), got {:?}", other),
        }
    }

    #[test]
    fn no_schema_fails_open() {
        let s = json!({"type": "object"});
        assert!(matches!(
            check(&s, r#"{"anything": 1, "else": "x"}"#),
            Outcome::Valid
        ));
    }

    #[test]
    fn non_object_args_with_object_schema() {
        let s = sample_schema();
        match check(&s, r#"[1,2,3]"#) {
            Outcome::Invalid { message, class } => {
                assert_eq!(class, "B");
                assert!(message.contains("object"), "{}", message);
            }
            other => panic!("expected Invalid, got {:?}", other),
        }
    }

    #[test]
    fn edit_distance_basic() {
        assert_eq!(edit_distance("", ""), 0);
        assert_eq!(edit_distance("abc", "abc"), 0);
        assert_eq!(edit_distance("path", "patch"), 1); // insert
        assert_eq!(edit_distance("patch", "path"), 1); // delete
        assert_eq!(edit_distance("cat", "cut"), 1); // substitute
        assert_eq!(edit_distance("path", "content"), 6);
    }

    #[test]
    fn multiple_violations_all_reported() {
        let s = sample_schema();
        // missing path (required) + bad action enum + unknown field
        match check(&s, r#"{"action":"foo","wat":"x"}"#) {
            Outcome::Invalid { message, .. } => {
                assert!(message.contains("required"), "{}", message);
                assert!(message.contains("one of"), "{}", message);
                assert!(message.contains("unknown"), "{}", message);
            }
            other => panic!("expected Invalid, got {:?}", other),
        }
    }
}
