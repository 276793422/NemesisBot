//! AI workflow generator capability declaration (对话生成).
//!
//! Single source of truth for the node types / trigger types the
//! `workflow_create` agent tool may emit, plus the config fields each node
//! executor actually reads. Two consumers:
//!
//! - `WorkflowCapabilitiesTool` (agent-facing): renders this table as a
//!   compact prompt block so the LLM generates valid definitions on the
//!   first try instead of guessing field names.
//! - Anti-drift tests (bottom of this file): pin the table against the real
//!   `NodeExecutorRegistry` so adding/removing an executor without updating
//!   this table fails the build loudly. The frontend NODE_CATALOG is a
//!   separate surface — this crate cannot depend on the Vue frontend — so
//!   the Rust executor registry is the anchor (frontend has drifted before:
//!   catalog once listed `question_classifier` years after the executor
//!   landed; tests like these are why the anchor is the registry itself).
//!
//! Config fields are transcribed from each executor's `config.get(...)`
//! reads in `nodes.rs`. When an executor learns a new config key, update
//! the matching row here — the summary line names the executor struct so
//! the reviewer knows where to look.

use serde::{Deserialize, Serialize};

/// One node type the generator may emit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeCapability {
    /// Value for `NodeDef.node_type`.
    pub node_type: String,
    /// One-line: what the node does + which executor implements it.
    pub summary: String,
    /// Config keys the executor cannot run without (failing NodeResult otherwise).
    pub required_config: Vec<String>,
    /// Optional config keys recognized by the executor.
    pub optional_config: Vec<String>,
    /// Value notes: accepted literals, defaults, template resolution.
    pub config_notes: String,
}

/// One trigger type the generator may attach to a workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerCapability {
    /// Value for `TriggerConfig.trigger_type`.
    pub trigger_type: String,
    /// One-line: when this trigger fires.
    pub summary: String,
    /// Expected keys inside `TriggerConfig.config`.
    pub config_notes: String,
}

/// Full capability declaration returned by `workflow_capabilities` and
/// rendered into the `workflow_create` tool bootstrap.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratorCapabilities {
    pub node_types: Vec<NodeCapability>,
    pub trigger_types: Vec<TriggerCapability>,
    /// Structural rules the generator must follow (ids unique, edges valid,
    /// terminal marking). Kept as data so the prompt and the tool schema stay
    /// in sync.
    pub structure_rules: Vec<String>,
}

