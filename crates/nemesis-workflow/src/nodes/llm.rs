// ---------------------------------------------------------------------------
// 2026-11-05 机械拆分：本文件内容由 nodes.rs 原样搬入（纯代码搬家，行为零变更）。
// ---------------------------------------------------------------------------

use super::*;

/// Built-in LLM node executor (mock).
///
/// Returns a canned response built from the `prompt`/`model` config fields
/// without calling any real provider. Useful for unit tests that need to
/// exercise the scheduler without an LLM backend. Production deployments
/// should register [`RealLLMNodeExecutor`] under the `llm` node type to
/// override this mock.
pub struct LLMNodeExecutor;

#[async_trait]
impl NodeExecutor for LLMNodeExecutor {
    async fn execute(
        &self,
        node: &NodeDef,
        _context: &HashMap<String, serde_json::Value>,
        _wf_ctx: &WorkflowContext,
    ) -> Result<NodeResult, String> {
        let now = Local::now();
        let prompt = node
            .config
            .get("prompt")
            .and_then(|v| v.as_str())
            .unwrap_or("default prompt");
        let model = node
            .config
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("default");
        Ok(NodeResult {
            node_id: node.id.clone(),
            output: serde_json::json!({
                "text": format!("LLM execution (model={}): {}", model, prompt),
            }),
            error: None,
            state: ExecutionState::Completed,
            started_at: now,
            ended_at: Local::now(),
            metadata: HashMap::new(),
        })
    }
}

/// Production-grade LLM node executor backed by [`nemesis_providers`].
///
/// Holds an `Arc<dyn LLMProvider>` (e.g., a `Router` or any concrete
/// provider) and calls `provider.chat()` on each execution. Node config
/// fields:
///
/// - `prompt` (required, string): User message content.
/// - `system_prompt` (optional, string): Prepended as a system message.
/// - `model` (optional, string): Defaults to `provider.default_model()`.
/// - `temperature` (optional, float): Maps to `ChatOptions.temperature`.
/// - `max_tokens` (optional, int): Maps to `ChatOptions.max_tokens`.
///
/// The node output is a JSON object:
/// ```json
/// { "text": "<content>", "model": "<model>",
///   "finish_reason": "<reason>", "usage": { ... } }
/// ```
///
/// On provider errors the executor returns a `NodeResult` with
/// `state = Failed` (rather than `Err`) so the workflow can observe the
/// failure via standard state inspection.
pub struct RealLLMNodeExecutor {
    provider: Arc<dyn LLMProvider>,
    usage_store: UsageStoreSlot,
}

impl RealLLMNodeExecutor {
    /// Construct a new executor that delegates to the given provider.
    /// No usage tracking is wired — use [`with_usage_store`] when the
    /// executor is owned by an engine that has a `DataStore`.
    pub fn new(provider: Arc<dyn LLMProvider>) -> Self {
        Self {
            provider,
            usage_store: new_usage_store_slot(),
        }
    }

    /// Internal constructor that shares the engine's usage slot so LLM calls
    /// are recorded whenever the gateway populates the slot via
    /// [`crate::engine::WorkflowEngine::set_usage_store`].
    pub(crate) fn with_usage_store(
        provider: Arc<dyn LLMProvider>,
        usage_store: UsageStoreSlot,
    ) -> Self {
        Self {
            provider,
            usage_store,
        }
    }

    /// Borrow the inner provider (used by tests + observers).
    pub fn provider(&self) -> &Arc<dyn LLMProvider> {
        &self.provider
    }
}

#[async_trait]
impl NodeExecutor for RealLLMNodeExecutor {
    async fn execute(
        &self,
        node: &NodeDef,
        context: &HashMap<String, serde_json::Value>,
        _wf_ctx: &WorkflowContext,
    ) -> Result<NodeResult, String> {
        let started = Local::now();

        // ---- Pull config ----
        let prompt = match node.config.get("prompt").and_then(|v| v.as_str()) {
            Some(p) => p.to_string(),
            None => {
                return Ok(failed_node_result(
                    &node.id,
                    started,
                    "llm node missing required 'prompt' config",
                ));
            }
        };

        // Prompt may reference context variables via {{var}} placeholders,
        // or carry a whole-value credential reference (env:/yaml:/vault:).
        let prompt = match resolve_runtime_string(&prompt, context, "llm.prompt") {
            Ok(p) => p,
            Err(e) => return Ok(failed_node_result(&node.id, started, &e)),
        };

        let model = node
            .config
            .get("model")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| self.provider.default_model().to_string());

        let temperature = node.config.get("temperature").and_then(|v| v.as_f64());
        let max_tokens = node.config.get("max_tokens").and_then(|v| v.as_i64());

