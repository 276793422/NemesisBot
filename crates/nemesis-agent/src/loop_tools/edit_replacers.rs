//! A4（2026-09-04 devtool-upgrade 阶段 2）：edit_file 模糊替换级联。
//!
//! 多级模糊替换级联（exact 之外的实用子集）：exact 级由
//! `EditFileTool` 自身承担（字节精确 + A1 提示），本模块承接 exact 未命中
//! 且 `!replace_all` 时的五级模糊匹配，每级**唯一命中才接受**：
//!
//! 1. `line-trimmed`：逐行 trim 后整块相等（行数必须一致）；替换时保留
//!    目标行原缩进（模型给的缩进风格不覆盖文件风格）；
//! 2. `block-anchor`：old 首行+尾行 trim 后在文件中**成对**唯一锚定，中段
//!    按非空白字符重合率（Dice ≥0.65）对齐（行数可以不同）；
//! 3. `whitespace-normalized`：行内连续空白折叠为单空格 + 端点 trim 后
//!    整块相等（行数一致）；
//! 4. `indentation-flexible`：按 old 块整体缩进差**平移**匹配（所有行共享
//!    同一缩进 delta），替换时把 delta 应用到 new 的行首；
//! 5. `trimmed-boundary`：剥掉 old 首尾空白行后核心行在文件中逐行精确
//!    唯一匹配（容忍首尾空行差异）。
//!
//! 级联纪律：0 命中进下一级；≥2 命中直接报错（附命中行号）——歧义是模型
//! 的问题，换更松的级只会更错；候选 span 字节 > `old.len()*4 + 1024` 时该
//! 候选被跨度保护拒绝（防模糊对齐吞掉大段文件）；某级候选全部被拒 → 该级
//! 记录 `Disproportionate` 后进下一级，全败时进错误说明。
//!
//! 已知边界（诚实记录）：`indentation-flexible` 的匹配域是 `line-trimmed`
//! 的真子集（行数一致 + 逐行 trim 相等是更松的条件），常规路径下会被
//! line-trimmed 先接住；它只在 line-trimmed 候选被跨度保护拒绝后有机会
//! 生效（原设计同样如此——保留该级是完整性与替换质量的冗余）。
//! `replace_all=true` 不进级联：模糊命中位置的内容各不相同，"全部替换"
//! 语义无法推广；此时 exact 未命中直接走 A1 提示。
//!
//! 换行风格：行 span 替换沿用该 span 首行的终止符（CRLF 文件整体保持
//! CRLF）；new_text 若带与文件不同的换行风格，verbatim 级会按模型字节
//! 落盘（diff 可见，诚实呈现）。

use std::collections::HashMap;

/// 级联成功：替换后的全文 + 命中级名（成功回执注明 `matched via {level}`）。
#[derive(Debug)]
pub(crate) struct FuzzyMatch {
    pub content: String,
    pub level: &'static str,
}

/// 级联失败。
#[derive(Debug)]
pub(crate) enum CascadeError {
    /// 某级多命中——直接报错（消息已含级名与 1-based 行号），不再尝试
    /// 后续级。调用方原样回给模型。
    Ambiguous(String),
    /// 全部级 0 命中（或候选全被跨度保护拒绝）——调用方回退 A1 提示。
    NoMatch { span_note: Option<String> },
}

/// 单级结果。
#[derive(Debug)]
enum LevelOutcome {
    /// 唯一命中（`content` 已是替换后全文）。
    Match { content: String },
    /// 该级 0 命中，进下一级。
    NoCandidate,
    /// 有候选但全部被跨度保护拒绝——记录后进下一级（全败时进错误说明）。
    Disproportionate { span_bytes: usize, limit: usize },
}

/// 跨度保护上限：候选 span 字节 > `old.len()*4 + 1024` 即拒绝。
fn span_limit(old_text: &str) -> usize {
    old_text.len() * 4 + 1024
}

