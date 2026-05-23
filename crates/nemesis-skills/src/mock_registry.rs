//! Mock registry for testing.
//!
//! Provides a controllable mock implementation of the SkillRegistry trait
//! for use in unit tests.

use crate::types::{InstallResult, SkillMeta, SkillSearchResult};

/// Mock registry for testing skill operations.
pub struct MockRegistry {
    name: String,
    search_results: Vec<SkillSearchResult>,
    skill_meta: std::collections::HashMap<String, SkillMeta>,
}

impl MockRegistry {
    /// Create a new mock registry with the given name.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            search_results: Vec::new(),
            skill_meta: std::collections::HashMap::new(),
        }
    }

    /// Add a search result to the mock.
    pub fn add_search_result(&mut self, result: SkillSearchResult) {
        self.search_results.push(result);
    }

    /// Set metadata for a specific skill slug.
    pub fn set_skill_meta(&mut self, slug: &str, meta: SkillMeta) {
        self.skill_meta.insert(slug.to_string(), meta);
    }

    /// Get the registry name.
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl MockRegistry {
    /// Search the mock registry.
    pub fn search(&self, query: &str, limit: usize) -> Vec<SkillSearchResult> {
        let query_lower = query.to_lowercase();
        let mut results = Vec::new();

        for result in &self.search_results {
            if limit > 0 && results.len() >= limit {
                break;
            }

            if query.is_empty()
                || result.slug.to_lowercase().contains(&query_lower)
                || result.summary.to_lowercase().contains(&query_lower)
            {
                results.push(result.clone());
            }
        }

        results
    }

    /// Get skill metadata by slug.
    pub fn get_skill_meta(&self, slug: &str) -> SkillMeta {
        self.skill_meta
            .get(slug)
            .cloned()
            .unwrap_or(SkillMeta {
                slug: slug.to_string(),
                display_name: slug.to_string(),
                summary: "Mock skill".to_string(),
                latest_version: "latest".to_string(),
                is_malware_blocked: false,
                is_suspicious: false,
                registry_name: self.name.clone(),
            })
    }

    /// Simulate downloading and installing a skill.
    pub fn download_and_install(
        &self,
        slug: &str,
        version: &str,
        _target_dir: &str,
    ) -> InstallResult {
        if let Some(meta) = self.skill_meta.get(slug) {
            if meta.is_malware_blocked {
                return InstallResult {
                    version: version.to_string(),
                    is_malware_blocked: true,
                    is_suspicious: meta.is_suspicious,
                    summary: meta.summary.clone(),
                };
            }
        }

        InstallResult {
            version: version.to_string(),
            is_malware_blocked: false,
            is_suspicious: false,
            summary: "Mock installation".to_string(),
        }
    }
}

#[cfg(test)]
mod tests;
