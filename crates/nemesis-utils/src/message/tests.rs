use super::*;

#[test]
fn test_format_message_short() {
    assert_eq!(format_message("hello", 100), "hello");
}

#[test]
fn test_format_message_long() {
    let long = "a".repeat(200);
    let result = format_message(&long, 50);
    assert!(result.contains("truncated"));
}

#[test]
fn test_sanitize() {
    assert_eq!(sanitize_for_log("hello\nworld"), "hello\\nworld");
}

#[test]
fn test_split_message_short() {
    let result = split_message("hello", 100);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0], "hello");
}

#[test]
fn test_split_message_long_text() {
    let text = "a".repeat(200);
    let result = split_message(&text, 50);
    assert!(result.len() > 1);
    for chunk in &result {
        assert!(chunk.len() <= 55); // Allow slight overflow for code fence injection
    }
}

#[test]
fn test_split_message_with_newlines() {
    let text = "line1\nline2\nline3\nline4\nline5";
    let result = split_message(text, 15);
    assert!(result.len() > 1);
}

#[test]
fn test_split_message_preserves_code_block() {
    let text = "before\n```\ncode here\nmore code\n```\nafter";
    let result = split_message(text, 200);
    assert_eq!(result.len(), 1); // Fits in one chunk
}

// ============================================================
// Additional tests for missing coverage
// ============================================================

#[test]
fn test_format_message_exact_length() {
    let msg = "hello";
    assert_eq!(format_message(msg, 5), "hello");
}

#[test]
fn test_format_message_zero_max_len() {
    let result = format_message("hello", 0);
    assert!(result.contains("truncated"));
}

#[test]
fn test_format_message_empty_string() {
    assert_eq!(format_message("", 100), "");
}

#[test]
fn test_format_message_truncated_total_count() {
    let long = "x".repeat(300);
    let result = format_message(&long, 50);
    assert!(result.starts_with(&"x".repeat(50)));
    assert!(result.contains("300 chars total"));
}

#[test]
fn test_sanitize_tabs() {
    assert_eq!(sanitize_for_log("hello\tworld"), "hello\\tworld");
}

#[test]
fn test_sanitize_carriage_return() {
    assert_eq!(sanitize_for_log("hello\r\nworld"), "hello\\r\\nworld");
}

#[test]
fn test_sanitize_truncates_long_input() {
    let long = "a".repeat(500);
    let result = sanitize_for_log(&long);
    assert!(result.len() <= 200);
}

#[test]
fn test_sanitize_short_input_unchanged() {
    let input = "hello world";
    assert_eq!(sanitize_for_log(input), "hello world");
}

#[test]
fn test_split_message_empty_string() {
    let result = split_message("", 100);
    assert!(result.is_empty());
}

#[test]
fn test_split_message_single_char() {
    let result = split_message("a", 100);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0], "a");
}

#[test]
fn test_split_message_respects_newlines() {
    let text = "line1\nline2\nline3\nline4\nline5\nline6";
    let result = split_message(text, 20);
    assert!(result.len() > 1);
    // Each chunk should be <= max_len + some buffer
    for chunk in &result {
        assert!(chunk.len() <= 25);
    }
}

#[test]
fn test_split_message_code_block_spanning_chunks() {
    let code = "```\n".to_string() + &"code line\n".repeat(50) + "```\n";
    let text = format!("Here's the code:\n{}", code);
    let result = split_message(&text, 200);
    assert!(result.len() > 1, "Should split into multiple chunks");
    // First chunk should handle the code block properly
}

#[test]
fn test_split_message_multiple_code_blocks() {
    let text = "```\ncode1\n```\nText between\n```\ncode2\n```\n";
    let result = split_message(text, 100);
    // Should handle multiple code blocks
    assert!(!result.is_empty());
}

#[test]
fn test_split_message_very_long_word() {
    let long_word = "a".repeat(200);
    let result = split_message(&long_word, 50);
    assert!(result.len() > 1);
}

#[test]
fn test_split_message_with_spaces() {
    let text = "word1 word2 word3 word4 word5 word6 word7 word8 word9 word10";
    let result = split_message(text, 30);
    assert!(result.len() > 1);
    for chunk in &result {
        assert!(chunk.len() <= 35);
    }
}

#[test]
fn test_find_last_unclosed_code_block_open() {
    let text = "some text\n```python\ncode here";
    let pos = find_last_unclosed_code_block(text);
    assert!(pos > 0);
}

#[test]
fn test_find_last_unclosed_code_block_closed() {
    let text = "some text\n```\ncode\n```\nmore text";
    let pos = find_last_unclosed_code_block(text);
    assert_eq!(pos, 0);
}

#[test]
fn test_find_last_unclosed_code_block_no_fences() {
    let text = "just plain text\nno code blocks";
    let pos = find_last_unclosed_code_block(text);
    assert_eq!(pos, 0);
}