/// 级联入口。见模块级文档。
pub(crate) fn cascade_replace(
    content: &str,
    old_text: &str,
    new_text: &str,
) -> Result<FuzzyMatch, CascadeError> {
    // 空/全空白 old 无锚可依，直接走兜底（也避免 0 行 old 匹配任意位置）。
    if old_text.trim().is_empty() {
        return Err(CascadeError::NoMatch { span_note: None });
    }
    let levels: [&dyn Fn(&str, &str, &str) -> Result<LevelOutcome, String>; 5] = [
        &line_trimmed,
        &block_anchor,
        &whitespace_normalized,
        &indentation_flexible,
        &trimmed_boundary,
    ];
    let names = [
        "line-trimmed",
        "block-anchor",
        "whitespace-normalized",
        "indentation-flexible",
        "trimmed-boundary",
    ];
    let mut disproportion: Option<(usize, usize)> = None;
    for (idx, level) in levels.iter().enumerate() {
        let outcome = match level(content, old_text, new_text) {
            Ok(o) => o,
            // 某级多命中：直接报错，不再尝试后续级。
            Err(msg) => return Err(CascadeError::Ambiguous(msg)),
        };
        match outcome {
            LevelOutcome::Match { content } => {
                return Ok(FuzzyMatch {
                    content,
                    level: names[idx],
                });
            }
            LevelOutcome::NoCandidate => {}
            LevelOutcome::Disproportionate { span_bytes, limit } => {
                disproportion = Some((span_bytes, limit));
            }
        }
    }
    Err(CascadeError::NoMatch {
        span_note: disproportion.map(|(b, l)| {
            format!(
                "Note: a fuzzy candidate was rejected as disproportionate \
                 (matched span {b} bytes > limit {l} bytes).\n"
            )
        }),
    })
}

// ---------------------------------------------------------------------------
// 行基础设施
// ---------------------------------------------------------------------------

/// 文件一行的字节视图。`text` 不含终止符；`term` 是该行终止符
/// （`"\n"` / `"\r\n"` / `""` = EOF 末行无终止符）。
struct FileLine<'a> {
    start: usize,
    /// 含终止符的末字节（= 下一行 start）。
    term_end: usize,
    term: &'static str,
    text: &'a str,
}

fn line_infos(content: &str) -> Vec<FileLine<'_>> {
    let bytes = content.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < content.len() {
        let start = i;
        let mut nl = content.len();
        for (j, b) in bytes.iter().enumerate().skip(start) {
            if *b == b'\n' {
                nl = j;
                break;
            }
        }
        let (term, content_end, term_end) = if nl == content.len() {
            ("", nl, nl)
        } else if nl > start && bytes[nl - 1] == b'\r' {
            ("\r\n", nl - 1, nl + 1)
        } else {
            ("\n", nl, nl + 1)
        };
        out.push(FileLine {
            start,
            term_end,
            term,
            text: &content[start..content_end],
        });
        i = term_end;
    }
    out
}

fn leading_ws(s: &str) -> &str {
    let end = s.find(|c: char| !c.is_whitespace()).unwrap_or(s.len());
    &s[..end]
}

fn is_blank(s: &str) -> bool {
    s.trim().is_empty()
}

/// 歧义错误消息（附 1-based 起始行号）。
fn ambiguous(level: &'static str, start_lines: &[usize]) -> String {
    format!(
        "old_text matched {} times via {level} matching (at lines {:?}); \
         add surrounding context to make it unambiguous",
        start_lines.len(),
        start_lines
    )
}

