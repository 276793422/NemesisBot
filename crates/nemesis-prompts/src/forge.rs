//! Forge 评审/分析提示词。
//!
//! 消费方 nemesis-forge 保留解析逻辑（`parse_insights`/`extract_json`）与
//! 数据驱动的统计 prompt 组装（`build_analysis_prompt`/`build_full_analysis_prompt`
//! 插值 `ExperienceStats`/`ReflectionStats` 等 forge 本地类型，属引擎件
//! 留在 forge）；本模块收敛**纯文本**的评审提示统一定义。
//!
//! JSON 键与权重是解析契约（消费端按 `correctness`/`quality`/`security`/
//! `reusability` 取数加权），任何一侧都不得单方面改动。

/// Forge 产物质量评审 user prompt 构造（LLM-as-Judge 统一定义）：
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

/// 技能修复 system prompt（自学习铺路批次四装前 tests 闸 refine 环节：
/// `skill_gate::refine_once` 喂失败明细要修正稿；批次四前为保留件）。
pub const SKILL_FIXER_SYSTEM_PROMPT: &str =
    "你是技能定义生成器。修复未通过校验的技能，返回完整修正后的 SKILL.md。";

/// 学习对话引擎 system prompt（自学习铺路批次三）。
///
/// 「LLM 当脑子、Rust 当路」：Rust 摆证据、运对话、落载体、跑护栏；学
/// 什么、学成什么形态、内容怎么写全部由本对话里的 LLM 判断。四个阶段各
/// 输出一个合法 JSON 对象（引擎侧 lenient 解析 + 单次重试，第二次仍非
/// 法 = 该项诚实丢弃）。
pub const LEARN_DIALOGUE_SYSTEM_PROMPT: &str = "你是自学习引擎：从会话证据中识别值得长期沉淀的知识，\
        决定它的载体形态并写出最终内容。全程四个阶段（提议→定形→对账→定稿），每阶段只回复一个合法 JSON 对象，\
        不写 JSON 之外的散文。\n\
        「没有值得学的」是一等答案：证据是噪声、一次性环境问题或已被覆盖时，诚实说没有，不硬编产出。\n\
        载体选择唯一判据是注入经济学：insight = 常驻注入，一句话说得清且每次对话都受益，值得每轮占字节；\
        skill = 按需拉取，内容大/步骤多/低频使用，不值得常驻。\n\
        主语分型是必要判定：每条结论的主语是助手/流程（对 agent 行为的要求）还是用户本人（用户的习惯/偏好）——\
        用户主语的结论不是 agent 资产：不产出 skill，类别改从画像词表选。\n\
        安全红线：不产出任何绕过安全闸门的建议；洞察是 soft 判断倾向，硬约束归安全层，二者不得混淆。";

/// 学习对话阶段 1/4（提议）user prompt：证据包注入。
///
/// JSON 契约（解析端按 `proposals`/`empty_reason` 取数）：
/// `{"proposals": [{"id","title","observation","lesson"}], "empty_reason": "..."}`
/// proposals 为空时 `empty_reason` 必填（「没有值得学的」的一等出口）。
pub fn learn_phase1_prompt(pack_text: &str) -> String {
    let mut sb = String::new();
    sb.push_str("# 学习对话阶段 1/4：提议\n\n");
    sb.push_str("以下是本批学习证据包：\n\n");
    sb.push_str(pack_text);
    sb.push_str(
        "\n\n请判断：这批证据里有什么值得沉淀为长期知识的教训？\n\
         - 证据有两类形状：「提问应答」（agent 停下问→用户答复）与「文本纠正」（用户在 agent \
         输出后用文字直接纠偏/纠错/改范围）——是否真为有价值的纠正、语义真假由你判断；\n\
         - 失败经验是流程级噪声还是暴露了可泛化的教训，也由你判断；\n\
         - 没有值得学的（证据全是噪声/一次性环境/已被覆盖）就返回空 proposals 并给出 empty_reason。\n\n\
         只回复一个 JSON 对象：\n\
         {\"proposals\": [{\"id\": \"p1\", \"title\": \"一句话主题\", \"observation\": \"证据里看到了什么\", \
         \"lesson\": \"值得学什么\"}], \"empty_reason\": \"proposals 为空时必填：为什么没有值得学的\"}",
    );
    sb
}

/// 学习对话阶段 2/4（定形）user prompt：形态决策（注入经济学）。
///
/// JSON 契约：`{"items": [{"id","form","reason"}]}`，form ∈
/// `insight|skill|both|none`（none 合法——有教训但不值得沉淀）。
pub fn learn_phase2_prompt(transcript: &str) -> String {
    let mut sb = String::new();
    sb.push_str("# 学习对话阶段 2/4：定形\n\n");
    sb.push_str("以下是前几轮对话记录：\n\n");
    sb.push_str(transcript);
    sb.push_str(
        "\n\n对每个 proposal 决定交付形态。唯一判据是注入经济学：\n\
         - \"insight\"（常驻洞察）：一句话说得清且每次对话都受益，值得每轮占字节；\n\
         - \"skill\"（按需技能）：内容大/步骤多/低频使用，按需拉取全文而非常驻；\n\
         - \"both\"：既有常驻准则又有完整配方，两边都要；\n\
         - \"none\"：有教训但不值得沉淀（一次性环境问题、通用常识等）。\n\n\
         只回复一个 JSON 对象：\n\
         {\"items\": [{\"id\": \"p1\", \"form\": \"insight|skill|both|none\", \"reason\": \"一句话\"}]}",
    );
    sb
}

