//! 工具事件广播 hook（devtool-upgrade）。
//!
//! 实现 [`ToolHook`] 的 around 包装：进入链前发 [`AgentEvent::ToolStarted`]，
//! `next()` 返回后发 [`AgentEvent::ToolFinished`]——纯观察者，不改写结果、
//! 不拦截。事件经 `tokio::sync:broadcast` 通道交给 gateway 侧
//! `SharedResources.agent_event_tx`，`nemesis-web` 的 pump 订阅后路由到
//! Dashboard WS push（`{type:"push", cmd:"tool_event"}`）+ EventHub（SSE）。
//!
//! # 布点语义（诚实边界）
//!
//! around 链在 security 闸 / pre hooks **之后**构造——被安全层或 pre hook
//! 拦停的调用根本不进链，因此**不产生事件**（什么都没执行，如实沉默）。
//! executor 分离（RemoteExecutorTool）与 MCP 工具都走同一条
//! `handle_tool_call` 调度路径，事件自动覆盖。
//!
//! ok 判定沿用 Forge 的启发式（loop.rs:5805 附近先例）：结果不含
//! `SECURITY BLOCKED` 且不含 `Tool error:` 即视为成功。around 链只能看到
//! String 形态的最终结果，这是该层能拿到的全部信息。

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use nemesis_types::agent::AgentEvent;
use tokio::sync::broadcast;

use crate::hooks::{HookToolCall, ToolHook};

/// args 预览字符上限（事件负载防膨胀；中文等多字节字符按 char 截断）。
pub const ARGS_PREVIEW_CAP: usize = 200;
/// result 预览字符上限。
pub const RESULT_PREVIEW_CAP: usize = 1024;

/// 按字符数截断（byte 切片会在多字节字符边界 panic——见项目 str-slice 教训）。
/// pub(crate)：estop 中断面（loop.rs `emit_tool_interrupted`）的补发预览走
/// 同一截断，两处预览口径不漂移。
pub(crate) fn truncate_chars(s: &str, cap: usize) -> String {
    if s.chars().count() <= cap {
        return s.to_string();
    }
    let mut out: String = s.chars().take(cap).collect();
    out.push('…');
    out
}

/// 把工具调度事件发布到 broadcast 通道的观察 hook。
/// 同时喂 Work 时间线累加器（与 tx 发射同点同源——落盘真相源
/// 与实时观察面同一发射点，无第二份转换逻辑；recorder 未 begin = 忽略）。
/// 同点喂 wip 侧车（批二 A）：turn 内执行现场逐事件落盘，registry 未注册
/// 的 session_key（detached 子代理/集群 worker）= 静默忽略零写。
pub struct ToolEventHook {
    tx: broadcast::Sender<AgentEvent>,
    work: crate::work_timeline::WorkRecorder,
    /// BUG-20261118-090：result 预览净化器（factory 经
    /// [`crate::r#loop::AgentLoop::output_preview_sanitizer`] 注入，与
    /// dispatch 尾部出侧复扫同源同 plugin）。`None`（测试 / 无安全构建）
    /// = 预览原样截断，行为不变。只作用于**预览副本**——返回给 dispatch
    /// 的原始 result 不经此处（权威复扫在 dispatch 尾，语义不变）。
    sanitize: Option<std::sync::Arc<dyn Fn(&str, &str) -> String + Send + Sync>>,
    /// wip 侧车共享句柄——必须与 AgentLoop 同一实例（loop 注册、hook 追加；
    /// 两实例 = 注册表不互通 = 零写）。`new_with_sanitizer` 第 4 参强制
    /// 生产装配点显式传值（防 FactoryConfig stream_mode 式静默漏接——
    /// 编译器兜底）。
    wip: crate::wip_journal::WipJournal,
}

impl ToolEventHook {
    pub fn new(
        tx: broadcast::Sender<AgentEvent>,
        work: crate::work_timeline::WorkRecorder,
    ) -> Self {
        Self {
            tx,
            work,
            sanitize: None,
            wip: crate::wip_journal::WipJournal::default(),
        }
    }

    /// 带出侧复扫同源净化器的构造形态（生产装配用；见字段注释）。
    /// `wip` 必须传 `agent_loop.wip_journal()`（同一 Arc 实例）。
    pub fn new_with_sanitizer(
        tx: broadcast::Sender<AgentEvent>,
        work: crate::work_timeline::WorkRecorder,
        sanitize: Option<std::sync::Arc<dyn Fn(&str, &str) -> String + Send + Sync>>,
        wip: crate::wip_journal::WipJournal,
    ) -> Self {
        Self {
            tx,
            work,
            sanitize,
            wip,
        }
    }
}

