//! AOR 批三 5.3：重规划器（replanner）纯逻辑层。
//!
//! 职责边界与 [`crate::planner`] / [`crate::review`] 同构：本模块**不含
//! 任何 LLM 调用**——系统提示词 re-export、建议输出 schema、解析与校验、
//! 回灌重试提示词都是纯函数（单测零依赖）；LLM 调用编排（`run_detached`
//! 裸提示词模式 + 失败回灌 ≤2 次）、触发点接线、assisted/autonomous 分流、
//! 策略闸与执行在 nemesisbot 装配层（`board_replan.rs`）。
//!
//! 核心不变量：**建议 ≠ 授权**（proposal ≠ authorization）。本层产出的
//! 是结构化建议（[`ReplanOutput`]），执行裁决在 master 侧策略闸/人工——
//! 宁可停不可闯。

use serde::{Deserialize, Serialize};

use crate::planner::PlannedSubIssue;

/// 重规划系统提示词（文本单一真相源在 `nemesis-prompts`；此处 re-export
/// 保持与 planner/review 同款公开路径不变）。
pub use nemesis_prompts::board::REPLAN_SYSTEM_PROMPT;

/// 重规划系统提示词软预算告警挂在包装层：超
/// [`nemesis_prompts::board::REPLAN_SOFT_BUDGET_BYTES`] 时 WARN 不截断
/// （提示词资产失控的治理信号，对齐 planner 先例；资产 crate 零运行时
/// 依赖，tracing 消费点住本 crate）。
pub fn replan_system_prompt() -> &'static str {
    let p = nemesis_prompts::board::REPLAN_SYSTEM_PROMPT;
    if p.len() > nemesis_prompts::board::REPLAN_SOFT_BUDGET_BYTES {
        tracing::warn!(
            "[prompt] replan 系统提示词共 {} 字节，超过软预算 {} 字节（检查 REPLAN_SYSTEM_PROMPT 是否失控）",
            p.len(),
            nemesis_prompts::board::REPLAN_SOFT_BUDGET_BYTES
        );
    }
    p
}

/// 单次建议动作数硬上限（防 LLM 失控爆建议；提示词纪律同步声明克制）。
pub const MAX_REPLAN_ACTIONS: usize = 24;

/// 重规划建议评论前缀（系统评论识别标记）：assisted 审批链与去重闸
/// 都按本前缀在父单评论里识别「建议在场」。计数即版本号（第 N 条同
/// 前缀评论 = 计划 vN 建议）。
pub const PROPOSAL_COMMENT_PREFIX: &str = "🔁 重规划建议：";

/// 建议动作类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReplanOp {
    /// 取消子单（重派无意义/被取代）。
    Cancel,
    /// 修改子单元数据（标题/描述/验收标准）。
    Modify,
    /// 追加新子单（探明/补救/替代路径）。
    Add,
}

/// 单条建议动作（schema；serde 宽容：缺字段用默认值，硬校验在
/// [`parse_replan`]）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ReplanAction {
    /// 动作类型（语义主干，缺失 = 解析失败回灌，不猜）。
    #[serde(default)]
    pub op: Option<ReplanOp>,
    /// cancel/modify：被处置子单的看板编号（如 `NB-12`）。
    #[serde(default)]
    pub number: String,
    /// add：新子单标题。
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub acceptance_criteria: String,
    /// add：执行所需职能 slug（`family[:spec]`；空串 = 无职能需求；
    /// 格式校验同 planner D9，同一 `validate_slug` 单一真相源）。
    #[serde(default)]
    pub required_profession: String,
    /// add：批内依赖序号（0 起始，**只计 add 动作**；与 planner 的
    /// 批内序号语义同款）。
    #[serde(default)]
    pub depends_on: Vec<usize>,
    /// 为什么这样处置（一句话）。
    #[serde(default)]
    pub reason: String,
    /// 证据引用（硬性非空：来自输入材料的具体条目——失败单 fail_class/
    /// gap、unknown 事项、[TOUCH] 冲突、已完成子单清单等）。
    #[serde(default)]
    pub evidence: String,
}

/// 重规划建议输出（`analysis` 可空；空 actions 合法 = 诚实无建议，
/// 仅落分析文转人工）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ReplanOutput {
    #[serde(default)]
    pub analysis: String,
    #[serde(default)]
    pub actions: Vec<ReplanAction>,
}

/// 解析失败的原因（`message` 直接回灌给 LLM，必须人读且指明怎么改）。
#[derive(Debug, Clone, PartialEq)]
pub struct ReplanParseError {
    pub message: String,
}

impl std::fmt::Display for ReplanParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// 按出现序抽出 add 动作的批内视图（序号 0 起始，只计 add）。
fn add_actions(output: &ReplanOutput) -> Vec<&ReplanAction> {
    output
        .actions
        .iter()
        .filter(|a| a.op == Some(ReplanOp::Add))
        .collect()
}

