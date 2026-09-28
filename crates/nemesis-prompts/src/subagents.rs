//! 子代理角色模板。
//!
//! 每个角色模板含四组件：角色定位一句 + 硬边界 + 输出契约 + 内容边界
//! （数据非指令）。文本单一真相源在 `subagents/<role>.md`。

/// 子代理角色（prompt-pack pro M4；五差距补齐 2026-09-28 扩至十角色；
/// 角色目录与分档供给 2026-09-28 扩至十七角色——每个角色都有真实调度
/// 理由：spawn `role` 参数 / chat.spawn / workflow 节点 / tools 档位映射）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubagentRole {
    /// 侦察员：只读调查与汇报（spawn `readonly` 档的默认映射）。
    Explorer,
    /// 规划员：只产出方案与步骤，不实现。
    Planner,
    /// 评审员：只评判取证，不修改被审对象。
    Reviewer,
    /// 执行员：动手实现最小范围改动。
    Worker,
    /// 观察员：客观记录现状，不评判不干预；预期稳态是静默。
    Observer,
    /// 通用子代理：范围自主 + 忠实汇报的轻量约束。
    Generic,
    /// 排障员：系统化根因定位，只诊断不修复。
    Debugger,
    /// 安全评审员：攻击者视角审查，分级发现，不做修改。
    SecurityReviewer,
    /// 测试工程师：测试设计与执行，用结果说话，不改被测实现。
    TestEngineer,
    /// 文档工程师：准确可回查的文档，不虚构。
    Documenter,
    /// 网页阅读员：只取所需页 + 网页内容注入防御 + 转述不转发。
    WebReader,
    /// 编排员：会话内多子代理拆派收汇，只编排不亲自实现。
    Coordinator,
    /// 工作流执行员：只执行本节点指令，输出完整自包含。
    WorkflowExecutor,
    /// 记忆提取员：只提炼不改事实，凭据不进记忆。
    MemoryExtractor,
    /// 轻量问答员：一问一答即停，不打断主代理（配 `agents.small_model`）。
    Qa,
    /// 分叉执行员：继承父上下文是参考非处境，一指令即停。
    Fork,
    /// 测试执行员：只跑测试读结果，不改产品也不改测试凑绿。
    TestRunner,
}

/// 角色供给分档（`min_tier` 线格式；镜像 nemesis-types 的 ModelTier
/// mini/normal/big——本 crate 零依赖不引它，字符串口径单一真相源在此，
/// 消费方（dispatch 闸 / roles.list）负责把当前 ModelTier 映射成同口径）。
///
/// 设计裁决（2026-09-28 用户重评）：目录完整性是硬指标，小模型适配是
/// 供给层的事——目录全量存在，`roles_visible_to` 按档过滤供给；mini
/// 用户看得少但目录不缺。
pub mod role_tier {
    /// 全档可见（mini 及以上）。
    pub const MINI: &str = "mini";
    /// normal 及以上可见。
    pub const NORMAL: &str = "normal";
    /// 仅 big 可见。
    pub const BIG: &str = "big";

    /// 分档序（mini=0 < normal=1 < big=2）；未知值按 0 处理（fail-open
    /// 到最宽档——分档是供给优化不是安全闸，宁可多给不可误杀）。
    pub fn rank(tier: &str) -> u8 {
        match tier {
            "mini" => 0,
            "normal" => 1,
            "big" => 2,
            _ => 0,
        }
    }
}

