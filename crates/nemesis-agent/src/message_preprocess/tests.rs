use super::*;
use tempfile::TempDir;

#[test]
fn test_expand_existing_file() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("foo.rs"), "fn main() {}").unwrap();
    let out = expand_at_files("@foo.rs explain this", tmp.path());
    assert!(out.contains("<file path=\"foo.rs\">"));
    assert!(out.contains("fn main() {}"));
    assert!(out.contains("explain this"));
}

#[test]
fn test_nonexistent_left_untouched() {
    let tmp = TempDir::new().unwrap();
    let content = "Hello @nonexistent world";
    let out = expand_at_files(content, tmp.path());
    assert_eq!(out, content);
}

#[test]
fn test_multiple_refs() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("a.txt"), "AAA").unwrap();
    std::fs::write(tmp.path().join("b.txt"), "BBB").unwrap();
    let out = expand_at_files("@a.txt and @b.txt", tmp.path());
    assert!(out.contains("AAA") && out.contains("BBB"));
    assert_eq!(out.matches("<file").count(), 2);
}

#[test]
fn test_subdirectory_file() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir(tmp.path().join("src")).unwrap();
    std::fs::write(tmp.path().join("src").join("main.rs"), "fn hello()").unwrap();
    let out = expand_at_files("check @src/main.rs", tmp.path());
    assert!(out.contains("fn hello()"));
    assert!(out.contains("src/main.rs") || out.contains("src\\main.rs"));
}

#[test]
fn test_trailing_punctuation_stripped() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("foo.rs"), "content").unwrap();
    let out = expand_at_files("see @foo.rs.", tmp.path());
    assert!(out.contains("<file"));
}

#[test]
fn test_no_at_sign_untouched() {
    let tmp = TempDir::new().unwrap();
    let content = "just plain text no refs";
    let out = expand_at_files(content, tmp.path());
    assert_eq!(out, content);
}

// ---- Edge cases / branch coverage ----

#[test]
fn test_absolute_path_outside_base() {
    // A file that exists but is absolute (outside base) should still be inlined.
    let tmp = TempDir::new().unwrap();
    let abs_file = tmp.path().join("external.txt");
    std::fs::write(&abs_file, "ABSOLUTE CONTENT").unwrap();
    let abs_str = abs_file.to_string_lossy().to_string();
    let content = format!("check @{}", abs_str);
    let out = expand_at_files(&content, std::path::Path::new("/nonexistent"));
    assert!(
        out.contains("ABSOLUTE CONTENT"),
        "should inline absolute file: {}",
        out
    );
}

#[test]
fn test_large_file_truncated() {
    let tmp = TempDir::new().unwrap();
    let big = "A".repeat(25000);
    std::fs::write(tmp.path().join("big.txt"), &big).unwrap();
    let out = expand_at_files("@big.txt explain", tmp.path());
    assert!(
        out.contains("truncated"),
        "should be truncated: {}",
        &out[out.len().min(200)..]
    );
}

#[test]
fn test_email_at_sign_not_matched() {
    let tmp = TempDir::new().unwrap();
    let content = "Contact me at user@example.com please";
    let out = expand_at_files(content, tmp.path());
    // @example.com is not a file → left untouched.
    assert_eq!(out, content);
}

#[test]
fn test_nonexistent_file_left_untouched() {
    let tmp = TempDir::new().unwrap();
    let content = "check @does_not_exist.rs here";
    let out = expand_at_files(content, tmp.path());
    assert_eq!(out, content);
}

#[test]
fn test_at_at_end_of_string() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("end.txt"), "END").unwrap();
    let out = expand_at_files("see @end.txt", tmp.path());
    assert!(out.contains("END"));
}

#[test]
fn test_multiple_same_file() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("dup.rs"), "DUP").unwrap();
    // Same file referenced twice — should inline twice (or at least not crash).
    let out = expand_at_files("@dup.rs and @dup.rs", tmp.path());
    assert!(out.contains("DUP"));
}

#[test]
fn test_empty_content() {
    let tmp = TempDir::new().unwrap();
    let out = expand_at_files("", tmp.path());
    assert_eq!(out, "");
}