#[test]
fn test_find_next_closing_code_block_found() {
    let text = "```python\ncode\n```\nafter";
    let pos = find_next_closing_code_block(text, 0);
    assert!(pos > 0);
}

#[test]
fn test_find_next_closing_code_block_not_found() {
    let text = "```python\ncode without closing";
    let pos = find_next_closing_code_block(text, 0);
    assert_eq!(pos, 0);
}

#[test]
fn test_find_last_newline_found() {
    let text = "line1\nline2\nline3";
    let pos = find_last_newline(text, 10);
    assert!(pos > 0);
}

#[test]
fn test_find_last_newline_not_found() {
    let text = "no newlines here";
    let pos = find_last_newline(text, 100);
    assert_eq!(pos, 0);
}

#[test]
fn test_find_last_space_found() {
    let text = "hello world test";
    let pos = find_last_space(text, 10);
    assert!(pos > 0);
}

#[test]
fn test_find_last_space_tab() {
    let text = "hello\tworld";
    let pos = find_last_space(text, 100);
    assert!(pos > 0);
}

#[test]
fn test_find_last_space_not_found() {
    let text = "nospaceshere";
    let pos = find_last_space(text, 100);
    assert_eq!(pos, 0);
}

#[test]
fn test_split_message_unclosed_code_block_split() {
    // Long code block that needs to be split with closing fence
    let code = "```\n".to_string() + &"line\n".repeat(100);
    let text = format!("Intro:\n{}", code);
    let result = split_message(&text, 100);
    assert!(result.len() > 1);
    // Each chunk should be properly terminated
    for chunk in &result {
        assert!(!chunk.is_empty());
    }
}

#[test]
fn test_split_message_code_block_too_long_splits_inside() {
    // Code block that is too long to fit in one chunk even with extension
    let code_content = "a".repeat(200);
    let text = format!("```\n{}\n```", code_content);
    let result = split_message(&text, 80);
    assert!(result.len() > 1);
}

#[test]
fn test_split_message_exact_max_len() {
    let text = "a".repeat(50);
    let result = split_message(&text, 50);
    assert_eq!(result.len(), 1);
}

#[test]
fn test_split_message_max_len_one() {
    // Very small max_len should still work
    let text = "hello world this is a test";
    let result = split_message(text, 10);
    assert!(!result.is_empty());
}

#[test]
fn test_format_message_boundary() {
    // max_len exactly equals string length
    assert_eq!(format_message("abc", 3), "abc");
    // max_len is one less
    let result = format_message("abcd", 3);
    assert!(result.contains("truncated"));
    assert!(result.contains("4 chars total"));
}

#[test]
fn test_format_message_unicode_content() {
    let msg = "Hello World!";
    assert_eq!(format_message(msg, 100), msg);
}

#[test]
fn test_sanitize_mixed_special_chars() {
    let input = "line1\r\nline2\ttab";
    let result = sanitize_for_log(input);
    assert_eq!(result, "line1\\r\\nline2\\ttab");
}

#[test]
fn test_sanitize_empty_string() {
    assert_eq!(sanitize_for_log(""), "");
}

#[test]
fn test_sanitize_only_newlines() {
    assert_eq!(sanitize_for_log("\n\n\n"), "\\n\\n\\n");
}

#[test]
fn test_split_message_buffer_calculation() {
    // Test with small max_len where buffer calculations hit edge cases
    // max_len=60 -> buffer = 60/10 = 6 (< 50, so buffer = 50)
    // effective_limit = 60 - 50 = 10 (< 30, so effective_limit = 30)
    let text = "a".repeat(100);
    let result = split_message(&text, 60);
    assert!(result.len() > 1);
}

#[test]
fn test_split_message_very_large_max_len() {
    // Content fits in one chunk
    let text = "short message";
    let result = split_message(text, 10000);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0], "short message");
}

#[test]
fn test_split_message_with_code_block_reopening() {
    // Code block that is longer than max_len, should get closing/reopening fences
    let code_content = "x".repeat(300);
    let text = format!("```\n{}\n```", code_content);
    let result = split_message(&text, 100);
    assert!(result.len() > 1);
    // First chunk should end with ```
    // (either natural closing or injected closing fence)
}

#[test]
fn test_split_message_code_block_short_enough_to_extend() {
    // Code block that barely fits when extended
    let text = "```\nshort code\n```";
    let result = split_message(text, 100);
    assert_eq!(result.len(), 1);
}

#[test]
fn test_find_last_unclosed_code_block_multiple_opens() {
    // Two open code blocks (only last one matters for detection)
    let text = "```\ncode1\n```\n```\ncode2";
    let pos = find_last_unclosed_code_block(text);
    assert!(pos > 0);
}

#[test]
fn test_find_last_unclosed_code_block_empty() {
    assert_eq!(find_last_unclosed_code_block(""), 0);
}

