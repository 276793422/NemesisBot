// team_memory.rs 覆盖率补充测试（format_experience_line 的空 source/空
// author 组合臂 74/79/81/86/88——既有测试全走「source+author 双全」的
// 收尾臂）。

use super::*;
use crate::models::{TeamMemoryEntry, team_memory_category};

fn entry(id: i64, source: &str, author: &str) -> TeamMemoryEntry {
    TeamMemoryEntry {
        id,
        category: team_memory_category::PATTERN.to_string(),
        scope: "rust".to_string(),
        content: "先编译再改".to_string(),
        source: source.to_string(),
        author: author.to_string(),
        use_count: 0,
        deprecated: false,
        created_at: 1_700_000_000,
    }
}

/// 三种来源/作者组合各自落到正确的尾巴形态：
/// - 双空 → 无尾巴（74/79/86）；
/// - 仅作者 → 「（蒸馏者 x）」单尾（81/88）；
/// - 双全 → 「（来源 s、蒸馏者 a）」合尾（既有测试已盖）。
#[test]
fn experience_line_tail_forms() {
    let lines = render_planner_experience_strings(&[&entry(1, "   ", ""), &entry(2, "", "node-x")]);
    assert_eq!(lines.len(), 2);
    // 双空：scope：content 后无任何尾巴。
    assert_eq!(lines[0], "[模式] rust：先编译再改");
    // 仅作者：来源段缺席，作者以蒸馏者形态收尾。
    assert_eq!(lines[1], "[模式] rust：先编译再改（蒸馏者 node-x）");
}