/// 用 `new_lines`（逐行，无终止符）替换文件行 span `[first..=last]`，
/// 行间连接沿用 span 首行的终止符风格；原 span **末行**带终止符且
/// new_lines 非空时补末行终止符（保持后续行不粘连；EOF 末行无终止符则
/// 不加——替换不引入多余换行）。
fn splice_span(
    content: &str,
    lines: &[FileLine<'_>],
    first: usize,
    last: usize,
    new_lines: &[String],
) -> String {
    let span_start = lines[first].start;
    let span_end = lines[last].term_end;
    let join_term = lines[first].term;
    let mut text = new_lines.join(join_term);
    if !new_lines.is_empty() && !lines[last].term.is_empty() {
        text.push_str(lines[last].term);
    }
    let mut out = String::with_capacity(content.len() + text.len());
    out.push_str(&content[..span_start]);
    out.push_str(&text);
    out.push_str(&content[span_end..]);
    out
}

/// 候选唯一性 + 跨度保护。`cands` 是 `(0-based 起始行, span 字节数)`；
/// span 字节由各 level 按自己的候选块行数算好。
/// 全部候选超限 → `Disproportionate`；无候选 → `NoCandidate`；唯一 →
/// `build(起始行)` 构造替换结果；多候选 → `Err(歧义消息)`。
fn accept_unique_spans(
    level: &'static str,
    cands: Vec<(usize, usize)>,
    old_text: &str,
    build: impl FnOnce(usize) -> Result<LevelOutcome, String>,
) -> Result<LevelOutcome, String> {
    let limit = span_limit(old_text);
    let mut rejected: Option<usize> = None;
    let mut within: Vec<usize> = Vec::new();
    for (i, span_bytes) in cands {
        if span_bytes > limit {
            rejected = Some(rejected.map_or(span_bytes, |b| b.max(span_bytes)));
            continue;
        }
        within.push(i);
    }
    match within.len() {
        0 => Ok(match rejected {
            Some(b) => LevelOutcome::Disproportionate {
                span_bytes: b,
                limit,
            },
            None => LevelOutcome::NoCandidate,
        }),
        1 => build(within[0]),
        _ => {
            let starts: Vec<usize> = within.iter().map(|i| i + 1).collect();
            Err(ambiguous(level, &starts))
        }
    }
}

/// 同块行数级（line-trimmed / whitespace-normalized / indentation-flexible /
/// trimmed-boundary 核心）共用的候选枚举：从 `first` 起连续 `block_len` 行
/// 逐行满足 `pred`。
fn same_count_candidates(
    lines: &[FileLine<'_>],
    block_len: usize,
    pred: impl Fn(&FileLine<'_>, usize) -> bool,
) -> Vec<(usize, usize)> {
    let mut cands = Vec::new();
    if block_len == 0 || block_len > lines.len() {
        return cands;
    }
    'outer: for i in 0..=lines.len() - block_len {
        for k in 0..block_len {
            if !pred(&lines[i + k], k) {
                continue 'outer;
            }
        }
        let span_bytes = lines[i + block_len - 1].term_end - lines[i].start;
        cands.push((i, span_bytes));
    }
    cands
}

// ---------------------------------------------------------------------------
// 级 1：line-trimmed（逐行 trim 整块相等；替换保留目标行原缩进）
// ---------------------------------------------------------------------------

fn line_trimmed(content: &str, old_text: &str, new_text: &str) -> Result<LevelOutcome, String> {
    let old_lines: Vec<&str> = old_text.lines().collect();
    let lines = line_infos(content);
    let old_trim: Vec<&str> = old_lines.iter().map(|l| l.trim()).collect();
    let cands = same_count_candidates(&lines, old_lines.len(), |fl, k| {
        fl.text.trim() == old_trim[k]
    });
    accept_unique_spans("line-trimmed", cands, old_text, |i| {
        // 计划语义：替换保留目标行原缩进——new 各行内容 + 文件对应行的
        // 原缩进（行数一致由匹配条件保证）。
        let rebuilt: Vec<String> = new_text
            .lines()
            .enumerate()
            .map(|(k, l)| format!("{}{}", leading_ws(lines[i + k].text), l.trim_start()))
            .collect();
        Ok(LevelOutcome::Match {
            content: splice_span(content, &lines, i, i + old_lines.len() - 1, &rebuilt),
        })
    })
}

// ---------------------------------------------------------------------------
// 级 2：block-anchor（首尾行成对唯一锚定 + 中段相似度 ≥0.65）
// ---------------------------------------------------------------------------

/// 字符重合率（Dice 系数，**非空白字符**）：2·|多重集交集| / (|a|+|b|)。
/// 空白不计——空白差异是 line-trimmed / whitespace-normalized 级的职责，
/// 计入会系统性低估短块（换行符占比高）的中段相似度。
fn char_similarity(a: &str, b: &str) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let mut counts: HashMap<char, usize> = HashMap::new();
    let mut na = 0usize;
    for c in a.chars() {
        if c.is_whitespace() {
            continue;
        }
        *counts.entry(c).or_insert(0) += 1;
        na += 1;
    }
    let mut inter = 0usize;
    let mut nb = 0usize;
    for c in b.chars() {
        if c.is_whitespace() {
            continue;
        }
        nb += 1;
        if let Some(x) = counts.get_mut(&c)
            && *x > 0
        {
            *x -= 1;
            inter += 1;
        }
    }
    if na + nb == 0 {
        return 1.0;
    }
    2.0 * inter as f64 / (na + nb) as f64
}

/// 中段相似度阈值。
const BLOCK_ANCHOR_SIMILARITY: f64 = 0.65;

fn block_anchor(content: &str, old_text: &str, new_text: &str) -> Result<LevelOutcome, String> {
    let old_lines: Vec<&str> = old_text.lines().collect();
    if old_lines.len() < 2 {
        return Ok(LevelOutcome::NoCandidate);
    }
    let first_anchor = old_lines[0].trim();
    let last_anchor = old_lines[old_lines.len() - 1].trim();
    // 空白行当锚无意义（处处命中 → 必然歧义）。
    if first_anchor.is_empty() || last_anchor.is_empty() {
        return Ok(LevelOutcome::NoCandidate);
    }
    let old_mid: String = old_lines[1..old_lines.len() - 1].join("\n");
    let lines = line_infos(content);
    let n = lines.len();
    let mut cands: Vec<(usize, usize)> = Vec::new(); // (first, last) 0-based
    for i in 0..n {
        if lines[i].text.trim() != first_anchor {
            continue;
        }
        for j in (i + 1)..n {
            if lines[j].text.trim() == last_anchor {
                let file_mid: String = lines[i + 1..j]
                    .iter()
                    .map(|l| l.text)
                    .collect::<Vec<_>>()
                    .join("\n");
                if char_similarity(&file_mid, &old_mid) >= BLOCK_ANCHOR_SIMILARITY {
                    cands.push((i, j));
                }
            }
        }
    }
    let spanned: Vec<(usize, usize)> = cands
        .iter()
        .map(|&(i, j)| (i, lines[j].term_end - lines[i].start))
        .collect();
    accept_unique_spans("block-anchor", spanned, old_text, |idx| {
        let (i, j) = cands[idx];
        let new_lines: Vec<String> = new_text.lines().map(String::from).collect();
        Ok(LevelOutcome::Match {
            content: splice_span(content, &lines, i, j, &new_lines),
        })
    })
}

// ---------------------------------------------------------------------------
// 级 3：whitespace-normalized（行内空白折叠 + 端点 trim 后整块相等）
// ---------------------------------------------------------------------------

fn normalize_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_ws = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !in_ws {
                out.push(' ');
                in_ws = true;
            }
        } else {
            out.push(c);
            in_ws = false;
        }
    }
    out.trim().to_string()
}

