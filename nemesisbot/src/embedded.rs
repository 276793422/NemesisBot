//! Embedded static resources and workspace templates.
//!
//! Uses `include_dir!` to embed:
//! - Web UI static files at compile time
//! - Workspace templates (skills, scripts, memory, md files)
//!
//! Static files are served directly from memory — zero disk IO.

use include_dir::{include_dir, Dir};
use std::sync::Arc;

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

// ---------------------------------------------------------------------------
// In-memory static file serving
// ---------------------------------------------------------------------------

/// Static file provider backed by compile-time embedded `include_dir::Dir`.
///
/// Files are served directly from the binary — zero disk IO.
pub struct EmbeddedStaticFiles(&'static Dir<'static>);

impl EmbeddedStaticFiles {
    pub fn new(dir: &'static Dir<'static>) -> Self {
        Self(dir)
    }
}

impl nemesis_web::StaticFiles for EmbeddedStaticFiles {
    fn get_file(&self, path: &str) -> Option<Vec<u8>> {
        let path = path.trim_start_matches('/');
        if path.contains("..") {
            return None;
        }
        self.0.get_file(path).map(|f| f.contents().to_vec())
    }

    fn list_files(&self) -> Vec<String> {
        let mut files = Vec::new();
        collect_files(self.0, &mut files);
        files
    }
}

/// Resolve the static files provider for the web server.
///
/// Priority:
/// 1. `static/` directory next to the executable (development override)
/// 2. Compile-time embedded files served from memory (production)
pub fn resolve_static_files() -> Arc<dyn nemesis_web::StaticFiles> {
    // Development override: disk directory next to executable
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let disk_static = exe_dir.join("static");
            if disk_static.exists() && disk_static.is_dir() {
                tracing::info!(
                    path = %disk_static.display(),
                    "Serving static files from disk (development override)"
                );
                return Arc::new(nemesis_web::DirectoryStaticFiles::new(disk_static));
            }
        }
    }

    // Production: serve directly from embedded memory
    tracing::info!("Serving static files from embedded memory (zero disk IO)");
    Arc::new(EmbeddedStaticFiles::new(&EMBEDDED_STATIC))
}

/// Resolve the static files directory for the web server (legacy).
///
/// **Deprecated**: Use [`resolve_static_files`] instead, which serves files
/// directly from memory without extracting to disk.
///
/// Only returns a disk path when `static/` exists next to the executable.
#[deprecated(note = "Use resolve_static_files() instead for in-memory serving")]
#[allow(dead_code)]
pub fn resolve_embedded_static_dir() -> Option<String> {
    // Only check disk — no more temp extraction
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let disk_static = exe_dir.join("static");
            if disk_static.exists() && disk_static.is_dir() {
                return Some(disk_static.to_string_lossy().to_string());
            }
        }
    }
    None
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
        let js_files: Vec<_> = std::fs::read_dir(&assets_dir).unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map(|ext| ext == "js").unwrap_or(false))
            .collect();
        let css_files: Vec<_> = std::fs::read_dir(&assets_dir).unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map(|ext| ext == "css").unwrap_or(false))
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
        assert!(files.iter().any(|f| f.contains("assets/") && f.ends_with(".css")));
    }

    #[test]
    fn test_get_embedded_file_js() {
        // Vue build puts JS into assets/ directory
        let files = list_embedded_files();
        assert!(files.iter().any(|f| f.contains("assets/") && f.ends_with(".js")));
    }

    #[test]
    fn test_list_embedded_files_contains_known_paths() {
        let files = list_embedded_files();
        assert!(files.iter().any(|f| f == "index.html" || f.contains("index.html")));
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
}
