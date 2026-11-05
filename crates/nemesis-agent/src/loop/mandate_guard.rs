//! b mandate 执行面守卫的 AgentLoop 接线（槽 + 轮顶检）。类型与纯裁决
//! 在 [`crate::mandate_guard`]；本文件是私有字段 glue（与 token_budget.rs
//! 同布局：私有字段访问需要 `r#loop` 模块后代身份）。dispatch 调用闸在
//! `tool_dispatch.rs`（estop 之后），轮顶检在 run_loop 骨架
//! `check_session_budget` 块之后。
use super::*;

impl AgentLoop {
    /// 装配守卫（nemesisbot cluster_agent 消费：任务带 mandate 才装配；
    /// set/take 对称——work queue 串行使 loop 级槽安全，与 cancel token 的
    /// per-session DashMap 不同：mandate 授权书绑定单个任务而非会话族）。
    /// token 基线在本时刻快照（session ledger delta 口径）。
    pub fn set_mandate_guard(
        &self,
        enforcement: nemesis_types::cluster::MandateEnforcement,
        session_key: &str,
    ) {
        let baseline = self.mandate_tokens_used(session_key);
        let task_id = enforcement.task_id.clone();
        *self.mandate_guard.write() = Some(std::sync::Arc::new(
            crate::mandate_guard::MandateGuard::new(enforcement, baseline),
        ));
        info!(
            session_key,
            task_id = %task_id,
            "[AgentLoop] mandate guard attached (task authorization active)"
        );
    }

    /// 摘除守卫（run 结束各臂共用；违规记录由调用方在摘除后经
    /// [`crate::mandate_guard:MandateGuard::violation_report`] 消费）。
    pub fn take_mandate_guard(&self) -> Option<std::sync::Arc<crate::mandate_guard::MandateGuard>> {
        self.mandate_guard.write().take()
    }

    /// session 用量合计（input+output；data_store 缺席/查询失败 = 0——
    /// 诚实下界：预算闸宁可晚炸不误炸）。
    fn mandate_tokens_used(&self, session_key: &str) -> u64 {
        let Some(ds) = self.data_store.as_deref() else {
            return 0;
        };
        let Ok(agg) = ds.aggregate_session_usage(session_key) else {
            return 0;
        };
        agg.input_tokens.max(0) as u64
    }

    /// 轮顶检器官（run_loop `check_session_budget` 块之后，每轮开头）。
    ///
    /// 判定序：时限 → token 预算 → 调用次数；任一超限 = 终局
    /// `Some(AgentEvent::Error(...))`——文案以
    /// [`crate::mandate_guard:MANDATE_VIOLATION_MARKER`] 开头，worker
    /// `classify_terminal_failure` 识别为 `mandate_violation`，master
    /// board_review 按能力类失败处理。提示同时入 instance 历史（下一会话
    /// 上下文可见停机原因，-B 同款形态）。guard 缺席 = 零开销直通。
    pub(crate) fn check_mandate_round(
        &self,
        instance: &AgentInstance,
        context: &RequestContext,
    ) -> Option<AgentEvent> {
        let guard = self.mandate_guard.read().clone()?;
        let now = crate::mandate_guard::now_unix();
        let used = self.mandate_tokens_used(&context.session_key);
        let verdict = guard
            .check_expiry(now)
            .and_then(|_| guard.check_token_budget(used))
            .and_then(|_| guard.check_tool_call_budget());
        if let Err(detail) = verdict {
            warn!("[AgentLoop] mandate violation (terminal): {detail}");
            let notice = crate::mandate_guard::terminal_notice(&detail);
            instance.add_assistant_message(&notice, Vec::new(), None);
            return Some(AgentEvent::Error(context.format_rpc_message(&notice)));
        }
        None
    }
}
