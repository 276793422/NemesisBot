//! 提示词包引擎门面（prompt pack facade）。
//!
//! 文本资产的单一真相源在 [`nemesis_prompts`] crate（独立零依赖，目录
//! `crates/nemesis-prompts/`，治理规范见其 README.md）。本模块只保留与
//! nemesis-agent / nemesis-config 类型绑定的引擎件：
//!
//! - [`PromptSystem`]：体系开关（pro/classic，解析 config 键）；
//! - tier→描述档位映射与查表回落（[`tool_description`]）；
//! - 组装预算告警（[`check_budget`]，tracing 消费点）。
//!
//! 全部文本符号经此处 re-export，crate 内调用方路径（`crate::prompt::X`）
//! 与既有代码保持不变。
//!
//! 设计约束（与资产 crate 一致）：
//! - 组装确定性：同一层级永远按注册表顺序渲染，同输入字节级一致（单测
//!   钉死）。
//! - 长度软预算：超限告警不截断——预算是治理信号，静默截断会丢规则，比
//!   超预算更危险。
//! - 全中文铁律：段落与描述文案一律简体中文；工具名、参数名、代码标识、
//!   路径保持英文原文。

use nemesis_types::capability::ModelTier;

// ---------------------------------------------------------------------------
// 文本资产 re-export（单一真相源在 nemesis-prompts）
// ---------------------------------------------------------------------------

pub use nemesis_prompts::aux::{
    FILE_LEDGER_HEADING, SUMMARY_SCHEMA_SECTIONS, render_external_channel_section,
    render_paste_data_section, render_summary_instruction, render_summary_merge_prompt,
    render_summary_schema_suffix, render_title_prompt,
};
pub use nemesis_prompts::board::{PrecedentEntry, ReviewTier, parse_review_tier};
pub use nemesis_prompts::slash;
pub use nemesis_prompts::subagents::SubagentRole;
pub use nemesis_prompts::system::{
    Entrance, Layer, SOFT_BUDGET_BYTES, render_layer, render_layer_for,
};
pub use nemesis_prompts::tools::{DescLevel, description_for, first_sentence};

// ---------------------------------------------------------------------------
// 体系开关
// ---------------------------------------------------------------------------

/// 提示词体系选择（`agents.prompt_system`）。
///
/// 启动装配型：system prompt 在启动时构建后随会话冻结，运行时改键需重启
/// （或重建实例）生效。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PromptSystem {
    /// 段落池组装的新体系（缺省）：前置段 → 人格文件 → 行为段池 → 环境段。
    #[default]
    Pro,
    /// 既有装配路径：人格文件拼接 + 固定尾巴（字节级不变）。
    Classic,
}

impl PromptSystem {
    /// 解析 config `agents.prompt_system` 原始字符串。
    ///
    /// 空串 = 未配置（含 `Default` 派生出的空串）→ [`PromptSystem::Pro`]；
    /// 未知非空值 → warn + [`PromptSystem::Classic`]（fail-safe 到已知旧行为，
    /// 宁可保守不猜新体系）。
    pub fn parse(raw: &str) -> Self {
        match raw.trim() {
            "" | "pro" => Self::Pro,
            "classic" => Self::Classic,
            other => {
                tracing::warn!(
                    "[prompt] 未知 agents.prompt_system 值 {:?}，按 classic 处理（可选值：pro | classic）",
                    other
                );
                Self::Classic
            }
        }
    }

    /// 从 `Config` 取键并解析的便捷入口（ factory 侧避免手写 trim 分派）。
    pub fn from_config(cfg: &nemesis_config::AgentsConfig) -> Self {
        Self::parse(&cfg.prompt_system)
    }
}

// ---------------------------------------------------------------------------
// 长度预算告警（tracing 消费点；预算常量在资产 crate）
// ---------------------------------------------------------------------------

/// 组装完成后调用；超软预算时 `tracing::warn!`（不截断、不拒绝）。
pub fn check_budget(total_bytes: usize) {
    if total_bytes > SOFT_BUDGET_BYTES {
        tracing::warn!(
            "[prompt] pro 模式 system prompt 共 {} 字节，超过软预算 {} 字节（检查段落池或人格文件是否失控）",
            total_bytes,
            SOFT_BUDGET_BYTES
        );
    }
}

// ---------------------------------------------------------------------------
// 工具描述：tier → 档位映射 + 查表回落
// ---------------------------------------------------------------------------

/// 模型能力档位 → 描述档位。mini 恒用 lean（长描述干扰小模型选型，既有
/// 结论）；normal/big 优先 full（查表未命中自然回落 lean）。
fn desc_level_for(tier: ModelTier) -> DescLevel {
    match tier {
        ModelTier::Mini => DescLevel::Lean,
        // Auto 理论上在构造期已 resolve；防御性按 normal 口径。
        ModelTier::Auto | ModelTier::Normal | ModelTier::Big => DescLevel::Full,
    }
}

/// 工具描述取值单点：先查档位表，未命中回落注册表原文。
///
/// `fallback` 永远是注册表的 `tool.description()`；`tier` 为当前生效档位。
/// schema 一律不走这里（保持注册表原文，prompt cache 与 args_validator
/// 兼容都依赖这一点）。
pub fn tool_description<'a>(name: &str, fallback: &'a str, tier: ModelTier) -> &'a str {
    description_for(name, desc_level_for(tier)).unwrap_or(fallback)
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
