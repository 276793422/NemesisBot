//! Unit tests for the language→server registry.

use super::*;

#[test]
fn routes_extensions_case_insensitively() {
    assert_eq!(lang_for_path(Path::new("/x/main.RS")), Some(Lang::Rust));
    assert_eq!(lang_for_path(Path::new("/x/mod.go")), Some(Lang::Go));
    assert_eq!(lang_for_path(Path::new("/x/a.tsx")), Some(Lang::TypeScript));
    assert_eq!(lang_for_path(Path::new("/x/a.jsx")), Some(Lang::TypeScript));
    assert_eq!(lang_for_path(Path::new("/x/a.py")), Some(Lang::Python));
    assert_eq!(lang_for_path(Path::new("/x/a.cpp")), Some(Lang::C));
}

#[test]
fn unknown_extension_is_none() {
    assert_eq!(lang_for_path(Path::new("/x/README.md")), None);
    assert_eq!(lang_for_path(Path::new("/x/noext")), None);
}

#[test]
fn every_lang_has_a_spec_and_label() {
    for spec in SERVERS {
        assert!(spec_for(spec.lang).is_some());
        assert!(!spec.lang.label().is_empty());
        assert!(!spec.extensions.is_empty());
        assert!(!spec.command.is_empty());
    }
}

#[test]
fn ts_and_js_share_one_server() {
    // Both route to Lang::TypeScript — same server process, no duplicate
    // session per file flavor.
    assert_eq!(
        lang_for_path(Path::new("/x/a.ts")),
        lang_for_path(Path::new("/x/b.js"))
    );
}

#[test]
fn find_command_finds_a_standard_tool() {
    // Every environment this test suite realistically runs in has some
    // interpreter on PATH — probe for a few and require at least one hit.
    let hits = [
        "rust-analyzer",
        "gopls",
        "clangd",
        "node",
        "python3",
        "python",
    ]
    .iter()
    .filter(|c| find_command(c).is_some())
    .count();
    assert!(hits > 0, "expected at least one common command on PATH");
}
