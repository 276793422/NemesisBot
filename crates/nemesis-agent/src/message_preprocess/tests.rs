use super::*;
use tempfile::TempDir;

/// 统一调用形态：无安全管线（None = 管线未挂直通，两 cfg 下类型都对）。
fn expand(content: &str, base: &Path) -> String {
    expand_at_files(content, base, "test", None)
}

// ---------------------------------------------------------------------------
// 提取（纯解析，不碰磁盘）
// ---------------------------------------------------------------------------

#[test]
fn test_extract_line_range() {
    let refs = extract_file_refs("see @src/main.rs#L10-20 end");
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].path, "src/main.rs");
    assert_eq!(refs[0].line_start, Some(10));
    assert_eq!(refs[0].line_end, Some(20));
}

#[test]
fn test_extract_single_line() {
    let refs = extract_file_refs("@a.rs#L5");
    assert_eq!(refs[0].path, "a.rs");
    assert_eq!(refs[0].line_start, Some(5));
    assert_eq!(refs[0].line_end, None);
}

#[test]
fn test_extract_multiple_and_order() {
    let refs = extract_file_refs("@b/a.txt and @b.txt");
    assert_eq!(refs.len(), 2);
    assert_eq!(refs[0].path, "b/a.txt");
    assert_eq!(refs[1].path, "b.txt");
}

#[test]
fn test_extract_skips_mid_word_and_bare_words() {
    // 邮箱：@ 前是单词字符；裸词 @john：无分隔符无扩展名（mention 语义）。
    assert!(extract_file_refs("user@example.com").is_empty());
    assert!(extract_file_refs("ping @john later").is_empty());
    assert!(extract_file_refs("no refs at all").is_empty());
}

#[test]
fn test_extract_trailing_punct_trimmed() {
    let refs = extract_file_refs("see @a.rs, then @b.rs.");
    assert_eq!(refs[0].path, "a.rs");
    assert_eq!(refs[1].path, "b.rs");
}

// ---------------------------------------------------------------------------
// 展开（磁盘 + 注记）
// ---------------------------------------------------------------------------

#[test]
fn test_expand_existing_file() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("foo.rs"), "fn main() {}").unwrap();
    let out = expand("@foo.rs explain this", tmp.path());
    // <file_ref> 块前置 + 原文保持。
    assert!(out.contains("<file_ref path=\"foo.rs\">"));
    assert!(out.contains("fn main() {}"));
    assert!(out.contains("explain this"));
    let block_end = out.find("</file_ref>").unwrap();
    let orig = out.find("explain this").unwrap();
    assert!(
        block_end < orig,
        "file_ref block must be PREPENDED: {}",
        out
    );
}

#[test]
fn test_nonexistent_rs_noted() {
    let tmp = TempDir::new().unwrap();
    let content = "check @does_not_exist.rs here";
    let out = expand(content, tmp.path());
    // 路径形 token（有扩展名）→ 诚实注记，不再静默吞。
    assert!(out.contains("check @does_not_exist.rs here"));
    assert!(out.contains("[文件引用失败: does_not_exist.rs: 文件不存在]"));
}

#[test]
fn test_bare_word_mention_untouched() {
    let tmp = TempDir::new().unwrap();
    let content = "ping @john later";
    assert_eq!(expand(content, tmp.path()), content);
}

#[test]
fn test_multiple_refs() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("a.txt"), "AAA").unwrap();
    std::fs::write(tmp.path().join("b.txt"), "BBB").unwrap();
    let out = expand("@a.txt and @b.txt", tmp.path());
    assert!(out.contains("AAA") && out.contains("BBB"));
    assert_eq!(out.matches("<file_ref").count(), 2);
}

#[test]
fn test_subdirectory_file() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir(tmp.path().join("src")).unwrap();
    std::fs::write(tmp.path().join("src").join("main.rs"), "fn hello()").unwrap();
    let out = expand("check @src/main.rs", tmp.path());
    assert!(out.contains("fn hello()"));
    assert!(out.contains("src/main.rs") || out.contains("src\\main.rs"));
}

