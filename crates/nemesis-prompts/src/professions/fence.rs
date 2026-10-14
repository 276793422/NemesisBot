//! 职能工具围栏（B2，profession-capability-evolution 2026-10-14）：契约文本
//! 内的 ` ```nemesis-tool-fence ` 代码块 = 该职能的实例级工具白名单。
//!
//! D3 裁决：能力边界跟职能文本放一处——内置契约与用户自定义职能
//! （`<workspace>/cluster/professions/<family>/<spec>.md`）走同一机制，
//! 用户写档案天然获得同款能力。解析器零依赖（手写行解析，prompts crate
//! 无 serde_yaml）。**fail-open**：无围栏块的职能 = 不围（[`parse_tool_fence`]
//! 返回 `None`）；`Some(空)` = 显式全禁（契约作者的明确选择，消费方照办）。
//!
//! 围栏块是**装配元数据不是提示词**——[`strip_tool_fence`] 由消费方在把
//! 契约正文拼进 system prompt 之前调用，yaml 永不进 LLM 上下文。
//!
//! 文件形态约定（单一真相源：本模块两个函数共用的行扫描）：
//!
//! ````text
//! ```nemesis-tool-fence
//! # 注释行
//! - read_file
//! - write_file
//! ```
//! ````
//!
//! - 多个围栏块取第一个（与「front-matter 唯一」口径一致）；
//! - 列表项支持引号包裹与行尾 `#` 注释；非 `-` 行（含空行、说明文字）
//!   容忍跳过；
//! - 未闭合的围栏块视为延伸到 EOF（两个函数同口径，不劈叉）。

/// 围栏块开栏行（trim 后精确匹配）。
pub const TOOL_FENCE_MARKER: &str = "```nemesis-tool-fence";

/// 围栏块的字节界（含开栏行与闭栏行）。`None` = 文本无围栏块。
/// 未闭合 = 延伸到 EOF。偏移都落在行首 / EOF（字符边界安全）。
fn fence_block_bounds(text: &str) -> Option<(usize, usize)> {
    let mut off = 0usize;
    let mut start: Option<usize> = None;
    for line in text.split_inclusive('\n') {
        let t = line.trim_end_matches(['\n', '\r']).trim();
        if start.is_none() {
            if t == TOOL_FENCE_MARKER {
                start = Some(off);
            }
        } else if t == "```" {
            return Some((start.unwrap_or(0), off + line.len()));
        }
        off += line.len();
    }
    start.map(|s| (s, text.len()))
}

/// 从职能契约文本解析工具围栏白名单（保持书写顺序，去重）。
/// 返回 `None` = 无围栏块（消费方 fail-open 不围）；`Some(list)` = 白名单
/// （可能为空 = 显式全禁）。多个围栏块取第一个。
pub fn parse_tool_fence(text: &str) -> Option<Vec<String>> {
    let (s, e) = fence_block_bounds(text)?;
    let mut out: Vec<String> = Vec::new();
    for line in text[s..e].lines().skip(1) {
        let t = line.trim();
        if t == "```" {
            break;
        }
        // 行尾注释 → 引号 → 空白，三连清洗；`#` 不出现在工具名词表。
        let entry = t.strip_prefix('-').unwrap_or(t);
        let entry = entry.split('#').next().unwrap_or("").trim();
        let entry = entry.trim_matches('"').trim_matches('\'').trim();
        if entry.is_empty() {
            continue;
        }
        if !out.iter().any(|x| x == entry) {
            out.push(entry.to_string());
        }
    }
    Some(out)
}

/// 从契约正文剥除围栏块（含开栏/闭栏行与其间全部内容），其余字节原样。
/// 无围栏块 = 原样借用返回（零拷贝快路径）。
pub fn strip_tool_fence(text: &str) -> std::borrow::Cow<'_, str> {
    let Some((s, e)) = fence_block_bounds(text) else {
        return std::borrow::Cow::Borrowed(text);
    };
    std::borrow::Cow::Owned(format!("{}{}", &text[..s], &text[e..]))
}
