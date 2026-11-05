//! Shared utility functions.

use uuid::Uuid;

/// Generate a new unique ID.
pub fn new_id() -> String {
    Uuid::new_v4().to_string()
}

/// Generate a correlation ID for RPC messages.
pub fn correlation_id(channel: &str, chat_id: &str) -> String {
    format!("{}:{}", channel, chat_id)
}

/// Format an RPC response with correlation ID prefix.
pub fn format_rpc_response(correlation_id: &str, content: &str) -> String {
    format!("[rpc:{}] {}", correlation_id, content)
}

/// Extract correlation ID from RPC response prefix.
pub fn extract_rpc_correlation_id(content: &str) -> Option<(&str, &str)> {
    if content.starts_with("[rpc:") {
        let end = content.find(']')?;
        let id = &content[5..end];
        let rest = content.get(end + 1..)?.trim_start();
        Some((id, rest))
    } else {
        None
    }
}

/// Get current ISO 8601 timestamp.
pub fn now_timestamp() -> String {
    chrono::Local::now().to_rfc3339()
}

/// Cosine similarity between two f32 vectors, accumulated in f64.
/// 0.0 when lengths differ, either is empty, or either norm is zero
/// (dimension mismatch = incomparable; zero vector = undefined direction).
///
/// Round-5 dedup: THE single cosine implementation — nemesis-memory's
/// `cosine_similarity` and nemesis-providers' router (semantic model
/// routing) both delegate here so a numerics fix lands once.
pub fn cosine_similarity_f32(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f64;
    let mut norm_a = 0.0f64;
    let mut norm_b = 0.0f64;
    for i in 0..a.len() {
        dot += f64::from(a[i]) * f64::from(b[i]);
        norm_a += f64::from(a[i]) * f64::from(a[i]);
        norm_b += f64::from(b[i]) * f64::from(b[i]);
    }
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b).sqrt()
}

/// Returns the largest byte index `i` such that `i <= max_len` and `s.is_char_boundary(i)`.
///
/// Safe to use for prefix slicing: `&s[..floor_char_boundary(s, n)]`
pub fn floor_char_boundary(s: &str, max_len: usize) -> usize {
    if max_len >= s.len() {
        return s.len();
    }
    let mut i = max_len;
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Returns the smallest byte index `i` such that `i >= min_start` and `s.is_char_boundary(i)`.
///
/// Safe to use for suffix slicing: `&s[ceil_char_boundary(s, start)..]`
pub fn ceil_char_boundary(s: &str, min_start: usize) -> usize {
    if min_start == 0 || min_start >= s.len() {
        return min_start.min(s.len());
    }
    let mut i = min_start;
    while !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// Truncate string to max byte length with ellipsis, UTF-8 safe.
///
/// Finds the largest char boundary not exceeding `max_len` before slicing.
/// If the string fits within `max_len` bytes, returns it unchanged.
/// Otherwise returns the longest prefix that fits (with room for "...").
pub fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        return s.to_string();
    }
    // Need at least 3 bytes for "..."
    let budget = max_len.saturating_sub(3);
    if budget == 0 {
        return "...".to_string();
    }
    // Find the largest char boundary <= budget.
    let boundary = s
        .char_indices()
        .take_while(|(idx, ch)| *idx + ch.len_utf8() <= budget)
        .last()
        .map(|(idx, ch)| idx + ch.len_utf8())
        .unwrap_or(0);
    if boundary == 0 {
        "...".to_string()
    } else {
        format!("{}...", &s[..boundary])
    }
}

/// 按字符预算拆分长消息（出站通道平台硬限共用件）。
///
/// 平台出站文本普遍有硬长度上限（telegram 4096 / LINE 5000 / mastodon
/// 500 / bluesky 300 …），超限 = 服务端 400 拒收、整条投递失败。本件
/// 把超长内容拆成多条：
/// - 预算按 **char 计**（不是字节）——CJK 一字一 char，社交平台限值
///   语义是字符数；
/// - 优先在窗口内最后一个 `\n` 切，其次空格，都没有才硬切——不切断
///   词句，且 `rfind`/`char_indices` 切点全部落在 char boundary；
/// - 短于预算原样返回单条；`max_chars == 0` 视为非法预算，不拆。
pub fn split_message(content: &str, max_chars: usize) -> Vec<String> {
    if max_chars == 0 || content.chars().count() <= max_chars {
        return vec![content.to_string()];
    }
    let mut chunks = Vec::new();
    let mut rest = content;
    while rest.chars().count() > max_chars {
        let limit = rest
            .char_indices()
            .nth(max_chars)
            .map(|(i, _)| i)
            .unwrap_or(rest.len());
        let window = &rest[..limit];
        // 切点优先换行、其次空格；filter(>0) 防空首块死循环。
        let cut = window
            .rfind('\n')
            .filter(|&i| i > 0)
            .or_else(|| window.rfind(' ').filter(|&i| i > 0))
            .map(|i| i + 1) // 吃掉分隔符（\n、空格均 1 字节，边界安全）
            .unwrap_or(limit);
        chunks.push(rest[..cut].to_string());
        rest = &rest[cut..];
    }
    if !rest.is_empty() {
        chunks.push(rest.to_string());
    }
    chunks
}

#[cfg(test)]
mod tests;
