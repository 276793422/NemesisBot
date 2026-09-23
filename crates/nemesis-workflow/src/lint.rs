//! 草稿期语义 lint（2026-09-23 计划类 C：生成质量保障机制化）。
//!
//! 与 `engine::validate_workflow`（error 通道，管结构合法性）分离：lint 是
//! **warning 通道**，管「能跑但大概率不合意图」的语义陷阱。规则结论经
//! `DraftSummary.warnings` 透传到草稿面板 / wsapi / `workflow_create` 工具
//! 响应的 hints——生成器按工具描述的「有错修了重存」回路当轮自纠。
//!
//! 新 pitfall 的接入方式：加一条规则函数，无需动任何消费方。

use crate::types::Workflow;

/// 返回人类可读的 warning 列表（中文，面向生成器与用户）。
pub fn lint(wf: &Workflow) -> Vec<String> {
    let mut warnings = Vec::new();
    for node in &wf.nodes {
        match node.node_type.as_str() {
            "llm" => lint_llm(
                node.id.as_str(),
                node.config.get("max_tokens"),
                node.config.get("model"),
                &mut warnings,
            ),
            "http" => lint_http(node, &mut warnings),
            _ => {}
        }
        lint_output_placeholder(node, &mut warnings);
    }
    if wf.triggers.is_empty() {
        warnings.push(
            "工作流没有配置任何触发器（triggers 为空）——只能通过 run_now 手动执行。\
             请确认这符合用户意图；若需要自动触发，添加 cron/event/message/webhook \
             触发器之一，并在回复中向用户说明启动方式"
                .to_string(),
        );
    }
    warnings
}

/// L1：推理模型的思维链会消耗输出预算——max_tokens 过小则只出思维链无文本
/// （glm-5.3-flash 实测 max_tokens=200 全军覆没，见 BUG 清账缺陷 2）。
/// L4：model 填了档位名（如 "small"）——provider 400「模型不存在」
/// （E2E 实测，生成器把模型档位概念泄漏进 model 字段）。
fn lint_llm(
    node_id: &str,
    max_tokens: Option<&serde_json::Value>,
    model: Option<&serde_json::Value>,
    warnings: &mut Vec<String>,
) {
    if let Some(v) = max_tokens.and_then(|v| v.as_f64())
        && v < 500.0
    {
        warnings.push(format!(
            "节点 {node_id}（llm）max_tokens={v} 过小：推理模型的思维链会把它全部消耗，\
             导致无文本输出且节点失败——建议 ≥2000，或去掉该配置用 provider 默认值"
        ));
    }
    if let Some(m) = model.and_then(|v| v.as_str()) {
        let tier = m.trim().to_lowercase();
        if matches!(
            tier.as_str(),
            "auto"
                | "tiny"
                | "mini"
                | "small"
                | "normal"
                | "medium"
                | "big"
                | "large"
                | "huge"
                | "smart"
                | "fast"
                | "pro"
                | "max"
                | "ultra"
                | "lite"
                | "flash"
                | "turbo"
        ) {
            warnings.push(format!(
                "节点 {node_id}（llm）model=\"{m}\" 疑似档位名而不是模型名——\
                 provider 会报「模型不存在」且节点 Failed。model 应填具体模型名\
                 （vendor/model 格式，如 zhipu/glm-5.3-flash，`model list` 可查）；\
                 不确定就删掉 model 配置，用当前默认模型"
            ));
        }
    }
}

/// L3：凭据引用必须整值（`env:`/`yaml:`/`vault:` 前缀开头）。内嵌在
/// `Bearer vault:x`、`?key=env:X` 这类值里不会被模板层识别，会当字面量
/// 静默发出——部署前拦住。
fn lint_http(node: &crate::types::NodeDef, warnings: &mut Vec<String>) {
    let mut scan = |field: &str, value: &str| {
        for prefix in ["env:", "yaml:", "vault:"] {
            if value.contains(prefix) && !value.trim_start().starts_with(prefix) {
                warnings.push(format!(
                    "节点 {}（http）{} 的值内嵌了 \"{}\" 但未以它开头——凭据引用必须\
                     是完整值才会在运行时解析，内嵌会被当字面量发出。把引用拆成\
                     独立 header/模板变量",
                    node.id, field, prefix
                ));
            }
        }
    };
    if let Some(u) = node.config.get("url").and_then(|v| v.as_str()) {
        scan("url", u);
    }
    if let Some(b) = node.config.get("body").and_then(|v| v.as_str()) {
        scan("body", b);
    }
    if let Some(headers) = node.config.get("headers").and_then(|v| v.as_object()) {
        for (k, v) in headers {
            if let Some(s) = v.as_str() {
                scan(&format!("headers.{k}"), s);
            }
        }
    }
}

