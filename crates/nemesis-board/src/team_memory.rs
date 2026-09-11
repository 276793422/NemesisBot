//! Swarm M4.5 §6.5：团队经验检索与渲染纯逻辑层。
//!
//! 职责边界：本模块**不含任何 IO 与 LLM 调用**——从 [`crate::BoardStore`]
//! 拿到的条目列表如何按任务文本/标签匹配、如何渲染成派发提示词段落或
//! planner 注入字符串，都是纯函数（单测零依赖）；查库时机、`use_count`
//! 计数与注入调用在装配层（board handler / board_review）完成。
//!
//! MVP 检索语义（impl-plan §6.5.2，D8 裁决不上向量）：scope 关键词与
//! 任务文本做大小写不敏感子串匹配，或与 required_tags 精确匹配；排除
//! deprecated 条目；按 use_count 降序取 top-N。

use crate::models::{TeamMemoryEntry, team_memory_category};

/// 派发/planner 注入的最大条数（§6.5.2：top-5，防提示词膨胀）。
pub const MAX_MATCHED_EXPERIENCES: usize = 5;

/// 从条目列表里匹配出与本任务相关的经验（注入端唯一入口）。
///
/// 匹配规则（任一命中即入选）：
/// - scope（大小写不敏感）是任务文本的子串——任务文本 = title +
///   description + 验收标准的拼接，由调用方传入；
/// - scope（大小写不敏感）与 required_tags 中某标签相等。
///
/// 排除 deprecated；排序 use_count 降序 → id 降序（高复用经验优先，
/// 同分新者优先）；取前 `top` 条。scope 为空的条目永不匹配（空标签
/// 会命中一切，等于放弃检索语义）。
pub fn match_experiences<'a>(
    entries: &'a [TeamMemoryEntry],
    issue_text: &str,
    tags: &[String],
    top: usize,
) -> Vec<&'a TeamMemoryEntry> {
    let issue_lower = issue_text.to_lowercase();
    let tags_lower: Vec<String> = tags.iter().map(|t| t.trim().to_lowercase()).collect();
    let mut hits: Vec<&TeamMemoryEntry> = entries
        .iter()
        .filter(|e| !e.deprecated && !e.scope.trim().is_empty())
        .filter(|e| {
            let scope_lower = e.scope.trim().to_lowercase();
            issue_lower.contains(&scope_lower) || tags_lower.contains(&scope_lower)
        })
        .collect();
    hits.sort_by(|a, b| b.use_count.cmp(&a.use_count).then(b.id.cmp(&a.id)));
    hits.truncate(top);
    hits
}

/// 渲染派发提示词的经验注入段（§6.5.2「# 团队过往经验」）。
///
/// 空匹配返回 `None`——调用方不往 worker 提示词里塞空段（保持与无经验
/// 时的提示词字节一致，prompt cache 友好）。
pub fn render_dispatch_experience_section(entries: &[&TeamMemoryEntry]) -> Option<String> {
    if entries.is_empty() {
        return None;
    }
    let mut s = String::from("\n# 团队过往经验（来自类似任务，供参考避坑）\n");
    for e in entries {
        s.push_str(&format_experience_line(e));
        s.push('\n');
    }
    Some(s)
}

/// 渲染 planner 注入字符串列表（填
/// [`crate::build_planner_user_prompt`] 的 `team_experience` 参数；
/// 该函数自带 `- ` 前缀与段头，此处只出条目行）。
pub fn render_planner_experience_strings(entries: &[&TeamMemoryEntry]) -> Vec<String> {
    entries.iter().map(|e| format_experience_line(e)).collect()
}

/// 单条经验的人读行：`[类别] scope：内容（来源 NB-x / 蒸馏者）`。
fn format_experience_line(e: &TeamMemoryEntry) -> String {
    let source = if e.source.trim().is_empty() {
        String::new()
    } else {
        format!(" / 来源 {}", e.source.trim())
    };
    let author = if e.author.trim().is_empty() {
        String::new()
    } else if source.is_empty() {
        format!("（蒸馏者 {}）", e.author.trim())
    } else {
        format!("、蒸馏者 {}）", e.author.trim())
    };
    let tail = if source.is_empty() && author.is_empty() {
        String::new()
    } else if source.is_empty() {
        author
    } else {
        format!("（{source}{author}")
    };
    format!(
        "[{}] {}：{}{}",
        category_label(e),
        e.scope.trim(),
        e.content,
        tail
    )
}

/// 类别显示名（词表四类给中文名；未知值诚实归入泛称「经验」）。
fn category_label(e: &TeamMemoryEntry) -> &'static str {
    match e.category.as_str() {
        team_memory_category::PITFALL => "坑",
        team_memory_category::PATTERN => "模式",
        team_memory_category::CONVENTION => "约定",
        team_memory_category::PREFERENCE => "偏好",
        _ => "经验",
    }
}

#[cfg(test)]
mod tests;
