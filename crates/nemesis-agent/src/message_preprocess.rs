//! Message preprocessing — `@文件引用` expansion（I2，devtool-upgrade 阶段 3）。
//!
//! 扫描用户消息里的 `@path` / `@path#L10` / `@path#L10-20` 引用，把目标文件
//! （或行切片）以 `<file_ref>` 块**前置**进消息文本——agent 无需额外的
//! read_file 工具轮次即可看到被引用内容（@引用 + 行号语法）。
//!
//! 语义要点（与多模态附加链同一套纪律）：
//! - **安全 8 层闸**：每个引用 = 一次程序化 `read_file` invocation 走
//!   `SecurityPlugin::execute`（附加与普通 read 同权；拒绝诚实注明）。
//! - **图片让位**：图片扩展名（[`image_path_detector::has_image_extension`]）
//!   的引用静默跳过——同一消息里 attach_turn_images 会把它作为图片附加，
//!   双注入（文本 + 像素）是浪费且有害。
//! - **失败诚实注明**：路径形 token（含分隔符或扩展名）解析失败 →
//!   `[文件引用失败: {path}: 原因]` 注记；**裸词**（`@john` 这类 mention
//!   语义，无分隔符无扩展名）不动不注记——IM 通道里 @提及 不能炸出噪声。
//! - **词中 @ 不算引用**：`user@example.com` 的 `@` 前面是单词字符 → 跳过
//!   （否则每封邮件都会尝试内联 `example.com`）。
//! - **上下文经济**：读入截头 1MB、注入截断 8KB（多字节安全），行号切片
//!   1-based 闭区间；同 (path, lines) 只注入一次。

use std::path::Path;

/// 单个 `<file_ref>` 块内容上限（计划 I2：≤8KB，超限截断 + locator）。
pub const MAX_REF_BYTES: usize = 8 * 1024;
/// 单文件读入上限（截头；行切片与 8KB 截断都只可能用头部）。
const MAX_READ_BYTES: u64 = 1024 * 1024;

/// 一个 @引用（提取产物；路径尚未解析）。
#[derive(Debug, Clone, PartialEq)]
pub struct FileRef {
    /// 含行号语法的原始 token（`src/main.rs#L10-20`）。
    pub raw: String,
    /// 纯路径部分（`src/main.rs`）。
    pub path: String,
    /// 1-based 起始行（含）。
    pub line_start: Option<usize>,
    /// 1-based 结束行（含）；None = 与 line_start 同行或全文件。
    pub line_end: Option<usize>,
}

fn at_file_re() -> &'static regex::Regex {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    // 排除空白 / @ / 反引号（反引号内是代码 span，不是文件引用）。
    RE.get_or_init(|| regex::Regex::new(r"@([^\s@`]+)").unwrap())
}

/// 裸词判定：无路径分隔符也无扩展名点 → mention 语义（`@john`），不算
/// 文件引用（静默不动，不注记失败——IM 通道提及不能炸噪声）。
fn looks_like_path(token: &str) -> bool {
    token.contains('/') || token.contains('\\') || token.contains('.')
}

/// 词中 @ 跳过（邮箱 `user@example.com`、双写 `@@`）：@ 前一个字符是
/// 单词字符或 @ → 不是引用起点。
fn mid_word_at(content: &str, at_index: usize) -> bool {
    content[..at_index]
        .chars()
        .next_back()
        .map(|c| c.is_alphanumeric() || c == '_' || c == '@')
        .unwrap_or(false)
}

/// 从 token 尾部解析 `#L10` / `#L10-20` 行号语法；解析失败整个 token 视为
/// 纯路径（文件名里带 `#L` 的极少见的形态保持原样可读）。
fn split_line_syntax(token: &str) -> (String, Option<usize>, Option<usize>) {
    if let Some(idx) = token.find("#L") {
        let (path, rest) = (&token[..idx], &token[idx + 2..]);
        let parse = |s: &str| s.parse::<usize>().ok().filter(|n| *n > 0);
        let parsed: Option<(usize, Option<usize>)> = match rest.split_once('-') {
            Some((a, b)) => parse(a).zip(parse(b)).map(|(s, e)| (s, Some(e))),
            None => parse(rest).map(|s| (s, None)),
        };
        if let Some((start, end)) = parsed {
            return (path.to_string(), Some(start), end);
        }
    }
    (token.to_string(), None, None)
}

