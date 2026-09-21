//! L2（devtool-upgrade 阶段 6）— per-session WS chat 帧环形缓冲。
//!
//! 与 [`crate::events::EventHub`]（SSE 全局事件流）平行：WS chat 推送帧
//!（`message.chat.receive`）按 session 记录并盖**会话内单调 seq**，
//! `chat.sync {session_id, after_seq}` 断线补拉（useWebSocket 重连点调用）。
//!
//! 存放形态：模块级 `OnceLock`（L1 同款理由——AppState 字面量散布 69 个
//! 测试文件 105 处不可动，加字段是全库爆破；模块级快照保有单一真相源
//! 性质且零爆破半径）。

use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::events::EventHub;

/// 每会话保留的最近 chat 帧条数（补拉重放窗口）。
pub const SESSION_REPLAY_CAP: usize = 200;
/// 最多同时跟踪的会话数（防泄漏上界；逐出会话失去补拉能力，
/// `replay_after` 以 gap=true 诚实上报，前端全量刷新兜底）。
const MAX_SESSIONS: usize = 128;

/// 一帧已推送的 chat 消息（`message.chat.receive` 的 data 子集 + seq）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ChatEvent {
    pub seq: u64,
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// HD（2026-09-17 顺带修）：记录时刻（后端钟）——chat.sync 重放路径
    /// 前端此前填本地 `new Date()`，补拉消息时间显示为拉取时刻而非真实
    /// 发生时刻。`skip_serializing_if` 兼容：旧条目无此字段，前端回退本地钟。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ts: Option<String>,
    /// P1（2026-09-21）：条目类别。`None` = 普通 chat 行（assistant/user
    /// 文本，兼容旧条目与旧测试）；`Some("tool")` = 工具事件——`tool` 字段
    /// 携带完整 push 载荷（ToolStarted/ToolFinished/ModeChanged），chat.sync
    /// 回放时前端据此恢复工具卡（切页/重连/重载后工具流程可见）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// `kind = "tool"` 条目的完整工具事件载荷（帧内层 data 原样）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<serde_json::Value>,
}

struct SessionLog {
    next_seq: AtomicU64,
    buf: VecDeque<Arc<ChatEvent>>,
}

/// 会话表 + 插入序（FIFO 逐出用）。map 与 order 始终同键集：
/// 条目唯一移除点是逐出，逐出时两处同步删。
struct SessionTable {
    map: HashMap<String, SessionLog>,
    order: VecDeque<String>,
}

static SESSION_LOGS: OnceLock<Mutex<SessionTable>> = OnceLock::new();

/// P8（2026-09-21）：EventHub 槽（chat.activity 多端感知信号的数据源）。
/// 模块级 OnceLock——与本文件 SESSION_LOGS 同款理由：AppState 字面量散布
/// 全库不可动，装配期 install 一次、record 路径只读。未安装时静默跳过
/// （单测/无 SSE 场景零开销）。
static EVENT_HUB: OnceLock<Arc<EventHub>> = OnceLock::new();

/// gateway 装配期安装 EventHub（P8：record 即广播 chat.activity）。
pub fn install_event_hub(hub: Arc<EventHub>) {
    let _ = EVENT_HUB.set(hub);
}

/// record 成功后广播多端感知信号：`chat.activity {session_id, seq, kind}`
/// （SSE 全局）。`record_key` 是会话键（`agent:main:session:{sid}`）——信号里
/// 还原裸 sid 与前端 currentId 同域比对；还原不了（连接级键 `web:...`）原样
/// 发，前端匹配不上自然忽略。`kind` 让落后端区分响应方式：`"tool"` → 增量
/// sync 补工具卡（user 行不落环，增量可达）；`"chat"` → 全量刷新兜底。
fn publish_activity(session_key: &str, seq: u64, kind: &str) {
    if let Some(hub) = EVENT_HUB.get() {
        let sid = session_key
            .strip_prefix("agent:main:session:")
            .unwrap_or(session_key);
        hub.publish(
            "chat.activity",
            serde_json::json!({ "session_id": sid, "seq": seq, "kind": kind }),
        );
    }
}

fn logs() -> &'static Mutex<SessionTable> {
    SESSION_LOGS.get_or_init(|| {
        Mutex::new(SessionTable {
            map: HashMap::new(),
            order: VecDeque::new(),
        })
    })
}

