//! M5（2026-10-10 集群无人值守硬化）— 跨节点批复桥（master 侧注册表 + 回源）。
//!
//! worker 执行看板任务时的 ABAC 审批 ask 卡在 worker 本地 Dashboard（裁决/
//! 超时真相源留 worker，300s 语义不变）；人不在 worker 前 = ask 必超时被拒
//! （评估期 A6/P6 实证）。本模块给 master 侧两个批复入口，裁决经 RPC 下发
//! worker 本地 pending：
//!
//! - **Dashboard 代理批复**：`approval.remote_list` / `approval.remote_respond`
//!   （handlers/approval.rs），卡片数据源 = SSE
//!   [`EVENT_REQUESTED`] / [`EVENT_RESOLVED`]（nb_bus ns="approval" 路由在
//!   nemesisbot 侧 board_bus.rs 发布）。
//! - **看板评论回源**：issue 线程评论 `/approve <短号>` / `/deny <短号>`
//!   （[`try_remote_approval_reply`]，挂 board.rs comment.add 与 board_bus
//!   post_discussion_core 两个落库点；短号在注册表才动作，杂散同形文本
//!   零影响）。
//!
//! 注册表 = 进程级 [`registry()`]（模块级 OnceLock——AppState 字面量散布
//! 全库不可动，chat_event_log::EVENT_HUB 同款理由），零装配接线：worker
//! 上行登记即入表，TTL 惰性清理（timeout + 60s 余量，ask_closed 丢失的
//! 卡面残留自然过期）。
//!
//! 信任级：能写看板评论 ≍ Dashboard 登录用户；远程批复 **always 钉死
//! false**（规则记忆只允许 worker 本地 Dashboard 显式批准落盘）。
#![cfg(feature = "cluster")]

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::OnceLock;

/// nb_bus 审批命名空间（worker 上行 ask_register/ask_closed）。
pub const NS_APPROVAL: &str = "approval";
/// RPC 下行 action：master → worker 裁决注入（worker 侧闭包调
/// `WebApprovalManager::respond`，runtime.rs 装配）。
pub const ACTION_APPROVAL_RESPOND: &str = "approval.respond";
/// RPC 下行 action：master → worker pending 查询（remote_list 的跨节点
/// 核对面；v1 仅注册 reserved——remote_list 只读 master 注册表）。
pub const ACTION_APPROVAL_PENDING: &str = "approval.pending";
/// SSE 事件：远程 ask 上行登记（master Dashboard 卡片出现）。
pub const EVENT_REQUESTED: &str = "approval-remote-requested";
/// SSE 事件：远程 ask 关闭（worker 裁决/超时/回源摘除）。
pub const EVENT_RESOLVED: &str = "approval-remote-resolved";

/// 一条等待远程批复的 ask（master 侧注册表条目）。
#[derive(Debug, Clone)]
pub struct RemoteAskEntry {
    /// 完整 request_id（裁决下发用——worker respond 按完整 id 摘表）。
    pub request_id: String,
    /// ask 发起节点（RPC 下发目标）。
    pub worker_node: String,
    pub operation: String,
    pub target: String,
    pub risk_level: String,
    pub reason: String,
    /// 关联看板单号（worker 侧从 cluster 会话键反查；非看板 ask = None）。
    pub issue_number: Option<i64>,
    /// worker 侧等待窗口（超时自动拒绝）——注册表 TTL 据此 + 60s 余量。
    pub timeout_secs: u64,
    pub created_at: std::time::Instant,
}

/// master 侧远程审批注册表：短号 → 条目。短号生成与 channel_approval /
/// WebApprovalManager 展示语义对齐（request_id 前 8 位；键冲突逐级加长
/// 12/16/24/32/全长）。`resolve` 同时接受短号与完整 id。
pub struct RemoteAskRegistry {
    inner: Mutex<HashMap<String, RemoteAskEntry>>,
}

