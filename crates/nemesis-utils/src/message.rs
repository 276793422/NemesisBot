//! Message splitting and formatting utilities.

/// Format a message with truncation.
pub fn format_message(content: &str, max_len: usize) -> String {
    if content.len() <= max_len {
        content.to_string()
    } else {
        let end = max_len.min(content.len());
        format!(
            "{}... (truncated, {} chars total)",
            &content[..end],
            content.len()
        )
    }
}

/// Sanitize message content for logging.
pub fn sanitize_for_log(content: &str) -> String {
    content
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
        .chars()
        .take(200)
        .collect()
}

/// Split long messages into chunks, preserving code block integrity.
/// The function reserves a buffer (10% of max_len, min 50) to leave room for closing code blocks.
/// Returns a vector of message chunks that each respect max_len and avoid splitting fenced code blocks.
pub fn split_message(content: &str, max_len: usize) -> Vec<String> {
    let mut messages = Vec::new();
    let mut content = content.to_string();

    // Dynamic buffer: 10% of max_len, but at least 50 chars if possible
    let mut code_block_buffer = max_len / 10;
    if code_block_buffer < 50 {
        code_block_buffer = 50;
    }
    if code_block_buffer > max_len / 2 {
        code_block_buffer = max_len / 2;
    }

    while !content.is_empty() {
        if content.len() <= max_len {
            messages.push(content.clone());
            break;
        }

        // Effective split point: max_len minus buffer
        let mut effective_limit = max_len - code_block_buffer;
        if effective_limit < max_len / 2 {
            effective_limit = max_len / 2;
        }

        // Find natural split point within the effective limit
        let mut msg_end = find_last_newline(&content[..effective_limit], 200);
        if msg_end == 0 {
            msg_end = find_last_space(&content[..effective_limit], 100);
        }
        if msg_end == 0 {
            msg_end = effective_limit;
        }

        // Check if this would end with an incomplete code block
        let candidate = &content[..msg_end];
        let unclosed_idx = find_last_unclosed_code_block(candidate);

        if unclosed_idx > 0 {
            // Message would end with incomplete code block
            if content.len() > msg_end {
                let closing_idx = find_next_closing_code_block(&content, msg_end);
                if closing_idx > 0 && closing_idx <= max_len {
                    // Extend to include the closing ```
                    msg_end = closing_idx;
                } else {
                    // Code block too long; split inside with closing/reopening fences
                    let header_end = content[unclosed_idx..]
                        .find('\n')
                        .map(|i| unclosed_idx + i)
                        .unwrap_or(unclosed_idx + 3);
                    let header = content[unclosed_idx..header_end].trim();

                    if msg_end > header_end + 20 {
                        let inner_limit = max_len - 5;
                        let better_end = find_last_newline(&content[..inner_limit], 200);
                        if better_end > header_end {
                            msg_end = better_end;
                        } else {
                            msg_end = inner_limit;
                        }
                        let chunk = format!(
                            "{}\n```",
                            content[..msg_end].trim_end_matches(|c| c == ' '
                                || c == '\t'
                                || c == '\n'
                                || c == '\r')
                        );
                        messages.push(chunk);
                        content = format!("{}\n{}", header, content[msg_end..].trim());
                        continue;
                    }

                    // Try to split before the code block starts
                    let new_end = find_last_newline(&content[..unclosed_idx], 200);
                    if new_end > 0 {
                        msg_end = new_end;
                    } else {
                        let new_end2 = find_last_space(&content[..unclosed_idx], 100);
                        if new_end2 > 0 {
                            msg_end = new_end2;
                        } else if unclosed_idx > 20 {
                            msg_end = unclosed_idx;
                        } else {
                            msg_end = max_len - 5;
                            let chunk = format!(
                                "{}\n```",
                                content[..msg_end].trim_end_matches(|c| c == ' '
                                    || c == '\t'
                                    || c == '\n'
                                    || c == '\r')
                            );
                            messages.push(chunk);
                            content = format!("{}\n{}", header, content[msg_end..].trim());
                            continue;
                        }
                    }
                }
            }
        }

        if msg_end == 0 {
            msg_end = effective_limit;
        }

        messages.push(content[..msg_end].to_string());
        content = content[msg_end..].trim().to_string();
    }

    messages
}

/// Find the last opening ``` that doesn't have a closing ```.
/// Returns the position of the opening ``` or 0 if all code blocks are complete.
fn find_last_unclosed_code_block(text: &str) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let mut in_code_block = false;
    let mut last_open_idx: usize = 0;
    let len = chars.len();

    let mut i = 0;
    while i < len {
        if i + 2 < len && chars[i] == '`' && chars[i + 1] == '`' && chars[i + 2] == '`' {
            if !in_code_block {
                last_open_idx = i;
            }
            in_code_block = !in_code_block;
            i += 3;
        } else {
            i += 1;
        }
    }

    if in_code_block { last_open_idx } else { 0 }
}

/// Find the next closing ``` starting from a position.
/// Returns the position after the closing ``` or 0 if not found.
fn find_next_closing_code_block(text: &str, start_idx: usize) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let mut in_code_block = false;
    let len = chars.len();

    // Determine state at start_idx
    let mut i = 0;
    while i < start_idx.min(len) {
        if i + 2 < len && chars[i] == '`' && chars[i + 1] == '`' && chars[i + 2] == '`' {
            in_code_block = !in_code_block;
            i += 3;
        } else {
            i += 1;
        }
    }

    // Search from start_idx
    let mut i = start_idx;
    while i < len {
        if i + 2 < len && chars[i] == '`' && chars[i + 1] == '`' && chars[i + 2] == '`' {
            in_code_block = !in_code_block;
            if !in_code_block {
                return i + 3;
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    0
}

/// Find the last newline character within the last N characters.
/// Returns the byte position of the newline or 0 if not found.
fn find_last_newline(s: &str, search_window: usize) -> usize {
    let search_start = if s.len() > search_window {
        s.len() - search_window
    } else {
        0
    };
    for i in (search_start..s.len()).rev() {
        if s.as_bytes()[i] == b'\n' {
            return i;
        }
    }
    0
}

/// Find the last space character within the last N characters.
/// Returns the byte position of the space or 0 if not found.
fn find_last_space(s: &str, search_window: usize) -> usize {
    let search_start = if s.len() > search_window {
        s.len() - search_window
    } else {
        0
    };
    for i in (search_start..s.len()).rev() {
        let b = s.as_bytes()[i];
        if b == b' ' || b == b'\t' {
            return i;
        }
    }
    0
}

#[cfg(test)]
mod tests;
