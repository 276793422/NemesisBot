//! Swarm M1：任务拆解 planner 纯逻辑层。
//!
//! 职责边界：本模块**不含任何 LLM 调用**——提示词模板、拆解输出 schema、
//! 解析与校验、回灌重试提示词都是纯函数（单测零依赖）；LLM 调用编排
//! （`run_detached` 裸提示词模式 + 失败回灌 ≤2 次）在 gateway 装配层。
//!
//! 拆解输出 schema 见 `docs/PLAN/2026-09-09_swarm-impl-plan.md` §3.1；
//! `depends_on` 用**批内序号**（0 起始）——LLM 无从预知落库 id，序号在
//! 落库时映射为真实 issue id（`issue_dependency` 表）。

use serde::{Deserialize, Serialize};

/// 单批拆解子单数硬上限（防 LLM 失控爆拆；提示词同步声明）。
pub const MAX_SUBISSUES: usize = 20;

/// planner 系统提示词（裸提示词模式的 system 段；经
/// `DetachedOpts.system_prompt` 注入，与主 agent 人格完全隔离）。
pub const PLANNER_SYSTEM_PROMPT: &str = r#"你是 NemesisBot 看板的任务拆解规划器（planner）。你的唯一职责是把一个父任务拆解为一组可独立执行、可独立验收的子任务。

# 输出格式（严格遵守）
只输出一个 JSON 数组，不要输出任何其他文字、解释或 markdown 代码围栏。数组元素形状：
[
  {
    "title": "子任务标题（一句话，动词开头，脱离上下文也能独立理解）",
    "description": "给执行者的完整说明：背景、目标、边界（明确不要做什么）、相关文件或位置线索",
    "required_role": "执行所需节点角色，如 worker；不确定填 worker",
    "required_tags": ["执行所需节点标签，如 rust、backend；没有就空数组"],
    "acceptance_criteria": "可客观检验的验收标准",
    "depends_on": [0]
  }
]
其中 depends_on 是依赖的本批内其他子任务的序号（0 起始）；无依赖用空数组。

# 拆解纪律
1. 单层拆解：子任务不再嵌套拆解。
2. 每个子任务必须能独立交付、独立验收；标题自包含。
3. depends_on 只允许引用本数组内的序号，且不得形成循环依赖。
4. 子任务总数不超过 20 个，3-7 个为佳；宁少勿滥。
5. 子任务之间有执行顺序要求（如先修编译再跑测试）用 depends_on 表达；相互独立则并行。"#;

/// planner 输出的单个子单（§3.1 schema；serde 宽容：缺字段用默认值）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlannedSubIssue {
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub required_role: String,
    #[serde(default)]
    pub required_tags: Vec<String>,
    #[serde(default)]
    pub acceptance_criteria: String,
    #[serde(default)]
    pub depends_on: Vec<usize>,
}

/// 解析失败的原因（`message` 直接回灌给 LLM，必须人读且指明怎么改）。
#[derive(Debug, Clone, PartialEq)]
pub struct PlanParseError {
    pub message: String,
}

impl std::fmt::Display for PlanParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// 解析 + 校验 planner 的 LLM 输出。
///
/// 容忍：markdown 代码围栏（```json … ```）、前后杂散文字（取第一个
/// `[` 到最后一个 `]`）。校验：非空、数量上限、title 非空、depends_on
/// 界内、不自引用、无循环依赖。
pub fn parse_plan(raw: &str) -> Result<Vec<PlannedSubIssue>, PlanParseError> {
    let json_text = extract_json_array(raw).ok_or_else(|| PlanParseError {
        message: "输出中找不到 JSON 数组（应以 '[' 开头、']' 结尾）。请只输出 JSON 数组本身。"
            .to_string(),
    })?;

    let plan: Vec<PlannedSubIssue> = serde_json::from_str(&json_text).map_err(|e| {
        PlanParseError {
            message: format!("JSON 解析失败：{e}。请检查引号/逗号/字段类型，只输出 JSON 数组。"),
        }
    })?;

    if plan.is_empty() {
        return Err(PlanParseError {
            message: "拆解结果为空数组。请至少拆解出 1 个子任务。".to_string(),
        });
    }
    if plan.len() > MAX_SUBISSUES {
        return Err(PlanParseError {
            message: format!(
                "子任务数量 {} 超过上限 {MAX_SUBISSUES}。请合并粒度、减少数量后重新输出。",
                plan.len()
            ),
        });
    }
    for (i, sub) in plan.iter().enumerate() {
        if sub.title.trim().is_empty() {
            return Err(PlanParseError {
                message: format!("第 {i} 个子任务的 title 为空。每个子任务都必须有非空标题。"),
            });
        }
        for &dep in &sub.depends_on {
            if dep >= plan.len() {
                return Err(PlanParseError {
                    message: format!(
                        "第 {i} 个子任务的 depends_on 引用了序号 {dep}，但本批只有 {} 个子任务（序号 0-{}）。",
                        plan.len(),
                        plan.len() - 1
                    ),
                });
            }
            if dep == i {
                return Err(PlanParseError {
                    message: format!("第 {i} 个子任务的 depends_on 包含自身（{dep}），不允许自引用。"),
                });
            }
        }
    }
    detect_cycle(&plan)?;

    Ok(plan)
}