impl RemoteAskRegistry {
    /// TTL 惰性清理（register/resolve/snapshot 共用；条目生命周期 =
    /// timeout + 60s 余量——ask_closed 上行丢失时的兜底过期）。
    fn sweep(map: &mut HashMap<String, RemoteAskEntry>) {
        map.retain(|_, e| {
            e.created_at.elapsed()
                < std::time::Duration::from_secs(e.timeout_secs.saturating_add(60))
        });
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, RemoteAskEntry>> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 登记一条远程 ask，返回展示用短号。同 request_id 重登 = 覆盖旧条目
    /// 返回同键（worker fire-and-forget 无重试；防御性幂等）。
    pub fn register(&self, entry: RemoteAskEntry) -> String {
        let mut map = self.lock();
        Self::sweep(&mut map);
        let id = entry.request_id.clone();
        let mut len = 8usize.min(id.len()).max(1);
        let key = loop {
            let candidate = id[..len].to_string();
            match map.get(&candidate) {
                Some(existing) if existing.request_id == id => break candidate,
                Some(_) if len < id.len() => len = (len * 2).min(id.len()),
                // 空位，或键被别的请求占满全长（uuid 撞满长≈不可能）：
                // 都落此键（后者覆盖）。
                Some(_) | None => break candidate,
            }
        };
        map.insert(key.clone(), entry);
        key
    }

    /// 查条目（短号或完整 id 均可），带回实际注册键（键随冲突加长后与
    /// 用户敲的完整 id 可能不同）；过期条目顺带清理。
    pub fn resolve_with_key(&self, id: &str) -> Option<(String, RemoteAskEntry)> {
        let mut map = self.lock();
        Self::sweep(&mut map);
        if let Some(e) = map.get(id) {
            return Some((id.to_string(), e.clone()));
        }
        map.iter()
            .find(|(_, e)| e.request_id == id)
            .map(|(k, e)| (k.clone(), e.clone()))
    }

    /// 查条目（短号或完整 id 均可）；过期条目顺带清理。
    pub fn resolve(&self, id: &str) -> Option<RemoteAskEntry> {
        self.resolve_with_key(id).map(|(_, e)| e)
    }

    /// 按短号摘除（转发成功后调用——一次裁决一次摘除）。
    pub fn remove(&self, short_id: &str) -> Option<RemoteAskEntry> {
        self.lock().remove(short_id)
    }

    /// 按完整 request_id 摘除，返回被摘条目的短号（ask_closed 上行的
    /// master 侧同步关卡）。条目不存在 = None（重复上行幂等）。
    pub fn remove_by_request_id(&self, request_id: &str) -> Option<String> {
        let mut map = self.lock();
        let key = map
            .iter()
            .find(|(_, e)| e.request_id == request_id)
            .map(|(k, _)| k.clone())?;
        map.remove(&key).map(|_| key)
    }

    /// 存活条目快照（remote_list；TTL 惰性清理 + 剩余秒数升序——快过期的
    /// 卡片排前面）。
    pub fn snapshot(&self) -> Vec<serde_json::Value> {
        let mut map = self.lock();
        Self::sweep(&mut map);
        let mut rows: Vec<_> = map.iter().map(|(k, e)| entry_json(k, e)).collect();
        rows.sort_by_key(|r| {
            r.get("remaining_secs")
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
        });
        rows
    }
}

/// 进程级注册表（零装配接线：worker 上行首个 ask_register 才写入）。
pub fn registry() -> &'static RemoteAskRegistry {
    static REG: OnceLock<RemoteAskRegistry> = OnceLock::new();
    REG.get_or_init(|| RemoteAskRegistry {
        inner: Mutex::new(HashMap::new()),
    })
}

/// 回执语法解析（与 channel_approval::parse_reply / loop 侧同源格式：
/// `/approve <id>` / `/deny <id>`，id 不含空白）。返回 (是否批准, id)。
/// 本函数是格式单一真相源——channel_approval 委托此处。
pub fn parse_approval_reply(content: &str) -> Option<(bool, &str)> {
    let trimmed = content.trim();
    let (approved, rest) = if let Some(rest) = trimmed.strip_prefix("/approve ") {
        (true, rest)
    } else if let Some(rest) = trimmed.strip_prefix("/deny ") {
        (false, rest)
    } else {
        return None;
    };
    let id = rest.trim();
    (!id.is_empty() && !id.chars().any(char::is_whitespace)).then_some((approved, id))
}

/// 廉价前缀预检（relay 挂点的门卫——绝大多数评论零分配直接排除）。
pub fn looks_like_approval_reply(content: &str) -> bool {
    let t = content.trim_start();
    t.starts_with("/approve ") || t.starts_with("/deny ")
}

