//! prompt 引擎门面单测：体系解析、tier→档位映射、查表回落、预算告警。
//! 资产本体（段落池/描述表/角色模板）的结构不变量测试在 nemesis-prompts
//! crate。

use super::*;
use nemesis_types::capability::ModelTier;

// ---------------------------------------------------------------------------
// PromptSystem::parse
// ---------------------------------------------------------------------------

#[test]
fn parse_empty_and_pro_default_to_pro() {
    // 空串 = 未配置（含 Default 派生空串）→ pro。
    assert_eq!(PromptSystem::parse(""), PromptSystem::Pro);
    assert_eq!(PromptSystem::parse("  "), PromptSystem::Pro);
    assert_eq!(PromptSystem::parse("pro"), PromptSystem::Pro);
}

#[test]
fn parse_classic_passthrough() {
    assert_eq!(PromptSystem::parse("classic"), PromptSystem::Classic);
    assert_eq!(PromptSystem::parse(" classic "), PromptSystem::Classic);
}

#[test]
fn parse_unknown_failsafe_to_classic() {
    // 未知值宁可保守回旧体系，不猜。
    assert_eq!(PromptSystem::parse("fancy"), PromptSystem::Classic);
    assert_eq!(PromptSystem::parse("PRO"), PromptSystem::Classic); // 大小写敏感，显式约定
}

// ---------------------------------------------------------------------------
// 预算告警
// ---------------------------------------------------------------------------

#[test]
fn budget_check_does_not_panic_below_and_above() {
    check_budget(0);
    check_budget(SOFT_BUDGET_BYTES);
    check_budget(SOFT_BUDGET_BYTES + 1); // 只告警不截断，调用方无感
}

// ---------------------------------------------------------------------------
// 工具描述查表（tier 映射 + 回落语义）
// ---------------------------------------------------------------------------

#[test]
fn tool_description_falls_back_verbatim_when_table_misses() {
    let fallback = "注册表原始描述";
    assert_eq!(
        tool_description("no_such_tool", fallback, ModelTier::Big),
        fallback,
        "查表未命中必须原文回落（forge/board/动态 MCP 依赖此语义）"
    );
}

#[test]
fn desc_level_mapping_mini_lean_others_full() {
    assert_eq!(desc_level_for(ModelTier::Mini), DescLevel::Lean);
    assert_eq!(desc_level_for(ModelTier::Normal), DescLevel::Full);
    assert_eq!(desc_level_for(ModelTier::Big), DescLevel::Full);
    // Auto 理论上构造期已 resolve；防御口径 = full 侧。
    assert_eq!(desc_level_for(ModelTier::Auto), DescLevel::Full);
}

#[test]
fn tool_description_serves_level_appropriate_text() {
    // 命中表项：mini 恒拿 lean；big 拿 full 且以 lean 首句开头。
    let lean = tool_description("exec", "fallback", ModelTier::Mini);
    let full = tool_description("exec", "fallback", ModelTier::Big);
    assert_ne!(lean, "fallback", "exec 应命中描述表");
    assert_eq!(lean.trim(), first_sentence(full).trim());
    assert!(full.len() > lean.len(), "full 档应比 lean 档更详尽");
}

// ---------------------------------------------------------------------------
// from_config 便捷入口
// ---------------------------------------------------------------------------

#[test]
fn from_config_reads_raw_key() {
    let mut cfg = nemesis_config::AgentsConfig::default();
    // 派生 Default 的空串 = 未配置 = pro。
    assert_eq!(PromptSystem::from_config(&cfg), PromptSystem::Pro);
    cfg.prompt_system = "classic".into();
    assert_eq!(PromptSystem::from_config(&cfg), PromptSystem::Classic);
}
