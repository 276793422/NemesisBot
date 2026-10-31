//! 内联 `<think>...</think>` 思考剥离（思考链捕获形态③）。
//!
//! 部分 OpenAI 兼容后端（llama.cpp 裸形态、部分 vLLM 模板等）把思考链
//! 以 `<think>...</think>` 标签直接内联在 `content` 正文里，不提供独立的
//! `reasoning_content` 字段。不剥离的话：思考混进正文（UI 把推理当回复），
//! tool-call repair 还可能扫到思考里的伪工具调用文本。
//!
//! 两个入口：
//! - [`strip_inline_thinking`]：非流式纯函数，一次处理完整正文；
//! - [`ThinkStripper`]：流式状态机，跨 chunk（`<think>` 标签可能被 SSE
//!   事件边界劈开，如 `<thi` + `nk>`），holdback 的疑似标签前缀在终态
//!   [`ThinkStripper::finish`] 时定谳——只应在终态调用 `finish`，流中途
//!   调用会把真标签前缀提前当正文放走。
//!
//! 剥出的思考经 [`merge_reasoning`] 并入 `reasoning_content`（与既有字段
//! 并存时 `\n\n` 拼接——任何形态都不丢内容）；此后全链只有一条通路。
//!
//! 边界（诚实语义）：
//! - 无闭合 `</think>`（截断响应）→ 已进入部分全归 thinking，后续正文照常；
//! - `</think>` 先于 `<think>`（畸形）→ 原样直通不猜（只对真实开标签响应）；
//! - 多个块 → 全部剥出，`\n\n` 拼接；
//! - 仅处理 `content` 正文；`reasoning_content` 字段不在剥离范围。

/// `<think>` 开标签。
const OPEN_TAG: &str = "<think>";
/// `</think>` 闭标签。
const CLOSE_TAG: &str = "</think>";

/// 剥离器状态：正文区 / 思考区。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StripState {
    /// 正文区——监视 `<think>` 开标签。
    Plain,
    /// 思考区——监视 `</think>` 闭标签。
    InThink,
}

/// 非流式纯函数：从完整正文剥出全部 `<think>` 块。
///
/// 返回 `(净正文, 思考)`；无标签时思考为 `None` 且正文逐字节不变
/// （零影响直通）。多个块 `\n\n` 拼接；未闭合块的已进入部分全归思考。
pub fn strip_inline_thinking(content: &str) -> (String, Option<String>) {
    if !content.contains(OPEN_TAG) {
        return (content.to_string(), None);
    }
    let mut out = String::with_capacity(content.len());
    let mut thinking = String::new();
    let mut rest = content;
    while let Some(start) = rest.find(OPEN_TAG) {
        out.push_str(&rest[..start]);
        let after = &rest[start + OPEN_TAG.len()..];
        let (seg, next) = match after.find(CLOSE_TAG) {
            Some(end) => (&after[..end], &after[end + CLOSE_TAG.len()..]),
            // 未闭合（截断响应）：已进入部分全归 thinking，扫描终止。
            None => (after, ""),
        };
        let seg = seg.trim();
        if !seg.is_empty() {
            if !thinking.is_empty() {
                thinking.push_str("\n\n");
            }
            thinking.push_str(seg);
        }
        rest = next;
    }
    out.push_str(rest);
    let thinking = if thinking.is_empty() {
        None
    } else {
        // 有剥出才修剪正文边缘（剥空壳标签不动正文，零影响面最小）。
        Some(thinking)
    };
    let cleaned = if thinking.is_some() {
        out.trim().to_string()
    } else {
        out
    };
    (cleaned, thinking)
}

