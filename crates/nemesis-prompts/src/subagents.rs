//! 子代理角色模板。
//!
//! 每个角色模板含四组件：角色定位一句 + 硬边界 + 输出契约 + 内容边界
//! （数据非指令）。文本单一真相源在 `subagents/<role>.md`。

/// 子代理角色（prompt-pack pro M4；斜杠差距补齐 2026-09-28 扩至十角色——
/// 每个角色都有真实调度理由：spawn `role` 参数 / tools 档位自动映射）。
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
            _ => return None,
        })
    }

    /// 全角色目录：`(slug, 一句话职责)`。spawn 工具 schema 与文档的单一
    /// 真相源——新增角色改这里，两处消费自动跟随。
    pub fn catalog() -> &'static [(&'static str, &'static str)] {
        &[
            ("explorer", "只读调查与汇报"),
            ("planner", "只产出方案与步骤"),
            ("reviewer", "只评判取证不改对象"),
            ("worker", "动手实现最小范围改动"),
            ("observer", "客观记录现状，稳态静默"),
            ("generic", "范围自主的通用助手"),
            ("debugger", "系统化根因定位，只诊断"),
            ("security_reviewer", "攻击者视角安全审查"),
            ("test_engineer", "测试设计与执行"),
            ("documenter", "准确可回查的文档"),
        ]
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