        let system_prompt = match node.config.get("system_prompt").and_then(|v| v.as_str()) {
            Some(s) => match resolve_runtime_string(s, context, "llm.system_prompt") {
                Ok(p) => Some(p),
                Err(e) => return Ok(failed_node_result(&node.id, started, &e)),
            },
            None => None,
        };

        // ---- Build chat request ----
        let mut messages: Vec<Message> = Vec::new();
        if let Some(sys) = system_prompt {
            messages.push(Message {
                role: "system".to_string(),
                content: sys.into(),
                tool_calls: Vec::new(),
                tool_call_id: None,
                timestamp: None,
                reasoning_content: None,
                extra: HashMap::new(),
            });
        }
        messages.push(Message {
            role: "user".to_string(),
            content: prompt.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: None,
            reasoning_content: None,
            extra: HashMap::new(),
        });

        let options = ChatOptions {
            temperature,
            max_tokens,
            top_p: None,
            stop: None,
            extra: HashMap::new(),
            reasoning_effort: None, // H4: workflow nodes don't set a tier
        };

        // ---- Invoke provider ----
        match self.provider.chat(&messages, &[], &model, &options).await {
            Ok(resp) => {
                if let Some(ref u) = resp.usage {
                    record_llm_usage(&self.usage_store, &node.id, &model, u, started);
                }
                Ok(llm_node_result(&node.id, started, &model, max_tokens, resp))
            }
            Err(err) => Ok(failed_node_result(
                &node.id,
                started,
                &format!("LLM provider error: {}", err),
            )),
        }
    }
}

/// Build a NodeResult from a successful LLMResponse, honoring the honest
/// failure contract (2026-09-23 计划 A1).
///
/// 空文本 = 节点 Failed（节点唯一产物就是文本）：`finish_reason=length`
/// 时错误点名 max_tokens 预算被思维链耗尽（推理模型典型病，glm-5.3-flash
/// 实测复现），output 保留 model/finish_reason/usage/思维链预览供诊断；
/// 其他 finish_reason 报「返回空文本」。有文本的 length 截断是可用产出，
/// 维持 Completed。
fn llm_node_result(
    node_id: &str,
    started: chrono::DateTime<Local>,
    model: &str,
    max_tokens: Option<i64>,
    resp: LLMResponse,
) -> NodeResult {
    let usage_json = resp.usage.as_ref().map(|u| {
        serde_json::json!({
            "prompt_tokens": u.prompt_tokens,
            "completion_tokens": u.completion_tokens,
            "total_tokens": u.total_tokens,
            "cached_tokens": u.cached_tokens,
            "cache_creation_tokens": u.cache_creation_tokens,
            "cache_read_tokens": u.cache_read_tokens,
        })
    });

    if resp.content.trim().is_empty() {
        let error = if resp.finish_reason == "length" {
            let budget = max_tokens
                .map(|n| n.to_string())
                .unwrap_or_else(|| "provider 默认".to_string());
            format!(
                "LLM 输出预算耗尽：max_tokens={budget} 被思维链全部消耗且无文本输出——\
                 提高该节点 max_tokens（推理模型建议 ≥2000）或换非推理模型"
            )
        } else {
            format!("LLM 返回空文本 (finish_reason={})", resp.finish_reason)
        };
        let reasoning_preview = resp
            .reasoning_content
            .as_deref()
            .map(|s| s.chars().take(200).collect::<String>());
        return NodeResult {
            node_id: node_id.to_string(),
            output: serde_json::json!({
                "model": model,
                "finish_reason": resp.finish_reason,
                "usage": usage_json,
                "reasoning_preview": reasoning_preview,
            }),
            error: Some(error),
            state: ExecutionState::Failed,
            started_at: started,
            ended_at: Local::now(),
            metadata: HashMap::new(),
        };
    }

    NodeResult {
        node_id: node_id.to_string(),
        output: serde_json::json!({
            "text": resp.content,
            "model": model,
            "finish_reason": resp.finish_reason,
            "usage": usage_json,
        }),
        error: None,
        state: ExecutionState::Completed,
        started_at: started,
        ended_at: Local::now(),
        metadata: HashMap::new(),
    }
}

/// Build a Failed NodeResult with the given error message.
pub(super) fn failed_node_result(
    node_id: &str,
    started: chrono::DateTime<Local>,
    error: &str,
) -> NodeResult {
    NodeResult {
        node_id: node_id.to_string(),
        output: serde_json::Value::Null,
        error: Some(error.to_string()),
        state: ExecutionState::Failed,
        started_at: started,
        ended_at: Local::now(),
        metadata: HashMap::new(),
    }
}

/// HTTP 状态码的标准原因短语（http 节点错误消息用）；未知码返回空串。
pub(super) fn resp_status_reason(status_code: u16) -> String {
    reqwest::StatusCode::from_u16(status_code)
        .ok()
        .and_then(|s| s.canonical_reason().map(|r| format!(" {r}")))
        .unwrap_or_default()
}