/// 条目 → 快照/SSE 行（short_id + 实时剩余秒数）。
pub fn entry_json(short_id: &str, e: &RemoteAskEntry) -> serde_json::Value {
    let elapsed = e.created_at.elapsed().as_secs();
    serde_json::json!({
        "short_id": short_id,
        "request_id": e.request_id,
        "worker": e.worker_node,
        "operation": e.operation,
        "target": e.target,
        "risk_level": e.risk_level,
        "reason": e.reason,
        "issue_number": e.issue_number,
        "timeout_secs": e.timeout_secs,
        "age_secs": elapsed,
        "remaining_secs": e.timeout_secs.saturating_sub(elapsed),
    })
}

/// SSE 发布口（EventHub 未安装时静默跳过——单测/无 SSE 场景零开销）。
pub fn publish_requested(data: serde_json::Value) {
    crate::chat_event_log::publish_global_event(EVENT_REQUESTED, data);
}

/// SSE 发布口（同 [`publish_requested`]）。
pub fn publish_resolved(data: serde_json::Value) {
    crate::chat_event_log::publish_global_event(EVENT_RESOLVED, data);
}

/// 裁决下发 worker（RPC [`ACTION_APPROVAL_RESPOND`]；**always 钉死 false**
/// ——规则记忆只允许 worker 本地 Dashboard 显式批准落盘，防远程误批产生
/// 永久放行）。Ok(()) = worker 确认送达；Err = 传输/超时/worker 报错
/// （调用方留痕，注册表条目保留可重试）。
pub async fn send_verdict(
    cluster: &nemesis_cluster::cluster::Cluster,
    worker: &str,
    request_id: &str,
    approved: bool,
    note: Option<&str>,
) -> Result<(), String> {
    let rpc = cluster
        .rpc_client_arc()
        .ok_or_else(|| "RPC client not available".to_string())?;
    let request = nemesis_cluster::rpc_types::RPCRequest {
        id: uuid::Uuid::new_v4().to_string(),
        action: nemesis_cluster::rpc_types::ActionType::Custom(ACTION_APPROVAL_RESPOND.to_string()),
        payload: serde_json::json!({
            "request_id": request_id,
            "approved": approved,
            "note": note,
        }),
        source: cluster.node_id().to_string(),
        target: Some(worker.to_string()),
    };
    let resp = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        rpc.call_with_timeout(worker, request, std::time::Duration::from_secs(10)),
    )
    .await
    .map_err(|_| format!("approval respond timeout (worker {worker}, 10s)"))?
    .map_err(|e| format!("RPC call to {worker} failed: {e}"))?;
    if let Some(err) = resp.error {
        return Err(format!("worker {worker} rejected verdict: {err}"));
    }
    Ok(())
}

/// 看板评论回源（board.rs comment.add 与 board_bus post_discussion_core
/// 两个落库点的共用 relay）：解析 `/approve|/deny <短号>`，短号在注册表
/// 才动作（杂散同形文本零影响）；下发成功 = 注册表摘除 + SSE 关卡 +
/// 系统评论留痕；失败 = 系统评论留痕（条目保留，可重发）。返回是否动作
/// （true = 本条评论被解释为远程批复）。
pub async fn try_remote_approval_reply(
    store: &nemesis_board::BoardStore,
    cluster: &nemesis_cluster::cluster::Cluster,
    issue_id: i64,
    content: &str,
) -> bool {
    let Some((approved, id)) = parse_approval_reply(content) else {
        return false;
    };
    let Some((short_id, entry)) = registry().resolve_with_key(id) else {
        return false;
    };
    let worker = entry.worker_node.clone();
    let receipt = match send_verdict(cluster, &worker, &entry.request_id, approved, None).await {
        Ok(()) => {
            registry().remove(&short_id);
            publish_resolved(serde_json::json!({
                "request_id": entry.request_id,
                "short_id": short_id,
                "decision": if approved { "approved" } else { "denied" },
                "source": "board_comment",
            }));
            format!(
                "✅ 批复已转发至节点 {worker}（{short_id} {}）。",
                if approved { "批准" } else { "拒绝" }
            )
        }
        Err(e) => format!("❌ 批复转发失败（{short_id} → {worker}）：{e}。条目保留，可重发。"),
    };
    if let Err(e) = store.add_comment(nemesis_board::NewComment {
        issue_id,
        author: nemesis_board::Actor::system("board"),
        content: receipt,
        parent_id: None,
        ctype: nemesis_board::CommentType::System,
    }) {
        tracing::warn!("[RemoteApproval] relay receipt comment failed: {e}");
    }
    true
}

#[cfg(test)]
mod tests;