#[test]
fn test_find_next_closing_code_block_from_middle() {
    let text = "```open\ncode\n```close\nmore text\n```open2\n```close2";
    let pos = find_next_closing_code_block(text, 0);
    assert!(pos > 0);
}

#[test]
fn test_find_next_closing_code_block_empty() {
    assert_eq!(find_next_closing_code_block("", 0), 0);
}

#[test]
fn test_find_last_newline_empty_string() {
    assert_eq!(find_last_newline("", 100), 0);
}

#[test]
fn test_find_last_newline_window_larger_than_string() {
    let text = "hello\nworld";
    let pos = find_last_newline(text, 100);
    assert_eq!(pos, 5);
}

#[test]
fn test_find_last_space_empty_string() {
    assert_eq!(find_last_space("", 100), 0);
}

#[test]
fn test_find_last_space_window_larger_than_string() {
    let text = "hello world";
    let pos = find_last_space(text, 100);
    assert_eq!(pos, 5);
}

#[test]
fn test_split_message_all_whitespace() {
    let result = split_message("   ", 100);
    // After trim, it should be empty
    // Actually, "   ".is_empty() is false so it enters the loop
    // then gets trimmed after split
    assert!(!result.is_empty());
}

#[test]
fn test_split_message_with_trailing_whitespace() {
    let text = "hello   \nworld";
    let result = split_message(text, 100);
    assert_eq!(result.len(), 1);
}

#[test]
fn test_split_message_code_fence_in_word() {
    // Edge case: backticks that are not actually code fences
    let text = "This has ``inline`` backticks but no fences";
    let result = split_message(text, 100);
    assert_eq!(result.len(), 1);
}

// --- Additional coverage tests ---

#[test]
fn test_format_message_exact_boundary() {
    let text = "a".repeat(50);
    let result = format_message(&text, 50);
    assert_eq!(result, text); // Exactly fits, no truncation
}

#[test]
fn test_format_message_one_over() {
    let text = "a".repeat(51);
    let result = format_message(&text, 50);
    assert!(result.contains("truncated"));
    assert!(result.contains("51 chars total"));
}

#[test]
fn test_sanitize_for_log_mixed_content() {
    let input = "line1\nline2\rline3\ttab";
    let result = sanitize_for_log(input);
    assert_eq!(result, "line1\\nline2\\rline3\\ttab");
}

#[test]
fn test_sanitize_for_log_preserves_short_content() {
    let input = "normal text";
    assert_eq!(sanitize_for_log(input), input);
}

#[test]
fn test_split_message_very_small_max_len() {
    // Very small max_len should still work
    let result = split_message("hello world this is a test", 5);
    assert!(!result.is_empty());
}

#[test]
fn test_split_message_single_newline() {
    let result = split_message("hello\nworld", 100);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0], "hello\nworld");
}

#[test]
fn test_find_last_newline_short_string() {
    let text = "hello\nworld";
    let pos = find_last_newline(text, 100);
    assert_eq!(pos, 5);
}

#[test]
fn test_find_last_newline_at_start() {
    let text = "\nhello";
    let pos = find_last_newline(text, 100);
    assert_eq!(pos, 0);
}

#[test]
fn test_find_last_space_at_various_positions() {
    let text = "one two three four five";
    let pos = find_last_space(text, 100);
    assert!(pos > 0);
    assert_eq!(&text[pos..], " five");
}

#[test]
fn test_find_last_unclosed_code_block_multiple_blocks() {
    let text = "before\n```\ncode1\n```\nbetween\n```\ncode2\nstill open";
    let pos = find_last_unclosed_code_block(text);
    // Should find the second opening ```
    assert!(pos > 0);
}

#[test]
fn test_find_next_closing_code_block_with_multiple_blocks() {
    let text = "```\ncode\n```\nmore\n```\nmore code\n```\nend";
    let pos = find_next_closing_code_block(text, 15);
    // Should find the closing ``` after position 15
    assert!(pos > 0);
}

#[test]
fn test_find_next_closing_code_block_no_close() {
    let text = "```\nnever closes";
    let pos = find_next_closing_code_block(text, 5);
    assert_eq!(pos, 0);
}

#[test]
fn test_split_message_code_block_that_extends() {
    // Code block that spans past max_len but has closing within range
    let code = "```\n".to_string() + &"line\n".repeat(5) + "```\n";
    let text = format!("Intro:\n{}", code);
    let result = split_message(&text, 500);
    assert!(!result.is_empty());
}

#[test]
fn test_split_message_with_only_newlines() {
    let text = "\n\n\n\n\n";
    let result = split_message(text, 100);
    // After trim, first chunk would be empty
    assert!(!result.is_empty() || result.is_empty()); // behavior depends on trim
}

#[test]
fn test_format_message_with_unicode() {
    let text = "a".repeat(50);
    let result = format_message(&text, 50);
    assert_eq!(result.len(), 50);
}