/// 解析 + 校验重规划器 LLM 输出。
///
/// 容忍：markdown 代码围栏、前后杂散文字（取第一个 `{` 到最后一个
/// `}`，与 review/planner 同款）。硬校验：
/// - actions 条数 ≤ [`MAX_REPLAN_ACTIONS`]；
/// - **每条 action 的 evidence 非空**（给不出证据引用的建议不合格，
///   回灌自纠）；
/// - op 缺失 = 解析失败（语义主干猜不得）；
/// - cancel/modify 的 number 非空；modify 至少改一个字段
///   （title/description/acceptance_criteria——v1 不做派发属性改写）；
/// - add 的 title 非空、required_profession 非空时过职能 slug 校验
///   （与 planner 同一 `validate_slug`）、depends_on 界内（add 批）+
///   无自引用 + 无环（复用 planner DFS 单一真相源）。
///
/// 空 actions 合法（诚实无建议）。
pub fn parse_replan(raw: &str) -> Result<ReplanOutput, ReplanParseError> {
    let json_text = crate::review::extract_json_object(raw).ok_or_else(|| ReplanParseError {
        message: "输出中找不到 JSON 对象（应以 '{' 开头、'}' 结尾）。请只输出 JSON 对象本身。"
            .to_string(),
    })?;

    let out: ReplanOutput = serde_json::from_str(&json_text).map_err(|e| ReplanParseError {
        message: format!("JSON 解析失败：{e}。请检查引号/逗号/字段类型，只输出 JSON 对象。"),
    })?;

    if out.actions.len() > MAX_REPLAN_ACTIONS {
        return Err(ReplanParseError {
            message: format!(
                "actions 共 {} 条，超过上限 {MAX_REPLAN_ACTIONS}。重规划建议要克制：只保留证据支持的最小改动集。",
                out.actions.len()
            ),
        });
    }

    // add 批内序号是 depends_on 的引用空间（只计 add 动作，0 起始）。
    let adds = add_actions(&out);
    let add_count = adds.len();

    for (i, a) in out.actions.iter().enumerate() {
        let Some(op) = a.op else {
            return Err(ReplanParseError {
                message: format!(
                    "actions[{i}] 缺 op 字段（值域 cancel|modify|add）。op 是语义主干，必须显式给出。"
                ),
            });
        };
        if a.evidence.trim().is_empty() {
            return Err(ReplanParseError {
                message: format!(
                    "actions[{i}] 的 evidence 为空。每条建议必须附证据引用（来自输入材料的具体条目：失败单 fail_class/gap、unknown 事项、[TOUCH] 冲突等）；给不出证据的建议不要提出。"
                ),
            });
        }
        match op {
            ReplanOp::Cancel | ReplanOp::Modify => {
                if a.number.trim().is_empty() {
                    return Err(ReplanParseError {
                        message: format!(
                            "actions[{i}] 是 {op:?} 动作但 number 为空。取消/修改必须指明目标子单的看板编号（NB-<n>）。"
                        ),
                    });
                }
                if op == ReplanOp::Modify
                    && a.title.trim().is_empty()
                    && a.description.trim().is_empty()
                    && a.acceptance_criteria.trim().is_empty()
                {
                    return Err(ReplanParseError {
                        message: format!(
                            "actions[{i}] 是 modify 动作但 title/description/acceptance_criteria 全空。修改必须至少指明一个要改的字段。"
                        ),
                    });
                }
            }
            ReplanOp::Add => {
                if a.title.trim().is_empty() {
                    return Err(ReplanParseError {
                        message: format!(
                            "actions[{i}] 是 add 动作但 title 为空。新子单必须有自包含标题。"
                        ),
                    });
                }
                if !a.required_profession.trim().is_empty()
                    && let Some(e) = nemesis_prompts::professions::meta::validate_slug(
                        a.required_profession.trim(),
                    )
                    .err()
                {
                    return Err(ReplanParseError {
                        message: format!(
                            "actions[{i}] 的 required_profession=\"{}\" 格式非法：{e}。职能 slug 形如 dev、dev:cpp（小写字母/数字/连字符/下划线，至多一个冒号分段）；无职能需求填空串。",
                            a.required_profession.trim()
                        ),
                    });
                }
                for &dep in &a.depends_on {
                    if dep >= add_count {
                        return Err(ReplanParseError {
                            message: format!(
                                "actions[{i}] 的 depends_on 引用序号 {dep} 越界（本批 add 共 {add_count} 条，序号 0-{}）。depends_on 只能引用本批 add 动作的序号。",
                                add_count.saturating_sub(1)
                            ),
                        });
                    }
                }
            }
        }
    }

    // add 批自引用 + 环检测：把 add 动作按批内序重建成 PlannedSubIssue
    // 视图（depends_on 语义就是 add 批内序号，与 planner 同款），复用
    // planner DFS 单一真相源。先显式查自引用（planner 视图里拿不到
    // 「这是第几个 add」的动作下标语境）。
    for (k, a) in adds.iter().enumerate() {
        if a.depends_on.contains(&k) {
            return Err(ReplanParseError {
                message: format!(
                    "第 {k} 个 add 动作的 depends_on 引用了自身（{k}），不允许自引用。"
                ),
            });
        }
    }
    let add_plan: Vec<PlannedSubIssue> = adds
        .iter()
        .map(|a| PlannedSubIssue {
            title: a.title.clone(),
            description: a.description.clone(),
            required_role: String::new(),
            required_tags: Vec::new(),
            required_profession: a.required_profession.clone(),
            required_backend: String::new(),
            required_capabilities: Vec::new(),
            acceptance_criteria: a.acceptance_criteria.clone(),
            depends_on: a.depends_on.clone(),
        })
        .collect();
    if !add_plan.is_empty() {
        crate::planner::detect_cycle(&add_plan).map_err(|e| ReplanParseError {
            message: format!("add 批 depends_on 存在问题：{}", e.message),
        })?;
    }

    Ok(out)
}

