//! Cluster node persona generation from JD / resume text.
//!
//! 三阶段机制（信息单元去向追踪 + 覆盖对账）：
//! - 阶段1 分段穷尽提取信息单元（带 key_entities，作为对账基准）
//! - 阶段2 基于单元定向创作 identity/soul/expertise
//! - 阶段3 程序确定性校验（实体字面覆盖 + 段落覆盖）+ 对抗性 LLM 审计 → 覆盖率报告
//!
//! 机制有效性保证：输入的每条信息都有明确去向（进产物 / 显式标注丢弃理由）；
//! 遗漏 = 对账缺口（可枚举、可计数），不靠人眼通读；校验主要靠程序（字面匹配）而非
//! LLM 自觉——程序判定的硬缺口（实体没出现、整段没产出）无法被模型蒙混过关。

use std::sync::Arc;

use nemesis_providers::http_provider::HttpProvider;
use nemesis_providers::router::LLMProvider;
use nemesis_providers::types::{
    ChatOptions, LLMResponse, Message, ToolDefinition, ToolFunctionDefinition,
};
use serde::{Deserialize, Serialize};

/// Minimum input length (in chars) to even attempt generation.
const MIN_INPUT_CHARS: usize = 40;
/// Hard cap on input length to bound cost (extra is truncated).
const MAX_INPUT_CHARS: usize = 20_000;

// ============================================================================
// 产物
// ============================================================================

/// 人格包。identity/soul 是标准集群人格；expertise 是知识库（高密度输入时产出）；
/// coverage 是完整性报告（对账结果），让完整性可审计而非靠人眼通读产出。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaPackage {
    pub node_name: String,
    pub display_name: String,
    pub emoji: String,
    pub role: String,
    pub category: String,
    pub tags: Vec<String>,
    pub identity_md: String,
    pub soul_md: String,
    /// 专业知识库（核心架构方案/踩坑经验的结构化沉淀）。空则不产出。
    #[serde(default)]
    pub expertise_md: String,
    /// 完整性覆盖报告（阶段3 对账结果）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage: Option<CoverageReport>,
}

// ============================================================================
// 信息单元（对账基准）
// ============================================================================

/// 一条信息单元——对账的原子。输入的每条关键技术决策/项目/业务知识/方法论都各成一个单元。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InformationUnit {
    /// 稳定 id（u1, u2, ...），供对账引用。
    pub id: String,
    /// 这条信息的内容。
    pub content: String,
    /// 类型：tech_decision / project / business_domain / methodology / skill / experience_signal。
    pub unit_type: String,
    /// 对人格的相关性：high / medium / low / none。
    pub relevance: String,
    /// 去向：identity / soul / expertise / archive / drop。
    pub disposition: String,
    /// disposition ∈ {archive, drop} 时的理由（为何不纳入人格）。必填。
    #[serde(default)]
    pub drop_reason: Option<String>,
    /// 关键实体词（2-5 个），供程序字面匹配校验覆盖。用原文里会出现的词。
    #[serde(default)]
    pub key_entities: Vec<String>,
}

/// 输入的一个结构段落（用于段落级覆盖反查，抓"整段被跳过"的硬缺口）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputSegment {
    pub id: String,
    pub label: String,
    /// 该段产出的信息单元数。0 = 整段被跳过 = 硬缺口。
    pub unit_count: usize,
}

/// 阶段1 产物：信息单元清单 + 段落覆盖统计。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InformationUnits {
    pub units: Vec<InformationUnit>,
    pub segments: Vec<InputSegment>,
}

// ============================================================================
// 覆盖校验
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CoverageStatus {
    Covered,
    Skipped,
    Missing,
    Suspect,
}

/// 单条覆盖记录：某个信息单元在产物里的覆盖状态。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverageEntry {
    pub unit_id: String,
    pub status: CoverageStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// 完整性报告。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverageReport {
    pub total: usize,
    pub covered: usize,
    pub skipped: usize,
    pub missing: usize,
    pub suspect: usize,
    /// 覆盖率 = covered / target_count（target = disposition 进产物的单元）。
    pub coverage_rate: f64,
    pub entries: Vec<CoverageEntry>,
    /// 整段没产出单元的段落（硬缺口）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub segment_gaps: Vec<String>,
}

