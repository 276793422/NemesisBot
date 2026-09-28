//! P30（WS14）Canvas widget —— agent 侧 ```canvas 块检出 + JS 语法预检。
//!
//! agent 终答里的 ```canvas fenced 块经本模块检出并预检后，由 AgentLoop
//! 发布 [`nemesis_types::agent::AgentEvent::CanvasOpen`]（web pump 转 SSE
//! `canvas.open`），前端 CanvasPanel 以 iframe 渲染。语法不过的块不发布，
//! 错误明细回灌模型自纠（`CanvasScan::feedback_text`，接入点在
//! `loop/run_loop.rs::judge_final_answer` 的 Accept 分支）。
//!
//! # v1 契约（Canvas 面板运行时语义，前端 CanvasPanel.vue 同文执行）
//!
//! - **完全无网络**：canvas 块在前端 iframe（`sandbox="allow-scripts"`，
//!   **无** `allow-same-origin`）+ 严格 CSP（`default-src 'none'`）里渲染
//!   ——fetch/XHR/WebSocket/图片/字体/外链脚本全部被 CSP 拦截，沙盒再断
//!   同源 DOM 访问。因此**静态数据必须内联**：允许 `<script
//!   type="application/json">` 数据岛（本模块预检跳过其内容，前端 srcdoc
//!   注入时原样保留），数据面不设任何运行时获取通道。
//! - **语法预检是轻量平衡检查，不是完整 parser**（诚实边界）：按计划评估
//!   过 Rust 侧 JS parser crate——`boa_engine` 是完整 JS 引擎（重），
//!   `swc_ecma_parser` 依赖树庞大（中偏重），对 v1 预检都过重，且 workspace
//!   有 IoT 裁剪叙事（iotsmall profile ~10MB），不愿为一个预检引入重量级
//!   依赖。降级方案为自写平衡检查（括号/引号/模板字面量/注释/正则字面量，
//!   报错带行列号，约 300 行）：**合法 JS 不误报的保证只到「平衡面」**，
//!   非平衡类语法错误（如 `const const`）检不出——该边界随回灌文本如实
//!   告知模型；漏网错误只能在浏览器 devtools 控制台看到（沙盒 iframe 的
//!   报错不会进宿主控制台）。
//! - 检出层选择：挂在 nemesis-agent 主循环终答判定处（而非 nemesis-web），
//!   因为错误回灌需要 loop 的续轮机制，且 AgentEvent 广播是 agent→web 的
//!   既有桥（RoundText/SessionCreated 同款）。agent_event_tx 未装配
//!   （CLI / B 端 worker）时事件自然 no-op，canvas 块以普通代码块形态留在
//!   回复文本里。

use std::fmt::Write as _;

/// 同一 turn 内 canvas 语法重试上限（防模型反复产出残缺 canvas 烧轮）。
/// 耗尽后诚实放行：终答照常送达（canvas 块留在正文里），不再发布事件。
pub const MAX_SYNTAX_RETRIES: u32 = 2;

/// 单条问题的行列定位（`line` 语义随容器标注：[`JsSyntaxIssue`] 相对 JS
/// 片段首行；[`CanvasIssue`] 相对整条回复首行——列号两者一致，1 起按字符
/// 计（非字节，多字节安全））。
#[derive(Debug, Clone)]
pub struct JsSyntaxIssue {
    /// 1 起行号（相对 JS 片段文本首行）。
    pub line: usize,
    /// 1 起列号（按字符计）。
    pub col: usize,
    pub message: String,
}

/// 块级问题（行列号已换算为整条回复的绝对行号，直接可用于回灌文本）。
#[derive(Debug, Clone)]
pub struct CanvasIssue {
    pub line: usize,
    pub col: usize,
    pub message: String,
}

/// 检出的单个 canvas 块。
#[derive(Debug, Clone)]
pub struct CanvasBlock {
    /// 块内容原文（fence 内，不含围栏行；行内以 \n 连接）。
    pub html: String,
    /// 块内容首行的行号（1 起，相对整条回复）。
    pub start_line: usize,
}