/// 解析失败后的回灌重试提示词（把错误与原输出喂回，要求自纠）。
pub fn build_replan_retry_prompt(prev_output: &str, error: &ReplanParseError) -> String {
    let prev = crate::review::truncate_bytes(prev_output, 4000);
    format!(
        "你上一次的重规划建议无法通过校验：{}\n\n上一次输出：\n{prev}\n\n请修正后重新输出：只输出符合格式的 JSON 对象，不要任何其他文字或解释。",
        error.message
    )
}

/// 从建议输出构造 add 批的 [`PlannedSubIssue`] 列表（供执行臂直接喂
/// `confirm_plan` 单一入口）。depends_on 保持 add 批内序号（parse_replan
/// 已校验界内/无自引用/无环，语义与 planner 拆解批一致，confirm_plan
/// 原样消费）；required_role 缺省 worker、tags/backend/capabilities
/// 空——v1 重规划不引入新资源声明，需要时人工补。
pub fn replan_adds_as_planned(output: &ReplanOutput) -> Vec<PlannedSubIssue> {
    add_actions(output)
        .into_iter()
        .map(|a| PlannedSubIssue {
            title: a.title.trim().to_string(),
            description: a.description.trim().to_string(),
            required_role: "worker".to_string(),
            required_tags: Vec::new(),
            required_profession: a.required_profession.trim().to_string(),
            required_backend: String::new(),
            required_capabilities: Vec::new(),
            acceptance_criteria: a.acceptance_criteria.trim().to_string(),
            depends_on: a.depends_on.clone(),
        })
        .collect()
}

/// 建议评论渲染（结构化：分析 + 逐条动作 + 证据引用；版本号由调用方
/// 传入 = 既有建议评论计数 + 1）。assisted 审批与审计回查都按此形态读。
pub fn render_proposal_comment(
    version: u64,
    analysis: &str,
    actions: &[ReplanAction],
    trigger_note: &str,
) -> String {
    let mut s = format!("{PROPOSAL_COMMENT_PREFIX}计划 v{version}（{trigger_note}）\n");
    if !analysis.trim().is_empty() {
        s.push_str(&format!("\n**分析**：{}\n", analysis.trim()));
    }
    if actions.is_empty() {
        s.push_str("\n（无具体改动建议——证据不足以支持任何 action，请人工定夺方向。）\n");
        return s;
    }
    s.push_str("\n**建议动作**：\n");
    for (i, a) in actions.iter().enumerate() {
        let op_label = match a.op {
            Some(ReplanOp::Cancel) => "取消",
            Some(ReplanOp::Modify) => "修改",
            Some(ReplanOp::Add) => "新增",
            None => "?",
        };
        let target = if a.number.trim().is_empty() {
            String::new()
        } else {
            format!(" `{}`", a.number.trim())
        };
        let title = if a.title.trim().is_empty() {
            String::new()
        } else {
            format!("「{}」", a.title.trim())
        };
        s.push_str(&format!(
            "\n{}. **{}**{target}{title}\n   - 理由：{}\n   - 证据：{}\n",
            i + 1,
            op_label,
            if a.reason.trim().is_empty() {
                "（未填写）"
            } else {
                a.reason.trim()
            },
            a.evidence.trim(),
        ));
        if a.op == Some(ReplanOp::Add) {
            if !a.description.trim().is_empty() {
                s.push_str(&format!("   - 描述：{}\n", a.description.trim()));
            }
            if !a.acceptance_criteria.trim().is_empty() {
                s.push_str(&format!(
                    "   - 验收标准：{}\n",
                    a.acceptance_criteria.trim()
                ));
            }
            if !a.required_profession.trim().is_empty() {
                s.push_str(&format!("   - 职能：{}\n", a.required_profession.trim()));
            }
            if !a.depends_on.is_empty() {
                let deps = a
                    .depends_on
                    .iter()
                    .map(|d| d.to_string())
                    .collect::<Vec<_>>()
                    .join(",");
                s.push_str(&format!("   - 批内依赖：[{deps}]\n"));
            }
        }
    }
    s
}

#[cfg(test)]
mod tests;
