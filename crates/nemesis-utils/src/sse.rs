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

/// SSE 流增量 UTF-8 解码：把一个网络 chunk 安全并入行缓冲 `buffer`，
/// `pending` 携带上一 chunk 留下的未完成尾随序列。
///
/// 逐 chunk 独立 `from_utf8_lossy` 会把被 chunk 边界劈开的多字节字符
/// （CJK 3 字节 / emoji 4 字节）替换成 U+FFFD——TCP 分片边界是任意的，
/// 长 CJK 回复几乎每个边界都可能烂字（nemesis-mcp HTTP/SSE 传输
/// BUG-20261109-007 实锤；nemesis-providers 同族缺陷 R38 先修）。
/// 这里分段扫描：尾部**不完整**序列（`error_len()==None`）留待与下一
/// chunk 拼接后整体解码；真坏字节（`error_len()==Some`）当场 lossy
/// 替换后**从坏段之后继续扫**（同一 chunk 里坏字节后面跟着的合法
/// 字符/残尾不受牵连）。`pending` 恒 ≤3 字节，有界。
pub fn push_chunk_incremental(buffer: &mut String, pending: &mut Vec<u8>, bytes: &[u8]) {
    let mut chunk = std::mem::take(pending);
    chunk.extend_from_slice(bytes);
    let mut pos = 0usize;
    loop {
        match std::str::from_utf8(&chunk[pos..]) {
            Ok(s) => {
                buffer.push_str(s);
                pos = chunk.len();
                break;
            }
            Err(e) => {
                // valid_up_to()/error_len() 都相对 &chunk[pos..]，v/n 换算回绝对下标。
                let v = pos + e.valid_up_to();
                match e.error_len() {
                    Some(n) => {
                        // 合法前缀 [pos..v) + 坏段 [v..v+n)（U+FFFD）入缓冲，
                        // 越过坏段继续扫（其后可能还有合法字符/残尾）。
                        buffer.push_str(&String::from_utf8_lossy(&chunk[pos..v + n]));
                        pos = v + n;
                        if pos == chunk.len() {
                            break;
                        }
                    }
                    None => {
                        // 尾部不完整（序列被 chunk 边界劈开）：合法前缀
                        // [pos..v) 入缓冲，残尾 [v..) 留给下一 chunk。
                        buffer.push_str(&String::from_utf8_lossy(&chunk[pos..v]));
                        pos = v;
                        break;
                    }
                }
            }
        }
    }
    *pending = chunk.split_off(pos);
}

#[cfg(test)]
mod tests;
