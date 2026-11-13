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
/// 平台出站文本普遍有硬长度上限（mastodon 500 / bluesky 300 …），超限
/// = 服务端 400 拒收、整条投递失败。本件把超长内容拆成多条：
/// - 预算按 **char 计**（不是字节）——CJK 一字一 char，社交平台限值
///   语义是字符数；
/// - 优先在窗口内最后一个 `\n` 切，其次空格，都没有才硬切——不切断
///   词句，且 `rfind`/`char_indices` 切点全部落在 char boundary；
/// - 短于预算原样返回单条；`max_chars == 0` 视为非法预算，不拆。
///
/// ⚠ 适用面：限值口径为「字符数 ≈ Unicode 标量」的平台（mastodon/
/// bluesky 按 grapheme 计——标量数 ≥ grapheme 数，本件恒保守安全）。
/// **Telegram / LINE 的限值按 UTF-16 code units 计**（官方文档明确），
/// astral emoji = 2 units——这两个平台必须用 [`split_message_utf16`]，
/// 纯标量计数在 emoji 密集内容上少算至 2×，拆分后单块仍会越限被整条
/// 拒收（BUG-20261113-166）。
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

/// 字符串的 UTF-16 code unit 长度（Telegram / LINE 平台长度口径）。
///
/// Telegram Bot API 与 LINE Messaging API 的文本长度上限均按 UTF-16
/// code units 计（官方文档明确：Telegram entities 文档、LINE
/// text-character-count 文档）。BMP 内字符（含常用 CJK）= 1 unit；
/// astral 平面字符（多数 emoji、CJK 扩展 B+）= 2 units。
pub fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// 按 UTF-16 code unit 预算拆分长消息（Telegram / LINE 专用口径）。
///
/// 两平台「字符」上限的真实口径是 UTF-16 code units 而非 Unicode 标量
/// （[`utf16_len`] 文档）：astral emoji = 2 units，纯标量计数在 emoji
/// 密集内容上少算至 2×——拆分后单块仍可越限被服务端整条拒收
/// （BUG-20261113-166）。与 [`split_message`] 同切点策略（窗口内最后
/// `\n` 优先、其次空格、否则硬切），预算维度换成 UTF-16 单位；
/// `max_units == 0` 视为非法预算不拆，与 [`split_message`] 语义对齐。
pub fn split_message_utf16(content: &str, max_units: usize) -> Vec<String> {
    if max_units == 0 || utf16_len(content) <= max_units {
        return vec![content.to_string()];
    }
    let mut chunks = Vec::new();
    let mut rest = content;
    while utf16_len(rest) > max_units {
        // units ≤ max_units 的最大字节切点：逐 char 累计 len_utf16，
        // 超预算前最后一个 char boundary。预算连首字符都装不下时
        // （如 max_units=1 遇 emoji）整字符让行——单块超预算是该预算
        // 下的不可避免形态，保证循环恒前进。
        let mut units = 0usize;
        let mut limit = rest.len();
        for (i, ch) in rest.char_indices() {
            let u = ch.len_utf16();
            if units + u > max_units {
                limit = i;
                break;
            }
            units += u;
        }
        if limit == 0 {
            let first = rest
                .chars()
                .next()
                .map(char::len_utf8)
                .unwrap_or(rest.len());
            chunks.push(rest[..first].to_string());
            rest = &rest[first..];
            continue;
        }
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
