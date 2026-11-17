//! 会话资源盘点——nemesis-web 侧 provider 集（2026-11-17 会话资源盘点
//! 看板计划 B1）。契约（trait / CategoryView）住 [`nemesis_types::resources`]；
//! 本模块交付 web 侧两类目：
//!
//! - [`UsageProvider`]（usage / 用量）：账本聚合（nemesis-data
//!   `aggregate_session_usage`，单会话精确匹配 + input 口径含 cache 全量；
//!   usage-store 未编译进构建 = 空账本 stub 恒零 → Empty 诚实降级）；
//! - [`SessionAllowProvider`]（session_allow / 全部放行，`security` feature
//!   门控）：会话级全部放行运行时态（2026-10-24 无人值守 Feature A）——
//!   未开启是常态 = Empty；开启 = ok（since + 本期自动批准计数）。
//!
//! 装配纪律同 agent 侧（resource_providers.rs 头注释）：构造收 `Option<件>`
//! **无条件注册**，collect 内诚实判（Unavailable / Empty），gateway 装配点
//! 保持「一行一个」平铺。

use nemesis_types::resources::{CategoryView, GlobalResourceProvider, ProviderMeta, ResourceCtx};
use serde_json::json;
use std::sync::Arc;

/// 用量（`usage` / 用量）。数据源 = DataStore 单会话用量聚合（requests
/// 计数 + token 三轴 + 成本；`input_tokens` 口径 = 完整输入含 cache
/// creation/read，与 loop 侧预算 delta 同源）。会话无请求行 / usage-store
/// 未编译（恒零聚合）= Empty；账本查询失败 = Unavailable。
pub struct UsageProvider {
    store: Option<Arc<nemesis_data::DataStore>>,
}

impl UsageProvider {
    pub fn new(store: Option<Arc<nemesis_data::DataStore>>) -> Self {
        Self { store }
    }
}

impl GlobalResourceProvider for UsageProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::new("usage", "用量", 130, false)
    }

    fn collect(&self, ctx: &ResourceCtx) -> CategoryView {
        let Some(store) = &self.store else {
            return CategoryView::unavailable("用量账本未装配");
        };
        let agg = match store.aggregate_session_usage(ctx.session_key) {
            Ok(a) => a,
            Err(reason) => return CategoryView::unavailable(reason),
        };
        if agg.requests == 0 {
            return CategoryView::empty();
        }
        CategoryView::ok(
            agg.requests as usize,
            json!({
                "requests": agg.requests,
                "input_tokens": agg.input_tokens,
                "output_tokens": agg.output_tokens,
                "total_cost_usd": agg.total_cost_usd,
            }),
        )
    }
}

/// 会话级全部放行（`session_allow` / 全部放行，`security` feature 门控）。
/// 数据源 = [`nemesis_security::session_allow_all::SessionAllowAllState`]
/// 运行时态（内存权威，sidecar 持久由 gateway 双写）。三态口径：
/// 未装配 = Unavailable；无条目 / 条目已关 = Empty（关过的旧条目保留
/// since/count 供回看，盘点不展示）；开启中 = ok。
#[cfg(feature = "security")]
pub struct SessionAllowProvider {
    state: Option<Arc<nemesis_security::session_allow_all::SessionAllowAllState>>,
}

#[cfg(feature = "security")]
impl SessionAllowProvider {
    pub fn new(
        state: Option<Arc<nemesis_security::session_allow_all::SessionAllowAllState>>,
    ) -> Self {
        Self { state }
    }
}

#[cfg(feature = "security")]
impl GlobalResourceProvider for SessionAllowProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::new("session_allow", "全部放行", 140, false)
    }

    fn collect(&self, ctx: &ResourceCtx) -> CategoryView {
        let Some(state) = &self.state else {
            return CategoryView::unavailable("全部放行状态未装配");
        };
        let Some((enabled, since, count)) = state.status(ctx.session_key) else {
            return CategoryView::empty();
        };
        if !enabled {
            return CategoryView::empty();
        }
        CategoryView::ok(
            1,
            json!({"enabled": true, "since": since, "auto_approved": count}),
        )
    }
}
