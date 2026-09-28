//! Spoken form — deterministic markdown→speech cleaner (pure functions, cross-platform).
//!
//! 语音 realtime P1（W4 L1）：LLM 回复含大量无法转音频的内容（markdown/代码/
//! URL/表格），逐形态确定性清洗成口语文本，TTS 念的是人话。工作于 W2 接力流
//! 的每个 piece（结构级）与句内（行内级）。清洗话术文案对齐计划 §七-6：
//! 「代码已显示在屏幕上」「详细回复已放在屏幕上」「其余内容看屏幕」。

/// 超过该 piece 数触发长度熔断（§七-6：~6 句）。
const MAX_PIECES: usize = 6;
/// 超长句软切预算（chars）。
/// 单句长度上限：超过的句子在切句后按标点二次细分，避免单条 TTS 过长。
pub const MAX_SENTENCE_CHARS: usize = 100;

const SPOKEN_CODE: &str = "代码已显示在屏幕上";
const SPOKEN_CODE_INLINE: &str = "代码见屏幕";
const SPOKEN_TABLE: &str = "表格已发到屏幕，共";
const SPOKEN_LINK: &str = "链接见聊天窗口";
const SPOKEN_FILE: &str = "文件见聊天窗口";
const FUSE_CODE_DENSITY: &str = "详细回复已放在屏幕上";
const SPOKEN_MORE: &str = "其余内容看屏幕";

/// Convert a full assistant reply into an ordered list of speakable pieces.
///
/// 结构级顺序处理：围栏代码块→占位话术、表格→行数话术、其余 prose 切句后
/// 逐句行内清洗。代码密度 ≥50% → 整体熔断为固定话术；piece 超过 [`MAX_PIECES`]
/// 截断补「其余内容看屏幕」。
pub fn spoken_pieces(text: &str) -> Vec<String> {
    if code_density(text) >= 0.5 {
        return vec![FUSE_CODE_DENSITY.to_string()];
    }

    let mut pieces: Vec<String> = Vec::new();
    let mut prose = String::new();

    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            flush_prose(&prose, &mut pieces);
            prose.clear();
            // skip fenced body (unterminated fence → rest of text is code)
            for l in lines.by_ref() {
                if l.trim_start().starts_with("```") {
                    break;
                }
            }
            pieces.push(SPOKEN_CODE.to_string());
            continue;
        }
        if trimmed.starts_with('|') {
            let mut rows = 1usize;
            while lines
                .peek()
                .is_some_and(|l| l.trim_start().starts_with('|'))
            {
                lines.next();
                rows += 1;
            }
            if rows >= 2 {
                flush_prose(&prose, &mut pieces);
                prose.clear();
                pieces.push(format!("{} {} 行", SPOKEN_TABLE, rows));
                continue;
            }
            // single stray '|' line → prose
        }
        prose.push_str(line);
        prose.push('\n');
    }
    flush_prose(&prose, &mut pieces);

    if pieces.len() > MAX_PIECES {
        pieces.truncate(MAX_PIECES);
        pieces.push(SPOKEN_MORE.to_string());
    }
    if pieces.is_empty() {
        let cleaned = clean_sentence(text);
        if !cleaned.is_empty() {
            pieces.push(cleaned);
        }
    }
    pieces
}

fn flush_prose(prose: &str, pieces: &mut Vec<String>) {
    if prose.trim().is_empty() {
        return;
    }
    for sent in crate::sentence::split_for_tts(prose, MAX_SENTENCE_CHARS) {
        if let Some(cleaned) = non_empty(clean_sentence(&sent)) {
            pieces.push(cleaned);
        }
    }
}

fn non_empty(s: String) -> Option<String> {
    if s.trim().is_empty() { None } else { Some(s) }
}

