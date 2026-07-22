//! Cluster node persona generation from JD / resume text.
//!
//! Format-agnostic by design: arbitrary-format input is fed to the LLM as an
//! *extraction* task (never parsed). The model is forced to return a structured
//! persona package via a tool-call whose parameters are the persona JSON schema.
//! We then validate + do a tier-budgeted corrective retry. The editable preview
//! in the dashboard is the final human safety net for any extraction noise.
//!
//! Wired by `handlers::cluster` (`persona_generate` / `persona_apply`).

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

/// The persona package — the format-agnostic contract produced from any JD or
/// resume. `persona_apply` writes `identity_md` / `soul_md` into
/// `workspace/cluster/` and the identity fields into `peers.toml [node]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaPackage {
    /// English kebab-case slug (e.g. `frontend-engineer`).
    pub node_name: String,
    /// Human-readable label (Chinese), used as the cluster node name.
    pub display_name: String,
    /// A single emoji representing the role.
    pub emoji: String,
    /// Cluster role — constrained to `worker` / `manager`.
    pub role: String,
    /// Free-text category (e.g. `development`, `data`, `devops`).
    pub category: String,
    /// Skill / domain tags.
    pub tags: Vec<String>,
    /// Full `IDENTITY.md` body — who this node is.
    pub identity_md: String,
    /// Full `SOUL.md` body — how this node behaves.
    pub soul_md: String,
}

/// Sanitize raw pasted text: drop C0 control chars (common PDF-paste garbage),
/// normalize line endings, trim, reject too-short input, cap length.
/// Does NOT interpret format.
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

/// Trim + normalize all fields, then enforce invariants. Mutates in place.
pub fn validate(pkg: &mut PersonaPackage) -> Result<(), String> {
    pkg.node_name = pkg.node_name.trim().to_string();
    pkg.display_name = pkg.display_name.trim().to_string();
    pkg.emoji = pkg.emoji.trim().to_string();
    pkg.role = pkg.role.trim().to_string();
    pkg.category = pkg.category.trim().to_string();
    pkg.identity_md = pkg.identity_md.trim().to_string();
    pkg.soul_md = pkg.soul_md.trim().to_string();
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
    // Cap emoji to a few code points (models sometimes append variation
    // selectors or a stray word).
    let emoji: String = pkg.emoji.chars().take(4).collect();
    pkg.emoji = if emoji.is_empty() {
        "🤖".to_string()
    } else {
        emoji
    };
    Ok(())
}

// ----------------------------------------------------------------------------
// Prompt + tool schema
// ----------------------------------------------------------------------------

fn persona_tool_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "node_name":    { "type": "string", "description": "英文短标识，kebab-case，如 frontend-engineer" },
            "display_name": { "type": "string", "description": "中文显示名，如 前端工程师" },
            "emoji":        { "type": "string", "description": "一个代表该角色的 emoji" },
            "role":         { "type": "string", "enum": ["worker", "manager"], "description": "集群角色，专家节点用 worker" },
            "category":     { "type": "string", "description": "分类，如 development / data / devops / security" },
            "tags":         { "type": "array", "items": { "type": "string" }, "description": "4-8 个技能/领域关键词" },
            "identity_md":  { "type": "string", "description": "完整 IDENTITY.md 内容（Markdown，一级标题开头）。定义这个节点是谁、专长什么。" },
            "soul_md":      { "type": "string", "description": "完整 SOUL.md 内容（Markdown，一级标题开头）。3-8 条可执行的行为规则。" }
        },
        "required": ["node_name", "display_name", "emoji", "role", "category", "tags", "identity_md", "soul_md"]
    })
}

fn tool_def() -> ToolDefinition {
    ToolDefinition {
        tool_type: "function".to_string(),
        function: ToolFunctionDefinition {
            name: "emit_cluster_persona".to_string(),
            description:
                "根据输入的 JD 或简历，生成一份集群节点人格包。必须调用此工具返回结果，不要输出其它内容。"
                    .to_string(),
            parameters: persona_tool_schema(),
        },
    }
}