#[test]
fn test_trailing_punctuation_stripped() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("foo.rs"), "content").unwrap();
    let out = expand("see @foo.rs.", tmp.path());
    assert!(out.contains("<file_ref"));
    // 尾部句号不应进路径（否则文件不存在）。
    assert!(!out.contains("文件引用失败"));
}

#[test]
fn test_no_at_sign_untouched() {
    let tmp = TempDir::new().unwrap();
    let content = "just plain text no refs";
    assert_eq!(expand(content, tmp.path()), content);
}

#[test]
fn test_absolute_path_inlined() {
    let tmp = TempDir::new().unwrap();
    let abs = tmp.path().join("abs.txt");
    std::fs::write(&abs, "ABS").unwrap();
    let out = expand(format!("see @{}", abs.display()).as_str(), tmp.path());
    assert!(out.contains("ABS"));
}

#[test]
fn test_absolute_path_outside_base() {
    let tmp = TempDir::new().unwrap();
    let abs_file = tmp.path().join("external.txt");
    std::fs::write(&abs_file, "ABSOLUTE CONTENT").unwrap();
    let content = format!("check @{}", abs_file.display());
    let out = expand(&content, Path::new("/nonexistent"));
    assert!(out.contains("ABSOLUTE CONTENT"), "should inline: {}", out);
}

#[test]
fn test_email_at_sign_not_matched() {
    let tmp = TempDir::new().unwrap();
    let content = "Contact me at user@example.com please";
    assert_eq!(expand(content, tmp.path()), content);
}

#[test]
fn test_empty_content() {
    let tmp = TempDir::new().unwrap();
    assert_eq!(expand("", tmp.path()), "");
}

#[test]
fn test_punctuation_only_reference_skipped() {
    let tmp = TempDir::new().unwrap();
    let out = expand("hello @. world @)!!!", tmp.path());
    assert_eq!(out, "hello @. world @)!!!");
    assert!(!out.contains("<file_ref"));
}

// ---------------------------------------------------------------------------
// 行号切片
// ---------------------------------------------------------------------------

#[test]
fn test_line_range_slice() {
    let tmp = TempDir::new().unwrap();
    let body: String = (1..=30).map(|i| format!("line{}\n", i)).collect();
    std::fs::write(tmp.path().join("big30.txt"), &body).unwrap();
    let out = expand("@big30.txt#L10-20", tmp.path());
    assert!(out.contains("lines=\"10-20\""));
    assert!(out.contains("line10\n"));
    assert!(out.contains("line20"));
    assert!(!out.contains("line9\n"), "line 9 must not leak: {}", out);
    assert!(!out.contains("line21"));
}

#[test]
fn test_single_line_slice() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("one.txt"), "l1\nl2\nl3\nl4\nl5\n").unwrap();
    let out = expand("@one.txt#L3", tmp.path());
    assert!(out.contains("lines=\"3\""));
    assert!(out.contains("l3"));
    assert!(!out.contains("l2"));
    assert!(!out.contains("l4"));
}

#[test]
fn test_line_out_of_range_noted() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("tiny.txt"), "only line\n").unwrap();
    let out = expand("@tiny.txt#L99", tmp.path());
    assert!(out.contains("[文件引用失败: tiny.txt: 行号 99 超出文件范围"));
}

// ---------------------------------------------------------------------------
// 去重 / 图片让位 / 截断
// ---------------------------------------------------------------------------

#[test]
fn test_dedupe_same_ref_once() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("dup.rs"), "DUP").unwrap();
    let out = expand("@dup.rs and @dup.rs", tmp.path());
    assert_eq!(
        out.matches("<file_ref").count(),
        1,
        "same ref deduped: {}",
        out
    );
    assert!(out.contains("DUP"));
}

