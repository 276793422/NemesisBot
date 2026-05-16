//! Text formatting and alignment utilities for CLI output.
//!
//! Provides box drawing, section titles, and display width calculation
//! with support for wide Unicode characters (CJK, etc.).

/// Calculate the display width of a string.
/// ASCII characters = 1 width, non-ASCII (CJK, etc.) = 2 width.
pub fn get_display_width(s: &str) -> usize {
    s.chars().map(|c| {
        if c.is_ascii() { 1 } else { 2 }
    }).sum()
}

/// Format a centered title inside a box.
/// Returns a String with box drawing characters.
pub fn format_box_title(title: &str, box_width: usize) -> String {
    let top = format!("\u{2554}{}\u{2557}\n", "\u{2550}".repeat(box_width.saturating_sub(2)));
    let title_width = get_display_width(title);
    let inner = box_width.saturating_sub(2);
    let left_pad = inner.saturating_sub(title_width) / 2;
    let right_pad = inner.saturating_sub(title_width).saturating_sub(left_pad);
    let middle = format!("\u{2551}{}{}{}\u{2551}\n", " ".repeat(left_pad), title, " ".repeat(right_pad));
    let bottom = format!("\u{255A}{}\u{255D}\n", "\u{2550}".repeat(box_width.saturating_sub(2)));
    format!("{}{}{}", top, middle, bottom)
}

/// Format a centered section title with underline.
pub fn format_section_title(title: &str, line_width: usize) -> String {
    let title_width = get_display_width(title);
    let left_pad = line_width.saturating_sub(title_width) / 2;
    let right_pad = line_width.saturating_sub(title_width).saturating_sub(left_pad);
    let line = "\u{2550}".repeat(line_width);
    format!("{}\n{}{}{}\n{}\n", line, " ".repeat(left_pad), title, " ".repeat(right_pad), line)
}

/// Format a separator line.
pub fn format_separator(char: &str, width: usize) -> String {
    format!("{}\n", char.repeat(width))
}

/// Format a text box with optional title and content.
pub fn format_box(title: &str, content: &str, box_width: usize) -> String {
    let mut result = String::new();

    // Top border with title
    if !title.is_empty() {
        let title_width = get_display_width(title);
        let padding = box_width.saturating_sub(3).saturating_sub(title_width).saturating_sub(1);
        result.push_str(&format!("\u{250C}\u{2500} {}{}\u{2510}\n", title, "\u{2500}".repeat(padding)));
    } else {
        result.push_str(&format!("\u{250C}{}\u{2510}\n", "\u{2500}".repeat(box_width.saturating_sub(2))));
    }

    // Content
    if !content.is_empty() {
        let content_width = get_display_width(content);
        let padding = box_width.saturating_sub(2).saturating_sub(content_width).saturating_sub(1);
        result.push_str(&format!("\u{2502} {}{}\u{2502}\n", content, " ".repeat(padding)));
    }

    // Bottom border
    result.push_str(&format!("\u{2514}{}\u{2518}\n", "\u{2500}".repeat(box_width.saturating_sub(2))));

    result
}

/// Truncate text to a display width, appending "..." if truncated.
pub fn truncate_text(text: &str, max_width: usize) -> String {
    let width = get_display_width(text);
    if width <= max_width {
        return text.to_string();
    }
    if max_width <= 3 {
        return text.chars().take(max_width).collect();
    }
    let mut result = String::new();
    let mut current_width = 0;
    for c in text.chars() {
        let char_width = if c.is_ascii() { 1 } else { 2 };
        if current_width + char_width + 3 > max_width {
            break;
        }
        result.push(c);
        current_width += char_width;
    }
    result.push_str("...");
    result
}

/// Left-pad text to a display width.
pub fn pad_left(text: &str, width: usize) -> String {
    let text_width = get_display_width(text);
    if text_width >= width {
        return text.to_string();
    }
    format!("{}{}", " ".repeat(width - text_width), text)
}

