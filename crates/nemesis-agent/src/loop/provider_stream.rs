//! L10 provider 流式输出 lane（2026-10-13 三方学习 T3）。
//!
//! 两个组件：
//! - [`ProviderStreamSink`]：增量合并窗 + `AgentEvent::ProviderStream` 发射
//!   器。增量按 ≥64 字符或 ≥150ms 合并成帧（防 delta 洪泛刷爆 SSE/broadcast）；
//!   `start`（首增量前）→ `delta`* → `end`/`error` 收尾。事件经 agent_event_tx
//!   broadcast 发布，web pump 转易逝 SSE/WS 面（不入补拉环）。
//! - [`StreamingLlmProvider`]：`LlmProvider` 包装器——`chat()` 委托
//!   `inner.chat_streamed()` 并把增量送进 sink。inner 未覆写 `chat_streamed`
//!   （trait 缺省实现忽略回调）时行为与纯 `chat()` 一致，但在成功后合成一条
//!   全量增量——非流式 provider 也产生一次完整的 start/delta/end「流」，前端
//!   消费点无差别。错误路径发 `error` 帧（text=诊断截断 300 字符）。
//!
//! 接线：`call_llm_with_recovery` 顶部把 `active_provider` 包进本包装器（仅当
//! agent_event_tx 已注入且 `agents.provider_stream_events` 开启）——五个调用
//! 点（首捕 + 四个重试环）共享同一包装实例，重试=新流（seq 从 0 重开 +
//! start 帧重置前端缓冲）。cache warmer / compact / probe / forge 等**不经过**
//! call_llm_with_recovery 的调用路径不产生流事件（观测面聚焦主 turn）。
//!
//! 已知边界：上游超时路径（`chat_call_bounded` 内部 `tokio::time::timeout`
//! drop chat future）没有同步钩子通知 sink——该次流的 error 帧缺席，靠重试
//! 新流的 start 帧重置前端缓冲、终答回复覆盖收口；cancel/estop 两臂在
//! recovery.rs 显式补 error 帧（sink 引用另持一份）。观测 lane 语义，最终
//! 状态以正常 receive 回复帧为准。
#[cfg(not(target_os = "linux"))]
use super::prelude::*;
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

/// 合并窗字符阈值（chars 计——CJK 与 ASCII 一视同仁）。
const FLUSH_CHARS: usize = 64;
/// 合并窗时间阈值（毫秒）。
const FLUSH_MS: u64 = 150;
/// error 帧诊断文本截断长度。
const ERROR_TEXT_MAX: usize = 300;

/// 单次 chat() 调用流的内部状态（每次 `chat()` 重开 seq 空间）。
struct SinkState {
    seq: u64,
    buf: String,
    last_flush: Option<std::time::Instant>,
    deltas_emitted: bool,
    started: std::time::Instant,
}

/// L10：provider 流事件的发射 sink（每 `call_llm_with_recovery` 一个）。
pub(crate) struct ProviderStreamSink {
    /// 显式全路径：prelude 的 `AgentEvent` 是本 crate legacy 旧枚举，M1a
    /// 事件枚举在 nemesis-types（与 agent_event_tx 字段同型）。
    tx: tokio::sync::broadcast::Sender<nemesis_types::agent::AgentEvent>,
    session_key: String,
    chat_id: String,
    model: String,
    state: parking_lot::Mutex<SinkState>,
    /// 本 sink 是否发过任何增量（StreamingLlmProvider 合成全量增量的判据）。
    any_deltas: AtomicBool,
}

impl ProviderStreamSink {
    pub(crate) fn new(
        tx: tokio::sync::broadcast::Sender<nemesis_types::agent::AgentEvent>,
        session_key: String,
        chat_id: String,
        model: String,
    ) -> Self {
        Self {
            tx,
            session_key,
            chat_id,
            model,
            state: parking_lot::Mutex::new(SinkState {
                seq: 0,
                buf: String::new(),
                last_flush: None,
                deltas_emitted: false,
                started: std::time::Instant::now(),
            }),
            any_deltas: AtomicBool::new(false),
        }
    }

    /// 流式增量入口（同步回调——provider 消费循环内直呼）。
    pub(crate) fn on_delta(&self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.any_deltas.store(true, Ordering::Relaxed);
        let mut st = self.state.lock();
        if !st.deltas_emitted {
            // 首增量前发 start 帧（前端据此重置本地缓冲）。
            let seq = st.seq;
            st.seq += 1;
            st.deltas_emitted = true;
            drop(st);
            self.emit(seq, "start", "");
            st = self.state.lock();
        }
        st.buf.push_str(text);
        let due = st.buf.chars().count() >= FLUSH_CHARS
            || st
                .last_flush
                .map(|t| t.elapsed().as_millis() as u64 >= FLUSH_MS)
                .unwrap_or(true);
        if due {
            let seq = st.seq;
            st.seq += 1;
            let text = std::mem::take(&mut st.buf);
            st.last_flush = Some(std::time::Instant::now());
            drop(st);
            self.emit(seq, "delta", &text);
        }
    }

