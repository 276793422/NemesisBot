//! LLM 调用恢复环（P2-1 器官 4 自 `loop/run_loop.rs` 物理搬迁；docs/PLAN/
//! 2026-09-23_agentloop-god-object-decomposition.md §4.2 表行 4/4a-4e）。
//! 语义零变化：首呼 select 超时包装、context 压缩环（4a）、429 预算环（4b）、
//! transient 退避环（4c）、LLM post-hooks 受守卫重呼（4d）；三处全量
//! tool_defs 重建合一为 [`AgentLoop::rebuild_full_tool_defs`]（4e，落
//! tool_defs.rs，语义保持全量）。
//!
//! 器官出口约定（§4.3 对照表）：原 `events.push(..) + break/ break 'turn`
//! 的六个终局出口统一改为 `Err(终局事件)` 返回，骨架（run_llm_loop）统一
//! `push + break`——事件向量与文案逐字节不变，golden transcript 把关。
use super::prelude::*;
use super::*;

impl AgentLoop {
    /// 器官 4：首呼（cancel/estop 可打断 + 超时包装）→ 失败分类进三恢复环 →
    /// post-hooks 终裁。`Ok` = 本轮响应；`Err` = 终局事件（含「已取消」/
    /// E-STOP / 恢复环耗尽 Error / post-hook Block），由骨架 push 后结束本
    /// turn——与原控制流逐出口等价（§4.3）。
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn call_llm_with_recovery(
        &self,
        instance: &AgentInstance,
        context: &RequestContext,
        trace_id: &str,
        messages: Vec<LlmMessage>,
        tool_defs: Vec<crate::types::ToolDefinition>,
        active_model: &str,
        cancel_token: &tokio_util::sync::CancellationToken,
        voice_playback: bool,
        request_had_images: bool,
        round_start: std::time::Instant,
        st: &TurnState,
    ) -> Result<LlmResponse, AgentEvent> {
        let turns_used = st.turns_used;
        let chat_opts = &st.chat_opts;
        // Clone provider Arc so RwLock guard is dropped before .await.
        let active_provider = self.provider.read().clone();

        // 订阅急停状态——LLM 调用进行中若触发急停，能即时打断：select 命中
        // estop arm 后，chat future 被 drop → reqwest 取消在途 HTTP 请求。
        // `None`（未接线）时该 arm 永不 resolve（pending），等价于没这条 arm。
        // 注意：subscribe() 返回的是 owned Receiver（不借用 guard），所以这里
        // 拿完就能放掉 estop 的读锁。
        let mut estop_rx = self.estop.read().as_ref().map(|e| e.subscribe());
        // Use tokio::select! to allow cancellation / e-stop during the LLM call.
        // P3B 撞墙检测起点：首次调用的失败时长供 transient 重试链比对。
        let first_call_start = std::time::Instant::now();
        // ②A：首捕同样包超时——挂死连接不再无限等（cancel/estop 臂
        // 仍然可打断超时等待，语义只增不减）。
        let first_call_timeout = self.current_provider_call_timeout_secs();
        let chat_result = tokio::select! {
            result = Self::chat_call_bounded(
                first_call_timeout,
                &active_provider,
                active_model,
                messages,
                Some(chat_opts.clone()),
                tool_defs,
            ) => result,
            _ = cancel_token.cancelled() => {
                info!("[AgentLoop] LLM call cancelled while waiting for response, turns_used={}", turns_used);
                return Err(AgentEvent::Done("已取消".to_string()));
            }
            // 急停：watch 翻成 engaged 才 resolve。（K1b 起该等待逻辑抽成
            // wait_estop_engaged 共享——hook 重呼路径用同一段，防漂移。）
            _ = Self::wait_estop_engaged(estop_rx.as_mut()) => {
                info!(
                    "[AgentLoop] E-stop engaged during LLM call, turns_used={}",
                    turns_used
                );
                return Err(AgentEvent::Done(
                    "⛔ 已急停 (E-STOP) — LLM 调用已中断。发送 `nemesisbot estop --release` 恢复。"
                        .to_string(),
                ));
            }
        };

        let response = match chat_result {
            Ok(resp) => resp,
            Err(err) => {
                // 首次调用从发起到失败的时长（Err 臂入口即失败点）。
                let first_call_failed_after = first_call_start.elapsed();
                let err_lower = err.to_lowercase();
                let is_context_error = ["token", "context", "length", "invalid"]
                    .iter()
                    .any(|keyword| err_lower.contains(keyword));

                if is_context_error {
                    // 器官 4a：context 压缩重试环（recovery.rs 方法）。
                    match self
                        .retry_after_context_error(
                            instance,
                            context,
                            trace_id,
                            active_model,
                            chat_opts,
                            voice_playback,
                            request_had_images,
                            turns_used,
                            round_start,
                            &active_provider,
                            err,
                        )
                        .await
                    {
                        Ok(resp) => resp,
                        Err(ev) => return Err(ev),
                    }
                } else {
                    // ③ Transient-error retry (network / stream / 5xx). Retries
                    // do NOT consume turns_used — the per-iteration increment
                    // below happens once regardless of how many retries it took
                    // to get a successful response. Messages + tool_defs are
                    // rebuilt fresh because the first-attempt values were moved
                    // into the failed call.
                    let is_transient_error = [
                        "timeout",
                        "timed out",
                        "connection reset",
                        "broken pipe",
                        "connect error",
                        "connection refused",
                        "temporarily unavailable",
                        "reset by peer",
                        "502",
                        "503",
                        "504",
                        "service unavailable",
                    ]
                    .iter()
                    .any(|k| err_lower.contains(k));

                    // ③a 429 限流重试环（2026-09-17 BUG 文档裁决④⑧）：
                    // 分类词表对齐 provider 侧。与 context-compression 环
                    // 互斥（限流文案不含 token/context/length/invalid），
                    // 与 transient 词表不重叠——三环不互吞。
                    let is_rate_limit_error = RATE_LIMIT_ERROR_KEYWORDS
                        .iter()
                        .any(|k| err_lower.contains(k));

                    let (maybe_resp, last_err) = if is_rate_limit_error {
                        self.retry_rate_limited(
                            context,
                            instance,
                            &active_provider,
                            active_model,
                            chat_opts,
                            err,
                        )
                        .await
                    } else if is_transient_error {
                        self.retry_transient(
                            instance,
                            &active_provider,
                            active_model,
                            chat_opts,
                            first_call_failed_after,
                            err,
                        )
                        .await
                    } else {
                        (None, err.clone())
                    };
                    if let Some(resp) = maybe_resp {
                        resp
                    } else {
                        // Non-transient error, or transient retries exhausted.
                        warn!("[AgentLoop] LLM call failed: {}", last_err);
                        let error_round = turns_used + 1;
                        let error_duration = round_start.elapsed();
                        self.emit_observer_sync(crate::loop_executor::ObserverEvent::LlmResponse {
                            trace_id: trace_id.to_string(),
                            round: error_round,
                            duration_ms: error_duration.as_millis() as u64,
                            has_tool_calls: false,
                            content: format!("Error: {}", last_err),
                            tool_calls: vec![],
                            tool_calls_count: 0,
                            finish_reason: Some("error".to_string()),
                            usage: None,
                            raw_request_body: None,
                            raw_response_body: None,
                        })
                        .await;
                        instance.add_assistant_message(
                            &format!("Error: {}", last_err),
                            Vec::new(),
                            None,
                        );
                        // [capture] Non-transient error or transient retries
                        // exhausted. Flush full last_err + trace_id.
                        if let Some(sink) = crate::capture_sink::CaptureSink::global() {
                            sink.flush(
                                &context.session_key,
                                "llm_call_failed",
                                Some(trace_id),
                                Some(last_err.as_str()),
                            );
                        }
                        // T10（多模态 D4 ③）：同上——提示只进用户可见
                        // 文案，last_err 原文继续走 history / observer /
                        // capture。
                        let formatted = context.format_rpc_message(&append_vision_fallback_hint(
                            format!("Error: {}", last_err),
                            request_had_images,
                        ));
                        return Err(AgentEvent::Error(formatted));
                    }
                }
            }
        };

        // 器官 4d：post-hooks 终裁（原 982-1123 搬入）。
        self.run_llm_post_hooks_guarded(
            instance,
            context,
            trace_id,
            active_model,
            &active_provider,
            chat_opts,
            cancel_token,
            turns_used,
            response,
        )
        .await
    }

