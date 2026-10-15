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

/// 技能定义生成器 system prompt（`learning_engine::generate_skill_draft`）。
pub const SKILL_GENERATOR_SYSTEM_PROMPT: &str =
    "你是技能定义生成器。生成带 YAML frontmatter 的合法 SKILL.md 内容。";

/// 技能修复 system prompt（`learning_engine::refine_skill_draft`）。
pub const SKILL_FIXER_SYSTEM_PROMPT: &str =
    "你是技能定义生成器。修复未通过校验的技能，返回完整修正后的 SKILL.md。";

/// 程序性知识蒸馏 system prompt（`reflector_llm::distill_skill_draft`，
/// artifacts.auto_skill 生成面）。
pub const SKILL_DISTILLATION_SYSTEM_PROMPT: &str = "你是程序性知识蒸馏器。从工具使用经验序列中识别\
        「同场景下工具序列多次成功收敛」的模式，产出可直接执行的步骤化 SKILL.md 草稿——\
        不是泛化建议，是别人拿着就能一步步照做的操作配方。只回复合法 JSON。";

/// 程序性知识蒸馏 user prompt 构造（`artifacts.auto_skill` 生成面）。
///
/// 输入是已脱敏的时序经验摘要（`build_experience_digest` 产出）与现有
/// Forge 产物清单；输出 JSON 契约（解析端按 `has_skill`/`name`/`skill_md`
/// 取数，任何一侧不得单方面改动）：
/// `{"has_skill": bool, "name": "...", "skill_md": "...", "rationale": "..."}`
pub fn skill_distillation_prompt(experiences_digest: &str, artifacts_list: &str) -> String {
    let mut sb = String::new();
    sb.push_str("以下是一段时间内按时间排序的工具使用经验（已脱敏）。\n\n");
    sb.push_str("## 经验序列\n");
    sb.push_str(experiences_digest);
    sb.push_str("\n\n## 已有 Forge 产物（勿重复产出同题材）\n");
    if artifacts_list.trim().is_empty() {
        sb.push_str("（无）\n");
    } else {
        sb.push_str(artifacts_list);
        sb.push('\n');
    }
    sb.push_str(
        "\n请判断：是否存在某个「同场景工具序列多次成功收敛」的模式，值得沉淀为\
         可复用的程序性技能（操作配方）？判断标准：\n\
         - 该序列在同场景中反复出现且最终成功（失败-修正-成功的收敛也算，\
         修正经验本身就是配方的一部分）；\n\
         - 步骤可泛化到其他机器/工作区（不绑定特定路径或一次性环境）；\n\
         - 是程序性知识（怎么做），不是统计观察（用了多少）。\n\n\
         若存在，生成完整 SKILL.md 草稿。形式要求：\n\
         - 以 --- 定界的 YAML frontmatter（字段：name、description、version:\"1.0\"）；\n\
         - 步骤直接可执行（具体命令/具体判断），不写「建议」「可以考虑」；\n\
         - 路径一律相对/占位符形态（如 <workspace>），不出现真实机器路径；\n\
         - 不含任何真实凭据（引用凭据时写「通过 vault/环境变量注入」）；\n\
         - 失败学费写成显式禁忌/注意事项，让读者不再付一遍。\n\n\
         若不存在（或已有产物覆盖同题材），has_skill 填 false，name/skill_md 留空串。\n\n\
         只回复一个 JSON 对象：\n\
         {\"has_skill\": true, \"name\": \"kebab-case-技能名\", \"skill_md\": \"完整SKILL.md内容\", \
         \"rationale\": \"一句话说明沉淀理由\"}",
    );
    sb
}
