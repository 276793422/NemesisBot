//! SSE 事件边界定位（单一真相源）。
//!
//! 消费方：nemesis-providers 三 lane（anthropic / http_provider /
//! codex 经 `stream_util` 再导出）+ nemesis-mcp HTTP/SSE 传输。
//! 此前各处内联双形态扫描（`\n\n` + `\r\n\r\n`）——SSE 规范行终止
//! LF/CR/CRLF 三态且**逐行独立**，CR-only 与混合行尾（规范合法）
//! 全部零事件解析：流式回复静默变空直到 EOF 兜底，内容全丢。

/// SSE 事件边界定位：返回 (块终点偏移, 分隔符字节长)。
///
/// 协议分隔符是空行；行终止按规范三态 LF/CR/CRLF **逐行独立**，
/// 扫描按贪婪匹配（`\r\n` 恒作 CRLF 消费，不拆成 CR+LF——与标准
/// SSE 解析器一致）。空行即边界，块 = 最近一个非空行内容终点之前
/// （流头空行则块为空，消费方按无 data 跳过）。
///
/// 半包纪律：尾随 `\r` 可能是 CR 终止符也可能是 CRLF 前半，不可
/// 判定时整段等待下一 chunk（返回 `None`），绝不提前消费——提前
/// 消费会把 CRLF 劈成「CR 边界 + 下一事件头残留 `\n`」。
///
/// 复杂度：逐字节单趟扫描；调用方每次消费边界后剩余缓冲重扫，
/// 与旧 `find` 双扫同阶。
pub fn next_event_boundary(buffer: &str) -> Option<(usize, usize)> {
    let bytes = buffer.as_bytes();
    let mut line_start = 0usize; // 当前行内容起点
    let mut last_content_end = 0usize; // 最近一个非空行的内容终点
    let mut seen_content = false;
    let mut i = 0usize;
    while i < bytes.len() {
        let tlen = match bytes[i] {
            b'\n' => 1usize,
            // 尾随 '\r'：CR 还是 CRLF 前半不可判定 → 整段等待（半包钉）。
            b'\r' if i + 1 == bytes.len() => return None,
            b'\r' if bytes[i + 1] == b'\n' => 2,
            b'\r' => 1,
            _ => {
                i += 1;
                continue;
            }
        };
        if i == line_start {
            // 空行 = 事件边界。块终点 = 最近非空行内容终点（流头空行取 0）。
            let pos = if seen_content { last_content_end } else { 0 };
            return Some((pos, i + tlen - pos));
        }
        last_content_end = i;
        seen_content = true;
        line_start = i + tlen;
        i += tlen;
    }
    None
}

#[cfg(test)]
mod tests;