/// 从 LLM 原始输出中提取 JSON 数组文本：剥 ``` 围栏、丢弃围栏外文字。
fn extract_json_array(raw: &str) -> Option<String> {
    let start = raw.find('[')?;
    let end = raw.rfind(']')?;
    if end < start {
        return None;
    }
    Some(raw[start..=end].to_string())
}

/// 批内依赖环检测（DFS 三色标记；发现环指明成员，便于回灌自纠）。
fn detect_cycle(plan: &[PlannedSubIssue]) -> Result<(), PlanParseError> {
    // 0=未访问 1=在栈 2=已完成
    let mut color = vec![0u8; plan.len()];
    let mut stack: Vec<usize> = Vec::new();

    fn visit(
        i: usize,
        plan: &[PlannedSubIssue],
        color: &mut [u8],
        stack: &mut Vec<usize>,
    ) -> Result<(), PlanParseError> {
        match color[i] {
            2 => return Ok(()),
            1 => {
                let cycle_start = stack.iter().position(|&s| s == i).unwrap_or(0);
                let members: Vec<String> = stack[cycle_start..]
                    .iter()
                    .chain(std::iter::once(&i))
                    .map(|&s| s.to_string())
                    .collect();
                return Err(PlanParseError {
                    message: format!(
                        "depends_on 存在循环依赖：{}。请打断环路后重新输出。",
                        members.join(" -> ")
                    ),
                });
            }
            _ => {}
        }
        color[i] = 1;
        stack.push(i);
        for &dep in &plan[i].depends_on {
            visit(dep, plan, color, stack)?;
        }
        stack.pop();
        color[i] = 2;
        Ok(())
    }

    for i in 0..plan.len() {
        visit(i, plan, &mut color, &mut stack)?;
    }
    Ok(())
}

/// 拆解用户提示词（user 段）：父任务上下文 + 可选团队经验注入槽
/// （M4.5 集体记忆接线点；空切片 = 不渲染该段）。
pub fn build_planner_user_prompt(
    title: &str,
    description: &str,
    acceptance_criteria: Option<&str>,
    team_experience: &[String],
) -> String {
    let mut prompt = String::new();
    prompt.push_str("# 父任务\n");
    prompt.push_str(&format!("标题：{title}\n"));
    prompt.push_str(&format!(
        "描述：\n{}\n",
        if description.trim().is_empty() {
            "（未提供）"
        } else {
            description
        }
    ));
    prompt.push_str(&format!(
        "\n# 整体验收标准\n{}\n",
        acceptance_criteria
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("（未提供）")
    ));
    if !team_experience.is_empty() {
        prompt.push_str("\n# 团队经验（历史沉淀，拆解时参考）\n");
        for exp in team_experience {
            prompt.push_str(&format!("- {exp}\n"));
        }
    }
    prompt.push_str("\n请拆解上述父任务，只输出 JSON 数组。");
    prompt
}

/// 解析失败后的回灌重试提示词（把错误与原输出喂回，要求自纠）。
pub fn build_retry_prompt(prev_output: &str, error: &PlanParseError) -> String {
    let prev = if prev_output.len() > 4000 {
        format!("{}…（已截断）", &prev_output[..4000])
    } else {
        prev_output.to_string()
    };
    format!(
        "你上一次的输出无法通过校验：{}\n\n上一次输出：\n{prev}\n\n请修正后重新输出：只输出符合格式的 JSON 数组，不要任何其他文字或解释。",
        error.message
    )
}

#[cfg(test)]
mod tests;