/// Right-pad text to a display width.
pub fn pad_right(text: &str, width: usize) -> String {
    let text_width = get_display_width(text);
    if text_width >= width {
        return text.to_string();
    }
    format!("{}{}", text, " ".repeat(width - text_width))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_display_width_ascii() {
        assert_eq!(get_display_width("hello"), 5);
        assert_eq!(get_display_width(""), 0);
    }

    #[test]
    fn test_display_width_cjk() {
        // Each CJK character is 2 width
        assert_eq!(get_display_width("\u{96C6}\u{7FA4}\u{72B6}\u{6001}"), 8);
    }

    #[test]
    fn test_display_width_mixed() {
        // "TestAIServer " (13 ASCII + 1 space) + 4 CJK chars (8 width) = 21
        assert_eq!(get_display_width("TestAIServer \u{5E2E}\u{52A9}\u{7CFB}\u{7EDF}"), 21);
    }

    #[test]
    fn test_format_box_title() {
        let result = format_box_title("Cluster Status", 66);
        assert!(result.contains("Cluster Status"));
        assert!(result.contains("\u{2554}")); // top-left corner
        assert!(result.contains("\u{255A}")); // bottom-left corner
    }

    #[test]
    fn test_format_section_title() {
        let result = format_section_title("RPC \u{65E5}\u{5FD7}\u{8BCA}\u{65AD}\u{5DE5}\u{5177}", 53);
        assert!(result.contains("\u{2550}"));
    }

    #[test]
    fn test_format_separator() {
        let result = format_separator("\u{2500}", 60);
        assert!(result.contains("\u{2500}"));
    }

    #[test]
    fn test_format_box() {
        let result = format_box("\u{57FA}\u{7840}\u{54CD}\u{5E94}\u{6A21}\u{578B}", "\u{5FEB}\u{901F}\u{6D4B}\u{8BD5}\u{548C}\u{6D88}\u{606F}\u{9A8C}\u{8BC1}", 62);
        assert!(result.contains("\u{250C}"));
        assert!(result.contains("\u{2514}"));
    }

    #[test]
    fn test_truncate_text_short() {
        assert_eq!(truncate_text("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_text_long() {
        let result = truncate_text("hello world this is long", 10);
        assert!(result.ends_with("..."));
        assert!(get_display_width(&result) <= 10);
    }

    #[test]
    fn test_pad_left() {
        let result = pad_left("hello", 10);
        assert_eq!(result, "     hello");
    }

    #[test]
    fn test_pad_right() {
        let result = pad_right("hello", 10);
        assert_eq!(result, "hello     ");
    }

    #[test]
    fn test_pad_cjk() {
        let result = pad_right("\u{96C6}\u{7FA4}", 10);
        // CJK chars take 2 width each = 4, need 6 spaces
        assert_eq!(get_display_width(&result), 10);
    }

    // --- Unicode / mixed content handling ---

    #[test]
    fn test_display_width_emoji() {
        // Each emoji is non-ASCII, counted as 2 width by get_display_width
        assert_eq!(get_display_width("\u{1F600}"), 2); // grinning face
        assert_eq!(get_display_width("hi\u{1F600}"), 4); // 2 ASCII + 1 emoji(2)
    }

    #[test]
    fn test_display_width_mixed_ascii_cjk_punctuation() {
        // "A" (1) + "\u{3001}" (2, CJK comma) + "B" (1) = 4
        assert_eq!(get_display_width("A\u{3001}B"), 4);
    }

    #[test]
    fn test_format_box_title_unicode() {
        let title = "\u{6D4B}\u{8BD5}Test\u{6D4B}\u{8BD5}";
        let result = format_box_title(title, 30);
        assert!(result.contains(title));
        // Middle line has border chars \u{2551} (each counted as 2 width by get_display_width)
        // but format_box_title pads based on char count of inner space, not display width.
        // Just verify the title is present and structure is correct.
        let lines: Vec<&str> = result.trim_end().lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[1].contains(title));
    }

    #[test]
    fn test_format_box_title_all_cjk() {
        let title = "\u{96C6}\u{7FA4}\u{72B6}\u{6001}";
        let result = format_box_title(title, 20);
        assert!(result.contains(title));
        // Verify structure: 3 lines, title present in middle
        let lines: Vec<&str> = result.trim_end().lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[1].contains(title));
    }

    #[test]
    fn test_format_section_title_unicode() {
        let result = format_section_title("Hello\u{4E16}\u{754C}", 30);
        // "Hello" (5) + 2 CJK (4) = 9 display width
        let lines: Vec<&str> = result.trim_end().lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[1].contains("Hello\u{4E16}\u{754C}"));
        // Separator lines use \u{2550} repeated by char count (30 chars, each 2 display width)
        assert_eq!(lines[0].chars().count(), 30);
        assert_eq!(lines[2].chars().count(), 30);
    }

    #[test]
    fn test_format_separator_unicode_char() {
        // Using a CJK character as separator
        let result = format_separator("\u{25A0}", 5); // black square
        assert_eq!(result.trim_end().chars().count(), 5);
    }

    #[test]
    fn test_truncate_text_unicode() {
        // 4 CJK chars = 8 width, truncate to 5
        let result = truncate_text("\u{96C6}\u{7FA4}\u{72B6}\u{6001}", 5);
        // Should truncate: first CJK (2) + second CJK (2) = 4, plus "..." = 7 > 5
        // So only first CJK fits (2) + "..." would be 5, but 2 + 3 = 5 and 2 + 2 + 3 = 7 > 5
        // Loop: c1(width 2): current=0, 0+2+3=5 <= 5 => push, current=2
        //       c2(width 2): current=2, 2+2+3=7 > 5 => break
        // result = c1 + "..." = display width 2 + 3 = 5
        assert!(result.ends_with("..."));
        assert_eq!(get_display_width(&result), 5);
    }

    #[test]
    fn test_truncate_text_mixed() {
        // "AB\u{96C6}\u{7FA4}CD" = 2+4+2 = 8 width, truncate to 6
        let result = truncate_text("AB\u{96C6}\u{7FA4}CD", 6);
        assert!(result.ends_with("..."));
        assert!(get_display_width(&result) <= 6);
    }

    #[test]
    fn test_pad_left_unicode() {
        // 2 CJK chars = 4 width, pad to 10 => 6 spaces + 2 CJK
        let result = pad_left("\u{96C6}\u{7FA4}", 10);
        assert_eq!(get_display_width(&result), 10);
        assert!(result.starts_with("      "));
    }

    #[test]
    fn test_pad_right_unicode() {
        let result = pad_right("A\u{96C6}B", 10);
        // "A" (1) + CJK (2) + "B" (1) = 4, need 6 spaces
        assert_eq!(get_display_width(&result), 10);
        assert!(result.ends_with("      "));
    }

    // --- Edge cases for very long text in truncate_text ---

    #[test]
    fn test_truncate_text_exact_width() {
        // Text display width exactly equals max_width
        assert_eq!(truncate_text("hello", 5), "hello");
    }

    #[test]
    fn test_truncate_text_one_over() {
        let result = truncate_text("helloX", 5);
        // "hello" = 5, "helloX" = 6 > 5
        // h(1): 0+1+3=4<=5 push, current=1
        // e(1): 1+1+3=5<=5 push, current=2
        // l(1): 2+1+3=6>5 break
        // result = "he..." = width 5
        assert_eq!(result, "he...");
        assert_eq!(get_display_width(&result), 5);
    }

    #[test]
    fn test_truncate_text_very_long_ascii() {
        let long_text = "a".repeat(1000);
        let result = truncate_text(&long_text, 20);
        assert!(result.ends_with("..."));
        assert!(get_display_width(&result) <= 20);
    }

    #[test]
    fn test_truncate_text_very_long_cjk() {
        // 500 CJK chars = 1000 display width
        let long_text: String = "\u{96C6}".repeat(500);
        let result = truncate_text(&long_text, 20);
        assert!(result.ends_with("..."));
        assert!(get_display_width(&result) <= 20);
    }

    #[test]
    fn test_truncate_text_max_width_zero() {
        assert_eq!(truncate_text("hello", 0), "");
    }

    #[test]
    fn test_truncate_text_max_width_one() {
        // max_width <= 3: just take first char
        let result = truncate_text("hello", 1);
        assert_eq!(result, "h");
    }

    #[test]
    fn test_truncate_text_max_width_two() {
        // max_width <= 3: just take first 2 chars
        let result = truncate_text("hello", 2);
        assert_eq!(result, "he");
    }

    #[test]
    fn test_truncate_text_max_width_three() {
        // max_width <= 3: just take first 3 chars
        let result = truncate_text("hello", 3);
        assert_eq!(result, "hel");
    }

    #[test]
    fn test_truncate_text_max_width_four() {
        // max_width > 3: normal truncation with "..."
        let result = truncate_text("hello world", 4);
        // h(1): 0+1+3=4<=4 push, current=1
        // e(1): 1+1+3=5>4 break
        // result = "h..."
        assert_eq!(result, "h...");
    }

    #[test]
    fn test_truncate_text_empty_string() {
        assert_eq!(truncate_text("", 10), "");
    }

    // --- Zero-width or very small width scenarios ---

    #[test]
    fn test_format_box_title_width_zero() {
        // box_width = 0: all inner widths become 0 via saturating_sub
        let result = format_box_title("X", 0);
        // top: ╔╖ (2 chars, inner = 0 dashes)
        // middle: ║X║ (but inner=0, title_width=1, left_pad=0, right_pad=0)
        //   so middle = ║X║ which is 3 display width
        // bottom: ╚╝
        assert!(result.contains("X"));
    }

    #[test]
    fn test_format_box_title_width_two() {
        let result = format_box_title("", 2);
        // inner = 0, title_width = 0, left_pad = 0, right_pad = 0
        // middle = ║║
        let lines: Vec<&str> = result.trim_end().lines().collect();
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn test_format_box_width_zero() {
        let result = format_box("title", "content", 0);
        // box_width=0: all saturating_sub yields 0
        // Should not panic
        assert!(result.contains("title"));
        assert!(result.contains("content"));
    }

    #[test]
    fn test_format_box_width_two() {
        let result = format_box("", "", 2);
        let lines: Vec<&str> = result.trim_end().lines().collect();
        assert_eq!(lines.len(), 2); // only top and bottom, no content line
    }

    #[test]
    fn test_format_section_title_width_zero() {
        let result = format_section_title("Test", 0);
        // line_width=0 => empty separator lines, middle has title with 0 padding
        // With width 0: the format produces a line for title plus two empty lines,
        // but since line_width is 0, separator lines are empty strings.
        // The actual output is: "\nTest\n\n" (empty line, title, empty line, newline)
        let trimmed = result.trim_end();
        assert!(trimmed.contains("Test"));
    }

    #[test]
    fn test_format_separator_width_zero() {
        let result = format_separator("-", 0);
        assert_eq!(result, "\n");
    }

    #[test]
    fn test_pad_left_zero_width() {
        assert_eq!(pad_left("hello", 0), "hello");
    }

    #[test]
    fn test_pad_right_zero_width() {
        assert_eq!(pad_right("hello", 0), "hello");
    }

    // --- format_box with different content types ---

    #[test]
    fn test_format_box_no_title() {
        let result = format_box("", "content", 20);
        // No title: top border is just ╔──────╗
        assert!(!result.contains(" \n"));
        assert!(result.contains("content"));
        // Should not have the " title " part in top line
        let top = result.lines().next().unwrap();
        assert!(top.starts_with('\u{250C}'));
        assert!(top.ends_with('\u{2510}'));
    }

    #[test]
    fn test_format_box_no_content() {
        let result = format_box("Title", "", 20);
        assert!(result.contains("Title"));
        // Should have top, bottom, but no content line
        let lines: Vec<&str> = result.trim_end().lines().collect();
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn test_format_box_no_title_no_content() {
        let result = format_box("", "", 20);
        let lines: Vec<&str> = result.trim_end().lines().collect();
        assert_eq!(lines.len(), 2); // only top and bottom borders
    }

    #[test]
    fn test_format_box_unicode_title() {
        let result = format_box("\u{6807}\u{9898}", "data", 20);
        assert!(result.contains("\u{6807}\u{9898}"));
        assert!(result.contains("data"));
    }

    #[test]
    fn test_format_box_unicode_content() {
        let result = format_box("Title", "\u{6570}\u{636E}", 20);
        assert!(result.contains("\u{6570}\u{636E}"));
    }

    #[test]
    fn test_format_box_mixed_title_and_content() {
        let result = format_box("ID-\u{7F16}\u{53F7}", "\u{503C}=123", 30);
        let lines: Vec<&str> = result.trim_end().lines().collect();
        assert_eq!(lines.len(), 3); // top + content + bottom
    }

    #[test]
    fn test_format_box_multiline_content() {
        // Content with newlines is treated as a single string
        let result = format_box("Title", "line1\nline2", 30);
        assert!(result.contains("line1\nline2"));
    }

    // --- pad_left and pad_right edge cases ---

    #[test]
    fn test_pad_left_wider_than_target() {
        // Text display width already exceeds target
        assert_eq!(pad_left("hello world", 5), "hello world");
    }

    #[test]
    fn test_pad_right_wider_than_target() {
        assert_eq!(pad_right("hello world", 5), "hello world");
    }

    #[test]
    fn test_pad_left_exact_width() {
        assert_eq!(pad_left("hello", 5), "hello");
    }

    #[test]
    fn test_pad_right_exact_width() {
        assert_eq!(pad_right("hello", 5), "hello");
    }

    #[test]
    fn test_pad_left_empty_string() {
        let result = pad_left("", 5);
        assert_eq!(result, "     ");
        assert_eq!(get_display_width(&result), 5);
    }

    #[test]
    fn test_pad_right_empty_string() {
        let result = pad_right("", 5);
        assert_eq!(result, "     ");
        assert_eq!(get_display_width(&result), 5);
    }

    #[test]
    fn test_pad_left_empty_zero_width() {
        assert_eq!(pad_left("", 0), "");
    }

    #[test]
    fn test_pad_right_empty_zero_width() {
        assert_eq!(pad_right("", 0), "");
    }

    #[test]
    fn test_pad_left_cjk_exceeds_width() {
        // 4 CJK = 8 width > 5, returns as-is
        let text = "\u{96C6}\u{7FA4}\u{72B6}\u{6001}";
        assert_eq!(pad_left(text, 5), text);
    }

    #[test]
    fn test_pad_right_cjk_exceeds_width() {
        let text = "\u{96C6}\u{7FA4}\u{72B6}\u{6001}";
        assert_eq!(pad_right(text, 5), text);
    }

    #[test]
    fn test_pad_left_single_cjk() {
        let result = pad_left("\u{96C6}", 5);
        // CJK char = 2 width, need 3 spaces
        assert_eq!(get_display_width(&result), 5);
    }

    #[test]
    fn test_pad_right_single_cjk() {
        let result = pad_right("\u{96C6}", 5);
        assert_eq!(get_display_width(&result), 5);
    }

    // --- format_separator with different widths ---

    #[test]
    fn test_format_separator_various_chars() {
        let result = format_separator("=", 10);
        assert_eq!(result, "==========\n");

        let result = format_separator("*", 5);
        assert_eq!(result, "*****\n");
    }

    #[test]
    fn test_format_separator_width_one() {
        let result = format_separator("=", 1);
        assert_eq!(result, "=\n");
    }

    #[test]
    fn test_format_separator_multi_char_pattern() {
        // Using a multi-char string as the separator unit
        let result = format_separator("ab", 3);
        // "ab".repeat(3) = "ababab"
        assert_eq!(result, "ababab\n");
    }

    #[test]
    fn test_format_separator_cjk_char() {
        let result = format_separator("\u{2500}", 40);
        assert_eq!(result.trim_end().chars().count(), 40);
    }

    #[test]
    fn test_format_separator_large_width() {
        let result = format_separator("-", 500);
        assert_eq!(result.trim_end().chars().count(), 500);
    }

    #[test]
    fn test_format_separator_unicode_unit() {
        // Each repeat uses a 1-char string that is non-ASCII (display width 2)
        let result = format_separator("\u{2588}", 3); // full block
        assert_eq!(result.trim_end().chars().count(), 3);
        assert_eq!(get_display_width(result.trim_end()), 6);
    }
}
