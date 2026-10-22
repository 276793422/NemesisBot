//! 循环流程策略 trait 层（WASM 样板工程批 1：Rust trait 化）。
//!
//! 切分原则见盘点文档 `docs/INFO/2026-10-22_agent-loop外部定制点全景盘点.md`：
//! **骨架**（安全 8 层调用点 / estop / 审计链 / checkpoint 协议 / 量化上限）
//! 不可替换；**流程策略**（压缩、终答裁决、参数校验、标题生成）经本层依赖
//! 倒置——宿主在固定策略点调槽，槽内实现可替换。
//!
//! ## 哨兵 Builtin 模式（批 1 的核心设计）
//!
//! 三个策略槽恒持一个实现，默认 = **零逻辑哨兵**（裁决恒返 `Builtin`，
//! 宿主走原有代码路径）。既有逻辑**全部留在宿主**：组件给出显式裁决时，
//! 宿主对组件裁决执行与内置裁决完全相同的分支代码（canvas 扫描 / steer
//! escape / turn-end hooks / Done 发射等骨架副作用不外泄）。这解决了
//! 「Builtin 实现拿不到每轮状态（`TurnGuard` 等）」的状态捕获问题——trait
//! 入参只是纯数据投影，真正有状态的判定留在宿主。
//!
//! ## 二期 WASM 对接边界（批 2-4）
//!
//! 本层 trait 签名 = 对外合同雏形；JSON 投影（截断 / 脱敏 / 序列化）在桥
//! 边界（PolicyBridge）做，trait 本身保持内部工效学（引用传递零克隆）。
//! 标题回调是**单点回调形态对照**（D5）：不做 WASM 化，`Option` 槽
//! （`None` = 内置小模型路径）——哨兵模式对它不成立，因为内置实现依赖
//! 宿主侧 `small_model` 资源，零逻辑哨兵无法自持。

use serde_json::Value;

// ---------------------------------------------------------------------------
// FinalJudgePolicy —— 终答裁决（盘点 #10）
// ---------------------------------------------------------------------------

/// 终答裁决入参（纯数据投影；宿主在 heartbeat 特例与 tool_calls 分流**之后**
/// 才调——能到这里的就是「无工具调用的候选终答」）。
pub struct FinalJudgeInput {
    pub session_key: String,
    /// 候选终答全文。Rust 层零截断；WASM 桥按宿主 config 投影。
    pub content: String,
    /// 本轮已消耗的 LLM 轮数（`TurnState.turns_used`）。
    pub turns_used: u32,
    /// 连续参数校验失败计数（`TurnState.validation_failures`）。
    pub validation_failures: u32,
}

/// 终答裁决。宿主对每种裁决执行与内置判定完全相同的分支逻辑
/// （Accept = canvas/hooks/Done 全套收尾；RetryWithNudge = nudge 回灌；
/// GiveUp = 兜底文案收轮）。
#[derive(Debug)]
pub enum FinalJudgeVerdict {
    /// 接受为最终答案。
    Accept,
    /// 拒绝并带提示回灌，让模型重试一次。
    RetryWithNudge(String),
    /// 重试预算耗尽——以此文案收轮。
    GiveUp(String),
    /// 单次显式回退宿主内置判定（`TurnGuard::check_final_answer`）。
    Builtin,
}

/// 终答裁决策略（盘点 #10；吸收域 #11 状态治理仍留宿主 `TurnGuard`）。
#[async_trait::async_trait]
pub trait FinalJudgePolicy: Send + Sync {
    async fn judge(&self, input: &FinalJudgeInput) -> FinalJudgeVerdict;
}

/// 零逻辑哨兵：恒返 [`FinalJudgeVerdict::Builtin`]（行为 = 无策略槽）。
pub struct BuiltinFinalJudgePolicy;

#[async_trait::async_trait]
impl FinalJudgePolicy for BuiltinFinalJudgePolicy {
    async fn judge(&self, _input: &FinalJudgeInput) -> FinalJudgeVerdict {
        FinalJudgeVerdict::Builtin
    }
}

// ---------------------------------------------------------------------------
// ValidationPolicy —— 工具参数校验（盘点 #14）
// ---------------------------------------------------------------------------

/// 参数校验入参。`schema: None` = 工具未声明 schema（内置语义直接 Valid，
/// 未知工具的报错归 `handle_tool_call`）——组件仍会看到该调用，可自定义
/// 强校验，但 `Builtin` 裁决维持内置行为。
pub struct ValidationInput {
    pub tool: String,
    pub schema: Option<Value>,
    /// 原始 args JSON 串（未解析——内置 autofix 直接消费原文）。
    pub args_json: String,
}

/// 校验裁决。宿主映射到 [`crate::args_validator::Outcome`]：
/// `Valid`/`Fixed` 直通；`Reject` 按 schema violation 类（class `"B"`）回灌
/// 模型并计入 validation_failures（遥测类暂复用 `"B"`，专用类随 WASM
/// 合同批引入）。
#[derive(Debug)]
pub enum ValidationVerdict {
    Valid,
    /// 携带修复后的 args JSON（喂给工具执行）。
    Fixed(String),
    Reject {
        message: String,
    },
    /// 单次显式回退内置校验（schema 校验 + 编辑距离 autofix）。
    Builtin,
}

