//! Build script for nemesis-web.
//!
//! P2-2 (2026-08-24 UI entry gap): compile-time packaging of the Python SDK
//! (`test-tools/python/sdk/`) into two zip artifacts embedded via
//! `include_bytes!` in `sdk_embed.rs`:
//!
//! - `sdk_export.zip` — SDK tree at the zip root (the 「导出 SDK 目录」
//!   download; unzip → complete browsable source tree).
//! - `sdk_sdist.zip` — same tree under a `nemesisbot-<version>/` top-level
//!   directory (sdist layout, the 「下载 pip 包」 download; `pip install
//!   ./nemesisbot-sdk-pip-<version>.zip` works because pip descends into the
//!   single top-level dir of a source archive).
//!
//! Why build.rs and not `include_dir!`: the SDK directory contains build
//! artifacts (`build/`, `nemesisbot.egg-info/`, `__pycache__/`,
//! `.pytest_cache/`) that must NOT ship inside the exe, and `include_dir!`
//! cannot exclude subtrees. Here we filter while walking.
//!
//! Version is parsed from `pyproject.toml` (`version = "…"`) and exported as
//! `NEMESIS_SDK_VERSION` for filename generation at serve time.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Directory/file names that never enter the artifacts (build junk, caches).
fn is_junk_name(name: &str) -> bool {
    name == "build"
        || name == "__pycache__"
        || name == ".pytest_cache"
        || name.ends_with(".egg-info")
        || name == ".git"
        || name == ".venv"
}

fn is_junk_file(name: &str) -> bool {
    name.ends_with(".pyc") || name.ends_with(".pyo")
}

/// Collect the filtered SDK tree as (zip-relative-path, absolute-file-path)
/// pairs, walked deterministically (sorted per directory).
fn collect_files(sdk_dir: &Path, prefix: &str, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = fs::read_dir(sdk_dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for path in paths {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if path.is_dir() {
            if is_junk_name(name) {
                continue;
            }
            collect_files(&path, &format!("{prefix}{name}/"), out);
        } else if !is_junk_file(name) {
            out.push((format!("{prefix}{name}"), path.clone()));
        }
    }
}

/// Parse `version = "x.y.z"` out of pyproject.toml (first occurrence — the
/// `[project]` table's version). Falls back to "0.0.0" so a malformed
/// pyproject fails loudly at serve time (unknown version), not at build time.
fn parse_version(pyproject: &str) -> String {
    for line in pyproject.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("version") {
            let rest = rest.trim_start();
            if let Some(rest) = rest.strip_prefix('=') {
                let v = rest.trim().trim_matches('"').trim_matches('\'');
                if !v.is_empty() {
                    return v.to_string();
                }
            }
        }
    }
    "0.0.0".to_string()
}

fn write_zip(out_path: &Path, files: &[(String, PathBuf)]) {
    let file = fs::File::create(out_path).expect("create sdk zip in OUT_DIR");
    let mut w = zip::ZipWriter::new(std::io::BufWriter::new(file));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (entry_name, src) in files {
        w.start_file(entry_name.as_str(), options)
            .unwrap_or_else(|e| panic!("zip start_file {entry_name}: {e}"));
        let mut buf = Vec::new();
        fs::File::open(src)
            .and_then(|mut f| f.read_to_end(&mut buf))
            .unwrap_or_else(|e| panic!("read {}: {e}", src.display()));
        w.write_all(&buf)
            .unwrap_or_else(|e| panic!("zip write {entry_name}: {e}"));
    }
    w.finish()
        .unwrap_or_else(|e| panic!("finalize {}: {e}", out_path.display()));
}

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let sdk_dir = manifest_dir.join("..").join("..").join("test-tools").join("python").join("sdk");
    let sdk_dir = sdk_dir.canonicalize().expect(
        "test-tools/python/sdk not found — the SDK source tree is required to build nemesis-web",
    );

    // Rebuild when any SDK file changes (directory-level tracking).
    println!("cargo:rerun-if-changed={}", sdk_dir.display());

    let pyproject = fs::read_to_string(sdk_dir.join("pyproject.toml"))
        .expect("read SDK pyproject.toml");
    let version = parse_version(&pyproject);
    println!("cargo:rustc-env=NEMESIS_SDK_VERSION={version}");

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());

    // Layout A: files at zip root.
    let mut files = Vec::new();
    collect_files(&sdk_dir, "", &mut files);
    assert!(!files.is_empty(), "SDK tree produced zero files");
    write_zip(&out_dir.join("sdk_export.zip"), &files);

    // Layout B: sdist-style single top-level dir.
    let prefix = format!("nemesisbot-{version}/");
    let files_sdist: Vec<(String, PathBuf)> = files
        .iter()
        .map(|(name, p)| (format!("{prefix}{name}"), p.clone()))
        .collect();
    write_zip(&out_dir.join("sdk_sdist.zip"), &files_sdist);
}
