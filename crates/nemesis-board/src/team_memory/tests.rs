//! team_memory 纯逻辑层单测（匹配/渲染零依赖）。

use super::{
    MAX_MATCHED_EXPERIENCES, match_experiences, render_dispatch_experience_section,
    render_planner_experience_strings,
};
use crate::models::{TeamMemoryEntry, team_memory_category};

fn entry(id: i64, category: &str, scope: &str, content: &str, use_count: i64) -> TeamMemoryEntry {
    TeamMemoryEntry {
        id,
        category: category.to_string(),
        scope: scope.to_string(),
        content: content.to_string(),
        source: "NB-7".to_string(),
        author: "node-a".to_string(),
        use_count,
        deprecated: false,
        created_at: 1_700_000_000,
    }
}

#[test]
fn match_by_scope_substring_case_insensitive() {
    let entries = vec![
        entry(1, team_memory_category::PITFALL, "auth", "token 过期要刷新", 0),
        entry(2, team_memory_category::PATTERN, "rust", "先 cargo check 再改", 0),
    ];
    let hits = match_experiences(&entries, "修复登录 bug（AUTH 模块）", &[], 5);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, 1);
}

#[test]
fn match_by_required_tag_equality() {
    let entries = vec![entry(3, team_memory_category::PATTERN, "sqlite", "WAL 并发读安全", 0)];
    // 标签大小写不敏感精确匹配；正文不命中。
    let hits = match_experiences(&entries, " entirely unrelated text ", &[" SQLite ".to_string()], 5);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, 3);
}

#[test]
fn match_excludes_deprecated_and_empty_scope() {
    let mut deprecated = entry(4, team_memory_category::PITFALL, "auth", "已过时的坑", 99);
    deprecated.deprecated = true;
    let entries = vec![deprecated, entry(5, team_memory_category::PATTERN, "", "空 scope", 99)];
    let hits = match_experiences(&entries, "auth 任务", &[], 5);
    assert!(hits.is_empty());
}

#[test]
fn match_orders_by_use_count_then_id_and_truncates() {
    let entries = vec![
        entry(10, team_memory_category::PATTERN, "db", "低分旧条", 0),
        entry(11, team_memory_category::PITFALL, "db", "高复用条", 5),
        entry(12, team_memory_category::PATTERN, "db", "同分新条", 5),
        entry(13, team_memory_category::PATTERN, "db", "多出的条", 1),
        entry(14, team_memory_category::PATTERN, "db", "多出的条2", 2),
        entry(15, team_memory_category::PATTERN, "db", "多出的条3", 3),
        entry(16, team_memory_category::PATTERN, "db", "多出的条4", 4),
    ];
    let hits = match_experiences(&entries, "db migration", &[], MAX_MATCHED_EXPERIENCES);
    assert_eq!(hits.len(), MAX_MATCHED_EXPERIENCES);
    // use_count 降序，同分 id 降序（12 在 11 前）。
    let ids: Vec<i64> = hits.iter().map(|e| e.id).collect();
    assert_eq!(ids, vec![12, 11, 16, 15, 14]);
}

#[test]
fn render_dispatch_section_none_when_empty() {
    assert!(render_dispatch_experience_section(&[]).is_none());
}

#[test]
fn render_dispatch_section_has_header_and_lines() {
    let entries = vec![
        entry(1, team_memory_category::PITFALL, "auth", "token 过期要刷新", 0),
        entry(2, "weird-kind", "db", "词表外类别", 0),
    ];
    let refs: Vec<&TeamMemoryEntry> = entries.iter().collect();
    let section = render_dispatch_experience_section(&refs).expect("非空必须渲染");
    assert!(section.contains("# 团队过往经验"));
    // 词表内类别给中文标签；词表外诚实归「经验」。
    assert!(section.contains("[坑] auth：token 过期要刷新"));
    assert!(section.contains("[经验] db：词表外类别"));
    // 来源/蒸馏者尾巴。
    assert!(section.contains("来源 NB-7"));
    assert!(section.contains("蒸馏者 node-a"));
}

#[test]
fn render_planner_strings_no_bullet_prefix() {
    let entries = vec![entry(1, team_memory_category::PATTERN, "rust", "先 cargo check", 0)];
    let refs: Vec<&TeamMemoryEntry> = entries.iter().collect();
    let lines = render_planner_experience_strings(&refs);
    assert_eq!(lines.len(), 1);
    // planner 端 build_planner_user_prompt 自带 "- " 前缀，这里不能重复。
    assert!(!lines[0].starts_with('-'));
    assert!(lines[0].contains("[模式] rust：先 cargo check"));
}

#[test]
fn render_planner_strings_empty_when_no_hits() {
    assert!(render_planner_experience_strings(&[]).is_empty());
}
