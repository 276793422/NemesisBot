//! Text formatting and alignment utilities for CLI output.
//!
//! Provides box drawing, section titles, and display width calculation
//! with support for wide Unicode characters (CJK, etc.).

/// Calculate the display width of a string.
/// ASCII characters = 1 width, non-ASCII (CJK, etc.) = 2 width.
pub fn get_display_width(s: &str) -> usize {
    s.chars().map(|c| if c.is_ascii() { 1 } else { 2 }).sum()
}

/// Format a centered title inside a box.
/// Returns a String with box drawing characters.
pub fn format_box_title(title: &str, box_width: usize) -> String {
    let top = format!(
        "\u{2554}{}\u{2557}\n",
        "\u{2550}".repeat(box_width.saturating_sub(2))
    );
    let title_width = get_display_width(title);
    let inner = box_width.saturating_sub(2);
    let left_pad = inner.saturating_sub(title_width) / 2;
    let right_pad = inner.saturating_sub(title_width).saturating_sub(left_pad);
    let middle = format!(
        "\u{2551}{}{}{}\u{2551}\n",
        " ".repeat(left_pad),
        title,
        " ".repeat(right_pad)
    );
    let bottom = format!(
        "\u{255A}{}\u{255D}\n",
        "\u{2550}".repeat(box_width.saturating_sub(2))
    );
    format!("{}{}{}", top, middle, bottom)
}

/// Format a centered section title with underline.
pub fn format_section_title(title: &str, line_width: usize) -> String {
    let title_width = get_display_width(title);
    let left_pad = line_width.saturating_sub(title_width) / 2;
    let right_pad = line_width
        .saturating_sub(title_width)
        .saturating_sub(left_pad);
    let line = "\u{2550}".repeat(line_width);
    format!(
        "{}\n{}{}{}\n{}\n",
        line,
        " ".repeat(left_pad),
        title,
        " ".repeat(right_pad),
        line
    )
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
        let padding = box_width
            .saturating_sub(3)
            .saturating_sub(title_width)
            .saturating_sub(1);
        result.push_str(&format!(
            "\u{250C}\u{2500} {}{}\u{2510}\n",
            title,
            "\u{2500}".repeat(padding)
        ));
    } else {
        result.push_str(&format!(
            "\u{250C}{}\u{2510}\n",
            "\u{2500}".repeat(box_width.saturating_sub(2))
        ));
    }

    // Content
    if !content.is_empty() {
        let content_width = get_display_width(content);
        let padding = box_width
            .saturating_sub(2)
            .saturating_sub(content_width)
            .saturating_sub(1);
        result.push_str(&format!(
            "\u{2502} {}{}\u{2502}\n",
            content,
            " ".repeat(padding)
        ));
    }

    // Bottom border
    result.push_str(&format!(
        "\u{2514}{}\u{2518}\n",
        "\u{2500}".repeat(box_width.saturating_sub(2))
    ));

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
mod tests;
