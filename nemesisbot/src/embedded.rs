//! Embedded static resources and workspace templates.
//!
//! Uses `include_dir!` to embed:
//! - Web UI static files at compile time
//! - Workspace templates (skills, scripts, memory, md files)
//!
//! Falls back to filesystem at runtime when static files are on disk.

use include_dir::{include_dir, Dir};

/// Embedded static files from `crates/nemesis-web/static/`.
///
/// At compile time this pulls in all Web UI files (HTML, CSS, JS, fonts).
/// At runtime, if files exist on disk next to the executable, those are
/// preferred (allows updating without recompiling).
static EMBEDDED_STATIC: Dir = include_dir!("$CARGO_MANIFEST_DIR/../crates/nemesis-web/static");

/// Embedded workspace templates from `nemesisbot/workspace/`.
///
/// Mirrors Go's `//go:embed workspace` — includes personality files,
/// built-in skills, scripts, memory template, etc.
static EMBEDDED_WORKSPACE: Dir = include_dir!("$CARGO_MANIFEST_DIR/workspace");

/// Resolve the static files directory for the web server.
///
/// Checks in order:
/// 1. Files on disk next to the executable (`<exe_dir>/static/`)
/// 2. Embedded static files extracted to a temp location
///
/// Returns the path to serve static files from.
pub fn resolve_embedded_static_dir() -> Option<String> {
    // Prefer on-disk files next to executable
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let disk_static = exe_dir.join("static");
            if disk_static.exists() && disk_static.is_dir() {
                return Some(disk_static.to_string_lossy().to_string());
            }
        }
    }

    // Fall back to embedded files — extract to temp directory
    // Use an extraction marker that changes when the extraction logic is
    // fixed, so stale/broken directories are re-extracted automatically.
    let temp_dir = std::env::temp_dir().join("nemesisbot-static");
    const EXTRACTION_VERSION: &str = "v2";
    let marker = temp_dir.join(".embedded-version");
    let current_marker = format!("{}-{}", env!("CARGO_PKG_VERSION"), EXTRACTION_VERSION);
    let needs_extract = !temp_dir.exists()
        || !marker.exists()
        || std::fs::read_to_string(&marker).unwrap_or_default() != current_marker;

    if !needs_extract {
        return Some(temp_dir.to_string_lossy().to_string());
    }

    // Remove old extracted directory to avoid stale nested paths
    if temp_dir.exists() {
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    // Extract embedded files to temp directory
    if let Err(e) = std::fs::create_dir_all(&temp_dir) {
        tracing::warn!("Failed to create static dir: {}", e);
        return None;
    }
    if let Err(e) = extract_dir(&EMBEDDED_STATIC, &temp_dir) {
        tracing::warn!("Failed to extract embedded static files: {}", e);
        return None;
    }

    // Write version marker
    let _ = std::fs::write(temp_dir.join(".embedded-version"), &current_marker);

    Some(temp_dir.to_string_lossy().to_string())
}

/// Recursively extract an embedded directory to disk.
///
/// Note: `include_dir`'s `File::path()` returns paths relative to the
/// **root** embedded directory, not the current subdirectory.  All files
/// must therefore be joined against the `base` (root output) directory,
/// and the same `base` is passed through recursive calls to avoid
/// duplicate path nesting (e.g. `css/css/theme.css`).
fn extract_dir(dir: &Dir, base: &std::path::Path) -> std::io::Result<()> {
    for file in dir.files() {
        let file_path = base.join(file.path());
        if let Some(parent) = file_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&file_path, file.contents())?;
    }

    for subdir in dir.dirs() {
        // Pass the SAME base directory — do NOT join with subdir.path()
        // because file paths are already relative to the root.
        extract_dir(subdir, base)?;
    }

    Ok(())
}

/// Get a specific embedded file by path.
#[allow(dead_code)]
pub fn get_embedded_file(path: &str) -> Option<&'static [u8]> {
    EMBEDDED_STATIC.get_file(path).map(|f| f.contents())
}

/// Extract all embedded workspace templates to the target directory.
///
/// Mirrors Go's `copyEmbeddedToTarget()` — walks the embedded `workspace/`
/// directory and copies every file to `target_dir`, preserving structure.
/// Does not overwrite existing files (allows user customizations to survive).
#[allow(dead_code)]
pub fn extract_workspace_templates(target_dir: &std::path::Path) -> std::io::Result<()> {
    extract_dir_skip_existing(&EMBEDDED_WORKSPACE, target_dir)
}

