//! Forge 微优化算子（自进化升级 D 线，SPO 精简版，2026-10-14）。
//!
//! 对**存量** skill body 的小步改写候选：四类受限算子（指令重排 / 示例
//! 增删 / 约束明确化 / 失败案例注入）。设计边界：
//! - 纯函数（body 文本 → 改写文本），不执行、不 IO——执行与闸由调用方接
//!   pipeline（回归全过才允许部署，v3.1 计划 §七）；
//! - 开放式重写**不做**（防漂移）；单次改写 token 预算 = 输出字符数上限
//!   （≈ tokens×2 的 CJK 保守估计，超限 = Err）；
//! - 确定性：同输入同输出（无随机源），便于回归。

use serde::{Deserialize, Serialize};

/// 单次改写输出 token 预算缺省值（forge.optimize.budget_tokens 可调；
/// CJK 保守 ×2 折算字符数做上限）。
pub const OPTIMIZE_BUDGET_TOKENS: u32 = 2000;

/// 可用算子（v1 集合，v3.1 计划裁决）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operator {
    /// 指令重排：把「约束/注意」类段落提前（约束前置提升遵从率）。
    ReorderConstraints,
    /// 示例增补：追加「## 示例」段（含 param 提供的示例文本）。
    AddExample,
    /// 约束明确化：把行内「不要/必须/避免」散点收拢为显式「## 约束」段。
    ClarifyConstraints,
    /// 失败案例注入：追加「## 已知失败案例」段（param 提供案例文本）。
    InjectFailureCase,
}

impl Operator {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "reorder_constraints" => Some(Self::ReorderConstraints),
            "add_example" => Some(Self::AddExample),
            "clarify_constraints" => Some(Self::ClarifyConstraints),
            "inject_failure_case" => Some(Self::InjectFailureCase),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ReorderConstraints => "reorder_constraints",
            Self::AddExample => "add_example",
            Self::ClarifyConstraints => "clarify_constraints",
            Self::InjectFailureCase => "inject_failure_case",
        }
    }
}

/// 改写候选。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizeCandidate {
    pub operator: String,
    /// 改写后的完整 SKILL.md（frontmatter 原样保留 + 新 body）。
    pub content: String,
    /// 变更摘要（人读）。
    pub summary: String,
}

/// 拆分 frontmatter 与 body（无围栏 = 全 body）。容忍 LF 与 CRLF——
/// Windows 编辑的 SKILL.md 常见 CRLF，首版只认 LF 时 frontmatter 识别
/// 失败 → 候选丢 frontmatter → 应用即元数据丢失（二轮复查根修）。
fn split_frontmatter(content: &str) -> (String, String) {
    let (open_len, nl) = if content.starts_with("---\r\n") {
        (5, "\r\n")
    } else if content.starts_with("---\n") {
        (4, "\n")
    } else {
        return (String::new(), content.to_string());
    };
    let rest = &content[open_len..];
    let close = format!("{nl}---");
    if let Some(end) = rest.find(&close) {
        let fm = &rest[..end];
        let body = &rest[end + close.len()..];
        return (format!("---\n{fm}\n---"), body.to_string());
    }
    (String::new(), content.to_string())
}

/// 应用算子生成候选。
///
/// `param`：AddExample / InjectFailureCase 的附加文本（其余算子忽略）。
/// `budget_tokens`：输出预算（forge.optimize.budget_tokens；CJK ×2 折算）。
pub fn apply(
    operator: Operator,
    content: &str,
    param: Option<&str>,
    budget_tokens: u32,
) -> Result<OptimizeCandidate, String> {
    let (fm, body) = split_frontmatter(content);
    let (new_body, summary) = match operator {
        Operator::ReorderConstraints => reorder_constraints(&body)?,
        Operator::AddExample => {
            let text = param
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or("add_example requires param (example text)")?;
            add_section(&body, "示例", text)
        }
        Operator::ClarifyConstraints => clarify_constraints(&body)?,
        Operator::InjectFailureCase => {
            let text = param
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or("inject_failure_case requires param (failure case text)")?;
            add_section(&body, "已知失败案例", text)
        }
    };

    let new_content = format!("{fm}\n{new_body}");
    let budget_chars = budget_tokens as usize * 2;
    if new_content.chars().count() > budget_chars {
        return Err(format!(
            "optimized content {} chars exceeds budget {budget_chars} ({budget_tokens} tokens)",
            new_content.chars().count()
        ));
    }
    if new_content == content {
        return Err("operator produced no change".into());
    }

    Ok(OptimizeCandidate {
        operator: operator.as_str().into(),
        content: new_content,
        summary,
    })
}