/// 一次终答扫描的完整结果。
#[derive(Debug, Clone, Default)]
pub struct CanvasScan {
    pub blocks: Vec<CanvasBlock>,
    pub issues: Vec<CanvasIssue>,
}

impl CanvasScan {
    /// 无任何问题（可发布）。
    pub fn is_ok(&self) -> bool {
        self.issues.is_empty()
    }

    /// 回灌给模型的修正提示。错误明细截前 8 条防提示膨胀；如实声明预检
    /// 是「轻量平衡检查，非完整语法分析」（见模块文档诚实边界）。
    pub fn feedback_text(&self, retry: u32, max_retry: u32) -> String {
        let mut s = String::new();
        let _ = write!(
            s,
            "[canvas 预检] 你的上一条回复中的 ```canvas 块未通过 JS 语法预检\
             （轻量平衡检查，非完整语法分析）。请重发**完整回复**（其余内容\
             保持不变，仅修正 canvas 块及其围栏）——第 {retry}/{max_retry} 次\
             自动重试机会："
        );
        for iss in self.issues.iter().take(8) {
            let _ = write!(
                s,
                "\n- 第 {} 行第 {} 列：{}",
                iss.line, iss.col, iss.message
            );
        }
        if self.issues.len() > 8 {
            let _ = write!(s, "\n- …另有 {} 条问题未列出", self.issues.len() - 8);
        }
        s
    }
}

/// 扫描终答里的全部 ```canvas fenced 块并对每块内联 JS 做语法预检。
///
/// 围栏规则（CommonMark 简化版）：开启行 = 行首（≤3 空格缩进）≥3 个
/// 反引号 + 信息串首 token 小写等于 `canvas`；关闭行 = 反引号数 ≥ 开启数
/// 且无信息串。info 串非 canvas 的围栏只参与状态机（其内容不检出也不
/// 预检——嵌套示例不误触发）。EOF 仍未闭合的 canvas 围栏记为块 + 未闭合
/// 问题。
pub fn scan_canvas_blocks(content: &str) -> CanvasScan {
    let mut blocks: Vec<CanvasBlock> = Vec::new();
    let mut issues: Vec<CanvasIssue> = Vec::new();

    // 围栏状态：(开启反引号数, 是否 canvas, 块内容首行号, 已收内容行)
    let mut fence: Option<(usize, bool, usize, Vec<String>)> = None;

    for (idx, raw_line) in content.lines().enumerate() {
        let line_no = idx + 1;
        let trimmed = raw_line.trim_start();
        let bt = trimmed.chars().take_while(|&c| c == '`').count();
        let is_fence_line = bt >= 3;

        let (_, is_canvas, start_line, is_closing) = match &fence {
            None => {
                if is_fence_line {
                    let info = trimmed[bt..].trim();
                    let lang = info
                        .split_whitespace()
                        .next()
                        .unwrap_or("")
                        .to_ascii_lowercase();
                    fence = Some((bt, lang == "canvas", line_no + 1, Vec::new()));
                }
                continue;
            }
            Some((open_len, is_canvas, start_line, _)) => {
                // 关闭行：反引号 ≥ 开启数且余下内容为空（关闭围栏不带 info）。
                let closing = is_fence_line && bt >= *open_len && trimmed[bt..].trim().is_empty();
                (*open_len, *is_canvas, *start_line, closing)
            }
        };

        if is_closing {
            if is_canvas {
                let html = finish_block(&mut fence);
                check_block(&html, start_line, &mut issues);
                blocks.push(CanvasBlock { html, start_line });
            } else {
                fence = None;
            }
            continue;
        }
        if is_canvas && let Some((.., buf)) = fence.as_mut() {
            buf.push(raw_line.to_string());
        }
    }

    // EOF 仍未闭合的 canvas 围栏：块照收（判定「有 canvas 但失败」），
    // 问题定位到开启围栏行（补围栏动作发生在末尾，指向开启行更可定位）。
    if let Some((_, true, start_line, _)) = fence {
        let html = finish_block(&mut fence);
        issues.push(CanvasIssue {
            line: start_line.saturating_sub(1).max(1),
            col: 1,
            message: "canvas 代码块未闭合（缺收尾 ``` 围栏行）——请在块末尾补上围栏".to_string(),
        });
        check_block(&html, start_line, &mut issues);
        blocks.push(CanvasBlock { html, start_line });
    }

    CanvasScan { blocks, issues }
}

