//! 会话维护：SessionMaintenance/parse_maintenance_command、parse_user_dispatch、build_dispatch_snapshot_messages、handle_maintenance、process_user_dispatch、clear_session。
//!
//! P1 自 `loop.rs` 物理搬迁（docs/PLAN/2026-09-23_agentloop-god-object-decomposition.md §3.2）；语义零变化。
use super::prelude::*;
use super::*;

/// E6: 会话维护命令种类（`/compact` 手动压缩 / `/clear` 清空会话）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMaintenance {
    /// 手动 compaction：推进摘要覆盖、收缩逐字尾巴（`force_compression`）。
    Compact,
    /// 清空会话历史（chat_log 截断 + SessionStore 清除，key 保留可用）。
    Clear,
}

/// E6: parse a session-maintenance slash command (`/compact` / `/clear`).
/// Returns (kind, pending receipt). Extra args are ignored (no-arg commands);
/// `None` for anything else — the caller falls through to normal handling.
pub(crate) fn parse_maintenance_command(content: &str) -> Option<(SessionMaintenance, String)> {
    let name = content.split_whitespace().next()?;
    match name {
        "/compact" => Some((SessionMaintenance::Compact, "⏳ 正在压缩会话…".to_string())),
        "/clear" => Some((SessionMaintenance::Clear, "⏳ 正在清空会话…".to_string())),
        _ => None,
    }
}

/// K4 (devtool-upgrade 阶段 7): 用户直发远程编码任务派发语法——
/// `/build <task> repo:<node>`（`/plan` 前缀同接受）。
///
/// 解析规则（可预测优先）：
/// - 前缀必须是 `/build ` 或 `/plan `（带参数形态）；裸 `/build`（F1 模式
///   切换）因无尾随空格天然不匹配，互不干扰；
/// - 目标节点 = **最后一个** `repo:` 前缀的空白分隔 token；其余部分为
///   任务文本；
/// - 任务文本为空 / 无 `repo:` token / token 后为空 → `None`（落普通轮，
///   不猜不做）。
///
/// 返回 (task_text, node_id)。
pub(crate) fn parse_user_dispatch(content: &str) -> Option<(String, String)> {
    let trimmed = content.trim();
    let rest = trimmed
        .strip_prefix("/build ")
        .or_else(|| trimmed.strip_prefix("/plan "))?
        .trim();
    if rest.is_empty() {
        return None;
    }
    let tokens: Vec<&str> = rest.split_whitespace().collect();
    let last = tokens.last()?;
    let node_id = last.strip_prefix("repo:").filter(|s| !s.is_empty())?;
    let task_text = tokens[..tokens.len() - 1].join(" ");
    if task_text.is_empty() {
        return None;
    }
    Some((task_text, node_id.to_string()))
}

/// K4: 构造用户直派发的自足续行快照消息。
///
/// 与 LLM 发起 `cluster_rpc` 的 `__ASYNC__` 路径同构，但快照不来自
/// instance 历史（用户派发没有 LLM 轮），而是直接合成最小合法序列：
/// `[user(任务文本), assistant(tool_calls: [同一 tc_id])]`。完成回调到达时
/// `merge_real_tool_result` 找不到既有 tool 槽位 → push 到末尾，得到
/// `[user, assistant(tc), tool(result)]` ——严格 provider 接受的标准序列
/// （tool 消息紧跟其 assistant tool_calls 消息）。续行 LLM 以自己的口吻
/// 向用户呈现远程执行结果，还可继续多步工具链。
pub(crate) fn build_dispatch_snapshot_messages(
    task_text: &str,
    target_node: &str,
    tool_call_id: &str,
) -> Vec<LlmMessage> {
    let args = serde_json::json!({
        "target": target_node,
        "message": task_text,
    })
    .to_string();
    vec![
        LlmMessage {
            role: "user".to_string(),
            content: task_text.to_string(),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
            images: Vec::new(),
        },
        LlmMessage {
            role: "assistant".to_string(),
            content: String::new(),
            tool_calls: Some(vec![ToolCallInfo {
                id: tool_call_id.to_string(),
                name: "cluster_rpc".to_string(),
                arguments: args,
            }]),
            tool_call_id: None,
            reasoning_content: None,
            images: Vec::new(),
        },
    ]
}

impl AgentLoop {
    /// Dispatch a maintenance command to its implementation (shared by the
    /// serial tail and the spawned pump task).
    pub(crate) async fn handle_maintenance(
        &self,
        kind: SessionMaintenance,
        session_key: &str,
    ) -> String {
        match kind {
            SessionMaintenance::Compact => match self.compact_session(session_key).await {
                Ok(receipt) => receipt,
                Err(e) => format!("⚠ 压缩未完成：{e}"),
            },
            SessionMaintenance::Clear => match self.clear_session(session_key).await {
                Ok(receipt) => receipt,
                Err(e) => format!("⚠ 清空未完成：{e}"),
            },
        }
    }

    // -----------------------------------------------------------------------
    // K4 (devtool-upgrade 阶段 7): 用户直发远程编码任务派发
    // -----------------------------------------------------------------------