fn system_prompt(kind: &str) -> String {
    let orientation = if kind == "resume" {
        "用户给你一份简历（任意格式）。把它转化成一个「具备这些技能与经验的集群节点人格」——一个 AI 工人，其能力对应该简历展现的专长。"
    } else {
        "用户给你一份 JD / 岗位描述（任意格式）。把它转化成一个「能胜任该岗位的集群节点人格」——一个 AI 工人，恰好满足这份工作的要求。"
    };
    format!(
"你是一个集群节点人格设计师。{orientation}

输入格式不定（带标题分段、纯段落、表格、PDF 乱序粘贴、中英混杂），你都要抽取并归一化成一份结构化人格包。

铁律：
1. 只外推角色定位与工作风格；绝不编造输入里没有的具体事实（年限、学历、公司名等）。没写的不要脑补。
2. 输出统一中文（display_name / identity_md / soul_md / category）；node_name 用英文 kebab-case；emoji 给一个。
3. role 只能是 worker 或 manager；专家节点一律 worker。
4. identity_md：定义这个节点「是谁」——身份、专长、擅长什么。Markdown，一级标题开头，简洁有力。
5. soul_md：定义「怎么干活」——3 到 8 条务实、可执行的行为规则 / 工作方式 / 产出习惯 / 边界。Markdown，一级标题开头。
6. tags：4-8 个技能/领域关键词。

你必须调用 emit_cluster_persona 工具返回结果，不要输出任何其它文字。"
    )
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

// ----------------------------------------------------------------------------
// Output extraction (robust to provider quirks)
// ----------------------------------------------------------------------------

/// Unwrap the common single-key wrapper some providers add around tool args,
/// e.g. `{"emit_cluster_persona": {…}}` — only when the inner object actually
/// looks like our schema.
fn unwrap_single_key(v: serde_json::Value) -> serde_json::Value {
    if let Some(obj) = v.as_object() {
        if !obj.contains_key("identity_md") && obj.len() == 1 {
            if let Some(inner) = obj.values().next() {
                if inner.is_object() && inner.get("identity_md").is_some() {
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

/// Strip a leading ```json / ``` fence pair if present.
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

/// Pull the persona JSON out of an LLM response: prefer a tool call, fall back
/// to JSON buried in the text content.
fn extract_persona_json(resp: &LLMResponse) -> Result<serde_json::Value, String> {
    // 1. tool call: function.arguments is a JSON string.
    for tc in &resp.tool_calls {
        if let Some(func) = &tc.function {
            let args = func.arguments.trim();
            if !args.is_empty() {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(args) {
                    return Ok(unwrap_single_key(v));
                }
            }
        }
        // 1b. some providers populate tool_call.arguments directly.
        if let Some(map) = &tc.arguments {
            if let Ok(v) = serde_json::to_value(map) {
                return Ok(unwrap_single_key(v));
            }
        }
    }
    // 2. text content: strip a code fence, carve out the JSON object.
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

// ----------------------------------------------------------------------------
// Entry point
// ----------------------------------------------------------------------------

/// Generate a persona package from JD/resume text via one LLM extraction call
/// (with up to `max_attempts` corrective retries). Side-effect free.
pub async fn generate_persona(
    provider: &Arc<HttpProvider>,
    model: &str,
    kind: &str,
    text: &str,
    max_attempts: usize,
) -> Result<PersonaPackage, String> {
    let clean = sanitize_input(text)?;
    let sys = system_prompt(kind);
    let tool = tool_def();
    let opts = ChatOptions {
        temperature: Some(0.2),
        max_tokens: Some(4096),
        top_p: None,
        stop: None,
        extra: std::collections::HashMap::new(),
    };

    // NOTE: we never push the assistant's (possibly malformed) tool-call turn
    // back into history — that avoids the "tool result required after tool_call"
    // API constraint on retry. The correction is just an extra user message.
    let mut messages = vec![mk_msg("system", sys), mk_msg("user", clean.clone())];
    let mut last_err = String::from("（无输出）");

    for attempt in 0..max_attempts {
        let resp = (&**provider)
            .chat(&messages, &[tool.clone()], model, &opts)
            .await
            .map_err(|e| format!("LLM 调用失败: {:?}", e))?;

        let parsed = extract_persona_json(&resp)
            .and_then(|v| serde_json::from_value::<PersonaPackage>(v).map_err(|e| e.to_string()));

        match parsed {
            Ok(mut pkg) => match validate(&mut pkg) {
                Ok(()) => return Ok(pkg),
                Err(e) => last_err = e,
            },
            Err(e) => last_err = e,
        }

        if attempt + 1 < max_attempts {
            messages.push(mk_msg(
                "user",
                format!(
                    "上次的输出有问题：{last_err}。请重新调用 emit_cluster_persona 工具，返回修正后的完整结果，不要输出其它内容。"
                ),
            ));
        }
    }

    Err(format!(
        "生成失败（尝试 {max_attempts} 次仍未通过校验）：{last_err}"
    ))
}

#[cfg(test)]
mod tests;
