use super::*;

#[test]
fn test_mock_registry_name() {
    let registry = MockRegistry::new("test-registry");
    assert_eq!(registry.name(), "test-registry");
}

#[test]
fn test_search_empty() {
    let registry = MockRegistry::new("test");
    let results = registry.search("query", 10);
    assert!(results.is_empty());
}

#[test]
fn test_search_with_results() {
    let mut registry = MockRegistry::new("test");
    registry.add_search_result(SkillSearchResult {
        score: 1.0,
        slug: "pdf-tool".to_string(),
        display_name: "PDF Tool".to_string(),
        summary: "Converts PDF files".to_string(),
        version: "1.0".to_string(),
        registry_name: "test".to_string(),
        source_repo: String::new(),
        download_path: String::new(),
        downloads: 0,
        truncated: false,
    });
    registry.add_search_result(SkillSearchResult {
        score: 0.8,
        slug: "csv-parser".to_string(),
        display_name: "CSV Parser".to_string(),
        summary: "Parses CSV data".to_string(),
        version: "2.0".to_string(),
        registry_name: "test".to_string(),
        source_repo: String::new(),
        download_path: String::new(),
        downloads: 0,
        truncated: false,
    });

    let results = registry.search("pdf", 10);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].slug, "pdf-tool");
}

#[test]
fn test_search_empty_query_returns_all() {
    let mut registry = MockRegistry::new("test");
    registry.add_search_result(SkillSearchResult {
        score: 1.0,
        slug: "a".to_string(),
        display_name: "A".to_string(),
        summary: "Skill A".to_string(),
        version: "1.0".to_string(),
        registry_name: "test".to_string(),
        source_repo: String::new(),
        download_path: String::new(),
        downloads: 0,
        truncated: false,
    });
    registry.add_search_result(SkillSearchResult {
        score: 0.8,
        slug: "b".to_string(),
        display_name: "B".to_string(),
        summary: "Skill B".to_string(),
        version: "1.0".to_string(),
        registry_name: "test".to_string(),
        source_repo: String::new(),
        download_path: String::new(),
        downloads: 0,
        truncated: false,
    });

    let results = registry.search("", 10);
    assert_eq!(results.len(), 2);
}

#[test]
fn test_search_respects_limit() {
    let mut registry = MockRegistry::new("test");
    for i in 0..10 {
        registry.add_search_result(SkillSearchResult {
            score: 1.0,
            slug: format!("skill-{}", i),
            display_name: format!("Skill {}", i),
            summary: "Test".to_string(),
            version: "1.0".to_string(),
            registry_name: "test".to_string(),
            source_repo: String::new(),
            download_path: String::new(),
            downloads: 0,
            truncated: false,
        });
    }

    let results = registry.search("", 3);
    assert_eq!(results.len(), 3);
}

#[test]
fn test_get_skill_meta_found() {
    let mut registry = MockRegistry::new("test");
    registry.set_skill_meta(
        "pdf-tool",
        SkillMeta {
            slug: "pdf-tool".to_string(),
            display_name: "PDF Tool".to_string(),
            summary: "Converts PDFs".to_string(),
            latest_version: "2.0".to_string(),
            is_malware_blocked: false,
            is_suspicious: false,
            registry_name: "test".to_string(),
            author: String::new(),
            downloads: 0,
        },
    );

    let meta = registry.get_skill_meta("pdf-tool");
    assert_eq!(meta.display_name, "PDF Tool");
    assert_eq!(meta.latest_version, "2.0");
}

#[test]
fn test_get_skill_meta_not_found_returns_default() {
    let registry = MockRegistry::new("test");
    let meta = registry.get_skill_meta("nonexistent");
    assert_eq!(meta.slug, "nonexistent");
}

#[test]
fn test_download_and_install_normal() {
    let registry = MockRegistry::new("test");
    let result = registry.download_and_install("skill", "1.0", "/tmp/skill");
    assert!(!result.is_malware_blocked);
    assert!(!result.is_suspicious);
    assert_eq!(result.version, "1.0");
}

#[test]
fn test_download_and_install_malware_blocked() {
    let mut registry = MockRegistry::new("test");
    registry.set_skill_meta(
        "malware",
        SkillMeta {
            slug: "malware".to_string(),
            display_name: "Malware".to_string(),
            summary: "Bad skill".to_string(),
            latest_version: "1.0".to_string(),
            is_malware_blocked: true,
            is_suspicious: true,
            registry_name: "test".to_string(),
            author: String::new(),
            downloads: 0,
        },
    );

    let result = registry.download_and_install("malware", "1.0", "/tmp/malware");
    assert!(result.is_malware_blocked);
    assert!(result.is_suspicious);
}

// ============================================================
// Additional mock_registry tests for missing coverage
// ============================================================