/// 约束前置：把「## 约束」「## 注意」「## 限制」标题段移到 body 首个
/// 「## 」段之前（首个标题前的引导文本保持在最前，不落进任何节内）；
/// 不存在约束段 = 无变化错误。
fn reorder_constraints(body: &str) -> Result<(String, String), String> {
    let (constraint_sec, others) = extract_section(body, &["约束", "注意", "限制"])?;
    let Some(sec) = constraint_sec else {
        return Err("no constraint section found to reorder".into());
    };
    if others.is_empty() {
        return Err("constraint section is the only section (nothing to reorder)".into());
    }
    // 布局：引导文本 → 约束段 → 其余段（原序）。引导文本必须在约束段
    // 之前（首版把 intro 拼在约束段之后且无标题分隔，渲染时被并进
    // 「## 约束」节 = 文档结构损坏，二轮复查根修）。
    let intro_src = body
        .lines()
        .take_while(|l| !l.starts_with("## "))
        .collect::<Vec<_>>()
        .join("\n");
    let intro = if intro_src.trim().is_empty() {
        String::new()
    } else {
        format!("{}\n\n", intro_src.trim())
    };
    let rebuilt = format!("{intro}{}\n\n{}", sec.trim_end(), others.join("\n\n"));
    Ok((rebuilt, "约束段已前置（提升遵从率）".into()))
}

/// 提取指定标题（任一别名）的完整段落（标题行到下一个 ## 或文末）。
/// 返回 (段文本含标题, 其余全部段)。
fn extract_section(body: &str, aliases: &[&str]) -> Result<(Option<String>, Vec<String>), String> {
    let sections = split_sections(body);
    let mut found = None;
    let mut others = Vec::new();
    for (title, text) in sections {
        if aliases.iter().any(|a| title.contains(a)) {
            if found.is_none() {
                let mut sec = format!("## {title}\n{text}");
                if !sec.ends_with('\n') {
                    sec.push('\n');
                }
                found = Some(sec);
            } else {
                others.push(format!("## {title}\n{text}"));
            }
        } else {
            others.push(format!("## {title}\n{text}"));
        }
    }
    Ok((found, others))
}

/// 按 `## ` 标题切分 body（返回 (标题文本, 段正文) 对；标题前导文本忽略）。
fn split_sections(body: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut cur_title: Option<String> = None;
    let mut cur_text = String::new();
    for line in body.lines() {
        if let Some(t) = line.strip_prefix("## ") {
            if let Some(prev) = cur_title.take() {
                out.push((prev, cur_text.trim_end().to_string()));
            }
            cur_title = Some(t.trim().to_string());
            cur_text.clear();
        } else if cur_title.is_some() {
            cur_text.push_str(line);
            cur_text.push('\n');
        }
    }
    if let Some(prev) = cur_title.take() {
        out.push((prev, cur_text.trim_end().to_string()));
    }
    out
}

/// 追加一个段落。
fn add_section(body: &str, title: &str, text: &str) -> (String, String) {
    let mut new_body = body.trim_end().to_string();
    new_body.push_str(&format!("\n\n## {title}\n\n{text}\n"));
    (new_body, format!("已追加「{title}」段"))
}

/// 约束明确化：从 body 行内抓取含「不要/必须/避免/禁止/不允许」的散点行
/// 收拢为显式「## 约束」段（已存在约束段 = 无变化错误）。
fn clarify_constraints(body: &str) -> Result<(String, String), String> {
    let (existing, _) = extract_section(body, &["约束", "注意", "限制"])?;
    if existing.is_some() {
        return Err("constraint section already exists (use reorder instead)".into());
    }
    const MARKERS: &[&str] = &["不要", "必须", "避免", "禁止", "不允许"];
    let picked: Vec<&str> = body
        .lines()
        .filter(|l| !l.starts_with('#') && MARKERS.iter().any(|m| l.contains(m)))
        .collect();
    if picked.is_empty() {
        return Err("no inline constraint lines found to clarify".into());
    }
    let sec = format!(
        "## 约束\n\n{}\n",
        picked
            .iter()
            .map(|l| format!("- {}", l.trim_start_matches("- ").trim()))
            .collect::<Vec<_>>()
            .join("\n")
    );
    // 散点行从原位置移除（去重收拢语义）。
    let remaining: Vec<&str> = body.lines().filter(|l| !picked.contains(l)).collect();
    let mut new_body = remaining.join("\n");
    if !new_body.ends_with('\n') && !new_body.is_empty() {
        new_body.push('\n');
    }
    new_body.push_str(&sec);
    Ok((
        new_body,
        format!("收拢 {} 条散点约束为显式段", picked.len()),
    ))
}

#[cfg(test)]
mod tests;