/// 提取消息里的全部 @文件引用（不去重不解析；顺序 = 出现序）。
pub fn extract_file_refs(content: &str) -> Vec<FileRef> {
    let mut refs = Vec::new();
    for cap in at_file_re().captures_iter(content) {
        let at = match cap.get(0) {
            Some(m) => m.start(),
            None => continue,
        };
        if mid_word_at(content, at) {
            continue;
        }
        let raw_token = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let trimmed = raw_token.trim_end_matches(['.', ',', ';', '!', '?', ')', ']']);
        if trimmed.is_empty() || !looks_like_path(trimmed) {
            continue;
        }
        let (path, line_start, line_end) = split_line_syntax(trimmed);
        if path.is_empty() {
            continue;
        }
        refs.push(FileRef {
            raw: trimmed.to_string(),
            path,
            line_start,
            line_end,
        });
    }
    refs
}

/// 多字节安全的截断（`&s[..max]` 在 char boundary 外会 panic——
/// str-slice-multibyte-panic 家族）。
fn truncate_at_char_boundary(s: &str, max: usize) -> (&str, bool) {
    if s.len() <= max {
        return (s, false);
    }
    let mut i = max;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    (&s[..i], true)
}

/// 1-based 闭区间行切片（line_end None = 与 start 同行）。
fn slice_lines(s: &str, start: usize, end: Option<usize>) -> String {
    let end_line = end.unwrap_or(start);
    s.lines()
        .enumerate()
        .filter(|(i, _)| (i + 1) >= start && (i + 1) <= end_line)
        .map(|(_, l)| l)
        .collect::<Vec<_>>()
        .join("\n")
}

/// 单引用安全闸（与 image_attach::gate_and_hash 同一套 invocation 形态：
/// tool_name=read_file，8 层全跑；拒绝时错误串带 layer 前缀）。
#[cfg(feature = "security")]
fn gate_read(
    security: Option<&nemesis_security::pipeline::SecurityPlugin>,
    path: &Path,
    channel: &str,
) -> Result<(), String> {
    if let Some(sec) = security {
        let invocation = nemesis_security::types::ToolInvocation {
            tool_name: "read_file".to_string(),
            args: serde_json::json!({ "path": path.to_string_lossy() }),
            user: String::new(),
            source: channel.to_string(),
            metadata: std::collections::HashMap::new(),
        };
        let (allowed, deny) = sec.execute(&invocation);
        if !allowed {
            let msg = match deny {
                Some(info) => format!("[layer:{}] {}", info.layer, info.summary),
                None => "operation denied by security policy".to_string(),
            };
            return Err(msg);
        }
    }
    Ok(())
}

/// 非 security 构建的同形闸门（管线整体裁剪 = 直通）。
#[cfg(not(feature = "security"))]
fn gate_read(_security: Option<()>, _path: &Path, _channel: &str) -> Result<(), String> {
    Ok(())
}

/// 读文件（截头 [`MAX_READ_BYTES`]）为 UTF-8 文本；审计链追加 path+hash。
#[cfg(feature = "security")]
fn read_head_text(
    security: Option<&nemesis_security::pipeline::SecurityPlugin>,
    path: &Path,
    channel: &str,
) -> Result<String, String> {
    let bytes = read_head_bytes(path)?;
    if let Some(sec) = security
        && let Some(chain) = sec.audit_chain()
    {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let hash = format!("{:x}", hasher.finalize());
        let _ = chain.append(
            "message_preprocess",
            "read_file",
            "",
            channel,
            &path.to_string_lossy(),
            "allowed",
            &format!("file ref inlined; sha256={}", hash),
        );
    }
    String::from_utf8(bytes).map_err(|_| "文件不是有效的 UTF-8 文本".to_string())
}

#[cfg(feature = "security")]
fn read_head_bytes(path: &Path) -> Result<Vec<u8>, String> {
    use std::io::Read;
    // Read::take(self) 按值消费 —— file 不需要 mut。
    let file = std::fs::File::open(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => "文件不存在".to_string(),
        _ => format!("文件无法读取 ({})", e),
    })?;
    let mut buf = Vec::new();
    file.take(MAX_READ_BYTES)
        .read_to_end(&mut buf)
        .map_err(|e| format!("文件无法读取 ({})", e))?;
    Ok(buf)
}

