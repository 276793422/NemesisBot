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
}
