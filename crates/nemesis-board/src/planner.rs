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
/// 其中 `[CHECK]` 锚点指令段与 `crate::anchor` 解析器同步演化
/// （同 `REPORT_FORMAT_SECTION` 契约；防误删快照见 planner/tests.rs）。
///
/// 文本单一真相源在 `nemesis-prompts`（M7 集中化）；此处 re-export 保持
/// 既有公开路径不变。
pub use nemesis_prompts::board::PLANNER_SYSTEM_PROMPT;

/// planner 系统提示词完整形态（基础契约 + 职能化拆解方法论；`&'static str`
/// 驻留缓存）。派发消费方（nemesis-web issue.plan）用本函数——直接用
/// [`PLANNER_SYSTEM_PROMPT`] 常量会缺职能方法论段。
pub use nemesis_prompts::board::planner_system_prompt;

/// planner 输出的单个子单（§3.1 schema；serde 宽容：缺字段用默认值）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PlannedSubIssue {
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub required_role: String,
    #[serde(default)]
    pub required_tags: Vec<String>,
    /// 集群专业职能框架（M2）：执行所需职能 slug（`family[:spec]`）；
    /// 空串 = 无职能需求。格式校验见 [`parse_plan`]（D9 三臂）。
    #[serde(default)]
    pub required_profession: String,
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

    let plan: Vec<PlannedSubIssue> =
        serde_json::from_str(&json_text).map_err(|e| PlanParseError {
            message: format!("JSON 解析失败：{e}。请检查引号/逗号/字段类型，只输出 JSON 数组。"),
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
        if let Some(err) = profession_format_error(i, &sub.required_profession) {
            return Err(err);
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
                    message: format!(
                        "第 {i} 个子任务的 depends_on 包含自身（{dep}），不允许自引用。"
                    ),
                });
            }
        }
    }
    detect_cycle(&plan)?;
    analyze_shared_touch(&plan)?;

    Ok(plan)
}

/// D9 校验臂内核：`required_profession` 非空时格式必须合法（slug 语法
/// `^[a-z0-9_-]+(:[a-z0-9_-]+)?$`）。返回 `Some(PlanParseError)` = 格式
/// 非法（挂进既有回灌重试）；格式合法但目录未知（用户自定义职能）=
/// `None` 保留派发（匹配端诚实找不到）。空串/空白 = 无职能需求。
fn profession_format_error(index: usize, raw: &str) -> Option<PlanParseError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    nemesis_prompts::professions::meta::validate_slug(trimmed).err().map(|e| PlanParseError {
        message: format!(
            "第 {index} 个子任务的 required_profession=\"{trimmed}\" 格式非法：{e}。职能 slug 形如 dev、dev:cpp（小写字母/数字/连字符/下划线，至多一个冒号分段）；无职能需求填空串。"
        ),
    })
}

/// 宽和解析结果（[`parse_plan_lenient`] 专用）：计划 + 降级清单。
#[derive(Debug, Clone, PartialEq)]
pub struct LenientPlan {
    pub plan: Vec<PlannedSubIssue>,
    /// 降级明细：`(子单序号, 原始非法值)`。落库层逐条写 issue 系统评论
    /// 「required_profession=`…` 非法，已降级无职能」+ WARN（D9）。
    pub downgraded: Vec<(usize, String)>,
}

