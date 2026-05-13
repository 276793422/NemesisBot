//! Skills utility functions.

/// Extract a skill name from a skill ID (registry/slug format).
pub fn extract_slug(skill_id: &str) -> &str {
    if let Some(idx) = skill_id.rfind('/') {
        &skill_id[idx + 1..]
    } else {
        skill_id
    }
}

/// Normalize a skill name for comparison.
pub fn normalize_skill_name(name: &str) -> String {
    name.to_lowercase()
        .replace(' ', "-")
        .replace('_', "-")
        .trim_matches('-')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_slug() {
        assert_eq!(extract_slug("anthropics/skills/coder"), "coder");
        assert_eq!(extract_slug("standalone"), "standalone");
    }

    #[test]
    fn test_normalize() {
        assert_eq!(normalize_skill_name("My Skill"), "my-skill");
        assert_eq!(normalize_skill_name("my_skill"), "my-skill");
    }

    // ============================================================
    // Additional tests for missing coverage
    // ============================================================

    #[test]
    fn test_extract_slug_two_level() {
        assert_eq!(extract_slug("author/category/my-skill"), "my-skill");
    }

    #[test]
    fn test_extract_slug_single_level() {
        assert_eq!(extract_slug("my-skill"), "my-skill");
    }

    #[test]
    fn test_extract_slug_empty_string() {
        assert_eq!(extract_slug(""), "");
    }

    #[test]
    fn test_extract_slug_trailing_slash() {
        assert_eq!(extract_slug("author/"), "");
    }

    #[test]
    fn test_normalize_mixed_case() {
        assert_eq!(normalize_skill_name("MySkill"), "myskill");
    }

    #[test]
    fn test_normalize_spaces_and_underscores() {
        assert_eq!(normalize_skill_name("my cool_skill"), "my-cool-skill");
    }

    #[test]
    fn test_normalize_leading_trailing_hyphens() {
        assert_eq!(normalize_skill_name("-my-skill-"), "my-skill");
    }

    #[test]
    fn test_normalize_empty_string() {
        assert_eq!(normalize_skill_name(""), "");
    }

    #[test]
    fn test_normalize_only_separators() {
        assert_eq!(normalize_skill_name("___"), "");
        assert_eq!(normalize_skill_name("   "), "");
        assert_eq!(normalize_skill_name("---"), "");
    }

    #[test]
    fn test_extract_slug_with_registry_prefix() {
        assert_eq!(extract_slug("anthropics/skills"), "skills");
    }

    #[test]
    fn test_extract_slug_multiple_slashes() {
        assert_eq!(extract_slug("a/b/c/d"), "d");
    }

    #[test]
    fn test_extract_slug_single_char() {
        assert_eq!(extract_slug("x"), "x");
    }

    #[test]
    fn test_extract_slug_slash_only() {
        assert_eq!(extract_slug("/"), "");
    }

    #[test]
    fn test_extract_slug_double_slash() {
        assert_eq!(extract_slug("a//b"), "b");
    }

    #[test]
    fn test_normalize_mixed_separators() {
        assert_eq!(normalize_skill_name("My_Cool Skill"), "my-cool-skill");
    }

    #[test]
    fn test_normalize_already_normalized() {
        assert_eq!(normalize_skill_name("my-skill"), "my-skill");
    }

    #[test]
    fn test_normalize_with_numbers() {
        assert_eq!(normalize_skill_name("Skill V2"), "skill-v2");
    }

    #[test]
    fn test_normalize_single_char() {
        assert_eq!(normalize_skill_name("a"), "a");
    }

    #[test]
    fn test_normalize_multiple_hyphens_collapsed() {
        // Spaces become hyphens, underscores become hyphens, leading/trailing stripped
        assert_eq!(normalize_skill_name("_my skill_"), "my-skill");
    }

    #[test]
    fn test_normalize_consecutive_separators() {
        // Consecutive separators are not collapsed by normalize_skill_name
        assert_eq!(normalize_skill_name("a__b  c"), "a--b--c");
        // Single separator works
        assert_eq!(normalize_skill_name("a_b c"), "a-b-c");
    }
}