/// The static capability table. Single source of truth — see module docs.
pub fn capabilities() -> GeneratorCapabilities {
    let node_types = vec![
        NodeCapability {
            node_type: "llm".into(),
            summary: "单次 LLM 调用（RealLLMNodeExecutor）".into(),
            required_config: vec!["prompt".into()],
            optional_config: vec![
                "model".into(),
                "temperature".into(),
                "max_tokens".into(),
                "system_prompt".into(),
            ],
            config_notes: "prompt/system_prompt 支持 {{var}} 模板；model 缺省用当前默认模型"
                .into(),
        },
        NodeCapability {
            node_type: "agent".into(),
            summary: "完整 agent 循环（AgentNodeExecutor，带工具）".into(),
            required_config: vec!["prompt".into()],
            optional_config: vec!["agent_id".into(), "max_turns".into(), "model".into()],
            config_notes: "max_turns 默认 5；需要真实工具链时选 agent 而不是 llm".into(),
        },
        NodeCapability {
            node_type: "tool".into(),
            summary: "调用一个已注册 agent 工具（RealToolNodeExecutor）".into(),
            required_config: vec!["name".into()],
            optional_config: vec!["args".into()],
            config_notes: "args 是 JSON 对象，{{var}} 会被解析；旧键 tool 兼容但请用 name".into(),
        },
        NodeCapability {
            node_type: "condition".into(),
            summary: "条件判断，输出 {condition_result: bool}（ConditionNodeExecutor）".into(),
            required_config: vec![],
            optional_config: vec!["condition".into()],
            config_notes: "表达式如 \"{{count}} > 5\"，支持 == != > < >= <=；缺省 \"false\"；通常作为出边的 condition 而不是独立节点".into(),
        },
        NodeCapability {
            node_type: "delay".into(),
            summary: "等待 N 秒（DelayNodeExecutor）".into(),
            required_config: vec!["seconds".into()],
            optional_config: vec![],
            config_notes: "单位是秒，支持小数（0.5 = 500ms）".into(),
        },
        NodeCapability {
            node_type: "transform".into(),
            summary: "文本变换（TransformNodeExecutor）".into(),
            required_config: vec!["input".into()],
            optional_config: vec!["expression".into(), "arg".into()],
            config_notes: "expression ∈ identity/trim/first_line/last_line/split_lines/json_extract/regex_match，缺省 identity；json_extract/regex_match 的参数放 arg；input 支持 {{var}}".into(),
        },
        NodeCapability {
            node_type: "http".into(),
            summary: "HTTP 请求（HTTPNodeExecutor）".into(),
            required_config: vec!["url".into()],
            optional_config: vec![
                "method".into(),
                "body".into(),
                "headers".into(),
                "timeout_secs".into(),
            ],
            config_notes: "method ∈ GET/POST/PUT/PATCH/DELETE/HEAD 缺省 GET；headers 是对象；timeout_secs 缺省 30；url/body/header 值支持 {{var}}".into(),
        },
        NodeCapability {
            node_type: "script".into(),
            summary: "系统解释器执行脚本（ScriptNodeExecutor）".into(),
            required_config: vec!["script".into()],
            optional_config: vec!["language".into(), "sandbox".into()],
            config_notes: "language ∈ bash/python/node/javascript/powershell/pwsh/sh/bat/cmd 缺省 bash；sandbox=false 显式退出沙盒执行车道；脚本支持 {{var}}".into(),
        },
        NodeCapability {
            node_type: "question_classifier".into(),
            summary: "LLM 分类器，输出 {class_id, confidence}（QuestionClassifierNodeExecutor）".into(),
            required_config: vec!["question".into(), "classes".into()],
            optional_config: vec![
                "system_prompt".into(),
                "model".into(),
                "max_attempts".into(),
                "temperature".into(),
            ],
            config_notes: "classes 是 [{id, description}] 数组；下游条件边用 {{class_id}} 分支；temperature 建议配 0".into(),
        },
        NodeCapability {
            node_type: "parameter_extractor".into(),
            summary: "LLM 参数抽取（ParameterExtractorNodeExecutor）".into(),
            required_config: vec!["text".into(), "parameters".into()],
            optional_config: vec![
                "system_prompt".into(),
                "model".into(),
                "max_attempts".into(),
                "temperature".into(),
            ],
            config_notes: "parameters 是字段定义数组（name/type/description/required）；抽取结果按字段名进输出".into(),
        },
        NodeCapability {
            node_type: "human_review".into(),
            summary: "人工审核，节点停在 Waiting 直到 resume（HumanReviewNodeExecutor）".into(),
            required_config: vec![],
            optional_config: vec!["message".into()],
            config_notes: "message 支持 {{var}}（审核者看到真实内容）；含此节点的工作流不可用于 workflow_chat 页面（chat_eligible 拒绝）".into(),
        },
        NodeCapability {
            node_type: "parallel".into(),
            summary: "并行执行子节点（ParallelNodeExecutor）".into(),
            required_config: vec!["nodes".into()],
            optional_config: vec!["branches".into()],
            config_notes: "nodes/branches 是内联 NodeDef 数组（每项含 id/node_type/config）；branches 仅作兜底，优先用 nodes".into(),
        },
        NodeCapability {
            node_type: "loop".into(),
            summary: "循环执行子节点（LoopNodeExecutor）".into(),
            required_config: vec!["nodes".into()],
            optional_config: vec!["mode".into(), "max_iterations".into()],
            config_notes: "mode ∈ counter/list；max_iterations 默认 10；nodes 是内联子节点数组".into(),
        },
        NodeCapability {
            node_type: "sub_workflow".into(),
            summary: "调用另一个已注册工作流（SubWorkflowNodeExecutor）".into(),
            required_config: vec!["workflow".into()],
            optional_config: vec![],
            config_notes: "workflow 是目标工作流名，必须已注册；禁止自引用（成环校验会拒）".into(),
        },
    ];

    let trigger_types = vec![
        TriggerCapability {
            trigger_type: "cron".into(),
            summary: "定时触发（croner 表达式）".into(),
            config_notes: "config: {\"schedule\": \"0 9 * * *\"}；6 段含秒或 5 段均可".into(),
        },
        TriggerCapability {
            trigger_type: "webhook".into(),
            summary: "HTTP webhook 触发".into(),
            config_notes: "config 可空；POST /api/workflow/webhook/<name> 即触发".into(),
        },
        TriggerCapability {
            trigger_type: "event".into(),
            summary: "事件总线订阅触发".into(),
            config_notes: "config: {\"event_type\": \"...\"} 匹配发布的 TriggerEvent".into(),
        },
        TriggerCapability {
            trigger_type: "message".into(),
            summary: "入站消息匹配触发".into(),
            config_notes: "config 可含 channel/content/sender 过滤条件，匹配即启动".into(),
        },
    ];

    let structure_rules = vec![
        "node id 全局唯一且非空；edges 的 from_node/to_node 必须引用存在的 id；图必须是 DAG（无环）"
            .into(),
        "需要向调用方返回结果的节点设 is_terminal=true（多个则输出按 node id 合并）；不设则取叶子节点"
            .into(),
        "依赖顺序可用 depends_on 显式声明（除边之外的控制流）；两机制可并存但不要冗余"
            .into(),
        "节点间传值用 {{节点id.字段}} 或 {{变量名}} 模板占位符（执行上下文变量）".into(),
        "工作流 name 用英文标识符风格（会作为文件名落盘，sanitize 只保留安全字符）".into(),
        "triggers 可为空数组（手动运行 / workflow_run 工具 / 对话测试驱动）；trigger_type 只能取声明的四种"
            .into(),
    ];

    GeneratorCapabilities {
        node_types,
        trigger_types,
        structure_rules,
    }
}

/// Render the capability table as a compact markdown block for LLM prompts
/// (tool description / bootstrap injection). Deliberately terse — every
/// token of the tool description is prompt-cache pressure.
pub fn render_for_prompt() -> String {
    let caps = capabilities();
    let mut out = String::with_capacity(4096);
    out.push_str("## 可用节点类型\n\n");
    out.push_str("| node_type | 必填 config | 可选 config | 说明 |\n|---|---|---|---|\n");
    for n in &caps.node_types {
        out.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            n.node_type,
            n.required_config.join(", "),
            n.optional_config.join(", "),
            n.summary
        ));
    }
    out.push_str("\n## 触发器类型\n\n");
    for t in &caps.trigger_types {
        out.push_str(&format!(
            "- `{}`: {}（{}）\n",
            t.trigger_type, t.summary, t.config_notes
        ));
    }
    out.push_str("\n## 结构规则\n\n");
    for (i, r) in caps.structure_rules.iter().enumerate() {
        out.push_str(&format!("{}. {}\n", i + 1, r));
    }
    out
}

#[cfg(test)]
mod tests;
