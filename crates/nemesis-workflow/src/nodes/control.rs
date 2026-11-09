// ---------------------------------------------------------------------------
// 机械拆分：本文件内容由 nodes.rs 原样搬入（纯代码搬家，行为零变更）。
// ---------------------------------------------------------------------------

use super::*;

// 跨子文件引用（机械拆分）：helpers 定义在 llm.rs
use super::llm::{failed_node_result, resolve_runtime_string};

/// Built-in condition node executor.
pub struct ConditionNodeExecutor;

#[async_trait]
impl NodeExecutor for ConditionNodeExecutor {
    async fn execute(
        &self,
        node: &NodeDef,
        context: &HashMap<String, serde_json::Value>,
        wf_ctx: &WorkflowContext,
    ) -> Result<NodeResult, String> {
        let now = Local::now();
        let condition = node
            .config
            .get("condition")
            .and_then(|v| v.as_str())
            .unwrap_or("false");

        // 守卫求值（缺陷 15 同类清账）：幻觉字段 → 节点 Failed 点名，
        // 不可证残留 → 条件按不成立（false），绝不静默选边。已完成集合
        // 每次执行取一次快照。
        let completed: std::collections::HashSet<String> = wf_ctx
            .get_all_node_results()
            .into_iter()
            .filter(|(_, r)| r.state == ExecutionState::Completed)
            .map(|(id, _)| id)
            .collect();
        let result = match eval_condition_with_falsifiability(
            condition,
            context,
            &completed,
            &format!("节点 {:?}（condition）的条件表达式", node.id),
        ) {
            Ok(r) => r,
            Err(e) => return Ok(failed_node_result(&node.id, now, &e)),
        };

        Ok(NodeResult {
            node_id: node.id.clone(),
            output: serde_json::json!({ "condition_result": result }),
            error: None,
            state: ExecutionState::Completed,
            started_at: now,
            ended_at: Local::now(),
            metadata: HashMap::new(),
        })
    }
}

// ---------------------------------------------------------------------------
// Parallel Node
// ---------------------------------------------------------------------------

/// Built-in parallel node executor.
///
/// Executes child nodes concurrently using tokio tasks. Child nodes are
/// looked up from the executor registry by their node_type.
pub struct ParallelNodeExecutor {
    registry: Arc<NodeExecutorRegistry>,
}

