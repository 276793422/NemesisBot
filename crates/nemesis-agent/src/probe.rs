//! Capability probe (small-model-tool-robustness plan, Phase 4b).
//!
//! Sends a fixed battery of 7 short tool-use prompts to a model and scores the
//! responses on three axes — format (did it use the `tool_calls` channel?),
//! selection (did it pick the right tool?), and schema (did the args validate?).
//! The aggregate scores map to a [`ModelTier`], giving a direct measurement of
//! tool-calling ability that complements the name/size heuristic.
//!
//! T10（多模态 goal，D9）：第 8 题为**视觉探针**——发一条带 1×1 PNG 的最小
//! 请求，不报错 = 模型接受图像输入，结果独立写入 `vision_probe`（不参与
//! tier 打分）。
//!
//! Scoring is pure and unit-tested; the LLM-call boundary is the async [`run`]
//! function. The probe is invoked only by the user (CLI `model probe` or
//! `--probe`) — never automatically injected into a live conversation.

use nemesis_types::capability::ModelTier;
use serde_json::Value;

use crate::r#loop::{LlmMessage, LlmProvider, LlmResponse};

/// A single probe task: a prompt, the tool we hope the model picks, and that
/// tool's parameter schema (used both to build the tool definition sent to the
/// model and to score the returned arguments).
#[derive(Debug, Clone)]
pub struct ProbeTask {
    pub prompt: &'static str,
    pub expected_tool: &'static str,
    pub schema: Value,
}

/// Per-axis score in `[0.0, 1.0]`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ProbeScore {
    pub format: f64,
    pub selection: f64,
    pub schema: f64,
}

/// Aggregate probe report.
#[derive(Debug, Clone)]
pub struct ProbeReport {
    pub format_score: f64,
    pub selection_score: f64,
    pub schema_score: f64,
    pub tier: ModelTier,
    pub per_task: Vec<(String, ProbeScore)>,
    /// T10（多模态 D9）：第 8 题视觉探针结果。`Some(true)` = 带图请求成功；
    /// `Some(false)` = 模型拒绝带图请求（非传输类错误）；`None` = 未定
    /// （传输类失败——网络/服务问题不说明模型不支持视觉，不给模型钉
    /// "不支持"）。`Some(_)` 由 CLI 写入 config 的 `vision_probe` 键。
    pub vision_probe: Option<bool>,
}

/// The fixed 7-task battery. Tool names match the production tools so the
/// scored behaviour reflects real tool-use ability. The cluster task is included
/// deliberately (cluster is a project highlight) even though small models often
/// struggle with it — that's exactly what selection_score measures.
pub fn probe_tasks() -> Vec<ProbeTask> {
    vec![
        ProbeTask {
            prompt: "现在几点了？请用一个工具获取当前时间。",
            expected_tool: "exec",
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string"}
                },
                "required": ["command"]
            }),
        },
        ProbeTask {
            prompt: "请读取 README.md 这个文件的内容。",
            expected_tool: "read_file",
            schema: serde_json::json!({
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"]
            }),
        },
        ProbeTask {
            prompt: "请创建一个名叫 test 的目录。",
            expected_tool: "create_dir",
            schema: serde_json::json!({
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"]
            }),
        },
        ProbeTask {
            prompt: "请在工作区里搜索字符串 TODO。",
            expected_tool: "grep",
            schema: serde_json::json!({
                "type": "object",
                "properties": {"pattern": {"type": "string"}},
                "required": ["pattern"]
            }),
        },
        ProbeTask {
            prompt: "请把刚刚读到的东西写进 note.md 里。",
            expected_tool: "write_file",
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "content": {"type": "string"}
                },
                "required": ["path", "content"]
            }),
        },
        ProbeTask {
            prompt: "请把 note.md 里的 foo 替换成 bar。",
            expected_tool: "edit_file",
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "old_text": {"type": "string"},
                    "new_text": {"type": "string"}
                },
                "required": ["path", "old_text", "new_text"]
            }),
        },
        ProbeTask {
            prompt: "请通过集群把消息「你好」转发给另一个节点。",
            expected_tool: "cluster_rpc",
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "target_node": {"type": "string"},
                    "message": {"type": "string"}
                },
                "required": ["target_node", "message"]
            }),
        },
    ]
}

/// Build the tool-definition list sent to the model for the battery. Each task
/// contributes its own (expected_tool, schema); duplicates are deduped by name.
pub fn probe_tool_defs() -> Vec<crate::types::ToolDefinition> {
    use std::collections::BTreeMap;
    let mut map: BTreeMap<&'static str, Value> = BTreeMap::new();
    for t in probe_tasks() {
        map.entry(t.expected_tool).or_insert(t.schema);
    }
    map.into_iter()
        .map(|(name, schema)| crate::types::ToolDefinition {
            tool_type: "function".to_string(),
            function: crate::types::ToolFunctionDef {
                name: name.to_string(),
                description: format!("Probe tool: {}", name),
                parameters: schema,
            },
        })
        .collect()
}

/// Score one model response against the task.
pub fn score_response(resp: &LlmResponse, task: &ProbeTask) -> ProbeScore {
    if resp.tool_calls.is_empty() {
        // No tool call emitted (class A format failure).
        return ProbeScore::default();
    }
    let format = 1.0;
    let tc = &resp.tool_calls[0];
    let selection = if tc.name == task.expected_tool {
        1.0
    } else {
        0.0
    };
    let schema = match crate::args_validator::check(&task.schema, &tc.arguments) {
        crate::args_validator::Outcome::Valid => 1.0,
        crate::args_validator::Outcome::Fixed(_) => 0.5,
        crate::args_validator::Outcome::Invalid { .. } => 0.0,
    };
    ProbeScore {
        format,
        selection,
        schema,
    }
}