/// Resolve `{{var}}` placeholders against the executor context.
///
/// Supports nested lookups: `{{node_id.field}}` resolves to the field of a
/// previously-executed node's output object. Missing keys resolve to empty
/// string. The implementation is intentionally minimal — full templating
/// belongs in the scheduler's context-builder, not here.
/// 替换 `{{key}}` 占位符（键存在才替换；缺失键保留原样——调度器的条件边
/// 语义靠这一点把「引用了不存在的字段」识别为不可解析，见
/// `scheduler::eval_edge_conditions`）。
pub(crate) fn resolve_prompt_template(
    template: &str,
    context: &HashMap<String, serde_json::Value>,
) -> String {
    let mut out = template.to_string();
    for (k, v) in context {
        let placeholder = format!("{{{{{}}}}}", k);
        let replacement = match v {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        out = out.replace(&placeholder, &replacement);
    }
    out
}

/// 提取表达式里的全部 `{{…}}` 占位符，拆成（首段, 可选字段名）。
/// 条件边 lint（字段幻觉预警）与调度器（未解析占位符可证伪性判定）共用。
pub(crate) fn placeholder_refs(cond: &str) -> Vec<(String, Option<String>)> {
    let mut out = Vec::new();
    let mut rest = cond;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            break;
        };
        let inner = after[..end].trim();
        if !inner.is_empty() {
            let mut it = inner.splitn(2, '.');
            let head = it.next().unwrap_or("").trim().to_string();
            let field = it.next().map(|f| f.trim().to_string());
            out.push((head, field));
        }
        rest = &after[end + 2..];
    }
    out
}

/// Resolve a node-config string: credential reference first, then `{{var}}`
/// template.
///
/// 诚实失败契约的模板层一半（2026-09-23 计划类 B）：
///
/// 1. **凭据引用（整值）**：配置**原文**以 `env:` / `yaml:` / `vault:` 开头
///    → 交给 [`nemesis_config::resolve_secret_field`] 解析（每次现查）。
///    解析失败返回 Err——调用方必须让节点 Failed 并保留错误上下文，绝不
///    静默降级为空值。只识别原文：`{{var}}` 替换产生的值不会被二次解释成
///    凭据（上游数据以 `env:` 开头不会误触发解析）。
/// 2. **`{{var}}` 模板**：对 executor context 占位符替换，缺失键解析为
///    空串（文档化行为）；此路径永不失败。
///
/// `field_ctx` 仅进错误消息（如 `http.headers.Authorization`）。
pub(super) fn resolve_runtime_string(
    raw: &str,
    context: &HashMap<String, serde_json::Value>,
    field_ctx: &str,
) -> Result<String, String> {
    if nemesis_config::is_secret_reference(raw) {
        return nemesis_config::resolve_secret_field(raw, field_ctx).map_err(|e| e.to_string());
    }
    Ok(resolve_prompt_template(raw, context))
}

// ---------------------------------------------------------------------------
// Tool Node
// ---------------------------------------------------------------------------
/// LLM-based classifier node (milestone 1b-D3).
///
/// Sends a structured prompt that asks the LLM to pick exactly one class from
/// a configured list. Output is `{class_id, confidence}` so downstream
/// conditional edges can branch on `class_id`. On parse failure the executor
/// retries up to `max_attempts` times (default 3) before failing the node.
///
/// Node config:
/// - `question` (required, string): the text to classify. Supports `{{var}}`
///   template resolution.
/// - `classes` (required, array): list of `{id, description}` objects.
/// - `system_prompt` (optional, string): defaults to a strict template that
///   tells the LLM to output only the class id.
/// - `model` (optional, string): defaults to provider's default.
/// - `max_attempts` (optional, int, default 3): how many times to retry on
///   parse failure / invalid class id.
/// - `temperature` (optional, float): usually 0 for deterministic output.
pub struct QuestionClassifierNodeExecutor {
    provider: Arc<dyn LLMProvider>,
    usage_store: UsageStoreSlot,
}

impl QuestionClassifierNodeExecutor {
    pub fn new(provider: Arc<dyn LLMProvider>) -> Self {
        Self {
            provider,
            usage_store: new_usage_store_slot(),
        }
    }

    /// Internal constructor that shares the engine's usage slot.
    pub(crate) fn with_usage_store(
        provider: Arc<dyn LLMProvider>,
        usage_store: UsageStoreSlot,
    ) -> Self {
        Self {
            provider,
            usage_store,
        }
    }

    /// Borrow the inner provider (used by tests).
    pub fn provider(&self) -> &Arc<dyn LLMProvider> {
        &self.provider
    }
}

/// One class entry: `id` (machine-readable) + `description` (LLM-facing).
#[derive(Debug, Clone, serde::Deserialize)]
struct ClassDef {
    id: String,
    description: String,
}