impl ParallelNodeExecutor {
    /// Create a new parallel executor with access to the given registry.
    pub fn new(registry: Arc<NodeExecutorRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl NodeExecutor for ParallelNodeExecutor {
    async fn execute(
        &self,
        node: &NodeDef,
        context: &HashMap<String, serde_json::Value>,
        _wf_ctx: &WorkflowContext,
    ) -> Result<NodeResult, String> {
        let now = Local::now();

        // Try "nodes" first, fall back to "branches"
        let children = {
            let nodes = get_config_node_list(&node.config, "nodes");
            if nodes.is_empty() {
                get_config_node_list(&node.config, "branches")
            } else {
                nodes
            }
        };

        if children.is_empty() {
            return Ok(NodeResult {
                node_id: node.id.clone(),
                output: serde_json::json!({ "results": [] }),
                error: None,
                state: ExecutionState::Completed,
                started_at: now,
                ended_at: Local::now(),
                metadata: HashMap::new(),
            });
        }

        // Spawn a task for each child node
        let mut handles = Vec::new();
        for (idx, child_def) in children.into_iter().enumerate() {
            let registry = Arc::clone(&self.registry);
            let ctx = context.clone();
            let branch_key = format!("branch_{}", idx);

            handles.push(tokio::spawn(async move {
                let executor = match registry.get(&child_def.node_type) {
                    Some(e) => e,
                    None => {
                        return (
                            branch_key,
                            Err(format!(
                                "unknown node type {:?} in parallel block",
                                child_def.node_type
                            )),
                        );
                    }
                };

                // Create a local context for the child node since we cannot
                // borrow the parent's wf_ctx across spawn boundaries.
                let local_wf_ctx = WorkflowContext::new(ctx.clone());
                let result = executor.execute(&child_def, &ctx, &local_wf_ctx).await;
                (branch_key, result)
            }));
        }

        // Collect results
        let mut merged = serde_json::Map::new();
        let mut first_error: Option<String> = None;

        for handle in handles {
            match handle.await {
                Ok((key, Ok(result))) => {
                    if result.state == ExecutionState::Failed {
                        if first_error.is_none() {
                            first_error = result.error.clone();
                        }
                        merged.insert(
                            key,
                            serde_json::json!({ "error": result.error, "output": result.output }),
                        );
                    } else if matches!(
                        result.state,
                        ExecutionState::Waiting | ExecutionState::Cancelled
                    ) {
                        // 运行期兜底：暂停类子状态不得被
                        // 当成功吞掉。注册期 validate 已拒绝 human_review 复合
                        // 子节点；此臂防程序化构造（绕过 validate 的直接
                        // schedule 路径）与子工作流停在审批的 sub_workflow
                        // 分支——诚实失败，绝不静默放行。
                        let msg = format!(
                            "parallel 分支 {key} 返回 {} 状态：暂停类节点不支持在 composite 内（审批落地前 workflow 不得继续）",
                            result.state
                        );
                        if first_error.is_none() {
                            first_error = Some(msg.clone());
                        }
                        merged.insert(
                            key,
                            serde_json::json!({ "error": msg, "state": result.state }),
                        );
                    } else {
                        merged.insert(key, result.output);
                    }
                }
                Ok((key, Err(e))) => {
                    if first_error.is_none() {
                        first_error = Some(e.clone());
                    }
                    merged.insert(key, serde_json::json!({ "error": e }));
                }
                Err(e) => {
                    if first_error.is_none() {
                        first_error = Some(e.to_string());
                    }
                }
            }
        }

        let state = if first_error.is_some() {
            ExecutionState::Failed
        } else {
            ExecutionState::Completed
        };

        Ok(NodeResult {
            node_id: node.id.clone(),
            output: serde_json::Value::Object(merged),
            error: first_error,
            state,
            started_at: now,
            ended_at: Local::now(),
            metadata: HashMap::new(),
        })
    }
}

// ---------------------------------------------------------------------------
// Delay Node
// ---------------------------------------------------------------------------

/// Built-in delay node executor.
pub struct DelayNodeExecutor;

#[async_trait]
impl NodeExecutor for DelayNodeExecutor {
    async fn execute(
        &self,
        node: &NodeDef,
        _context: &HashMap<String, serde_json::Value>,
        _wf_ctx: &WorkflowContext,
    ) -> Result<NodeResult, String> {
        let now = Local::now();
        // Form label is "等待秒数" — treat the value as seconds, not millis.
        // Parse as f64 so fractional seconds work (e.g. seconds=0.01 → 10ms),
        // matching what every form placeholder implies. Whole numbers (2) and
        // floats (0.5) both serialize to JSON numbers that as_f64() accepts.
        // (Earlier code used `from_millis(secs)` which made a `seconds=2` config
        // delay for 2ms, contradicting every form placeholder.)
        let secs = node
            .config
            .get("seconds")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        // from_secs_f64 对负数/NaN/超 u64 秒直接 panic（工作流引擎任务被
        // 一个坏节点配置炸掉）；try 形态把非法值归约为诚实节点失败，引擎
        // 照常存活。
        let delay = std::time::Duration::try_from_secs_f64(secs)
            .map_err(|e| format!("延迟秒数非法 {secs}: {e}"))?;
        tokio::time::sleep(delay).await;
        Ok(NodeResult {
            node_id: node.id.clone(),
            output: serde_json::json!({ "delayed_seconds": secs }),
            error: None,
            state: ExecutionState::Completed,
            started_at: now,
            ended_at: Local::now(),
            metadata: HashMap::new(),
        })
    }
}

// ---------------------------------------------------------------------------
// Transform Node
// ---------------------------------------------------------------------------

/// Built-in transform node executor.
///
/// Applies a named transform to `config.input`. Available expressions:
/// - `identity` — return input unchanged
/// - `trim` — strip leading/trailing whitespace
/// - `first_line` / `last_line` — first/last non-empty line of input
/// - `split_lines` — split input into an array of lines
/// - `json_extract` — extract a field from a JSON document via dotted path
///   (`arg` = path, e.g. `data.user.name`); supports array index `data[0].id`
/// - `regex_match` — apply a regex (`arg` = pattern); if the pattern has a
///   capture group, returns the first capture, otherwise the full match
pub struct TransformNodeExecutor;

#[async_trait]
impl NodeExecutor for TransformNodeExecutor {
    async fn execute(
        &self,
        node: &NodeDef,
        context: &HashMap<String, serde_json::Value>,
        _wf_ctx: &WorkflowContext,
    ) -> Result<NodeResult, String> {
        let started = Local::now();

        let expression = node
            .config
            .get("expression")
            .and_then(|v| v.as_str())
            .unwrap_or("identity")
            .to_string();

        let input_raw = node
            .config
            .get("input")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        // Resolve {{var}} (or a whole-value credential reference) in the
        // input before transforming.
        let input = match resolve_runtime_string(&input_raw, context, "transform.input") {
            Ok(v) => v,
            Err(e) => return Ok(failed_node_result(&node.id, started, &e)),
        };

        let arg = node
            .config
            .get("arg")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let output = match expression.as_str() {
            "identity" | "passthrough" => serde_json::json!({ "text": input }),

            "trim" => serde_json::json!({ "text": input.trim() }),

            "first_line" => {
                let line = input
                    .lines()
                    .map(str::trim)
                    .find(|l| !l.is_empty())
                    .unwrap_or("")
                    .to_string();
                serde_json::json!({ "text": line })
            }

            "last_line" => {
                let line = input
                    .lines()
                    .map(str::trim)
                    .rfind(|l| !l.is_empty())
                    .unwrap_or("")
                    .to_string();
                serde_json::json!({ "text": line })
            }

            "split_lines" => {
                let lines: Vec<&str> = input
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .collect();
                serde_json::json!({ "lines": lines })
            }

            "json_extract" => {
                let trimmed = input.trim();
                let parsed: serde_json::Value = if trimmed.is_empty() {
                    serde_json::Value::Null
                } else {
                    match serde_json::from_str(trimmed) {
                        Ok(v) => v,
                        Err(e) => {
                            return Ok(failed_node_result(
                                &node.id,
                                started,
                                &format!("json_extract: input is not valid JSON: {}", e),
                            ));
                        }
                    }
                };
                let extracted = json_path_lookup(&parsed, arg);
                match extracted {
                    serde_json::Value::String(s) => serde_json::json!({ "text": s }),
                    other => serde_json::json!({ "value": other }),
                }
            }

            "regex_match" => {
                if arg.is_empty() {
                    return Ok(failed_node_result(
                        &node.id,
                        started,
                        "regex_match: 'arg' (pattern) is required",
                    ));
                }
                let re = match regex::Regex::new(arg) {
                    Ok(r) => r,
                    Err(e) => {
                        return Ok(failed_node_result(
                            &node.id,
                            started,
                            &format!("regex_match: invalid pattern '{}': {}", arg, e),
                        ));
                    }
                };
                let matched = match re.captures(&input) {
                    Some(caps) => {
                        // If pattern has a capture group, return the first
                        // capture; otherwise return the full match.
                        if caps.len() > 1 {
                            caps.get(1).map(|m| m.as_str().to_string())
                        } else {
                            caps.get(0).map(|m| m.as_str().to_string())
                        }
                    }
                    None => None,
                };
                serde_json::json!({ "text": matched.unwrap_or_default() })
            }

            other => {
                return Ok(failed_node_result(
                    &node.id,
                    started,
                    &format!("transform: unknown expression '{}'", other),
                ));
            }
        };

        // If output_type is text/markdown/xml, unwrap {"text": "..."} into
        // a bare JSON string so downstream observers (workflow_chat reply)
        // can pass it through as the user-facing reply without JSON-dumping.
        // json/unset/incompatible-shape: leave the object shape unchanged
        // (best-effort; observer JSON-dumps as fallback).
        let output = unwrap_text_output_if_requested(output, node);

        Ok(NodeResult {
            node_id: node.id.clone(),
            output,
            error: None,
            state: ExecutionState::Completed,
            started_at: started,
            ended_at: Local::now(),
            metadata: HashMap::new(),
        })
    }
}

/// If the node declared `output_type: text|markdown|xml`, convert the
/// `{"text": "..."}` envelope into a bare JSON string. Returns the original
/// output unchanged for any other case (no output_type, output_type=json,
/// or the expression produced a shape we don't know how to unwrap).
///
/// Best-effort: when output_type is text/markdown/xml but the expression
/// produced a different shape (e.g. `split_lines` → `{lines: [...]}`),
/// the original output is returned as-is. The workflow_chat reply observer
/// will then JSON-dump it via the existing fallback path — same behavior
/// as if output_type had not been set.
fn unwrap_text_output_if_requested(output: serde_json::Value, node: &NodeDef) -> serde_json::Value {
    let output_type = match node.config.get("output_type").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => return output,
    };
    match output_type {
        "text" | "markdown" | "xml" => {}
        // json or anything else: leave as-is.
        _ => return output,
    }
    match output.get("text").and_then(|v| v.as_str()) {
        Some(s) => serde_json::Value::String(s.to_string()),
        // Can't unwrap (e.g. split_lines → {lines: [...]}, or json_extract
        // hit a non-string). Leave as-is; observer's JSON-dump fallback
        // handles it.
        None => output,
    }
}

/// Look up a dotted path inside a JSON value.
///
/// Supports:
/// - Field access: `data.name`, `data.user.id`
/// - Array indexing: `items[0]`, `data.users[2].name`
/// - Negative indices: `items[-1]` (last element)
///
/// Missing keys / out-of-range indices resolve to `Value::Null`.
pub(super) fn json_path_lookup(root: &serde_json::Value, path: &str) -> serde_json::Value {
    if path.is_empty() {
        return root.clone();
    }
    let mut current = root.clone();
    // Tokenise: split on '.' but keep bracketed indices attached to the
    // preceding key (so `data.users[0].name` → ["data", "users[0]", "name"]).
    for raw in path.split('.') {
        // Extract a leading key (may be empty if path starts with [0]).
        let bytes = raw.as_bytes();
        let key_end = bytes.iter().position(|&b| b == b'[').unwrap_or(raw.len());
        if key_end > 0 {
            let key = &raw[..key_end];
            current = match &current {
                serde_json::Value::Object(obj) => {
                    obj.get(key).cloned().unwrap_or(serde_json::Value::Null)
                }
                _ => serde_json::Value::Null,
            };
        }
        // Apply each `[index]` after the key.
        let rest = &raw[key_end..];
        let mut idx = 0;
        while idx < rest.len() {
            let bytes = rest.as_bytes();
            if bytes[idx] != b'[' {
                break;
            }
            let close = rest[idx..].find(']').map(|p| p + idx);
            let close = match close {
                Some(p) => p,
                None => break,
            };
            let inner = &rest[idx + 1..close];
            let i: isize = match inner.parse() {
                Ok(n) => n,
                Err(_) => return serde_json::Value::Null,
            };
            current = match &current {
                serde_json::Value::Array(arr) => {
                    let real_idx = if i < 0 {
                        (arr.len() as isize + i) as usize
                    } else {
                        i as usize
                    };
                    arr.get(real_idx)
                        .cloned()
                        .unwrap_or(serde_json::Value::Null)
                }
                _ => serde_json::Value::Null,
            };
            idx = close + 1;
            // Skip optional UTF-8 whitespace.
            while idx < rest.len() && rest.as_bytes()[idx].is_ascii_whitespace() {
                idx += 1;
            }
        }
    }
    current
}

// ---------------------------------------------------------------------------
// Loop Node
// ---------------------------------------------------------------------------
/// Built-in human review node executor.
///
/// Pauses workflow execution until a human reviews and approves/rejects.
/// Returns a Waiting state; the engine is responsible for pausing.
pub struct HumanReviewNodeExecutor;

#[async_trait]
impl NodeExecutor for HumanReviewNodeExecutor {
    async fn execute(
        &self,
        node: &NodeDef,
        context: &HashMap<String, serde_json::Value>,
        _wf_ctx: &WorkflowContext,
    ) -> Result<NodeResult, String> {
        let now = Local::now();

        let raw_message = node
            .config
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("Human review required");
        // Form placeholder is `请审核是否发送给客户：{{draft}}` — resolve
        // {{var}} placeholders (or a whole-value credential reference)
        // against the execution context so reviewers see the actual
        // content, not literal `{{draft}}`.
        let message = match resolve_runtime_string(raw_message, context, "human_review.message") {
            Ok(v) => v,
            Err(e) => return Ok(failed_node_result(&node.id, now, &e)),
        };

        Ok(NodeResult {
            node_id: node.id.clone(),
            output: serde_json::json!({
                "message": message,
                "status": "waiting_for_review",
            }),
            error: None,
            state: ExecutionState::Waiting,
            started_at: now,
            ended_at: Local::now(),
            metadata: HashMap::new(),
        })
    }
}

// ---------------------------------------------------------------------------
// Question Classifier Node (1b-)
// ---------------------------------------------------------------------------
/// Evaluate a condition string against the context.
///
/// Supports:
/// - `{{var}}` template substitution OR bare variable names
///   (both `{{count}} > 5` and `count > 5` work)
/// - `==`, `!=`, `>`, `<`, `>=`, `<=` operators
///   (numeric comparison when both sides parse as numbers,
///   otherwise lexicographic string comparison)
/// - Bare variable name — truthy check against the context value
///   (numbers: non-zero; strings: non-empty; etc.)
/// - Literal `"true"` / `"false"` — boolean
pub fn evaluate_condition(condition: &str, context: &HashMap<String, serde_json::Value>) -> bool {
    let condition = condition.trim();
    if condition.is_empty() {
        return false;
    }

    // Step 1: Resolve {{var}} placeholders. After this the condition is a
    // literal expression like `5 > 3` or `hello == hello`.
    // 注意：条件边是表达式不是数据通道，不走 `resolve_runtime_string` 的
    // 凭据引用识别（`env:` 开头的条件表达式无意义；且本函数无节点语境，
    // 解析失败无处落 Failed—— 计划类 B 范围裁定）。
    let resolved = resolve_prompt_template(condition, context);
    let resolved = resolved.trim();

    // Step 1.5: Negation prefix（缺陷 14 修复）。`!{{flag}}` 是
    // LLM 生成器表达否定分支的自然写法（能力表已同步收录该语法）；模板解析
    // 后字面以 `!` 开头 → 对剩余表达式取反。旧实现四步全不匹配落到 Step 4
    // 「非空即真」，"!true" 恒为真——否定分支无条件放行（静默错，违反诚实
    // 失败契约）。`!=` 是中缀运算符不会出现在表达式开头，无歧义。
    if let Some(rest) = resolved.strip_prefix('!') {
        return !evaluate_condition(rest, context);
    }

    // Step 2: Literal booleans (also covers cases where {{var}} resolved to
    // a boolean JSON value, which `resolve_prompt_template` stringifies as
    // "true"/"false"). "yes"/"no"/"1"/"0" 是边条件与人工配置的常见布尔拼写，
    // 统一在此收敛（统一定义；原 should_run_node 私有表已随缺陷 8 修复并入）。
    match resolved.to_lowercase().as_str() {
        "true" | "yes" | "1" => return true,
        "false" | "no" | "0" => return false,
        _ => {}
    }

    // Step 3: Comparison operators. Try longest-match first so `>=` doesn't
    // accidentally match the `>` inside `a >= b`.
    for op in [">=", "<=", "==", "!=", ">", "<"] {
        if let Some((left, right)) = resolved.split_once(op) {
            return compare_values(left.trim(), right.trim(), op, context);
        }
    }

    // Step 4: No operator — truthy check. If the expression is a bare
    // variable name, look it up in context and use the JSON value's
    // truthiness. Otherwise fall back to string truthiness.
    if let Some(val) = context.get(resolved) {
        return is_truthy(val);
    }
    is_truthy_str(resolved)
}

fn compare_values(
    left: &str,
    right: &str,
    op: &str,
    context: &HashMap<String, serde_json::Value>,
) -> bool {
    let lv = resolve_operand(left, context);
    let rv = resolve_operand(right, context);

    // Numeric comparison when both sides parse as numbers.
    if let (Some(a), Some(b)) = (lv.as_f64(), rv.as_f64()) {
        return match op {
            ">=" => a >= b,
            "<=" => a <= b,
            "==" => a == b,
            "!=" => a != b,
            ">" => a > b,
            "<" => a < b,
            _ => false,
        };
    }

    // Fall back to string comparison.
    let ls = match &lv {
        serde_json::Value::String(s) => s.clone(),
        o => o.to_string(),
    };
    let rs = match &rv {
        serde_json::Value::String(s) => s.clone(),
        o => o.to_string(),
    };
    match op {
        ">=" => ls >= rs,
        "<=" => ls <= rs,
        "==" => ls == rs,
        "!=" => ls != rs,
        ">" => ls > rs,
        "<" => ls < rs,
        _ => false,
    }
}

/// Resolve one operand (left or right side of a comparison) to a JSON value.
///
/// - `"quoted"` / `'quoted'` → unquoted literal string
/// - bare name in context → context value (variable lookup)
/// - numeric/bool/null literal → corresponding JSON value
/// - anything else → unquoted literal string
pub(super) fn resolve_operand(
    s: &str,
    context: &HashMap<String, serde_json::Value>,
) -> serde_json::Value {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return serde_json::Value::Null;
    }
    let bytes = trimmed.as_bytes();
    if bytes.len() >= 2
        && ((bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\''))
    {
        return serde_json::Value::String(trimmed[1..trimmed.len() - 1].to_string());
    }
    if let Some(v) = context.get(trimmed) {
        return v.clone();
    }
    if let Ok(n) = trimmed.parse::<f64>()
        && let Some(num) = serde_json::Number::from_f64(n)
    {
        return serde_json::Value::Number(num);
    }
    if trimmed == "true" {
        return serde_json::Value::Bool(true);
    }
    if trimmed == "false" {
        return serde_json::Value::Bool(false);
    }
    if trimmed == "null" {
        return serde_json::Value::Null;
    }
    serde_json::Value::String(trimmed.to_string())
}

