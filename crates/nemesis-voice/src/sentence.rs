//! Sentence splitting for TTS relay — pure functions, cross-platform.
//!
//! 语音 realtime P1（W2 前级）：把 LLM 回复切成适合逐句合成的句子序列。
//! 终结标点（。！？；!?; 换行）必切；ASCII 句点仅在「后随空白且前非数字」时切
//! （避开小数/URL/缩写误切）；超长句再按逗号级标点二次切（首句首声延迟优化）。

/// Terminal (strong) splitters — always split after these.
fn is_terminal(c: char) -> bool {
    matches!(c, '。' | '！' | '？' | '；' | '!' | '?' | ';' | '\n' | '…')
}

/// Secondary splitters for over-long sentences (comma level).
fn is_soft(c: char) -> bool {
    matches!(c, '，' | '、' | '：' | ',')
}

/// Split text into sentences. Terminal punctuation stays attached to its
/// sentence; empty/whitespace-only pieces are dropped.
pub fn split_sentences(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        cur.push(c);
        if is_terminal(c) {
            // '\n' — swallow consecutive newlines (blank lines between paragraphs)
            while chars.peek() == Some(&'\n') {
                chars.next();
            }
            out.push(std::mem::take(&mut cur));
            continue;
        }
        // ASCII period: split only when followed by whitespace/end and not
        // part of a decimal (digit.digit) — avoids "3.5" / "v1.2" splits.
        if c == '.' {
            let next_ws = match chars.peek() {
                None => true,
                Some(n) => n.is_whitespace(),
            };
            let prev_digit = cur.chars().rev().nth(1).is_some_and(|p| p.is_ascii_digit());
            let next_digit = chars
                .peek()
                .is_some_and(|n: &char| !n.is_whitespace() && n.is_ascii_digit());
            if next_ws && !prev_digit && !next_digit {
                out.push(std::mem::take(&mut cur));
            }
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }

    out.into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Split for TTS: terminal-punctuation sentences, then any sentence longer
/// than `max_chars` is further cut at comma-level punctuation (soft split
/// points stay attached; trailing piece may exceed the limit slightly).
pub fn split_for_tts(text: &str, max_chars: usize) -> Vec<String> {
    let mut out = Vec::new();
    for sent in split_sentences(text) {
        if sent.chars().count() <= max_chars {
            out.push(sent);
            continue;
        }
        // over-long: accumulate soft-split pieces up to the budget
        let mut buf = String::new();
        let mut last_soft = None; // byte idx after last soft punct inside buf
        for c in sent.chars() {
            buf.push(c);
            if is_soft(c) {
                last_soft = Some(buf.len());
            }
            if buf.chars().count() >= max_chars {
                let cut = last_soft.unwrap_or(buf.len());
                let head: String = buf.drain(..cut).collect();
                let head = head.trim().to_string();
                if !head.is_empty() {
                    out.push(head);
                }
                last_soft = None;
            }
        }
        let tail = buf.trim().to_string();
        if !tail.is_empty() {
            out.push(tail);
        }
    }
    out
}

#[cfg(test)]
mod sentence_tests;