impl CoverageReport {
    /// 完整 = 无硬缺口（无 missing 且无 segment_gap）。suspect 是软疑点，不阻断。
    pub fn is_complete(&self) -> bool {
        self.missing == 0 && self.segment_gaps.is_empty()
    }
}

// ============================================================================
// 输入清洗
// ============================================================================

/// Sanitize raw pasted text: drop C0 control chars, normalize line endings, trim,
/// reject too-short input, cap length. Does NOT interpret format.
pub fn sanitize_input(text: &str) -> Result<String, String> {
    let cleaned: String = text
        .chars()
        .filter(|c| (*c >= ' ' && *c != '\u{7f}') || *c == '\n' || *c == '\t')
        .collect();
    let collapsed = cleaned
        .replace('\r', "")
        .lines()
        .map(|l| l.trim_end().to_string())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();
    let len = collapsed.chars().count();
    if len < MIN_INPUT_CHARS {
        return Err(format!(
            "内容太短（约 {len} 字），至少需要 {MIN_INPUT_CHARS} 字才能生成"
        ));
    }
    if len > MAX_INPUT_CHARS {
        return Ok(collapsed.chars().take(MAX_INPUT_CHARS).collect());
    }
    Ok(collapsed)
}

// ============================================================================
// 校验
// ============================================================================

/// Trim + normalize all fields, then enforce invariants. Mutates in place.
pub fn validate(pkg: &mut PersonaPackage) -> Result<(), String> {
    pkg.node_name = pkg.node_name.trim().to_string();
    pkg.display_name = pkg.display_name.trim().to_string();
    pkg.emoji = pkg.emoji.trim().to_string();
    pkg.role = pkg.role.trim().to_string();
    pkg.category = pkg.category.trim().to_string();
    pkg.identity_md = pkg.identity_md.trim().to_string();
    pkg.soul_md = pkg.soul_md.trim().to_string();
    pkg.expertise_md = pkg.expertise_md.trim().to_string();
    pkg.tags = pkg
        .tags
        .iter()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();

    if pkg.node_name.is_empty() {
        return Err("node_name 为空".into());
    }
    if pkg.display_name.is_empty() {
        return Err("display_name 为空".into());
    }
    if pkg.identity_md.is_empty() {
        return Err("identity_md 为空".into());
    }
    if pkg.soul_md.is_empty() {
        return Err("soul_md 为空".into());
    }
    if pkg.role != "worker" && pkg.role != "manager" {
        return Err(format!("role 必须 worker/manager，模型给出 '{}'", pkg.role));
    }
    let emoji: String = pkg.emoji.chars().take(4).collect();
    pkg.emoji = if emoji.is_empty() {
        "🤖".to_string()
    } else {
        emoji
    };
    Ok(())
}

// ============================================================================
// Tool schemas
// ============================================================================

fn persona_tool_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "node_name":    { "type": "string", "description": "英文短标识，kebab-case，如 backend-architect" },
            "display_name": { "type": "string", "description": "中文显示名" },
            "emoji":        { "type": "string", "description": "一个代表该角色的 emoji" },
            "role":         { "type": "string", "enum": ["worker", "manager"], "description": "集群角色，专家节点用 worker" },
            "category":     { "type": "string", "description": "分类，如 development / data / devops" },
            "tags":         { "type": "array", "items": { "type": "string" }, "description": "4-8 个【具体】技术栈/领域词，禁纯软技能（沟通/协作）" },
            "identity_md":  { "type": "string", "description": "完整 IDENTITY.md。固定四节：## 定位 / ## 业务领域 / ## 专长 / ## 方法论与性格" },
            "soul_md":      { "type": "string", "description": "完整 SOUL.md。固定四节：## 工作哲学 / ## 行为准则 / ## 沟通风格 / ## 边界" },
            "expertise_md": { "type": "string", "description": "EXPERTISE.md 知识库。把核心架构方案/踩坑经验结构化（每个：问题/方案/关键细节）。无则空字符串。" }
        },
        "required": ["node_name", "display_name", "emoji", "role", "category", "tags", "identity_md", "soul_md", "expertise_md"]
    })
}