    /// 器官 4a：context 压缩重试环（原 run_loop.rs 的 is_context_error 臂整体
    /// 搬入）。环内逐次 force_compression → 重建消息 → 重呼；重呼成功返回
    /// `Ok`；两次耗尽则完成原耗尽尾部（observer 错误事件 + history 落账 +
    /// capture flush + vision 兜底提示）后 `Err(Error(格式化文案))`。
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn retry_after_context_error(
        &self,
        instance: &AgentInstance,
        context: &RequestContext,
        trace_id: &str,
        active_model: &str,
        chat_opts: &crate::types::ChatOptions,
        voice_playback: bool,
        request_had_images: bool,
        turns_used: u32,
        round_start: std::time::Instant,
        active_provider: &Arc<dyn LlmProvider>,
        err: String,
    ) -> Result<LlmResponse, AgentEvent> {
        // Attempt up to 2 retries with progressive history compression.
        let mut retry_count = 0u32;
        let max_retries = 2u32;
        let mut retry_err = err.clone();
        let mut got_response = None;

        // Notify user about compression.
        info!(
            "[AgentLoop] LLM context error, attempting compression and retry: {}",
            retry_err
        );

        while retry_count < max_retries {
            retry_count += 1;

            // Force-compress: advance the summary cache (tail → SMALL_K_FORCE,
            // then → 0 on a second pass). History is not mutated.
            self.force_compression(instance).await;

            // Rebuild messages from compressed history.
            let mut compressed_messages = self.build_messages(instance);

            // Re-apply voice playback prompt after compression.
            if voice_playback
                && let Some(last_user) = compressed_messages
                    .iter_mut()
                    .rev()
                    .find(|m| m.role == "user")
            {
                last_user.content.push_str("（语音播报模式已开启，请用简洁、便于口语播报的方式回复，避免使用代码块、表格等不适合语音的内容。）");
            }
            debug!(
                "[AgentLoop] Retry {}: sending {} messages after compression",
                retry_count,
                compressed_messages.len()
            );

            let retry_tool_defs = self.rebuild_full_tool_defs();

            // ②A：压缩重试环内调用同样包超时（挂死连接有限
            // 重试后终局，不无限挂）。
            match Self::chat_call_bounded(
                self.current_provider_call_timeout_secs(),
                active_provider,
                active_model,
                compressed_messages,
                Some(chat_opts.clone()),
                retry_tool_defs,
            )
            .await
            {
                Ok(resp) => {
                    got_response = Some(resp);
                    break;
                }
                Err(e) => {
                    retry_err = e;
                    warn!(
                        "[AgentLoop] LLM retry {} failed: {}",
                        retry_count, retry_err
                    );
                }
            }
        }

        match got_response {
            Some(resp) => Ok(resp),
            None => {
                warn!("[AgentLoop] All LLM retries exhausted: {}", retry_err);
                let error_round = turns_used + 1;
                let error_duration = round_start.elapsed();
                self.emit_observer_sync(crate::loop_executor::ObserverEvent::LlmResponse {
                    trace_id: trace_id.to_string(),
                    round: error_round,
                    duration_ms: error_duration.as_millis() as u64,
                    has_tool_calls: false,
                    content: format!("Error: {}", retry_err),
                    tool_calls: vec![],
                    tool_calls_count: 0,
                    finish_reason: Some("error".to_string()),
                    usage: None,
                    raw_request_body: None,
                    raw_response_body: None,
                })
                .await;
                instance.add_assistant_message(&format!("Error: {}", retry_err), Vec::new(), None);
                // [capture] LLM retries exhausted (context-error
                // retry path). Flush the full retry_err — the raw
                // provider error, likely source of the user-visible
                // "tools" wording — plus trace_id to correlate with
                // request_logs/' now-complete failed round (组1).
                if let Some(sink) = crate::capture_sink::CaptureSink::global() {
                    sink.flush(
                        &context.session_key,
                        "llm_retry_exhausted",
                        Some(trace_id),
                        Some(retry_err.as_str()),
                    );
                }
                // T10（多模态 D4 ③）：兜底提示只进用户可见
                // 文案；retry_err 原文继续走 history /
                // observer / capture（诊断不掺提示）。
                let formatted = context.format_rpc_message(&append_vision_fallback_hint(
                    format!("Error: {}", retry_err),
                    request_had_images,
                ));
                Err(AgentEvent::Error(formatted))
            }
        }
    }