/// Default system prompt: forces the LLM to output only the class id and
/// nothing else. We wrap the list of classes inline so the model can't
/// hallucinate ids outside the configured set.
/// 分类器 system prompt：从给定类 id 集合中挑且只挑一个。类清单经
/// `{classes}` 占位内联注入，模型无法幻觉出集合外的 id。
/// 文本单一真相源在 `nemesis-prompts`（M7 集中化）。
use nemesis_prompts::workflow::CLASSIFIER_SYSTEM_PROMPT;

#[async_trait]
impl NodeExecutor for QuestionClassifierNodeExecutor {
    async fn execute(
        &self,
        node: &NodeDef,
        context: &HashMap<String, serde_json::Value>,
        _wf_ctx: &WorkflowContext,
    ) -> Result<NodeResult, String> {
        let started = Local::now();

        // ---- Parse config ----
        let question_raw = match node.config.get("question").and_then(|v| v.as_str()) {
            Some(q) => q.to_string(),
            None => {
                return Ok(failed_node_result(
                    &node.id,
                    started,
                    "question_classifier node missing required 'question' config",
                ));
            }
        };
        let question = match resolve_runtime_string(&question_raw, context, "classifier.question") {
            Ok(v) => v,
            Err(e) => return Ok(failed_node_result(&node.id, started, &e)),
        };

        let classes_value = match node.config.get("classes") {
            Some(v) => v,
            None => {
                return Ok(failed_node_result(
                    &node.id,
                    started,
                    "question_classifier node missing required 'classes' config",
                ));
            }
        };
        let classes: Vec<ClassDef> = match serde_json::from_value(classes_value.clone()) {
            Ok(c) => c,
            Err(e) => {
                return Ok(failed_node_result(
                    &node.id,
                    started,
                    &format!("invalid 'classes' config: {}", e),
                ));
            }
        };
        if classes.is_empty() {
            return Ok(failed_node_result(
                &node.id,
                started,
                "question_classifier node has empty 'classes' list",
            ));
        }

        let max_attempts = node
            .config
            .get("max_attempts")
            .and_then(|v| v.as_u64())
            .unwrap_or(3)
            .max(1) as usize;

        let model = node
            .config
            .get("model")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| self.provider.default_model().to_string());

        let temperature = node
            .config
            .get("temperature")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        let system_prompt = match node.config.get("system_prompt").and_then(|v| v.as_str()) {
            Some(s) => match resolve_runtime_string(s, context, "classifier.system_prompt") {
                Ok(p) => Some(p),
                Err(e) => return Ok(failed_node_result(&node.id, started, &e)),
            },
            None => None,
        }
        .unwrap_or_else(|| {
            let classes_block: String = classes
                .iter()
                .map(|c| format!("- {}: {}", c.id, c.description))
                .collect::<Vec<_>>()
                .join("\n");
            CLASSIFIER_SYSTEM_PROMPT.replace("{classes}", &classes_block)
        });

        // ---- Retry loop ----
        let mut last_error: Option<String> = None;
        for attempt in 1..=max_attempts {
            let messages = vec![
                Message {
                    role: "system".to_string(),
                    content: system_prompt.clone().into(),
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                    timestamp: None,
                    reasoning_content: None,
                    extra: HashMap::new(),
                },
                Message {
                    role: "user".to_string(),
                    content: question.clone().into(),
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                    timestamp: None,
                    reasoning_content: None,
                    extra: HashMap::new(),
                },
            ];
            let options = ChatOptions {
                temperature: Some(temperature),
                max_tokens: None,
                top_p: None,
                stop: None,
                extra: HashMap::new(),
                reasoning_effort: None, // H4: workflow nodes don't set a tier
            };

            // Capture per-call start so each retry's RequestLog has its own
            // latency + trace_id. Using the node-level `started` would make
            // every attempt share the same trace_id and report cumulative
            // latency drift on later retries.
            let call_started = Local::now();
            match self.provider.chat(&messages, &[], &model, &options).await {
                Ok(resp) => {
                    if let Some(ref u) = resp.usage {
                        record_llm_usage(&self.usage_store, &node.id, &model, u, call_started);
                    }
                    let content = resp.content;
                    let class_id = parse_classifier_output(&content);
                    if let Some(ref id) = class_id
                        && classes.iter().any(|c| &c.id == id)
                    {
                        return Ok(NodeResult {
                            node_id: node.id.clone(),
                            output: serde_json::json!({
                                "class_id": id,
                                "confidence": confidence_for(attempt, max_attempts),
                                "raw_response": content,
                                "model": model,
                                "attempts": attempt,
                            }),
                            error: None,
                            state: ExecutionState::Completed,
                            started_at: started,
                            ended_at: Local::now(),
                            metadata: HashMap::new(),
                        });
                    }
                    last_error = Some(format!(
                        "attempt {}: LLM returned invalid class id {:?} (raw: {:?})",
                        attempt,
                        class_id,
                        content.trim()
                    ));
                }
                Err(e) => {
                    last_error = Some(format!("attempt {}: provider error: {}", attempt, e));
                }
            }
        }

