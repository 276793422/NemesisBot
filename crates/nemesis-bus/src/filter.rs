//! 入站过滤链框架（InboundFilterChain）。
//!
//! 框架级消息拦截点（同构 Windows 设备栈过滤驱动 / minifilter 契约）：
//! 挂载点在消息进入扇出**之前**（如 web 入站桥 → `bus.publish_inbound`
//! 之前的咽喉位），过滤器可对消息类做「就地消费（Intercepted）」或
//! 「放行（Pass）」。首个非 Pass 决策终止链。
//!
//! **防腐红线**：链本体（[`FilterChain`]）永远不允许出现任何业务消息
//! 判断（如 `msg.module == "chat"` 之类的字段嗅探）——业务语义只住在
//! 具体 [`Filter`] 实现里。链只管三件事：有序、决策语义、可观测。
//! 哪个 PR 把业务语义塞进链本体，哪个 PR 打回。
//!
//! 使用方式（谁想用，谁注册）：
//!
//! 1. 挂载点持有一条 [`FilterChain<T>`] 实例（**每挂载点独立实例**，
//!    不共享全局单链——不同入口的过滤语义与次序天然不同）；
//! 2. 功能方实现 [`Filter`]，在装配期 `attach` 进链；
//! 3. 挂载点在扇出前调用 `run(&msg)`，按决策继续/终止。
//!
//! 首个使用者：web 入站桥的 `history` 过滤器（BUG 2026-09-23 项目会话
//! 历史加载修复）；`project-route`（ProjectLoopManager）是预留的未来
//! 迁移位。

use std::sync::Arc;

use parking_lot::RwLock;

/// 过滤决策（封闭枚举）。
///
/// v1 刻意**不含「修改后放行」**：[`Filter::inspect`] 对消息只读。
/// 过滤器 B 若能看到过滤器 A 改过的消息而浑然不觉，次序语义会变成
/// 隐形地雷。未来真出现改写需求，作为显式的新决策变体（带审计）设计，
/// 不悄悄加参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterDecision {
    /// 本过滤器已消费该消息（自行应答），链终止——消息不继续下传。
    Intercepted,
    /// 放行：交给下一过滤器 / 最终路由。
    Pass,
    /// 链终止并拒绝。**应答格式由挂载点决定**——链不知道对面是 web 帧
    /// 还是 IM 消息，不越权格式化；挂载点收到后按自己的通道诚实回错。
    Rejected(String),
}

/// 入站过滤器（框架契约）。
///
/// 实现要求：轻副作用、语义自足（不依赖链上其他过滤器的执行与否）、
/// 具名可观测。
#[async_trait::async_trait]
pub trait Filter<T>: Send + Sync {
    /// 过滤器名（日志与链清单用）。
    fn name(&self) -> &'static str;

    /// 显式次序（小者先跑；仿 minifilter altitude——**不依赖注册顺序**）。
    fn priority(&self) -> i32;

    /// 检查一条消息。只读：实现不得修改消息本体。
    async fn inspect(&self, msg: &T) -> FilterDecision;
}

/// 有序过滤链。挂载点在扇出前对每条消息调用 [`FilterChain::run`]。
pub struct FilterChain<T> {
    filters: RwLock<Vec<Arc<dyn Filter<T>>>>,
}

impl<T: Send + Sync + 'static> FilterChain<T> {
    /// 空链（run 恒 Pass）。
    pub fn new() -> Self {
        Self {
            filters: RwLock::new(Vec::new()),
        }
    }

    /// 附加过滤器（≈ IoAttachDeviceToDeviceStack）。按 [`Filter::priority`]
    /// 稳定插入（同 priority 保注册顺序）。
    pub fn attach(&self, f: Arc<dyn Filter<T>>) {
        let mut filters = self.filters.write();
        let pos = filters.partition_point(|existing| existing.priority() <= f.priority());
        filters.insert(pos, f);
    }

    /// 链清单（`[(name, priority)]`，已排序）——启动日志与测试断言用。
    pub fn list(&self) -> Vec<(&'static str, i32)> {
        self.filters
            .read()
            .iter()
            .map(|f| (f.name(), f.priority()))
            .collect()
    }

    /// 依序跑；首个非 Pass 决策终止。可观测性契约：**拦截永不静默**——
    /// Intercepted 记 info、Rejected 记 warn（各含过滤器名），Pass 仅
    /// trace（热路径降噪）。
    pub async fn run(&self, msg: &T) -> FilterDecision {
        let filters = self.filters.read().clone();
        for f in &filters {
            match f.inspect(msg).await {
                FilterDecision::Pass => {
                    tracing::trace!(filter = f.name(), "[FilterChain] pass");
                }
                decision @ (FilterDecision::Intercepted | FilterDecision::Rejected(_)) => {
                    let reason = match &decision {
                        FilterDecision::Rejected(r) => format!(" rejected: {r}"),
                        _ => String::new(),
                    };
                    tracing::info!(
                        filter = f.name(),
                        "[FilterChain] intercepted{reason}（消息未进入扇出）"
                    );
                    return decision;
                }
            }
        }
        FilterDecision::Pass
    }
}

impl<T: Send + Sync + 'static> Default for FilterChain<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> std::fmt::Debug for FilterChain<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 不触碰内部过滤器（Debug 应免锁、免副作用），只报规模。
        f.debug_struct("FilterChain")
            .field("filters", &self.filters.read().len())
            .finish()
    }
}
