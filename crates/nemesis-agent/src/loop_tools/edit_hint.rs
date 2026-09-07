//! A1/A2（2026-09-04 devtool-upgrade 阶段 1）：edit_file 失败反馈 → 修复指令，
//! 成功回 unified diff。
//!
//! 设计立场：所有失败消息都是**给模型的修复指令**——
//! 上下文片段 + 编辑距离最近候选 + 唯一性判定——而不是一句静态文案。
//! 消费方：`loop_tools.rs` 的 `EditFileTool`；`unified_diff`（A2）后续被
//! A5/M2/A6 复用。

use crate::args_validator::edit_distance;

/// 单行预览：trim 行尾空白，超长行按 char boundary 截断（中文安全）。
fn preview_line(line: &str, max_chars: usize) -> String {
    let t = line.trim_end();
    if t.chars().count() <= max_chars {
        t.to_string()
    } else {
        let cut: String = t.chars().take(max_chars).collect();
        format!("{}…", cut)
    }
}

/// 带行号的文件内容预览：≤60 行全量；更长则 head 30 + 省略标记 + tail 15
/// （与实施计划 A1 一致）。超长行截断到 200 字符。
fn numbered_preview(content: &str) -> String {
    let total = content.lines().count();
    let mut out = String::new();
    let show = |out: &mut String, start: usize, end: usize| {
        for (i, line) in content.lines().enumerate().skip(start).take(end - start) {
            out.push_str(&format!("{:>5} | {}\n", i + 1, preview_line(line, 200)));
        }
    };
    if total > 60 {
        show(&mut out, 0, 30);
        out.push_str(&format!("  ... ({} more lines) ...\n", total - 45));
        show(&mut out, total - 15, total);
    } else {
        show(&mut out, 0, total);
    }
    out
}

