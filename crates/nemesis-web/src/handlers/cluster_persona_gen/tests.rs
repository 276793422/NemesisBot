use super::*;

#[test]
fn sanitize_rejects_short_and_strips_controls() {
    assert!(sanitize_input("太短").is_err());
    let r = sanitize_input(
            "这是一段足够长的有效岗位描述文本，用于通过最小长度校验门槛，这里再多写一些内容确保超过四十个字符 abcdefgh",
        )
        .unwrap();
    assert!(!r.contains('\u{7f}'));
    assert!(!r.contains('\r'));
}

#[test]
fn validate_enforces_role_enum() {
    let mut pkg = PersonaPackage {
        node_name: "x".into(),
        display_name: "X".into(),
        emoji: "🤖".into(),
        role: "admin".into(),
        category: "dev".into(),
        tags: vec![" a ".into()],
        identity_md: "# X\nwho".into(),
        soul_md: "# Rules\n- a".into(),
    };
    assert!(validate(&mut pkg).is_err());
    pkg.role = "worker".into();
    assert!(validate(&mut pkg).is_ok());
    assert_eq!(pkg.tags, vec!["a".to_string()]);
}

#[test]
fn unwrap_single_key_handles_wrapped_args() {
    let wrapped =
        serde_json::json!({ "emit_cluster_persona": { "identity_md": "# x", "soul_md": "# y" } });
    let v = unwrap_single_key(wrapped);
    assert!(v.get("identity_md").is_some());
}

#[test]
fn extract_json_span_and_fence() {
    assert_eq!(extract_json_span("noise {\"a\":1} tail"), Some("{\"a\":1}"));
    let stripped = strip_code_fence("```json\n{\"a\":1}\n```");
    assert_eq!(stripped, "{\"a\":1}");
}