pub(super) fn is_truthy_str(s: &str) -> bool {
    let t = s.trim();
    !t.is_empty() && !t.eq_ignore_ascii_case("false") && t != "0" && t != "0.0"
}

fn is_truthy(val: &serde_json::Value) -> bool {
    match val {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(b) => *b,
        serde_json::Value::Number(n) => n.as_f64().is_some_and(|v| v != 0.0),
        serde_json::Value::String(s) => !s.is_empty(),
        serde_json::Value::Array(a) => !a.is_empty(),
        serde_json::Value::Object(o) => !o.is_empty(),
    }
}

/// 条件表达式守卫求值——缺陷 15「可证伪性二分」的单一真相源。三个生产
/// 求值面共用：条件边（`scheduler::eval_edge_conditions`）、condition
/// 节点（[`ConditionNodeExecutor`]）、loop 条件（LoopNodeExecutor /
/// LoopNodeStub）。
///
/// `resolve_prompt_template` 对缺失键保留 `{{…}}` 原样，残留 = 不可解析
/// 信号，按可证伪性分流：
/// - `Err`：占位符首段命中 `completed`（已完成节点集合）——上游输出已
///   完全落定、字段不存在可证 → 调用方必须诚实失败（节点 Failed / 执行
///   Failed / loop_error），绝不静默选边（缺陷 15 语义，能力表对条件边
///   明文承诺的行为）；
/// - `Ok(false)`：不可证（未定义变量 / 未执行节点——被丢弃分支、可选
///   触发器入参）→ 整条条件按不成立（保留 s12b 跳过语义）；
/// - 无残留 → 正常 [`evaluate_condition`]。
///
/// `site` 是错误消息的位点描述（如 `节点 branch 的条件边`）。注意：上游
/// 数据值本身含 `{{` 字面量时会误判为不可解析——可证臂落诚实 Failed、
/// 不可证臂落 false，两个方向都确定，不产生垃圾真值（与条件边的既有
/// 语义一致）。
pub(crate) fn eval_condition_with_falsifiability(
    condition: &str,
    context: &HashMap<String, serde_json::Value>,
    completed: &std::collections::HashSet<String>,
    site: &str,
) -> Result<bool, String> {
    let resolved = resolve_prompt_template(condition, context);
    if resolved.contains("{{") {
        let provable = placeholder_refs(condition)
            .iter()
            .any(|(head, _)| completed.contains(head.as_str()));
        if provable {
            return Err(format!(
                "{site}引用了已完成节点输出中不存在的字段（模板解析后仍残留 \
                 {{{{…}}}}）：{:?}。请核对上游节点 id 与其输出字段名（condition 节点 \
                 的输出字段是 condition_result）",
                condition
            ));
        }
        // 引用不可证伪：变量未提供/节点未执行 → 整条条件不成立。
        return Ok(false);
    }
    Ok(evaluate_condition(condition, context))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
