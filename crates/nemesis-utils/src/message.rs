//! Message splitting and formatting utilities.

/// Format a message with truncation.
pub fn format_message(content: &str, max_len: usize) -> String {
    if content.len() <= max_len {
        content.to_string()
    } else {
        // 截断点是算术字节上限，可能落在多字节 UTF-8 字符中间（CJK panic）：
        // 回退到最近的字符边界再切片。
        let end = floor_char_boundary(content, max_len.min(content.len()));
        format!(
            "{}... (truncated, {} chars total)",
            &content[..end],
            content.len()
        )
    }
}

/// 把字节上限 `limit` 回退到 `s` 的最近 UTF-8 字符边界；`limit` 超过
/// `s.len()` 时返回 `s.len()`。
fn floor_char_boundary(s: &str, limit: usize) -> usize {
    if limit >= s.len() {
        return s.len();
    }
    let mut i = limit;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// 可用作切点的上限：在 [`floor_char_boundary`] 基础上保证**恒前进**——
/// 若回退到 0（首字符比上限还大，如 max_len 极小且文本以多字节字符开头），
/// 则取第一个完整字符的结尾，避免切分循环原地打转（代码围栏注入本就允许
/// 轻微超限，见既有测试的宽容断言）。
fn usable_cut(s: &str, limit: usize) -> usize {
    let i = floor_char_boundary(s, limit);
    if i > 0 {
        return i;
    }
    s.char_indices()
        .nth(1)
        .map(|(b, _)| b)
        .unwrap_or_else(|| s.len())
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
        // 算术上限可能落在多字节字符中间（CJK panic 源）：回退到字符边界，
        // 回退到 0 时取第一个完整字符保证循环恒前进。
        effective_limit = usable_cut(&content, effective_limit);

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
                        let inner_limit = usable_cut(&content, max_len - 5);
                        let better_end = find_last_newline(&content[..inner_limit], 200);
                        if better_end > header_end {
                            msg_end = better_end;
                        } else {
                            msg_end = inner_limit;
                        }
                        let chunk = format!(
                            "{}\n```",
                            content[..msg_end].trim_end_matches([' ', '\t', '\n', '\r'])
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
                            msg_end = usable_cut(&content, max_len - 5);
                            let chunk = format!(
                                "{}\n```",
                                content[..msg_end].trim_end_matches([' ', '\t', '\n', '\r'])
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
/// Returns the byte position of the opening ``` or 0 if all code blocks are complete.
/// 按字节扫描：反引号是 ASCII 单字节，字节扫描结果即字节偏移，与调用方的
/// str 切片语义一致。（曾用 chars().collect() 的 char 索引当字节偏移返回，
/// 纯 ASCII 下恰好相等，CJK 文本+围栏混合即错切/panic。）
fn find_last_unclosed_code_block(text: &str) -> usize {
    let bytes = text.as_bytes();
    let mut in_code_block = false;
    let mut last_open_idx: usize = 0;
    let len = bytes.len();

    let mut i = 0;
    while i + 2 < len {
        if bytes[i] == b'`' && bytes[i + 1] == b'`' && bytes[i + 2] == b'`' {
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

/// Find the next closing ``` starting from byte position `start_idx`.
/// Returns the byte position after the closing ``` or 0 if not found.
/// 按字节扫描（同 [`find_last_unclosed_code_block`]，start_idx/返回值均为
/// 字节偏移）。
fn find_next_closing_code_block(text: &str, start_idx: usize) -> usize {
    let bytes = text.as_bytes();
    let mut in_code_block = false;
    let len = bytes.len();
    let start = start_idx.min(len);

    // Determine state at start_idx
    let mut i = 0;
    while i < start {
        if i + 2 < len && bytes[i] == b'`' && bytes[i + 1] == b'`' && bytes[i + 2] == b'`' {
            in_code_block = !in_code_block;
            i += 3;
        } else {
            i += 1;
        }
    }

    // Search from start_idx
    let mut i = start;
    while i < len {
        if i + 2 < len && bytes[i] == b'`' && bytes[i + 1] == b'`' && bytes[i + 2] == b'`' {
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
mod cov_tests;
#[cfg(test)]
mod tests;
