//! §6.2（2026-10-12 无人值守计划）：验收/对话 LLM 空输出撞 max_tokens 根修——
//! **撞顶翻倍阶梯**。
//!
//! 事实链（GLM 真机实证）：思考型模型思考 token 烧穿 `max_tokens`（config
//! 条目缺键时 8192 硬默认）→ 正文为空 → 连续 2 轮空输出 guard 提前停轮 →
//! 评审自身失败恒转人工。本模块在主 loop 的 LLM 调用收口处（[`AgentLoop::chat_call_escalating`]，
//! recovery.rs 五个调用点）包翻倍阶梯：
//!
//! - **判定**：`finish_reason == "length"` 且正文空白，且（① 无工具调用 =
//!   纯思考烧穿，§6.2 原形态；② 工具调用参数全为空对象/空串/非法 JSON 且
//!   schema 需要参数 = 工具参数截断，BUG 2026-10-23 第四形态扩展——主
//!   agent 轮恒挂工具，同一故障必然带着 tool_calls 出现）＝ 烧穿形态。
//!   非空截断 / 带合法参数的工具调用走既有 continue-generation，
//!   [`super::run_loop] 的 `handle_length_continuation`，分工边界。
//! - **阶梯**：`next = max(current × 2, 32768)`（32K 地板：条目 8192/16384
//!   撞顶直接跳 32K 不浪费一轮），`next ≤ ceiling` 才重发（封顶
//!   `agents.defaults.max_output_ceiling`，默认 262144，每轮 config 现读）。
//!   触封顶仍空 = 现行失败语义收口（空响应照常返回，空输出 guard 终局；
//!   响应带 `escalated_to` 供守卫文案如实报告「已试到 N」）。
//! - **不回写 config**：翻倍只改当次请求。per-model **learned floor** 缓存
//!   （[`AgentLoop::note_escalated_floor`]）让同 turn 后续轮 / 后续 turn
//!   直接从 floor 起步不再烧撞顶轮；config.json mtime 变化清空（用户显式
//!   改配置 = 重置学习，`check_config_reload`）。
//! - **estop**：急停挂起时不再翻倍重发（LLM 活动冻结语义），当前响应照常
//!   返回由上游收口。
//! - **可观测**：每次翻倍 WARN（撞顶值/翻倍值/第几次/封顶值）；每次重试
//!   走正常调用链 = 请求日志/审计天然可见。
//!
//! cache_warmer 的 1-token warm 调用**不接**（恒 length+空形态，翻倍毫无
//! 意义）——继续走纯超时包装 [`AgentLoop::chat_call_bounded`]。

use std::sync::Arc;

use tracing::warn;

use super::AgentLoop;
use super::llm_types::{LlmMessage, LlmProvider, LlmResponse};
use crate::types::{ToolCallInfo, ToolDefinition};

/// 工具参数截断签名（BUG 2026-10-23 第四形态）：全部工具调用的参数均为
/// 空串 / 空对象 / 非法 JSON，且（schema 可查时）该工具声明需要参数。
/// 任一工具调用带合法参数 = 不是纯截断形态（可能有可执行进度），不判烧穿。
fn tool_args_all_truncated(tool_calls: &[ToolCallInfo], tools: &[ToolDefinition]) -> bool {
    !tool_calls.is_empty()
        && tool_calls.iter().all(|tc| {
            let a = tc.arguments.trim();
            let looks_truncated =
                a.is_empty() || a == "{}" || serde_json::from_str::<serde_json::Value>(a).is_err();
            if !looks_truncated {
                return false;
            }
            match tools.iter().find(|d| d.function.name == tc.name) {
                Some(d) => schema_expects_args(&d.function.parameters),
                // schema 未知（工具不在供给清单）→ 保守按截断算
                None => true,
            }
        })
}

/// 工具参数 schema 是否声明需要参数（required 或 properties 非空）。
fn schema_expects_args(parameters: &serde_json::Value) -> bool {
    parameters
        .get("required")
        .and_then(|v| v.as_array())
        .is_some_and(|a| !a.is_empty())
        || parameters
            .get("properties")
            .and_then(|v| v.as_object())
            .is_some_and(|o| !o.is_empty())
}

/// 阶梯地板：撞顶后第一次重发的 max_tokens 下限（三机 GLM 条目已配 32768
/// 的现状 ×2 顺滑衔接；8192 硬默认条目撞顶直接跳 32K）。
pub(crate) const ESCALATION_FLOOR: u64 = 32_768;

