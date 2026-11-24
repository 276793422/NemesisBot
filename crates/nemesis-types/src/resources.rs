//! 会话资源盘点（session resources）——Provider 注册制共享类型。
//!
//! 设计真相源：`docs/PLAN/2026-11-17_会话资源盘点看板计划.md`（§3.1/§3.2）。
//! 本模块只承载**契约**：trait 两变体（Global / PerLoop）、元数据、类目视图、
//! 注册表与 per-loop 最小查询抽象（[`SessionLoopApi`]）。全部是共享类型——
//! 零 IO、零进程副作用，实现方（各子系统 crate）与消费方（nemesis-web
//! handler / gateway 装配点）共同依赖。
//!
//! 依赖方向是本设计的成败点：trait 住 `nemesis-types`（最底层共享类型
//! crate）——若住 nemesis-web，则 nemesis-cron 等实现方要反向依赖
//! nemesis-web（循环依赖编译炸）。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

/// Provider 元数据——随响应逐类目下发，前端零硬编码类目清单。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderMeta {
    /// 稳定键，snake_case（"cron" / "durable_loops"…）。**对外契约，命名后
    /// 不改**——前端按它注册 CTA 映射与图标。
    pub category_id: &'static str,
    /// 看板卡片显示名。
    pub label: &'static str,
    /// 卡片排序，小在前。
    pub order: u32,
    /// 瞬态类目（审批/提问/执行态）——有值时看板高亮。
    pub transient: bool,
}

impl ProviderMeta {
    /// 便捷构造（装配点一行一个 provider 时少敲字）。
    pub const fn new(
        category_id: &'static str,
        label: &'static str,
        order: u32,
        transient: bool,
    ) -> Self {
        Self {
            category_id,
            label,
            order,
            transient,
        }
    }
}

/// 类目诚实三态：有值 / 合法无 / 子系统未装配。
///
/// 三态**统一由 [`CategoryView::status`] 单点表达**——ProviderMeta 不设
/// availability 字段（collect 必然被调，meta 上再判一次是同一信息两处表达）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CategoryStatus {
    /// 有值。
    Ok,
    /// 合法无（该会话没有这类资源是常态）。
    Empty,
    /// 子系统未装配 / 查询失败——如实标注不静默吞。
    Unavailable {
        /// 人读原因（前端「⚠ 不可用：原因」直显）。
        reason: String,
    },
}

impl CategoryStatus {
    /// 序列化短标（响应行 `status` 字段：`"ok" / "empty" / "unavailable"`）。
    pub fn as_str(&self) -> &'static str {
        match self {
            CategoryStatus::Ok => "ok",
            CategoryStatus::Empty => "empty",
            CategoryStatus::Unavailable { .. } => "unavailable",
        }
    }

    /// Unavailable 的原因（非 Unavailable 返回 None）。
    pub fn unavailable_reason(&self) -> Option<&str> {
        match self {
            CategoryStatus::Unavailable { reason } => Some(reason),
            _ => None,
        }
    }
}

/// 单类目盘点视图——summary 用自由 [`Value`]（开放插入的前提：新 provider
/// 不改聚合协议类型）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CategoryView {
    pub status: CategoryStatus,
    /// 资源条数（Empty = 0；Unavailable 语义上无计数，恒 0）。
    pub total: usize,
    /// 摘要 + 前几条；重明细走既有专用命令二次拉取。
    pub summary: Value,
}

impl CategoryView {
    pub fn ok(total: usize, summary: Value) -> Self {
        Self {
            status: CategoryStatus::Ok,
            total,
            summary,
        }
    }

    pub fn empty() -> Self {
        Self {
            status: CategoryStatus::Empty,
            total: 0,
            summary: Value::Null,
        }
    }

    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            status: CategoryStatus::Unavailable {
                reason: reason.into(),
            },
            total: 0,
            summary: Value::Null,
        }
    }
}

/// 收集上下文——聚合面按 session_id 归一后构造，逐 provider 下发。
#[derive(Debug, Clone, Copy)]
pub struct ResourceCtx<'a> {
    /// 完整键 `agent:main:session:{sid}`。
    pub session_key: &'a str,
    /// 剥前缀裸 sid（hooks 双口径用，口径地雷 3）。
    pub bare_sid: &'a str,
    /// 工作区根（文件读类 provider 用：hooks.json / todo / loops）。
    pub workspace_root: &'a Path,
}

/// 全局单例资源 provider（审批/提问/后台进程/cron/durable loops……——
/// 聚合面直接可达，不经 per-loop 路由）。
pub trait GlobalResourceProvider: Send + Sync {
    fn meta(&self) -> ProviderMeta;
    fn collect(&self, ctx: &ResourceCtx) -> CategoryView;
}