/// Fenced-code chars / total chars. Unterminated fence → rest counts as code.
fn code_density(text: &str) -> f32 {
    let total = text.chars().count();
    if total == 0 {
        return 0.0;
    }
    let mut in_fence = false;
    let mut code_chars = 0usize;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            code_chars += line.chars().count() + 1;
        }
    }
    code_chars as f32 / total as f32
}

/// Clean one sentence into spoken form (inline level).
pub fn clean_sentence(s: &str) -> String {
    let mut out = s.to_string();
    out = replace_md_links(&out);
    out = replace_urls(&out);
    out = replace_inline_code(&out);
    out = replace_paths(&out);
    out = strip_emphasis(&out);
    out = strip_line_markers(&out);
    out = strip_symbols(&out);
    collapse_spaces(&out).trim().to_string()
}

/// `[text](url)` → `text（链接见聊天窗口）`
fn replace_md_links(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            // [text](url)
            if let Some(close) = find_byte(bytes, b']', i + 1)
                && bytes.get(close + 1) == Some(&b'(')
                && let Some(end) = find_byte(bytes, b')', close + 2)
            {
                let label = &s[i + 1..close];
                if !label.is_empty() {
                    out.push_str(label);
                    out.push('（');
                    out.push_str(SPOKEN_LINK);
                    out.push('）');
                    i = end + 1;
                    continue;
                }
            }
        }
        // advance one char (UTF-8 safe)
        let ch_len = s[i..].chars().next().map_or(1, |c| c.len_utf8());
        out.push_str(&s[i..i + ch_len]);
        i += ch_len;
    }
    out
}

fn find_byte(hay: &[u8], needle: u8, from: usize) -> Option<usize> {
    hay[from..]
        .iter()
        .position(|&b| b == needle)
        .map(|p| p + from)
}