impl AgentLoop {
    /// §6.2 主入口：recovery.rs 五个 LLM 调用点（首捕/压缩重试环/429 环/
    /// transient 环/hook 重呼）统一走这里——在 [`AgentLoop::chat_call_bounded`]
    /// 超时包装之上叠加翻倍阶梯。签名与 chat_call_bounded 完全一致（调用点
    /// 机械替换）。
    pub(crate) async fn chat_call_escalating(
        &self,
        call_timeout: u64,
        provider: &Arc<dyn LlmProvider>,
        model: &str,
        messages: Vec<LlmMessage>,
        options: Option<crate::types::ChatOptions>,
        tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        // learned floor 叠加：同模型已知需要更大输出时，本次请求直接从
        // floor 起步（config 值更大则尊重 config）。floor 来自上一轮撞顶
        // 学习；config mtime 变化已清空，用户显式配置不会被陈旧学习覆盖。
        let learned = self.escalated_floor_for_model(model);
        let options = options.map(|mut o| {
            if learned > 0 && o.max_tokens.is_none_or(|t| (t as u64) < learned) {
                // 与下方 write-back 的 `.min(u32::MAX as u64)` 同一防护：
                // ceiling 被配成超 u32 巨值时 learned 可超 u32::MAX，裸 `as`
                // 回绕成 0/小值反而砍上限（2026-10-23）。
                o.max_tokens = Some(learned.min(u32::MAX as u64) as u32);
            }
            o
        });
        let mut current = options
            .as_ref()
            .and_then(|o| o.max_tokens)
            .map(|t| t as u64);
        let ceiling = self.current_max_output_ceiling();
        let mut attempt: u32 = 0;
        let resp = loop {
            // messages/tools 按 attempts 克隆（原件保持完好供重试）：常规
            // 路径恰 1 次克隆与按值消费等价；翻倍重试是稀有路径，多一次
            // 克隆换掉「预 clone 双缓冲」的常驻开销。max_tokens 每次从
            // `current` 写回——learned floor 叠加值与翻倍阶梯值都经它生效。
            let mut call_opts = options.clone();
            if let (Some(o), Some(cur)) = (call_opts.as_mut(), current) {
                o.max_tokens = Some(cur.min(u32::MAX as u64) as u32);
            }
            let r = Self::chat_call_bounded(
                call_timeout,
                provider,
                model,
                messages.clone(),
                call_opts,
                tools.clone(),
            )
            .await?;
            // 思考烧穿形态：截断且零产出。两类指纹（BUG 2026-10-23 第四
            // 形态扩展，docs/BUG/2026-10-23_max-tokens-8192截断撞顶复发.md）：
            // ① 纯思考烧穿——正文空白 + 无工具调用（§6.2 原形态，评审 LLM
            //   出生场景）；
            // ② 工具参数截断——正文空白 + 工具调用参数为空对象/空串/非法
            //   JSON（主 agent 轮恒挂 40+ 工具，同一故障在这里必然带着
            //   tool_calls 出现：anthropic lane 把截断 input 归一化成
            //   `{}`（anthropic.rs unwrap_or(json!({}))），openai 兼容 lane
            //   截断 args 是非法 JSON 原文。2026-10-23 ziwei-e2e 事故：glm-
            //   5.3-flash 思考 + multiedit 大参数三轮恒 8192 截断，阶梯因
            //   `tool_calls.is_empty()` 指纹不匹配从未触发）。
            // 工具 schema 声明需要参数（required/properties 非空）才判截断
            // ——无参工具的 `{}` 是合法调用不算；schema 未知保守按截断算：
            // 误升级代价 = 一次更高 cap 的重发（收敛为正常完成），漏判代价
            // = 零进度循环到停轮。
            let burned = r.finish_reason.as_deref() == Some("length")
                && r.content.trim().is_empty()
                && (r.tool_calls.is_empty() || tool_args_all_truncated(&r.tool_calls, &tools));
            let next = current.map(|c| (c * 2).max(ESCALATION_FLOOR));
            // 终止三态：非烧穿形态（非空截断 → 既有续写机制接管）/ estop
            // 挂起（LLM 活动冻结）/ 起点 max_tokens 未知 / 翻倍超封顶。
            // 超封顶仍空 = 现行失败语义收口（空输出 guard 终局，文案经
            // escalated_to 如实报告）。
            if !burned
                || self.security.is_engaged()
                || current.is_none()
                || next.is_none_or(|n| n > ceiling)
            {
                break r;
            }
            attempt += 1;
            warn!(
                "[AgentLoop] ⚠ 输出撞顶（max_tokens={}）正文为空（思考烧穿形态），翻倍至 {} 重试（第 {attempt} 次，封顶 {ceiling}）",
                current.unwrap_or(0),
                next.unwrap_or(0),
            );
            current = next;
            self.note_escalated_floor(model, next.unwrap_or(0));
        };
        let mut resp = resp;
        if attempt > 0 {
            // last attempt 的 max_tokens——token_cap 判定与触顶文案的事实值。
            resp.escalated_to = current;
        }
        Ok(resp)
    }
    /// per-model learned floor 读取（无记录 = 0，不叠加）。
    pub(crate) fn escalated_floor_for_model(&self, model: &str) -> u64 {
        self.escalated_floors
            .lock()
            .get(model)
            .copied()
            .unwrap_or(0)
    }

    /// per-model learned floor 记录（单调不降；翻倍决策即记录——重试本身
    /// 失败不回退学习，模型烧穿上一档是既成事实）。
    pub(crate) fn note_escalated_floor(&self, model: &str, value: u64) {
        let mut floors = self.escalated_floors.lock();
        let slot = floors.entry(model.to_string()).or_insert(0);
        if value > *slot {
            *slot = value;
        }
    }

    /// 学习缓存清空（config.json mtime 变化时调用——用户显式改配置 =
    /// 重置学习，下次撞顶重新起步）。
    pub(crate) fn clear_escalated_floors(&self) {
        self.escalated_floors.lock().clear();
    }
}