fn units_tool_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "units": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "id":            { "type": "string", "description": "稳定 id：u1, u2, ..." },
                        "content":       { "type": "string", "description": "这条信息的内容" },
                        "unit_type":     { "type": "string", "enum": ["tech_decision","project","business_domain","methodology","skill","experience_signal"] },
                        "relevance":     { "type": "string", "enum": ["high","medium","low","none"] },
                        "disposition":   { "type": "string", "enum": ["identity","soul","expertise","archive","drop"] },
                        "drop_reason":   { "type": "string", "description": "disposition=archive/drop 时必填：为何不纳入人格" },
                        "key_entities":  { "type": "array", "items": { "type": "string" }, "description": "2-5 个关键实体词，用原文里会出现的词（供字面匹配校验覆盖）" }
                    },
                    "required": ["id","content","unit_type","relevance","disposition","key_entities"]
                }
            },
            "segments": {
                "type": "array",
                "description": "输入的结构段落清单 + 每段产出的单元数（用于抓整段被跳过的硬缺口）",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "id":          { "type": "string" },
                        "label":       { "type": "string", "description": "段落名，如「技能」「工作经历-司顺」「项目-OMS」" },
                        "unit_count":  { "type": "integer", "description": "该段产出的 unit 数；0 = 整段被跳过（硬缺口）" }
                    },
                    "required": ["id","label","unit_count"]
                }
            }
        },
        "required": ["units", "segments"]
    })
}

fn audit_tool_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "entries": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "unit_id":  { "type": "string", "description": "被判定单元的 id" },
                        "status":   { "type": "string", "enum": ["covered","missing","suspect"], "description": "covered=含义确实体现了；missing=完全没体现；suspect=模糊" },
                        "location": { "type": "string", "description": "covered 时填：体现在哪个产物的哪一节" },
                        "reason":   { "type": "string", "description": "missing/suspect 时填理由" }
                    },
                    "required": ["unit_id","status"]
                }
            }
        },
        "required": ["entries"]
    })
}

fn persona_tool_def() -> ToolDefinition {
    ToolDefinition {
        tool_type: "function".to_string(),
        function: ToolFunctionDefinition {
            name: "emit_cluster_persona".to_string(),
            description: "基于信息单元清单创作集群节点人格包，必须调用此工具返回结果，不要输出其它内容。"
                .to_string(),
            parameters: persona_tool_schema(),
        },
    }
}

fn units_tool_def() -> ToolDefinition {
    ToolDefinition {
        tool_type: "function".to_string(),
        function: ToolFunctionDefinition {
            name: "extract_information_units".to_string(),
            description: "把输入穷尽拆成信息单元 + 段落覆盖统计，必须调用此工具返回结果，不要输出其它内容。"
                .to_string(),
            parameters: units_tool_schema(),
        },
    }
}

fn audit_tool_def() -> ToolDefinition {
    ToolDefinition {
        tool_type: "function".to_string(),
        function: ToolFunctionDefinition {
            name: "audit_coverage".to_string(),
            description: "完整性审计：逐条判定信息单元是否在产物里体现，必须调用此工具返回结果。"
                .to_string(),
            parameters: audit_tool_schema(),
        },
    }
}

// ============================================================================
// Prompts
// ============================================================================