/// 取走围栏缓冲内容并复位围栏状态。
fn finish_block(fence: &mut Option<(usize, bool, usize, Vec<String>)>) -> String {
    match fence.take() {
        Some((.., buf)) => buf.join("\n"),
        None => String::new(),
    }
}

/// 对单个块跑 JS 预检，行列号换算为整条回复绝对行号后追加进 `issues`。
fn check_block(html: &str, start_line: usize, issues: &mut Vec<CanvasIssue>) {
    for seg in extract_scripts(html) {
        // 只预检内联 JS；json 数据岛是静态数据（原样保留）、src 外链在
        // 无网络 iframe 里本就不加载——都不属于语法预检对象。
        if seg.kind != ScriptKind::Js {
            continue;
        }
        // seg.line_offset = 块内容里 script 正文前的换行数；绝对行 =
        // 块首行 + 偏移 + (JS 片段内行号 - 1)。
        let base = start_line + seg.line_offset;
        for iss in check_js_syntax(&seg.js) {
            issues.push(CanvasIssue {
                line: base + iss.line.saturating_sub(1),
                col: iss.col,
                message: iss.message,
            });
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScriptKind {
    /// 内联 JS（预检对象）。
    Js,
    /// `<script type="application/json">` 数据岛（静态数据，跳过预检）。
    JsonData,
    /// `<script src=...>` 外链（无网络 iframe 不加载，跳过预检）。
    External,
}

struct ScriptSegment {
    js: String,
    /// script 正文前的换行数（块内容内偏移，用于行号换算）。
    line_offset: usize,
    kind: ScriptKind,
}

/// 提取块内全部 `<script>` 段。`</script>` 终止语义与浏览器 HTML 解析
/// 一致：正文里第一个 `</script` 即终止（无论是否在 JS 字符串里）。
/// 已知简化（诚实边界）：`<!-- -->` HTML 注释里的 `<script>` 会被当真——
/// 模型产出的 canvas 罕见此形态，误报方向是「多预检」，可接受。
fn extract_scripts(html: &str) -> Vec<ScriptSegment> {
    let lower = html.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut pos = 0usize;
    while let Some(rel) = lower[pos..].find("<script") {
        let tag_start = pos + rel;
        let after = tag_start + "<script".len();
        // 标签边界：`<scriptx` 不是 script 标签。
        match lower[after..].chars().next() {
            Some(' ') | Some('\t') | Some('\n') | Some('\r') | Some('>') | Some('/') => {}
            _ => {
                pos = after;
                continue;
            }
        }
        // 找开标签收尾 '>'（属性值内的 '>' 不算——尊重引号）。
        let mut i = after;
        let mut quote: Option<char> = None;
        while i < html.len() {
            let c = html[i..].chars().next().unwrap();
            if let Some(q) = quote {
                if c == q {
                    quote = None;
                }
            } else if c == '>' {
                break;
            } else if c == '"' || c == '\'' {
                quote = Some(c);
            }
            i += c.len_utf8();
        }
        if i >= html.len() {
            break; // 开标签未闭合，放弃剩余扫描
        }
        let tag_text = &lower[after..i];
        let content_start = i + 1;
        let Some(close_rel) = lower[content_start..].find("</script") else {
            break; // 无闭合标签，其余内容按浏览器语义也不是 script 正文
        };
        let content_end = content_start + close_rel;
        let kind = if attr_value(tag_text, "src").is_some() {
            ScriptKind::External
        } else if attr_value(tag_text, "type")
            .map(|t| t.trim().to_ascii_lowercase())
            .as_deref()
            == Some("application/json")
        {
            ScriptKind::JsonData
        } else {
            ScriptKind::Js
        };
        let line_offset = html[..content_start].matches('\n').count();
        out.push(ScriptSegment {
            js: html[content_start..content_end].to_string(),
            line_offset,
            kind,
        });
        pos = content_end + "</script".len();
    }
    out
}

/// 从开标签文本里取属性值（name 不区分大小写；值支持双/单引号与裸值，
/// 无值属性返回空串）。找不到返回 None。
fn attr_value(tag: &str, name: &str) -> Option<String> {
    let bytes = tag.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        // 读属性名（到空白或 '='）。
        let start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'=' {
            i += 1;
        }
        let attr_name = tag[start..i].to_ascii_lowercase();
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i < bytes.len() && bytes[i] == b'=' {
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < bytes.len() && (bytes[i] == b'"' || bytes[i] == b'\'') {
                let q = bytes[i];
                let vs = i + 1;
                let mut j = vs;
                while j < bytes.len() && bytes[j] != q {
                    j += 1;
                }
                let val = tag[vs..j.min(bytes.len())].to_string();
                if attr_name == name {
                    return Some(val);
                }
                i = (j + 1).min(bytes.len());
            } else {
                let vs = i;
                let mut j = vs;
                while j < bytes.len() && !bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                let val = tag[vs..j].to_string();
                if attr_name == name {
                    return Some(val);
                }
                i = j;
            }
        } else {
            // 无值属性（defer / nomodule 等）。
            if attr_name == name {
                return Some(String::new());
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// JS 语法预检（轻量平衡检查——非完整 parser，见模块文档诚实边界）
// ---------------------------------------------------------------------------

/// 这些关键字之后 `/` 应读作正则字面量（而非除号）——经典启发式，
/// 覆盖语句/表达式位置的常见形态。
const KEYWORDS_BEFORE_VALUE: &[&str] = &[
    "return",
    "typeof",
    "instanceof",
    "in",
    "of",
    "new",
    "delete",
    "void",
    "throw",
    "case",
    "do",
    "else",
    "yield",
    "await",
];

/// 单问题上限（防病态输入刷出海量问题撑爆回灌文本）。
const MAX_ISSUES: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Code,
    LineComment,
    BlockComment,
    /// 普通字符串（' 或 "；开启引号记在 str_opener）。
    Str,
    /// 模板字面量（`；开启位置记在 templates 栈）。
    Template,
    /// 正则字面量（/.../；[...] 字符类内的 / 不终止，记 regex_class）。
    Regex,
}

/// 括号栈帧。`template_expr = true` 的帧是模板 `${` 表达式：只能被 `}`
/// 关闭，关闭时同时退回 Template 模式。
struct BracketFrame {
    opener: char,
    line: usize,
    col: usize,
    template_expr: bool,
}

/// 轻量 JS 平衡检查：扫描内联 JS 片段，报告括号不匹配/未闭合、字符串与
/// 模板字面量未闭合、块注释未闭合、多余闭括号等问题（行列号相对片段
/// 首行，1 起；列按字符计）。已知启发式边界：
/// - `/` 的正则 vs 除号歧义用「前驱 token」启发式判定（关键字/运算符/
///   开括号后 = 正则；标识符/数字/闭括号/字符串后 = 除号）——极端书写
///   形态可能误判；误判为除号只损失正则内的括号隔离（罕见），误判为正则
///   会吞掉后续真实代码（已尽量往除号方向保守）。
/// - 非平衡类语法错误（关键字重复、缺分号等）不检——如实边界。
pub fn check_js_syntax(js: &str) -> Vec<JsSyntaxIssue> {
    let chars: Vec<char> = js.chars().collect();
    let mut issues: Vec<JsSyntaxIssue> = Vec::new();

    let mut modes: Vec<Mode> = vec![Mode::Code];
    let mut brackets: Vec<BracketFrame> = Vec::new();
    // 模板字面量开启反引号位置（与 modes 里的 Template 一一对应）。
    let mut templates: Vec<(usize, usize)> = Vec::new();
    let mut str_opener: Option<(char, usize, usize)> = None;
    let mut comment_opener: Option<(usize, usize)> = None;
    let mut regex_class = false;
    let mut regex_allowed = true;
    let mut word = String::new();
    let mut line = 1usize;
    let mut col = 0usize;

    let mut i = 0usize;
    while i < chars.len() {
        if issues.len() >= MAX_ISSUES {
            issues.push(JsSyntaxIssue {
                line,
                col: col + 1,
                message: format!("问题数超过 {MAX_ISSUES} 条上限，剩余略（先修已列出者）"),
            });
            return issues;
        }
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        let cl = line;
        let cc = col + 1; // 当前字符的 1-based 列

        // 词边界收尾：标识符/数字结束的瞬间定 regex_allowed。
        let is_word_char = c.is_alphanumeric() || c == '_' || c == '$';
        if !is_word_char && !word.is_empty() {
            finalize_word(&mut word, &mut regex_allowed);
        }

        // 本轮前进量（双字符 token / 转义为 2，其余 1）。
        let mut advance = 1usize;

        match modes.last().copied().unwrap_or(Mode::Code) {
            Mode::Code => match c {
                '/' if next == Some('/') => {
                    modes.push(Mode::LineComment);
                    advance = 2;
                }
                '/' if next == Some('*') => {
                    modes.push(Mode::BlockComment);
                    comment_opener = Some((cl, cc));
                    advance = 2;
                }
                '/' if regex_allowed => {
                    modes.push(Mode::Regex);
                    regex_class = false;
                }
                '"' | '\'' => {
                    modes.push(Mode::Str);
                    str_opener = Some((c, cl, cc));
                    regex_allowed = false;
                }
                '`' => {
                    modes.push(Mode::Template);
                    templates.push((cl, cc));
                    regex_allowed = false;
                }
                '(' | '[' | '{' => {
                    brackets.push(BracketFrame {
                        opener: c,
                        line: cl,
                        col: cc,
                        template_expr: false,
                    });
                    regex_allowed = true;
                }
                ')' | ']' | '}' => {
                    close_bracket(&mut brackets, &mut modes, &mut issues, c, cl, cc);
                    regex_allowed = false;
                }
                c if is_word_char => {
                    word.push(c);
                }
                c if c.is_whitespace() => {}
                _ => {
                    // 其余标点/运算符：其后可接正则字面量。
                    regex_allowed = true;
                }
            },
            Mode::LineComment => {
                if c == '\n' {
                    modes.pop();
                }
            }
            Mode::BlockComment => {
                if c == '*' && next == Some('/') {
                    modes.pop();
                    comment_opener = None;
                    advance = 2;
                }
            }
            Mode::Str => {
                let q = str_opener.map(|(q, _, _)| q).unwrap_or('"');
                if c == '\\' {
                    advance = 2; // 转义序列整体跳过
                } else if c == q {
                    modes.pop();
                    str_opener = None;
                } else if c == '\n' {
                    // 普通字符串禁止裸换行——报错并按已闭合恢复（换行本身
                    // 仍由尾部统一前进计行）。
                    if let Some((q, l, oc)) = str_opener {
                        issues.push(JsSyntaxIssue {
                            line: l,
                            col: oc,
                            message: format!("字符串引号未闭合（'{q}' 开启于此，行尾前无配对）"),
                        });
                    }
                    modes.pop();
                    str_opener = None;
                }
            }
            Mode::Template => {
                if c == '\\' {
                    advance = 2;
                } else if c == '`' {
                    modes.pop();
                    templates.pop();
                } else if c == '$' && next == Some('{') {
                    advance = 2;
                    modes.push(Mode::Code);
                    brackets.push(BracketFrame {
                        opener: '{',
                        line: cl,
                        col: cc + 1,
                        template_expr: true,
                    });
                    regex_allowed = true;
                }
            }
            Mode::Regex => {
                if c == '\\' {
                    advance = 2;
                } else if c == '[' {
                    regex_class = true;
                } else if c == ']' {
                    regex_class = false;
                } else if c == '/' && !regex_class {
                    modes.pop();
                } else if c == '\n' {
                    issues.push(JsSyntaxIssue {
                        line: cl,
                        col: cc,
                        message: "正则字面量未闭合（行尾前无配对的 /）".to_string(),
                    });
                    modes.pop();
                }
            }
        }

        // 前进并计行：advance=2 的转义窗口可能吞进换行（字符串/模板里的
        // `\<newline>` 行续行——此时 c 是反斜杠，只看 c 会漏计行号，后续
        // 全部问题行号错位）。列号取最后一个换行之后的剩余宽度。
        let win_start = i;
        let win_end = (i + advance).min(chars.len());
        i = win_end;
        let mut nl_count = 0usize;
        let mut last_nl = None;
        for (off, wc) in chars[win_start..win_end].iter().enumerate() {
            if *wc == '\n' {
                nl_count += 1;
                last_nl = Some(off);
            }
        }
        match last_nl {
            Some(off) => {
                line += nl_count;
                col = win_end - (win_start + off) - 1;
            }
            None => col += advance,
        }
    }

    // EOF 收尾：各未闭合态逐一报告。
    if modes.contains(&Mode::BlockComment)
        && let Some((l, oc)) = comment_opener
    {
        issues.push(JsSyntaxIssue {
            line: l,
            col: oc,
            message: "块注释未闭合（/* 后无配对的 */）".to_string(),
        });
    }
    if modes.contains(&Mode::Str)
        && let Some((q, l, oc)) = str_opener
    {
        issues.push(JsSyntaxIssue {
            line: l,
            col: oc,
            message: format!("字符串引号未闭合（'{q}' 开启于此，片段结束前无配对）"),
        });
    }
    for (l, oc) in &templates {
        issues.push(JsSyntaxIssue {
            line: *l,
            col: *oc,
            message: "模板字面量未闭合（` 后无配对的反引号）".to_string(),
        });
    }
    for f in &brackets {
        // 模板表达式帧的未闭合已由模板反引号问题覆盖（同源），不重复报。
        if f.template_expr {
            continue;
        }
        issues.push(JsSyntaxIssue {
            line: f.line,
            col: f.col,
            message: format!("括号未闭合：'{}' 开启于此，片段结束前无配对", f.opener),
        });
    }
    issues
}

/// 词收尾：数字字面量 / 普通标识符后 `/` 是除号；关键字后是正则。
fn finalize_word(word: &mut String, regex_allowed: &mut bool) {
    let w = std::mem::take(word);
    if w.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        *regex_allowed = false;
    } else {
        *regex_allowed = KEYWORDS_BEFORE_VALUE.contains(&w.as_str());
    }
}

/// 关闭一个括号帧（含模板表达式帧的特判）。 mismatch 时帧照常弹出
/// （一错一报，靠问题明细里的开启位置定位，不级联刷屏）。
fn close_bracket(
    brackets: &mut Vec<BracketFrame>,
    modes: &mut Vec<Mode>,
    issues: &mut Vec<JsSyntaxIssue>,
    found: char,
    line: usize,
    col: usize,
) {
    match brackets.pop() {
        None => issues.push(JsSyntaxIssue {
            line,
            col,
            message: format!("多余的闭括号 '{found}'：没有待匹配的开启括号"),
        }),
        Some(f) => {
            if f.template_expr {
                if found == '}' {
                    modes.pop(); // 退回 Template 模式
                } else {
                    issues.push(JsSyntaxIssue {
                        line,
                        col,
                        message: format!(
                            "'{found}' 不能关闭模板表达式（${{ 于第 {} 行第 {} 列引入，只接受 '}}'）",
                            f.line, f.col
                        ),
                    });
                    brackets.push(f); // 放回，等真正的 '}'
                }
            } else {
                let expect = match f.opener {
                    '(' => ')',
                    '[' => ']',
                    _ => '}',
                };
                if found != expect {
                    issues.push(JsSyntaxIssue {
                        line,
                        col,
                        message: format!(
                            "括号不匹配：'{found}' 应为 '{expect}'（'{}' 开启于第 {} 行第 {} 列）",
                            f.opener, f.line, f.col
                        ),
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