#[test]
fn test_image_ext_silent_skip() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("photo.png"), [0x89, b'P', b'N', b'G']).unwrap();
    let content = "look at @photo.png please";
    // 图片让位给 attach_turn_images：无 <file_ref> 块、无失败注记。
    assert_eq!(expand(content, tmp.path()), content);
}

#[test]
fn test_large_file_truncated() {
    let tmp = TempDir::new().unwrap();
    let big = "A".repeat(25000);
    std::fs::write(tmp.path().join("big.txt"), &big).unwrap();
    let out = expand("@big.txt explain", tmp.path());
    assert!(
        out.contains("已截断，原内容共 25000 字节"),
        "should carry truncate locator: {}",
        &out[..out.len().min(400)]
    );
}

/// 多字节安全截断回归：8KB 边界落在 CJK 字符中间（旧实现
/// `&body[..20000.min(len)]` 在此 panic —— str-slice-multibyte-panic 家族）。
#[test]
fn test_multibyte_truncation_no_panic() {
    let tmp = TempDir::new().unwrap();
    // 3 字节/字符 × 4000 = 12000 字节，8192 % 3 ≠ 0 → 边界必在字符中间。
    let big = "中".repeat(4000);
    assert_eq!(big.len(), 12000);
    std::fs::write(tmp.path().join("cjk.txt"), &big).unwrap();
    let out = expand("@cjk.txt 说明", tmp.path()); // must not panic
    assert!(out.contains("已截断"));
    assert!(out.is_char_boundary(out.find("已截断").unwrap() - 1));
}

#[test]
fn test_non_utf8_noted() {
    let tmp = TempDir::new().unwrap();
    let bad = tmp.path().join("bad.bin");
    std::fs::write(&bad, [0xffu8, 0xfe, 0x00, 0x01]).unwrap();
    let out = expand("see @bad.bin", tmp.path());
    assert!(out.contains("[文件引用失败: bad.bin: 文件不是有效的 UTF-8 文本]"));
}

// ---------------------------------------------------------------------------
// 安全闸（真实 SecurityPlugin；feature=security 才编译）
// ---------------------------------------------------------------------------

#[cfg(feature = "security")]
fn multithread_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("multi-thread runtime")
}

/// deny *.rs 的最小管线（同 image_attach::deny_png_plugin 形态）。
#[cfg(feature = "security")]
fn deny_rs_plugin() -> nemesis_security::pipeline::SecurityPlugin {
    use nemesis_security::pipeline::{SecurityPlugin, SecurityPluginConfig};
    use nemesis_security::types::SecurityRule;
    SecurityPlugin::new(SecurityPluginConfig {
        enabled: true,
        default_action: "allow".to_string(),
        file_rules: vec![SecurityRule {
            pattern: "*.rs".to_string(),
            action: "deny".to_string(),
            comment: "test: deny rs file refs".to_string(),
        }],
        ..Default::default()
    })
}

#[test]
#[cfg(feature = "security")]
fn test_security_deny_noted() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("secret.rs"), "SECRET").unwrap();
    let rt = multithread_runtime();
    let out = rt.block_on(async {
        let plugin = deny_rs_plugin();
        expand_at_files("see @secret.rs now", tmp.path(), "test", Some(&plugin))
    });
    assert!(out.contains("see @secret.rs now"), "original kept: {}", out);
    assert!(
        out.contains("[文件引用失败: secret.rs: [layer:"),
        "deny note with layer prefix: {}",
        out
    );
    assert!(!out.contains("SECRET"), "denied content must not inline");
}

#[test]
#[cfg(feature = "security")]
fn test_security_allow_inlines() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("ok.txt"), "OKAY").unwrap();
    use nemesis_security::pipeline::{SecurityPlugin, SecurityPluginConfig};
    let plugin = SecurityPlugin::new(SecurityPluginConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    });
    let rt = multithread_runtime();
    let out =
        rt.block_on(async { expand_at_files("see @ok.txt", tmp.path(), "test", Some(&plugin)) });
    assert!(out.contains("<file_ref path=\"ok.txt\">"));
    assert!(out.contains("OKAY"));
}