        Ok(failed_node_result(
            &node.id,
            started,
            &format!(
                "question_classifier failed after {} attempts: {}",
                max_attempts,
                last_error.unwrap_or_else(|| "unknown".to_string())
            ),
        ))
    }
}

/// Extract the class id from an LLM response.
///
/// The model is told to output only the id, but real-world responses often
/// include surrounding prose ("The class is: foo") or punctuation. We strip
/// common wrappers and pick the first token that matches a known id.
///
/// Note: validation against the configured class list happens in the caller;
/// this function returns whatever looks like an id so the caller can decide.
pub(super) fn parse_classifier_output(content: &str) -> Option<String> {
    let trimmed = content.trim().trim_matches(|c: char| {
        c == '"' || c == '\'' || c == '.' || c == ',' || c == '!' || c == '\n'
    });
    if trimmed.is_empty() {
        return None;
    }
    // If the entire response is a single bare token, return it.
    if !trimmed.contains(char::is_whitespace) {
        return Some(trimmed.to_string());
    }
    // Otherwise, take the first whitespace-delimited token. This handles
    // cases like "foo\n(because the input is clearly about food)".
    Some(trimmed.split_whitespace().next()?.to_string())
}

/// Heuristic confidence: 1.0 on first attempt, decreasing for retries.
fn confidence_for(attempt: usize, _max_attempts: usize) -> f64 {
    // First attempt = high confidence. Each retry shaves 0.15.
    let conf = 1.0 - (attempt.saturating_sub(1) as f64) * 0.15;
    conf.max(0.1)
}

// ---------------------------------------------------------------------------
// Parameter Extractor Node (1b-D4)
// ---------------------------------------------------------------------------
/// LLM-based parameter extractor (milestone 1b-D4).
///
/// Asks the LLM to read a chunk of free-form text and pull out structured
/// fields according to a declared schema. The output is a JSON object with
/// one key per declared parameter. Missing or unextractable parameters are
/// set to null rather than omitted, so downstream nodes can rely on the
/// shape being stable.
///
/// Node config:
/// - `text` (required, string): the source text to extract from. Supports
///   `{{var}}` template resolution.
/// - `parameters` (required, array): list of `{name, type, description,
///   required}` objects. `type` is informational only (string/number/...),
///   we don't enforce it on the LLM output beyond making sure required
///   fields are present and non-null.
/// - `system_prompt` (optional, string): defaults to a strict template.
/// - `model` (optional, string): defaults to provider's default.
/// - `max_attempts` (optional, int, default 3): how many times to retry on
///   JSON parse failure / missing required field.
/// - `temperature` (optional, float): usually 0 for deterministic output.
pub struct ParameterExtractorNodeExecutor {
    provider: Arc<dyn LLMProvider>,
    usage_store: UsageStoreSlot,
}

impl ParameterExtractorNodeExecutor {
    pub fn new(provider: Arc<dyn LLMProvider>) -> Self {
        Self {
            provider,
            usage_store: new_usage_store_slot(),
        }
    }

    /// Internal constructor that shares the engine's usage slot.
    pub(crate) fn with_usage_store(
        provider: Arc<dyn LLMProvider>,
        usage_store: UsageStoreSlot,
    ) -> Self {
        Self {
            provider,
            usage_store,
        }
    }

    /// Borrow the inner provider (used by tests).
    pub fn provider(&self) -> &Arc<dyn LLMProvider> {
        &self.provider
    }
}

/// One declared parameter. `type` is a free-form hint (e.g. "string",
/// "number", "boolean"); we don't coerce the LLM output, only check that
/// required params are present and non-null.
#[derive(Debug, Clone, serde::Deserialize)]
pub(super) struct ParamDef {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    #[allow(dead_code)]
    r#type: String,
    #[serde(default)]
    required: bool,
}

/// Default system prompt. We embed the parameter schema inline so the
/// model has a clear contract, and we ask for a single JSON object (no
/// markdown fences, no commentary) to keep parsing trivial.
/// 抽取器 system prompt：按字段清单抽信息、输出单一 JSON 对象。
/// 字段清单经 `{parameters}` 占位注入。文本单一真相源在
/// `nemesis-prompts`（M7 集中化）。
use nemesis_prompts::workflow::EXTRACTOR_SYSTEM_PROMPT;