    /// 器官 4b：429 限流预算环（原 is_rate_limit_error 臂整体搬入）。梯子 +
    /// Retry-After 取 max + 显式进度 + 实时快照 + 预算硬上限（②B）；调用全
    /// 部包超时（②A）。成功返回 `(Some(resp), 最终 last_err)`；次数/预算
    /// 耗尽返回 `(None, 终局文案)`（裁决④「终局诚实」结构化标注，文案
    /// 逐字节保持）。
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn retry_rate_limited(
        &self,
        context: &RequestContext,
        instance: &AgentInstance,
        active_provider: &Arc<dyn LlmProvider>,
        active_model: &str,
        chat_opts: &crate::types::ChatOptions,
        err: String,
    ) -> (Option<LlmResponse>, String) {
        let mut last_err = err;
        let mut maybe_resp: Option<LlmResponse> = None;
        let max_retries = self.current_rate_limit_retries().max(0) as u32;
        let mut retries = 0u32;
        // BUG 2026-09-21 ②B：总预算（等待 + 调用累计）——
        // 0 = 不限；②A：单次调用超时——0 = 不设。
        let budget_secs = self.current_rate_limit_budget_secs();
        let call_timeout = self.current_provider_call_timeout_secs();
        // tokio 时钟（非 std Instant）：sleep/timeout 都走
        // tokio 定时器，预算必须同源——否则 start_paused
        // 测试下预算永不推进，真实场景语义不变。
        let budget_started = tokio::time::Instant::now();
        let mut budget_exhausted = false;
        while retries < max_retries {
            // ②B：进入下一轮等待+调用前先看预算——预算是
            // 硬上限，超了直接终局（ sleep 也要算进去，
            // 所以检查放循环头顶部）。
            if budget_secs > 0
                && budget_started.elapsed() >= std::time::Duration::from_secs(budget_secs)
            {
                budget_exhausted = true;
                warn!(
                    "[AgentLoop] rate-limit retry budget exhausted ({}s), giving up",
                    budget_secs
                );
                break;
            }
            retries += 1;
            let wait = rate_limit_wait_secs(&last_err, retries);
            // 裁决③「显式进度」：每次重试对用户可见；B 端
            // outbound_tx 未装配 → 静默重试。
            self.send_retry_progress(
                context,
                format!(
                    "⏳ 上游限流（{}），第 {retries}/{max_retries} 次重试，等待 {wait} 秒…",
                    self.current_display_model()
                ),
            )
            .await;
            info!(
                "[AgentLoop] LLM rate limited, retry {retries}/{max_retries} in {wait}s: {}",
                last_err
            );
            // BUG 2026-09-21 ①：实时快照（WSAPI
            // agent.retry_status）——切走切回后前端占位区
            // 可查「第 N/M 次重试」，不再是哑转圈。
            self.set_rate_limit_status(
                &context.session_key,
                RateLimitStatus {
                    retry: retries,
                    max_retries,
                    wait_secs: wait,
                    model: self.current_display_model(),
                    updated_at: std::time::Instant::now(),
                },
            );
            tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
            // 消息 + tool_defs 重建（首捕值已随失败调用
            // move，同 transient 环纪律）。
            let r_msgs = self.build_messages(instance);
            let r_tools = self.rebuild_full_tool_defs();
            // BUG 2026-09-21 ②A：单次调用包超时（统一走
            // chat_call_bounded）——上游 429 后连接挂起
            // （不回响应体）时不再无限等待（实测挂死 27
            // 分钟）；超时按该次重试失败计，走既有环
            // （次数/预算双重有界）。
            let call_result = Self::chat_call_bounded(
                call_timeout,
                active_provider,
                active_model,
                r_msgs,
                Some(chat_opts.clone()),
                r_tools,
            )
            .await;
            match call_result {
                Ok(resp) => {
                    maybe_resp = Some(resp);
                    break;
                }
                Err(e) => {
                    last_err = e;
                    warn!(
                        "[AgentLoop] rate-limit retry {retries}/{max_retries} failed: {}",
                        last_err
                    );
                }
            }
        }
        // BUG 2026-09-21 ①：环出口统一清快照（成功 break /
        // 次数耗尽 / 预算终局三路都过这里；E-STOP abort
        // 路径由读侧过期兜底）。
        self.clear_rate_limit_status(&context.session_key);
        if maybe_resp.is_none() {
            // 裁决④「终局诚实」：结构化标注，不再裸抛
            // 原始错误。
            last_err = if budget_exhausted {
                format!(
                    "上游限流，重试总时长超过 {budget_secs} 秒预算（已重试 {retries} 次），停止重试"
                )
            } else {
                format!("上游限流，已重试 {retries} 次：{}", last_err)
            };
        }
        (maybe_resp, last_err)
    }