#[async_trait]
impl ToolHook for ToolEventHook {
    fn name(&self) -> String {
        "tool-event".to_string()
    }

    async fn around_tool_use(
        &self,
        call: HookToolCall,
        next: Arc<
            dyn Fn(
                    HookToolCall,
                )
                    -> std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send>>
                + Send
                + Sync,
        >,
    ) -> String {
        // call_id 改为消费 dispatch 传入的稳定
        // id（与 turn WAL `call_id_for` 同一派生点）——急停中断时 dispatch
        // 能按同 id 补发 interrupted ToolFinished（此前这里自生成 uuid，与
        // WAL id / 中断补发永不相等，UI 卡片只能悬空）。`None`（测试/非
        // dispatch 调用方）回退自生成 uuid，行为不变。
        let call_id = call
            .call_id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let session_key = call.session_key.clone();
        let chat_id = call.chat_id.clone();
        let tool = call.name.clone();

        // 无订阅者时 send 返回 Err——观察者通道空转是常态，静默忽略。
        let args_preview = truncate_chars(&call.arguments, ARGS_PREVIEW_CAP);
        self.work
            .tool_started(&session_key, &call_id, &tool, &args_preview);
        // wip 侧车：工具调度事实落盘（registry 未注册 = 静默忽略）。
        self.wip.append(
            &session_key,
            crate::wip_journal::WipEvent::ToolStart {
                ts: crate::wip_journal::WipEvent::now_ms(),
                call_id: call_id.clone(),
                tool: tool.clone(),
                args: args_preview.clone(),
            },
        );
        let _ = self.tx.send(AgentEvent::ToolStarted {
            session_key: session_key.clone(),
            chat_id: chat_id.clone(),
            call_id: call_id.clone(),
            tool: tool.clone(),
            args_preview,
        });

        // 优先用 dispatch 入口时刻计时（含安全段
        // 审批等待），与守卫重放池的 Local.md 时长同源；`None`（测试/非
        // dispatch 调用方）回退自身计时。`next(call)` 消费 call，先取出
        // Copy 的 Instant 再 await。
        let fallback_start = Instant::now();
        let dispatch_started = call.dispatch_started.unwrap_or(fallback_start);
        let result = next(call).await;
        let duration_ms = dispatch_started.elapsed().as_millis() as u64;

        // 成败判定换 [`classify_result`] 单一权威定义——
        // 按工具类选精确判别器（exec = Exit code: 前缀判据，非零退出不再
        // 被标成功）；拒绝族（⛔/Plan/hidden/安全拦截）= None → ok:false
        // （拒绝不是成功，旧 contains 启发式把 ⛔ ESTOP 拒绝标成了 ok）。
        let ok = crate::checks_capture::classify_result(&tool, &result).unwrap_or(false);

        // BUG-20261118-090：预览必须过出侧复扫同源净化器——本 hook 在
        // dispatch 尾部复扫（tool_dispatch rescan 臂）**之前**拿到工具
        // 结果，原样截断会把复扫前的明文凭据/敏感信息带进 work 时间线
        // （session_log jsonl 持久化）与 Dashboard WS/SSE 事件面，而模型
        // 上下文拿到的已是脱敏后文本——观测面与上下文口径分裂。净化器
        // 缺席（测试/无安全构建）= 原样截断，行为不变。
        let preview_source = match &self.sanitize {
            Some(f) => f(&tool, &result),
            None => result.clone(),
        };

        let result_preview = truncate_chars(&preview_source, RESULT_PREVIEW_CAP);
        self.work
            .tool_finished(&session_key, &call_id, ok, duration_ms, &result_preview);
        // wip 侧车：工具结束事实落盘（与 work 时间线同一预览口径——
        // 已净化 + 已截断，不落复扫前原文）。
        self.wip.append(
            &session_key,
            crate::wip_journal::WipEvent::ToolEnd {
                ts: crate::wip_journal::WipEvent::now_ms(),
                call_id: call_id.clone(),
                ok,
                dur_ms: duration_ms,
                result: result_preview.clone(),
            },
        );
        let _ = self.tx.send(AgentEvent::ToolFinished {
            session_key,
            chat_id,
            call_id,
            tool,
            duration_ms,
            ok,
            result_preview,
        });

        result
    }
}

#[cfg(test)]
mod tests;