#[async_trait]
impl NodeExecutor for ParameterExtractorNodeExecutor {
    async fn execute(
        &self,
        node: &NodeDef,
        context: &HashMap<String, serde_json::Value>,
        _wf_ctx: &WorkflowContext,
    ) -> Result<NodeResult, String> {
        let started = Local::now();

        // ---- Parse config ----
        let text_raw = match node.config.get("text").and_then(|v| v.as_str()) {
            Some(t) => t.to_string(),
            None => {
                return Ok(failed_node_result(
                    &node.id,
                    started,
                    "parameter_extractor node missing required 'text' config",
                ));
            }
        };
        let text = match resolve_runtime_string(&text_raw, context, "parameter_extractor.text") {
            Ok(v) => v,
            Err(e) => return Ok(failed_node_result(&node.id, started, &e)),
        };

        let params_value = match node.config.get("parameters") {
            Some(v) => v,
            None => {
                return Ok(failed_node_result(
                    &node.id,
                    started,
                    "parameter_extractor node missing required 'parameters' config",
                ));
            }
        };
        let params: Vec<ParamDef> = match serde_json::from_value(params_value.clone()) {
            Ok(p) => p,
            Err(e) => {
                return Ok(failed_node_result(
                    &node.id,
                    started,
                    &format!("invalid 'parameters' config: {}", e),
                ));
            }
        };
        if params.is_empty() {
            return Ok(failed_node_result(
                &node.id,
                started,
                "parameter_extractor node has empty 'parameters' list",
            ));
        }

        let max_attempts = node
            .config
            .get("max_attempts")
            .and_then(|v| v.as_u64())
            .unwrap_or(3)
            .max(1) as usize;

        let model = node
            .config
            .get("model")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| self.provider.default_model().to_string());

        let temperature = node
            .config
            .get("temperature")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        let system_prompt = match node.config.get("system_prompt").and_then(|v| v.as_str()) {
            Some(s) => {
                match resolve_runtime_string(s, context, "parameter_extractor.system_prompt") {
                    Ok(p) => Some(p),
                    Err(e) => return Ok(failed_node_result(&node.id, started, &e)),
                }
            }
            None => None,
        }
        .unwrap_or_else(|| {
            let params_block: String = params
                .iter()
                .map(|p| {
                    let req = if p.required { " (required)" } else { "" };
                    format!("- {} [{}]{}: {}", p.name, p.r#type, req, p.description)
                })
                .collect::<Vec<_>>()
                .join("\n");
            EXTRACTOR_SYSTEM_PROMPT.replace("{parameters}", &params_block)
        });

        // ---- Retry loop ----
        let mut last_error: Option<String> = None;
        for attempt in 1..=max_attempts {
            let messages = vec![
                Message {
                    role: "system".to_string(),
                    content: system_prompt.clone().into(),
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                    timestamp: None,
                    reasoning_content: None,
                    extra: HashMap::new(),
                },
                Message {
                    role: "user".to_string(),
                    content: text.clone().into(),
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                    timestamp: None,
                    reasoning_content: None,
                    extra: HashMap::new(),
                },
            ];
            let options = ChatOptions {
                temperature: Some(temperature),
                max_tokens: None,
                top_p: None,
                stop: None,
                extra: HashMap::new(),
                reasoning_effort: None, // H4: workflow nodes don't set a tier
            };

            // Capture per-call start so each retry's RequestLog has its own
            // latency + trace_id (same rationale as the classifier executor).
            let call_started = Local::now();
            match self.provider.chat(&messages, &[], &model, &options).await {
                Ok(resp) => {
                    if let Some(ref u) = resp.usage {
                        record_llm_usage(&self.usage_store, &node.id, &model, u, call_started);
                    }
                    let content = resp.content;
                    match parse_json_object(&content) {
                        Ok(obj) => {
                            let normalized = normalize_object(&obj, &params);
                            if let Err(missing) = validate_required_params(&normalized, &params) {
                                last_error = Some(format!(
                                    "attempt {}: missing required parameters: {}",
                                    attempt, missing
                                ));
                                continue;
                            }
                            return Ok(NodeResult {
                                node_id: node.id.clone(),
                                output: serde_json::json!({
                                    "parameters": normalized,
                                    "raw_response": content,
                                    "model": model,
                                    "attempts": attempt,
                                    "confidence": confidence_for(attempt, max_attempts),
                                }),
                                error: None,
                                state: ExecutionState::Completed,
                                started_at: started,
                                ended_at: Local::now(),
                                metadata: HashMap::new(),
                            });
                        }
                        Err(e) => {
                            last_error = Some(format!(
                                "attempt {}: JSON parse error: {} (raw: {:?})",
                                attempt,
                                e,
                                content.trim()
                            ));
                        }
                    }
                }
                Err(e) => {
                    last_error = Some(format!("attempt {}: provider error: {}", attempt, e));
                }
            }
        }

        Ok(failed_node_result(
            &node.id,
            started,
            &format!(
                "parameter_extractor failed after {} attempts: {}",
                max_attempts,
                last_error.unwrap_or_else(|| "unknown".to_string())
            ),
        ))
    }
}