/// per-loop 资源 provider（context/checkpoint/execution……——必须经
/// `resolve_session_loop` 单一裁决点路由到所属 loop 实例；硬编码主 loop =
/// 项目会话假成功的根因模式）。
pub trait PerLoopResourceProvider: Send + Sync {
    fn meta(&self) -> ProviderMeta;
    /// `loop_api` 由聚合面 resolve 后填充；None（loop 缺席）时聚合面统一回
    /// Unavailable——「loop 缺席」错误路径不存在于 provider 侧，无需每个
    /// 实现重复处理。
    fn collect(&self, ctx: &ResourceCtx, loop_api: &dyn SessionLoopApi) -> CategoryView;
}

/// 两变体的类型级打包（注册表保序容器元素）。
pub enum ResourceProvider {
    Global(Box<dyn GlobalResourceProvider>),
    PerLoop(Box<dyn PerLoopResourceProvider>),
}

impl ResourceProvider {
    /// 元数据委托（注册表排序 / 响应 meta 投影共用）。`ProviderMeta` 是
    /// `Copy` 小结构——owned 返回零开销，不做引用中转。
    pub fn meta(&self) -> ProviderMeta {
        match self {
            ResourceProvider::Global(p) => p.meta(),
            ResourceProvider::PerLoop(p) => p.meta(),
        }
    }
}

/// per-loop 查询的最小抽象——`ResourceCtx` 不能带 `&AgentLoop` 具体类型
/// （types 依赖 agent 方向反）。
///
/// **防膨胀纪律**：方法集只准加「资源盘点必需」项，不是 AgentLoop 的通用
/// 投影；当前 4 方法封顶起步（每个方法都必须有明确类目消费方——
/// `inbox_pending` 消费方 = execution）。将来出现无归属方法即删。
pub trait SessionLoopApi {
    /// 委托既有 `AgentLoop::session_context_status`（M5 快照，现成）。
    fn context_status(&self, session_key: &str) -> Option<Value>;
    /// 委托既有 `AgentLoop::inbox_status`（P4 WS8）——(steer, follow_up)
    /// 两条队列计数。消费方 = execution 类目（排队中消息是执行态的一部分）。
    fn inbox_pending(&self, session_key: &str) -> (usize, usize);
    /// loop 运行态快照（B1 新建）：当前轮次在跑什么工具、何时开始。
    /// None = 该会话当前无在跑轮次（空闲）。
    fn work_timeline(&self, session_key: &str) -> Option<Value>;
    /// 等待队列只读快照（2026-11-24 资源看板「等待队列」气泡）：
    /// `{steer: [...], follow_up: [...], capacity}`——每条
    /// `{content, sender_id, timestamp, priority}`，content 单条 8000 字
    /// 上限截断（上限住在 agent inbox 侧，本 trait 只传形状）。开放 Value
    /// 形状与 context_status 同先例（types 层不依赖 agent inbox 类型）。
    /// None = 查询面缺席。
    fn queue_peek(&self, session_key: &str) -> Option<Value>;
    /// 投影扩展产物（CheckpointMeta 加 session 字段，serde default 加法，
    /// rewind/redo/file_diff 既有消费方零影响）。只返回**本会话**的条目。
    fn checkpoint_summary(&self, session_key: &str) -> Vec<CheckpointSummary>;
}

/// checkpoint 元数据的 types 层投影（§口径地雷 5：`CheckpointMeta` 不带
/// session——聚合面直接用会拿到全 store 无差别条目；投影扩展一体交付）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointSummary {
    pub turn: usize,
    pub time: String,
    pub prompt: String,
    /// 归属会话键（投影自带，聚合面二次过滤/展示用）。
    pub session: Option<String>,
}

/// Provider 注册表——AppState 持 `Option<Arc<...>>`（Option = 聚合特性自身
/// 可缺席，诚实降级）。**实现分散、注册集中**：provider 代码住在各自子系统
/// crate，gateway 装配点一行注册一个；本结构只保序遍历。
#[derive(Default)]
pub struct SessionResourceRegistry {
    providers: Vec<ResourceProvider>,
}

impl SessionResourceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册一个 provider（保序；输出按 `meta.order` 排）。
    pub fn register(&mut self, provider: ResourceProvider) -> &mut Self {
        self.providers.push(provider);
        self
    }

    /// 全部注册项（保序；响应侧按 meta.order 排序展示）。
    pub fn providers(&self) -> &[ResourceProvider] {
        &self.providers
    }

    /// 是否空注册表（None 注册 + 空注册表都走「未装配」诚实降级）。
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }
}
// 测试在 crate 根的 resources_tests.rs（lib.rs 级 `#[cfg(test)] mod
// resources_tests;` 声明，与 memory_tests 等同款惯例）。