impl SubagentRole {
    /// 角色模板正文（编译期嵌入）。
    pub fn template(&self) -> &'static str {
        match self {
            Self::Explorer => include_str!("subagents/explorer.md"),
            Self::Planner => include_str!("subagents/planner.md"),
            Self::Reviewer => include_str!("subagents/reviewer.md"),
            Self::Worker => include_str!("subagents/worker.md"),
            Self::Observer => include_str!("subagents/observer.md"),
            Self::Generic => include_str!("subagents/generic.md"),
            Self::Debugger => include_str!("subagents/debugger.md"),
            Self::SecurityReviewer => include_str!("subagents/security_reviewer.md"),
            Self::TestEngineer => include_str!("subagents/test_engineer.md"),
            Self::Documenter => include_str!("subagents/documenter.md"),
            Self::WebReader => include_str!("subagents/web_reader.md"),
            Self::Coordinator => include_str!("subagents/coordinator.md"),
            Self::WorkflowExecutor => include_str!("subagents/workflow_executor.md"),
            Self::MemoryExtractor => include_str!("subagents/memory_extractor.md"),
            Self::Qa => include_str!("subagents/qa.md"),
            Self::Fork => include_str!("subagents/fork.md"),
            Self::TestRunner => include_str!("subagents/test_runner.md"),
        }
    }

    /// 角色 slug（spawn `role` 参数的线格式；snake_case）。
    pub fn slug(&self) -> &'static str {
        match self {
            Self::Explorer => "explorer",
            Self::Planner => "planner",
            Self::Reviewer => "reviewer",
            Self::Worker => "worker",
            Self::Observer => "observer",
            Self::Generic => "generic",
            Self::Debugger => "debugger",
            Self::SecurityReviewer => "security_reviewer",
            Self::TestEngineer => "test_engineer",
            Self::Documenter => "documenter",
            Self::WebReader => "web_reader",
            Self::Coordinator => "coordinator",
            Self::WorkflowExecutor => "workflow_executor",
            Self::MemoryExtractor => "memory_extractor",
            Self::Qa => "qa",
            Self::Fork => "fork",
            Self::TestRunner => "test_runner",
        }
    }

    /// slug → 角色（spawn `role` 参数解析；未知值 `None`，调用方诚实拒绝）。
    pub fn from_slug(s: &str) -> Option<Self> {
        Some(match s {
            "explorer" => Self::Explorer,
            "planner" => Self::Planner,
            "reviewer" => Self::Reviewer,
            "worker" => Self::Worker,
            "observer" => Self::Observer,
            "generic" => Self::Generic,
            "debugger" => Self::Debugger,
            "security_reviewer" => Self::SecurityReviewer,
            "test_engineer" => Self::TestEngineer,
            "documenter" => Self::Documenter,
            "web_reader" => Self::WebReader,
            "coordinator" => Self::Coordinator,
            "workflow_executor" => Self::WorkflowExecutor,
            "memory_extractor" => Self::MemoryExtractor,
            "qa" => Self::Qa,
            "fork" => Self::Fork,
            "test_runner" => Self::TestRunner,
            _ => return None,
        })
    }

    /// 全角色目录：`(slug, 一句话职责, min_tier)`。spawn 工具 schema、
    /// dispatch 分档闸、`roles.list` 的单一真相源——新增角色改这里，
    /// 三处消费自动跟随。
    pub fn catalog() -> &'static [(&'static str, &'static str, &'static str)] {
        &[
            ("explorer", "只读调查与汇报", role_tier::MINI),
            ("planner", "只产出方案与步骤", role_tier::NORMAL),
            ("reviewer", "只评判取证不改对象", role_tier::MINI),
            ("worker", "动手实现最小范围改动", role_tier::MINI),
            ("observer", "客观记录现状，稳态静默", role_tier::NORMAL),
            ("generic", "范围自主的通用助手", role_tier::MINI),
            ("debugger", "系统化根因定位，只诊断", role_tier::NORMAL),
            ("security_reviewer", "攻击者视角安全审查", role_tier::BIG),
            ("test_engineer", "测试设计与执行", role_tier::NORMAL),
            ("documenter", "准确可回查的文档", role_tier::NORMAL),
            ("web_reader", "网页阅读与转述", role_tier::NORMAL),
            ("coordinator", "多子代理拆派收汇", role_tier::BIG),
            ("workflow_executor", "工作流节点执行", role_tier::BIG),
            ("memory_extractor", "记忆提炼不改事实", role_tier::NORMAL),
            ("qa", "轻量问答一问一答", role_tier::MINI),
            ("fork", "继承上下文一指令即停", role_tier::BIG),
            ("test_runner", "跑测试读结果报真相", role_tier::NORMAL),
        ]
    }

    /// 分档供给：当前 tier（"mini"/"normal"/"big"）可见的角色 slug 集合
    /// （`min_tier` rank ≤ 当前 tier rank 的目录子集，保持目录顺序）。
    /// dispatch 闸、schema 枚举、`roles.list` 三处共用——与
    /// `tier_allowed_tools` 同构的供给过滤（目录全量，供给分档）。
    /// 未知 tier 按最宽档处理（与 `resolve_active_tier` 缺省 big 同一
    /// 哲学：分档是供给优化不是安全闸，宁可多给不可误杀）。
    pub fn roles_visible_to(tier: &str) -> Vec<&'static str> {
        let cur = if matches!(tier, role_tier::MINI | role_tier::NORMAL | role_tier::BIG) {
            role_tier::rank(tier)
        } else {
            role_tier::rank(role_tier::BIG)
        };
        Self::catalog()
            .iter()
            .filter(|(_, _, min)| role_tier::rank(min) <= cur)
            .map(|(slug, _, _)| *slug)
            .collect()
    }

    /// 渲染子代理 system prompt：主人格在前、角色段叠加在后（主人格继续
    /// 适用）。主人格为空时角色段独立成 prompt（standalone loop 场景）。
    pub fn render_system_prompt(&self, base_persona: Option<&str>) -> String {
        match base_persona {
            Some(base) if !base.trim().is_empty() => {
                format!("{}\n\n---\n\n{}", base.trim_end(), self.template())
            }
            _ => self.template().to_string(),
        }
    }
}