/// Parse the LLM response into a JSON object.
///
/// The model is told to emit only JSON, but we tolerate a few common
/// wrappers: leading/trailing whitespace, an optional ```json fence
/// (single or triple backticks), and surrounding prose that still leaves
/// the JSON parseable once we extract the outermost `{ ... }` block.
pub(super) fn parse_json_object(content: &str) -> Result<serde_json::Value, String> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Err("empty response".to_string());
    }

    // Fast path: the whole response is already a JSON object.
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if v.is_object() {
            return Ok(v);
        }
        return Err(format!("expected JSON object, got {}", type_name(&v)));
    }

    // Strip ```json ... ``` fences.
    let fence_stripped = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .map(|s| s.trim_start_matches('\n').trim())
        .and_then(|s| s.strip_suffix("```").map(|s| s.trim()))
        .unwrap_or(trimmed);
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(fence_stripped)
        && v.is_object()
    {
        return Ok(v);
    }

    // Last resort: pull out the outermost {...} region. Handles
    // "Sure, here's the JSON:\n{ \"name\": \"foo\" }\nHope this helps!".
    if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}'))
        && start < end
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(&trimmed[start..=end])
        && v.is_object()
    {
        return Ok(v);
    }

    Err("not a JSON object".to_string())
}

/// Ensure every declared parameter appears in the output object. Missing
/// keys are filled in with null so downstream consumers see a stable shape.
pub(super) fn normalize_object(
    parsed: &serde_json::Value,
    params: &[ParamDef],
) -> serde_json::Value {
    let mut obj = match parsed.as_object() {
        Some(o) => o.clone(),
        None => {
            let mut m = serde_json::Map::new();
            m.insert("_value".to_string(), parsed.clone());
            m
        }
    };
    for p in params {
        if !obj.contains_key(&p.name) {
            obj.insert(p.name.clone(), serde_json::Value::Null);
        }
    }
    serde_json::Value::Object(obj)
}

/// Return Err(message) if any `required: true` parameter is null or missing.
pub(super) fn validate_required_params(
    normalized: &serde_json::Value,
    params: &[ParamDef],
) -> Result<(), String> {
    let obj = match normalized.as_object() {
        Some(o) => o,
        None => return Err("output is not an object".to_string()),
    };
    let missing: Vec<&str> = params
        .iter()
        .filter(|p| p.required)
        .filter(|p| obj.get(&p.name).is_none_or(|v| v.is_null()))
        .map(|p| p.name.as_str())
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(missing.join(", "))
    }
}

