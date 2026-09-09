//! Swarm M3（§5.6 交付线程）：worker 汇报格式解析——交付线程首评与
//! M4 验收 agent 的共同地基（单一真相源：`REPORT_FORMAT_SECTION` 与
//! 本解析器必须同步演化，`build_dispatch_prompt` 直接引用本模块）。
//!
//! 解析从宽（与主持人裁决同一信任模型）：worker 的最终回复经 LLM 生成，
//! 可能带人格前后缀——按「段标题是否出现」判定格式，段落内容取标题行
//! 之后到下一个已知标题之间；前导散文忽略。核心三段（结论/交付物清单/
//! 自检结果）齐备才算结构化汇报；「风险与未尽事项」「经验与坑」缺失时
//! 按空串处理（M4.5 起五段）。不满足 → `None`（调用方诚实降级为普通评论）。

use serde::{Deserialize, Serialize};

/// 汇报格式五段标题（派发提示词结尾原样下发；worker 按此组织最终回复）。
pub const REPORT_FORMAT_SECTION: &str = "\
## 结论
（一句话：完成 / 部分完成 / 失败）
## 交付物清单
（branch、commits、改动/新建文件路径，逐条列出；没有则写\"无\"）
## 自检结果
（对照上面的验收标准逐条自检）
## 风险与未尽事项
（没有则写\"无\"）
## 经验与坑
（本次踩过的坑/可复用的做法，写清适用范围；没有则写\"无\"）";

/// 段标题（判定与切分的词表；与 `REPORT_FORMAT_SECTION` 同步）。
const SECTION_HEADERS: [&str; 5] = [
    "## 结论",
    "## 交付物清单",
    "## 自检结果",
    "## 风险与未尽事项",
    "## 经验与坑",
];

/// 评论内联字节上限（§5.6：diff/汇报 ≤64KB 内联，超限截断 + 引用注记，
/// 全文走层 2 HTTP 资产拉取）。
pub const MAX_INLINE_BYTES: usize = 64 * 1024;

/// 结构化汇报（交付线程首评 / 验收 agent 输入的统一形状）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryReport {
    /// 「## 结论」段。
    pub conclusion: String,
    /// 「## 交付物清单」段。
    pub deliverables: String,
    /// 「## 自检结果」段。
    pub self_check: String,
    /// 「## 风险与未尽事项」段（缺失 = 空串）。
    pub risks: String,
    /// 「## 经验与坑」段（M4.5；缺失 = 空串——旧四段汇报向后兼容）。
    #[serde(default)]
    pub experience: String,
}

/// 标题的全部出现位置（首现之后还有重复标题时，重复处也切段——
/// 「首现段」到重复处为止，读取时自然取到干净的首段内容）。
fn all_header_positions(text: &str) -> Vec<usize> {
    let mut out = Vec::new();
    for header in SECTION_HEADERS {
        let mut from = 0;
        while let Some(rel) = text[from..].find(header) {
            out.push(from + rel);
            from += rel + header.len();
        }
    }
    out.sort_unstable();
    out
}

/// 解析 worker 结构化汇报。核心三段（结论/交付物清单/自检结果）任一
/// 缺失 → None。段内容 trim 后原样保留（验收 agent 要读原文）；末段
/// 之后的尾部散文并入末段（从宽——无法区分正文与人格后缀）。
pub fn parse_delivery_report(text: &str) -> Option<DeliveryReport> {
    // 核心段缺失 → 非结构化汇报（调用方诚实降级为普通评论）。
    for header in &SECTION_HEADERS[..3] {
        text.find(header)?;
    }

    let mut bounds = all_header_positions(text);
    bounds.push(text.len());

    let section_text = |header: &str| -> String {
        let start = match text.find(header) {
            Some(p) => p + header.len(),
            None => return String::new(),
        };
        let end = bounds
            .iter()
            .find(|b| **b > start)
            .copied()
            .unwrap_or(text.len());
        text[start..end].trim().to_string()
    };

    Some(DeliveryReport {
        conclusion: section_text("## 结论"),
        deliverables: section_text("## 交付物清单"),
        self_check: section_text("## 自检结果"),
        risks: section_text("## 风险与未尽事项"),
        experience: section_text("## 经验与坑"),
    })
}

#[cfg(test)]
mod tests;