fn whitespace_normalized(
    content: &str,
    old_text: &str,
    new_text: &str,
) -> Result<LevelOutcome, String> {
    let old_lines: Vec<&str> = old_text.lines().collect();
    let old_norm: Vec<String> = old_lines.iter().map(|l| normalize_ws(l)).collect();
    let lines = line_infos(content);
    let cands = same_count_candidates(&lines, old_lines.len(), |fl, k| {
        normalize_ws(fl.text) == old_norm[k]
    });
    accept_unique_spans("whitespace-normalized", cands, old_text, |i| {
        let new_lines: Vec<String> = new_text.lines().map(String::from).collect();
        Ok(LevelOutcome::Match {
            content: splice_span(content, &lines, i, i + old_lines.len() - 1, &new_lines),
        })
    })
}

// ---------------------------------------------------------------------------
// 级 4：indentation-flexible（整块统一缩进差平移；delta 应用到 new 行首）
// ---------------------------------------------------------------------------

#[derive(Clone)]
enum IndentDelta {
    /// 文件比 old 多缩 `String`（前缀扩展）。
    Add(String),
    /// 文件比 old 少缩 `String`（old 的前缀被剥掉）。
    Strip(String),
}

/// 由基准行对（old 首个非空白行 ↔ 文件对应行）求缩进差。
/// 前置条件：两行前导空白成前缀关系（调用方先验证）。
fn delta_of_pair(f_ws: &str, o_ws: &str) -> IndentDelta {
    if f_ws.len() >= o_ws.len() && f_ws.starts_with(o_ws) {
        IndentDelta::Add(f_ws[o_ws.len()..].to_string())
    } else {
        IndentDelta::Strip(o_ws[f_ws.len()..].to_string())
    }
}