pub(super) fn type_name(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

// ---------------------------------------------------------------------------
// Agent Node (1b-D2)
// ---------------------------------------------------------------------------
/// Result returned by an [`AgentRunner`] after a one-shot direct run.
///
/// `response` is the assistant's final message. `tools_used` lists tool names
/// invoked during the run (in invocation order; duplicates preserved so callers
/// can count how many times each tool fired).
#[derive(Debug, Clone, Default)]
pub struct AgentRunResult {
    /// The assistant's final reply text.
    pub response: String,
    /// Tool names invoked, in order (duplicates allowed).
    pub tools_used: Vec<String>,
}

/// Abstraction over a one-shot direct agent invocation.
///
/// Implementations live outside `nemesis-workflow` (typically in `nemesisbot`)
/// to avoid pulling `nemesis-agent` — and its large dependency closure — into
/// the workflow crate. The workflow engine only needs the ability to kick off
/// an agent run by prompt + identity; it doesn't care how the agent produces
/// the reply.
///
/// The trait is async + `Send + Sync` so it can be wrapped in `Arc<dyn …>`
/// and shared across the scheduler's tokio tasks.
#[async_trait]
pub trait AgentRunner: Send + Sync {
    /// Run a one-shot direct prompt through the underlying agent loop.
    ///
    /// - `prompt`: fully-resolved user-facing prompt.
    /// - `agent_id`: stable identifier for the agent instance / session.
    ///   Multiple invocations with the same id SHOULD reuse the same
    ///   conversation memory (so the agent can recall prior turns inside
    ///   this workflow run).
    /// - `max_turns`: safety cap on LLM ↔ tool iteration. The runner SHOULD
    ///   refuse to run more than this many rounds to prevent token blowups.
    /// - `model`: optional model override (e.g. `zhipu/glm-4.7`). Runners
    ///   that don't support per-call model switching SHOULD log a warning
    ///   and fall back to their default model rather than failing.
    async fn run_direct(
        &self,
        prompt: &str,
        agent_id: &str,
        max_turns: u32,
        model: Option<&str>,
    ) -> Result<AgentRunResult, String>;
}

/// Workflow node that delegates to an [`AgentRunner`] (milestone 1b-D2).
///
/// The node kicks off a one-shot agent run with the resolved prompt and
/// surfaces the final response. Internal agent iterations (LLM ↔ tool
/// cycles) are NOT checkpointed — only the node start / end are visible
/// to the workflow engine, so a crash mid-iteration restarts the whole
/// agent node. That's intentional: agent memory is hard to snapshot
/// cleanly, and rerunning is cheaper than getting it wrong.
///
/// Node config:
/// - `prompt` (required, string): the user-facing prompt. Supports
///   `{{var}}` template resolution.
/// - `agent_id` (optional, string, default `"workflow_agent"`): stable id
///   used as session key, so multiple agent nodes in the same workflow
///   can either share or isolate memory by setting this differently.
/// - `max_turns` (optional, int, default 5): safety cap on agent
///   iterations.
pub struct AgentNodeExecutor {
    runner: Arc<dyn AgentRunner>,
}

impl AgentNodeExecutor {
    pub fn new(runner: Arc<dyn AgentRunner>) -> Self {
        Self { runner }
    }
}

#[async_trait]
impl NodeExecutor for AgentNodeExecutor {
    async fn execute(
        &self,
        node: &NodeDef,
        context: &HashMap<String, serde_json::Value>,
        _wf_ctx: &WorkflowContext,
    ) -> Result<NodeResult, String> {
        let started = Local::now();

        let prompt_raw = match node.config.get("prompt").and_then(|v| v.as_str()) {
            Some(p) => p.to_string(),
            None => {
                return Ok(failed_node_result(
                    &node.id,
                    started,
                    "agent node missing required 'prompt' config",
                ));
            }
        };
        let prompt = match resolve_runtime_string(&prompt_raw, context, "agent.prompt") {
            Ok(v) => v,
            Err(e) => return Ok(failed_node_result(&node.id, started, &e)),
        };

        let agent_id = node
            .config
            .get("agent_id")
            .and_then(|v| v.as_str())
            .unwrap_or("workflow_agent")
            .to_string();

        let max_turns = node
            .config
            .get("max_turns")
            .and_then(|v| v.as_u64())
            .unwrap_or(5) as u32;

        // Optional model override (form exposes a "模型" field). The runner
        // is responsible for honoring it; GatewayAgentRunner currently logs
        // and falls back to the default model — same pattern as max_turns.
        let model = node
            .config
            .get("model")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());

        // 角色绑定（2026-09-28 角色目录与分档供给）：`role` = 子代理角色
        // slug，模板（四组件：硬边界/输出契约/内容边界）前置进 prompt——
        // run_direct 无独立 system prompt 通道，前置等价生效（模板在前，
        // 节点指令在后）。未知 slug = 诚实节点失败并列合法清单（不静默
        // 裸跑）。空/缺省 = 无模板（旧行为）。
        let prompt = match node
            .config
            .get("role")
            .and_then(|v| v.as_str())
            .map(str::trim)
        {
            Some(r) if !r.is_empty() => {
                let role = match nemesis_prompts::subagents::SubagentRole::from_slug(r) {
                    Some(role) => role,
                    None => {
                        let valid = nemesis_prompts::subagents::SubagentRole::catalog()
                            .iter()
                            .map(|(slug, _, _)| *slug)
                            .collect::<Vec<_>>()
                            .join(", ");
                        return Ok(failed_node_result(
                            &node.id,
                            started,
                            &format!("未知角色 '{r}'（agent 节点 role 字段）。合法角色: {valid}"),
                        ));
                    }
                };
                format!("{}\n\n---\n\n{}", role.template(), prompt)
            }
            _ => prompt,
        };

        match self
            .runner
            .run_direct(&prompt, &agent_id, max_turns, model)
            .await
        {
            Ok(result) => {
                // 诚实失败契约（2026-09-23 计划 A3）：空响应 = 无可用产出，
                // 节点 Failed（max_turns 耗尽或模型无输出）。
                if result.response.trim().is_empty() {
                    return Ok(failed_node_result(
                        &node.id,
                        started,
                        &format!(
                            "agent 返回空响应 (agent_id={agent_id}, max_turns={max_turns})\
                             —— 提高 max_turns 或检查 agent 配置"
                        ),
                    ));
                }
                Ok(NodeResult {
                    node_id: node.id.clone(),
                    output: serde_json::json!({
                        "response": result.response,
                        "tools_used": result.tools_used,
                        "agent_id": agent_id,
                        "max_turns": max_turns,
                    }),
                    error: None,
                    state: ExecutionState::Completed,
                    started_at: started,
                    ended_at: Local::now(),
                    metadata: HashMap::new(),
                })
            }
            Err(e) => Ok(failed_node_result(
                &node.id,
                started,
                &format!("agent error: {}", e),
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Executor Registry
// ---------------------------------------------------------------------------