fn extract_prompt(kind: &str) -> String {
    let orientation = if kind == "resume" {
        "用户给你一份简历（任意格式）。"
    } else {
        "用户给你一份 JD / 岗位描述（任意格式）。"
    };
    format!(
"你是简历/JD 分析师。{orientation}把输入【穷尽】拆成信息单元（information units）+ 段落覆盖统计。

方法：
1. 先识别输入的结构段落（如「技能」「工作经历-司顺」「项目-OMS」「任职要求」等），每段一个 segment。
2. 对每段穷尽提取其中的关键技术决策 / 项目难点 / 业务知识 / 方法论 / 经验信号，每个一个 unit。
3. 每个 unit 标注：id（u1,u2...）、content、unit_type、relevance（对人格的相关性）、disposition（去向）、key_entities（2-5 个关键实体词）。

铁律：
1. relevance=none 的也要列出（disposition=drop 或 archive，并填 drop_reason），【绝不默默丢弃】任何输入信息。
2. segment.unit_count 必须如实反映该段产出的 unit 数；某段若被整体跳过，unit_count=0（会被程序标记为硬缺口）。
3. key_entities 必须是【原文产物里会字面出现】的词（如 RocketMQ、事务消息、分库分表），不要写成「消息中间件」「性能优化」这种泛词——后续程序靠字面匹配校验覆盖。
4. disposition 决定该 unit 去向：核心架构方案/踩坑经验→expertise；身份/专长→identity；工作方式/准则→soul；与人格无关→drop+理由；冗余备查→archive。

你必须调用 extract_information_units 工具返回 {{units, segments}}，不要输出任何其它文字。"
    )
}

fn author_prompt(kind: &str, missing_hint: Option<&str>) -> String {
    let orientation = if kind == "resume" {
        "把这份简历转化成一个具备这些技能与经验的集群节点人格。"
    } else {
        "把这份 JD 转化成一个能胜任该岗位的集群节点人格。"
    };
    let hint = match missing_hint {
        Some(h) => format!(
            "\n\n⚠️ 上一轮覆盖校验发现以下单元的 key_entities 没出现在产物里，本次【必须】把它们补进对应产物：\n{h}"
        ),
        None => String::new(),
    };
    format!(
"你是集群节点人格设计师。{orientation}给你：① 原始输入 ② 信息单元清单（每个 unit 标了去向 disposition 和 key_entities）。

⚠️ 核心心态：你在【转化这份具体输入】，不是【套一个通用工程师模板】。如果换一份同岗位的简历、你的人格几乎不变，那就是失败。

硬要求：
1. identity_md 固定四节：## 定位（一句话角色本质+最熟的战场）/ ## 业务领域（这个角色【懂什么业务】，必填）/ ## 专长（写「我用 X 做过 Y / 治理过 Z」的故事，【禁止技能清单】）/ ## 方法论与性格（工作范式落到具体形态，不停在口号）。
2. soul_md 固定四节：## 工作哲学 / ## 行为准则 / ## 沟通风格 / ## 边界。
   行为准则每条【必须锚定一个 unit 里的真实技术决策】。【禁止行业通用最佳实践】——以下绝对不许出现：
   「要注重性能」「要保证一致性」「要保证可扩展」「善于沟通」「有团队精神」「持续学习」「解决问题能力强」以及任何换个角色也能用、不可证伪的泛泛之词。
3. expertise_md：把 disposition=expertise 的 unit（核心架构方案/踩坑经验）结构化沉淀，每个方案写「问题 / 方案 / 关键细节」。
4. 落点约束：每个 disposition ∈ {{identity, soul, expertise}} 的 unit，其 key_entities 必须字面出现在对应产物里（程序会校验，漏了会判 missing）。
5. 结构字段（年限/学历/公司）不许编造；专家级默认关切（后端→并发/一致性/可观测性；前端→性能/可访问性；安全→纵深/最小权限）可基于输入合理演绎并体现。
6. tags 用具体技术栈/领域词，禁纯软技能。{hint}

你必须调用 emit_cluster_persona 工具返回 identity_md / soul_md / expertise_md + 身份字段，不要输出任何其它文字。"
    )
}