    /// 调用成功收尾：冲刷残余缓冲 + `end` 帧（elapsed 自 sink 创建起）。
    /// 空流（纯 tool-call 轮零文本、零增量）不发帧——避免前端面板噪音。
    pub(crate) fn finish(&self) {
        let mut st = self.state.lock();
        let pending = !st.buf.is_empty();
        if !pending && !self.any_deltas.load(Ordering::Relaxed) {
            return;
        }
        let seq = st.seq;
        st.seq += 1;
        let text = std::mem::take(&mut st.buf);
        let elapsed = st.started.elapsed().as_millis() as u64;
        drop(st);
        if pending {
            self.emit(seq, "delta", &text);
            self.emit_with_elapsed(seq + 1, "end", "", Some(elapsed));
        } else {
            self.emit_with_elapsed(seq, "end", "", Some(elapsed));
        }
    }

    /// 调用失败收尾：`error` 帧（text=诊断截断）。不补 end 帧——error 即终态。
    pub(crate) fn error(&self, msg: &str) {
        let st = self.state.lock();
        let seq = st.seq;
        let elapsed = st.started.elapsed().as_millis() as u64;
        drop(st);
        let text = nemesis_types::utils::truncate(msg, ERROR_TEXT_MAX);
        self.emit_with_elapsed(seq, "error", &text, Some(elapsed));
    }

    /// 本 sink 是否发过任何增量（含合成）。
    pub(crate) fn deltas_emitted(&self) -> bool {
        self.any_deltas.load(Ordering::Relaxed)
    }

    fn emit(&self, seq: u64, phase: &str, text: &str) {
        self.emit_with_elapsed(seq, phase, text, None);
    }

    fn emit_with_elapsed(&self, seq: u64, phase: &str, text: &str, elapsed_ms: Option<u64>) {
        // 注意：M1a 事件枚举是 `nemesis_types::agent::AgentEvent`（agent_event_tx
        // 同型）；prelude 的 `AgentEvent` 是本 crate legacy types.rs 的旧枚举。
        let _ = self
            .tx
            .send(nemesis_types::agent::AgentEvent::ProviderStream {
                session_key: self.session_key.clone(),
                chat_id: self.chat_id.clone(),
                seq,
                phase: phase.to_string(),
                text: text.to_string(),
                model: self.model.clone(),
                elapsed_ms,
            });
    }
}

/// L10：把 `chat()` 路由进流式 lane 的 provider 包装器（见模块头）。
pub(crate) struct StreamingLlmProvider {
    inner: Arc<dyn LlmProvider>,
    sink: std::sync::Arc<ProviderStreamSink>,
}

impl StreamingLlmProvider {
    pub(crate) fn new(
        inner: Arc<dyn LlmProvider>,
        sink: std::sync::Arc<ProviderStreamSink>,
    ) -> Self {
        Self { inner, sink }
    }
}

#[async_trait]
impl LlmProvider for StreamingLlmProvider {
    async fn chat(
        &self,
        model: &str,
        messages: Vec<LlmMessage>,
        options: Option<crate::types::ChatOptions>,
        tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        let sink = self.sink.clone();
        let on_delta = move |d: String| sink.on_delta(&d);
        match self
            .inner
            .chat_streamed(model, messages, options, tools, &on_delta)
            .await
        {
            Ok(resp) => {
                // inner 走了 trait 缺省实现（忽略回调）→ 合成一条全量增量，
                // 让非流式 provider 也产生一次完整「流」；流式 provider 已有
                // 增量时跳过（any_deltas 静默判据不区分来源——增量已在 sink）。
                if !self.sink.deltas_emitted() && !resp.content.is_empty() {
                    self.sink.on_delta(&resp.content);
                }
                self.sink.finish();
                Ok(resp)
            }
            Err(e) => {
                self.sink.error(&e);
                Err(e)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// AgentLoop 接线（sink 构造 + 每轮开关新鲜读）
// ---------------------------------------------------------------------------

impl AgentLoop {
    /// `agents.provider_stream_events` 新鲜读（config.json 唯一真相源；无
    /// config / 段缺失 → 默认 true——观测面默认开，wire 行为回落见
    /// `chat_streamed` 缺省实现）。
    pub(crate) fn provider_stream_events_enabled(&self) -> bool {
        self.read_config_json()
            .and_then(|v| {
                v.get("agents")
                    .and_then(|a| a.get("provider_stream_events"))
                    .and_then(|b| b.as_bool())
            })
            .unwrap_or(true)
    }

    /// L10：为本轮 LLM 调用构造流 sink。前置双闸：agent_event_tx 已注入
    /// （standalone / 未接线 → None，零开销）+ `agents.provider_stream_
    /// events` 开启（每轮新鲜读，与 tier/config_watch 同语义——dashboard
    /// 或 CLI 改配置下轮即生效）。
    pub(crate) fn provider_stream_sink(
        &self,
        context: &RequestContext,
        model: &str,
    ) -> Option<std::sync::Arc<ProviderStreamSink>> {
        let tx = self.agent_event_tx.read().clone();
        let tx = tx?;
        if !self.provider_stream_events_enabled() {
            return None;
        }
        Some(std::sync::Arc::new(ProviderStreamSink::new(
            tx,
            context.session_key.clone(),
            context.chat_id.clone(),
            model.to_string(),
        )))
    }
}