/// L5：`{{节点id.output}}` 占位符——`output` 不是任何节点的输出字段名，
/// 模板不会替换、字面量原样发给下游（E2E 实测生成器高频臆造此形式，
/// 见 BUG 清账缺陷 6）。字段名按节点类型枚举：http 是 body/status_code，
/// llm 是 text；整个输出 JSON 用 `{{节点id}}`。
fn lint_output_placeholder(node: &crate::types::NodeDef, warnings: &mut Vec<String>) {
    let fields: &[&str] = match node.node_type.as_str() {
        "llm" => &["prompt", "system_prompt"],
        "agent" => &["prompt"],
        "http" => &["url", "body"],
        _ => return,
    };
    for field in fields {
        if let Some(s) = node.config.get(*field).and_then(|v| v.as_str())
            && (s.contains(".output}}") || s.contains(".output }}"))
        {
            warnings.push(format!(
                "节点 {}（{}）{} 里写了 {{{{….output}}}}——output 不是输出字段名，\
                 不会被替换，字面量会原样进入下游。http 节点字段用 \
                 {{{{节点id.body}}}}/{{{{节点id.status_code}}}}，llm 节点用 \
                 {{{{节点id.text}}}}，整个输出 JSON 用 {{{{节点id}}}}",
                node.id, node.node_type, field
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{NodeDef, TriggerConfig};
    use std::collections::HashMap;

    fn wf(nodes: Vec<NodeDef>, triggers: Vec<TriggerConfig>) -> Workflow {
        Workflow {
            name: "t".into(),
            description: String::new(),
            version: "1.0.0".into(),
            triggers,
            nodes,
            edges: Vec::new(),
            variables: HashMap::new(),
            metadata: HashMap::new(),
        }
    }

    fn node(id: &str, node_type: &str, config: serde_json::Value) -> NodeDef {
        NodeDef {
            id: id.into(),
            node_type: node_type.into(),
            config: serde_json::from_value(config).unwrap(),
            depends_on: Vec::new(),
            retry_count: 0,
            timeout: None,
            is_terminal: false,
        }
    }

    #[test]
    fn l1_small_llm_max_tokens_warns() {
        let w = wf(
            vec![node(
                "n1",
                "llm",
                serde_json::json!({"prompt": "p", "max_tokens": 200}),
            )],
            vec![TriggerConfig {
                trigger_type: "cron".into(),
                config: HashMap::new(),
            }],
        );
        let warnings = lint(&w);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("n1") && warnings[0].contains("max_tokens=200"));
    }

    #[test]
    fn l1_adequate_max_tokens_and_missing_key_stay_silent() {
        let w = wf(
            vec![
                node(
                    "n1",
                    "llm",
                    serde_json::json!({"prompt": "p", "max_tokens": 2000}),
                ),
                node("n2", "llm", serde_json::json!({"prompt": "p"})),
            ],
            vec![],
        );
        let warnings = lint(&w);
        // 只剩 triggers 为空这一条
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("触发器"));
    }

    #[test]
    fn l2_empty_triggers_warn_once() {
        let w = wf(
            vec![node("n1", "http", serde_json::json!({"url": "https://x"}))],
            vec![],
        );
        let warnings = lint(&w);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("run_now"));
    }

    #[test]
    fn l3_embedded_credential_reference_warns() {
        let w = wf(
            vec![node(
                "h",
                "http",
                serde_json::json!({
                    "url": "https://api.example.com?token=env:TOKEN",
                    "headers": {"Authorization": "Bearer vault:tok", "X-Ok": "vault:whole"}
                }),
            )],
            vec![TriggerConfig {
                trigger_type: "cron".into(),
                config: HashMap::new(),
            }],
        );
        let warnings = lint(&w);
        // url 内嵌 env: + Authorization 内嵌 vault: 各一条；X-Ok 整值合法
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings.iter().any(|w| w.contains("url")));
        assert!(warnings.iter().any(|w| w.contains("headers.Authorization")));
    }

    #[test]
    fn l4_tier_name_as_model_warns_real_model_stays_silent() {
        let w = wf(
            vec![
                node(
                    "n1",
                    "llm",
                    serde_json::json!({"prompt": "p", "model": "small"}),
                ),
                node(
                    "n2",
                    "llm",
                    serde_json::json!({"prompt": "p", "model": "zhipu/glm-5.3-flash"}),
                ),
                node(
                    "n3",
                    "llm",
                    serde_json::json!({"prompt": "p", "model": "gemini-2.5-pro"}),
                ),
            ],
            vec![TriggerConfig {
                trigger_type: "cron".into(),
                config: HashMap::new(),
            }],
        );
        let warnings = lint(&w);
        // 只有 n1 的档位名触发；真名（含档位词的完整模型名）不误报
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].contains("n1") && warnings[0].contains("档位名"),
            "{warnings:?}"
        );
    }

    #[test]
    fn l5_output_placeholder_typo_warns_with_field_guidance() {
        let w = wf(
            vec![
                node(
                    "s",
                    "llm",
                    serde_json::json!({"prompt": "汇总：{{fetch_todo.output}}"}),
                ),
                node(
                    "h",
                    "http",
                    serde_json::json!({"url": "https://x?p={{a.output}}"}),
                ),
            ],
            vec![TriggerConfig {
                trigger_type: "cron".into(),
                config: HashMap::new(),
            }],
        );
        let warnings = lint(&w);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        for w in &warnings {
            assert!(
                w.contains("output 不是输出字段名") && w.contains("{{节点id."),
                "{w}"
            );
        }
        assert!(warnings[0].contains("s") && warnings[0].contains("prompt"));
        assert!(warnings[1].contains("h") && warnings[1].contains("url"));
    }

    #[test]
    fn l5_correct_field_references_stay_silent() {
        let w = wf(
            vec![node(
                "s",
                "llm",
                serde_json::json!({
                    "prompt": "汇总：{{fetch_todo.body}} / {{fetch_todo}} / {{user_name}}"
                }),
            )],
            vec![TriggerConfig {
                trigger_type: "cron".into(),
                config: HashMap::new(),
            }],
        );
        assert!(lint(&w).is_empty(), "{:?}", lint(&w));
    }

    #[test]
    fn no_warnings_for_clean_workflow() {
        let w = wf(
            vec![node(
                "h",
                "http",
                serde_json::json!({
                    "url": "https://api.example.com",
                    "headers": {"Authorization": "vault:tok"}
                }),
            )],
            vec![TriggerConfig {
                trigger_type: "cron".into(),
                config: HashMap::new(),
            }],
        );
        assert!(lint(&w).is_empty(), "{:?}", lint(&w));
    }
}
