use super::*;

#[test]
fn test_url_encode_component_unreserved_passthrough() {
    // Unreserved set (RFC 3986) passes through unchanged.
    assert_eq!(url_encode_component("PantherAng"), "PantherAng");
    assert_eq!(url_encode_component("zhima_credit_tech"), "zhima_credit_tech");
    assert_eq!(url_encode_component("a.b-c_d~e"), "a.b-c_d~e");
}

#[test]
fn test_url_encode_component_encodes_at_and_reserved() {
    // ModelScope namespaces use a leading '@'; reserved chars get percent-encoded.
    assert_eq!(url_encode_component("@anthropics"), "%40anthropics");
    assert_eq!(url_encode_component("a b/c"), "a%20b%2Fc");
}

#[test]
fn test_url_encode_component_non_ascii_is_utf8_bytes() {
    // Non-ASCII is encoded as UTF-8 bytes (支 = E6 94 AF).
    assert_eq!(url_encode_component("支"), "%E6%94%AF");
}

#[test]
fn test_parse_github_tree_url_valid() {
    let (o, r, b, p) = parse_github_tree_url(
        "https://github.com/duclm1x1/dive-ai/tree/main/skills_library/web/mingli",
    )
    .unwrap();
    assert_eq!(o, "duclm1x1");
    assert_eq!(r, "dive-ai");
    assert_eq!(b, "main");
    assert_eq!(p, "skills_library/web/mingli");
}

#[test]
fn test_parse_github_tree_url_rejects_non_tree_shapes() {
    // bare repo root, blob URL, non-github host, tree without a path
    assert!(parse_github_tree_url("https://github.com/o/repo").is_none());
    assert!(parse_github_tree_url("https://github.com/o/repo/blob/main/x").is_none());
    assert!(parse_github_tree_url("https://gitlab.com/o/r/tree/main/x").is_none());
    assert!(parse_github_tree_url("https://github.com/o/r/tree/main").is_none());
}
