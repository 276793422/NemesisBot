//! M5（2026-10-10 集群无人值守硬化）— 跨节点批复桥（worker 侧）。
//!
//! worker 执行看板任务时的 ABAC ask（auditor require_approval）卡在 worker
//! 本地 Dashboard；本模块把 ask 上行登记到 master（nb_bus ns="approval"
//! op="ask_register"），master 侧据此提供 Dashboard 代理批复与看板评论回源
//! 两个入口（真相源 `nemesis_web::remote_approval`）。裁决真相源仍在本节点
//! pending 表（300s 超时语义不变）：本节点裁决生效后经 [`RemoteNotifyResponder`]
//! 上行 op="ask_closed" 同步 master 关卡。
//!
//! 上行 fire-and-forget 单次尝试（与看板注记 `spawn_board_ask_notice` 同
//! 语义——旁路不阻塞 ask 阻塞等待）；失败不重试：ask_register 丢失 = master
//! 无卡（本地 Dashboard 仍可批复，评审照常）；ask_closed 丢失的卡面残留由
//! master 注册表 TTL（timeout + 60s 余量）自然过期。本节点即 master（无
//! 在线 coordinator ≠ self）= 上行静默跳过。
//!
//! 装配（唯一消费点 = gateway runtime.rs 的 security cfg 审批装配块）：
//! `set_board_notifier` 闭包扩展调 [`spawn_remote_ask_register`]；
//! `set_approval_responder` 换装饰器；RPC 下行 handler
//! `approval.respond`/`approval.pending` 直接捕获内层 web_mgr（master 转发
//! 的批复不再二次上行）。
#![cfg(all(feature = "cluster", feature = "security"))]

use std::sync::Arc;

use nemesis_cluster::cluster::Cluster;
use nemesis_types::agent::ApprovalResponder;

use crate::channel_approval::BoardAskNotice;

/// ask 上行登记（ns="approval" op="ask_register"）。在 [`BoardAskNotice`]
/// 观察者闭包内调用（runtime.rs 装配点）；无 tokio 运行时（纯同步测试
/// 环境）诚实记日志跳过。
pub(crate) fn spawn_remote_ask_register(cluster: Arc<Cluster>, notice: &BoardAskNotice) {
    let payload = serde_json::json!({
        "request_id": notice.request_id,
        "operation": notice.operation,
        "target": notice.target,
        "risk_level": notice.risk_level,
        // 策略理由截断（防长策略刷屏；200 字符足够定位，与注记 120 同理）。
        "reason": truncate_chars(&notice.reason, 200),
        "timeout_secs": notice.timeout_secs,
        "worker": cluster.node_id(),
        "issue_number": crate::cluster_agent::issue_number_from_cluster_chat_id(&notice.chat_id),
    });
    spawn_ask_envelope(cluster, "ask_register", payload);
}

/// 裁决摘除上行（ns="approval" op="ask_closed"）。[`RemoteNotifyResponder`]
/// 在内层 respond 成功后调用。
pub(crate) fn spawn_remote_ask_closed(
    cluster: Arc<Cluster>,
    request_id: &str,
    approved: bool,
    note: Option<&str>,
) {
    let payload = serde_json::json!({
        "request_id": request_id,
        "approved": approved,
        "note": note,
    });
    spawn_ask_envelope(cluster, "ask_closed", payload);
}

fn spawn_ask_envelope(cluster: Arc<Cluster>, op: &'static str, body: serde_json::Value) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        tracing::debug!("[RemoteApproval] {op}: no tokio runtime, upload skipped");
        return;
    };
    handle.spawn(async move {
        if let Err(e) = post_ask_envelope(cluster, op, &body).await {
            // 旁路语义：单次尝试不重试（master TTL 兜底 / 本地 Dashboard
            // 批复不受影响）。
            tracing::warn!("[RemoteApproval] {op} upload failed: {e}");
        }
    });
}

async fn post_ask_envelope(
    cluster: Arc<Cluster>,
    op: &str,
    body: &serde_json::Value,
) -> Result<(), String> {
    // 本节点即 master（或无集群）→ 没有别的 coordinator 可上行：静默跳过
    // （master 自身 ask 的本地批复不需要任何跨节点同步）。
    let Some(target) = find_online_coordinator(&cluster) else {
        tracing::debug!("[RemoteApproval] {op}: no online coordinator, upload skipped");
        return Ok(());
    };
    let rpc = cluster
        .rpc_client_arc()
        .ok_or_else(|| "rpc client unavailable（cluster 未启动）".to_string())?;
    let env = nemesis_cluster::envelope::Envelope {
        ns: nemesis_web::remote_approval::NS_APPROVAL.to_string(),
        op: op.to_string(),
        corr_id: uuid::Uuid::new_v4().to_string(),
        ..nemesis_cluster::envelope::Envelope::default()
    };
    let payload =
        nemesis_cluster::envelope::EnvelopeResponse::success(&env, body.clone()).to_json();
    let self_id = cluster.node_id().to_string();
    let resp = crate::cluster_agent::send_nb_bus(&rpc, &self_id, &target, payload).await?;
    if resp.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
        tracing::info!("[RemoteApproval] {op} delivered to {target}");
        Ok(())
    } else {
        Err(format!(
            "master rejected {op}: {}",
            resp.pointer("/error/message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error")
        ))
    }
}

/// 找一个在线 coordinator（排除本节点）。找不到 = None（本节点即 master
/// 或全离线；语义见 [`post_ask_envelope`]）。与 `post_board_ask_notice`
/// 同款谓词（role/id/online 三条件）。
fn find_online_coordinator(cluster: &Cluster) -> Option<String> {
    cluster
        .list_nodes()
        .into_iter()
        .find(|n| {
            n.base.role == nemesis_types::cluster::NodeRole::Coordinator
                && n.base.id != cluster.node_id()
                && n.is_online()
        })
        .map(|n| n.base.id)
}

/// [`ApprovalResponder`] 装饰器：内层裁决成功后上行 ask_closed（master
/// 远程审批卡同步关闭）。`pending` 直通。装配点 = runtime.rs 的
/// `set_approval_responder`（只包 Dashboard 卡通路；RPC 下行 handler 直接
/// 捕获内层 web_mgr——master 转发的批复不再二次上行）。
pub(crate) struct RemoteNotifyResponder {
    inner: Arc<dyn ApprovalResponder>,
    cluster: Arc<Cluster>,
}

impl RemoteNotifyResponder {
    pub(crate) fn new(inner: Arc<dyn ApprovalResponder>, cluster: Arc<Cluster>) -> Self {
        Self { inner, cluster }
    }
}

impl ApprovalResponder for RemoteNotifyResponder {
    fn respond(
        &self,
        request_id: &str,
        approved: bool,
        always: bool,
        note: Option<String>,
    ) -> Result<bool, String> {
        // 先裁决后上行：裁决失败（unknown/已处理）不产生摘除信号——
        // worker 本地 pending 才是真相源。
        let delivered = self
            .inner
            .respond(request_id, approved, always, note.clone())?;
        // 上行是旁路（fire-and-forget 单次尝试）——失败不影响已生效裁决。
        spawn_remote_ask_closed(self.cluster.clone(), request_id, approved, note.as_deref());
        Ok(delivered)
    }

    fn pending(&self) -> Vec<serde_json::Value> {
        self.inner.pending()
    }
}

/// 按字符数截断（char boundary 安全；超出补省略号。board_bus 同款）。
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let t: String = s.chars().take(max).collect();
    format!("{t}…")
}

#[cfg(test)]
mod tests;