/// Extract all embedded workspace templates, **always overwriting** existing files.
///
/// Used by `onboard` to ensure template files are restored to their original
/// state.  Mirrors Go's `copyEmbeddedToTarget()` which always overwrites.
pub fn extract_workspace_templates_overwrite(target_dir: &std::path::Path) -> std::io::Result<()> {
    extract_dir(&EMBEDDED_WORKSPACE, target_dir)
}

/// Recursively extract an embedded directory, skipping files that already exist.
#[allow(dead_code)]
fn extract_dir_skip_existing(dir: &Dir, base: &std::path::Path) -> std::io::Result<()> {
    for file in dir.files() {
        let file_path = base.join(file.path());
        if let Some(parent) = file_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Skip if file already exists (user may have customized)
        if !file_path.exists() {
            std::fs::write(&file_path, file.contents())?;
        }
    }

    for subdir in dir.dirs() {
        extract_dir_skip_existing(subdir, base)?;
    }

    Ok(())
}

/// List all embedded file paths.
#[allow(dead_code)]
pub fn list_embedded_files() -> Vec<String> {
    let mut files = Vec::new();
    collect_files(&EMBEDDED_STATIC, &mut files);
    files
}

#[allow(dead_code)]
fn collect_files(dir: &Dir, files: &mut Vec<String>) {
    for file in dir.files() {
        files.push(file.path().to_string_lossy().replace('\\', "/"));
    }
    for subdir in dir.dirs() {
        collect_files(subdir, files);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(files.iter().any(|f| f.contains("css/")));
        assert!(files.iter().any(|f| f.contains("js/")));
    }

    #[test]
    fn test_extract_dir() {
        let temp = tempfile::tempdir().unwrap();
        extract_dir(&EMBEDDED_STATIC, temp.path()).unwrap();

        assert!(temp.path().join("index.html").exists());
        assert!(temp.path().join("css").is_dir());
        assert!(temp.path().join("js").is_dir());
        assert!(temp.path().join("fonts").is_dir());

        // Verify files are at the correct depth (not nested like css/css/theme.css)
        assert!(
            temp.path().join("css/theme.css").exists(),
            "css/theme.css should exist at correct depth"
        );
        assert!(
            temp.path().join("css/layout.css").exists(),
            "css/layout.css should exist at correct depth"
        );
        assert!(
            temp.path().join("css/components.css").exists(),
            "css/components.css should exist at correct depth"
        );
        assert!(
            temp.path().join("js/app.js").exists(),
            "js/app.js should exist at correct depth"
        );
        assert!(
            temp.path().join("js/api.js").exists(),
            "js/api.js should exist at correct depth"
        );

        // Verify files are NOT at doubled paths
        assert!(
            !temp.path().join("css/css/theme.css").exists(),
            "css/css/theme.css should NOT exist (path nesting bug)"
        );
        assert!(
            !temp.path().join("js/js/app.js").exists(),
            "js/js/app.js should NOT exist (path nesting bug)"
        );
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
        let content = get_embedded_file("css/theme.css");
        assert!(content.is_some());
        let css = std::str::from_utf8(content.unwrap()).unwrap();
        assert!(!css.is_empty());
    }

    #[test]
    fn test_get_embedded_file_js() {
        let content = get_embedded_file("js/app.js");
        assert!(content.is_some());
    }

    #[test]
    fn test_list_embedded_files_contains_known_paths() {
        let files = list_embedded_files();
        assert!(files.iter().any(|f| f == "index.html" || f.contains("index.html")));
        assert!(files.iter().any(|f| f.contains("css/")));
        assert!(files.iter().any(|f| f.contains("js/")));
    }

    #[test]
    fn test_list_embedded_files_no_backslashes() {
        let files = list_embedded_files();
        for f in &files {
            assert!(!f.contains('\\'), "Path should use forward slashes: {}", f);
        }
    }

    #[test]
    fn test_resolve_embedded_static_dir_returns_path() {
        // This tests the full resolve logic - may create temp extraction
        let result = resolve_embedded_static_dir();
        assert!(result.is_some());
        let path = result.unwrap();
        assert!(!path.is_empty());
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
}
