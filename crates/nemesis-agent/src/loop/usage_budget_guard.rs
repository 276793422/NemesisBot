//! #30 全局用量配额顶检器官（W1 安全图谱批，2026-11-19）。
//!
//! config `usage.budget`（typed [`nemesis_config::UsageBudgetConfig`]）：
//! 日/月 token + 日/月费用四维上限，各维 0 = 关（**默认全关** = 零查询
//! 零开销直通，既有部署零行为变化）。达限行为 `mode`：`reject`（默认）
//! = 轮顶检终局停车（诚实报错文案，本消息入 instance 历史下一会话可见
//! 停机原因——mandate 同款形态）；`warn` = 只 tracing WARN 不阻断。
//!
//! 口径（与账本聚合同源）：token = 完整输入（prompt + cache_creation +
//! cache_read）+ output——`aggregate_usage_since` 的聚合列直和，与
//! mandate/session 预算同源；费用 = 账本 `total_cost_usd` 折算直和。
//! 窗口 = **本地时区**自然日/自然月。
//!
//! 诚实下界：data_store 缺席（headless 无账本）/查询失败/usage-store
//! feature 关（恒全零聚合）→ 用量按 0 处理放行 + WARN——配额闸宁可晚
//! 炸不误炸（不因观测面故障把生产停车）。
//!
//! 范围边界：本器官只管**本进程** AgentLoop 的轮顶检；worker 侧集群
//! 委派任务的预算归 master（board.budget.max_tokens_per_parent，E1
//! 二期）——两闸独立，不互相替代。
use super::*;
use chrono::{Datelike, TimeZone};

/// 单窗口用量聚合（tokens = 完整输入+output 直和；cost_usd = 折算直和）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct UsageWindowAgg {
    pub tokens: u64,
    pub cost_usd: f64,
}

impl UsageWindowAgg {
    const ZERO: UsageWindowAgg = UsageWindowAgg {
        tokens: 0,
        cost_usd: 0.0,
    };
}

/// 纯裁决：四维按序检查（日 token → 月 token → 日费用 → 月费用），
/// 返回**首个**超限的诚实详情（None = 全部未超限/全关）。
pub(crate) fn budget_breach(
    settings: &nemesis_config::UsageBudgetSettings,
    day: &UsageWindowAgg,
    month: &UsageWindowAgg,
) -> Option<String> {
    if settings.daily_token_limit > 0 && day.tokens >= settings.daily_token_limit {
        return Some(format!(
            "今日 token 用量 {} 已达日上限 {}（usage.budget.daily_token_limit）",
            day.tokens, settings.daily_token_limit
        ));
    }
    if settings.monthly_token_limit > 0 && month.tokens >= settings.monthly_token_limit {
        return Some(format!(
            "本月 token 用量 {} 已达月上限 {}（usage.budget.monthly_token_limit）",
            month.tokens, settings.monthly_token_limit
        ));
    }
    if settings.daily_cost_limit_usd > 0.0 && day.cost_usd >= settings.daily_cost_limit_usd {
        return Some(format!(
            "今日费用 ${:.2} 已达日上限 ${:.2}（usage.budget.daily_cost_limit_usd）",
            day.cost_usd, settings.daily_cost_limit_usd
        ));
    }
    if settings.monthly_cost_limit_usd > 0.0 && month.cost_usd >= settings.monthly_cost_limit_usd {
        return Some(format!(
            "本月费用 ${:.2} 已达月上限 ${:.2}（usage.budget.monthly_cost_limit_usd）",
            month.cost_usd, settings.monthly_cost_limit_usd
        ));
    }
    None
}

/// 本地时区某自然日 00:00 的 unix 秒。DST 间隙（午夜不存在，极罕见）=
/// None——调用方按「窗口不可判定」跳过该维（诚实下界，不猜）。
pub(crate) fn unix_secs_at_local_midnight(day: chrono::NaiveDate) -> Option<i64> {
    let ndt = day.and_hms_opt(0, 0, 0)?;
    match chrono::Local.from_local_datetime(&ndt) {
        chrono::LocalResult::Single(dt) => Some(dt.timestamp()),
        chrono::LocalResult::Ambiguous(dt, _) => Some(dt.timestamp()),
        chrono::LocalResult::None => None,
    }
}

fn window_aggs(ds: &nemesis_data::DataStore) -> (UsageWindowAgg, UsageWindowAgg) {
    let now = chrono::Local::now();
    let zero = (UsageWindowAgg::ZERO, UsageWindowAgg::ZERO);
    // 日窗口：本地自然日 00:00；月窗口：本地自然月 1 日 00:00。窗口起点
    // 不可判定（DST 间隙）→ 该窗口按 0 放行（诚实下界 + WARN）。
    let Some(day_start) = unix_secs_at_local_midnight(now.date_naive()) else {
        tracing::warn!(
            "[AgentLoop] usage budget: 本地日窗口起点不可判定（DST 间隙），本窗口按 0 放行"
        );
        return zero;
    };
    let month_first = now.date_naive().with_day(1).unwrap_or(now.date_naive());
    let Some(month_start) = unix_secs_at_local_midnight(month_first) else {
        tracing::warn!(
            "[AgentLoop] usage budget: 本地月窗口起点不可判定（DST 间隙），本窗口按 0 放行"
        );
        return zero;
    };
    let agg = |since: i64| -> UsageWindowAgg {
        match ds.aggregate_usage_since(since) {
            Ok(a) => UsageWindowAgg {
                tokens: a.input_tokens.max(0) as u64 + a.output_tokens.max(0) as u64,
                cost_usd: a.total_cost_usd,
            },
            Err(e) => {
                tracing::warn!("[AgentLoop] usage budget: 账本聚合失败（按 0 放行）: {e}");
                UsageWindowAgg::ZERO
            }
        }
    };
    (agg(day_start), agg(month_start))
}

impl AgentLoop {
    /// 轮顶检器官（run_loop 骨架，`check_mandate_round` 块之后）。全维
    /// 0 = 关（默认）零开销直通（连 data_store 都不碰）。
    pub(crate) fn check_usage_budget_round(
        &self,
        instance: &AgentInstance,
        context: &RequestContext,
    ) -> Option<AgentEvent> {
        let settings = nemesis_config::resolve_usage_budget(self.config_generation().as_deref());
        let any_enabled = settings.daily_token_limit > 0
            || settings.monthly_token_limit > 0
            || settings.daily_cost_limit_usd > 0.0
            || settings.monthly_cost_limit_usd > 0.0;
        if !any_enabled {
            return None;
        }
        let Some(ds) = self.data_store.as_deref() else {
            tracing::warn!("[AgentLoop] usage budget: data_store 缺席（无账本），配额按 0 放行");
            return None;
        };
        let (day, month) = window_aggs(ds);
        let detail = budget_breach(&settings, &day, &month)?;
        if !settings.reject {
            tracing::warn!("[AgentLoop] usage budget: {detail}（mode=warn 只告警不阻断）");
            return None;
        }
        let notice = format!(
            "[用量配额已达上限] {detail}。agent 停车（usage.budget.mode=reject）：请调整 config.json 配额、切换计费维度或等待自然窗口重置后继续。"
        );
        tracing::warn!("[AgentLoop] usage budget breach (terminal): {detail}");
        instance.add_assistant_message(&notice, Vec::new(), None);
        Some(AgentEvent::Error(context.format_rpc_message(&notice)))
    }
}