/// Map aggregate axis scores to a capability tier.
pub fn tier_from_scores(format_score: f64, selection_score: f64, schema_score: f64) -> ModelTier {
    if format_score >= 0.8 && selection_score >= 0.8 && schema_score >= 0.8 {
        ModelTier::Big
    } else if schema_score >= 0.6 && format_score >= 0.5 && selection_score >= 0.5 {
        ModelTier::Normal
    } else {
        ModelTier::Mini
    }
}

/// Run the probe battery against `provider`/`model`. One LLM call per task
/// (7 tool tasks + 1 vision probe).
///
/// **Cost**: 8 short chat completions. The caller MUST be the user (CLI) — never
/// invoke this automatically inside a live conversation.
pub async fn run(provider: &dyn LlmProvider, model: &str) -> Result<ProbeReport, String> {
    let tasks = probe_tasks();
    let tool_defs = probe_tool_defs();
    let opts = crate::types::ChatOptions::default();

    let mut per_task: Vec<(String, ProbeScore)> = Vec::with_capacity(tasks.len());
    let mut fmt_sum = 0.0;
    let mut sel_sum = 0.0;
    let mut sch_sum = 0.0;

    for task in &tasks {
        let messages = vec![
            LlmMessage {
                role: "system".to_string(),
                content: "You are a helpful assistant. When the user asks for an action, \
                          use the appropriate tool. Respond concisely."
                    .to_string(),
                tool_calls: None,
                tool_call_id: None,
                reasoning_content: None,
                images: Vec::new(),
            },
            LlmMessage {
                role: "user".to_string(),
                content: task.prompt.to_string(),
                tool_calls: None,
                tool_call_id: None,
                reasoning_content: None,
                images: Vec::new(),
            },
        ];
        let resp = provider
            .chat(model, messages, Some(opts.clone()), tool_defs.clone())
            .await
            .map_err(|e| format!("LLM chat failed on task '{}': {}", task.expected_tool, e))?;
        let score = score_response(&resp, task);
        fmt_sum += score.format;
        sel_sum += score.selection;
        sch_sum += score.schema;
        per_task.push((task.expected_tool.to_string(), score));
    }

    let n = tasks.len() as f64;
    let format_score = fmt_sum / n;
    let selection_score = sel_sum / n;
    let schema_score = sch_sum / n;
    let tier = tier_from_scores(format_score, selection_score, schema_score);

    // T10（多模态 D9）：第 8 题——视觉探针（在工具电池全部跑通后执行；工具
    // 任务失败会提前返回 Err，探针不跑）。
    let vision_probe = run_vision_probe(provider, model).await;

    Ok(ProbeReport {
        format_score,
        selection_score,
        schema_score,
        tier,
        per_task,
        vision_probe,
    })
}

/// T10（多模态 D9）：视觉探针载荷——1×1 透明 PNG（70 字节，业界通用最小
/// 合法 PNG）的 base64。以 base64 常量内联避免手抄十六进制出错；载荷合法性
/// 有单测钉死（签名 + chunk CRC + IDAT 可解压）。
const VISION_PROBE_PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

/// 传输类错误标记——这类失败不说明模型不支持视觉（网络/服务问题），探针
/// 结果记为未定（`None`），不写 config。
/// F-G（2026-09-04 四轮盲审）：补 FailoverError 的 Display 形态——限流渲染
/// 为 "rate limited by provider p/m"、过载渲染为 "provider p is overloaded"
/// （都不含原始数字码，旧表只匹配 "502"/"503" 数字漏接）→ 瞬态 429/5xx
/// 会被误判 `Some(false)` **永久钉死** vision_probe（探针 > 名字识别的解析
/// 优先级，污染后续所有带图会话）。
const VISION_PROBE_TRANSPORT_MARKERS: &[&str] = &[
    "timeout",
    "timed out",
    "connection",
    "connect",
    "refused",
    "reset",
    "unreachable",
    "dns",
    "temporarily",
    "502",
    "503",
    "504",
    // F-G：FailoverError Display 形态（限流/过载/429 数字码兜底）。
    "rate limited",
    "rate limit",
    "overloaded",
    "429",
];

/// T10（多模态 D9）：第 8 题视觉探针——发一条带 1×1 PNG 的最小 user 请求
/// （无工具），请求不报错 = 模型接受图像输入。传输类失败 → `None`（未定）；
/// 其余错误（典型为 provider 4xx 拒绝图像字段）→ `Some(false)`。
/// **只由 CLI `model probe` 调用**，绝不在对话中自动运行。
async fn run_vision_probe(provider: &dyn LlmProvider, model: &str) -> Option<bool> {
    let messages = vec![LlmMessage {
        role: "user".to_string(),
        content: "请用一个词描述这张图片的颜色。".to_string(),
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
        // data 字段就是 base64 字符串形态（与 hydrate_image_refs 产出一致），
        // 载荷常量直接可用；其 PNG 合法性由单测钉死。
        images: vec![crate::image_attach::LlmImage {
            path: "probe_vision.png".to_string(),
            media_type: "image/png".to_string(),
            data: VISION_PROBE_PNG_B64.to_string(),
        }],
    }];
    match provider
        .chat(
            model,
            messages,
            Some(crate::types::ChatOptions::default()),
            Vec::new(),
        )
        .await
    {
        Ok(_) => Some(true),
        Err(e) => {
            let lower = e.to_lowercase();
            if VISION_PROBE_TRANSPORT_MARKERS
                .iter()
                .any(|m| lower.contains(m))
            {
                tracing::warn!("[Probe] vision probe inconclusive (transport error): {}", e);
                None
            } else {
                Some(false)
            }
        }
    }
}

#[cfg(test)]
mod tests;