/// 学习对话阶段 3/4（对账）user prompt：对照现有台账与技能库去重。
///
/// JSON 契约：`{"items": [{"id","verdict","target","note"}]}`，verdict ∈
/// `keep|duplicate|conflict_drop|supersede`；duplicate/supersede 时
/// `target` 指向现有条目（洞察 id 或技能名）。
pub fn learn_phase3_prompt(transcript: &str, ledger_digest: &str, skills_digest: &str) -> String {
    let mut sb = String::new();
    sb.push_str("# 学习对话阶段 3/4：对账\n\n");
    sb.push_str("以下是前几轮对话记录，以及现有洞察台账与技能库清单：\n\n## 对话记录\n\n");
    sb.push_str(transcript);
    sb.push_str("\n\n## 现有洞察台账\n\n");
    if ledger_digest.trim().is_empty() {
        sb.push_str("（空——还没有任何条目）\n");
    } else {
        sb.push_str(ledger_digest);
        sb.push('\n');
    }
    sb.push_str("\n## 现有技能库\n\n");
    if skills_digest.trim().is_empty() {
        sb.push_str("（空）\n");
    } else {
        sb.push_str(skills_digest);
        sb.push('\n');
    }
    sb.push_str(
        "\n对照现有条目逐项去重，只保留真正新增的知识：\n\
         - \"keep\"：新知识，现有条目没有覆盖；\n\
         - \"duplicate\"：现有条目已覆盖同样内容；\n\
         - \"conflict_drop\"：与现有条目矛盾但没有足够证据推翻它；\n\
         - \"supersede\"：与现有条目矛盾且新证据更可信，应取代旧条目。\n\n\
         只回复一个 JSON 对象：\n\
         {\"items\": [{\"id\": \"p1\", \"verdict\": \"keep|duplicate|conflict_drop|supersede\", \
         \"target\": \"duplicate/supersede 时指向的现有洞察 id 或技能名\", \"note\": \"一句话\"}]}",
    );
    sb
}

/// 学习对话阶段 4/4（定稿）user prompt：最终条目 + skill 草稿（含 tests 段）。
///
/// JSON 契约（解析端按 `insights`/`skills` 取数；双缝互通 2026-11-25 起
/// 带主语分型）：
/// - `{"insights": [{"id","subject","statement","category","evidence","confidence"}],`
/// - `"skills": [{"id","subject","slug","skill_md"}]}`
///
/// subject ∈ `assistant|user`：assistant 条目 category 从行为词表选；
/// user 条目 category 从画像词表选（workflow|correction|tool_preference|
/// style|environment|quality_bar），且只进 insights 数组（用户习惯不是
/// agent 技能——Rust 侧对 subject=user 的 skill 诚实丢弃）。
///
/// `skill_md` = SKILL.md 全文，frontmatter 可带 `tests` 段——格式即
/// `skill_tests.rs` 的 `SkillTestsSpec`（name/turns≤6/fixtures≤8 且单文件
/// ≤64KB/expect 至少一断言或 judge），批次四装前实跑按此消费。
pub fn learn_phase4_prompt(transcript: &str, kept_items: &str) -> String {
    let mut sb = String::new();
    sb.push_str("# 学习对话阶段 4/4：定稿\n\n");
    sb.push_str("以下是前几轮对话记录与对账后保留的条目清单：\n\n## 对话记录\n\n");
    sb.push_str(transcript);
    sb.push_str("\n\n## 对账保留清单\n\n");
    sb.push_str(kept_items);
    sb.push_str(
        "\n\n请为每个 keep/supersede 条目产出最终内容（duplicate/conflict_drop/none 不产出）。\n\n\
         洞察条目要求：\n\
         - statement ≤ 120 字：一句话准则，可执行可判断，不是泛泛建议；\n\
         - subject 判定条目主语：\"assistant\"（对助手/流程的要求——默认）\
         或 \"user\"（对用户本人习惯/偏好的发现）；\n\
         - category 按 subject 分词表：assistant 从 \
         behavior|process_constraint|error_signature|user_preference|efficiency 中选；\n\
           user 从 workflow|correction|tool_preference|style|environment|quality_bar 中选（画像词表）；\n\
         - evidence ≤ 300 字证据摘要；\n\
         - confidence 0.0-1.0。\n\n\
         技能条目要求（skill_md = 完整 SKILL.md 全文）：\n\
         - subject 恒 \"assistant\"：主语为用户的结论只产出洞察条目，不产出技能；\n\
         - 以 --- 定界的 YAML frontmatter（字段：name、description、version: \"1.0\"）；\n\
         - frontmatter 可带 tests 段（行为用例，装前实跑验证）：\n\
           tests: [{name, turns: [{user}], fixtures: [{path, content}], expect: {...}, judge: {rubric, threshold}}]；\n\
           turns ≤ 6 轮；fixtures ≤ 8 个且单文件 ≤ 64KB；每条用例至少一个断言\n\
           （expect 的 tools_called/contains/files_exist 等任一，或 judge.rubric）；\n\
         - 步骤直接可执行，不写「建议」「可以考虑」；路径一律相对/占位符形态；不含真实凭据；\n\
         - 失败学费写成显式禁忌/注意事项。\n\n\
         只回复一个 JSON 对象：\n\
         {\"insights\": [{\"id\": \"p1\", \"subject\": \"assistant|user\", \"statement\": \"...\", \"category\": \"...\", \
         \"evidence\": \"...\", \"confidence\": 0.8}],\n\
          \"skills\": [{\"id\": \"p2\", \"subject\": \"assistant\", \"slug\": \"kebab-case-name\", \"skill_md\": \"完整SKILL.md全文\"}]}\n\n\
         不产出对应形态时对应数组留空数组。",
    );
    sb
}
