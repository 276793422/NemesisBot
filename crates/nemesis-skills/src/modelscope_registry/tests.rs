use super::*;

#[test]
fn test_source_url_to_raw() {
    let url = "https://github.com/majiayu000/claude-skill-registry/tree/main/skills/data/7-debug";
    let raw = ModelScopeRegistry::source_url_to_raw(url).unwrap();
    assert_eq!(
        raw,
        "https://raw.githubusercontent.com/majiayu000/claude-skill-registry/main/skills/data/7-debug/SKILL.md"
    );
}

#[test]
fn test_source_url_to_raw_simple() {
    let url = "https://github.com/anthropics/claude-plugins-official/tree/main/plugins/skill-creator/skills/skill-creator";
    let raw = ModelScopeRegistry::source_url_to_raw(url).unwrap();
    assert_eq!(
        raw,
        "https://raw.githubusercontent.com/anthropics/claude-plugins-official/main/plugins/skill-creator/skills/skill-creator/SKILL.md"
    );
}

#[test]
fn test_source_url_invalid() {
    assert!(ModelScopeRegistry::source_url_to_raw("https://example.com/foo").is_none());
    assert!(ModelScopeRegistry::source_url_to_raw("https://github.com/owner/repo").is_none());
}