/// Bare http(s) URLs → （链接见聊天窗口）
fn replace_urls(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(pos) = rest.find("http://").or_else(|| rest.find("https://")) {
        out.push_str(&rest[..pos]);
        let tail = &rest[pos..];
        let end = tail
            .char_indices()
            .find(|(_, c)| {
                c.is_whitespace()
                    || matches!(c, ')' | '）' | '」' | '》' | '，' | '。' | '！' | '？')
            })
            .map(|(i, _)| i)
            .unwrap_or(tail.len());
        out.push('（');
        out.push_str(SPOKEN_LINK);
        out.push('）');
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// `` `code` `` → keep simple identifiers verbatim, else 「代码见屏幕」.
fn replace_inline_code(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(open) = rest.find('`') {
        out.push_str(&rest[..open]);
        match rest[open + 1..].find('`') {
            Some(close_rel) => {
                let code = &rest[open + 1..open + 1 + close_rel];
                if is_simple_identifier(code) {
                    out.push_str(code);
                } else {
                    out.push_str(SPOKEN_CODE_INLINE);
                }
                rest = &rest[open + 1 + close_rel + 1..];
            }
            None => {
                // unmatched backtick — keep as-is
                out.push('`');
                rest = &rest[open + 1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn is_simple_identifier(s: &str) -> bool {
    !s.is_empty()
        && s.chars().count() <= 24
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/'))
}

/// Path-like spans → （文件见聊天窗口）。反斜杠几乎只出现在 Windows 路径里；
/// 正斜杠要求 `./` `../` `~/` 或行首 `/` 开头才判定（避开「和/或」）。
fn replace_paths(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let is_path_char = |c: char| {
        c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '\\' | '~' | ':')
    };

    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' || c == '/' {
            // expand left/right over path chars
            let mut lo = i;
            while lo > 0 && is_path_char(chars[lo - 1]) {
                lo -= 1;
            }
            let mut hi = i + 1;
            while hi < chars.len() && is_path_char(chars[hi]) {
                hi += 1;
            }
            // trim drive-prefix ':' noise like "在C:\x" — leading CJK already stops expansion
            let span: String = chars[lo..hi].iter().collect();
            let looks_like_path = span.contains('\\')
                || span.starts_with("./")
                || span.starts_with("../")
                || span.starts_with("~/")
                || (span.len() > 2 && span.starts_with('/'));
            if looks_like_path {
                out.push('（');
                out.push_str(SPOKEN_FILE);
                out.push('）');
                i = hi;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// `**x**` `__x__` `*x*` `~~x~~` → x（内容不含空白/下划线约束防 snake_case 误伤）
fn strip_emphasis(s: &str) -> String {
    let mut out = s.to_string();
    for marker in ["**", "~~"] {
        while let Some(open) = out.find(marker) {
            match out[open + marker.len()..].find(marker) {
                Some(close_rel) if close_rel > 0 => {
                    let inner = &out[open + marker.len()..open + marker.len() + close_rel];
                    let replacement = format!("{}{}", &out[..open], inner);
                    out = replacement + &out[open + marker.len() + close_rel + marker.len()..];
                }
                _ => break,
            }
        }
    }
    // single-char emphasis: *x* — inner must be compact (no spaces).
    // `_x_` 斜体不处理：下划线配对会把 snake_case 标识符当强调剥掉
    // （markdown 规范本身也不建议词内下划线斜体；LLM 输出斜体几乎全是 *x* 形态）。
    strip_paired(&mut out, '*', '*', |inner| {
        !inner.is_empty() && !inner.contains(' ')
    });
    out
}

fn strip_paired(s: &mut String, open: char, close: char, keep: impl Fn(&str) -> bool) {
    let mut result = String::with_capacity(s.len());
    let mut rest = s.as_str();
    loop {
        match rest.find(open) {
            Some(o) if rest[o + open.len_utf8()..].contains(close) => {
                let after = &rest[o + open.len_utf8()..];
                let c_rel = after.find(close).unwrap();
                let inner = &after[..c_rel];
                if keep(inner) {
                    result.push_str(&rest[..o]);
                    result.push_str(inner);
                    rest = &after[c_rel + close.len_utf8()..];
                } else {
                    let head_len = o + open.len_utf8();
                    result.push_str(&rest[..head_len]);
                    rest = &rest[head_len..];
                }
            }
            _ => {
                result.push_str(rest);
                break;
            }
        }
    }
    *s = result;
}

/// 行首形态剥离：标题 `#+`、列表 `-`/`*`/`+`/`1.`、引用 `>`（句子已按行切，
/// 句首即行首）。
fn strip_line_markers(s: &str) -> String {
    let mut out = s.trim_start().to_string();
    loop {
        if out.starts_with('#') {
            out = out.trim_start_matches('#').trim_start().to_string();
            continue;
        }
        if out.starts_with("> ") {
            out = out[2..].to_string();
            continue;
        }
        for m in ["- ", "* ", "+ "] {
            if let Some(stripped) = out.strip_prefix(m) {
                out = stripped.to_string();
            }
        }
        // ordered list "12. " / "12) "
        let digits = out.chars().take_while(|c| c.is_ascii_digit()).count();
        if digits > 0 {
            let rest = &out[digits..];
            if rest.starts_with(". ") || rest.starts_with(") ") {
                out = rest[2..].to_string();
            }
        }
        break;
    }
    // hr line
    if out.chars().all(|c| c == '-' || c == ' ') && out.contains('-') {
        return String::new();
    }
    out
}

/// emoji / 符号剥离（替换为空格防粘连）。
fn strip_symbols(s: &str) -> String {
    s.chars()
        .map(|c| {
            let u = c as u32;
            let symbol = (0x1F000..=0x1FFFF).contains(&u)
                || (0x2600..=0x27BF).contains(&u)
                || (0x2B00..=0x2BFF).contains(&u)
                || (0x2190..=0x21FF).contains(&u)
                || (0xFE00..=0xFE0F).contains(&u)
                || u == 0x200D;
            if symbol { ' ' } else { c }
        })
        .collect()
}

fn collapse_spaces(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !prev_space {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    out
}

#[cfg(test)]
mod spoken_form_tests;
