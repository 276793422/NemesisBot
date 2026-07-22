use super::*;
use nemesis_web::StaticFiles;

#[test]
fn test_embedded_static_exists() {
    // The macro should have embedded the static directory
    assert!(EMBEDDED_STATIC.contains("index.html"));
}

#[test]
fn test_get_embedded_file() {
    let content = get_embedded_file("index.html");
    assert!(content.is_some());
    let html = std::str::from_utf8(content.unwrap()).unwrap();
    assert!(html.contains("<!DOCTYPE html>") || html.contains("<html"));
}

#[test]
fn test_list_embedded_files() {
    let files = list_embedded_files();
    assert!(!files.is_empty());
    assert!(files.iter().any(|f| f.contains("index.html")));
    assert!(files.iter().any(|f| f.contains("assets/")));
}

#[test]
fn test_extract_dir() {
    let temp = tempfile::tempdir().unwrap();
    extract_dir(&EMBEDDED_STATIC, temp.path()).unwrap();

    assert!(temp.path().join("index.html").exists());
    assert!(temp.path().join("assets").is_dir());
    assert!(temp.path().join("fonts").is_dir());

    // Verify assets contain JS and CSS bundles
    let assets_dir = temp.path().join("assets");
    let js_files: Vec<_> = std::fs::read_dir(&assets_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|ext| ext == "js").unwrap_or(false))
        .collect();
    let css_files: Vec<_> = std::fs::read_dir(&assets_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map(|ext| ext == "css")
                .unwrap_or(false)
        })
        .collect();
    assert!(!js_files.is_empty(), "assets/ should contain JS bundles");
    assert!(!css_files.is_empty(), "assets/ should contain CSS bundles");
}

// ============================================================
// Additional tests for coverage improvement
// ============================================================

#[test]
fn test_get_embedded_file_nonexistent() {
    let content = get_embedded_file("nonexistent_file_12345.html");
    assert!(content.is_none());
}

#[test]
fn test_get_embedded_file_css() {
    // Vue build puts CSS into assets/ directory
    let files = list_embedded_files();
    assert!(
        files
            .iter()
            .any(|f| f.contains("assets/") && f.ends_with(".css"))
    );
}

#[test]
fn test_get_embedded_file_js() {
    // Vue build puts JS into assets/ directory
    let files = list_embedded_files();
    assert!(
        files
            .iter()
            .any(|f| f.contains("assets/") && f.ends_with(".js"))
    );
}

#[test]
fn test_list_embedded_files_contains_known_paths() {
    let files = list_embedded_files();
    assert!(
        files
            .iter()
            .any(|f| f == "index.html" || f.contains("index.html"))
    );
    assert!(files.iter().any(|f| f.contains("assets/")));
    assert!(files.iter().any(|f| f.contains("fonts/")));
}

#[test]
fn test_list_embedded_files_no_backslashes() {
    let files = list_embedded_files();
    for f in &files {
        assert!(!f.contains('\\'), "Path should use forward slashes: {}", f);
    }
}

#[test]
#[allow(deprecated)]
fn test_resolve_embedded_static_dir_returns_path() {
    // Legacy function only returns disk path now
    let result = resolve_embedded_static_dir();
    // Result depends on whether static/ exists next to exe
    let _ = result;
}

#[test]
fn test_resolve_static_files_returns_provider() {
    let provider = resolve_static_files();
    // Should be able to get index.html from either disk or memory
    let content = provider.get_file("index.html");
    assert!(content.is_some());
    let content = content.unwrap();
    let html = std::str::from_utf8(&content).unwrap();
    assert!(html.contains("<!DOCTYPE html>") || html.contains("<html"));
}

#[test]
fn test_resolve_static_files_list() {
    let provider = resolve_static_files();
    let files = provider.list_files();
    assert!(!files.is_empty());
    assert!(files.iter().any(|f| f.contains("index.html")));
}

#[test]
fn test_embedded_static_files_get_file() {
    let provider = EmbeddedStaticFiles::new(&EMBEDDED_STATIC);
    let content = provider.get_file("index.html");
    assert!(content.is_some());
    assert!(provider.get_file("nonexistent.html").is_none());
}

#[test]
fn test_embedded_static_files_path_traversal() {
    let provider = EmbeddedStaticFiles::new(&EMBEDDED_STATIC);
    assert!(provider.get_file("../Cargo.toml").is_none());
    assert!(provider.get_file("../../secret").is_none());
}

#[test]
fn test_extract_workspace_templates_to_temp() {
    let temp = tempfile::tempdir().unwrap();
    let result = extract_workspace_templates(temp.path());
    // Should succeed as long as embedded workspace exists
    assert!(result.is_ok());
}

#[test]
fn test_extract_workspace_templates_overwrite() {
    let temp = tempfile::tempdir().unwrap();

    // First extraction
    extract_workspace_templates_overwrite(temp.path()).unwrap();

    // Create a modified file
    let config_path = temp.path().join("config.json");
    if config_path.exists() {
        let original = std::fs::read_to_string(&config_path).unwrap();
        std::fs::write(&config_path, "modified content").unwrap();

        // Overwrite should restore original
        extract_workspace_templates_overwrite(temp.path()).unwrap();
        let restored = std::fs::read_to_string(&config_path).unwrap();
        assert_eq!(restored, original);
    }
}

#[test]
fn test_extract_dir_skip_existing_preserves_user_files() {
    let temp = tempfile::tempdir().unwrap();

    // Create a file first
    let user_file = temp.path().join("config.json");
    std::fs::write(&user_file, "user customization").unwrap();

    // Extract should skip existing files
    let result = extract_dir_skip_existing(&EMBEDDED_WORKSPACE, temp.path());
    assert!(result.is_ok());

    // User file should not be overwritten
    let content = std::fs::read_to_string(&user_file).unwrap();
    assert_eq!(content, "user customization");
}

#[test]
fn test_embedded_static_has_fonts() {
    assert!(EMBEDDED_STATIC.contains("fonts"));
}

#[test]
fn test_embedded_workspace_exists() {
    // EMBEDDED_WORKSPACE should be valid and contain files
    let files: Vec<_> = EMBEDDED_WORKSPACE.files().collect();
    assert!(!files.is_empty(), "Embedded workspace should contain files");
}

#[test]
fn test_embedded_static_index_html_content() {
    let file = EMBEDDED_STATIC.get_file("index.html").unwrap();
    let content = std::str::from_utf8(file.contents()).unwrap();
    assert!(content.contains("<!DOCTYPE html>") || content.contains("<html"));
    assert!(content.len() > 100);
}

#[test]
fn test_extract_dir_idempotent() {
    let temp1 = tempfile::tempdir().unwrap();
    let temp2 = tempfile::tempdir().unwrap();

    extract_dir(&EMBEDDED_STATIC, temp1.path()).unwrap();
    extract_dir(&EMBEDDED_STATIC, temp2.path()).unwrap();

    // Both should have identical content
    let file1 = std::fs::read(temp1.path().join("index.html")).unwrap();
    let file2 = std::fs::read(temp2.path().join("index.html")).unwrap();
    assert_eq!(file1, file2);
}

#[test]
fn test_collect_files_returns_all() {
    let mut files = Vec::new();
    collect_files(&EMBEDDED_STATIC, &mut files);
    assert!(!files.is_empty());
    // All entries should be relative paths with forward slashes
    for f in &files {
        assert!(!f.is_empty());
        assert!(!f.contains('\\'));
    }
}