fn audit_prompt() -> String {
    "你是完整性审计员，任务是【找漏洞】——假设生成的人格必有遗漏，把它找出来。你不是创作者的帮手。

给你：① 信息单元清单（每个 unit 有 id / disposition / key_entities）② 生成的人格产物（identity_md / soul_md / expertise_md）。

只判定 disposition ∈ {identity, soul, expertise} 的 unit，逐条给出：
- covered：该 unit 的 key_entities 的【含义】确实在对应产物的某一节里体现了（不只是词在，意思要到位）。location 填具体哪一节。
- missing：该 unit 的含义在产物里完全没体现。
- suspect：模糊，介于两者之间。

铁律：宁可多报 missing/suspect，不要轻易判 covered。判 covered 时必须能在产物里【指出具体哪句】对应这个 unit；指不出就报 missing/suspect。

你必须调用 audit_coverage 工具返回 entries，不要输出任何其它文字。".to_string()
}

fn mk_msg(role: &str, content: String) -> Message {
    Message {
        role: role.to_string(),
        content,
        tool_calls: Vec::new(),
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: std::collections::HashMap::new(),
    }
}

// ============================================================================
// 输出提取（robust to provider quirks）
// ============================================================================

fn unwrap_single_key(v: serde_json::Value) -> serde_json::Value {
    // Some providers wrap tool args as {"<tool_name>": {...}}. Unwrap only when
    // the single inner object actually looks like one of our schemas.
    if let Some(obj) = v.as_object() {
        if obj.len() == 1 {
            if let Some(inner) = obj.values().next() {
                if inner.is_object()
                    && (inner.get("identity_md").is_some()
                        || inner.get("units").is_some()
                        || inner.get("entries").is_some())
                {
                    return inner.clone();
                }
            }
        }
    }
    v
}

fn extract_json_span(s: &str) -> Option<&str> {
    let start = s.find('{')?;
    let end = s.rfind('}')?;
    if end >= start {
        Some(&s[start..=end])
    } else {
        None
    }
}

fn strip_code_fence(s: &str) -> String {
    let t = s.trim();
    if !t.starts_with("```") {
        return t.to_string();
    }
    let after = match t.find('\n') {
        Some(i) => t[i + 1..].to_string(),
        None => return t.to_string(),
    };
    let after = after.trim_end();
    if let Some(stripped) = after.strip_suffix("```") {
        stripped.trim().to_string()
    } else {
        after.to_string()
    }
}

/// 通用：从 LLM 响应里提取首个 tool_call 的 JSON 参数，退化到文本里的 JSON。
fn extract_response_json(resp: &LLMResponse) -> Result<serde_json::Value, String> {
    for tc in &resp.tool_calls {
        if let Some(func) = &tc.function {
            let args = func.arguments.trim();
            if !args.is_empty() {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(args) {
                    return Ok(unwrap_single_key(v));
                }
            }
        }
        if let Some(map) = &tc.arguments {
            if let Ok(v) = serde_json::to_value(map) {
                return Ok(unwrap_single_key(v));
            }
        }
    }
    let cleaned = strip_code_fence(&resp.content);
    let candidate = extract_json_span(&cleaned).unwrap_or(cleaned.as_str());
    match serde_json::from_str::<serde_json::Value>(candidate) {
        Ok(v) => Ok(unwrap_single_key(v)),
        Err(e) => Err(format!(
            "无法把模型输出解析为 JSON（{}）；前 200 字预览：{}",
            e,
            resp.content.chars().take(200).collect::<String>()
        )),
    }
}

// ============================================================================
// 程序确定性校验（机制的硬骨架，100% 程序判定）
// ============================================================================

/// 段落级覆盖：unit_count==0 的段落 = 整段被跳过 = 硬缺口。
fn check_segment_coverage(units: &InformationUnits) -> Vec<String> {
    units
        .segments
        .iter()
        .filter(|s| s.unit_count == 0)
        .map(|s| format!("段落「{}」产出 0 个单元（整段被跳过）", s.label))
        .collect()
}