/// 归一合并：wire 字段（形态①②④既有值）与内联剥出（形态③）并存时
/// `\n\n` 拼接，任何来源都不丢内容。两空 = `None`。
pub fn merge_reasoning(field: Option<String>, inline: Option<String>) -> Option<String> {
    match (field, inline) {
        (Some(mut a), Some(b)) => {
            a.push_str("\n\n");
            a.push_str(&b);
            Some(a)
        }
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

/// `buf` 末尾能构成 `tag` 真前缀的最长长度（真前缀 = 短于完整 tag——
/// 完整 tag 已被 find 命中，不会落到这里）。跨界 holdback 的依据：
/// chunk 边界劈开的标签片段（`<thi` / `</th`）先扣住不下发，等下一
/// chunk 补齐后统一判定。
fn pending_tag_len(buf: &str, tag: &str) -> usize {
    // 上界收两端：真前缀（< 完整 tag）且不超出 buf 本身（短 buf 的整体
    // 也可以是真前缀，如 buf="<thi"；空 buf 直接 0）。
    let max = tag.len().saturating_sub(1).min(buf.len());
    for len in (1..=max).rev() {
        let i = buf.len() - len;
        if buf.is_char_boundary(i) && buf[i..] == tag[..len] {
            return len;
        }
    }
    0
}

/// 流式 `<think>` 剥离状态机（跨 chunk）。
///
/// 用法：每个 content delta 过一次 [`feed`](ThinkStripper::feed)，返回值是
/// **可安全下发的净正文**（可能因 holdback 而短暂少于入参，后续补齐）；
/// 剥出的思考经 [`take_thinking`](ThinkStripper::take_thinking) 取走（仅在
/// 块闭合后交出，流中途截取会把未闭合块劈成两段）。终态（finish_reason
/// 帧 / [DONE] 合成 / EOF 合成）调一次 [`finish`](ThinkStripper::finish)：
/// holdback 残留按状态定谳——Plain 残留是正文（响应结束了，不是真标签），
/// InThink 残留归思考（无闭合截断语义）。
#[derive(Debug)]
pub struct ThinkStripper {
    state: StripState,
    /// 未决字节：待判定的疑似标签前缀（Plain）或未下发思考尾（InThink）。
    buf: String,
    /// 已剥出的思考原文（未修剪；`take`/`finish` 时统一 trim）。
    thinking: String,
    /// 是否已闭合过至少一个块（`take_thinking` 的交出门闩）。
    block_closed: bool,
}

impl ThinkStripper {
    pub fn new() -> Self {
        Self {
            state: StripState::Plain,
            buf: String::new(),
            thinking: String::new(),
            block_closed: false,
        }
    }

    /// 送入一个 content delta，返回可安全下发的净正文。
    ///
    /// holdback 的疑似标签前缀暂不返回（可能 7 字节内）；块闭合瞬间把
    /// 累积思考交由 [`take_thinking`](ThinkStripper::take_thinking) 取走。
    pub fn feed(&mut self, chunk: &str) -> String {
        self.buf.push_str(chunk);
        let mut out = String::new();
        loop {
            match self.state {
                StripState::Plain => {
                    if let Some(i) = self.buf.find(OPEN_TAG) {
                        out.push_str(&self.buf[..i]);
                        self.buf.drain(..i + OPEN_TAG.len());
                        self.state = StripState::InThink;
                    } else {
                        let keep = pending_tag_len(&self.buf, OPEN_TAG);
                        let emit = self.buf.len() - keep;
                        out.push_str(&self.buf[..emit]);
                        self.buf.drain(..emit);
                        break;
                    }
                }
                StripState::InThink => {
                    if let Some(i) = self.buf.find(CLOSE_TAG) {
                        self.thinking.push_str(&self.buf[..i]);
                        self.buf.drain(..i + CLOSE_TAG.len());
                        self.state = StripState::Plain;
                        self.block_closed = true;
                    } else {
                        let keep = pending_tag_len(&self.buf, CLOSE_TAG);
                        let emit = self.buf.len() - keep;
                        self.thinking.push_str(&self.buf[..emit]);
                        self.buf.drain(..emit);
                        break;
                    }
                }
            }
        }
        out
    }

    /// 取走已剥出的思考（trim 后空段 = `None`）。
    ///
    /// 门闩语义：至少一个块闭合后才交出——流中途每 chunk 都调时，未闭合
    /// 块的半截思考不会被提前取走劈段（调用方模式 = 每 chunk `feed` 后
    /// `take_thinking`，终态再 `finish` 兜底）。
    pub fn take_thinking(&mut self) -> Option<String> {
        if !self.block_closed {
            return None;
        }
        self.block_closed = false;
        self.drain_thinking()
    }

    /// 终态收尾：holdback 残留按状态定谳（Plain = 正文返回值；InThink =
    /// 归思考），全部思考并入 `reasoning`（非空时 `\n\n` 分隔）。
    ///
    /// **只在终态调用**（finish_reason 帧 / [DONE] 合成 / EOF 合成）——
    /// 流中途调用会把真标签前缀提前当正文放走。返回值是要补发的正文增量。
    pub fn finish(&mut self, reasoning: &mut String) -> String {
        let flushed = self.flush();
        if let Some(th) = self.drain_thinking() {
            if !reasoning.is_empty() {
                reasoning.push_str("\n\n");
            }
            reasoning.push_str(&th);
        }
        flushed
    }

    /// 下发 holdback 残留（Plain = 正文；InThink = 未闭合，归思考）。
    fn flush(&mut self) -> String {
        match self.state {
            StripState::Plain => std::mem::take(&mut self.buf),
            StripState::InThink => {
                let held = std::mem::take(&mut self.buf);
                self.thinking.push_str(&held);
                String::new()
            }
        }
    }

    /// 取走并清空累积思考（trim 后空段 = `None`）。内部无门闩版。
    fn drain_thinking(&mut self) -> Option<String> {
        if self.thinking.is_empty() {
            return None;
        }
        let t = std::mem::take(&mut self.thinking);
        let trimmed = t.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    }
}

impl Default for ThinkStripper {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
