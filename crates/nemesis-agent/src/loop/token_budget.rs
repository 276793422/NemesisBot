//! P8 会话 token 预算闸（集群无人值守硬化 A2）——`impl AgentLoop` 接线。
//!
//! 纯类型/账本/文案在 [`crate::token_budget`]（顶层 pub 模块：marker 常量
//! 供 nemesisbot `classify_terminal_failure` 与 board_review 单一真相源）；
//! 本文件只是 glue——私有字段访问需要 `r#loop` 模块后代身份，与 fs_watch/
//! config_watch 等同居 `loop/` 的理由一致。
//!
//! 干预链四处落点：
//! 1. 每轮记账：`record_round_usage` 末尾（observer.rs）；
//! 2. 轮顶检：run_loop 骨架 `prepare_round` 块之后——hard ≥100% 终局 /
//!    soft ≥75% 一次性提示 + 压缩；
//! 3. 观测：`session_context_status`（compact.rs）加 `session_tokens_used`；
//! 4. 入口拒接/转人工：nemesisbot cluster_agent + board_review（经 marker
//!    与 `config_json_path` 访问器对接）。
use super::*;

/// 完整输入 token 口径单一真相源（R2 整改 2026-10-22）：
/// `prompt + cache_creation + cache_read`。两族 provider 语义对齐的依据 =
/// provider 归一化层已把 OpenAI/DeepSeek 的 cached 子集从 `prompt_tokens`
/// 剥出（剥出后 prompt = 非缓存纯输入），Anthropic 的 prompt_tokens 本来
/// 就不含缓存两列——所以「三项相加」对两族都是完整输入（整改前漏计
/// cache_creation，Anthropic 形态会话预算/压缩信号系统性偏低）。
/// `.or(cached_tokens)` 兼容旧 lane 报告形态（两键不同时出现，不双计）。
/// 全字段钳负 + 饱和加。计价分项（input_cost 三列拆分）不受此影响。
pub(crate) fn usage_full_input(usage: &crate::loop_executor::ObserverUsageInfo) -> u64 {
    let creation = usage.cache_creation_tokens.unwrap_or(0).max(0) as u64;
    let read = usage
        .cache_read_tokens
        .or(usage.cached_tokens)
        .unwrap_or(0)
        .max(0) as u64;
    (usage.prompt_tokens.max(0) as u64)
        .saturating_add(creation)
        .saturating_add(read)
}

impl AgentLoop {
    /// `agents.defaults.session_token_budget` fresh-read（F8 模式，
    /// config.json 唯一真相源——改键下一轮生效，无快照表）。
    /// **0 = 关（默认）**，与 `max_tool_iterations` 的 0=不限惯例相反。
    /// pub 供 nemesisbot 入口拒接（cluster_agent）复用同一读法单一真相源。
    pub fn current_session_token_budget(&self) -> u64 {
        self.agents_defaults_u64("session_token_budget", 0)
    }

    /// config.json 绝对路径（P8 入口拒接用：nemesisbot cluster_agent 无
    /// config_path 访问权，经此只读 accessor 单一真相源——工厂装配约定
    /// `<home>/config.json` 的路径知识不外泄）。`None` = standalone/未注入。
    pub fn config_json_path(&self) -> Option<std::path::PathBuf> {
        self.config_path.read().clone()
    }

    /// 每轮记账（observer.rs `record_round_usage` 末尾调用；预算关闭时
    /// 调用方已短路，此处零 config 读）。seed 防双计语义见
    /// [`crate::token_budget::SessionTokenBudgetLedger`]。
    pub(crate) fn record_session_tokens(&self, session_key: &str, delta: u64) {
        self.token_budget
            .record(self.data_store.as_deref(), session_key, delta);
    }

    /// 只读：会话累计 token（`chat.context_status` 观测面；预算关闭时也
    /// 可查——账目缺失路径从底账种入）。
    pub fn session_tokens_used(&self, session_key: &str) -> u64 {
        self.token_budget
            .current(self.data_store.as_deref(), session_key)
    }

    /// 骨架顶检器官（run_loop `prepare_round` 块之后，每轮开头）。
    ///
    /// - hard（≥100%）：`Some(AgentEvent::Error(...))`——文案以
    ///   [`crate::token_budget::SESSION_TOKEN_BUDGET_MARKER`] 开头，worker
    ///   `classify_terminal_failure` 识别为 `fail_class=token_budget`，
    ///   master board_review 转人工不重派。提示同时入 instance 历史
    ///   （下一会话上下文可见停机原因，M4-B 同款形态）。
    /// - soft（≥75%）：一次性提示（info 日志，仿 soft_noticed 不 nag）+
    ///   `force_compression_opts(instance, false)`（主模型压缩一次，prefill
    ///   减负——预算耗尽前多挤几轮），返回 `None` 继续本轮。
    /// - budget == 0（关）：零开销直通。
    pub(crate) async fn check_session_budget(
        &self,
        instance: &AgentInstance,
        context: &RequestContext,
    ) -> Option<AgentEvent> {
        let budget = self.current_session_token_budget();
        if budget == 0 {
            return None;
        }
        let session_key = context.session_key.as_str();
        let used = self
            .token_budget
            .current(self.data_store.as_deref(), session_key);
        if used >= budget {
            warn!(
                "[AgentLoop] session token budget exhausted: {} >= {} (session {})",
                used, budget, session_key
            );
            let notice = crate::token_budget::hard_stop_notice(used, budget);
            instance.add_assistant_message(&notice, Vec::new(), None);
            return Some(AgentEvent::Error(context.format_rpc_message(&notice)));
        }
        // soft：used*4 >= budget*3（75%），saturating 防溢出；一次性闩防 nag。
        if used.saturating_mul(4) >= budget.saturating_mul(3)
            && self.token_budget.soft_latch(session_key)
        {
            info!(
                "[AgentLoop] session token budget at >={}% ({} / {}); \
                 forcing one compression pass",
                crate::token_budget::SESSION_BUDGET_SOFT_RATIO,
                used,
                budget
            );
            self.force_compression_opts(instance, false).await;
        }
        None
    }
}
