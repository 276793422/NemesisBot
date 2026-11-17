//! 会话运行态快照（work_timeline）——资源看板 execution 类目的数据源
//! （2026-11-17 会话资源盘点，计划 §3.2）。
//!
//! 此前 AgentLoop 只有全局 `in_flight_turns` gauge 与推送型 tool_event，
//! 无 per-session「当前轮次在跑什么工具/何时开始」的同步查询面。本模块
//! 补上：turn 任务体入口插入条目（[`TurnTimelineGuard`]，Drop 清理——
//! panic/abort 展开路径同样减账，与 InFlightTurnGuard 同纪律），工具
//! dispatch 前后经 [`AgentLoop` 方法] 更新当前工具名。
//!
//! 纯内存态、零持久化；会话切换/重启即丢——盘点是运行态快照，不是历史。

use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;

/// 单会话在途轮次快照。
#[derive(Debug, Clone)]
pub(crate) struct WorkTimelineEntry {
    /// 本轮开始时刻（epoch ms）。
    pub turn_started_ms: u64,
    /// 当前（最内层）在跑工具与开始时刻；None = LLM 调用段或轮次间隙。
    pub current_tool: Option<(String, u64)>,
}

pub(crate) type WorkTimelineMap = Arc<Mutex<HashMap<String, WorkTimelineEntry>>>;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// turn 任务体守卫——构造即插入条目，Drop（含 panic/abort 展开）即移除。
/// 生命周期 = 整个 turn（spawn 臂 + inline continuation 臂，与
/// InFlightTurnGuard 同点 acquire）。
pub(crate) struct TurnTimelineGuard {
    map: WorkTimelineMap,
    session_key: String,
}

impl TurnTimelineGuard {
    pub(crate) fn acquire(map: &WorkTimelineMap, session_key: &str) -> Self {
        map.lock().insert(
            session_key.to_string(),
            WorkTimelineEntry {
                turn_started_ms: now_ms(),
                current_tool: None,
            },
        );
        Self {
            map: Arc::clone(map),
            session_key: session_key.to_string(),
        }
    }
}

impl Drop for TurnTimelineGuard {
    fn drop(&mut self) {
        self.map.lock().remove(&self.session_key);
    }
}

/// 工具段守卫——dispatch 执行体入口置当前工具，Drop 复位为 None
/// （条目保留：轮次还在跑，只是回到 LLM 段）。条目不存在（turn 已收尾的
/// 极端交错）= 静默 no-op，绝不复活条目。
pub(crate) struct ToolTimelineGuard {
    map: WorkTimelineMap,
    session_key: String,
}

impl ToolTimelineGuard {
    pub(crate) fn acquire(map: &WorkTimelineMap, session_key: &str, tool: &str) -> Self {
        if let Some(entry) = map.lock().get_mut(session_key) {
            entry.current_tool = Some((tool.to_string(), now_ms()));
        }
        Self {
            map: Arc::clone(map),
            session_key: session_key.to_string(),
        }
    }
}

impl Drop for ToolTimelineGuard {
    fn drop(&mut self) {
        if let Some(entry) = self.map.lock().get_mut(&self.session_key) {
            entry.current_tool = None;
        }
    }
}

/// AgentLoop 侧的查询面（SessionLoopApi::work_timeline 的原料）。
impl super::AgentLoop {
    /// 运行态图句柄（new() 初始化；字段本体私有）。
    pub(crate) fn work_timeline_map(&self) -> &WorkTimelineMap {
        &self.work_timeline
    }

    /// 会话运行态快照；None = 当前无在跑轮次（空闲）。
    /// crate 内可见（resource_providers 消费；WorkTimelineEntry 是
    /// pub(crate) 类型，pub 方法会把私有类型泄进公开 API）。
    pub(crate) fn work_timeline_snapshot(&self, session_key: &str) -> Option<WorkTimelineEntry> {
        self.work_timeline.lock().get(session_key).cloned()
    }
}
