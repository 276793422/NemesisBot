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
                    "[Main] Serving static files from disk (development override)"
                );
                return Arc::new(nemesis_web::DirectoryStaticFiles::new(disk_static));
            }
        }
    }

    // Production: serve directly from embedded memory
    tracing::info!("[Main] Serving static files from embedded memory (zero disk IO)");
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
mod tests;