/// D9 末轮宽和臂：与 [`parse_plan`] 同一解析与全部校验，唯一差别 =
/// `required_profession` 格式非法**不报错**——该子单职能置空照常收进
/// 计划，降级明细随结果返回。仅供回灌预算（首跑+≤2）耗尽的最后一轮
/// 使用；严格版 [`parse_plan`] 是常规路径。
pub fn parse_plan_lenient(raw: &str) -> Result<LenientPlan, PlanParseError> {
    let json_text = extract_json_array(raw).ok_or_else(|| PlanParseError {
        message: "输出中找不到 JSON 数组（应以 '[' 开头、']' 结尾）。请只输出 JSON 数组本身。"
            .to_string(),
    })?;

    let mut plan: Vec<PlannedSubIssue> =
        serde_json::from_str(&json_text).map_err(|e| PlanParseError {
            message: format!("JSON 解析失败：{e}。请检查引号/逗号/字段类型，只输出 JSON 数组。"),
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

    let mut downgraded: Vec<(usize, String)> = Vec::new();
    let sub_count = plan.len();
    for (i, sub) in plan.iter_mut().enumerate() {
        if sub.title.trim().is_empty() {
            return Err(PlanParseError {
                message: format!("第 {i} 个子任务的 title 为空。每个子任务都必须有非空标题。"),
            });
        }
        // 宽和臂唯一放宽点：职能格式非法 → 置空 + 记降级（其余字段照常严校）。
        if profession_format_error(i, &sub.required_profession).is_some() {
            downgraded.push((i, std::mem::take(&mut sub.required_profession)));
        }
        for &dep in &sub.depends_on {
            if dep >= sub_count {
                return Err(PlanParseError {
                    message: format!(
                        "第 {i} 个子任务的 depends_on 引用了序号 {dep}，但本批只有 {sub_count} 个子任务（序号 0-{}）。",
                        sub_count - 1
                    ),
                });
            }
            if dep == i {
                return Err(PlanParseError {
                    message: format!(
                        "第 {i} 个子任务的 depends_on 包含自身（{dep}），不允许自引用。"
                    ),
                });
            }
        }
    }
    detect_cycle(&plan)?;
    analyze_shared_touch(&plan)?;

    Ok(LenientPlan { plan, downgraded })
}

/// E6 共享文件分析（看板项目档案 goal，拆解期第 1 层冲突防线）：
/// [TOUCH] 声明聚合——两个子任务声明写同一路径且**无直接依赖边**
/// （互不 depends_on）= 并行改同一文件，回传合并必然冲突 → 校验失败
/// 回灌重试（复用既有 PLAN_BAD 机制与重试预算）。有依赖边=串行执行，
/// 后者拿到的基线已含前者的合入，不拦。路径匹配与调度层 [TOUCH] 互斥
/// 同口径：trim 后精确相等。
fn analyze_shared_touch(plan: &[PlannedSubIssue]) -> Result<(), PlanParseError> {
    // path -> 声明它的子任务序号列表。
    let mut owners: std::collections::BTreeMap<String, Vec<usize>> = Default::default();
    for (i, sub) in plan.iter().enumerate() {
        for p in parse_touch_paths(&sub.acceptance_criteria) {
            owners.entry(p).or_default().push(i);
        }
    }
    let mut violations: Vec<String> = Vec::new();
    for (path, subs) in &owners {
        if subs.len() < 2 {
            continue;
        }
        for (a_idx, &a) in subs.iter().enumerate() {
            for &b in subs.iter().skip(a_idx + 1) {
                let has_edge = plan[a].depends_on.contains(&b) || plan[b].depends_on.contains(&a);
                if !has_edge {
                    violations.push(format!(
                        "第 {a} 和第 {b} 个子任务都声明写「{path}」但没有依赖边（并行执行会在合并时冲突）"
                    ));
                }
            }
        }
    }
    if violations.is_empty() {
        return Ok(());
    }
    Err(PlanParseError {
        message: format!(
            "共享文件冲突：{}。请修正后重新输出：给冲突双方之一加 depends_on 串行化、或合并为一个子任务、或修正 [TOUCH] 声明使其准确反映各自实际写入范围。",
            violations.join("；")
        ),
    })
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
    cluster_profile: Option<&str>,
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
    // R-10（goal P4）：集群画像注入——拆解即按可执行者拆（技术选型/role/
    // required_tags 与真实节点能力对齐，防「拆出无人能执行的任务」）。
    if let Some(profile) = cluster_profile
        && !profile.trim().is_empty()
    {
        prompt.push_str(&format!(
            "\n# 可用执行节点（集群画像——拆解必须与此对齐）\n{profile}\n\
             约束：required_tags 只能从上述节点的 tags/category 中选；技术选型\
             （语言/运行时/工具）必须落在节点具备的能力内；每个子单的描述里\
             写明运行环境假设。\n"
        ));
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
mod cov_tests;
#[cfg(test)]
mod tests;

/// R-9（goal P4）：解析 acceptance_criteria 里的 `[TOUCH] <路径>` 行——
/// 子单写资源声明，调度互斥与交付回传清单的素材。非 [TOUCH] 行忽略；
/// 路径 trim 后去空；相对路径形态不做强校验（宽容解析）。
pub fn parse_touch_paths(acceptance_criteria: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in acceptance_criteria.lines() {
        let t = line.trim();
        if let Some(p) = t.strip_prefix("[TOUCH]") {
            let p = p.trim();
            if !p.is_empty() && !out.iter().any(|x| x == p) {
                out.push(p.to_string());
            }
        }
    }
    out
}
