//! S9 覆盖率批次：context.rs 剩余未覆盖行。
//! - 274：load_skills 真实扫描目录（SKILL.md 加载成功 → if-let 块收尾）。
//! - 334/508-509：debug! 参数表达式行（需 subscriber）。
//! - 408：空 description 的 skill 渲染 "(no description)"。

use super::*;
use crate::test_support::capture_logs;
use crate::types::ConversationTurn;

fn temp_ws(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "nemesis_ctx_s9_{}_{}_{}",
        tag,
        std::process::id(),
        line!()
    ))
}

/// load_skills 从目录读 SKILL.md → skills_info 填充（274 循环收尾）。
#[test]
fn load_skills_reads_skill_md_from_dir() {
    let ws = temp_ws("loadskills");
    let _ = std::fs::remove_dir_all(&ws);
    let skills = ws.join("skills").join("weather");
    std::fs::create_dir_all(&skills).unwrap();
    std::fs::write(
        skills.join("SKILL.md"),
        "# Weather skill\n\nqueries the weather",
    )
    .unwrap();
    // 一个没有 SKILL.md 的子目录（走 skip 路径）
    std::fs::create_dir_all(ws.join("skills").join("empty")).unwrap();

    let mut builder = ContextBuilder::new(&ws);
    builder.load_skills(&ws.join("skills"));
    let infos = builder.get_skills_info();
    assert_eq!(infos.len(), 1, "only the SKILL.md-bearing dir loads");
    assert_eq!(infos[0].name, "weather");
    assert!(infos[0].description.contains("Weather skill"));
    let _ = std::fs::remove_dir_all(&ws);
}

/// build_system_prompt 的 debug! 参数行（334）+ 空 description 渲染占位（408）。
#[test]
fn build_system_prompt_with_empty_desc_skill_logs_debug() {
    let _logs = capture_logs();
    let ws = temp_ws("emptydesc");
    let _ = std::fs::remove_dir_all(&ws);
    std::fs::create_dir_all(&ws).unwrap();
    let mut builder = ContextBuilder::new(&ws);
    builder.set_skills_info(vec![SkillInfo {
        name: "silent".to_string(),
        description: String::new(),
        active: true,
    }]);
    let prompt = builder.build_system_prompt(false);
    assert!(
        prompt.contains("(no description)"),
        "empty desc renders placeholder"
    );
    assert!(prompt.contains("silent"));
    let _ = std::fs::remove_dir_all(&ws);
}

/// build_messages 的 debug! 参数行（508-509）。
#[test]
fn build_messages_logs_debug_lines() {
    let _logs = capture_logs();
    let ws = temp_ws("buildmsgs");
    let _ = std::fs::remove_dir_all(&ws);
    std::fs::create_dir_all(&ws).unwrap();
    let builder = ContextBuilder::new(&ws);
    let history = vec![ConversationTurn {
        role: "user".to_string(),
        content: "hi".to_string(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        timestamp: String::new(),
        reasoning_content: None,
        tool_name: None,
        tool_result_projection: None,
    }];
    let msgs = builder.build_messages(
        &history,
        "summary of earlier talk",
        "hello",
        "web",
        "chat1",
        false,
    );
    assert!(msgs.len() >= 2, "system + history + current");
    assert_eq!(msgs[0].role, "system");
    let _ = std::fs::remove_dir_all(&ws);
}