/// 工具参数校验策略（盘点 #14；串行 / 并行两条执行路径共用同一槽）。
#[async_trait::async_trait]
pub trait ValidationPolicy: Send + Sync {
    async fn validate(&self, input: &ValidationInput) -> ValidationVerdict;
}

/// 零逻辑哨兵：恒返 [`ValidationVerdict::Builtin`]。
pub struct BuiltinValidationPolicy;

#[async_trait::async_trait]
impl ValidationPolicy for BuiltinValidationPolicy {
    async fn validate(&self, _input: &ValidationInput) -> ValidationVerdict {
        ValidationVerdict::Builtin
    }
}

// ---------------------------------------------------------------------------
// CompactPolicy —— 上下文压缩（盘点 #17/#18；触发 + 执行两刀）
// ---------------------------------------------------------------------------

/// 压缩触发入参（混合信号投影；宿主在 stuck 闩 / soft 记账 / summarizing
/// 互斥等状态治理**之前**调——组件只决定「压不压」，状态治理照走原路径）。
pub struct CompactTriggerInput {
    pub session_key: String,
    /// 模型侧投影尾巴的 token 估算。
    pub tail_tokens: usize,
    /// 上一轮 provider 实报的整请求输入 token（0 = 无 usage 回落）。
    pub provider_signal: usize,
    /// 混合信号 = max(tail, provider)——偏早触发（安全侧）。
    pub signal: usize,
    /// 有效压缩点（token）。
    pub threshold: usize,
    /// 软位（压缩点 × 2/3，仅记账用）。
    pub soft: usize,
    /// 真实上下文窗口（三级链解析值）。
    pub window: usize,
    /// 当前摘要覆盖边界 `c`（0 = 无摘要）。
    pub covers_up_to: usize,
    /// 新折叠边界 `new_c`（P5 token 预算 + 工具对齐回退后）。
    pub new_boundary: usize,
}

/// 触发裁决。`Builtin` = 保留内置式（`signal >= threshold && new > covers`）；
/// `Compact` = 强制压缩；`Skip` = 本轮跳过（软位/闩记账仍按信号走）。
#[derive(Debug)]
pub enum CompactTriggerVerdict {
    Compact,
    Skip,
    Builtin,
}

/// 压缩执行入参（引用传递零克隆；WASM 桥在边界做截断投影）。
pub struct CompactExecInput<'a> {
    pub session_key: &'a str,
    /// 待折叠前缀 `history[..new_c]`（含旧摘要覆盖区间——内置语义每次从
    /// 源头全量重摘，不增量）。
    pub prefix: &'a [crate::types::ConversationTurn],
    /// 既有摘要（空串 = 首次压缩）。
    pub existing_summary: &'a str,
    /// 真实窗口（summarize 的 max_msg_tokens 守卫用）。
    pub window: usize,
    /// 当前活跃模型名。
    pub model: &'a str,
}

/// 执行裁决。`Summary` = 采用该文本（宿主照跑无进展守卫 B2 / covers 推进 /
/// 通知 / hooks 全套）；`Skip` = 本轮不出摘要（同内置「无可摘要内容」路径：
/// loud warn + covers 不推进）；`Builtin` = 内置 provider 摘要链。
#[derive(Debug)]
pub enum CompactExecVerdict {
    Summary(String),
    Skip,
    Builtin,
}

/// 上下文压缩策略（盘点 #17/#18：触发信号 + 摘要执行两刀）。
#[async_trait::async_trait]
pub trait CompactPolicy: Send + Sync {
    async fn should_compact(&self, input: &CompactTriggerInput) -> CompactTriggerVerdict;

    async fn generate_summary(&self, input: &CompactExecInput<'_>) -> CompactExecVerdict;
}

/// 零逻辑哨兵：两刀恒返 `Builtin`。
pub struct BuiltinCompactPolicy;

#[async_trait::async_trait]
impl CompactPolicy for BuiltinCompactPolicy {
    async fn should_compact(&self, _input: &CompactTriggerInput) -> CompactTriggerVerdict {
        CompactTriggerVerdict::Builtin
    }

    async fn generate_summary(&self, _input: &CompactExecInput<'_>) -> CompactExecVerdict {
        CompactExecVerdict::Builtin
    }
}

// ---------------------------------------------------------------------------
// TitleCallback —— 会话标题生成（盘点 #20；D5 单点回调形态对照）
// ---------------------------------------------------------------------------

/// 标题生成入参。
pub struct TitleInput {
    pub session_key: String,
    /// 首条 user 消息（宿主已按 `E7_TITLE_INPUT_MAX_CHARS` 截断）。
    pub first_user_message: String,
}

/// 标题生成回调（D5：单点回调形态对照样例——不做 WASM 化）。
///
/// 与三策略槽的**哨兵模式不同**，本槽是 `Option`（`None` = 内置小模型
/// 路径）：内置实现依赖宿主侧 `small_model` 资源，零逻辑哨兵无法自持；
/// 且标题生成是整函数接管（无「每调用回退」语义）。
#[async_trait::async_trait]
pub trait TitleCallback: Send + Sync {
    /// 返回 `None` = 放弃（宿主跳过写入，行为同内置生成失败）。
    async fn generate_title(&self, input: &TitleInput) -> Option<String>;
}