    /// 器官 4c：transient 退避环（原 is_transient_error 臂整体搬入）。P3B
    /// 撞墙检测 + 退避资格（<1s 立即拒绝不退避，≥1s 按 1s/2s/4s 退避）+
    /// 调用包超时。返回 `(Option, 最终 last_err)`。
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn retry_transient(
        &self,
        instance: &AgentInstance,
        active_provider: &Arc<dyn LlmProvider>,
        active_model: &str,
        chat_opts: &crate::types::ChatOptions,
        first_call_failed_after: std::time::Duration,
        err: String,
    ) -> (Option<LlmResponse>, String) {
        let mut last_err = err;
        let mut maybe_resp: Option<LlmResponse> = None;
        info!(
            "[AgentLoop] LLM transient error, retrying up to {} times: {}",
            MAX_TRANSIENT_RETRIES, last_err
        );
        // P3B 撞墙检测（2026-09-12）：prev 带入首次调用的失败
        // 时长——相邻两次失败几乎同时长（±1s）= 大概率固定
        // 超时阈值拦截（provider timeout / 上游网关 / CC
        // Switch 类代理的固定超时），继续盲重试只是重复烧墙。
        // 双端真机 S2 实证：评审 LLM 连续 4 次精确 120.01s
        // 超时——根因 anthropic lane 默认 120s（已另行根修
        // 为全 lane 600s + per-model timeout_secs 覆盖）。
        let mut prev_fail_after = Some(first_call_failed_after);
        let mut retries = 0u32;
        // ②A：transient 环内调用同样包超时——挂死的
        // 「真网络等待」形态不再把有限重试环变成无限挂。
        let call_timeout = self.current_provider_call_timeout_secs();
        // 退避资格（2026-09-18 CI 实调）：失败耗时 <1s =
        // 上游**立即**拒绝（连接拒绝 / DNS / mock 类）——
        // sleep 无意义（门已关上，等待解决不了），立即重试；
        // ≥1s = 真网络等待（上游在挣扎），按 1s/2s/4s 退避
        // 才有效。不设门槛的副作用实锤：integration
        // gateway/concurrent 5 会话共享 legacy session 串行
        // 处理，mock 立即失败链从 40ms 膨胀到 8s，5×8s 推出
        // 30s 窗口（首挂实录；历史 3 连 PASS）。与 P3B 撞墙
        // 检测同哲学：立即失败的重试无效信号更早暴露。
        let mut backoff_eligible = first_call_failed_after >= std::time::Duration::from_secs(1);
        while retries < MAX_TRANSIENT_RETRIES {
            retries += 1;
            // 429 文档关联隐患 1（可重试类统一设计）：transient
            // 环对真网络等待加 1s/2s/4s 小退避——此前无 sleep
            // 立即重发，对上游抖动基本无效。立即拒绝（见
            // backoff_eligible）不退避。测试经 tokio
            // start_paused 自动推进，零等待。
            if backoff_eligible {
                tokio::time::sleep(std::time::Duration::from_secs(1u64 << (retries - 1).min(2)))
                    .await;
            }
            let r_msgs = self.build_messages(instance);
            let r_tools = self.rebuild_full_tool_defs();
            let attempt_start = std::time::Instant::now();
            match Self::chat_call_bounded(
                call_timeout,
                active_provider,
                active_model,
                r_msgs,
                Some(chat_opts.clone()),
                r_tools,
            )
            .await
            {
                Ok(resp) => {
                    maybe_resp = Some(resp);
                    break;
                }
                Err(e) => {
                    last_err = e;
                    let failed_after = attempt_start.elapsed();
                    // 下一轮退避资格按本次实际耗时重判
                    // （立即拒绝 ↔ 真等待可能在重试间切换）。
                    backoff_eligible = failed_after >= std::time::Duration::from_secs(1);
                    if let Some(prev) = prev_fail_after
                        && prev.abs_diff(failed_after) <= std::time::Duration::from_secs(1)
                    {
                        warn!(
                            "[AgentLoop] ⚠ 疑似固定超时阈值拦截：相邻两次失败时长几乎相同（前次 {:.1}s / 本次 {:.1}s），重试大概率无效——检查模型条目 timeout_secs / provider 超时与上游可用性，而不是继续重试。最后错误: {}",
                            prev.as_secs_f32(),
                            failed_after.as_secs_f32(),
                            last_err
                        );
                    }
                    prev_fail_after = Some(failed_after);
                    warn!(
                        "[AgentLoop] transient retry {}/{} failed: {}",
                        retries, MAX_TRANSIENT_RETRIES, last_err
                    );
                }
            }
        }
        (maybe_resp, last_err)
    }