fn indentation_flexible(
    content: &str,
    old_text: &str,
    new_text: &str,
) -> Result<LevelOutcome, String> {
    let old_lines: Vec<&str> = old_text.lines().collect();
    let lines = line_infos(content);
    let Some(k0) = old_lines.iter().position(|l| !is_blank(l)) else {
        return Ok(LevelOutcome::NoCandidate);
    };
    let mut cands: Vec<(usize, usize)> = Vec::new();
    'outer: for i in 0..=lines.len().saturating_sub(old_lines.len()) {
        if old_lines.len() > lines.len() {
            break;
        }
        let f_ws = leading_ws(lines[i + k0].text);
        let o_ws = leading_ws(old_lines[k0]);
        // 前导空白必须成前缀关系才能定义 delta（tab/空格混排不成前缀 →
        // 该级不适用，留给 line-trimmed）。
        let delta = if f_ws.starts_with(o_ws) || o_ws.starts_with(f_ws) {
            delta_of_pair(f_ws, o_ws)
        } else {
            continue 'outer;
        };
        for (k, ol) in old_lines.iter().enumerate() {
            let f = &lines[i + k];
            match (is_blank(ol), is_blank(f.text)) {
                (true, true) => continue,
                (true, false) | (false, true) => continue 'outer,
                (false, false) => {}
            }
            let f_ws = leading_ws(f.text);
            let o_ws = leading_ws(ol);
            // 同一 delta + 各剥各行自己的前导空白后内容逐行相等。
            let ok = match &delta {
                IndentDelta::Add(extra) => {
                    f_ws.len() == o_ws.len() + extra.len() && f_ws.starts_with(o_ws)
                }
                IndentDelta::Strip(miss) => {
                    o_ws.len() == f_ws.len() + miss.len() && o_ws.starts_with(f_ws)
                }
            } && f.text[f_ws.len()..] == ol[o_ws.len()..];
            if !ok {
                continue 'outer;
            }
        }
        let span_bytes = lines[i + old_lines.len() - 1].term_end - lines[i].start;
        cands.push((i, span_bytes));
    }
    accept_unique_spans("indentation-flexible", cands, old_text, |i| {
        let delta = delta_of_pair(leading_ws(lines[i + k0].text), leading_ws(old_lines[k0]));
        // 把同一 delta 应用到 new 各行首（空白行不动）。
        let rebuilt: Vec<String> = new_text
            .lines()
            .map(|l| {
                if is_blank(l) {
                    return l.to_string();
                }
                match &delta {
                    IndentDelta::Add(extra) => format!("{extra}{l}"),
                    IndentDelta::Strip(miss) => {
                        let ws = leading_ws(l);
                        if ws.len() >= miss.len() && ws.starts_with(miss.as_str()) {
                            l[miss.len()..].to_string()
                        } else {
                            l.to_string()
                        }
                    }
                }
            })
            .collect();
        Ok(LevelOutcome::Match {
            content: splice_span(content, &lines, i, i + old_lines.len() - 1, &rebuilt),
        })
    })
}

// ---------------------------------------------------------------------------
// 级 5：trimmed-boundary（剥 old 首尾空白行，核心行逐行精确唯一匹配）
// ---------------------------------------------------------------------------

fn trimmed_boundary(content: &str, old_text: &str, new_text: &str) -> Result<LevelOutcome, String> {
    let old_lines: Vec<&str> = old_text.lines().collect();
    let (Some(cs), Some(ce)) = (
        old_lines.iter().position(|l| !is_blank(l)),
        old_lines.iter().rposition(|l| !is_blank(l)),
    ) else {
        return Ok(LevelOutcome::NoCandidate);
    };
    let core = &old_lines[cs..=ce];
    let lines = line_infos(content);
    let cands = same_count_candidates(&lines, core.len(), |fl, k| fl.text == core[k]);
    accept_unique_spans("trimmed-boundary", cands, old_text, |i| {
        let new_lines: Vec<String> = new_text.lines().map(String::from).collect();
        Ok(LevelOutcome::Match {
            content: splice_span(content, &lines, i, i + core.len() - 1, &new_lines),
        })
    })
}

#[cfg(test)]
mod tests;