/// 实体级覆盖：对每个 disposition ∈ {identity,soul,expertise} 的 unit，检查其 key_entities
/// 是否字面出现在对应产物里。全部命中=Covered；有缺失=Missing（硬缺口，附缺失实体）。
fn check_entity_coverage(units: &[InformationUnit], pkg: &PersonaPackage) -> Vec<CoverageEntry> {
    let mut entries = Vec::new();
    for u in units {
        let target = match u.disposition.as_str() {
            "identity" => &pkg.identity_md,
            "soul" => &pkg.soul_md,
            "expertise" => &pkg.expertise_md,
            _ => continue, // archive/drop 不查字面
        };
        if u.key_entities.is_empty() {
            // 无 key_entities 无法字面校验，跳过（不判 Missing，避免误报）。
            continue;
        }
        let target_lower = target.to_lowercase();
        let missed: Vec<&str> = u
            .key_entities
            .iter()
            .filter(|e| !target_lower.contains(&e.to_lowercase()))
            .map(|e| e.as_str())
            .collect();
        let status = if missed.is_empty() {
            CoverageStatus::Covered
        } else {
            CoverageStatus::Missing
        };
        let reason = if missed.is_empty() {
            None
        } else {
            Some(format!(
                "key_entities 未在 {} 字面出现：{}",
                u.disposition,
                missed.join(", ")
            ))
        };
        entries.push(CoverageEntry {
            unit_id: u.id.clone(),
            status,
            location: Some(u.disposition.clone()),
            reason,
        });
    }
    entries
}

/// 合成覆盖率报告：程序字面校验（硬）+ 对抗审计（语义）合并。
/// - 程序判 Missing → Missing（硬缺口，程序说了算，最高优先级）
/// - 程序 Covered + 审计 covered → Covered
/// - 程序 Covered + 审计 missing/suspect → Suspect（字面在但语义存疑，软疑点）
/// - 程序 Covered + 审计无此条 → Covered（信程序字面）
fn build_coverage_report(
    units: &[InformationUnit],
    prog: Vec<CoverageEntry>,
    audit: Vec<CoverageEntry>,
    segment_gaps: Vec<String>,
) -> CoverageReport {
    use std::collections::HashMap;
    let target_ids: Vec<&str> = units
        .iter()
        .filter(|u| matches!(u.disposition.as_str(), "identity" | "soul" | "expertise"))
        .map(|u| u.id.as_str())
        .collect();
    let prog_map: HashMap<&str, &CoverageEntry> =
        prog.iter().map(|e| (e.unit_id.as_str(), e)).collect();
    let audit_map: HashMap<&str, &CoverageEntry> =
        audit.iter().map(|e| (e.unit_id.as_str(), e)).collect();

    let mut entries = Vec::new();
    let mut covered = 0;
    let mut missing = 0;
    let mut suspect = 0;

    for id in &target_ids {
        let p = prog_map.get(id);
        let a = audit_map.get(id);
        let status = if let Some(pe) = p {
            if matches!(pe.status, CoverageStatus::Missing) {
                CoverageStatus::Missing
            } else if let Some(ae) = a {
                match ae.status {
                    CoverageStatus::Missing | CoverageStatus::Suspect => CoverageStatus::Suspect,
                    _ => CoverageStatus::Covered,
                }
            } else {
                CoverageStatus::Covered
            }
        } else {
            // 程序未判（无 key_entities 的 target unit）→ 信审计，审计也没有则 Suspect。
            a.map(|ae| ae.status.clone()).unwrap_or(CoverageStatus::Suspect)
        };
        let reason = match &status {
            CoverageStatus::Missing => p.and_then(|pe| pe.reason.clone()),
            CoverageStatus::Suspect => a
                .and_then(|ae| ae.reason.clone())
                .or_else(|| p.and_then(|pe| pe.reason.clone())),
            _ => None,
        };
        let location = p
            .and_then(|pe| pe.location.clone())
            .or_else(|| a.and_then(|ae| ae.location.clone()));
        match status {
            CoverageStatus::Covered => covered += 1,
            CoverageStatus::Missing => missing += 1,
            CoverageStatus::Suspect => suspect += 1,
            CoverageStatus::Skipped => {}
        }
        entries.push(CoverageEntry {
            unit_id: (*id).to_string(),
            status,
            location,
            reason,
        });
    }

    let skipped = units
        .iter()
        .filter(|u| matches!(u.disposition.as_str(), "archive" | "drop"))
        .count();
    let total = units.len();
    let target_count = target_ids.len();
    let coverage_rate = if target_count == 0 {
        1.0
    } else {
        covered as f64 / target_count as f64
    };

    CoverageReport {
        total,
        covered,
        skipped,
        missing,
        suspect,
        coverage_rate,
        entries,
        segment_gaps,
    }
}

