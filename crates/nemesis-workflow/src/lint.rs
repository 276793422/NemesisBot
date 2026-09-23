//! 草稿期语义 lint（2026-09-23 计划类 C：生成质量保障机制化）。
//!
//! 与 `engine::validate_workflow`（error 通道，管结构合法性）分离：lint 是
//! **warning 通道**，管「能跑但大概率不合意图」的语义陷阱。规则结论经
//! `DraftSummary.warnings` 透传到草稿面板 / wsapi / `workflow_create` 工具
//! 响应的 hints——生成器按工具描述的「有错修了重存」回路当轮自纠。
//!
//! 新 pitfall 的接入方式：加一条规则函数，无需动任何消费方。

use crate::nodes::placeholder_refs;
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
    lint_edge_conditions(wf, &mut warnings);
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

/// L6：条件边引用了不存在的节点 id，或引用 condition 节点时字段名不是
/// `condition_result`（E 级复核实测：生成器把 condition 输出字段幻觉成
/// `passed`，`{{check.passed}}` 运行期解析不到、落「非空即真」兜底，路由
/// 与数据脱钩）。草稿期点名字段让生成器当轮自纠；运行期调度器把未解析
/// 条件升级为 Failed（缺陷 15，双保险）。首段不是任何节点 id 的引用
/// （变量/触发器入参，如 `{{payload.value}}`）静态不可判，交运行期把关。
fn lint_edge_conditions(wf: &Workflow, warnings: &mut Vec<String>) {
    let node_types: std::collections::HashMap<&str, &str> = wf
        .nodes
        .iter()
        .map(|n| (n.id.as_str(), n.node_type.as_str()))
        .collect();
    for edge in &wf.edges {
        let Some(cond) = edge.condition.as_deref() else {
            continue;
        };
        for (head, field) in placeholder_refs(cond) {
            let Some(node_type) = node_types.get(head.as_str()) else {
                // 引用的首段不是节点 id：变量/触发器入参，运行期把关。
                continue;
            };
            let Some(field) = field else {
                // `{{节点id}}` 整体输出，合法。
                continue;
            };
            if node_type == &"condition" && field != "condition_result" {
                // 占位符原文用独立 format 构造，避免 format! 花括号转义错数。
                let bad = format!("{{{{{}.{}}}}}", head, field);
                let good = format!("{{{{{}.condition_result}}}}", head);
                warnings.push(format!(
                    "条件边 {}→{} 引用了 {}——condition 节点的输出字段是 \
                     condition_result，不存在字段 {:?}，运行期会因解析不到直接 \
                     Failed。改用 {}",
                    edge.from_node, edge.to_node, bad, field, good
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests;