    /// 器官 4d：LLM post-hooks 受守卫终裁（原 run_loop.rs 982-1123 搬入）。
    /// Allow 采纳终稿；Block → `Err(Done(HOOK BLOCKED…))`；Retry 预算内重建
    /// 消息（# Hook feedback 注入）+ 全量 defs 折叠重呼，重呼 cancel/estop
    /// 臂与首呼同构（`Err`）；重呼失败 fail-open 保留原响应。
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn run_llm_post_hooks_guarded(
        &self,
        instance: &AgentInstance,
        context: &RequestContext,
        trace_id: &str,
        active_model: &str,
        active_provider: &Arc<dyn LlmProvider>,
        chat_opts: &crate::types::ChatOptions,
        cancel_token: &tokio_util::sync::CancellationToken,
        turns_used: u32,
        mut response: LlmResponse,
    ) -> Result<LlmResponse, AgentEvent> {
        let llm_hooks = self.hooks.llm_hooks.read().snapshot();
        if !llm_hooks.is_empty() {
            let mut hook_retries: u32 = 0;
            loop {
                let hook_call = crate::hooks::HookLlmCall {
                    model: active_model.to_string(),
                    session_key: context.session_key.clone(),
                    round: turns_used as usize + 1,
                };
                // Pass a clone: on fail-open paths (budget exhausted /
                // retry call failed) `response` still holds the
                // original and needs no restore.
                match crate::hooks::run_llm_post_hooks(&llm_hooks, &hook_call, response.clone())
                    .await
                {
                    crate::hooks::PostLlmOutcome::Allow(final_resp) => {
                        response = final_resp;
                        break;
                    }
                    crate::hooks::PostLlmOutcome::Block { reason } => {
                        warn!(
                            "[AgentLoop] LLM hook blocked the response, turns_used={}: {}",
                            turns_used, reason
                        );
                        return Err(AgentEvent::Done(format!(
                            "⛔ HOOK BLOCKED [layer:hook|policy:llm_hook] {} — A registered LLM hook terminated this round. Inform the user.",
                            reason
                        )));
                    }
                    crate::hooks::PostLlmOutcome::Retry { reason } => {
                        if hook_retries >= crate::hooks::MAX_LLM_HOOK_RETRIES {
                            warn!(
                                "[AgentLoop] LLM hook retry budget exhausted ({}), allowing the previous response",
                                crate::hooks::MAX_LLM_HOOK_RETRIES
                            );
                            break;
                        }
                        hook_retries += 1;
                        // Regenerate: rebuild messages (the first
                        // attempt's were moved into the call), append
                        // the hook feedback, re-call under the same
                        // cancel/e-stop guard.
                        let mut r_msgs = self.build_messages(instance);
                        r_msgs.push(LlmMessage {
                            role: "system".to_string(),
                            content: format!("# Hook feedback: {}", reason),
                            tool_calls: None,
                            tool_call_id: None,
                            reasoning_content: None,
                            images: Vec::new(),
                        });
                        // Y1 (Phase4-a): fold the retry call's defs with
                        // the same gates/rendering as the main call —
                        // same query ⇒ same fold bytes.
                        let r_tools = self.effective_tool_defs(instance);
                        self.emit_observer_sync(crate::loop_executor::ObserverEvent::LlmRequest {
                            trace_id: trace_id.to_string(),
                            round: turns_used + 1,
                            model: active_model.to_string(),
                            messages_count: r_msgs.len(),
                            tools_count: r_tools.len(),
                            messages: observer_msg_values(&r_msgs),
                            tools: r_tools
                                .iter()
                                .filter_map(|t| serde_json::to_value(t).ok())
                                .collect(),
                            provider_name: String::new(),
                            api_key: String::new(),
                            api_base: String::new(),
                        })
                        .await;
                        let mut r_estop_rx = self.estop.read().as_ref().map(|e| e.subscribe());
                        // ②A：hook 重呼同样包超时（cancel/estop 臂不
                        // 变，仍可打断超时等待）。
                        let r = tokio::select! {
                            res = Self::chat_call_bounded(
                                self.current_provider_call_timeout_secs(),
                                active_provider,
                                active_model,
                                r_msgs,
                                Some(chat_opts.clone()),
                                r_tools,
                            ) => res,
                            _ = cancel_token.cancelled() => {
                                info!("[AgentLoop] Hook retry call cancelled, turns_used={}", turns_used);
                                return Err(AgentEvent::Done("已取消".to_string()));
                            }
                            _ = Self::wait_estop_engaged(r_estop_rx.as_mut()) => {
                                info!(
                                    "[AgentLoop] E-stop engaged during hook retry call, turns_used={}",
                                    turns_used
                                );
                                return Err(AgentEvent::Done(
                                    "⛔ 已急停 (E-STOP) — LLM 调用已中断。发送 `nemesisbot estop --release` 恢复。"
                                        .to_string(),
                                ));
                            }
                        };
                        match r {
                            Ok(resp) => {
                                info!(
                                    "[AgentLoop] Hook retry {} succeeded, re-checking response",
                                    hook_retries
                                );
                                response = resp;
                                continue;
                            }
                            Err(e) => {
                                // Fail-open: keep the response the hook
                                // rejected — a failed re-call must not
                                // lose the round's only answer.
                                warn!(
                                    "[AgentLoop] Hook retry call failed, keeping previous response: {}",
                                    e
                                );
                                break;
                            }
                        }
                    }
                }
            }
        }
        Ok(response)
    }
}