#[test]
fn test_search_case_insensitive() {
    let mut registry = MockRegistry::new("test");
    registry.add_search_result(SkillSearchResult {
        score: 1.0,
        slug: "PDF-Tool".to_string(),
        display_name: "PDF Tool".to_string(),
        summary: "Converts PDF files".to_string(),
        version: "1.0".to_string(),
        registry_name: "test".to_string(),
        source_repo: String::new(),
        download_path: String::new(),
        downloads: 0,
        truncated: false,
    });

    let results = registry.search("pdf", 10);
    assert_eq!(results.len(), 1);

    let results_upper = registry.search("PDF", 10);
    assert_eq!(results_upper.len(), 1);
}

#[test]
fn test_search_matches_summary() {
    let mut registry = MockRegistry::new("test");
    registry.add_search_result(SkillSearchResult {
        score: 1.0,
        slug: "tool-a".to_string(),
        display_name: "Tool A".to_string(),
        summary: "A great CSV parser".to_string(),
        version: "1.0".to_string(),
        registry_name: "test".to_string(),
        source_repo: String::new(),
        download_path: String::new(),
        downloads: 0,
        truncated: false,
    });

    let results = registry.search("csv", 10);
    assert_eq!(results.len(), 1);
}

#[test]
fn test_search_no_limit_when_zero() {
    let mut registry = MockRegistry::new("test");
    for i in 0..10 {
        registry.add_search_result(SkillSearchResult {
            score: 1.0,
            slug: format!("skill-{}", i),
            display_name: format!("Skill {}", i),
            summary: "Test".to_string(),
            version: "1.0".to_string(),
            registry_name: "test".to_string(),
            source_repo: String::new(),
            download_path: String::new(),
            downloads: 0,
            truncated: false,
        });
    }

    let results = registry.search("", 0);
    assert_eq!(results.len(), 10);
}

#[test]
fn test_get_skill_meta_default_values() {
    let registry = MockRegistry::new("my-registry");
    let meta = registry.get_skill_meta("unknown-skill");
    assert_eq!(meta.slug, "unknown-skill");
    assert_eq!(meta.display_name, "unknown-skill");
    assert_eq!(meta.summary, "Mock skill");
    assert_eq!(meta.latest_version, "latest");
    assert!(!meta.is_malware_blocked);
    assert!(!meta.is_suspicious);
    assert_eq!(meta.registry_name, "my-registry");
}

#[test]
fn test_download_and_install_not_malware_meta() {
    let mut registry = MockRegistry::new("test");
    registry.set_skill_meta(
        "safe-skill",
        SkillMeta {
            slug: "safe-skill".to_string(),
            display_name: "Safe Skill".to_string(),
            summary: "A safe skill".to_string(),
            latest_version: "2.0".to_string(),
            is_malware_blocked: false,
            is_suspicious: false,
            registry_name: "test".to_string(),
            author: String::new(),
            downloads: 0,
        },
    );

    let result = registry.download_and_install("safe-skill", "2.0", "/tmp/safe");
    assert!(!result.is_malware_blocked);
    assert!(!result.is_suspicious);
    assert_eq!(result.version, "2.0");
}

#[test]
fn test_download_and_install_unknown_skill() {
    let registry = MockRegistry::new("test");
    let result = registry.download_and_install("unknown-skill", "1.0", "/tmp/unknown");
    assert!(!result.is_malware_blocked);
    assert_eq!(result.version, "1.0");
    assert_eq!(result.summary, "Mock installation");
}

#[test]
fn test_add_multiple_search_results() {
    let mut registry = MockRegistry::new("test");
    for i in 0..5 {
        registry.add_search_result(SkillSearchResult {
            score: 1.0 - i as f64 * 0.1,
            slug: format!("skill-{}", i),
            display_name: format!("Skill {}", i),
            summary: format!("Skill number {}", i),
            version: "1.0".to_string(),
            registry_name: "test".to_string(),
            source_repo: String::new(),
            download_path: String::new(),
            downloads: 0,
            truncated: false,
        });
    }

    // Search for "skill" which matches all slugs
    let results = registry.search("skill", 10);
    assert_eq!(results.len(), 5);
}

#[test]
fn test_search_no_match() {
    let mut registry = MockRegistry::new("test");
    registry.add_search_result(SkillSearchResult {
        score: 1.0,
        slug: "pdf-tool".to_string(),
        display_name: "PDF Tool".to_string(),
        summary: "PDF converter".to_string(),
        version: "1.0".to_string(),
        registry_name: "test".to_string(),
        source_repo: String::new(),
        download_path: String::new(),
        downloads: 0,
        truncated: false,
    });

    let results = registry.search("weather", 10);
    assert!(results.is_empty());
}
