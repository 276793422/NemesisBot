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
mod tests;
