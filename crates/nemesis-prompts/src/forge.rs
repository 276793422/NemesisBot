//! Forge 评审/分析提示词。
//!
//! 消费方 nemesis-forge 保留解析逻辑（`parse_insights`/`extract_json`）与
//! 数据驱动的统计 prompt 组装（`build_analysis_prompt`/`build_full_analysis_prompt`
//! 插值 `ExperienceStats`/`ReflectionStats` 等 forge 本地类型，属引擎件
//! 留在 forge）；本模块收敛**纯文本**的评审提示单一真相源。
//!
//! JSON 键与权重是解析契约（消费端按 `correctness`/`quality`/`security`/
//! `reusability` 取数加权），任何一侧都不得单方面改动。

/// Forge 产物质量评审 user prompt 构造（LLM-as-Judge 单一真相源）：
/// evaluator / validator / pipeline 三个调用点共用同一份中文计分卡与 JSON
/// schema（`version` 传 `None` 时省略版本行，兼容不带版本号的调用面）。
pub fn quality_review_prompt(
    kind: &str,
    name: &str,
    version: Option<&str>,
    content: &str,
) -> String {
    let mut sb = String::new();
    sb.push_str("请评审以下 Forge 产物的质量。\n\n");
    sb.push_str(&format!("类型：{}\n", kind));
    sb.push_str(&format!("名称：{}\n", name));
    if let Some(v) = version {
        sb.push_str(&format!("版本：{}\n", v));
    }
    sb.push_str("\n内容：\n");
    sb.push_str(content);
    sb.push_str("\n\n请对每个维度打 0-100 分：\n");
    sb.push_str("- correctness（正确性，权重 40%）：内容是否正确实现其声明的用途？\n");
    sb.push_str("- quality（质量，权重 20%）：代码/文本质量、清晰度、文档完备度。\n");
    sb.push_str("- security（安全性，权重 20%）：安全考量是否到位，无危险模式。\n");
    sb.push_str("- reusability（可复用性，权重 20%）：能否在其他上下文中复用？\n");
    sb.push_str("\n只回复一个 JSON 对象：\n");
    sb.push_str(
        "{\"correctness\": N, \"quality\": N, \"security\": N, \"reusability\": N, \"notes\": \"一句话简要说明\"}",
    );
    sb
}

/// 产物质量评审员 system prompt（三个评审调用点共用）。
pub const QUALITY_REVIEWER_SYSTEM_PROMPT: &str = "你是代码质量评审员。只回复合法的 JSON。";

/// 语义分析 system prompt（`semantic_analysis` 全量统计洞察路径）。
pub const SEMANTIC_ANALYSIS_SYSTEM_PROMPT: &str = "你是智能系统分析员。分析工具使用数据，给出简明、可执行的洞察。\
        聚焦可自动化、可改进或可沉淀为可复用组件的模式。\
        回复控制在 500 字以内。";

/// 技能作者 system prompt（`factory::generate_skill_llm`）。
pub const SKILL_AUTHOR_SYSTEM_PROMPT: &str = "你是智能代理系统的技能作者。生成一份结构良好的 SKILL.md 文档，描述一个可复用的技能。文档应包含 YAML frontmatter（以 --- 定界）、描述、用法说明、示例与注意事项。只回复技能内容本身，不要任何额外解说。";

/// 脚本开发者 system prompt（`factory::generate_script_llm`）。
pub const SCRIPT_AUTHOR_SYSTEM_PROMPT: &str = "你是智能代理系统的脚本开发者。生成一份结构良好的 bash 脚本，用于自动化常见任务。脚本应安全、注释充分、遵循最佳实践。只回复脚本内容本身，不要任何额外解说。";

/// 技能定义生成器 system prompt（`learning_engine::generate_skill_draft`）。
pub const SKILL_GENERATOR_SYSTEM_PROMPT: &str =
    "你是技能定义生成器。生成带 YAML frontmatter 的合法 SKILL.md 内容。";

/// 技能修复 system prompt（`learning_engine::refine_skill_draft`）。
pub const SKILL_FIXER_SYSTEM_PROMPT: &str =
    "你是技能定义生成器。修复未通过校验的技能，返回完整修正后的 SKILL.md。";
