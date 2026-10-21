//! 插件资源限制表 + 并发实例闸。
//!
//! 内存/表/fuel/墙钟预算的实际执行在宿主壳 DLL（plugin_wasm.dll）里；
//! 本模块只剩额度表（单一真相源）与信号量闸。默认额度（可在 config `plugins.limits` 全局收紧；manifest `[limits]`
//! 只能在全局上限内进一步收紧，放宽被忽略并留 warn）。

use std::sync::Arc;

/// 插件资源限制表。
#[derive(Debug, Clone)]
pub struct PluginLimits {
    /// 单次调用 fuel 预算（确定性指令预算；耗尽 = trap）。
    pub fuel: u64,
    /// 单实例内存上限（字节）。
    pub memory_bytes: usize,
    /// 单实例表项上限。
    pub table_elements: usize,
    /// 进程内并发实例上限（信号量）。
    pub max_instances: usize,
    /// 单次调用墙钟超时（毫秒；epoch 闸 + 外层 tokio 超时同值双保险）。
    pub timeout_ms: u64,
    /// 每帧（单次 execute / 单条 observe）宿主调用预算。
    pub host_call_budget: u32,
    /// 每插件观察者事件队列深度（满则丢+计数，永不反压事件路径）。
    pub observer_queue_depth: usize,
    /// 宿主日志环形缓冲条数上限。
    pub log_ring_capacity: usize,
    /// 单条日志最大字节（超长截断）。
    pub log_line_max_bytes: usize,
    /// 出站 HTTP 响应体上限（字节；超限截断丢弃）。
    pub egress_body_max_bytes: usize,
    /// 工作区只读单次字节上限。
    pub workspace_read_max_bytes: usize,
    /// 出站 HTTP 单请求超时毫秒。
    pub egress_timeout_ms: u64,
}

impl Default for PluginLimits {
    fn default() -> Self {
        Self {
            fuel: 1_000_000_000,
            memory_bytes: 256 * 1024 * 1024,
            table_elements: 100_000,
            max_instances: 8,
            timeout_ms: 30_000,
            host_call_budget: 1000,
            observer_queue_depth: 256,
            log_ring_capacity: 1024,
            log_line_max_bytes: 64 * 1024,
            egress_body_max_bytes: 1024 * 1024,
            workspace_read_max_bytes: 8 * 1024 * 1024,
            egress_timeout_ms: 30_000,
        }
    }
}

impl PluginLimits {
    /// 用 manifest 声明的覆盖值收紧本表（只允许收紧；放宽请求被忽略并
    /// 返回收紧后的表 + 被忽略字段清单供调用方 warn）。
    #[must_use]
    pub fn tighten_with(
        &self,
        overrides: &std::collections::BTreeMap<String, u64>,
    ) -> (PluginLimits, Vec<String>) {
        let mut ignored = Vec::new();
        let mut out = self.clone();
        for (k, v) in overrides {
            match k.as_str() {
                "fuel" if *v > 0 && *v <= out.fuel => out.fuel = *v,
                "timeout-ms" if *v > 0 && *v <= out.timeout_ms => out.timeout_ms = *v,
                "host-call-budget" if *v > 0 && *v <= u64::from(out.host_call_budget) => {
                    out.host_call_budget = *v as u32;
                }
                "memory-bytes" if *v > 0 && (*v as usize) <= out.memory_bytes => {
                    out.memory_bytes = *v as usize;
                }
                _ => ignored.push(k.clone()),
            }
        }
        (out, ignored)
    }
}

/// 并发实例闸（进程级信号量的薄包装，测试可注入）。
#[derive(Clone)]
pub(crate) struct InstanceGate {
    semaphore: Arc<tokio::sync::Semaphore>,
    max: usize,
}

impl InstanceGate {
    pub(crate) fn new(max_instances: usize) -> Self {
        Self {
            semaphore: Arc::new(tokio::sync::Semaphore::new(max_instances)),
            max: max_instances,
        }
    }

    /// 尝试获取一个实例槽；满员立即返回 `PluginError::Busy`（不排队——
    /// agent 调度下排队会把延迟藏进 dispatch，满员是配置问题要暴露）。
    pub(crate) fn try_acquire(
        &self,
    ) -> Result<tokio::sync::OwnedSemaphorePermit, crate::error::PluginError> {
        self.semaphore
            .clone()
            .try_acquire_owned()
            .map_err(|_| crate::error::PluginError::Busy(self.max))
    }
}