// ============================================================================
// LLM 阶段
// ============================================================================

async fn chat_json(
    provider: &Arc<HttpProvider>,
    model: &str,
    system: &str,
    user: String,
    tool: ToolDefinition,
    temperature: f64,
) -> Result<serde_json::Value, String> {
    let messages = vec![mk_msg("system", system.to_string()), mk_msg("user", user)];
    let opts = ChatOptions {
        temperature: Some(temperature),
        max_tokens: Some(8192),
        top_p: None,
        stop: None,
        extra: std::collections::HashMap::new(),
    };
    let resp = (&**provider)
        .chat(&messages, &[tool], model, &opts)
        .await
        .map_err(|e| format!("LLM 调用失败: {:?}", e))?;
    extract_response_json(&resp)
}

/// 阶段1：分段穷尽提取信息单元。
async fn extract_units(
    provider: &Arc<HttpProvider>,
    model: &str,
    kind: &str,
    clean: &str,
) -> Result<InformationUnits, String> {
    let v = chat_json(
        provider,
        model,
        &extract_prompt(kind),
        clean.to_string(),
        units_tool_def(),
        0.2,
    )
    .await?;
    serde_json::from_value::<InformationUnits>(v).map_err(|e| format!("解析信息单元失败: {}", e))
}

/// 阶段2：基于信息单元创作人格。
async fn author_persona(
    provider: &Arc<HttpProvider>,
    model: &str,
    kind: &str,
    clean: &str,
    units: &InformationUnits,
    missing_hint: Option<&str>,
) -> Result<PersonaPackage, String> {
    let user = format!(
        "【原始输入】\n{clean}\n\n【信息单元清单】\n{}",
        serde_json::to_string_pretty(units).unwrap_or_else(|_| serde_json::to_string(units).unwrap_or_default())
    );
    let v = chat_json(
        provider,
        model,
        &author_prompt(kind, missing_hint),
        user,
        persona_tool_def(),
        0.7,
    )
    .await?;
    serde_json::from_value::<PersonaPackage>(v).map_err(|e| format!("解析人格包失败: {}", e))
}

/// 阶段3-审计：对抗性 LLM 审计（找茬模式）。失败时返回空（程序校验兜底）。
async fn audit_coverage(
    provider: &Arc<HttpProvider>,
    model: &str,
    units: &InformationUnits,
    pkg: &PersonaPackage,
) -> Result<Vec<CoverageEntry>, String> {
    let user = format!(
        "【信息单元清单】\n{}\n\n【生成的人格产物】\nIDENTITY.md:\n{}\n\nSOUL.md:\n{}\n\nEXPERTISE.md:\n{}",
        serde_json::to_string_pretty(&units.units).unwrap_or_default(),
        pkg.identity_md,
        pkg.soul_md,
        if pkg.expertise_md.is_empty() { "（无）" } else { &pkg.expertise_md }
    );
    let v = chat_json(provider, model, &audit_prompt(), user, audit_tool_def(), 0.2).await?;
    #[derive(Deserialize)]
    struct AuditOut {
        entries: Vec<CoverageEntry>,
    }
    let out: AuditOut = serde_json::from_value(v).map_err(|e| format!("解析审计结果失败: {}", e))?;
    Ok(out.entries)
}

// ============================================================================
// 主流程
// ============================================================================