    /// 处理 `/build <task> repo:<node>` 用户直派发（serial tail 与 spawned
    /// pump task 共用）。会话已在 gate 获取；本 fn 结束前负责释放。
    ///
    /// 流程：
    /// ① ⏳ 收据先发（ACK 等待可达分钟级，不晾用户）；
    /// ② 会话一致性——user 行落 chat_log + session store（原命令原文），
    ///    续行端只追加 assistant 行，两半对称；
    /// ③ 合成 `cluster_rpc` 工具调用走 `handle_tool_call` 全管线——estop /
    ///    hidden / Plan 闸 / 安全 8 层 / guardian 全部生效（9 层安全零降级，
    ///    这是直接调工具对象被否决的原因）；
    /// ④ `__ASYNC__` ACK → 存自足续行快照（`build_dispatch_snapshot_messages`
    ///    合成最小合法序列），完成回调复用 `handle_cluster_continuation`
    ///    全套（续行 LLM / 持久化 / 回原通道 / G5 崩溃恢复）；
    /// ⑤ 同步结果或 Err（集群未启用 / 节点不存在 / 自调用守卫）→ 直接回复。
    pub(crate) async fn process_user_dispatch(
        &self,
        msg: &nemesis_types::channel::InboundMessage,
        task_text: &str,
        node_id: &str,
        session_key: &str,
    ) {
        // ① 收据先发。
        let receipt = format!("⏳ 已把编码任务派发给节点 {node_id}，完成后自动回复结果。");
        self.finish_message(msg, receipt, None, false).await;

        // ② 会话一致性：user 行落盘（原命令原文——派发上下文可追溯）。
        // SB（2026-09-17）：同正常轮——首行落盘 = 会话物化 → 发布事件。
        let log_existed = Self::session_log_exists_before_append(session_key);
        crate::chat_log::append_chat_log_meta(
            session_key,
            "user",
            &msg.content,
            &crate::chat_log::ChatLogMeta {
                model: None,
                cron_job_id: None,
                cron_job_name: None,
                images: &[],
                file_changes: &[],
                checkpoint_turn: None,
            },
        );
        if !log_existed {
            self.emit_session_created(session_key);
        }
        if let Some(ref store) = self.session_store {
            store.add_message(session_key, "user", &msg.content);
        }

        // ③ 合成工具调用 → 全管线提交。
        let tc_id = format!("ud-{}", uuid::Uuid::new_v4().simple());
        let tool_call = ToolCallInfo {
            id: tc_id.clone(),
            name: "cluster_rpc".to_string(),
            arguments: serde_json::json!({
                "target": node_id,
                "message": task_text,
            })
            .to_string(),
        };
        let context = RequestContext::new(&msg.channel, &msg.chat_id, &msg.sender_id, session_key);
        let result = self.handle_tool_call(&tool_call, &context).await;

        // ④ ACK → 存自足续行快照。
        if let Some(rest) = result.strip_prefix("__ASYNC__:") {
            let parts: Vec<&str> = rest.splitn(4, ':').collect();
            if parts.len() >= 2 {
                let task_id = parts[0];
                let target_id = parts[1];
                if let Some(ref mgr) = self.continuation_manager {
                    let messages = build_dispatch_snapshot_messages(task_text, node_id, &tc_id);
                    // G5：对端 ID 随快照落盘——A 侧重启后恢复轮询知道问谁。
                    mgr.save_continuation_with_images(
                        task_id,
                        messages,
                        &tc_id,
                        &msg.channel,
                        &msg.chat_id,
                        session_key,
                        target_id,
                        &[],
                    )
                    .await;
                    info!(
                        "[AgentLoop] User dispatch accepted: task_id={}, target={}, session={}",
                        task_id, target_id, session_key
                    );
                } else {
                    // 无续行管理器（测试/独立模式）：结果无法回灌，诚实告知。
                    warn!(
                        "[AgentLoop] User dispatch ACK but no continuation manager; result cannot be delivered"
                    );
                    self.finish_message(
                        msg,
                        "⚠ 任务已提交但本实例不支持续行回灌（无 continuation manager），结果无法自动回复。".to_string(),
                        None,
                        true,
                    )
                    .await;
                }
                self.release_session(session_key);
                return;
            }
            // 格式异常（段数不足）→ 落到下方按普通文本回复。
        }

        // ⑤ 同步结果或错误：直接回复（handle_tool_call 把错误编码在返回串）。
        self.finish_message(msg, result, None, true).await;
        self.release_session(session_key);
    }

    /// E6: 手动清空会话入口。chat_log 截断（jsonl-first，令 rebuild 路径
    /// 无旧内容可复活——sessions.clear 同纪律）+ SessionStore 清除（内存
    /// 条目 + 磁盘 json，下一回合 get_or_create 重建空会话）。key 保留，
    /// 会话继续可用。实例按回合从 store 重建，无驻留内存态需要清。
    pub async fn clear_session(&self, session_key: &str) -> Result<String, String> {
        crate::chat_log::clear_chat_log(session_key);
        if let Some(ref store) = self.session_store {
            store.clear_session(session_key);
        }
        info!("[AgentLoop] Manual clear for {}", session_key);
        Ok("✓ 已清空会话历史".to_string())
    }
}
