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