/// 从 JD/简历文本生成人格包（三阶段 + 覆盖校验，带最多 `max_attempts` 次补全重试）。
///
/// 覆盖校验失败不硬 Err——返回带覆盖率报告的 pkg（report 里标清 missing/segment_gaps），
/// 让调用方/用户审计完整性。只有 LLM 调用/解析彻底失败才 Err。
pub async fn generate_persona(
    provider: &Arc<HttpProvider>,
    model: &str,
    kind: &str,
    text: &str,
    max_attempts: usize,
) -> Result<PersonaPackage, String> {
    let clean = sanitize_input(text)?;

    // 阶段1：提取信息单元（对账基准）。
    let units = extract_units(provider, model, kind, &clean).await?;
    let seg_gaps = check_segment_coverage(&units);
    if !seg_gaps.is_empty() {
        tracing::warn!(gaps = ?seg_gaps, "[Persona] 阶段1 段落覆盖缺口（整段被跳过）");
    }

    let mut last_pkg: Option<PersonaPackage> = None;
    let mut last_report: Option<CoverageReport> = None;
    let mut missing_hint: Option<String> = None;

    for attempt in 0..max_attempts {
        // 阶段2：创作。
        let mut pkg = match author_persona(
            provider,
            model,
            kind,
            &clean,
            &units,
            missing_hint.as_deref(),
        )
        .await
        {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, attempt, "[Persona] 阶段2 创作失败");
                missing_hint = Some(format!("（上轮创作/解析失败：{e}）"));
                continue;
            }
        };
        if let Err(e) = validate(&mut pkg) {
            tracing::warn!(error = %e, attempt, "[Persona] 校验失败");
            missing_hint = Some(format!("（上轮校验失败：{e}）"));
            continue;
        }

        // 阶段3：程序确定性校验。
        let prog = check_entity_coverage(&units.units, &pkg);
        // 阶段3：对抗性 LLM 审计（失败则空，程序校验兜底）。
        let audit = match audit_coverage(provider, model, &units, &pkg).await {
            Ok(a) => a,
            Err(e) => {
                tracing::warn!(error = %e, "[Persona] 对抗审计失败，仅用程序校验");
                Vec::new()
            }
        };

        let report = build_coverage_report(&units.units, prog, audit, seg_gaps.clone());
        tracing::info!(
            attempt,
            coverage_rate = %report.coverage_rate,
            covered = report.covered,
            missing = report.missing,
            suspect = report.suspect,
            seg_gaps = report.segment_gaps.len(),
            "[Persona] 覆盖率报告"
        );

        if report.is_complete() {
            pkg.coverage = Some(report);
            return Ok(pkg);
        }

        // 未通过：把 missing 明细喂回下轮创作。
        let missing_units: Vec<String> = report
            .entries
            .iter()
            .filter(|e| matches!(e.status, CoverageStatus::Missing))
            .map(|e| {
                let u = units.units.iter().find(|u| u.id == e.unit_id);
                let ents = u
                    .map(|u| u.key_entities.join("/"))
                    .unwrap_or_default();
                format!(
                    "- unit {} (→{}): {}{}",
                    e.unit_id,
                    e.location.as_deref().unwrap_or("?"),
                    ents,
                    e.reason
                        .as_deref()
                        .map(|r| format!("；{}", r))
                        .unwrap_or_default()
                )
            })
            .collect();
        missing_hint = if missing_units.is_empty() && seg_gaps.is_empty() {
            None
        } else {
            Some(if missing_units.is_empty() {
                format!("段落缺口：{}", seg_gaps.join("; "))
            } else {
                missing_units.join("\n")
            })
        };

        last_pkg = Some(pkg);
        last_report = Some(report);
    }

    // 重试耗尽：返回最后的 pkg + 报告（标清缺口），不硬 Err。
    if let Some(mut pkg) = last_pkg {
        pkg.coverage = last_report;
        tracing::warn!(
            "[Persona] 覆盖校验未在 {} 次内完全通过，返回带缺口报告的 pkg",
            max_attempts
        );
        return Ok(pkg);
    }
    Err(format!(
        "生成失败：{} 次尝试均未能产出有效人格包",
        max_attempts
    ))
}

#[cfg(test)]
mod tests;