/// 非 security 构建的同形读取（无审计链可记）。
#[cfg(not(feature = "security"))]
fn read_head_text(_security: Option<()>, path: &Path, _channel: &str) -> Result<String, String> {
    let bytes = {
        use std::io::Read;
        // Read::take(self) 按值消费 —— file 不需要 mut。
        let file = std::fs::File::open(path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => "文件不存在".to_string(),
            _ => format!("文件无法读取 ({})", e),
        })?;
        let mut buf = Vec::new();
        file.take(MAX_READ_BYTES)
            .read_to_end(&mut buf)
            .map_err(|e| format!("文件无法读取 ({})", e))?;
        buf
    };
    String::from_utf8(bytes).map_err(|_| "文件不是有效的 UTF-8 文本".to_string())
}

/// 展开消息里的 @文件引用：`<file_ref>` 块前置，原文保持不变。
///
/// - `base`：相对路径解析基准（生产传 workspace 根）。
/// - `security`：生产传 SecurityPlugin（feature=security）；None = 管线未挂
///   直通（与工具调用/图片附加同语义）。
/// - 图片扩展名引用静默让位（同消息 attach_turn_images 处理）；行号越界、
///   读取失败、闸门拒绝 → `[文件引用失败: …]` 注记追加在文本末尾。
pub fn expand_at_files(
    content: &str,
    base: &Path,
    channel: &str,
    #[cfg(feature = "security")] security: Option<&nemesis_security::pipeline::SecurityPlugin>,
    #[cfg(not(feature = "security"))] security: Option<()>,
) -> String {
    let refs = extract_file_refs(content);
    if refs.is_empty() {
        return content.to_string();
    }

    let mut blocks: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<(String, Option<usize>, Option<usize>)> =
        std::collections::HashSet::new();

    for r in refs {
        // 图片让位：同一消息 attach_turn_images 会以像素形态附加，双注入有害。
        if crate::image_path_detector::has_image_extension(&r.path) {
            continue;
        }
        // 同 (path, lines) 只注入一次。
        if !seen.insert((r.path.clone(), r.line_start, r.line_end)) {
            continue;
        }
        let candidate = if Path::new(&r.path).is_absolute() {
            Path::new(&r.path).to_path_buf()
        } else {
            base.join(&r.path)
        };
        if let Err(reason) = gate_read(security, &candidate, channel) {
            notes.push(format!("[文件引用失败: {}: {}]", r.path, reason));
            continue;
        }
        let body = match read_head_text(security, &candidate, channel) {
            Ok(text) => text,
            Err(reason) => {
                notes.push(format!("[文件引用失败: {}: {}]", r.path, reason));
                continue;
            }
        };
        let sliced = match (r.line_start, r.line_end) {
            (Some(start), end) => {
                let slice = slice_lines(&body, start, end);
                if slice.is_empty() {
                    notes.push(format!(
                        "[文件引用失败: {}: 行号 {} 超出文件范围（共 {} 行）]",
                        r.path,
                        start,
                        body.lines().count()
                    ));
                    continue;
                }
                slice
            }
            (None, _) => body,
        };
        let display = candidate.strip_prefix(base).unwrap_or(&candidate).display();
        let (truncated, was_cut) = truncate_at_char_boundary(&sliced, MAX_REF_BYTES);
        let body_out = if was_cut {
            format!("{truncated}\n(已截断，原内容共 {} 字节)", sliced.len())
        } else {
            truncated.to_string()
        };
        let lines_attr = match (r.line_start, r.line_end) {
            (Some(a), Some(b)) => format!(" lines=\"{}-{}\"", a, b),
            (Some(a), None) => format!(" lines=\"{}\"", a),
            (None, _) => String::new(),
        };
        blocks.push(format!(
            "<file_ref path=\"{}\"{}>\n{}\n</file_ref>",
            display, lines_attr, body_out
        ));
    }

    if blocks.is_empty() && notes.is_empty() {
        return content.to_string();
    }
    let mut out = String::new();
    for block in &blocks {
        out.push_str(block);
        out.push_str("\n\n");
    }
    out.push_str(content);
    for note in &notes {
        out.push('\n');
        out.push_str(note);
    }
    out
}

#[cfg(test)]
mod tests;