/// 行尾归一：CRLF→LF + 逐行 trim_end（用于检测「行尾空白/换行风格差异」）。
fn normalize_line_endings_and_trailing(s: &str) -> String {
    s.replace("\r\n", "\n")
        .split('\n')
        .map(|l| l.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

/// A1：`old_text` 未命中时生成给模型的修复指令。
///
/// 组成（按信息量排序）：
/// 1. CRLF 差异提示——old_text 在行尾归一后能命中 = 换行风格不匹配；
/// 2. 行尾空白差异提示——归一（含 trim_end）后能命中 = 尾随空白不匹配；
/// 3. 编辑距离最近候选行（对 old_text 首行算 Levenshtein，阈值
///    `max(3, 首行字符数/3)`，最近 3 条带行号 + 距离）；
/// 4. 文件内容预览（带行号；>60 行 head30+tail15）。
pub(crate) fn build_not_found_hint(content: &str, old_text: &str) -> String {
    let mut hints: Vec<String> = Vec::new();

    // 1) 纯换行风格差异（先于 trim_end 检测，语义更精确）。
    let lf_only = content.replace("\r\n", "\n");
    let old_lf_only = old_text.replace("\r\n", "\n");
    if old_text.contains('\n') && lf_only.contains(&old_lf_only) {
        hints.push(
            "Hint: the file uses CRLF line endings and your old_text matches after \
             normalizing line endings. Retry with a shorter fragment that stays on one \
             line, or copy the text exactly including its line endings."
                .to_string(),
        );
    }

    // 2) 行尾空白差异（归一后命中而归一前未命中——调用方仅在未命中时调用，
    //    这里只需再确认归一命中）。
    if hints.is_empty()
        && normalize_line_endings_and_trailing(content)
            .contains(&normalize_line_endings_and_trailing(old_text))
    {
        hints.push(
            "Hint: your old_text matches only after trimming trailing whitespace. \
             Some lines have trailing spaces — copy the text exactly as-is, or match a \
             shorter unique fragment."
                .to_string(),
        );
    }

    // 3) 编辑距离最近候选行。
    let old_first = old_text.lines().next().unwrap_or("").trim();
    if !old_first.is_empty() {
        let max_dist = std::cmp::max(3, old_first.chars().count() / 3);
        let mut cands: Vec<(usize, usize, &str)> = content
            .lines()
            .enumerate()
            .filter(|(_, l)| !l.trim().is_empty())
            .map(|(i, l)| (edit_distance(old_first, l.trim()), i + 1, l.trim()))
            .filter(|(d, _, _)| *d <= max_dist)
            .collect();
        cands.sort_by_key(|(d, i, _)| (*d, *i));
        cands.truncate(3);
        if !cands.is_empty() {
            let list = cands
                .iter()
                .map(|(d, ln, l)| {
                    format!(
                        "  - line {}: {} (edit distance {})",
                        ln,
                        preview_line(l, 120),
                        d
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            hints.push(format!("Closest matching lines in the file:\n{}", list));
        }
    }

    let mut out = String::new();
    for h in &hints {
        out.push_str(h);
        out.push('\n');
    }
    out.push_str("\nFile content preview (match exactly, including whitespace):\n");
    out.push_str(&numbered_preview(content));
    out
}

/// A1：`old_text` 多处命中时算每个命中的 1-based 行号。
/// `match_indices` 语义与 `matches().count()` 一致（非重叠）。
pub(crate) fn match_line_numbers(content: &str, needle: &str) -> Vec<usize> {
    content
        .match_indices(needle)
        .map(|(idx, _)| content[..idx].matches('\n').count() + 1)
        .collect()
}

/// unified diff 输出超过该字节数时回退统计摘要（全量语义由 loop 层 spill
/// 闸兜底——工具回灌不是变更审计的唯一真相源）。
const DIFF_MAX_BYTES: usize = 4096;

/// unified diff 上下文行数（对齐 `diff -u` 惯例）。
const DIFF_CONTEXT: usize = 3;

/// A2（2026-09-04）：手写简易 unified diff（不引新依赖）。
///
/// 算法：公共前缀/后缀行定位唯一变更窗口 + 3 行上下文 + `-`/`+` 块。
/// 单 hunk 视角（edit_file 每次只替换一处，多窗口场景不存在）。
/// 行尾统一 LF 视角（`str::lines` 剥 `\r`）；无差异返回空串。
/// 超长输出回退统计摘要 `+n / -m lines (diff truncated …)`。
pub(crate) fn unified_diff(path: &str, old: &str, new: &str) -> String {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let (o, n) = (old_lines.len(), new_lines.len());

    // 公共前缀行数
    let mut p = 0;
    while p < o && p < n && old_lines[p] == new_lines[p] {
        p += 1;
    }
    // 公共后缀行数（不得越过前缀，防重叠命中）
    let mut s = 0;
    while s < o - p && s < n - p && old_lines[o - 1 - s] == new_lines[n - 1 - s] {
        s += 1;
    }

    let del = &old_lines[p..o - s];
    let add = &new_lines[p..n - s];
    if del.is_empty() && add.is_empty() {
        return String::new();
    }

    let head_ctx = p.min(DIFF_CONTEXT);
    let tail_ctx = s.min(DIFF_CONTEXT);
    let cs = p - head_ctx; // 0-based hunk 首行
    let old_rows = head_ctx + del.len() + tail_ctx;
    let new_rows = head_ctx + add.len() + tail_ctx;

    let mut out = String::new();
    out.push_str(&format!("--- a/{path}\n+++ b/{path}\n"));
    out.push_str(&format!(
        "@@ -{},{} +{},{} @@\n",
        cs + 1,
        old_rows,
        cs + 1,
        new_rows
    ));
    for l in &old_lines[cs..p] {
        out.push(' ');
        out.push_str(l);
        out.push('\n');
    }
    for l in del {
        out.push('-');
        out.push_str(l);
        out.push('\n');
    }
    for l in add {
        out.push('+');
        out.push_str(l);
        out.push('\n');
    }
    for l in &old_lines[o - s..o - s + tail_ctx] {
        out.push(' ');
        out.push_str(l);
        out.push('\n');
    }

    if out.len() > DIFF_MAX_BYTES {
        return format!(
            "--- a/{path}\n+++ b/{path}\n+{} / -{} lines (diff truncated: {} bytes > \
             {DIFF_MAX_BYTES}; read the file for full content)",
            add.len(),
            del.len(),
            out.len()
        );
    }
    out
}

#[cfg(test)]
mod tests;