/// 入环共用点：分配 seq（会话内单调）、插入环形缓冲、FIFO 逐出、广播
/// chat.activity。record 与 record_tool 共用——工具事件与 chat 行落在
/// 同一 seq 序列上，回放按 seq 交错还原真实时序；逐出规则也只有一份。
fn insert_event(
    session_id: &str,
    role: &str,
    content: &str,
    model: Option<&str>,
    kind: Option<String>,
    tool: Option<serde_json::Value>,
) -> u64 {
    let mut table = logs().lock();
    let created = !table.map.contains_key(session_id);
    let entry = table
        .map
        .entry(session_id.to_string())
        .or_insert_with(|| SessionLog {
            next_seq: AtomicU64::new(0),
            buf: VecDeque::new(),
        });
    let seq = entry.next_seq.fetch_add(1, Ordering::SeqCst) + 1;
    // is_tool 先于 move 求值：kind 属权随后移入 ChatEvent（E0382）。
    let is_tool = kind.is_some();
    let event = Arc::new(ChatEvent {
        seq,
        role: role.to_string(),
        content: content.to_string(),
        model: model.map(String::from),
        ts: Some(chrono::Local::now().to_rfc3339()),
        kind,
        tool,
    });
    entry.buf.push_back(Arc::clone(&event));
    while entry.buf.len() > SESSION_REPLAY_CAP {
        entry.buf.pop_front();
    }
    // 新会话导致超员：逐出**最老插入**且非本会话的条目（FIFO——victim 确定
    // 可推理，最旧的会话最可能已死；被逐会话只是失去补拉能力，客户端
    // after>latest 会拿到 gap 诚实重同步）。
    if created {
        table.order.push_back(session_id.to_string());
        if table.map.len() > MAX_SESSIONS {
            let victim_idx = table
                .order
                .iter()
                .position(|k| k.as_str() != session_id && table.map.contains_key(k.as_str()));
            if let Some(idx) = victim_idx {
                let key = table
                    .order
                    .remove(idx)
                    .expect("idx from position() is valid");
                table.map.remove(&key);
            }
        }
    }
    drop(table); // 广播前放锁：publish 内部可能再取全局资源，不持锁跨调用
    publish_activity(session_id, seq, if is_tool { "tool" } else { "chat" });
    seq
}

/// 记录一帧 chat 推送并返回其 seq（会话内单调，1 起）。
pub fn record(session_id: &str, role: &str, content: &str, model: Option<&str>) -> u64 {
    insert_event(session_id, role, content, model, None, None)
}

/// P1（2026-09-21）：记录一条工具事件（kind="tool"，`tool` 携带完整 push
/// 载荷）。与 `record` 同键空间、同 seq 序列——回放时工具事件与 assistant
/// 行按 seq 交错还原真实时序。返回其 seq。
pub fn record_tool(session_id: &str, tool: serde_json::Value) -> u64 {
    insert_event(
        session_id,
        "",
        "",
        None,
        Some("tool".to_string()),
        Some(tool),
    )
}

/// 断线补拉：返回 `seq > after` 的帧（时间序，从内部 Arc clone 出）。
///
/// `gap=true` 表示缺口已滑出重放窗口（或会话记录不存在/超前——网关重启
/// 后会话环形为空而客户端持有旧 seq），调用方应提示前端全量刷新兜底。
pub fn replay_after(session_id: &str, after: u64) -> (Vec<ChatEvent>, bool) {
    let table = logs().lock();
    match table.map.get(session_id) {
        // 会话无记录：客户端 from-scratch（after=0）无需补；持旧 seq 即缺口。
        None => (Vec::new(), after > 0),
        Some(session) => {
            let latest = session.next_seq.load(Ordering::SeqCst);
            if after >= latest {
                return (Vec::new(), after > latest);
            }
            let events: Vec<ChatEvent> = session
                .buf
                .iter()
                .filter(|e| e.seq > after)
                .map(|e| (**e).clone())
                .collect();
            let gap = session
                .buf
                .front()
                .is_none_or(|oldest| oldest.seq > after + 1);
            (events, gap)
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    /// 测试专用串行闸：SESSION_LOGS 是进程级全局表且带逐出——并行测试
    /// 不互斥会互相逐掉对方会话（env-test-race 家族）。tokio Mutex：
    /// async 测试持锁跨 await 合法（clippy await_holding_lock 不碰 tokio 闸）。
    pub static GLOBAL_TABLE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
}

#[cfg(test)]
mod tests;
